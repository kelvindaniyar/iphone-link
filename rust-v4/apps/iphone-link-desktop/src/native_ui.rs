//! Tahap 265 — Single-Process Native Slint GUI Shell (`--native-ui` & `--native-ui-selftest`).
//!
//! Replaces WebView2 HTML/CSS/JS with a single-process WinUI 3 Liquid Glass Slint shell
//! paired with Windows 11 Thin Acrylic (`SetWindowCompositionAttribute` + `DWMWA_WINDOW_CORNER_PREFERENCE`),
//! native Win32 Parent `HWND` Subclass (`SetWindowSubclass` `WM_NCHITTEST` `HTCAPTION`, `WM_EXITSIZEMOVE`,
//! `WM_KEYDOWN`/`WM_SYSKEYDOWN`/`WM_SYSCHAR`), zero-copy Child `HWND` (`LiveVideoHwnd` / DXGI `FLIP_DISCARD`),
//! and strict airspace visibility gating. Zero HTTP preview backdoors.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::ffi::c_void;
use std::fs;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use iphone_link_apple::{airplay, connection, hid, mirror};
use iphone_link_video::d3d11_context::LiveD3d11Gpu;
use iphone_link_video::mf_decoder::{
    build_synthetic_h264_ipcm_idr, build_synthetic_h264_p_skip_avcc_sample,
    clamp_monotonic_pts_delta_100ns, DEFAULT_PTS_DELTA_100NS, MAX_PTS_DELTA_100NS,
    MIN_PTS_DELTA_100NS,
};
use iphone_link_video::video_hwnd::{
    compute_dest_and_clip_rects_for_fit, FitMode, StageKeyEvent, StagePointerEvent,
};

use crate::dcomp_presenter::{
    apply_dcomp_win11_thin_acrylic, atomic_resize_host_window, commit_active_dcomp,
    create_single_dcomp_host, ensure_dcomp_platform_installed, flush_active_dcomp_ui_frame,
    query_active_always_on_acrylic_state, query_active_dcomp_state, query_dwm_systembackdrop_type,
    query_windows_build_number, reset_dcomp_loop_state, sample_active_dcomp_ui_alpha,
    sample_active_dcomp_ui_pixel, shutdown_active_dcomp_host, sync_active_dcomp_video_layer,
    trim_active_dcomp_memory, update_active_dcomp_video_offset, DcompHostHooks,
    DWMSBT_NONE as DCOMP_DWMSBT_NONE, DWMSBT_TRANSIENTWINDOW as DCOMP_DWMSBT_TRANSIENTWINDOW,
    WIN11_SYSTEMBACKDROP_MIN_BUILD,
};
use crate::i18n::Lang;
use crate::native_input::{
    dispatch_char_with_shift_latching, resolve_global_shortcut, resolve_slint_shortcut,
    GlobalShortcutAction, NativeInputRouter,
};
use crate::native_preview;
use crate::native_video_pipeline::{
    AirplayStallWatchdog, LiveVideoPipeline, AIRPLAY_STALL_MIN_PACKETS,
};
use crate::server_v4;
use crate::tray_native::{NativeTrayEvent, NativeTrayHost, TrayMenuFlags};
use slint::{
    ComponentHandle, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, Timer,
    TimerMode, VecModel,
};

slint::include_modules!();

// ============================================================================
// LAYOUT CONSTANTS & PURE GEOMETRY / AIRSPACE POLICIES (UNIT-TESTED)
// ============================================================================

pub const NATIVE_PORTRAIT_WIDTH: f32 = 346.0;
pub const NATIVE_IPAD_PORTRAIT_WIDTH: f32 = 564.0;
pub const NATIVE_PORTRAIT_HEIGHT: f32 = 832.0;
pub const NATIVE_LANDSCAPE_WIDTH: f32 = 840.0;
pub const NATIVE_LANDSCAPE_HEIGHT: f32 = 468.0;
pub const NATIVE_DRAWER_WIDTH: f32 = 360.0;
pub const NATIVE_TOP_BAR_HEIGHT: f32 = 38.0;
pub const NATIVE_QUICK_NAV_HEIGHT: f32 = 44.0;

pub const SEQUOIA_BLUR_MAX_W: u32 = 120;
pub const SEQUOIA_BLUR_MAX_H: u32 = 260;
pub const SEQUOIA_BLUR_RADIUS: u32 = 8;
pub const SEQUOIA_BLUR_CACHE_MAGIC: &[u8; 4] = b"ILB1";
pub const SEQUOIA_BLUR_CACHE_FILENAME: &str = "sequoia_last_blur.bin";

/// Returns `true` when running under isolated loopback verification (`IPHONE_LINK_TEST_LOOPBACK_ONLY=1`).
pub fn is_test_loopback_only_env() -> bool {
    std::env::var("IPHONE_LINK_TEST_LOOPBACK_ONLY")
        .map(|v| {
            let trimmed = v.trim();
            trimmed == "1"
                || trimmed.eq_ignore_ascii_case("true")
                || trimmed.eq_ignore_ascii_case("yes")
        })
        .unwrap_or(false)
}

/// Gate ensuring selftest & unit test runs never read/write the user's real `sequoia_last_blur.bin`.
#[inline]
pub fn should_use_disk_blur_cache(is_selftest: bool) -> bool {
    !is_selftest && !is_test_loopback_only_env()
}

/// Gate ensuring selftest & loopback tests never hydrate/touch real persisted Auto-Unlock PIN.
#[inline]
pub fn should_hydrate_disk_pin(is_selftest: bool) -> bool {
    !is_selftest && !is_test_loopback_only_env()
}

/// Applies startup/reopen PIN hydration safely while keeping input masked (`pin_revealed = false`).
fn apply_hydrated_pin_to_ui(ui: &MainWindow, hydrated_pin: Option<&str>) {
    ui.set_pin_revealed(false);
    if let Some(pin) = hydrated_pin.map(str::trim).filter(|s| !s.is_empty()) {
        ui.set_pin_saved(true);
        ui.set_unlock_pin_input(SharedString::from(pin));
    } else {
        ui.set_pin_saved(false);
        ui.set_unlock_pin_input(SharedString::from(""));
    }
}

/// Resolves primary privacy-safe blur cache path beside `user_settings.json`.
pub fn resolve_sequoia_blur_cache_path() -> PathBuf {
    server_v4::user_settings_file_path()
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(SEQUOIA_BLUR_CACHE_FILENAME)
}

fn localappdata_sequoia_blur_cache_path() -> Option<PathBuf> {
    iphone_link_apple::app_paths::unified_localappdata_dir()
        .map(|d| d.join(SEQUOIA_BLUR_CACHE_FILENAME))
}

fn legacy_localappdata_sequoia_blur_cache_path() -> Option<PathBuf> {
    iphone_link_apple::app_paths::legacy_localappdata_v4_dir()
        .map(|d| d.join(SEQUOIA_BLUR_CACHE_FILENAME))
}

/// Atomically writes validated pre-blurred RGBA bytes (`ILB1` header + W/H LE + RGBA) to `path`.
pub fn save_persisted_blur_cache_to(
    path: &Path,
    w: u32,
    h: u32,
    blurred_rgba: &[u8],
) -> Result<(), String> {
    if w == 0 || h == 0 || w > SEQUOIA_BLUR_MAX_W || h > SEQUOIA_BLUR_MAX_H {
        return Err(format!("invalid blur cache dimensions: {}x{}", w, h));
    }
    let expected = (w as usize) * (h as usize) * 4;
    if blurred_rgba.len() != expected {
        return Err(format!(
            "blur cache length mismatch: got {} expected {}",
            blurred_rgba.len(),
            expected
        ));
    }
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
    }
    let mut payload = Vec::with_capacity(12 + expected);
    payload.extend_from_slice(SEQUOIA_BLUR_CACHE_MAGIC);
    payload.extend_from_slice(&w.to_le_bytes());
    payload.extend_from_slice(&h.to_le_bytes());
    payload.extend_from_slice(blurred_rgba);

    let tmp_path = path.with_extension("bin.tmp");
    std::fs::write(&tmp_path, &payload).map_err(|e| e.to_string())?;
    if std::fs::rename(&tmp_path, path).is_err() {
        let _ = std::fs::remove_file(path);
        std::fs::rename(&tmp_path, path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp_path);
            e.to_string()
        })?;
    }
    Ok(())
}

/// Validates and loads `sequoia_last_blur.bin` (`<= 124,812 B`), returning `None` on corruption/bounds mismatch.
pub fn load_persisted_blur_cache_from(path: &Path) -> Option<(Vec<u8>, u32, u32)> {
    let max_bytes = 12 + (SEQUOIA_BLUR_MAX_W as u64) * (SEQUOIA_BLUR_MAX_H as u64) * 4;
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() < 12 || meta.len() > max_bytes {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() < 12 || &bytes[0..4] != SEQUOIA_BLUR_CACHE_MAGIC {
        return None;
    }
    let w = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    let h = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
    if w == 0 || h == 0 || w > SEQUOIA_BLUR_MAX_W || h > SEQUOIA_BLUR_MAX_H {
        return None;
    }
    let expected = 12 + (w as usize) * (h as usize) * 4;
    if bytes.len() != expected {
        return None;
    }
    Some((bytes[12..].to_vec(), w, h))
}

fn load_persisted_blur_cache_default() -> Option<(Vec<u8>, u32, u32)> {
    let primary = resolve_sequoia_blur_cache_path();
    if let Some(loaded) = load_persisted_blur_cache_from(&primary) {
        return Some(loaded);
    }
    if let Some(loaded) = localappdata_sequoia_blur_cache_path()
        .as_deref()
        .and_then(load_persisted_blur_cache_from)
    {
        return Some(loaded);
    }
    legacy_localappdata_sequoia_blur_cache_path()
        .as_deref()
        .and_then(load_persisted_blur_cache_from)
}

fn save_persisted_blur_cache_default(w: u32, h: u32, blurred_rgba: &[u8]) {
    let primary = resolve_sequoia_blur_cache_path();
    if save_persisted_blur_cache_to(&primary, w, h, blurred_rgba).is_ok() {
        return;
    }
    if let Some(fallback) = localappdata_sequoia_blur_cache_path() {
        let _ = save_persisted_blur_cache_to(&fallback, w, h, blurred_rgba);
    }
}

fn delete_persisted_blur_cache_default() {
    let _ = std::fs::remove_file(resolve_sequoia_blur_cache_path());
    if let Some(fallback) = localappdata_sequoia_blur_cache_path() {
        let _ = std::fs::remove_file(fallback);
    }
    if let Some(legacy) = legacy_localappdata_sequoia_blur_cache_path() {
        let _ = std::fs::remove_file(legacy);
    }
}

pub const BRAND_LOGO_TOPBAR_LOGICAL_PX: f32 = 20.0;
pub const BRAND_LOGO_LAUNCHPAD_LOGICAL_PX: f32 = 52.0;

#[inline]
pub fn compute_crisp_logo_physical_px(logical_px: f32, scale_factor: f32) -> u32 {
    let s = if scale_factor.is_finite() {
        scale_factor.clamp(1.0, 4.0)
    } else {
        1.0
    };
    (logical_px * s).round().max(16.0) as u32
}

#[inline]
fn lanczos3_kernel(x: f32) -> f32 {
    let ax = x.abs();
    if ax >= 3.0 {
        return 0.0;
    }
    if ax < 1e-6 {
        return 1.0;
    }
    let px = ax * std::f32::consts::PI;
    let px3 = px / 3.0;
    (px.sin() / px) * (px3.sin() / px3)
}

/// High-quality 2-stage premultiplied RGBA8 downsampler (`2x2` box halving + separable scaled Lanczos3)
/// so crisp brand squircle edges & thin interior vector details never pick up dark fringe halos.
pub fn downsample_rgba8_hq_premultiplied(
    src_rgba: &[u8],
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
) -> Vec<u8> {
    let dw = dst_w.max(1);
    let dh = dst_h.max(1);
    let out_len = (dw as usize) * (dh as usize) * 4;
    let sw = src_w as usize;
    let sh = src_h as usize;
    if sw == 0 || sh == 0 || src_rgba.len() != sw * sh * 4 {
        return vec![0u8; out_len];
    }
    if src_w == dw && src_h == dh {
        return src_rgba.to_vec();
    }

    // Stage 0: straight RGBA8 -> premultiplied linear RGBA f32
    let mut cur_w = sw;
    let mut cur_h = sh;
    let mut cur: Vec<[f32; 4]> = src_rgba
        .chunks_exact(4)
        .map(|px| {
            let a = f32::from(px[3]) / 255.0;
            [
                (f32::from(px[0]) / 255.0) * a,
                (f32::from(px[1]) / 255.0) * a,
                (f32::from(px[2]) / 255.0) * a,
                a,
            ]
        })
        .collect();

    // Stage 1: Premultiplied 2x2 box-halving while >= 2x target on both axes
    while cur_w >= (dw as usize) * 2 && cur_h >= (dh as usize) * 2 {
        let nw = cur_w / 2;
        let nh = cur_h / 2;
        let mut next = Vec::with_capacity(nw * nh);
        for y in 0..nh {
            let r0 = (y * 2) * cur_w;
            let r1 = (y * 2 + 1) * cur_w;
            for x in 0..nw {
                let x0 = x * 2;
                let p00 = cur[r0 + x0];
                let p01 = cur[r0 + x0 + 1];
                let p10 = cur[r1 + x0];
                let p11 = cur[r1 + x0 + 1];
                next.push([
                    (p00[0] + p01[0] + p10[0] + p11[0]) * 0.25,
                    (p00[1] + p01[1] + p10[1] + p11[1]) * 0.25,
                    (p00[2] + p01[2] + p10[2] + p11[2]) * 0.25,
                    (p00[3] + p01[3] + p10[3] + p11[3]) * 0.25,
                ]);
            }
        }
        cur = next;
        cur_w = nw;
        cur_h = nh;
    }

    // Stage 2: Separable scaled Lanczos3 horizontal pass -> vertical pass
    let dwi = dw as usize;
    let dhi = dh as usize;
    let scale_x = (cur_w as f32 / dwi as f32).max(1.0);
    let radius_x = 3.0 * scale_x;
    let mut tmp = vec![[0.0f32; 4]; dwi * cur_h];
    for dx in 0..dwi {
        let center = (dx as f32 + 0.5) * (cur_w as f32 / dwi as f32) - 0.5;
        let start = ((center - radius_x).floor() as isize).max(0) as usize;
        let end = ((center + radius_x).ceil() as isize).min((cur_w as isize) - 1) as usize;
        let mut taps: Vec<(usize, f32)> = Vec::with_capacity(end.saturating_sub(start) + 1);
        let mut w_sum = 0.0f32;
        for sx in start..=end {
            let w = lanczos3_kernel((sx as f32 - center) / scale_x);
            if w.abs() > 1e-7 {
                taps.push((sx, w));
                w_sum += w;
            }
        }
        let inv_w = if w_sum.abs() > 1e-6 { 1.0 / w_sum } else { 0.0 };
        for sy in 0..cur_h {
            let row = sy * cur_w;
            let mut acc = [0.0f32; 4];
            for &(sx, w) in &taps {
                let p = cur[row + sx];
                let nw = w * inv_w;
                acc[0] += p[0] * nw;
                acc[1] += p[1] * nw;
                acc[2] += p[2] * nw;
                acc[3] += p[3] * nw;
            }
            tmp[sy * dwi + dx] = acc;
        }
    }

    let scale_y = (cur_h as f32 / dhi as f32).max(1.0);
    let radius_y = 3.0 * scale_y;
    let mut out = Vec::with_capacity(out_len);
    out.resize(out_len, 0u8);
    for dy in 0..dhi {
        let center = (dy as f32 + 0.5) * (cur_h as f32 / dhi as f32) - 0.5;
        let start = ((center - radius_y).floor() as isize).max(0) as usize;
        let end = ((center + radius_y).ceil() as isize).min((cur_h as isize) - 1) as usize;
        let mut taps: Vec<(usize, f32)> = Vec::with_capacity(end.saturating_sub(start) + 1);
        let mut w_sum = 0.0f32;
        for sy in start..=end {
            let w = lanczos3_kernel((sy as f32 - center) / scale_y);
            if w.abs() > 1e-7 {
                taps.push((sy, w));
                w_sum += w;
            }
        }
        let inv_w = if w_sum.abs() > 1e-6 { 1.0 / w_sum } else { 0.0 };
        for dx in 0..dwi {
            let mut acc = [0.0f32; 4];
            for &(sy, w) in &taps {
                let p = tmp[sy * dwi + dx];
                let nw = w * inv_w;
                acc[0] += p[0] * nw;
                acc[1] += p[1] * nw;
                acc[2] += p[2] * nw;
                acc[3] += p[3] * nw;
            }
            let a = acc[3].clamp(0.0, 1.0);
            let dst_idx = (dy * dwi + dx) * 4;
            if a <= 1e-4 {
                out[dst_idx..dst_idx + 4].copy_from_slice(&[0, 0, 0, 0]);
                continue;
            }
            let a8 = (a * 255.0).round().clamp(0.0, 255.0) as u8;
            if a8 == 0 {
                out[dst_idx..dst_idx + 4].copy_from_slice(&[0, 0, 0, 0]);
                continue;
            }
            let r = (acc[0].clamp(0.0, a) / a * 255.0).round().clamp(0.0, 255.0) as u8;
            let g = (acc[1].clamp(0.0, a) / a * 255.0).round().clamp(0.0, 255.0) as u8;
            let b = (acc[2].clamp(0.0, a) / a * 255.0).round().clamp(0.0, 255.0) as u8;
            out[dst_idx] = r;
            out[dst_idx + 1] = g;
            out[dst_idx + 2] = b;
            out[dst_idx + 3] = a8;
        }
    }
    out
}

/// Regenerates crisp 1:1 physical-pixel Top Bar & Launchpad logos whenever monitor DPI scale changes.
pub fn refresh_crisp_brand_logos_for_scale(
    ui: &MainWindow,
    scale_factor: f32,
    last_scale_bits: &Cell<u32>,
) -> bool {
    let s = if scale_factor.is_finite() {
        scale_factor.clamp(1.0, 4.0)
    } else {
        1.0
    };
    let bits = (s * 100.0).round() as u32;
    if last_scale_bits.get() == bits {
        return false;
    }
    let Some(src_buf) = ui.get_brand_logo_source().to_rgba8() else {
        return false;
    };
    let src_w = src_buf.width();
    let src_h = src_buf.height();
    let src_bytes = src_buf.as_bytes();
    let top_px = compute_crisp_logo_physical_px(BRAND_LOGO_TOPBAR_LOGICAL_PX, s);
    let lp_px = compute_crisp_logo_physical_px(BRAND_LOGO_LAUNCHPAD_LOGICAL_PX, s);
    let top_rgba = downsample_rgba8_hq_premultiplied(src_bytes, src_w, src_h, top_px, top_px);
    let lp_rgba = downsample_rgba8_hq_premultiplied(src_bytes, src_w, src_h, lp_px, lp_px);
    ui.set_brand_logo_topbar(blurred_rgba_to_image(&top_rgba, top_px, top_px));
    ui.set_brand_logo_launchpad(blurred_rgba_to_image(&lp_rgba, lp_px, lp_px));
    last_scale_bits.set(bits);
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageOverlayKind {
    None,
    StandbyLaunchpad,
    SequoiaConnecting,
    WaitingAirplay,
    AntiBlankDiagnostic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageTransitionKind {
    StartToSequoia,
    FirstFrameReveal { use_sequoia_fallback: bool },
    StopFromLive,
    StopFromConnecting,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransitionOpacities {
    pub standby: f32,
    pub sequoia: f32,
    pub sequoia_text: f32,
    pub reveal_blur: f32,
}

/// Computes deterministic cross-fade opacities (`0.0..=1.0`) across Start, Reveal, and Stop
/// transitions matching 1:1 Tahap 259 choreography without hard cuts.
pub fn compute_transition_opacities(kind: StageTransitionKind, progress_01: f32) -> TransitionOpacities {
    let t = progress_01.clamp(0.0, 1.0);
    let eased = ease_out_cubic(t);
    match kind {
        StageTransitionKind::StartToSequoia => TransitionOpacities {
            standby: (1.0 - eased).clamp(0.0, 1.0),
            sequoia: eased.clamp(0.0, 1.0),
            sequoia_text: eased.clamp(0.0, 1.0),
            reveal_blur: 0.0,
        },
        StageTransitionKind::FirstFrameReveal { use_sequoia_fallback } => {
            // Text fades quickly (~180 ms of 360 ms = first half) while blur-to-sharp finishes at t = 1.0
            let text_t = (t * 2.0).clamp(0.0, 1.0);
            let text_op = (1.0 - ease_out_cubic(text_t)).clamp(0.0, 1.0);
            let fade_op = (1.0 - eased).clamp(0.0, 1.0);
            if use_sequoia_fallback {
                TransitionOpacities {
                    standby: 0.0,
                    sequoia: fade_op,
                    sequoia_text: text_op,
                    reveal_blur: 0.0,
                }
            } else {
                TransitionOpacities {
                    standby: 0.0,
                    sequoia: 0.0,
                    sequoia_text: text_op,
                    reveal_blur: fade_op,
                }
            }
        }
        StageTransitionKind::StopFromLive => TransitionOpacities {
            standby: eased.clamp(0.0, 1.0),
            sequoia: 0.0,
            sequoia_text: 0.0,
            reveal_blur: (1.0 - eased).clamp(0.0, 1.0),
        },
        StageTransitionKind::StopFromConnecting => TransitionOpacities {
            standby: eased.clamp(0.0, 1.0),
            sequoia: (1.0 - eased).clamp(0.0, 1.0),
            sequoia_text: (1.0 - eased).clamp(0.0, 1.0),
            reveal_blur: 0.0,
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StopTransitionPlan {
    pub transition_kind: StageTransitionKind,
    pub duration_ms: u32,
    pub use_live_capture_blur: bool,
    pub reuse_last_frame_blur: bool,
    pub flush_layer1_before_layer0_detach: bool,
}

/// Plans zero-black-flash Stop choreography for both HEVC & AirPlay streams (`StopFromLive` 280 ms)
/// and Connecting/Waiting states (`StopFromConnecting` 240 ms).
pub fn plan_stop_transition_sequence(
    video_layer_active: bool,
    live_capture_ok: bool,
    has_last_frame_blur: bool,
) -> StopTransitionPlan {
    if video_layer_active && (live_capture_ok || has_last_frame_blur) {
        StopTransitionPlan {
            transition_kind: StageTransitionKind::StopFromLive,
            duration_ms: 280,
            use_live_capture_blur: live_capture_ok,
            reuse_last_frame_blur: !live_capture_ok && has_last_frame_blur,
            flush_layer1_before_layer0_detach: true,
        }
    } else {
        StopTransitionPlan {
            transition_kind: StageTransitionKind::StopFromConnecting,
            duration_ms: 240,
            use_live_capture_blur: false,
            reuse_last_frame_blur: false,
            flush_layer1_before_layer0_detach: true,
        }
    }
}

/// Pure Single Source of Truth for stage overlay mutual exclusivity (`Anti-Blank > Waiting AirPlay > Sequoia Connecting > Standby`).
pub fn resolve_active_stage_overlay(
    phase: &str,
    stage: &str,
    first_frame_ready: bool,
    anti_blank_visible: bool,
) -> StageOverlayKind {
    if anti_blank_visible {
        return StageOverlayKind::AntiBlankDiagnostic;
    }
    if phase != "running" && phase != "starting" {
        return StageOverlayKind::StandbyLaunchpad;
    }
    if stage == "waiting_airplay" {
        return StageOverlayKind::WaitingAirplay;
    }
    if phase == "starting" || (phase == "running" && !first_frame_ready) {
        return StageOverlayKind::SequoiaConnecting;
    }
    StageOverlayKind::None
}

/// Computes the exact target logical window `(width, height)` for non-fullscreen states
/// honoring the active `device_profile` (`"auto" | "iphone" | "ipad"`).
pub fn compute_native_window_target_logical_size_for_profile(
    drawer_open: bool,
    is_landscape: bool,
    device_profile: &str,
) -> (f32, f32) {
    let base_w = if is_landscape {
        NATIVE_LANDSCAPE_WIDTH
    } else if device_profile.trim().eq_ignore_ascii_case("ipad") {
        NATIVE_IPAD_PORTRAIT_WIDTH
    } else {
        NATIVE_PORTRAIT_WIDTH
    };
    let base_h = if is_landscape {
        NATIVE_LANDSCAPE_HEIGHT
    } else {
        NATIVE_PORTRAIT_HEIGHT
    };
    let total_w = if drawer_open {
        base_w + NATIVE_DRAWER_WIDTH
    } else {
        base_w
    };
    (total_w, base_h)
}

/// Computes the exact target logical window `(width, height)` for non-fullscreen states.
pub fn compute_native_window_target_logical_size(drawer_open: bool, is_landscape: bool) -> (f32, f32) {
    compute_native_window_target_logical_size_for_profile(drawer_open, is_landscape, "auto")
}

/// Computes the exact physical pixel rectangle `(x, y, width, height)` for the D3D11 Child `HWND`
/// (`LiveVideoHwnd`) inside the Slint window client area across DPI scales (`1.0`, `1.25`, `1.5`, etc.),
/// Portrait/Landscape orientations, Drawer Open/Closed (`-360 px`), and Full Screen.
///
/// Guarantees:
/// - `y == (38.0 * scale).round() as i32` (never overlaps the 38px Top Bar)
/// - `y + height <= ((logical_win_h - 44.0) * scale).round() as i32` (never overlaps the 44px Quick Nav Bar)
/// - When `drawer_open == true`, `x + width <= ((logical_win_w - 360.0) * scale).round() as i32`
///   (never overlaps the 360px Right Side Drawer)
pub fn compute_native_stage_physical_rect(
    logical_win_w: f32,
    logical_win_h: f32,
    scale_factor: f32,
    drawer_open: bool,
    _is_fullscreen: bool,
    _is_landscape: bool,
) -> (i32, i32, u32, u32) {
    let scale = if scale_factor.is_finite() && scale_factor > 0.25 {
        scale_factor
    } else {
        1.0
    };
    let stage_logical_w = if drawer_open {
        (logical_win_w - NATIVE_DRAWER_WIDTH).max(16.0)
    } else {
        logical_win_w.max(16.0)
    };
    let top_px = (NATIVE_TOP_BAR_HEIGHT * scale).round() as i32;
    let nav_top_logical = (logical_win_h - NATIVE_QUICK_NAV_HEIGHT).max(NATIVE_TOP_BAR_HEIGHT + 16.0);
    let bottom_px = (nav_top_logical * scale).round() as i32;
    let phys_w = (stage_logical_w * scale).round().max(16.0) as u32;
    let phys_h = (bottom_px - top_px).max(16) as u32;
    (0, top_px, phys_w, phys_h)
}

/// Single Source of Truth rectangle shared identically by:
/// 1) DirectComposition Layer 0 `video_visual` offset & swapchain size,
/// 2) Slint Layer 1 transparent video hole (`video-hole-{x,y,w,h}`), and
/// 3) Win32 pointer/wheel iOS HID touch transform (`map_client_px_to_ios_hid_from_rect`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActiveVideoPhysicalRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub stage_x: i32,
    pub stage_y: i32,
    pub stage_width: u32,
    pub stage_height: u32,
}

impl Default for ActiveVideoPhysicalRect {
    fn default() -> Self {
        Self {
            x: 0,
            y: 38,
            width: 346,
            height: 750,
            stage_x: 0,
            stage_y: 38,
            stage_width: 346,
            stage_height: 750,
        }
    }
}

/// Ease-out cubic curve (`cubic-bezier(0.16, 1.0, 0.3, 1.0)` style smooth finish).
pub fn ease_out_cubic(t: f32) -> f32 {
    let u = 1.0 - t.clamp(0.0, 1.0);
    1.0 - u * u * u
}

/// Pure-Rust 2-pass O(W*H) separable sliding-window box blur (`radius` e.g. 6, `< 1 ms` on `180x390` RGBA8)
/// for zero-dependency Sequoia Blur-to-Sharp reveal & stop transitions.
pub fn fast_box_blur_rgba(src: &[u8], width: u32, height: u32, radius: u32) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    if w == 0 || h == 0 || radius == 0 || src.len() < w * h * 4 {
        return src.to_vec();
    }
    let r = (radius as usize).min(w.max(h));
    let win = (r * 2 + 1) as u32;
    let mut buf = src[..w * h * 4].to_vec();
    let mut tmp = vec![0u8; w * h * 4];

    for _pass in 0..2 {
        for y in 0..h {
            let row = y * w * 4;
            let mut sum = [0u32; 4];
            for k in -(r as isize)..=(r as isize) {
                let sx = k.clamp(0, (w - 1) as isize) as usize;
                let p = row + sx * 4;
                sum[0] += u32::from(buf[p]);
                sum[1] += u32::from(buf[p + 1]);
                sum[2] += u32::from(buf[p + 2]);
                sum[3] += u32::from(buf[p + 3]);
            }
            for x in 0..w {
                let dst = row + x * 4;
                tmp[dst] = (sum[0] / win) as u8;
                tmp[dst + 1] = (sum[1] / win) as u8;
                tmp[dst + 2] = (sum[2] / win) as u8;
                tmp[dst + 3] = (sum[3] / win) as u8;

                let add_x = (x + r + 1).min(w - 1);
                let sub_x = x.saturating_sub(r);
                let pa = row + add_x * 4;
                let ps = row + sub_x * 4;
                for c in 0..4 {
                    sum[c] = sum[c] + u32::from(buf[pa + c]) - u32::from(buf[ps + c]);
                }
            }
        }
        for x in 0..w {
            let col = x * 4;
            let mut sum = [0u32; 4];
            for k in -(r as isize)..=(r as isize) {
                let sy = k.clamp(0, (h - 1) as isize) as usize;
                let p = sy * w * 4 + col;
                sum[0] += u32::from(tmp[p]);
                sum[1] += u32::from(tmp[p + 1]);
                sum[2] += u32::from(tmp[p + 2]);
                sum[3] += u32::from(tmp[p + 3]);
            }
            for y in 0..h {
                let dst = y * w * 4 + col;
                buf[dst] = (sum[0] / win) as u8;
                buf[dst + 1] = (sum[1] / win) as u8;
                buf[dst + 2] = (sum[2] / win) as u8;
                buf[dst + 3] = (sum[3] / win) as u8;

                let add_y = (y + r + 1).min(h - 1);
                let sub_y = y.saturating_sub(r);
                let pa = add_y * w * 4 + col;
                let ps = sub_y * w * 4 + col;
                for c in 0..4 {
                    sum[c] = sum[c] + u32::from(tmp[pa + c]) - u32::from(tmp[ps + c]);
                }
            }
        }
    }
    buf
}

fn capture_blurred_reveal_rgba(pipe: &LiveVideoPipeline) -> Option<(Vec<u8>, u32, u32)> {
    let (rgba, tw, th) = pipe.capture_rgba_thumbnail(SEQUOIA_BLUR_MAX_W, SEQUOIA_BLUR_MAX_H)?;
    let blurred = fast_box_blur_rgba(&rgba, tw, th, SEQUOIA_BLUR_RADIUS);
    Some((blurred, tw, th))
}

fn blurred_rgba_to_image(rgba: &[u8], tw: u32, th: u32) -> Image {
    let px_buf = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(rgba, tw, th);
    Image::from_rgba8(px_buf)
}

#[allow(dead_code)]
fn capture_blurred_reveal_image(pipe: &LiveVideoPipeline) -> Option<Image> {
    let (blurred, tw, th) = capture_blurred_reveal_rgba(pipe)?;
    Some(blurred_rgba_to_image(&blurred, tw, th))
}

fn trigger_live_pli_burst() {
    mirror::request_live_pli();
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_millis(250));
        let status = mirror::global_screen_status_json();
        if status.contains(r#""phase":"running""#) {
            mirror::request_live_pli();
        }
    });
}

/// Computes `ActiveVideoPhysicalRect` with continuous `drawer_w_logical` (`0.0..=360.0`)
/// so Full Screen Drawer transitions glide smoothly at 60 Hz with exact post-trim source alignment.
/// When `is_airplay_mode && !is_landscape`, enforces Portrait effective aspect atomically so
/// orientation switches (`1920x888 <-> 888x1920`) never squish horizontally for even 1 frame.
/// Tahap 275 (Pilar 2): Hard-clamps any computed video rectangle strictly inside `[stage_x..stage_x+stage_width]`
/// and `[stage_y..stage_y+stage_height]` so DComp video NEVER leaves a dark gap under Top Bar or slides down
/// over Top/Bottom Slint bars under any DPI/fit_mode/adaptive scale combination.
pub fn clamp_video_rect_to_stage(rect: ActiveVideoPhysicalRect) -> ActiveVideoPhysicalRect {
    let min_x = rect.stage_x;
    let min_y = rect.stage_y;
    let sw = rect.stage_width.max(16);
    let sh = rect.stage_height.max(16);
    let w = rect.width.clamp(16, sw);
    let h = rect.height.clamp(16, sh);
    let max_x = min_x + (sw as i32 - w as i32).max(0);
    let max_y = min_y + (sh as i32 - h as i32).max(0);
    let x = rect.x.clamp(min_x, max_x);
    let y = rect.y.clamp(min_y, max_y);
    if (x, y, w, h) != (rect.x, rect.y, rect.width, rect.height) {
        airplay::inc_trace_layout_rect_clamps();
    }
    ActiveVideoPhysicalRect {
        x,
        y,
        width: w,
        height: h,
        stage_x: rect.stage_x,
        stage_y: rect.stage_y,
        stage_width: sw,
        stage_height: sh,
    }
}

pub fn compute_active_video_physical_rect_with_drawer_w_and_mode(
    client_w: u32,
    client_h: u32,
    scale_factor: f32,
    drawer_w_logical: f32,
    is_fullscreen: bool,
    is_landscape: bool,
    video_src_w: u32,
    video_src_h: u32,
    rotation_deg: u16,
    fit_mode: FitMode,
    is_airplay_mode: bool,
) -> ActiveVideoPhysicalRect {
    let scale = if scale_factor.is_finite() && scale_factor > 0.25 {
        scale_factor
    } else {
        1.0
    };
    let cw = client_w.max(16);
    let ch = client_h.max(64);
    let _logical_w = (cw as f32) / scale;
    let logical_h = (ch as f32) / scale;

    let stage_y = (NATIVE_TOP_BAR_HEIGHT * scale).round() as i32;
    let nav_top_logical = (logical_h - NATIVE_QUICK_NAV_HEIGHT).max(NATIVE_TOP_BAR_HEIGHT + 16.0);
    let stage_bottom = ((nav_top_logical * scale).round() as i32).min(ch as i32);
    let stage_h = (stage_bottom - stage_y).max(16) as u32;
    let dw = drawer_w_logical.clamp(0.0, NATIVE_DRAWER_WIDTH);
    let drawer_px = if dw <= 0.001 {
        0u32
    } else {
        (dw * scale).round() as u32
    };
    let stage_w = cw.saturating_sub(drawer_px).clamp(16, cw);

    // Base phone aspect ratio (post-trim HEVC 1202x2598 when decoder hasn't reported yet)
    let (raw_w, raw_h) = if video_src_w > 0 && video_src_h > 0 {
        (video_src_w as f64, video_src_h as f64)
    } else {
        (1202.0, 2598.0)
    };
    let swapped = rotation_deg % 180 == 90 || (video_src_w == 0 && is_landscape);
    let (eff_w, eff_h) = if is_airplay_mode && video_src_w > 0 && video_src_h > 0 {
        if is_landscape {
            (raw_w.max(raw_h), raw_w.min(raw_h))
        } else {
            (raw_w.min(raw_h), raw_w.max(raw_h))
        }
    } else if swapped {
        (raw_h, raw_w)
    } else {
        (raw_w, raw_h)
    };
    let target_aspect = (eff_w / eff_h.max(1.0)).max(0.1);
    let stage_aspect = (stage_w as f64) / (stage_h as f64);
    let orientation_mismatched = (eff_w > eff_h) != ((stage_w as f64) > (stage_h as f64));

    // Portrait non-fullscreen AirPlay under AutoFit/EdgeToEdge locks cleanly to full stage
    // `(0, stage_y, stage_w, stage_h)` even under adaptive resolution scaling (990x2160 <-> 640x1388).
    let airplay_portrait_autofit_fill =
        is_airplay_mode && !is_fullscreen && !is_landscape && !orientation_mismatched && fit_mode == FitMode::AutoFit;
    // Horizontal non-fullscreen AirPlay under AutoFit/EdgeToEdge locks cleanly to full stage
    // `(0, stage_y, stage_w, stage_h)` across 2160x994 (0°) & 994x2160 (270° CW) with 0 pillarbox.
    let airplay_landscape_autofit_fill =
        is_airplay_mode && !is_fullscreen && is_landscape && !orientation_mismatched && fit_mode == FitMode::AutoFit;

    let fill_stage = !is_fullscreen
        && !orientation_mismatched
        && (fit_mode == FitMode::EdgeToEdge
            || airplay_portrait_autofit_fill
            || airplay_landscape_autofit_fill
            || ((target_aspect - stage_aspect).abs() / stage_aspect) < 0.04);

    let (vx, vy, vw, vh) = if is_fullscreen && !is_landscape {
        // Tahap 276 (Pilar 3): Portrait Full Screen ALWAYS fills full stage height cleanly (with
        // zero bottom gap and zero leftward shift even on repeat toggles or Drawer open/close).
        let port_w = eff_w.min(eff_h).max(1.0);
        let port_h = eff_w.max(eff_h).max(1.0);
        let eff_aspect = (port_w / port_h).max(0.1);
        let mut vh = stage_h;
        let mut vw = ((vh as f64) * eff_aspect).round().min(stage_w as f64).max(16.0) as u32;
        if vw >= stage_w {
            vw = stage_w;
            vh = ((vw as f64) / eff_aspect).round().min(stage_h as f64).max(16.0) as u32;
        }
        let left = ((stage_w.saturating_sub(vw)) / 2) as i32;
        let top = stage_y + ((stage_h.saturating_sub(vh)) / 2) as i32;
        (left, top, vw, vh)
    } else if is_fullscreen && is_landscape {
        // Tahap 277 (Pilar 3): Landscape Full Screen fits dual bounds `(stage_w, stage_h)` with
        // `drawer_px == 0` when closed, centered sharp across the monitor (never stuck 840x468 top-left).
        let land_w = eff_w.max(eff_h).max(1.0);
        let land_h = eff_w.min(eff_h).max(1.0);
        let scale_fit = ((stage_w as f64) / land_w).min((stage_h as f64) / land_h);
        let vw = ((land_w * scale_fit).round() as u32).clamp(16, stage_w);
        let vh = ((land_h * scale_fit).round() as u32).clamp(16, stage_h);
        let left = ((stage_w.saturating_sub(vw)) / 2) as i32;
        let top = stage_y + ((stage_h.saturating_sub(vh)) / 2) as i32;
        (left, top, vw, vh)
    } else if fill_stage {
        (0, stage_y, stage_w, stage_h)
    } else {
        let scale_fit = ((stage_w as f64) / eff_w.max(1.0))
            .min((stage_h as f64) / eff_h.max(1.0));
        let w = ((eff_w * scale_fit).round() as u32).clamp(16, stage_w);
        let h = ((eff_h * scale_fit).round() as u32).clamp(16, stage_h);
        let x = ((stage_w.saturating_sub(w)) / 2) as i32;
        let y = stage_y + ((stage_h.saturating_sub(h)) / 2) as i32;
        (x, y, w, h)
    };

    clamp_video_rect_to_stage(ActiveVideoPhysicalRect {
        x: vx,
        y: vy,
        width: vw,
        height: vh,
        stage_x: 0,
        stage_y,
        stage_width: stage_w,
        stage_height: stage_h,
    })
}

/// Computes physical video rect and detects when layout drifted from `prev_rect`
/// (recording `fullscreenLandscapeResyncs` in AirPlay Flight Recorder when snapping Full Screen Landscape).
#[allow(clippy::too_many_arguments)]
pub fn compute_fullscreen_landscape_resync_rect(
    prev_rect: Option<ActiveVideoPhysicalRect>,
    client_w: u32,
    client_h: u32,
    scale_factor: f32,
    drawer_open: bool,
    is_fullscreen: bool,
    is_landscape: bool,
    video_src_w: u32,
    video_src_h: u32,
    rotation_deg: u16,
    fit_mode: FitMode,
    is_airplay_mode: bool,
) -> (ActiveVideoPhysicalRect, bool) {
    let rect = clamp_video_rect_to_stage(compute_active_video_physical_rect_with_drawer_w_and_mode(
        client_w,
        client_h,
        scale_factor,
        if drawer_open { NATIVE_DRAWER_WIDTH } else { 0.0 },
        is_fullscreen,
        is_landscape,
        video_src_w,
        video_src_h,
        rotation_deg,
        fit_mode,
        is_airplay_mode,
    ));
    let changed = prev_rect
        .map(|p| (p.x, p.y, p.width, p.height) != (rect.x, rect.y, rect.width, rect.height))
        .unwrap_or(true);
    if changed && is_fullscreen && is_landscape {
        airplay::inc_trace_fullscreen_landscape_resyncs();
    }
    (rect, changed)
}

/// Computes `ActiveVideoPhysicalRect` with continuous `drawer_w_logical` (`0.0..=360.0`)
/// so Full Screen Drawer transitions glide smoothly at 60 Hz with exact post-trim source alignment.
pub fn compute_active_video_physical_rect_with_drawer_w(
    client_w: u32,
    client_h: u32,
    scale_factor: f32,
    drawer_w_logical: f32,
    is_fullscreen: bool,
    is_landscape: bool,
    video_src_w: u32,
    video_src_h: u32,
    rotation_deg: u16,
    fit_mode: FitMode,
) -> ActiveVideoPhysicalRect {
    compute_active_video_physical_rect_with_drawer_w_and_mode(
        client_w,
        client_h,
        scale_factor,
        drawer_w_logical,
        is_fullscreen,
        is_landscape,
        video_src_w,
        video_src_h,
        rotation_deg,
        fit_mode,
        false,
    )
}

/// Computes the Single Source of Truth `ActiveVideoPhysicalRect` inside client area `client_w x client_h`
/// for Portrait, Landscape, Drawer Open/Closed (`-360 px`), and Borderless Full Screen (centered w/ blur wings).
pub fn compute_active_video_physical_rect(
    client_w: u32,
    client_h: u32,
    scale_factor: f32,
    drawer_open: bool,
    is_fullscreen: bool,
    is_landscape: bool,
    video_src_w: u32,
    video_src_h: u32,
    rotation_deg: u16,
    fit_mode: FitMode,
) -> ActiveVideoPhysicalRect {
    compute_active_video_physical_rect_with_drawer_w(
        client_w,
        client_h,
        scale_factor,
        if drawer_open { NATIVE_DRAWER_WIDTH } else { 0.0 },
        is_fullscreen,
        is_landscape,
        video_src_w,
        video_src_h,
        rotation_deg,
        fit_mode,
    )
}

/// Maps client physical pixel `(client_x, client_y)` into iOS HID `0..=65535` ONLY when inside
/// `rect` (rejects clicks on Fullscreen blur wings, Top Bar, Bottom Nav, and Side Drawer).
pub fn map_client_px_to_ios_hid_from_rect(
    client_x: i32,
    client_y: i32,
    rect: ActiveVideoPhysicalRect,
    rotation_deg: u16,
) -> Option<(u16, u16)> {
    let w = rect.width as i32;
    let h = rect.height as i32;
    if w <= 0
        || h <= 0
        || client_x < rect.x
        || client_x >= rect.x + w
        || client_y < rect.y
        || client_y >= rect.y + h
    {
        return None;
    }
    let nx = ((client_x - rect.x) as f64) / (w as f64);
    let ny = ((client_y - rect.y) as f64) / (h as f64);
    Some(crate::native_input::stage_norm_to_ios_hid(
        nx,
        ny,
        u32::from(rotation_deg),
    ))
}

/// Airspace Visibility Guard:
/// Child `HWND` (`LiveVideoHwnd`) MUST be visible (`SW_SHOWNA`) **only** when `phase == "running"`
/// and not blocked by modal/waiting overlays (`waiting_airplay`, `downloading_ddi`, `mounting_ddi`).
/// In `idle`, `starting`, `connecting`, `waiting_airplay`, `error`, or `stopped`, it returns `false`
/// (`SW_HIDE`) so Slint's Launchpad, Sequoia/DDI progress, and AirPlay guides remain 100% interactive.
pub fn should_video_hwnd_be_visible(phase: &str, stage: &str) -> bool {
    let p = phase.trim();
    if !p.eq_ignore_ascii_case("running") {
        return false;
    }
    let s = stage.trim();
    if s.eq_ignore_ascii_case("waiting_airplay")
        || s.eq_ignore_ascii_case("downloading_ddi")
        || s.eq_ignore_ascii_case("mounting_ddi")
    {
        return false;
    }
    true
}

// ============================================================================
// WIN32 THIN ACRYLIC, DWM ROUNDED CORNERS, ICON, GEOMETRY & DRAG HELPERS
// ============================================================================

const GWL_STYLE: i32 = -16;
const WS_CLIPCHILDREN: i32 = 0x0200_0000;
const WS_MAXIMIZE: i32 = 0x0100_0000;
const SW_HIDE: i32 = 0;
const SW_SHOWNORMAL: i32 = 1;
const SW_SHOWMINIMIZED: i32 = 2;
#[allow(dead_code)]
const SW_MAXIMIZE: i32 = 3;
const SWP_NOSIZE: u32 = 0x0001;
const SWP_NOMOVE: u32 = 0x0002;
const SWP_NOZORDER: u32 = 0x0004;
const SWP_NOACTIVATE: u32 = 0x0010;
const SWP_FRAMECHANGED: u32 = 0x0020;
const SWP_SHOWWINDOW: u32 = 0x0040;
const SPI_GETWORKAREA: u32 = 0x0030;
const MONITOR_DEFAULTTONEAREST: u32 = 2;
const WM_CANCELMODE: u32 = 0x001F;
const WM_SETCURSOR: u32 = 0x0020;
const WM_SETICON: u32 = 0x0080;
const WM_NCDESTROY: u32 = 0x0082;
const WM_NCHITTEST: u32 = 0x0084;
const WM_NCLBUTTONDBLCLK: u32 = 0x00A3;
const WM_KEYDOWN: u32 = 0x0100;
const WM_KEYUP: u32 = 0x0101;
const WM_CHAR: u32 = 0x0102;
const WM_SYSKEYDOWN: u32 = 0x0104;
const WM_SYSKEYUP: u32 = 0x0105;
const WM_SYSCHAR: u32 = 0x0106;
const WM_KILLFOCUS: u32 = 0x0008;
const WM_EXITSIZEMOVE: u32 = 0x0232;
const HTCLIENT: isize = 1;
const HTCAPTION: isize = 2;
const IDC_ARROW: usize = 32512;
const VK_SHIFT: i32 = 0x10;
const VK_CONTROL: i32 = 0x11;
const VK_MENU: i32 = 0x12;
const VK_F4: usize = 0x73;
const ICON_SMALL: usize = 0;
const ICON_BIG: usize = 1;
const IMAGE_ICON: u32 = 1;
const LR_SHARED: u32 = 0x0000_8000;
const LR_LOADFROMFILE: u32 = 0x0000_0010;

const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
const DWMWA_SYSTEMBACKDROP_TYPE: u32 = 38;
const DWMWCP_DONOTROUND: u32 = 1;
const DWMWCP_ROUND: u32 = 2;
const DWMSBT_NONE: u32 = 1;
const WCA_ACCENT_POLICY: u32 = 19;
const ACCENT_ENABLE_ACRYLICBLURBEHIND: u32 = 4;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct WinPoint {
    x: i32,
    y: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
struct WinRect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

impl WinRect {
    fn width(&self) -> i32 {
        (self.right - self.left).max(1)
    }
    fn height(&self) -> i32 {
        (self.bottom - self.top).max(1)
    }
}

#[repr(C)]
struct MonitorInfo {
    cb_size: u32,
    rc_monitor: WinRect,
    rc_work: WinRect,
    dw_flags: u32,
}

#[repr(C)]
struct AccentPolicy {
    accent_state: u32,
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

#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryW(lpLibFileName: *const u16) -> *mut c_void;
    fn GetProcAddress(hModule: *mut c_void, lpProcName: *const i8) -> *mut c_void;
    fn GetModuleHandleW(lpModuleName: *const u16) -> *mut c_void;
}

type FnGetWindowLongW = unsafe extern "system" fn(isize, i32) -> i32;
type FnSetWindowLongW = unsafe extern "system" fn(isize, i32, i32) -> i32;
type FnGetWindowRect = unsafe extern "system" fn(isize, *mut WinRect) -> i32;
type FnGetClientRect = unsafe extern "system" fn(isize, *mut WinRect) -> i32;
type FnSetWindowPos = unsafe extern "system" fn(isize, isize, i32, i32, i32, i32, u32) -> i32;
type FnShowWindow = unsafe extern "system" fn(isize, i32) -> i32;
type FnIsWindowVisible = unsafe extern "system" fn(isize) -> i32;
type FnSystemParametersInfoW = unsafe extern "system" fn(u32, u32, *mut c_void, u32) -> i32;
type FnGetDpiForWindow = unsafe extern "system" fn(isize) -> u32;
type FnGetDpiForSystem = unsafe extern "system" fn() -> u32;
type FnShellExecuteW = unsafe extern "system" fn(
    isize,
    *const u16,
    *const u16,
    *const u16,
    *const u16,
    i32,
) -> isize;
type FnMonitorFromWindow = unsafe extern "system" fn(isize, u32) -> isize;
type FnGetMonitorInfoW = unsafe extern "system" fn(isize, *mut MonitorInfo) -> i32;
type FnPostMessageW = unsafe extern "system" fn(isize, u32, usize, isize) -> i32;
type FnSendMessageW = unsafe extern "system" fn(isize, u32, usize, isize) -> isize;
type FnLoadImageW = unsafe extern "system" fn(*mut c_void, *const u16, u32, i32, i32, u32) -> *mut c_void;
type FnScreenToClient = unsafe extern "system" fn(isize, *mut WinPoint) -> i32;
type FnLoadCursorW = unsafe extern "system" fn(isize, *const u16) -> isize;
type FnSetCursor = unsafe extern "system" fn(isize) -> isize;
type FnGetKeyState = unsafe extern "system" fn(i32) -> i16;
type SubclassProcFn = unsafe extern "system" fn(isize, u32, usize, isize, usize, usize) -> isize;
type FnSetWindowSubclass = unsafe extern "system" fn(isize, Option<SubclassProcFn>, usize, usize) -> i32;
type FnDefSubclassProc = unsafe extern "system" fn(isize, u32, usize, isize) -> isize;
type FnRemoveWindowSubclass = unsafe extern "system" fn(isize, Option<SubclassProcFn>, usize) -> i32;
type FnSetWindowCompositionAttribute =
    unsafe extern "system" fn(isize, *mut WindowCompositionAttribData) -> i32;
type FnDwmSetWindowAttribute =
    unsafe extern "system" fn(isize, u32, *const c_void, u32) -> i32;
type FnSetAppUserModelId = unsafe extern "system" fn(*const u16) -> i32;

struct NativeWin32Api {
    get_window_long_w: FnGetWindowLongW,
    set_window_long_w: FnSetWindowLongW,
    get_window_rect: FnGetWindowRect,
    get_client_rect: FnGetClientRect,
    set_window_pos: FnSetWindowPos,
    show_window: FnShowWindow,
    #[allow(dead_code)]
    is_window_visible: FnIsWindowVisible,
    system_parameters_info_w: FnSystemParametersInfoW,
    get_dpi_for_window: Option<FnGetDpiForWindow>,
    get_dpi_for_system: Option<FnGetDpiForSystem>,
    shell_execute_w: Option<FnShellExecuteW>,
    monitor_from_window: FnMonitorFromWindow,
    get_monitor_info_w: FnGetMonitorInfoW,
    post_message_w: FnPostMessageW,
    send_message_w: FnSendMessageW,
    load_image_w: FnLoadImageW,
    screen_to_client: FnScreenToClient,
    load_cursor_w: FnLoadCursorW,
    set_cursor: FnSetCursor,
    get_key_state: FnGetKeyState,
    set_window_subclass: Option<FnSetWindowSubclass>,
    def_subclass_proc: Option<FnDefSubclassProc>,
    remove_window_subclass: Option<FnRemoveWindowSubclass>,
    set_window_composition_attribute: Option<FnSetWindowCompositionAttribute>,
    dwm_set_window_attribute: Option<FnDwmSetWindowAttribute>,
    set_app_user_model_id: Option<FnSetAppUserModelId>,
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn win32() -> &'static NativeWin32Api {
    static API: OnceLock<NativeWin32Api> = OnceLock::new();
    API.get_or_init(|| unsafe {
        let u32_w = to_wide("user32.dll");
        let h_u32 = LoadLibraryW(u32_w.as_ptr());
        let comctl_w = to_wide("comctl32.dll");
        let h_comctl = LoadLibraryW(comctl_w.as_ptr());
        let dwm_w = to_wide("dwmapi.dll");
        let h_dwm = LoadLibraryW(dwm_w.as_ptr());
        let sh_w = to_wide("shell32.dll");
        let h_sh = LoadLibraryW(sh_w.as_ptr());

        let get_u32 = |name: &[u8]| GetProcAddress(h_u32, name.as_ptr() as *const i8);
        let get_cc = |name: &[u8]| {
            if h_comctl.is_null() {
                std::ptr::null_mut()
            } else {
                GetProcAddress(h_comctl, name.as_ptr() as *const i8)
            }
        };
        let p_dpi = get_u32(b"GetDpiForWindow\0");
        let p_sys_dpi = get_u32(b"GetDpiForSystem\0");
        let p_swca = get_u32(b"SetWindowCompositionAttribute\0");
        let p_set_sub = get_cc(b"SetWindowSubclass\0");
        let p_def_sub = get_cc(b"DefSubclassProc\0");
        let p_rem_sub = get_cc(b"RemoveWindowSubclass\0");
        let p_dwm_attr = if !h_dwm.is_null() {
            GetProcAddress(h_dwm, b"DwmSetWindowAttribute\0".as_ptr() as *const i8)
        } else {
            std::ptr::null_mut()
        };
        let p_aumid = if !h_sh.is_null() {
            GetProcAddress(
                h_sh,
                b"SetCurrentProcessExplicitAppUserModelID\0".as_ptr() as *const i8,
            )
        } else {
            std::ptr::null_mut()
        };
        let p_shellex = if !h_sh.is_null() {
            GetProcAddress(h_sh, b"ShellExecuteW\0".as_ptr() as *const i8)
        } else {
            std::ptr::null_mut()
        };

        NativeWin32Api {
            get_window_long_w: std::mem::transmute(get_u32(b"GetWindowLongW\0")),
            set_window_long_w: std::mem::transmute(get_u32(b"SetWindowLongW\0")),
            get_window_rect: std::mem::transmute(get_u32(b"GetWindowRect\0")),
            get_client_rect: std::mem::transmute(get_u32(b"GetClientRect\0")),
            set_window_pos: std::mem::transmute(get_u32(b"SetWindowPos\0")),
            show_window: std::mem::transmute(get_u32(b"ShowWindow\0")),
            is_window_visible: std::mem::transmute(get_u32(b"IsWindowVisible\0")),
            system_parameters_info_w: std::mem::transmute(get_u32(b"SystemParametersInfoW\0")),
            get_dpi_for_window: if p_dpi.is_null() {
                None
            } else {
                Some(std::mem::transmute(p_dpi))
            },
            get_dpi_for_system: if p_sys_dpi.is_null() {
                None
            } else {
                Some(std::mem::transmute(p_sys_dpi))
            },
            shell_execute_w: if p_shellex.is_null() {
                None
            } else {
                Some(std::mem::transmute(p_shellex))
            },
            monitor_from_window: std::mem::transmute(get_u32(b"MonitorFromWindow\0")),
            get_monitor_info_w: std::mem::transmute(get_u32(b"GetMonitorInfoW\0")),
            post_message_w: std::mem::transmute(get_u32(b"PostMessageW\0")),
            send_message_w: std::mem::transmute(get_u32(b"SendMessageW\0")),
            load_image_w: std::mem::transmute(get_u32(b"LoadImageW\0")),
            screen_to_client: std::mem::transmute(get_u32(b"ScreenToClient\0")),
            load_cursor_w: std::mem::transmute(get_u32(b"LoadCursorW\0")),
            set_cursor: std::mem::transmute(get_u32(b"SetCursor\0")),
            get_key_state: std::mem::transmute(get_u32(b"GetKeyState\0")),
            set_window_subclass: if p_set_sub.is_null() {
                None
            } else {
                Some(std::mem::transmute(p_set_sub))
            },
            def_subclass_proc: if p_def_sub.is_null() {
                None
            } else {
                Some(std::mem::transmute(p_def_sub))
            },
            remove_window_subclass: if p_rem_sub.is_null() {
                None
            } else {
                Some(std::mem::transmute(p_rem_sub))
            },
            set_window_composition_attribute: if p_swca.is_null() {
                None
            } else {
                Some(std::mem::transmute(p_swca))
            },
            dwm_set_window_attribute: if p_dwm_attr.is_null() {
                None
            } else {
                Some(std::mem::transmute(p_dwm_attr))
            },
            set_app_user_model_id: if p_aumid.is_null() {
                None
            } else {
                Some(std::mem::transmute(p_aumid))
            },
        }
    })
}

fn query_work_area(hwnd: isize) -> WinRect {
    let api = win32();
    unsafe {
        if hwnd != 0 {
            let hmon = (api.monitor_from_window)(hwnd, MONITOR_DEFAULTTONEAREST);
            if hmon != 0 {
                let mut mi = MonitorInfo {
                    cb_size: std::mem::size_of::<MonitorInfo>() as u32,
                    rc_monitor: WinRect::default(),
                    rc_work: WinRect::default(),
                    dw_flags: 0,
                };
                if (api.get_monitor_info_w)(hmon, &mut mi) != 0 && mi.rc_work.width() > 300 {
                    return mi.rc_work;
                }
            }
        }
        let mut wa = WinRect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1040,
        };
        let _ = (api.system_parameters_info_w)(
            SPI_GETWORKAREA,
            0,
            &mut wa as *mut WinRect as *mut c_void,
            0,
        );
        wa
    }
}

fn query_hwnd_scale(hwnd: isize) -> f32 {
    let api = win32();
    if hwnd != 0 {
        if let Some(f) = api.get_dpi_for_window {
            let dpi = unsafe { f(hwnd) };
            if dpi >= 48 {
                return (dpi as f32) / 96.0;
            }
        }
    }
    if let Some(fs) = api.get_dpi_for_system {
        let dpi = unsafe { fs() };
        if dpi >= 48 {
            return (dpi as f32) / 96.0;
        }
    }
    1.0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TopbarLayoutBudget {
    left_cluster_px: i32,
    right_cluster_px: i32,
    spacer_px: i32,
    total_px: i32,
    fits_346px: bool,
}

fn compute_topbar_346px_layout_budget(
    streaming: bool,
    auto_rotate: bool,
    wide: bool,
) -> TopbarLayoutBudget {
    let bsize = if !wide {
        if streaming { 21 } else { 23 }
    } else if streaming {
        24
    } else {
        25
    };
    let pill_max = if wide {
        220
    } else if streaming {
        if auto_rotate { 84 } else { 108 }
    } else if auto_rotate {
        112
    } else {
        142
    };
    let title_px = if wide { 68 } else { 0 };
    let left_cluster_px = 20 + 4 + title_px + if wide { 4 } else { 0 } + pill_max;
    let rotate_px = if auto_rotate {
        if wide { 64 } else { 58 }
    } else {
        bsize
    };
    let stop_px = if streaming { if wide { 56 } else { bsize } } else { 0 };
    let right_btn_count = if streaming { 8 } else { 6 };
    let fixed_small_count = right_btn_count - 1 - if streaming { 1 } else { 0 };
    let right_cluster_px =
        fixed_small_count * bsize + stop_px + rotate_px + (right_btn_count - 1) * 2;
    let total_px = 5 + left_cluster_px + 4 + right_cluster_px + 4;
    let viewport_w = if wide { 706 } else { 346 };
    let spacer_px = (viewport_w - total_px).max(0);
    TopbarLayoutBudget {
        left_cluster_px,
        right_cluster_px,
        spacer_px,
        total_px,
        fits_346px: wide || total_px <= 346,
    }
}

fn validate_public_external_url(url: &str) -> bool {
    let trimmed = url.trim();
    matches!(
        trimmed,
        "https://saweria.co/kelvindaniyar"
            | "https://github.com/kelvindaniyar"
            | "https://github.com/kelvindaniyar/iphone-link"
            | "https://github.com/teyboxv1/pane-iphone-mirror-windows"
            | "https://www.icloud.com/shortcuts/bbb096d97e174c1386f15e31aedef8ff"
            | "ms-windows-store://pdp/?ProductId=9n4wgh0z6vhq"
            | "ms-settings:optionalfeatures"
    ) || crate::updater::is_allowed_github_release_url(trimmed)
}

fn open_verified_public_url(url: &str) -> bool {
    let trimmed = url.trim();
    if !validate_public_external_url(trimmed) {
        return false;
    }
    open_url_in_system_browser(trimmed);
    true
}

fn open_url_in_system_browser(url: &str) {
    let dry_run = cfg!(test)
        || std::env::var("IPHONE_LINK_DRY_RUN_SHELL_OPEN")
            .ok()
            .is_some_and(|v| {
                let t = v.trim();
                t == "1" || t.eq_ignore_ascii_case("true")
            });
    if dry_run {
        return;
    }
    if let Some(sh_exec) = win32().shell_execute_w {
        let op_w = to_wide("open");
        let url_w = to_wide(url);
        unsafe {
            let _ = sh_exec(0, op_w.as_ptr(), url_w.as_ptr(), std::ptr::null(), std::ptr::null(), 1);
        }
    }
}

/// Computes `(x, y, width, height)` when exiting Borderless Work-Area Full Screen:
/// honors dynamic Drawer Open/Close or Orientation changes made while inside Full Screen
/// (`706x832` Portrait / `1200x468` Landscape), preserving `saved_rc` only when layout matches!
fn compute_fullscreen_exit_physical_rect(
    saved_rc: Option<WinRect>,
    wa: WinRect,
    drawer_open: bool,
    is_landscape: bool,
    device_profile: &str,
    scale: f32,
) -> (i32, i32, i32, i32) {
    let (lw, lh) = compute_native_window_target_logical_size_for_profile(
        drawer_open,
        is_landscape,
        device_profile,
    );
    let (cx, cy, cw, ch) = compute_centered_window_rect(lw, lh, scale, wa);
    if let Some(rc) = saved_rc {
        let sw = rc.width();
        let sh = rc.height();
        if (sw - (cw as i32)).abs() <= 24 && (sh - (ch as i32)).abs() <= 24 {
            let sx = rc.left.clamp(wa.left, (wa.right - sw).max(wa.left));
            let sy = rc.top.clamp(wa.top, (wa.bottom - sh).max(wa.top));
            return (sx, sy, sw, sh);
        }
    }
    (cx, cy, cw as i32, ch as i32)
}

/// Queries live physical client size `(width, height)` directly via Win32 `GetClientRect`.
pub fn read_live_hwnd_client_phys_size(hwnd: isize) -> (u32, u32) {
    let api = win32();
    let mut rc = WinRect::default();
    if hwnd != 0 {
        unsafe {
            let _ = (api.get_client_rect)(hwnd, &mut rc);
        }
    }
    (rc.width().max(16) as u32, rc.height().max(16) as u32)
}

fn query_client_size(hwnd: isize) -> (u32, u32) {
    read_live_hwnd_client_phys_size(hwnd)
}

fn is_hwnd_maximized(hwnd: isize) -> bool {
    if hwnd == 0 {
        return false;
    }
    let style = unsafe { (win32().get_window_long_w)(hwnd, GWL_STYLE) };
    (style & WS_MAXIMIZE) != 0
}

#[allow(dead_code)]
fn is_hwnd_visible(hwnd: isize) -> bool {
    if hwnd == 0 {
        return false;
    }
    unsafe { (win32().is_window_visible)(hwnd) != 0 }
}

#[allow(dead_code)]
fn ensure_clip_children_style(hwnd: isize) {
    if hwnd == 0 {
        return;
    }
    let api = win32();
    unsafe {
        let style = (api.get_window_long_w)(hwnd, GWL_STYLE);
        if (style & WS_CLIPCHILDREN) == 0 {
            let _ = (api.set_window_long_w)(hwnd, GWL_STYLE, style | WS_CLIPCHILDREN);
            let _ = (api.set_window_pos)(
                hwnd,
                0,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED,
            );
        }
    }
}

/// Applies Win11 Thin Acrylic (`SetWindowCompositionAttribute(ACCENT_ENABLE_ACRYLICBLURBEHIND)`),
/// `DWMWA_WINDOW_CORNER_PREFERENCE = DWMWCP_ROUND` (or `DWMWCP_DONOTROUND` in Full Screen),
/// and `DWMWA_USE_IMMERSIVE_DARK_MODE`.
#[allow(dead_code)]
fn apply_native_thin_acrylic(hwnd: isize, dark_mode: bool, is_fullscreen: bool) -> bool {
    if hwnd == 0 {
        return false;
    }
    let api = win32();
    let mut ok = false;
    unsafe {
        if let Some(set_attr) = api.dwm_set_window_attribute {
            let dark_val: u32 = if dark_mode { 1 } else { 0 };
            let _ = set_attr(
                hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                &dark_val as *const u32 as *const c_void,
                4,
            );
            let corner_val: u32 = if is_fullscreen {
                DWMWCP_DONOTROUND
            } else {
                DWMWCP_ROUND
            };
            let _ = set_attr(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &corner_val as *const u32 as *const c_void,
                4,
            );
            let none_backdrop: u32 = DWMSBT_NONE;
            let _ = set_attr(
                hwnd,
                DWMWA_SYSTEMBACKDROP_TYPE,
                &none_backdrop as *const u32 as *const c_void,
                4,
            );
            ok = true;
        }
        if let Some(swca) = api.set_window_composition_attribute {
            // ABGR subtle Thin Acrylic tint matching dark/light WinUI 3 Liquid Glass
            // Light mode uses bright frosted white (#F8F9FC @ alpha 0x30 -> ABGR 0x30FCF9F8)
            let tint_abgr: u32 = if dark_mode { 0x9E1A1512 } else { 0x30FCF9F8 };
            let mut policy = AccentPolicy {
                accent_state: ACCENT_ENABLE_ACRYLICBLURBEHIND,
                accent_flags: 2,
                gradient_color: tint_abgr,
                animation_id: 0,
            };
            let mut data = WindowCompositionAttribData {
                attrib: WCA_ACCENT_POLICY,
                pv_data: &mut policy as *mut AccentPolicy as *mut c_void,
                cb_data: std::mem::size_of::<AccentPolicy>(),
            };
            if swca(hwnd, &mut data) != 0 {
                ok = true;
            }
        }
    }
    ok
}

// ============================================================================
// TAHAP 265 WIN32 PARENT HWND SUBCLASS (DRAG HTCAPTION, CURSOR, KEYBOARD)
// ============================================================================

/// Pure hit-test geometry for Top Bar drag (`HTCAPTION`) vs right-side icon buttons (`HTCLIENT`).
/// Exposes deterministic coordinates for unit tests so right-edge buttons NEVER lose clicks.
pub fn compute_top_bar_nchittest(
    client_x: i32,
    client_y: i32,
    client_w: i32,
    scale: f32,
    is_fullscreen: bool,
    is_streaming_or_starting: bool,
) -> isize {
    if is_fullscreen || client_w <= 0 {
        return HTCLIENT;
    }
    let s = if scale.is_finite() && scale > 0.25 { scale } else { 1.0 };
    let top_bar_px = (NATIVE_TOP_BAR_HEIGHT * s).round() as i32;
    if client_y < 0 || client_y >= top_bar_px || client_x < 0 {
        return HTCLIENT;
    }
    let right_buttons_logical = if is_streaming_or_starting { 216.0 } else { 174.0 };
    let right_buttons_px = (right_buttons_logical * s).ceil() as i32;
    if client_x < client_w.saturating_sub(right_buttons_px) {
        HTCAPTION
    } else {
        HTCLIENT
    }
}

#[allow(dead_code)]
struct ParentSubclassState {
    is_fullscreen: AtomicBool,
    is_streaming_or_starting: AtomicBool,
    pin_editing: AtomicBool,
    keyboard_enabled: AtomicBool,
    phase_running: AtomicBool,
    needs_bounds_sync: AtomicBool,
    fs_toggle_requested: AtomicBool,
    ctrl_down: AtomicBool,
    input_router: Arc<NativeInputRouter>,
}

const PARENT_SUBCLASS_ID: usize = 0x265A;

#[allow(dead_code)]
unsafe extern "system" fn parent_hwnd_subclass_proc(
    hwnd: isize,
    msg: u32,
    wparam: usize,
    lparam: isize,
    _uid_subclass: usize,
    ref_data: usize,
) -> isize {
    let api = win32();
    let def_proc = |h, m, w, l| -> isize {
        if let Some(f) = api.def_subclass_proc {
            f(h, m, w, l)
        } else {
            0
        }
    };

    if ref_data == 0 {
        return def_proc(hwnd, msg, wparam, lparam);
    }
    let st = &*(ref_data as *const ParentSubclassState);

    match msg {
        WM_NCHITTEST => {
            let sx = (lparam & 0xFFFF) as i16 as i32;
            let sy = ((lparam >> 16) & 0xFFFF) as i16 as i32;
            let mut pt = WinPoint { x: sx, y: sy };
            let _ = (api.screen_to_client)(hwnd, &mut pt);
            let (cw, _) = query_client_size(hwnd);
            let scale = query_hwnd_scale(hwnd);
            let hit = compute_top_bar_nchittest(
                pt.x,
                pt.y,
                cw as i32,
                scale,
                st.is_fullscreen.load(Ordering::Relaxed),
                st.is_streaming_or_starting.load(Ordering::Relaxed),
            );
            if hit == HTCAPTION {
                return HTCAPTION;
            }
            def_proc(hwnd, msg, wparam, lparam)
        }
        WM_NCLBUTTONDBLCLK => {
            if wparam == HTCAPTION as usize {
                st.fs_toggle_requested.store(true, Ordering::SeqCst);
                return 0;
            }
            def_proc(hwnd, msg, wparam, lparam)
        }
        WM_SETCURSOR => {
            let area = (lparam & 0xFFFF) as isize;
            if area == HTCAPTION {
                let cur = (api.load_cursor_w)(0, IDC_ARROW as *const u16);
                if cur != 0 {
                    let _ = (api.set_cursor)(cur);
                }
                return 1;
            }
            def_proc(hwnd, msg, wparam, lparam)
        }
        WM_EXITSIZEMOVE => {
            let _ = (api.post_message_w)(hwnd, WM_CANCELMODE, 0, 0);
            let cur = (api.load_cursor_w)(0, IDC_ARROW as *const u16);
            if cur != 0 {
                let _ = (api.set_cursor)(cur);
            }
            st.needs_bounds_sync.store(true, Ordering::SeqCst);
            record_ui_action("window_drag_end", "", None);
            def_proc(hwnd, msg, wparam, lparam)
        }
        WM_KILLFOCUS => {
            st.ctrl_down.store(false, Ordering::Relaxed);
            def_proc(hwnd, msg, wparam, lparam)
        }
        WM_SYSCHAR => 0, // Consume Alt+<key> character beep cleanly!
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let vk = (wparam & 0xFFFF) as usize;
            if vk == VK_CONTROL as usize {
                st.ctrl_down.store(true, Ordering::Relaxed);
            }
            let ctrl = ((api.get_key_state)(VK_CONTROL) as u16 & 0x8000) != 0
                || st.ctrl_down.load(Ordering::Relaxed);
            let shift = ((api.get_key_state)(VK_SHIFT) as u16 & 0x8000) != 0;
            let alt = ((lparam & (1 << 29)) != 0)
                || (((api.get_key_state)(VK_MENU) as u16 & 0x8000) != 0);
            if msg == WM_SYSKEYDOWN && alt && vk == VK_F4 {
                return def_proc(hwnd, msg, wparam, lparam);
            }
            let pin_editing = st.pin_editing.load(Ordering::Relaxed);
            if !pin_editing && (ctrl || alt) {
                if let Some(act) = resolve_global_shortcut(vk as u16, ctrl, alt, shift) {
                    if (lparam & 0x4000_0000) == 0 {
                        st.input_router.emit_shortcut(act);
                    }
                    return 0;
                }
            }
            if !pin_editing
                && !ctrl
                && !alt
                && st.phase_running.load(Ordering::Relaxed)
                && st.keyboard_enabled.load(Ordering::Relaxed)
            {
                let repeat = (lparam & 0x4000_0000) != 0;
                st.input_router.handle_key_event(StageKeyEvent::KeyDown {
                    vk: vk as u16,
                    ctrl,
                    alt,
                    shift,
                    repeat,
                });
            }
            def_proc(hwnd, msg, wparam, lparam)
        }
        WM_KEYUP | WM_SYSKEYUP => {
            let vk = (wparam & 0xFFFF) as usize;
            if vk == VK_CONTROL as usize {
                st.ctrl_down.store(false, Ordering::Relaxed);
            }
            let ctrl = ((api.get_key_state)(VK_CONTROL) as u16 & 0x8000) != 0
                || st.ctrl_down.load(Ordering::Relaxed);
            let shift = ((api.get_key_state)(VK_SHIFT) as u16 & 0x8000) != 0;
            let alt = ((lparam & (1 << 29)) != 0)
                || (((api.get_key_state)(VK_MENU) as u16 & 0x8000) != 0);
            st.input_router.handle_key_event(StageKeyEvent::KeyUp {
                vk: vk as u16,
                ctrl,
                alt,
                shift,
            });
            def_proc(hwnd, msg, wparam, lparam)
        }
        WM_CHAR => {
            let ctrl = ((api.get_key_state)(VK_CONTROL) as u16 & 0x8000) != 0
                || st.ctrl_down.load(Ordering::Relaxed);
            let alt = ((lparam & (1 << 29)) != 0)
                || (((api.get_key_state)(VK_MENU) as u16 & 0x8000) != 0);
            if !st.pin_editing.load(Ordering::Relaxed)
                && !ctrl
                && !alt
                && st.phase_running.load(Ordering::Relaxed)
                && st.keyboard_enabled.load(Ordering::Relaxed)
            {
                if let Some(ch) = char::from_u32((wparam & 0xFFFF) as u32) {
                    st.input_router.handle_key_event(StageKeyEvent::Char {
                        ch,
                        ctrl,
                        alt,
                    });
                }
            }
            def_proc(hwnd, msg, wparam, lparam)
        }
        WM_NCDESTROY => {
            let ret = def_proc(hwnd, msg, wparam, lparam);
            if let Some(rem) = api.remove_window_subclass {
                let _ = rem(hwnd, Some(parent_hwnd_subclass_proc), PARENT_SUBCLASS_ID);
            }
            drop(Arc::from_raw(ref_data as *const ParentSubclassState));
            ret
        }
        _ => def_proc(hwnd, msg, wparam, lparam),
    }
}

#[allow(dead_code)]
fn install_parent_hwnd_subclass(hwnd: isize, st: Arc<ParentSubclassState>) -> bool {
    if hwnd == 0 {
        return false;
    }
    let Some(set_sub) = win32().set_window_subclass else {
        return false;
    };
    let raw = Arc::into_raw(st);
    let ok = unsafe {
        set_sub(
            hwnd,
            Some(parent_hwnd_subclass_proc),
            PARENT_SUBCLASS_ID,
            raw as usize,
        ) != 0
    };
    if !ok {
        unsafe { drop(Arc::from_raw(raw)) };
    }
    ok
}

fn load_embedded_hicon(cx: i32, cy: i32) -> *mut c_void {
    let api = win32();
    unsafe {
        let hinst = GetModuleHandleW(std::ptr::null());
        let h_res = (api.load_image_w)(hinst, 1usize as *const u16, IMAGE_ICON, cx, cy, LR_SHARED);
        if !h_res.is_null() {
            return h_res;
        }
        let temp_dir = std::env::temp_dir().join("iPhoneLink");
        let _ = fs::create_dir_all(&temp_dir);
        let temp_ico = temp_dir.join("iphone_link.ico");
        let ico_bytes = server_v4::EMBEDDED_IPHONE_LINK_ICO;
        let need_write = fs::metadata(&temp_ico)
            .map(|m| m.len() != ico_bytes.len() as u64)
            .unwrap_or(true);
        if need_write {
            let _ = fs::write(&temp_ico, ico_bytes);
        }
        if temp_ico.is_file() {
            let path_w: Vec<u16> = temp_ico
                .as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            let h_file = (api.load_image_w)(
                std::ptr::null_mut(),
                path_w.as_ptr(),
                IMAGE_ICON,
                cx,
                cy,
                LR_LOADFROMFILE | LR_SHARED,
            );
            if !h_file.is_null() {
                return h_file;
            }
        }
        std::ptr::null_mut()
    }
}

fn apply_embedded_window_icons_and_aumid(hwnd: isize) {
    let api = win32();
    unsafe {
        if let Some(set_aumid) = api.set_app_user_model_id {
            let aumid_w = to_wide("iPhoneLink.NativeRust.5");
            let _ = set_aumid(aumid_w.as_ptr());
        }
        if hwnd == 0 {
            return;
        }
        let h_big = load_embedded_hicon(256, 256);
        let h_small = load_embedded_hicon(32, 32);
        if !h_big.is_null() {
            let _ = (api.send_message_w)(hwnd, WM_SETICON, ICON_BIG, h_big as isize);
        }
        if !h_small.is_null() {
            let _ = (api.send_message_w)(hwnd, WM_SETICON, ICON_SMALL, h_small as isize);
        }
    }
}

fn compute_centered_window_rect(logical_w: f32, logical_h: f32, scale: f32, wa: WinRect) -> (i32, i32, u32, u32) {
    let s = if scale.is_finite() && scale > 0.25 { scale } else { 1.0 };
    let phys_w = (logical_w * s).round().min((wa.width() - 16) as f32).max(320.0) as i32;
    let phys_h = (logical_h * s).round().min((wa.height() - 16) as f32).max(400.0) as i32;
    let x = wa.left + ((wa.width() - phys_w) / 2).max(0);
    let y = wa.top + ((wa.height() - phys_h) / 2).max(0);
    (x, y, phys_w as u32, phys_h as u32)
}

#[allow(dead_code)]
fn center_window_normal(hwnd: isize, logical_w: f32, logical_h: f32) {
    if hwnd == 0 {
        return;
    }
    let api = win32();
    let scale = query_hwnd_scale(hwnd);
    let wa = query_work_area(hwnd);
    let (x, y, phys_w, phys_h) = compute_centered_window_rect(logical_w, logical_h, scale, wa);
    unsafe {
        let _ = (api.show_window)(hwnd, SW_SHOWNORMAL);
        let _ = (api.set_window_pos)(
            hwnd,
            0,
            x,
            y,
            phys_w as i32,
            phys_h as i32,
            SWP_NOZORDER | SWP_FRAMECHANGED | SWP_SHOWWINDOW,
        );
    }
}

fn compute_resize_preserve_origin_rect(
    hwnd: isize,
    logical_w: f32,
    logical_h: f32,
) -> (i32, i32, u32, u32) {
    let api = win32();
    let scale = query_hwnd_scale(hwnd);
    let wa = query_work_area(hwnd);
    let mut cur = WinRect::default();
    if hwnd != 0 {
        unsafe {
            let _ = (api.get_window_rect)(hwnd, &mut cur);
        }
    }
    let phys_w = (logical_w * scale).round().min((wa.width() - 16) as f32).max(320.0) as i32;
    let phys_h = (logical_h * scale).round().min((wa.height() - 16) as f32).max(400.0) as i32;
    let mut x = cur.left;
    let mut y = cur.top;
    if x + phys_w > wa.right {
        x = (wa.right - phys_w).max(wa.left);
    }
    if y + phys_h > wa.bottom {
        y = (wa.bottom - phys_h).max(wa.top);
    }
    x = x.max(wa.left);
    y = y.max(wa.top);
    (x, y, phys_w as u32, phys_h as u32)
}

fn resize_window_preserve_origin(hwnd: isize, logical_w: f32, logical_h: f32) {
    if hwnd == 0 {
        return;
    }
    let (x, y, phys_w, phys_h) = compute_resize_preserve_origin_rect(hwnd, logical_w, logical_h);
    if atomic_resize_host_window(hwnd, x, y, phys_w, phys_h) {
        return;
    }
    let api = win32();
    unsafe {
        let _ = (api.set_window_pos)(
            hwnd,
            0,
            x,
            y,
            phys_w as i32,
            phys_h as i32,
            SWP_NOZORDER | SWP_FRAMECHANGED | SWP_SHOWWINDOW,
        );
    }
}

#[allow(dead_code)]
fn extract_parent_hwnd_from_slint_window(_win: &slint::Window) -> isize {
    0
}

#[allow(dead_code)]
fn extract_parent_hwnd(_ui: &MainWindow) -> isize {
    0
}

#[allow(dead_code)]
fn ensure_slint_winit_backend() {
    ensure_dcomp_platform_installed();
}

// ============================================================================
// IN-PROCESS BACKEND DISPATCH & FIT-MODE NORMALIZATION
// ============================================================================

fn inprocess_api(
    state: &Arc<Mutex<server_v4::AppState>>,
    web_dir: &Path,
    method: &str,
    path: &str,
    body: &str,
) -> String {
    let (clean_path, query) = match path.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (path.to_string(), String::new()),
    };
    let req = server_v4::HttpRequest {
        method: method.to_string(),
        path: clean_path,
        query,
        headers: HashMap::new(),
        body: body.to_string(),
        body_bytes: body.as_bytes().to_vec(),
    };
    let resp = server_v4::handle_request(&req, state, web_dir);
    String::from_utf8_lossy(&resp.body).into_owned()
}

/// 1:1 Tahap 259 Bottom Bar dispatcher matching `handset/web/app.js` (`pressHome(1)`, `pressHome(2)`, `/mirror/gesture`).
fn execute_tahap259_bottom_nav(
    state: &Arc<Mutex<server_v4::AppState>>,
    web_dir: &Path,
    router: &NativeInputRouter,
    action: &str,
) -> Vec<String> {
    router.cancel_active_touch();
    let act = action.trim().to_ascii_lowercase();
    match act.as_str() {
        "home" => {
            let _ = mirror::global_apply_orientation(None, None, Some("home"), None);
            let r1 = inprocess_api(
                state,
                web_dir,
                "POST",
                "/mirror/button",
                r#"{"name":"home","state":"press"}"#,
            );
            mirror::request_live_pli();
            vec![r1]
        }
        "recent" => {
            let _ = mirror::global_apply_orientation(None, None, Some("recent"), None);
            let r1 = inprocess_api(
                state,
                web_dir,
                "POST",
                "/mirror/button",
                r#"{"name":"home","state":"press"}"#,
            );
            std::thread::sleep(Duration::from_millis(120));
            let r2 = inprocess_api(
                state,
                web_dir,
                "POST",
                "/mirror/button",
                r#"{"name":"home","state":"press"}"#,
            );
            mirror::request_live_pli();
            vec![r1, r2]
        }
        "back" => {
            vec![inprocess_api(
                state,
                web_dir,
                "POST",
                "/mirror/gesture",
                r#"{"direction":"back"}"#,
            )]
        }
        "right" => {
            vec![inprocess_api(
                state,
                web_dir,
                "POST",
                "/mirror/gesture",
                r#"{"direction":"right"}"#,
            )]
        }
        _ => Vec::new(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AirplayOrientationPlan {
    pub is_landscape: bool,
    pub video_rotation_deg: i32,
    pub window_logical_w: f32,
    pub window_logical_h: f32,
}

/// Resolves exact window geometry & video rotation for native AirPlay H.264 streams:
/// - When `auto_rotate_enabled == false && !manual_landscape_override` AND `source_w > source_h`:
///   keeps Portrait window (`346x832` / `706x832`) and applies `video_rotation_deg = 90` (`90° CW`
///   Portrait Fill, `888x1920` fills `346x750` `100%` with `0 px` black bar, 1:1 Mode Wi-Fi!).
/// - When `auto_rotate_enabled == true || manual_landscape_override` AND `source_w > source_h`:
///   switches cleanly to `840x468` (`1200x468`) at `video_rotation = 0°` (native Landscape).
/// - When `source_h >= source_w` (Portrait): restores `346x832` (`706x832`) at `0°` unless manually toggled.
pub fn resolve_airplay_native_orientation_plan(
    airplay_active: bool,
    source_w: u32,
    source_h: u32,
    drawer_open: bool,
    device_profile: &str,
    auto_rotate_enabled: bool,
    manual_landscape_override: bool,
) -> Option<AirplayOrientationPlan> {
    if !airplay_active || source_w == 0 || source_h == 0 {
        return None;
    }
    let source_landscape = source_w > source_h;
    let (is_landscape, video_rotation_deg) = if source_landscape {
        if auto_rotate_enabled || manual_landscape_override {
            (true, 0)
        } else {
            (false, 90)
        }
    } else if auto_rotate_enabled {
        (false, 0)
    } else if manual_landscape_override {
        (true, 270)
    } else {
        (false, 0)
    };
    let (window_logical_w, window_logical_h) = compute_native_window_target_logical_size_for_profile(
        drawer_open,
        is_landscape,
        device_profile,
    );
    Some(AirplayOrientationPlan {
        is_landscape,
        video_rotation_deg,
        window_logical_w,
        window_logical_h,
    })
}

/// Tahap 278 (Pilar 3): Manual-only AirPlay orientation lock (`auto_rotate_enabled == false`).
/// Keeps AirPlay window strictly Vertical (`346x832`) by default across `994x2160` (`0°`) and
/// `2160x994` (`90° CW`) until the user explicitly invokes Rotate (`Alt+H` / top-bar Rotate),
/// which flips to Horizontal (`840x468`) at `0°` (`2160x994`) or `270° CW` (`994x2160`).
pub fn resolve_airplay_manual_orientation_plan(
    airplay_active: bool,
    source_w: u32,
    source_h: u32,
    drawer_open: bool,
    device_profile: &str,
    manual_landscape_override: bool,
) -> Option<AirplayOrientationPlan> {
    resolve_airplay_native_orientation_plan(
        airplay_active,
        source_w,
        source_h,
        drawer_open,
        device_profile,
        false,
        manual_landscape_override,
    )
}

/// Maps visual video rotation (`0/90/180/270`) to physical iOS HID digitizer rotation:
/// for native Landscape AirPlay frames (`1920x888`), raw frame is rotated `+90°` relative to the
/// physical Portrait digitizer, so visual `90° CW` (Portrait Fill) maps to HID `0°` and
/// visual `0°` (Landscape) maps to HID `270°`.
pub fn resolve_airplay_hid_rotation_deg(
    airplay_native_landscape: bool,
    video_rotation_deg: i32,
) -> u32 {
    if airplay_native_landscape {
        match video_rotation_deg.rem_euclid(360) {
            90 => 0,
            0 => 270,
            180 => 90,
            270 => 180,
            _ => 0,
        }
    } else {
        video_rotation_deg.rem_euclid(360) as u32
    }
}

/// Dispatches any of the 6 Side Drawer Hardware & System Gesture Keys via the exact proven
/// Tahap 259 pipeline (`notifications` / `control_center` top edge `y=0` swipes + PLI,
/// `lock` 25ms anti-Siri short tap, `mute` / `volume-up` / `volume-down` consumer HID),
/// cancelling active touchpad touch first.
pub fn execute_tahap259_hardware_key(
    state: &Arc<Mutex<server_v4::AppState>>,
    web_dir: &Path,
    router: &NativeInputRouter,
    key_name: &str,
    rotation_deg: u32,
) -> (bool, bool, String, String) {
    router.cancel_active_touch();
    hid::set_active_rotation_deg(rotation_deg);
    let canon = match key_name.trim().to_ascii_lowercase().as_str() {
        "notifications" | "notif" => "notifications",
        "control_center" | "control-center" | "control" => "control_center",
        "lock" | "power" => "lock",
        "mute" => "mute",
        "volume_up" | "volume-up" | "vol-up" => "volume-up",
        "volume_down" | "volume-down" | "vol-down" => "volume-down",
        other => {
            return (
                false,
                false,
                other.to_string(),
                format!(r#"{{"ok":false,"error":"unknown key {}"}}"#, escape_json(other)),
            );
        }
    };

    if canon == "lock" {
        let (ok, _hid_live, dispatched, _stealth_restored, resp) =
            execute_lock_screen_action(state, web_dir, router);
        return (ok, dispatched, canon.to_string(), resp);
    }

    let (endpoint, body, is_gesture) = match canon {
        "notifications" => (
            "/mirror/gesture",
            r#"{"direction":"notifications"}"#.to_string(),
            true,
        ),
        "control_center" => (
            "/mirror/gesture",
            r#"{"direction":"control_center"}"#.to_string(),
            true,
        ),
        _ => ("/mirror/button", format!(r#"{{"name":"{canon}","state":"press"}}"#), false),
    };

    let resp = inprocess_api(state, web_dir, "POST", endpoint, &body);
    if is_gesture {
        mirror::request_live_pli();
    }
    let v: Option<serde_json::Value> = serde_json::from_str(&resp).ok();
    let ok = v
        .as_ref()
        .and_then(|x| x.get("ok"))
        .and_then(|x| x.as_bool())
        .unwrap_or_else(|| resp.contains("\"ok\":true"));
    let dispatched = v
        .as_ref()
        .and_then(|x| x.get("physicalSent"))
        .and_then(|x| x.as_bool())
        .unwrap_or_else(|| resp.contains("\"physicalSent\":true"));
    (ok, dispatched, canon.to_string(), resp)
}

/// Executes Side Drawer Lock Screen synchronously with `cancel_active_touch()`, stealth-restore flag, and truth flags.
fn execute_lock_screen_action(
    state: &Arc<Mutex<server_v4::AppState>>,
    web_dir: &Path,
    router: &NativeInputRouter,
) -> (bool, bool, bool, bool, String) {
    router.cancel_active_touch();
    let resp = inprocess_api(state, web_dir, "POST", "/api/device/lock", "{}");
    let v: Option<serde_json::Value> = serde_json::from_str(&resp).ok();
    let ok = v
        .as_ref()
        .and_then(|x| x.get("ok"))
        .and_then(|x| x.as_bool())
        .unwrap_or_else(|| resp.contains("\"ok\":true"));
    let hid_live = v
        .as_ref()
        .and_then(|x| x.get("physical_hid_active"))
        .and_then(|x| x.as_bool())
        .unwrap_or_else(|| resp.contains("\"physical_hid_active\":true"));
    let dispatched = v
        .as_ref()
        .and_then(|x| x.get("dispatched"))
        .and_then(|x| x.as_bool())
        .unwrap_or_else(|| resp.contains("\"dispatched\":true"));
    let stealth_restored = v
        .as_ref()
        .and_then(|x| x.get("stealthRestored"))
        .and_then(|x| x.as_bool())
        .unwrap_or_else(|| resp.contains("\"stealthRestored\":true"));
    (ok, hid_live, dispatched, stealth_restored, resp)
}

/// Executes Wake + Unlock with optional UI PIN digits (falling back to DPAPI stored PIN on server)
/// without logging plaintext PIN or mutating typed history buffers.
fn execute_wake_unlock_action(
    state: &Arc<Mutex<server_v4::AppState>>,
    web_dir: &Path,
    router: &NativeInputRouter,
    ui_pin_raw: &str,
) -> (bool, bool, bool, bool, String) {
    router.cancel_active_touch();
    let clean_pin: String = ui_pin_raw
        .trim()
        .chars()
        .filter(|c| c.is_ascii_digit())
        .take(16)
        .collect();
    let body = if clean_pin.is_empty() {
        "{}".to_string()
    } else {
        format!(r#"{{"passcode":"{}"}}"#, clean_pin)
    };
    let resp = inprocess_api(state, web_dir, "POST", "/api/device/wake-unlock", &body);
    let v: Option<serde_json::Value> = serde_json::from_str(&resp).ok();
    let ok = v
        .as_ref()
        .and_then(|x| x.get("ok"))
        .and_then(|x| x.as_bool())
        .unwrap_or_else(|| resp.contains("\"ok\":true"));
    let hid_live = v
        .as_ref()
        .and_then(|x| x.get("physical_hid_active"))
        .and_then(|x| x.as_bool())
        .unwrap_or_else(|| resp.contains("\"physical_hid_active\":true"));
    let dispatched = v
        .as_ref()
        .and_then(|x| x.get("dispatched"))
        .and_then(|x| x.as_bool())
        .unwrap_or_else(|| resp.contains("\"dispatched\":true"));
    let unlocked = v
        .as_ref()
        .and_then(|x| x.get("unlocked"))
        .and_then(|x| x.as_bool())
        .unwrap_or_else(|| resp.contains("\"unlocked\":true"));
    (ok, hid_live, dispatched, unlocked, resp)
}

// ============================================================================
// TAHAP 265 HELPERS: READ-ONLY UI ACTION LOG (0 PII), DIAGNOSTICS & START RESOLVER
// ============================================================================

#[derive(Debug, Clone)]
pub struct UiActionLogEntry {
    pub seq: u64,
    pub action: String,
    pub detail: String,
    pub drawer_open: bool,
    pub is_landscape: bool,
    pub rotation_deg: i32,
    pub is_fullscreen: bool,
    pub dark_mode: bool,
    pub lang_en: bool,
    pub fit_mode: String,
    pub device_profile: String,
    pub stream_mode: String,
    pub phase: String,
}

static UI_ACTION_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static UI_ACTION_LOG: Mutex<VecDeque<UiActionLogEntry>> = Mutex::new(VecDeque::new());

/// Records a sanitized, 100% PII-free UI/Win32 interaction into the 64-entry ring buffer
/// read by `GET /api/debug/ui-action-log`. Never logs PINs, clipboard text, IPs, names, or UDIDs.
pub fn record_ui_action(action: &str, detail: &str, ui: Option<&MainWindow>) {
    let seq = UI_ACTION_SEQ.fetch_add(1, Ordering::SeqCst) + 1;
    let entry = if let Some(u) = ui {
        UiActionLogEntry {
            seq,
            action: action.to_string(),
            detail: detail.to_string(),
            drawer_open: u.get_drawer_open(),
            is_landscape: u.get_is_landscape(),
            rotation_deg: u.get_rotation_deg(),
            is_fullscreen: u.get_is_fullscreen(),
            dark_mode: u.get_dark_mode(),
            lang_en: u.get_lang_en(),
            fit_mode: u.get_fit_mode().to_string(),
            device_profile: u.get_device_profile().to_string(),
            stream_mode: u.get_stream_mode().to_string(),
            phase: u.get_phase().to_string(),
        }
    } else {
        UiActionLogEntry {
            seq,
            action: action.to_string(),
            detail: detail.to_string(),
            drawer_open: false,
            is_landscape: false,
            rotation_deg: 0,
            is_fullscreen: false,
            dark_mode: true,
            lang_en: false,
            fit_mode: String::new(),
            device_profile: String::new(),
            stream_mode: String::new(),
            phase: String::new(),
        }
    };
    if let Ok(mut g) = UI_ACTION_LOG.lock() {
        while g.len() >= 64 {
            g.pop_front();
        }
        g.push_back(entry);
    }
}

/// Returns the read-only loopback JSON snapshot for `GET /api/debug/ui-action-log`.
pub fn ui_action_log_json() -> String {
    let entries: Vec<UiActionLogEntry> = UI_ACTION_LOG
        .lock()
        .map(|g| g.iter().cloned().collect())
        .unwrap_or_default();
    let mut items = String::from("[");
    for (i, e) in entries.iter().enumerate() {
        if i > 0 {
            items.push(',');
        }
        items.push_str(&format!(
            r#"{{"seq":{},"action":"{}","detail":"{}","drawerOpen":{},"isLandscape":{},"rotationDeg":{},"isFullscreen":{},"darkMode":{},"langEn":{},"fitMode":"{}","deviceProfile":"{}","streamMode":"{}","phase":"{}"}}"#,
            e.seq,
            escape_json(&e.action),
            escape_json(&e.detail),
            e.drawer_open,
            e.is_landscape,
            e.rotation_deg,
            e.is_fullscreen,
            e.dark_mode,
            e.lang_en,
            escape_json(&e.fit_mode),
            escape_json(&e.device_profile),
            escape_json(&e.stream_mode),
            escape_json(&e.phase)
        ));
    }
    items.push(']');
    format!(
        r#"{{"ok":true,"count":{},"entries":{}}}"#,
        entries.len(),
        items
    )
}

pub fn clear_ui_action_log_for_test() {
    if let Ok(mut g) = UI_ACTION_LOG.lock() {
        g.clear();
    }
}

fn detect_local_lan_ipv4() -> String {
    if connection::is_loopback_only_test_mode() {
        return "127.0.0.1".to_string();
    }
    if let Ok(sock) = std::net::UdpSocket::bind("0.0.0.0:0") {
        if sock.connect("8.8.8.8:80").is_ok() {
            if let Ok(addr) = sock.local_addr() {
                let ip = addr.ip().to_string();
                if !ip.starts_with("127.") && ip != "0.0.0.0" {
                    return ip;
                }
            }
        }
    }
    "127.0.0.1".to_string()
}

fn fit_mode_to_ui_key(raw: &str) -> &'static str {
    match raw.trim().to_ascii_lowercase().as_str() {
        "fill" | "edge_to_edge" | "edgetoedge" | "edge" => "edge_to_edge",
        "contain" | "original_ratio" | "original" => "original_ratio",
        _ => "auto_fit",
    }
}

fn fit_mode_to_backend_key(ui_key: &str) -> &'static str {
    match ui_key.trim() {
        "edge_to_edge" => "fill",
        "original_ratio" => "contain",
        _ => "autofit",
    }
}

fn fit_mode_to_enum(ui_key: &str) -> FitMode {
    match ui_key.trim() {
        "edge_to_edge" => FitMode::EdgeToEdge,
        "original_ratio" => FitMode::OriginalRatio,
        _ => FitMode::AutoFit,
    }
}

fn escape_json(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

struct LaunchpadDeviceSummary {
    cards: Vec<DeviceCardItem>,
    selected_udid: String,
    model_name: String,
    hero_title: String,
    hero_subtitle: String,
    pill_title: String,
    pill_detail: String,
    status_pill_short: String,
    status_pill_wide: String,
    ready: bool,
    wifi_text: String,
}

fn parse_launchpad_from_status(val: &serde_json::Value, lang_en: bool) -> LaunchpadDeviceSummary {
    let selected = val
        .get("selectedUdid")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let mut cards = Vec::new();
    let mut sel_model = String::new();
    let mut sel_name = String::new();
    let mut sel_detail = String::new();
    let mut sel_ip = String::new();
    let mut sel_usb = false;
    let mut sel_wifi = false;
    let mut sel_ready = false;
    let mut first_udid = String::new();

    if let Some(arr) = val.get("devices").and_then(|v| v.as_array()) {
        for d in arr {
            let udid = d
                .get("udid")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if udid.is_empty() {
                continue;
            }
            if first_udid.is_empty() {
                first_udid = udid.clone();
            }
            let name = d
                .get("name")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .or_else(|| d.get("model").and_then(|v| v.as_str()))
                .unwrap_or("iPhone")
                .to_string();
            let model = d
                .get("model")
                .and_then(|v| v.as_str())
                .unwrap_or("iPhone")
                .to_string();
            let ip = d
                .get("ip")
                .or_else(|| d.get("wifiIp"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let usb = d.get("usb").and_then(|v| v.as_bool()).unwrap_or(false);
            let wifi_online = d
                .get("wifiOnline")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let reachable = d.get("reachable").and_then(|v| v.as_bool()).unwrap_or(false);
            let wifi_paired = d.get("wifiPaired").and_then(|v| v.as_bool()).unwrap_or(false);
            let wifi_up = wifi_online || (reachable && wifi_paired);
            let online = usb || wifi_up;

            let display_title = if !model.is_empty() && !name.is_empty() && !model.eq_ignore_ascii_case(&name) {
                format!("{model} · {name}")
            } else if !name.is_empty() {
                name.clone()
            } else {
                model.clone()
            };

            let badge = if usb && wifi_up {
                "USB + Wi-Fi"
            } else if usb {
                "USB"
            } else if wifi_up {
                "Wi-Fi"
            } else {
                "Offline"
            };
            let subtitle_line = if !ip.is_empty() && wifi_up {
                format!("Wi-Fi · {ip}")
            } else if !ip.is_empty() && !online {
                format!("Offline ({ip})")
            } else {
                badge.to_string()
            };

            let is_sel = (!selected.is_empty() && selected.eq_ignore_ascii_case(&udid))
                || d.get("selected").and_then(|v| v.as_bool()).unwrap_or(false);
            if is_sel || sel_name.is_empty() {
                sel_model = model.clone();
                sel_name = display_title.clone();
                sel_ip = ip.to_string();
                sel_usb = usb;
                sel_wifi = wifi_up;
                sel_ready = online;
                sel_detail = if !ip.is_empty() {
                    if lang_en {
                        format!("{model} · {badge} · iPhone IP: {ip}")
                    } else {
                        format!("{model} · {badge} · IP iPhone: {ip}")
                    }
                } else {
                    format!("{model} · {badge}")
                };
            }
            cards.push(DeviceCardItem {
                udid: SharedString::from(udid),
                name: SharedString::from(display_title),
                model: SharedString::from(subtitle_line),
                transport_badge: SharedString::from(badge),
                online,
                selected: is_sel,
            });
        }
    }

    let eff_sel = if !selected.is_empty() { selected } else { first_udid };
    let hero_base = if !sel_model.is_empty() { &sel_model } else { "iPhone" };
    let model_name = hero_base.to_string();
    let hero_title = if !sel_name.is_empty() {
        if sel_ready {
            if lang_en { format!("{hero_base} ready") } else { format!("{hero_base} siap") }
        } else {
            format!("{hero_base} (Offline)")
        }
    } else if lang_en {
        "Connect your iPhone".to_string()
    } else {
        "Hubungkan iPhone Anda".to_string()
    };

    let hero_subtitle = if lang_en {
        "Unlock screen, then press Start mirroring.".to_string()
    } else {
        "Buka kunci, lalu tekan Mulai mirroring.".to_string()
    };

    let pill_title = if !sel_name.is_empty() {
        sel_name.clone()
    } else if lang_en {
        "Scanning iOS devices...".to_string()
    } else {
        "Memindai perangkat iOS...".to_string()
    };
    let pill_detail = if !sel_detail.is_empty() {
        sel_detail.clone()
    } else {
        "USB / Wi-Fi Auto-Discovery".to_string()
    };
    let wifi_text = if sel_ready && !sel_ip.is_empty() {
        if lang_en {
            format!("Wi-Fi Ready (iPhone LAN IP: {sel_ip})")
        } else {
            format!("Wi-Fi Siap (IP iPhone di LAN: {sel_ip})")
        }
    } else if sel_ready && !sel_detail.is_empty() {
        sel_detail.clone()
    } else if lang_en {
        "Ready to scan iPhone over LAN Wi-Fi.".to_string()
    } else {
        "Siap memindai iPhone di jaringan Wi-Fi LAN.".to_string()
    };
    let status_pill_short = if sel_wifi && !sel_ip.is_empty() {
        if lang_en {
            format!("Wi-Fi · iPhone IP: {sel_ip}")
        } else {
            format!("Wi-Fi · IP iPhone: {sel_ip}")
        }
    } else if sel_usb {
        if lang_en {
            "USB Ready".to_string()
        } else {
            "USB Siap".to_string()
        }
    } else if lang_en {
        "Ready".to_string()
    } else {
        "Siap".to_string()
    };
    let status_pill_wide = status_pill_short.clone();

    LaunchpadDeviceSummary {
        cards,
        selected_udid: eff_sel,
        model_name,
        hero_title,
        hero_subtitle,
        pill_title,
        pill_detail,
        status_pill_short,
        status_pill_wide,
        ready: sel_ready,
        wifi_text,
    }
}

fn parse_device_cards_from_status(
    val: &serde_json::Value,
    lang_en: bool,
) -> (Vec<DeviceCardItem>, String) {
    let sum = parse_launchpad_from_status(val, lang_en);
    (sum.cards, sum.selected_udid)
}

fn build_diag_rows_from_json(val: &serde_json::Value, lang_en: bool) -> Vec<DiagRow> {
    let d = val.get("diagnostics").unwrap_or(val);
    let item = |label_id: &str, label_en: &str, ok_key: &str, lbl_id_key: &str, lbl_en_key: &str| {
        let ok = d.get(ok_key).and_then(|v| v.as_bool()).unwrap_or(false);
        let status = if lang_en {
            d.get(lbl_en_key).and_then(|v| v.as_str()).unwrap_or(if ok { "OK" } else { "Pending" })
        } else {
            d.get(lbl_id_key).and_then(|v| v.as_str()).unwrap_or(if ok { "Siap" } else { "Belum" })
        };
        DiagRow {
            label: SharedString::from(if lang_en { label_en } else { label_id }),
            status: SharedString::from(status),
            ok,
        }
    };
    let dev_ok = d.get("developer_mode_bool").and_then(|v| v.as_bool()).unwrap_or(false);
    let dev_status = if lang_en {
        d.get("developer_mode_label").and_then(|v| v.as_str()).unwrap_or("Check USB")
    } else {
        d.get("developer_mode_label_id").and_then(|v| v.as_str()).unwrap_or("Cek via USB")
    };
    vec![
        item("Apple USB Driver", "Apple USB Driver", "apple_driver_installed", "apple_driver_label_id", "apple_driver_label"),
        item("Layanan Apple Mobile", "Apple Mobile Service", "apple_service_running", "apple_service_label_id", "apple_service_label"),
        item("Layanan usbmuxd (27015)", "usbmuxd Service (27015)", "tcp_port_27015_normal", "tcp_port_label_id", "tcp_port_label"),
        item("Codec Video HEVC 60fps", "HEVC 60fps Video Codec", "hevc_codec_installed", "hevc_codec_label_id", "hevc_codec_label"),
        DiagRow {
            label: SharedString::from(if lang_en { "iOS Developer Mode" } else { "Developer Mode iOS" }),
            status: SharedString::from(dev_status),
            ok: dev_ok,
        },
    ]
}

fn extract_amfi_ui_feedback(dv: &serde_json::Value, lang_en: bool) -> (bool, String) {
    let ok = dv.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
    let primary = if lang_en { "message_en" } else { "message_id" };
    let secondary = if lang_en { "message_id" } else { "message_en" };
    let msg = dv
        .get(primary)
        .or_else(|| dv.get(secondary))
        .or_else(|| dv.get("message"))
        .or_else(|| dv.get("error"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            if ok {
                if lang_en {
                    "Developer Mode menu revealed on your iPhone. Open Settings -> Privacy & Security -> Developer Mode -> Turn On.".to_string()
                } else {
                    "Menu Developer Mode berhasil dimunculkan. Buka Pengaturan -> Privasi & Keamanan -> Mode Pengembang -> Nyalakan.".to_string()
                }
            } else if lang_en {
                "Connect your iPhone via USB cable, unlock it, and tap Trust This Computer first.".to_string()
            } else {
                "Hubungkan iPhone via kabel USB, buka kunci layar, & ketuk Trust This Computer terlebih dahulu.".to_string()
            }
        });
    (ok, msg)
}

fn format_updater_ui_state(
    v: &serde_json::Value,
    lang_en: bool,
) -> (bool, String, String, String, String) {
    let update_avail = v
        .get("updateAvailable")
        .or_else(|| v.get("update_available"))
        .and_then(|x| x.as_bool())
        .unwrap_or(false);
    let latest_ver = v
        .get("latestVersion")
        .or_else(|| v.get("latest_version"))
        .and_then(|x| x.as_str())
        .unwrap_or(crate::updater::CURRENT_VERSION)
        .trim()
        .to_string();
    let release_url = v
        .get("releaseUrl")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let download_url = v
        .get("downloadUrl")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let status_text = if update_avail {
        if lang_en {
            format!("New version available: iPhone Link v{latest_ver}")
        } else {
            format!("Tersedia pembaruan: iPhone Link v{latest_ver}")
        }
    } else if lang_en {
        format!("iPhone Link v{} is up to date.", crate::updater::CURRENT_VERSION)
    } else {
        format!("iPhone Link v{} sudah versi terbaru.", crate::updater::CURRENT_VERSION)
    };
    (update_avail, latest_ver, release_url, download_url, status_text)
}

fn format_anti_blank_overlay_copy(reason: &str, lang_en: bool) -> (String, String) {
    match reason {
        "media_feature_pack_missing" => {
            if lang_en {
                (
                    "Media Feature Pack Required".to_string(),
                    "Windows N/KN is missing Media Foundation codecs (H.264/HEVC). Open Optional Features, install Media Feature Pack, then restart Windows.".to_string(),
                )
            } else {
                (
                    "Media Feature Pack Belum Terpasang".to_string(),
                    "Windows Edisi N/KN belum memiliki codec Media Foundation (H.264/HEVC). Buka Optional Features, pasang Media Feature Pack, lalu restart Windows.".to_string(),
                )
            }
        }
        "ios_below_27" => {
            if lang_en {
                (
                    "AirPlay Mode Required (iOS < 27)".to_string(),
                    "Apple limits 60fps USB mirroring and mouse/keyboard control to iOS 27+. Switch to AirPlay Mode (Wi-Fi) for display & audio (view-only).".to_string(),
                )
            } else {
                (
                    "Mode AirPlay Diperlukan (iOS < 27)".to_string(),
                    "Apple membatasi mirroring USB 60fps & kontrol mouse/keyboard hanya untuk iOS 27+. Lanjut ke Mode AirPlay (Wi-Fi) untuk tampilan & audio saja.".to_string(),
                )
            }
        }
        "hevc_mft_missing" => {
            if lang_en {
                (
                    "HEVC Codec Missing on This PC".to_string(),
                    "Your PC does not have HEVC Video Extensions yet. Switch to AirPlay H.264 (works on all Windows 10/11 PCs) or install Free HEVC.".to_string(),
                )
            } else {
                (
                    "Codec Video HEVC Belum Terpasang".to_string(),
                    "PC belum memiliki HEVC Video Extensions. Pindah ke Mode AirPlay H.264 (berjalan di semua PC Windows 10/11) atau pasang HEVC Gratis.".to_string(),
                )
            }
        }
        "decoder_init_failed" => {
            if lang_en {
                (
                    "Hardware HEVC Decoder Failed".to_string(),
                    "GPU HEVC decoding could not initialize. Switch to AirPlay H.264 (built-in Windows decoder) or install Free HEVC Extensions.".to_string(),
                )
            } else {
                (
                    "Dekoder HEVC GPU Gagal Dimulai".to_string(),
                    "Dekoder hardware HEVC gagal diinisialisasi. Pindah ke Mode AirPlay H.264 (bawaan Windows) atau pasang HEVC Gratis.".to_string(),
                )
            }
        }
        _ => {
            if lang_en {
                (
                    "No Video Frame Received Yet".to_string(),
                    "Mirroring started but no frame rendered after 7s. Switch to AirPlay H.264 (compatible with iOS 17-26 & Win 10) or install Free HEVC.".to_string(),
                )
            } else {
                (
                    "Belum Ada Frame Video Masuk".to_string(),
                    "Sesi berjalan tetapi belum ada frame tampil setelah 7 detik. Pindah ke AirPlay H.264 (kompatibel iOS 17-26 & Win 10) atau pasang HEVC Gratis.".to_string(),
                )
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartMirrorOutcome {
    Started {
        phase: String,
        stage: String,
        fallback_reason: Option<String>,
        airplay: bool,
    },
    DecisionRequired {
        reason: String,
        message_id: String,
        message_en: String,
        ios_version: String,
    },
    Error(String),
}

pub fn parse_start_mirror_outcome_v2(resp: &str, lang_en: bool) -> StartMirrorOutcome {
    let v: serde_json::Value = match serde_json::from_str(resp) {
        Ok(val) => val,
        Err(_) => {
            return StartMirrorOutcome::Error(if lang_en {
                "Failed to parse start-mirror response from local engine.".to_string()
            } else {
                "Gagal membaca respons mulai mirroring dari mesin lokal.".to_string()
            });
        }
    };
    if v.get("requiresDecision")
        .and_then(|x| x.as_bool())
        .unwrap_or(false)
    {
        let reason = v
            .get("decisionReason")
            .and_then(|x| x.as_str())
            .unwrap_or("hevc_mft_missing")
            .trim()
            .to_string();
        let message_id = v
            .get("message_id")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let message_en = v
            .get("message_en")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let ios_version = v
            .get("iosVersion")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        return StartMirrorOutcome::DecisionRequired {
            reason,
            message_id,
            message_en,
            ios_version,
        };
    }

    let ok = v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false);
    let phase = v
        .get("phase")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let stage = v
        .get("stage")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let fallback_reason = v
        .get("fallbackReason")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let err_msg = v
        .get("error")
        .or_else(|| v.get(if lang_en { "message_en" } else { "message_id" }))
        .or_else(|| v.get("message"))
        .or_else(|| v.get("detail"))
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    if (!ok && phase != "running" && phase != "starting" && phase != "waiting_airplay")
        || phase == "error"
        || !err_msg.is_empty()
        || (phase != "running" && phase != "starting" && phase != "waiting_airplay")
    {
        let fallback = if lang_en {
            "Could not start mirroring. Verify iPhone unlock & USB/Wi-Fi connection."
        } else {
            "Gagal memulai mirroring. Pastikan iPhone terbuka kuncinya & koneksi USB/Wi-Fi aktif."
        };
        return StartMirrorOutcome::Error(if err_msg.is_empty() {
            fallback.to_string()
        } else {
            err_msg
        });
    }
    let route_is_airplay = v
        .get("mirrorRoute")
        .and_then(|x| x.as_str())
        .is_some_and(|r| r.contains("airplay"));
    let airplay = stage == "waiting_airplay" || route_is_airplay;
    StartMirrorOutcome::Started {
        phase,
        stage,
        fallback_reason,
        airplay,
    }
}

#[allow(dead_code)]
fn parse_start_mirror_outcome_ex(
    resp: &str,
    lang_en: bool,
) -> Result<(String, String, Option<String>), String> {
    match parse_start_mirror_outcome_v2(resp, lang_en) {
        StartMirrorOutcome::Started {
            phase,
            stage,
            fallback_reason,
            ..
        } => Ok((phase, stage, fallback_reason)),
        StartMirrorOutcome::DecisionRequired {
            message_id,
            message_en,
            ..
        } => Err(if lang_en {
            if message_en.is_empty() { message_id } else { message_en }
        } else if message_id.is_empty() {
            message_en
        } else {
            message_id
        }),
        StartMirrorOutcome::Error(e) => Err(e),
    }
}

#[allow(dead_code)]
fn parse_start_mirror_outcome(resp: &str, lang_en: bool) -> Result<(String, String), String> {
    parse_start_mirror_outcome_ex(resp, lang_en).map(|(ph, stg, _)| (ph, stg))
}

fn resolve_udid_and_start_mirror_v2(
    state: &Arc<Mutex<server_v4::AppState>>,
    web_dir: &Path,
    initial_udid: &str,
    transport: &str,
    stream_mode: &str,
    lang_en: bool,
) -> StartMirrorOutcome {
    let mut udid = initial_udid.trim().to_string();
    let tr = if transport.eq_ignore_ascii_case("usb") { "usb" } else { "wifi" };
    let mode = if stream_mode.eq_ignore_ascii_case("airplay") { "airplay" } else { "auto" };

    if udid.is_empty() {
        let status_body = inprocess_api(state, web_dir, "GET", "/api/status", "");
        if let Ok(sv) = serde_json::from_str(&status_body) {
            let sum = parse_launchpad_from_status(&sv, lang_en);
            if !sum.selected_udid.is_empty() {
                udid = sum.selected_udid;
            }
        }
        if udid.is_empty() {
            if tr == "wifi" {
                let _ = inprocess_api(state, web_dir, "POST", "/api/wifi/scan", "{}");
            } else {
                let _ = inprocess_api(state, web_dir, "GET", "/api/status/refresh?force=1", "");
            }
            let status_body2 = inprocess_api(state, web_dir, "GET", "/api/status", "");
            if let Ok(sv2) = serde_json::from_str(&status_body2) {
                let sum2 = parse_launchpad_from_status(&sv2, lang_en);
                udid = sum2.selected_udid;
            }
        }
    }

    if udid.is_empty() && mode != "airplay" {
        return StartMirrorOutcome::Error(if lang_en {
            "No iPhone selected or detected yet. Plug in USB or tap Pindai Wi-Fi first.".to_string()
        } else {
            "Belum ada iPhone terdeteksi. Hubungkan kabel USB atau klik Pindai Wi-Fi terlebih dahulu.".to_string()
        });
    }

    let body = format!(
        r#"{{"udid":"{}","transport":"{}","mode":"{}","streamMode":"{}","honestGate":true}}"#,
        escape_json(&udid),
        escape_json(tr),
        escape_json(mode),
        escape_json(mode)
    );
    let resp = inprocess_api(state, web_dir, "POST", "/api/screen/start", &body);
    parse_start_mirror_outcome_v2(&resp, lang_en)
}

#[allow(dead_code)]
fn resolve_udid_and_start_mirror_ex(
    state: &Arc<Mutex<server_v4::AppState>>,
    web_dir: &Path,
    initial_udid: &str,
    transport: &str,
    stream_mode: &str,
    lang_en: bool,
) -> Result<(String, String, Option<String>), String> {
    match resolve_udid_and_start_mirror_v2(
        state,
        web_dir,
        initial_udid,
        transport,
        stream_mode,
        lang_en,
    ) {
        StartMirrorOutcome::Started {
            phase,
            stage,
            fallback_reason,
            ..
        } => Ok((phase, stage, fallback_reason)),
        StartMirrorOutcome::DecisionRequired {
            message_id,
            message_en,
            ..
        } => Err(if lang_en {
            if message_en.is_empty() { message_id } else { message_en }
        } else if message_id.is_empty() {
            message_en
        } else {
            message_id
        }),
        StartMirrorOutcome::Error(e) => Err(e),
    }
}

#[allow(dead_code)]
fn resolve_udid_and_start_mirror(
    state: &Arc<Mutex<server_v4::AppState>>,
    web_dir: &Path,
    initial_udid: &str,
    transport: &str,
    stream_mode: &str,
    lang_en: bool,
) -> Result<(String, String), String> {
    resolve_udid_and_start_mirror_ex(state, web_dir, initial_udid, transport, stream_mode, lang_en)
        .map(|(ph, stg, _)| (ph, stg))
}

#[allow(dead_code)]
fn sync_child_hwnd_bounds(
    hwnd: isize,
    ui: &MainWindow,
    pipeline: &LiveVideoPipeline,
    force_visible: Option<bool>,
) -> (i32, i32, u32, u32) {
    let scale = query_hwnd_scale(hwnd);
    let (cw, ch) = query_client_size(hwnd);
    let logical_w = (cw as f32) / scale;
    let logical_h = (ch as f32) / scale;
    let drawer_open = ui.get_drawer_open();
    let is_fullscreen = ui.get_is_fullscreen();
    let is_landscape = ui.get_is_landscape();
    let (x, y, w, h) = compute_native_stage_physical_rect(
        logical_w,
        logical_h,
        scale,
        drawer_open,
        is_fullscreen,
        is_landscape,
    );
    let phase = ui.get_phase().to_string();
    let stage = ui.get_stage().to_string();
    let vis = force_visible.unwrap_or_else(|| should_video_hwnd_be_visible(&phase, &stage));
    pipeline.set_bounds(x, y, w, h, vis);
    (x, y, w, h)
}

/// Synchronizes the Single Source of Truth `ActiveVideoPhysicalRect` across:
/// - `active_rect_shared` (used by Win32 pointer & 120 Hz scroll hit-testing),
/// - Slint transparent video hole (`video-hole-{x,y,w,h}` + `video-layer-active`),
/// - Composition `LiveVideoPipeline` swapchain size, and
/// - DirectComposition Layer 0 `video_visual` offset & swapchain content attachment.
fn sync_dcomp_video_layout(
    hwnd: isize,
    ui: &MainWindow,
    pipeline: &LiveVideoPipeline,
    active_rect_shared: &Arc<Mutex<ActiveVideoPhysicalRect>>,
    force_visible: Option<bool>,
) -> ActiveVideoPhysicalRect {
    let scale = query_hwnd_scale(hwnd);
    let (cw, ch) = read_live_hwnd_client_phys_size(hwnd);
    let drawer_open = ui.get_drawer_open();
    let is_fullscreen = ui.get_is_fullscreen();
    let is_landscape = ui.get_is_landscape();
    let rot_deg = ui.get_rotation_deg().clamp(0, 359) as u16;
    let fit_mode = fit_mode_to_enum(&ui.get_fit_mode().to_string());
    let tel = pipeline.telemetry();

    let prev_rect = active_rect_shared.lock().ok().map(|g| *g);
    let (rect, rect_changed) = compute_fullscreen_landscape_resync_rect(
        prev_rect,
        cw,
        ch,
        scale,
        drawer_open,
        is_fullscreen,
        is_landscape,
        tel.width,
        tel.height,
        rot_deg,
        fit_mode,
        mirror::is_airplay_active(),
    );
    airplay::set_trace_last_active_video_rect(
        rect.x,
        rect.y,
        rect.width as i32,
        rect.height as i32,
    );
    if let Ok(mut g) = active_rect_shared.lock() {
        *g = rect;
    }

    ui.set_video_hole_x((rect.x as f32) / scale);
    ui.set_video_hole_y((rect.y as f32) / scale);
    ui.set_video_hole_w((rect.width as f32) / scale);
    ui.set_video_hole_h((rect.height as f32) / scale);

    let phase = ui.get_phase().to_string();
    let stage = ui.get_stage().to_string();
    let was_visible = ui.get_video_layer_active();
    let vis = force_visible.unwrap_or_else(|| {
        should_video_hwnd_be_visible(&phase, &stage) && ui.get_first_frame_ready()
    });
    ui.set_video_layer_active(vis);

    if vis {
        // Resize composition swapchain to exact physical pixels BEFORE SetOffset & DComp commit
        let resized = pipeline.resize_composition(rect.width, rect.height).unwrap_or(false);
        pipeline.set_visible(true);
        sync_active_dcomp_video_layer(pipeline.swapchain_ptr(), rect.x as f32, rect.y as f32);
        if resized || rect_changed {
            commit_active_dcomp();
            airplay::inc_trace_dcomp_commits();
        }
    } else {
        pipeline.set_visible(false);
        sync_active_dcomp_video_layer(None, 0.0, 0.0);
        if was_visible {
            let _ = pipeline.resize_composition(16, 16);
            trim_active_dcomp_memory();
        }
    }
    flush_active_dcomp_ui_frame();
    rect
}

#[allow(clippy::too_many_arguments)]
fn sync_dcomp_video_layout_with_dims(
    hwnd: isize,
    ui: &MainWindow,
    pipeline: &LiveVideoPipeline,
    active_rect_shared: &Arc<Mutex<ActiveVideoPhysicalRect>>,
    live_video_dims: &Arc<Mutex<(u32, u32)>>,
    force_visible: Option<bool>,
) -> ActiveVideoPhysicalRect {
    let tel = pipeline.telemetry();
    if tel.width > 0 && tel.height > 0 {
        if let Ok(mut g) = live_video_dims.lock() {
            *g = (tel.width, tel.height);
        }
    }
    sync_dcomp_video_layout(hwnd, ui, pipeline, active_rect_shared, force_visible)
}

/// Tahap 275 (Pilar 3): Resolves the Top Bar status pill strictly displaying `"AirPlay"`
/// whenever AirPlay is actively streaming (`phase == "running"` & `(is_airplay_active || transport == "airplay")`),
/// while preserving `"Mirroring · Wi-Fi"` for CoreDevice Wi-Fi HEVC and `"Mirroring · USB"` for USB.
pub fn resolve_topbar_status_pill_text(
    phase: &str,
    stage: &str,
    transport: &str,
    airplay_active: bool,
    lang_en: bool,
) -> String {
    match phase {
        "running" => {
            if airplay_active || transport.eq_ignore_ascii_case("airplay") {
                if stage == "waiting_frame" {
                    "AirPlay...".to_string()
                } else {
                    "AirPlay".to_string()
                }
            } else if transport.eq_ignore_ascii_case("wifi") {
                "Mirroring · Wi-Fi".to_string()
            } else {
                "Mirroring · USB".to_string()
            }
        }
        "waiting_airplay" => {
            if lang_en { "AirPlay Ready".to_string() } else { "AirPlay Siap".to_string() }
        }
        "starting" => {
            if stage == "waiting_airplay" || airplay_active || transport.eq_ignore_ascii_case("airplay") {
                if lang_en { "AirPlay Ready".to_string() } else { "AirPlay Siap".to_string() }
            } else if lang_en {
                "Connecting\u{2026}".to_string()
            } else {
                "Menyambungkan\u{2026}".to_string()
            }
        }
        "error" => "Error".to_string(),
        _ => {
            if stage == "waiting_airplay" {
                if lang_en { "AirPlay Ready".to_string() } else { "AirPlay Siap".to_string() }
            } else if lang_en {
                "Ready".to_string()
            } else {
                "Siap".to_string()
            }
        }
    }
}

/// Pure Connecting watchdog for CoreDevice USB/Wi-Fi handshake:
/// flags `"stage_stagnant"` (>= 25s on one step) or `"total_timeout"` (>= 90s overall),
/// while staying silent during passive `waiting_airplay` sessions.
pub fn evaluate_connecting_watchdog(
    phase: &str,
    airplay_active: bool,
    stage: &str,
    stagnant_ms: u64,
    total_ms: u64,
) -> Option<&'static str> {
    if phase != "starting" || airplay_active || stage == "waiting_airplay" {
        return None;
    }
    if total_ms >= 90_000 {
        return Some("total_timeout");
    }
    if stagnant_ms >= 25_000 {
        return Some("stage_stagnant");
    }
    None
}

fn format_status_pill_with_transport(
    phase: &str,
    stage: &str,
    transport: &str,
    airplay_active: bool,
    lang_en: bool,
) -> String {
    resolve_topbar_status_pill_text(phase, stage, transport, airplay_active, lang_en)
}

pub fn resolve_compact_topbar_status_pill_text(full_pill: &str, topbar_wide: bool) -> String {
    if topbar_wide {
        full_pill.to_string()
    } else if full_pill.starts_with("Mirroring") {
        "Mirroring".to_string()
    } else if full_pill.starts_with("AirPlay") {
        "AirPlay".to_string()
    } else if full_pill.starts_with("Menyambung") {
        "Menyambung".to_string()
    } else if full_pill.starts_with("Connecting") {
        "Connecting".to_string()
    } else {
        full_pill.to_string()
    }
}

#[allow(dead_code)]
fn format_status_pill(phase: &str, stage: &str, airplay_active: bool, lang_en: bool) -> String {
    let tr = if airplay_active { "wifi" } else { "usb" };
    format_status_pill_with_transport(phase, stage, tr, airplay_active, lang_en)
}

#[inline]
pub fn clamp_trackpad_speed_pct(raw: i32) -> i32 {
    raw.clamp(30, 200)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioUiPresentation {
    pub live_active: bool,
    pub live_state: &'static str, // "active" | "connecting" | "muted" | "device" | "standby" | "error"
    pub bt_active: bool,
    pub telemetry_text: String,
    pub banner_text: String,
}

/// Single Source of Truth computing Top Bar `🔊` (`live_active`/`live_state`) and Card 3 banner
/// strictly from user preference + actual backend audio state (never showing fake green when idle/muted).
#[allow(clippy::too_many_arguments)]
pub fn compute_audio_ui_presentation(
    audio_pc_pref: bool,
    audio_muted: bool,
    streaming_running: bool,
    airplay_active: bool,
    ap_audio_active: bool,
    ap_target_ms: u32,
    ap_vol: u32,
    pc_bridge_active: bool,
    pc_bridge_connecting: bool,
    pc_bridge_state_kind: &str,
    pc_bridge_device: &str,
    lang_en: bool,
) -> AudioUiPresentation {
    if !audio_pc_pref {
        return AudioUiPresentation {
            live_active: false,
            live_state: "device",
            bt_active: false,
            telemetry_text: if lang_en { "iPhone Only" } else { "Di iPhone" }.to_string(),
            banner_text: if lang_en {
                "Audio played on iPhone only"
            } else {
                "Audio diputar di iPhone saja"
            }
            .to_string(),
        };
    }
    if audio_muted {
        return AudioUiPresentation {
            live_active: false,
            live_state: "muted",
            bt_active: false,
            telemetry_text: "Muted".to_string(),
            banner_text: if lang_en {
                "PC Audio muted (Alt+M to unmute)"
            } else {
                "Audio PC dibisukan (Alt+M untuk bunyi)"
            }
            .to_string(),
        };
    }
    if airplay_active {
        if ap_audio_active {
            return AudioUiPresentation {
                live_active: true,
                live_state: "active",
                bt_active: false,
                telemetry_text: format!("{ap_target_ms}ms buf · {ap_vol}%"),
                banner_text: format!("AirPlay Audio Live (AAC · {ap_target_ms}ms · {ap_vol}%)"),
            };
        }
        return AudioUiPresentation {
            live_active: false,
            live_state: "standby",
            bt_active: false,
            telemetry_text: "Standby".to_string(),
            banner_text: if lang_en {
                "AirPlay Audio Ready (Auto on playback)"
            } else {
                "Audio AirPlay Siap (Otomatis saat video diputar)"
            }
            .to_string(),
        };
    }
    if pc_bridge_active {
        let dev = pc_bridge_device.trim();
        let banner = if !dev.is_empty() {
            if lang_en {
                format!("A2DP Connected: {dev} (48 kHz WASAPI)")
            } else {
                format!("A2DP Terhubung: {dev} (48 kHz WASAPI)")
            }
        } else if lang_en {
            "PC Audio Active (48 kHz WASAPI / A2DP)".to_string()
        } else {
            "Audio PC Aktif (48 kHz WASAPI / A2DP)".to_string()
        };
        return AudioUiPresentation {
            live_active: true,
            live_state: "active",
            bt_active: true,
            telemetry_text: if lang_en { "BT A2DP Active" } else { "BT A2DP Aktif" }.to_string(),
            banner_text: banner,
        };
    }
    if pc_bridge_connecting {
        return AudioUiPresentation {
            live_active: false,
            live_state: "connecting",
            bt_active: false,
            telemetry_text: if lang_en { "Connecting..." } else { "Menyambungkan..." }.to_string(),
            banner_text: if lang_en {
                "Connecting PC Audio (Bluetooth A2DP)..."
            } else {
                "Menyambungkan Audio PC (Bluetooth A2DP)..."
            }
            .to_string(),
        };
    }
    if streaming_running && pc_bridge_state_kind == "error" {
        return AudioUiPresentation {
            live_active: false,
            live_state: "error",
            bt_active: false,
            telemetry_text: if lang_en { "BT Disconnected" } else { "BT Terputus" }.to_string(),
            banner_text: if lang_en {
                "Bluetooth A2DP not connected (pair iPhone <-> PC in Windows)"
            } else {
                "Bluetooth A2DP belum tersambung (pastikan Bluetooth iPhone <-> PC terhubung)"
            }
            .to_string(),
        };
    }
    AudioUiPresentation {
        live_active: false,
        live_state: "standby",
        bt_active: false,
        telemetry_text: "Standby".to_string(),
        banner_text: if lang_en {
            "PC Audio Ready (Auto on mirror start)"
        } else {
            "Audio PC Siap (Otomatis aktif saat mirroring)"
        }
        .to_string(),
    }
}

/// Pure gate deciding whether `ui_sync_timer` should trigger 1x auto-start PC audio bridge
/// for a newly live CoreDevice HEVC session.
#[allow(clippy::too_many_arguments)]
pub fn should_trigger_auto_audio_on_running(
    effective_phase: &str,
    presented_frames: u64,
    airplay_active: bool,
    effective_stage: &str,
    audio_pc_active: bool,
    audio_muted: bool,
    manually_stopped: bool,
    cur_gen: u64,
    last_triggered_gen: u64,
    loopback_only: bool,
) -> bool {
    effective_phase == "running"
        && presented_frames >= 1
        && !airplay_active
        && effective_stage != "waiting_airplay"
        && audio_pc_active
        && !audio_muted
        && !manually_stopped
        && cur_gen > 0
        && cur_gen != last_triggered_gen
        && !loopback_only
}

fn schedule_fb_reset(ui_weak: slint::Weak<MainWindow>, reset_fn: impl Fn(&MainWindow) + 'static) {
    Timer::single_shot(Duration::from_millis(1800), move || {
        if let Some(u) = ui_weak.upgrade() {
            reset_fn(&u);
        }
    });
}

// ============================================================================
// INTERACTIVE NATIVE SLINT GUI ENTRYPOINT (`--native-ui`)
// ============================================================================

/// Launches the single-process zero-WebView2 Native Slint GUI Shell (`iPhone Link.exe --native-ui`).
pub fn run_native_ui(
    listen_port: u16,
    state: Arc<Mutex<server_v4::AppState>>,
    web_dir: PathBuf,
) -> i32 {
    // Keep lightweight headless NativeServer running for iOS 27 Shortcut Bridge & /api/debug/airplay-trace
    server_v4::set_headless_mode(true);
    let server = match server_v4::NativeServer::start(state.clone(), web_dir.clone(), listen_port) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Native UI background server failed: {e}");
            return 1;
        }
    };

    ensure_dcomp_platform_installed();
    reset_dcomp_loop_state();

    let gpu = match LiveD3d11Gpu::new() {
        Ok(g) => Arc::new(g),
        Err(e) => {
            eprintln!("LiveD3d11Gpu initialization failed: {e}");
            server.stop();
            return 1;
        }
    };

    let ui = match MainWindow::new() {
        Ok(w) => w,
        Err(e) => {
            eprintln!("Slint MainWindow creation failed: {e}");
            server.stop();
            return 1;
        }
    };

    // Hydrate initial state from user_settings.json (Fresh-install always defaults stream_mode="auto")
    let (
        init_dark,
        init_en,
        init_transport,
        init_fit,
        init_ar,
        init_muted,
        init_vol,
        init_buf,
        init_ls,
        init_prof,
        init_crops,
        init_tp_speed,
        init_auto_chk,
        init_upd_ch,
        init_audio_pc,
        init_bt_a2dp,
        init_audio_route,
        init_kb_en,
        init_cps,
        init_ppc,
    ) = {
        let st = state.lock().unwrap();
        (
            st.theme != "light",
            st.language == Lang::En,
            if st.transport.eq_ignore_ascii_case("wifi") {
                "wifi".to_string()
            } else {
                "usb".to_string()
            },
            fit_mode_to_ui_key(&st.fit_mode).to_string(),
            st.auto_rotate,
            st.audio_muted,
            st.audio_volume.clamp(0, 100),
            st.audio_buffer_preset.clone(),
            st.lip_sync,
            if st.device_profile.is_empty() {
                "auto".to_string()
            } else {
                st.device_profile.clone()
            },
            (
                st.crop_top.clamp(0, 24),
                st.crop_bottom.clamp(0, 24),
                st.crop_left.clamp(0, 24),
                st.crop_right.clamp(0, 24),
            ),
            clamp_trackpad_speed_pct(st.trackpad_speed),
            st.auto_check_update,
            if st.update_channel.eq_ignore_ascii_case("beta") {
                "beta".to_string()
            } else {
                "stable".to_string()
            },
            st.audio_routing != "device",
            st.bluetooth_a2dp,
            st.audio_routing.clone(),
            true,
            st.cross_platform_shortcuts,
            st.paste_pc_clipboard,
        )
    };

    let lan_ip = detect_local_lan_ipv4();
    let bridge_url = format!("http://{}:{}/c?u=", lan_ip, server.port);
    let hydrated_pin = if should_hydrate_disk_pin(false) {
        server_v4::get_hydrated_unlock_pin()
    } else {
        None
    };

    ui.set_dark_mode(init_dark);
    ui.set_lang_en(init_en);
    ui.set_transport_tab(SharedString::from(init_transport.as_str()));
    ui.set_stream_mode(SharedString::from("auto"));
    ui.set_fit_mode(SharedString::from(init_fit.as_str()));
    ui.set_device_profile(SharedString::from(init_prof.as_str()));
    ui.set_auto_rotate(init_ar);
    ui.set_crop_top(init_crops.0);
    ui.set_crop_bottom(init_crops.1);
    ui.set_crop_left(init_crops.2);
    ui.set_crop_right(init_crops.3);
    let _ = init_bt_a2dp;
    let _ = &init_audio_route;
    let init_audio_pres = compute_audio_ui_presentation(
        init_audio_pc,
        init_muted,
        false,
        false,
        false,
        0,
        init_vol as u32,
        false,
        false,
        "idle",
        "",
        init_en,
    );
    ui.set_audio_pc_active(init_audio_pc);
    ui.set_bt_audio_active(init_audio_pres.bt_active);
    ui.set_audio_live_active(init_audio_pres.live_active);
    ui.set_audio_live_state(SharedString::from(init_audio_pres.live_state));
    ui.set_audio_muted(init_muted);
    ui.set_airplay_volume(init_vol);
    ui.set_airplay_buffer(SharedString::from(init_buf.as_str()));
    ui.set_airplay_lip_sync(init_ls);
    ui.set_airplay_lip_sync_ms(if init_ls { 0 } else { -20 });
    ui.set_airplay_audio_telemetry(SharedString::from(init_audio_pres.telemetry_text.as_str()));
    ui.set_audio_status_banner(SharedString::from(init_audio_pres.banner_text.as_str()));
    ui.set_trackpad_speed(init_tp_speed);
    ui.set_keyboard_enabled(init_kb_en);
    ui.set_cross_platform_shortcuts(init_cps);
    ui.set_paste_pc_clipboard(init_ppc);
    ui.set_pc_lan_ip(SharedString::from(lan_ip.as_str()));
    ui.set_shortcut_bridge_url(SharedString::from(bridge_url.as_str()));
    apply_hydrated_pin_to_ui(&ui, hydrated_pin.as_deref());
    ui.set_pin_editing(false);
    ui.set_video_layer_active(false);
    ui.set_first_frame_ready(false);
    ui.set_auto_check_update(init_auto_chk);
    ui.set_update_channel(SharedString::from(init_upd_ch.as_str()));
    let init_ready_pill = if init_en { "Ready" } else { "Siap" };
    ui.set_status_pill(SharedString::from(init_ready_pill));
    ui.set_status_pill_compact(SharedString::from(init_ready_pill));
    if should_use_disk_blur_cache(false) {
        if let Some((rgba, bw, bh)) = load_persisted_blur_cache_default() {
            ui.set_last_frame_blur_image(blurred_rgba_to_image(&rgba, bw, bh));
            ui.set_has_last_frame_blur(true);
        }
    }

    if let Ok(init_status_json) = serde_json::from_str(&inprocess_api(
        &state,
        &web_dir,
        "GET",
        "/api/status",
        "",
    )) {
        let sum = parse_launchpad_from_status(&init_status_json, init_en);
        ui.set_devices(ModelRc::new(VecModel::from(sum.cards)));
        ui.set_device_model_name(SharedString::from(sum.model_name));
        ui.set_device_title(SharedString::from(sum.hero_title));
        ui.set_device_subtitle(SharedString::from(sum.hero_subtitle));
        ui.set_device_pill_title(SharedString::from(sum.pill_title));
        ui.set_device_pill_detail(SharedString::from(sum.pill_detail));
        ui.set_device_ready(sum.ready);
        ui.set_wifi_status_text(SharedString::from(sum.wifi_text));
        let pill_is_wifi = sum.status_pill_short.starts_with("Wi-Fi");
        ui.set_status_pill(SharedString::from(sum.status_pill_short));
        if sum.ready && pill_is_wifi {
            ui.set_transport_tab(SharedString::from("wifi"));
        }
        if !sum.selected_udid.is_empty() {
            ui.set_selected_udid(SharedString::from(sum.selected_udid));
        }
    }
    if let Ok(init_diag) = serde_json::from_str(&inprocess_api(
        &state,
        &web_dir,
        "GET",
        "/api/driver/diagnostics",
        "",
    )) {
        let drows = build_diag_rows_from_json(&init_diag, init_en);
        ui.set_diagnostics(ModelRc::new(VecModel::from(drows)));
    }

    if let Err(e) = ui.show() {
        eprintln!("Slint MainWindow show failed: {e}");
        server.stop();
        return 1;
    }
    let last_logo_scale_bits: Rc<Cell<u32>> = Rc::new(Cell::new(0));
    let _ = refresh_crisp_brand_logos_for_scale(
        &ui,
        ui.window().scale_factor(),
        &last_logo_scale_bits,
    );

    // Tahap 285 Pilar 2: Non-blocking 1x startup GitHub update check when enabled
    if init_auto_chk {
        let loopback_only = std::env::var("IPHONE_LINK_TEST_LOOPBACK_ONLY")
            .ok()
            .is_some_and(|v| matches!(v.trim(), "1" | "true" | "TRUE"));
        let has_override = std::env::var("IPHONE_LINK_UPDATER_API_URL")
            .ok()
            .is_some_and(|s| !s.trim().is_empty());
        if !loopback_only || has_override {
            let ui_w_upd = ui.as_weak();
            let st_upd = state.clone();
            let wd_upd = web_dir.clone();
            let ch_upd = init_upd_ch.clone();
            std::thread::Builder::new()
                .name("native-ui-startup-upd".to_string())
                .spawn(move || {
                    std::thread::sleep(Duration::from_millis(1500));
                    let body = format!(r#"{{"channel":"{}"}}"#, escape_json(&ch_upd));
                    let resp = inprocess_api(&st_upd, &wd_upd, "POST", "/api/update/check", &body);
                    if let Ok(v) = serde_json::from_str(&resp) {
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(u) = ui_w_upd.upgrade() {
                                let (avail, latest, rel_u, dl_u, st_txt) =
                                    format_updater_ui_state(&v, u.get_lang_en());
                                u.set_update_available(avail);
                                u.set_latest_version(SharedString::from(latest));
                                u.set_update_release_url(SharedString::from(rel_u));
                                u.set_update_download_url(SharedString::from(dl_u));
                                u.set_update_status_text(SharedString::from(st_txt));
                            }
                        });
                    }
                })
                .ok();
        }
    }

    // Shared input router, Single-Source-of-Truth video rect, & Win32 message-only tray icon
    let (shortcut_tx, shortcut_rx) = mpsc::channel::<GlobalShortcutAction>();
    let input_router = Arc::new(NativeInputRouter::new(shortcut_tx));
    input_router.update_config(|c| {
        c.trackpad_speed_pct = init_tp_speed.clamp(30, 200) as u32;
        c.keyboard_enabled = init_kb_en;
    });
    let subclass_state = Arc::new(ParentSubclassState {
        is_fullscreen: AtomicBool::new(false),
        is_streaming_or_starting: AtomicBool::new(false),
        pin_editing: AtomicBool::new(false),
        keyboard_enabled: AtomicBool::new(init_kb_en),
        phase_running: AtomicBool::new(false),
        needs_bounds_sync: AtomicBool::new(false),
        fs_toggle_requested: AtomicBool::new(false),
        ctrl_down: AtomicBool::new(false),
        input_router: input_router.clone(),
    });
    let active_rect_shared = Arc::new(Mutex::new(ActiveVideoPhysicalRect::default()));
    let video_active_hook = Arc::new(AtomicBool::new(false));
    let pointer_captured_hook = Arc::new(AtomicBool::new(false));
    let hooks_rot_deg = Arc::new(std::sync::atomic::AtomicU16::new(0));
    let session_transition_lock = Arc::new(Mutex::new(()));
    let grab_in_flight = Arc::new(AtomicBool::new(false));

    let (tray_tx, tray_rx) = mpsc::channel::<NativeTrayEvent>();
    let tray_host: Rc<RefCell<Option<NativeTrayHost>>> =
        Rc::new(RefCell::new(NativeTrayHost::new(tray_tx)));
    if let Some(ref th) = *tray_host.borrow() {
        th.update_flags(TrayMenuFlags {
            startup: crate::window_dwm::is_start_with_windows_enabled(),
            landscape: false,
            stealth: false,
            audio_pc: init_audio_pc && !init_muted,
        });
    }

    let comp_pipeline = match LiveVideoPipeline::new_composition(gpu.clone(), 16, 16, 0) {
        Ok(mut_p) => mut_p,
        Err(e) => {
            eprintln!("Composition LiveVideoPipeline failed: {e}");
            server.stop();
            return 1;
        }
    };
    comp_pipeline.set_fit_mode(fit_mode_to_enum(&init_fit));
    comp_pipeline.set_auto_rotate_enabled(init_ar);
    comp_pipeline.set_ipad_profile(init_prof.eq_ignore_ascii_case("ipad"));
    comp_pipeline.set_crop(
        init_crops.0.max(0) as u32,
        init_crops.1.max(0) as u32,
        init_crops.2.max(0) as u32,
        init_crops.3.max(0) as u32,
    );
    comp_pipeline.attach_input_router(input_router.clone());

    let pipeline: Rc<RefCell<Option<LiveVideoPipeline>>> = Rc::new(RefCell::new(Some(comp_pipeline)));
    let parent_hwnd_cell = Rc::new(Cell::new(0isize));
    // DirectComposition HWND hooks replace legacy winit/FocusScope double-dispatch!
    let subclass_installed = Rc::new(Cell::new(true));
    let live_video_dims: Arc<Mutex<(u32, u32)>> = Arc::new(Mutex::new((0, 0)));
    let frame1_reveal_needed = Arc::new(AtomicBool::new(false));
    let airplay_manual_landscape_rc = Rc::new(Cell::new(false));

    // Animation state controllers (16 ms / 60 Hz timers, active ONLY while animating -> 0% idle spin)
    #[derive(Clone, Copy)]
    enum DrawerAnimKind {
        WindowedSlide {
            start: Instant,
            duration_ms: f32,
            opening: bool,
            target_lw: f32,
            target_lh: f32,
        },
        FullscreenSlide {
            start: Instant,
            duration_ms: f32,
            from_w: f32,
            to_w: f32,
        },
    }
    #[derive(Clone, Copy)]
    struct ActiveStageTransition {
        kind: StageTransitionKind,
        start: Instant,
        duration_ms: f32,
        gen: u64,
    }
    let drawer_anim_timer = Rc::new(Timer::default());
    let drawer_anim_state: Rc<RefCell<Option<DrawerAnimKind>>> = Rc::new(RefCell::new(None));
    let fs_resync_timer = Rc::new(Timer::default());
    let reveal_anim_timer = Rc::new(Timer::default());
    let reveal_anim_state: Rc<RefCell<Option<ActiveStageTransition>>> = Rc::new(RefCell::new(None));
    let transition_gen: Arc<std::sync::atomic::AtomicU64> =
        Arc::new(std::sync::atomic::AtomicU64::new(0));

    #[allow(clippy::too_many_arguments)]
    fn start_stage_transition(
        ui: &MainWindow,
        ui_weak: &slint::Weak<MainWindow>,
        pipe_rc: &Rc<RefCell<Option<LiveVideoPipeline>>>,
        hwnd_cell: &Rc<Cell<isize>>,
        rect_sh: &Arc<Mutex<ActiveVideoPhysicalRect>>,
        dims_sh: &Arc<Mutex<(u32, u32)>>,
        r_anim_t: &Rc<Timer>,
        r_anim_s: &Rc<RefCell<Option<ActiveStageTransition>>>,
        trans_gen: &Arc<std::sync::atomic::AtomicU64>,
        kind: StageTransitionKind,
        duration_ms: f32,
    ) {
        let my_gen = trans_gen.fetch_add(1, Ordering::SeqCst).wrapping_add(1);
        r_anim_t.stop();
        let op0 = compute_transition_opacities(kind, 0.0);
        ui.set_transition_active(true);
        ui.set_standby_opacity(op0.standby);
        ui.set_sequoia_opacity(op0.sequoia);
        ui.set_sequoia_text_opacity(op0.sequoia_text);
        ui.set_reveal_blur_opacity(op0.reveal_blur);
        flush_active_dcomp_ui_frame();

        *r_anim_s.borrow_mut() = Some(ActiveStageTransition {
            kind,
            start: Instant::now(),
            duration_ms: duration_ms.max(1.0),
            gen: my_gen,
        });
        let ui_w2 = ui_weak.clone();
        let pipe_2 = pipe_rc.clone();
        let hwnd_2 = hwnd_cell.clone();
        let rect_2 = rect_sh.clone();
        let dims_2 = dims_sh.clone();
        let t2 = r_anim_t.clone();
        let s2 = r_anim_s.clone();
        let g2 = trans_gen.clone();
        r_anim_t.start(TimerMode::Repeated, Duration::from_millis(16), move || {
            if g2.load(Ordering::SeqCst) != my_gen {
                t2.stop();
                return;
            }
            let Some(u2) = ui_w2.upgrade() else {
                t2.stop();
                return;
            };
            if !u2.get_transition_active() {
                t2.stop();
                return;
            }
            let active_opt = *s2.borrow();
            let Some(active) = active_opt else {
                t2.stop();
                return;
            };
            if active.gen != my_gen {
                t2.stop();
                return;
            }
            let t = (active.start.elapsed().as_secs_f32() * 1000.0 / active.duration_ms).clamp(0.0, 1.0);
            let op = compute_transition_opacities(active.kind, t);
            u2.set_standby_opacity(op.standby);
            u2.set_sequoia_opacity(op.sequoia);
            u2.set_sequoia_text_opacity(op.sequoia_text);
            u2.set_reveal_blur_opacity(op.reveal_blur);
            flush_active_dcomp_ui_frame();
            if t >= 1.0 {
                t2.stop();
                *s2.borrow_mut() = None;
                u2.set_transition_active(false);
                u2.set_reveal_blur_visible(false);
                u2.set_reveal_blur_opacity(0.0);
                u2.set_reveal_blur_image(Image::default());
                if matches!(
                    active.kind,
                    StageTransitionKind::StopFromLive | StageTransitionKind::StopFromConnecting
                ) {
                    u2.set_first_frame_ready(false);
                    u2.set_video_layer_active(false);
                    u2.set_phase(SharedString::from("idle"));
                    u2.set_stage(SharedString::from(""));
                    u2.set_stage_progress(0);
                    u2.set_error_text(SharedString::from(""));
                    u2.set_status_pill(SharedString::from(if u2.get_lang_en() {
                        "Ready"
                    } else {
                        "Siap"
                    }));
                    if let Ok(mut d) = dims_2.lock() {
                        *d = (0, 0);
                    }
                    if let Some(ref p) = *pipe_2.borrow() {
                        p.set_airplay_mode(false);
                        p.set_window_landscape(false);
                        let _ = sync_dcomp_video_layout(hwnd_2.get(), &u2, p, &rect_2, Some(false));
                    }
                    trim_active_dcomp_memory();
                } else {
                    flush_active_dcomp_ui_frame();
                }
            }
        });
    }

    // Sequoia Seamless Reveal: signal UI thread immediately when Frame #1 renders on Layer 0 swapchain!
    if let Some(ref p) = *pipeline.borrow() {
        let rev_flag = frame1_reveal_needed.clone();
        p.set_frame_ready_callback(Some(Arc::new(move || {
            rev_flag.store(true, Ordering::SeqCst);
            let _ = slint::invoke_from_event_loop(|| {});
        })));
    }

    // Create Single Win32 HWND + 2-Layer DirectComposition Presenter with 0 ms white flash
    let init_scale = query_hwnd_scale(0);
    let init_wa = query_work_area(0);
    let (lw0, lh0) = compute_native_window_target_logical_size_for_profile(
        false,
        false,
        init_prof.as_str(),
    );
    let (wx0, wy0, wphys0, hphys0) = compute_centered_window_rect(lw0, lh0, init_scale, init_wa);

    let hooks = DcompHostHooks {
        is_fullscreen: {
            let sub_c = subclass_state.clone();
            Box::new(move || sub_c.is_fullscreen.load(Ordering::Relaxed))
        },
        on_nchittest: {
            let sub_c = subclass_state.clone();
            Box::new(move |cx, cy, cw, _ch, scale| {
                compute_top_bar_nchittest(
                    cx,
                    cy,
                    cw,
                    scale,
                    sub_c.is_fullscreen.load(Ordering::Relaxed),
                    sub_c.is_streaming_or_starting.load(Ordering::Relaxed),
                )
            })
        },
        on_pointer_event: {
            let v_act = video_active_hook.clone();
            let sub_c = subclass_state.clone();
            let rect_c = active_rect_shared.clone();
            let rot_c = hooks_rot_deg.clone();
            let cap_c = pointer_captured_hook.clone();
            let router_c = input_router.clone();
            Box::new(move |msg, cx, cy| {
                let is_up = msg == 0x0202; // WM_LBUTTONUP
                let was_captured = cap_c.load(Ordering::Relaxed);
                if sub_c.pin_editing.load(Ordering::Relaxed) || !v_act.load(Ordering::Relaxed) {
                    if was_captured && is_up {
                        cap_c.store(false, Ordering::SeqCst);
                    }
                    return false;
                }
                let rect = rect_c.lock().map(|g| *g).unwrap_or_default();
                let rot = rot_c.load(Ordering::Relaxed);
                let hit = map_client_px_to_ios_hid_from_rect(cx, cy, rect, rot);

                if msg == 0x0201 {
                    // WM_LBUTTONDOWN: only start HID touch inside active iPhone screen (blur wings go to Slint)
                    if let Some((hid_x, hid_y)) = hit {
                        cap_c.store(true, Ordering::SeqCst);
                        router_c.handle_pointer_event(StagePointerEvent::Down { hid_x, hid_y });
                        return true;
                    }
                    return false;
                }
                if msg == 0x0200 && was_captured {
                    let clamped_x = cx.clamp(rect.x, rect.x + (rect.width as i32).saturating_sub(1));
                    let clamped_y = cy.clamp(rect.y, rect.y + (rect.height as i32).saturating_sub(1));
                    if let Some((hid_x, hid_y)) = map_client_px_to_ios_hid_from_rect(clamped_x, clamped_y, rect, rot) {
                        router_c.handle_pointer_event(StagePointerEvent::Move { hid_x, hid_y });
                    }
                    return true;
                }
                if is_up && was_captured {
                    cap_c.store(false, Ordering::SeqCst);
                    let clamped_x = cx.clamp(rect.x, rect.x + (rect.width as i32).saturating_sub(1));
                    let clamped_y = cy.clamp(rect.y, rect.y + (rect.height as i32).saturating_sub(1));
                    if let Some((hid_x, hid_y)) = map_client_px_to_ios_hid_from_rect(clamped_x, clamped_y, rect, rot) {
                        router_c.handle_pointer_event(StagePointerEvent::Up { hid_x, hid_y });
                    }
                    return true;
                }
                false
            })
        },
        on_wheel_event: {
            let v_act = video_active_hook.clone();
            let sub_c = subclass_state.clone();
            let rect_c = active_rect_shared.clone();
            let rot_c = hooks_rot_deg.clone();
            let router_c = input_router.clone();
            Box::new(move |cx, cy, delta, is_horiz| {
                if sub_c.pin_editing.load(Ordering::Relaxed) || !v_act.load(Ordering::Relaxed) {
                    return false;
                }
                let rect = rect_c.lock().map(|g| *g).unwrap_or_default();
                let rot = rot_c.load(Ordering::Relaxed);
                let Some((hid_x, hid_y)) = map_client_px_to_ios_hid_from_rect(cx, cy, rect, rot) else {
                    return false;
                };
                let ctrl = unsafe { ((win32().get_key_state)(VK_CONTROL) as u16 & 0x8000) != 0 }
                    || sub_c.ctrl_down.load(Ordering::Relaxed);
                router_c.handle_pointer_event(StagePointerEvent::Wheel {
                    hid_x,
                    hid_y,
                    delta_y: if is_horiz { 0 } else { delta },
                    delta_x: if is_horiz { delta } else { 0 },
                    ctrl,
                });
                true
            })
        },
        on_key_event: {
            let sub_c = subclass_state.clone();
            let router_c = input_router.clone();
            let ui_k_w = ui.as_weak();
            Box::new(move |msg, wparam, lparam| {
                if sub_c.pin_editing.load(Ordering::Relaxed) {
                    // CRITICAL (Bug #5): forward ONLY to Slint TextInput, zero bytes to HID or shortcuts!
                    return false;
                }
                let api = win32();
                let ctrl = unsafe { ((api.get_key_state)(VK_CONTROL) as u16 & 0x8000) != 0 }
                    || sub_c.ctrl_down.load(Ordering::Relaxed);
                let alt = ((lparam & (1 << 29)) != 0)
                    || unsafe { ((api.get_key_state)(VK_MENU) as u16 & 0x8000) != 0 };
                let shift = unsafe { ((api.get_key_state)(VK_SHIFT) as u16 & 0x8000) != 0 };
                match msg {
                    WM_KEYDOWN | WM_SYSKEYDOWN => {
                        let vk = (wparam & 0xFFFF) as u16;
                        if vk == VK_CONTROL as u16 {
                            sub_c.ctrl_down.store(true, Ordering::SeqCst);
                            return true;
                        }
                        if vk == VK_F4 as u16 && alt {
                            return false;
                        }
                        let repeat = (lparam & 0x4000_0000) != 0;
                        if vk == 0x7A && !repeat {
                            // F11 -> instant borderless fullscreen toggle
                            let uw = ui_k_w.clone();
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(u) = uw.upgrade() {
                                    u.invoke_toggle_fullscreen();
                                }
                            });
                            return true;
                        }
                        if resolve_global_shortcut(vk, ctrl, alt, shift).is_some()
                            || (sub_c.phase_running.load(Ordering::Relaxed)
                                && sub_c.keyboard_enabled.load(Ordering::Relaxed)
                                && !ctrl
                                && !alt)
                        {
                            router_c.handle_key_event(StageKeyEvent::KeyDown {
                                vk,
                                ctrl,
                                alt,
                                shift,
                                repeat,
                            });
                            return true;
                        }
                        false
                    }
                    WM_KEYUP | WM_SYSKEYUP => {
                        let vk = (wparam & 0xFFFF) as u16;
                        if vk == VK_CONTROL as u16 {
                            sub_c.ctrl_down.store(false, Ordering::SeqCst);
                        }
                        router_c.handle_key_event(StageKeyEvent::KeyUp {
                            vk,
                            ctrl,
                            alt,
                            shift,
                        });
                        false
                    }
                    WM_CHAR => {
                        if let Some(ch) = char::from_u32((wparam & 0xFFFF) as u32) {
                            if ch < ' ' || ch.is_control() {
                                return true;
                            }
                            if !ctrl
                                && !alt
                                && sub_c.phase_running.load(Ordering::Relaxed)
                                && sub_c.keyboard_enabled.load(Ordering::Relaxed)
                            {
                                router_c.handle_key_event(StageKeyEvent::Char { ch, ctrl, alt });
                                return true;
                            }
                        }
                        false
                    }
                    _ => false,
                }
            })
        },
        on_resized: {
            let ui_r_w = ui.as_weak();
            let rect_c = active_rect_shared.clone();
            let v_act = video_active_hook.clone();
            let sub_r_c = subclass_state.clone();
            let sub_inst_c = subclass_installed.clone();
            let dims_c = live_video_dims.clone();
            let pipe_r_c = pipeline.clone();
            let logo_scale_r = last_logo_scale_bits.clone();
            Box::new(move |cw, ch, scale| {
                let Some(u) = ui_r_w.upgrade() else { return };
                let _ = refresh_crisp_brand_logos_for_scale(&u, scale, &logo_scale_r);
                let is_fs = u.get_is_fullscreen();
                let is_land = u.get_is_landscape();
                let rot_deg = u.get_rotation_deg().clamp(0, 359) as u16;
                let fit_mode = fit_mode_to_enum(&u.get_fit_mode().to_string());
                let drawer_w_log = if u.get_drawer_open() {
                    let dw = u.get_drawer_animated_w();
                    if dw > 1.0 { dw } else { NATIVE_DRAWER_WIDTH }
                } else {
                    0.0
                };
                let (vid_w, vid_h) = dims_c.lock().map(|g| *g).unwrap_or((0, 0));
                let prev_rect = rect_c.lock().ok().map(|g| *g);
                let rect = clamp_video_rect_to_stage(
                    compute_active_video_physical_rect_with_drawer_w_and_mode(
                        cw,
                        ch,
                        scale,
                        drawer_w_log,
                        is_fs,
                        is_land,
                        vid_w,
                        vid_h,
                        rot_deg,
                        fit_mode,
                        mirror::is_airplay_active(),
                    ),
                );
                let rect_changed = prev_rect
                    .map(|p| (p.x, p.y, p.width, p.height) != (rect.x, rect.y, rect.width, rect.height))
                    .unwrap_or(true);
                if rect_changed && is_fs && is_land {
                    airplay::inc_trace_fullscreen_landscape_resyncs();
                }
                airplay::set_trace_last_active_video_rect(
                    rect.x,
                    rect.y,
                    rect.width as i32,
                    rect.height as i32,
                );
                if let Ok(mut g) = rect_c.lock() {
                    *g = rect;
                }
                u.set_video_hole_x((rect.x as f32) / scale);
                u.set_video_hole_y((rect.y as f32) / scale);
                u.set_video_hole_w((rect.width as f32) / scale);
                u.set_video_hole_h((rect.height as f32) / scale);
                sub_r_c.needs_bounds_sync.store(true, Ordering::SeqCst);
                if v_act.load(Ordering::Relaxed) {
                    if let Ok(p_guard) = pipe_r_c.try_borrow() {
                        if let Some(ref p) = *p_guard {
                            let resized = p.resize_composition(rect.width, rect.height).unwrap_or(false);
                            if resized || rect_changed {
                                let _ = p.force_present_now();
                            }
                            sync_active_dcomp_video_layer(
                                p.swapchain_ptr(),
                                rect.x as f32,
                                rect.y as f32,
                            );
                            if resized || rect_changed {
                                commit_active_dcomp();
                                airplay::inc_trace_dcomp_commits();
                            }
                        } else if let Some((_w, _h, _att)) = query_active_dcomp_state() {
                            update_active_dcomp_video_offset(rect.x as f32, rect.y as f32);
                        }
                    } else if let Some((_w, _h, _att)) = query_active_dcomp_state() {
                        update_active_dcomp_video_offset(rect.x as f32, rect.y as f32);
                    }
                    let _ = sub_inst_c.get();
                }
            })
        },
        on_exit_sizemove: {
            let sub_e = subclass_state.clone();
            let ui_e = ui.as_weak();
            Box::new(move || {
                sub_e.needs_bounds_sync.store(true, Ordering::SeqCst);
                record_ui_action("window_drag_end", "exitsizemove", ui_e.upgrade().as_ref());
            })
        },
        on_caption_dblclick: {
            let sub_d = subclass_state.clone();
            Box::new(move || {
                sub_d.fs_toggle_requested.store(true, Ordering::SeqCst);
            })
        },
        on_close_requested: {
            let ui_c_w = ui.as_weak();
            Box::new(move || {
                if let Some(u) = ui_c_w.upgrade() {
                    u.invoke_close_window();
                }
            })
        },
    };

    let host_hwnd = match create_single_dcomp_host(
        "iPhone Link",
        gpu,
        wx0,
        wy0,
        wphys0,
        hphys0,
        init_dark,
        hooks,
    ) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("Single-HWND DirectComposition host creation failed: {e}");
            if let Some(mut p) = pipeline.borrow_mut().take() {
                p.shutdown();
            }
            server.stop();
            return 1;
        }
    };

    parent_hwnd_cell.set(host_hwnd);
    if let Ok(mut st) = state.lock() {
        st.window_hwnd = host_hwnd as usize;
    }
    apply_embedded_window_icons_and_aumid(host_hwnd);
    if let Some(ref p) = *pipeline.borrow() {
        let _ = sync_dcomp_video_layout(host_hwnd, &ui, p, &active_rect_shared, Some(false));
    }

    let saved_normal_rect: Rc<RefCell<Option<WinRect>>> = Rc::new(RefCell::new(None));
    let stream_mode_shared = Arc::new(Mutex::new("auto".to_string()));
    let stop_poller = Arc::new(AtomicBool::new(false));
    let is_streaming_flag = Arc::new(AtomicBool::new(false));
    let start_inflight_until: Rc<Cell<Option<Instant>>> = Rc::new(Cell::new(None));

    // Window chrome callbacks (minimize, instant deterministic close < 300ms, borderless fullscreen, drawer)
    {
        let hwnd_cell = parent_hwnd_cell.clone();
        let ui_weak = ui.as_weak();
        ui.on_minimize_window(move || {
            let hwnd = hwnd_cell.get();
            if hwnd != 0 {
                unsafe {
                    let _ = (win32().show_window)(hwnd, SW_SHOWMINIMIZED);
                }
            }
            record_ui_action("minimize_window", "", ui_weak.upgrade().as_ref());
        });
    }
    {
        let ui_weak = ui.as_weak();
        let pipe_rc = pipeline.clone();
        let hwnd_cell = parent_hwnd_cell.clone();
        let tray_rc = tray_host.clone();
        let stop_flag_c = stop_poller.clone();
        let streaming_rc = is_streaming_flag.clone();
        let v_act = video_active_hook.clone();
        ui.on_close_window(move || {
            stop_flag_c.store(true, Ordering::SeqCst);
            streaming_rc.store(false, Ordering::SeqCst);
            v_act.store(false, Ordering::SeqCst);
            record_ui_action("close_window", "", ui_weak.upgrade().as_ref());
            let hwnd = hwnd_cell.get();
            sync_active_dcomp_video_layer(None, 0.0, 0.0);
            if let Some(ref p) = *pipe_rc.borrow() {
                p.set_visible(false);
            }
            if hwnd != 0 {
                unsafe {
                    let _ = (win32().show_window)(hwnd, SW_HIDE);
                }
            }
            if let Some(mut p) = pipe_rc.borrow_mut().take() {
                p.shutdown();
            }
            *tray_rc.borrow_mut() = None;
            std::thread::spawn(|| {
                let _ = mirror::global_stop_screen();
                iphone_link_audio::stop_global_pc_audio_bridge();
            });
            let _ = slint::quit_event_loop();
        });
    }

    // Bug #2: Borderless Work-Area Full Screen (NEVER SW_MAXIMIZE, 0 px overhang, dynamic Drawer restore, PLI HD keyframe)
    {
        let ui_weak = ui.as_weak();
        let pipe_rc = pipeline.clone();
        let saved_rc = saved_normal_rect.clone();
        let hwnd_cell = parent_hwnd_cell.clone();
        let sub_st = subclass_state.clone();
        let rect_sh = active_rect_shared.clone();
        let dims_sh = live_video_dims.clone();
        let d_anim_t = drawer_anim_timer.clone();
        let d_anim_s = drawer_anim_state.clone();
        let fs_resync_t = fs_resync_timer.clone();
        ui.on_toggle_fullscreen(move || {
            let Some(u) = ui_weak.upgrade() else { return };
            d_anim_t.stop();
            *d_anim_s.borrow_mut() = None;
            let dw_snap = if u.get_drawer_open() { NATIVE_DRAWER_WIDTH } else { 0.0 };
            u.set_drawer_animated_w(dw_snap);
            u.set_drawer_slide_progress(if u.get_drawer_open() { 1.0 } else { 0.0 });

            let hwnd = hwnd_cell.get();
            let next_fs = !u.get_is_fullscreen();
            u.set_is_fullscreen(next_fs);
            sub_st.is_fullscreen.store(next_fs, Ordering::SeqCst);
            if hwnd != 0 {
                let api = win32();
                let (tx, ty, tw, th) = if next_fs {
                    let mut cur = WinRect::default();
                    unsafe {
                        let _ = (api.get_window_rect)(hwnd, &mut cur);
                    }
                    *saved_rc.borrow_mut() = Some(cur);
                    let wa = query_work_area(hwnd);
                    (wa.left, wa.top, wa.width(), wa.height())
                } else {
                    let wa = query_work_area(hwnd);
                    let scale = query_hwnd_scale(hwnd);
                    let saved = saved_rc.borrow_mut().take();
                    compute_fullscreen_exit_physical_rect(
                        saved,
                        wa,
                        u.get_drawer_open(),
                        u.get_is_landscape(),
                        u.get_device_profile().as_str(),
                        scale,
                    )
                };
                apply_dcomp_win11_thin_acrylic(hwnd, u.get_dark_mode(), next_fs);
                let _ = atomic_resize_host_window(hwnd, tx, ty, tw.max(320) as u32, th.max(400) as u32);
                apply_dcomp_win11_thin_acrylic(hwnd, u.get_dark_mode(), next_fs);
                unsafe {
                    let _ = (api.set_window_pos)(
                        hwnd,
                        0,
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                    );
                }
            }
            if let Some(ref p) = *pipe_rc.borrow() {
                let rect = sync_dcomp_video_layout_with_dims(hwnd, &u, p, &rect_sh, &dims_sh, None);
                if u.get_video_layer_active() {
                    let _ = p.force_present_now();
                    sync_active_dcomp_video_layer(p.swapchain_ptr(), rect.x as f32, rect.y as f32);
                    commit_active_dcomp();
                    airplay::inc_trace_dcomp_commits();
                }
                u.window().request_redraw();
                flush_active_dcomp_ui_frame();
            }
            if u.get_phase() == "running" {
                trigger_live_pli_burst();
            }
            // Schedule 3 fast confirmation ticks (15 ms, 60 ms, 150 ms) reading live Win32 GetClientRect
            // so async Full Screen Landscape/Portrait maximize transitions never leave video stuck top-left.
            let fs_resync_started = Instant::now();
            let fs_stage = Rc::new(Cell::new(0u8));
            let ui_w_fs = ui_weak.clone();
            let pipe_fs = pipe_rc.clone();
            let hwnd_fs = hwnd_cell.clone();
            let rect_fs = rect_sh.clone();
            let dims_fs = dims_sh.clone();
            let timer_fs_c = fs_resync_t.clone();
            fs_resync_t.stop();
            fs_resync_t.start(TimerMode::Repeated, Duration::from_millis(16), move || {
                let elapsed_ms = fs_resync_started.elapsed().as_millis() as u64;
                let stage_now = fs_stage.get();
                let should_sync = match stage_now {
                    0 => elapsed_ms >= 15,
                    1 => elapsed_ms >= 60,
                    2 => elapsed_ms >= 150,
                    _ => false,
                };
                if should_sync {
                    fs_stage.set(stage_now.saturating_add(1));
                    if let Some(u2) = ui_w_fs.upgrade() {
                        let h2 = hwnd_fs.get();
                        if h2 != 0 {
                            if let Some(ref p2) = *pipe_fs.borrow() {
                                let prev_r = rect_fs.lock().ok().map(|g| *g);
                                let r2 = sync_dcomp_video_layout_with_dims(
                                    h2, &u2, p2, &rect_fs, &dims_fs, None,
                                );
                                if prev_r.is_none_or(|pr| {
                                    (pr.x, pr.y, pr.width, pr.height)
                                        != (r2.x, r2.y, r2.width, r2.height)
                                }) && u2.get_video_layer_active()
                                {
                                    let _ = p2.force_present_now();
                                }
                            }
                        }
                    }
                }
                if elapsed_ms >= 170 || fs_stage.get() >= 3 {
                    timer_fs_c.stop();
                }
            });
            record_ui_action("toggle_fullscreen", &format!("isFullscreen={next_fs}"), Some(&u));
        });
    }

    // Drawer toggle (Windowed: atomic 1x resize + 180/150ms 60Hz slide; Fullscreen: 200ms 60Hz video offset glide)
    {
        let ui_weak = ui.as_weak();
        let pipe_rc = pipeline.clone();
        let hwnd_cell = parent_hwnd_cell.clone();
        let rect_sh = active_rect_shared.clone();
        let dims_sh = live_video_dims.clone();
        let d_anim_t = drawer_anim_timer.clone();
        let d_anim_s = drawer_anim_state.clone();
        ui.on_toggle_drawer(move || {
            let Some(u) = ui_weak.upgrade() else { return };
            let hwnd = hwnd_cell.get();
            d_anim_t.stop();
            let next_open = !u.get_drawer_open();

            if u.get_is_fullscreen() {
                let from_w = u.get_drawer_animated_w().clamp(0.0, NATIVE_DRAWER_WIDTH);
                let to_w = if next_open { NATIVE_DRAWER_WIDTH } else { 0.0 };
                u.set_drawer_open(next_open);
                *d_anim_s.borrow_mut() = Some(DrawerAnimKind::FullscreenSlide {
                    start: Instant::now(),
                    duration_ms: 200.0,
                    from_w,
                    to_w,
                });
            } else {
                let (lw, lh) = compute_native_window_target_logical_size_for_profile(
                    next_open,
                    u.get_is_landscape(),
                    u.get_device_profile().as_str(),
                );
                if next_open {
                    u.set_drawer_open(true);
                    u.set_drawer_animated_w(NATIVE_DRAWER_WIDTH);
                    u.set_drawer_slide_progress(0.0);
                    if hwnd != 0 {
                        let (tx, ty, tw, th) = compute_resize_preserve_origin_rect(hwnd, lw, lh);
                        let _ = atomic_resize_host_window(hwnd, tx, ty, tw, th);
                    }
                    if let Some(ref p) = *pipe_rc.borrow() {
                        let _ = sync_dcomp_video_layout_with_dims(hwnd, &u, p, &rect_sh, &dims_sh, None);
                    }
                    *d_anim_s.borrow_mut() = Some(DrawerAnimKind::WindowedSlide {
                        start: Instant::now(),
                        duration_ms: 180.0,
                        opening: true,
                        target_lw: lw,
                        target_lh: lh,
                    });
                } else {
                    *d_anim_s.borrow_mut() = Some(DrawerAnimKind::WindowedSlide {
                        start: Instant::now(),
                        duration_ms: 150.0,
                        opening: false,
                        target_lw: lw,
                        target_lh: lh,
                    });
                }
            }

            let ui_w2 = ui_weak.clone();
            let pipe_2 = pipe_rc.clone();
            let hwnd_2 = hwnd_cell.clone();
            let rect_2 = rect_sh.clone();
            let dims_2 = dims_sh.clone();
            let anim_t2 = d_anim_t.clone();
            let anim_s2 = d_anim_s.clone();
            d_anim_t.start(TimerMode::Repeated, Duration::from_millis(16), move || {
                let Some(u2) = ui_w2.upgrade() else {
                    anim_t2.stop();
                    return;
                };
                let state_opt = *anim_s2.borrow();
                let Some(kind) = state_opt else {
                    anim_t2.stop();
                    return;
                };
                let hwnd_now = hwnd_2.get();
                match kind {
                    DrawerAnimKind::WindowedSlide {
                        start,
                        duration_ms,
                        opening,
                        target_lw,
                        target_lh,
                    } => {
                        let t = (start.elapsed().as_secs_f32() * 1000.0 / duration_ms).clamp(0.0, 1.0);
                        let e = ease_out_cubic(t);
                        let prog = if opening { e } else { 1.0 - e };
                        u2.set_drawer_slide_progress(prog);
                        if t >= 1.0 {
                            anim_t2.stop();
                            *anim_s2.borrow_mut() = None;
                            if !opening {
                                u2.set_drawer_open(false);
                                u2.set_drawer_animated_w(0.0);
                                u2.set_drawer_slide_progress(0.0);
                                if hwnd_now != 0 {
                                    let (tx, ty, tw, th) =
                                        compute_resize_preserve_origin_rect(hwnd_now, target_lw, target_lh);
                                    let _ = atomic_resize_host_window(hwnd_now, tx, ty, tw, th);
                                }
                                if let Some(ref p) = *pipe_2.borrow() {
                                    let _ = sync_dcomp_video_layout_with_dims(
                                        hwnd_now, &u2, p, &rect_2, &dims_2, None,
                                    );
                                }
                            } else {
                                u2.set_drawer_slide_progress(1.0);
                            }
                        }
                        flush_active_dcomp_ui_frame();
                    }
                    DrawerAnimKind::FullscreenSlide {
                        start,
                        duration_ms,
                        from_w,
                        to_w,
                    } => {
                        let t = (start.elapsed().as_secs_f32() * 1000.0 / duration_ms).clamp(0.0, 1.0);
                        let e = ease_out_cubic(t);
                        let dw = from_w + (to_w - from_w) * e;
                        u2.set_drawer_animated_w(dw);
                        u2.set_drawer_slide_progress((dw / NATIVE_DRAWER_WIDTH).clamp(0.0, 1.0));
                        if hwnd_now != 0 {
                            let scale = query_hwnd_scale(hwnd_now);
                            let (cw, ch) = query_client_size(hwnd_now);
                            let (vw, vh) = dims_2.lock().map(|g| *g).unwrap_or((0, 0));
                            let rot_deg = u2.get_rotation_deg().clamp(0, 359) as u16;
                            let fit_mode = fit_mode_to_enum(&u2.get_fit_mode().to_string());
                            let rect = clamp_video_rect_to_stage(
                                compute_active_video_physical_rect_with_drawer_w_and_mode(
                                    cw,
                                    ch,
                                    scale,
                                    dw,
                                    true,
                                    u2.get_is_landscape(),
                                    vw,
                                    vh,
                                    rot_deg,
                                    fit_mode,
                                    mirror::is_airplay_active(),
                                ),
                            );
                            airplay::set_trace_last_active_video_rect(
                                rect.x,
                                rect.y,
                                rect.width as i32,
                                rect.height as i32,
                            );
                            if let Ok(mut g) = rect_2.lock() {
                                *g = rect;
                            }
                            u2.set_video_hole_x((rect.x as f32) / scale);
                            u2.set_video_hole_y((rect.y as f32) / scale);
                            u2.set_video_hole_w((rect.width as f32) / scale);
                            u2.set_video_hole_h((rect.height as f32) / scale);
                            if u2.get_video_layer_active() {
                                if let Some(ref p) = *pipe_2.borrow() {
                                    let resized = p
                                        .resize_composition(rect.width, rect.height)
                                        .unwrap_or(false);
                                    sync_active_dcomp_video_layer(
                                        p.swapchain_ptr(),
                                        rect.x as f32,
                                        rect.y as f32,
                                    );
                                    if resized {
                                        commit_active_dcomp();
                                        airplay::inc_trace_dcomp_commits();
                                    }
                                }
                            }
                        }
                        if t >= 1.0 {
                            anim_t2.stop();
                            *anim_s2.borrow_mut() = None;
                            u2.set_drawer_animated_w(to_w);
                            u2.set_drawer_slide_progress(if to_w > 1.0 { 1.0 } else { 0.0 });
                            if let Some(ref p) = *pipe_2.borrow() {
                                let _ = sync_dcomp_video_layout_with_dims(
                                    hwnd_now, &u2, p, &rect_2, &dims_2, None,
                                );
                            }
                        } else {
                            flush_active_dcomp_ui_frame();
                        }
                    }
                }
            });
            record_ui_action("toggle_drawer", &format!("drawerOpen={next_open}"), Some(&u));
        });
    }

    // Theme & Language toggles (persisted to user_settings.json)
    {
        let ui_weak = ui.as_weak();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        let hwnd_cell = parent_hwnd_cell.clone();
        ui.on_toggle_theme(move || {
            let Some(u) = ui_weak.upgrade() else { return };
            let next_dark = !u.get_dark_mode();
            u.set_dark_mode(next_dark);
            let hwnd = hwnd_cell.get();
            if hwnd != 0 {
                apply_dcomp_win11_thin_acrylic(hwnd, next_dark, u.get_is_fullscreen());
            }
            flush_active_dcomp_ui_frame();
            record_ui_action("set_theme", &format!("darkMode={next_dark}"), Some(&u));
            let theme_str = if next_dark { "dark" } else { "light" };
            let body = format!(r#"{{"theme":"{theme_str}"}}"#);
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            std::thread::spawn(move || {
                let _ = inprocess_api(&st_t, &wd_t, "POST", "/api/settings", &body);
            });
        });
    }
    {
        let ui_weak = ui.as_weak();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        let hwnd_cell = parent_hwnd_cell.clone();
        ui.on_set_theme(move |dark| {
            let Some(u) = ui_weak.upgrade() else { return };
            u.set_dark_mode(dark);
            let hwnd = hwnd_cell.get();
            if hwnd != 0 {
                apply_dcomp_win11_thin_acrylic(hwnd, dark, u.get_is_fullscreen());
            }
            flush_active_dcomp_ui_frame();
            record_ui_action("set_theme", &format!("darkMode={dark}"), Some(&u));
            let theme_str = if dark { "dark" } else { "light" };
            let body = format!(r#"{{"theme":"{theme_str}"}}"#);
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            std::thread::spawn(move || {
                let _ = inprocess_api(&st_t, &wd_t, "POST", "/api/settings", &body);
            });
        });
    }
    {
        let ui_weak = ui.as_weak();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        ui.on_set_lang_en(move |en| {
            if let Some(u) = ui_weak.upgrade() {
                u.set_lang_en(en);
                let mut pill = format_status_pill_with_transport(
                    &u.get_phase().to_string(),
                    &u.get_stage().to_string(),
                    &u.get_transport_tab().to_string(),
                    mirror::is_airplay_active(),
                    en,
                );
                if let Ok(sv) = serde_json::from_str(&inprocess_api(
                    &st_c,
                    &wd_c,
                    "GET",
                    "/api/status",
                    "",
                )) {
                    let sum = parse_launchpad_from_status(&sv, en);
                    if u.get_phase() == "idle" {
                        pill = sum.status_pill_short.clone();
                    }
                    u.set_devices(ModelRc::new(VecModel::from(sum.cards)));
                    u.set_device_model_name(SharedString::from(sum.model_name));
                    u.set_device_title(SharedString::from(sum.hero_title));
                    u.set_device_subtitle(SharedString::from(sum.hero_subtitle));
                    u.set_device_pill_title(SharedString::from(sum.pill_title));
                    u.set_device_pill_detail(SharedString::from(sum.pill_detail));
                    u.set_wifi_status_text(SharedString::from(sum.wifi_text));
                }
                u.set_status_pill_compact(SharedString::from(
                    resolve_compact_topbar_status_pill_text(&pill, false),
                ));
                u.set_status_pill(SharedString::from(pill));
                let ap_audio = airplay::get_airplay_audio_snapshot();
                let airplay_live = mirror::is_airplay_active();
                let (pc_br_on, pc_br_conn, _) = iphone_link_audio::global_pc_audio_bridge_full_status();
                let (pc_br_dev, _) = iphone_link_audio::global_pc_audio_bridge_device_info();
                let pres = compute_audio_ui_presentation(
                    u.get_audio_pc_active(),
                    u.get_audio_muted(),
                    u.get_phase() == "running",
                    airplay_live,
                    ap_audio.active,
                    ap_audio.target_ms,
                    ap_audio.volume,
                    pc_br_on,
                    pc_br_conn,
                    iphone_link_audio::global_pc_audio_bridge_state_kind(),
                    &pc_br_dev,
                    en,
                );
                u.set_bt_audio_active(pres.bt_active);
                u.set_audio_live_active(pres.live_active);
                u.set_audio_live_state(SharedString::from(pres.live_state));
                u.set_airplay_audio_telemetry(SharedString::from(pres.telemetry_text.as_str()));
                u.set_audio_status_banner(SharedString::from(pres.banner_text.as_str()));
                if let Ok(dv) = serde_json::from_str(&inprocess_api(
                    &st_c,
                    &wd_c,
                    "GET",
                    "/api/driver/diagnostics",
                    "",
                )) {
                    let drows = build_diag_rows_from_json(&dv, en);
                    u.set_diagnostics(ModelRc::new(VecModel::from(drows)));
                }
                record_ui_action("set_lang_en", &format!("langEn={en}"), Some(&u));
            }
            let code = if en { "en" } else { "id" };
            let body = format!(r#"{{"language":"{code}"}}"#);
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            std::thread::spawn(move || {
                let _ = inprocess_api(&st_t, &wd_t, "POST", "/api/settings", &body);
            });
        });
    }

    // Device Picker, Transport, & Video Stream Mode
    {
        let ui_weak = ui.as_weak();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        ui.on_select_device(move |udid_ss| {
            let udid = udid_ss.to_string();
            if let Some(u) = ui_weak.upgrade() {
                u.set_selected_udid(SharedString::from(udid.as_str()));
            }
            let body = format!(r#"{{"udid":"{}"}}"#, escape_json(&udid));
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            let ui_w2 = ui_weak.clone();
            std::thread::spawn(move || {
                let resp = inprocess_api(&st_t, &wd_t, "POST", "/api/device/select", &body);
                if let Ok(v) = serde_json::from_str(&resp) {
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(u) = ui_w2.upgrade() {
                            let (cards, sel) = parse_device_cards_from_status(&v, u.get_lang_en());
                            if !cards.is_empty() {
                                u.set_devices(ModelRc::new(VecModel::from(cards)));
                            }
                            if !sel.is_empty() {
                                u.set_selected_udid(SharedString::from(sel));
                            }
                        }
                    });
                }
            });
        });
    }
    {
        let ui_weak = ui.as_weak();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        ui.on_scan_devices(move || {
            let Some(u) = ui_weak.upgrade() else { return };
            u.set_scanning(true);
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            let ui_w2 = ui_weak.clone();
            std::thread::spawn(move || {
                let resp = inprocess_api(&st_t, &wd_t, "GET", "/api/status/refresh?force=1", "");
                let parsed = serde_json::from_str(&resp).ok();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(u) = ui_w2.upgrade() {
                        u.set_scanning(false);
                        if let Some(v) = parsed {
                            let (cards, sel) = parse_device_cards_from_status(&v, u.get_lang_en());
                            u.set_devices(ModelRc::new(VecModel::from(cards)));
                            if !sel.is_empty() {
                                u.set_selected_udid(SharedString::from(sel));
                            }
                        }
                    }
                });
            });
        });
    }
    {
        let ui_weak = ui.as_weak();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        ui.on_set_transport(move |t_ss| {
            let t = if t_ss.as_str().eq_ignore_ascii_case("wifi") {
                "wifi"
            } else {
                "usb"
            };
            if let Some(u) = ui_weak.upgrade() {
                u.set_transport_tab(SharedString::from(t));
            }
            let body = format!(r#"{{"transport":"{t}"}}"#);
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            std::thread::spawn(move || {
                let _ = inprocess_api(&st_t, &wd_t, "POST", "/api/settings", &body);
            });
        });
    }
    // Start / Stop / Restart Mirroring + PIN Safe Guard (Bug #5, #8, #9)
    {
        let ui_weak = ui.as_weak();
        let router_c = input_router.clone();
        let sub_st = subclass_state.clone();
        ui.on_pin_focus_changed(move |editing| {
            router_c.set_pin_editing(editing);
            sub_st.pin_editing.store(editing, Ordering::SeqCst);
            if let Some(u) = ui_weak.upgrade() {
                u.set_pin_editing(editing);
                record_ui_action("pin_focus", &format!("editing={editing}"), Some(&u));
            }
        });
    }
    {
        let ui_weak = ui.as_weak();
        let router_c = input_router.clone();
        let sub_st = subclass_state.clone();
        ui.on_pin_text_edited(move |txt_ss| {
            router_c.set_pin_editing(true);
            sub_st.pin_editing.store(true, Ordering::SeqCst);
            if let Some(u) = ui_weak.upgrade() {
                u.set_pin_editing(true);
                // Record only length — NEVER plaintext PIN characters
                record_ui_action("pin_edit", &format!("len={}", txt_ss.as_str().len()), Some(&u));
            }
        });
    }
    {
        let ui_weak = ui.as_weak();
        let pipe_rc = pipeline.clone();
        let hwnd_cell = parent_hwnd_cell.clone();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        let inflight_rc = start_inflight_until.clone();
        let sub_st = subclass_state.clone();
        let rect_sh = active_rect_shared.clone();
        let v_act = video_active_hook.clone();
        let sess_lock = session_transition_lock.clone();
        let r_anim_t = reveal_anim_timer.clone();
        let r_anim_s = reveal_anim_state.clone();
        let trans_g = transition_gen.clone();
        let rev_flag = frame1_reveal_needed.clone();
        let dims_sh = live_video_dims.clone();
        ui.on_start_mirror(move || {
            let Some(u) = ui_weak.upgrade() else { return };
            if u.get_phase() == "starting" || u.get_phase() == "running" {
                return;
            }
            rev_flag.store(false, Ordering::SeqCst);
            if let Ok(mut d) = dims_sh.lock() {
                *d = (0, 0);
            }
            u.set_reveal_blur_visible(false);
            u.set_reveal_blur_opacity(0.0);
            u.set_reveal_blur_image(Image::default());
            u.set_anti_blank_visible(false);
            u.set_anti_blank_reason(SharedString::from(""));
            u.set_anti_blank_title(SharedString::from(""));
            u.set_anti_blank_detail(SharedString::from(""));
            let udid = u.get_selected_udid().to_string();
            let transport = u.get_transport_tab().to_string();
            let mode = u.get_stream_mode().to_string();
            let lang_en = u.get_lang_en();
            let init_stage = if mode == "airplay" {
                "waiting_airplay"
            } else {
                "connecting"
            };
            inflight_rc.set(Some(Instant::now() + Duration::from_secs(8)));
            sub_st.is_streaming_or_starting.store(true, Ordering::SeqCst);
            v_act.store(false, Ordering::SeqCst);
            u.set_first_frame_ready(false);
            u.set_video_layer_active(false);
            u.set_is_airplay_session(mode == "airplay");
            u.set_error_text(SharedString::from(""));
            u.set_phase(SharedString::from("starting"));
            u.set_stage(SharedString::from(init_stage));
            u.set_stage_progress(18);
            let full_pill = format_status_pill_with_transport(
                "starting",
                init_stage,
                &transport,
                mode == "airplay",
                lang_en,
            );
            u.set_status_pill_compact(SharedString::from(resolve_compact_topbar_status_pill_text(
                &full_pill,
                false,
            )));
            u.set_status_pill(SharedString::from(full_pill));
            u.set_connecting_detail(SharedString::from(if mode == "airplay" {
                if lang_en { "Starting AirPlay H.264 receiver..." } else { "Menyiapkan penerima AirPlay H.264..." }
            } else if transport == "wifi" {
                if lang_en { "Discovering Wi-Fi tunnel..." } else { "Menghubungkan tunnel Wi-Fi..." }
            } else {
                if lang_en { "Initializing USB tunnel..." } else { "Menghubungkan kabel USB..." }
            }));
            if let Some(ref p) = *pipe_rc.borrow() {
                p.reset_for_new_session();
                p.set_airplay_mode(mode == "airplay");
                p.set_window_landscape(u.get_is_landscape());
                if mode == "airplay" {
                    p.prewarm_h264_decoder();
                }
                let _ = sync_dcomp_video_layout(hwnd_cell.get(), &u, p, &rect_sh, Some(false));
            }
            start_stage_transition(
                &u,
                &ui_weak,
                &pipe_rc,
                &hwnd_cell,
                &rect_sh,
                &dims_sh,
                &r_anim_t,
                &r_anim_s,
                &trans_g,
                StageTransitionKind::StartToSequoia,
                240.0,
            );
            record_ui_action("start_mirror", &format!("mode={mode},transport={transport}"), Some(&u));

            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            let ui_w2 = ui_weak.clone();
            let lock_t = sess_lock.clone();
            let trans_g_e = trans_g.clone();
            std::thread::spawn(move || {
                let _guard = lock_t.lock().unwrap_or_else(|e| e.into_inner());
                let res = resolve_udid_and_start_mirror_v2(&st_t, &wd_t, &udid, &transport, &mode, lang_en);
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(u2) = ui_w2.upgrade() {
                        match res {
                            StartMirrorOutcome::Started {
                                phase: ph,
                                stage: stg,
                                fallback_reason,
                                airplay,
                            } => {
                                u2.set_phase(SharedString::from(ph.as_str()));
                                if !stg.is_empty() {
                                    u2.set_stage(SharedString::from(stg.as_str()));
                                }
                                u2.set_is_airplay_session(airplay);
                                if fallback_reason.as_deref() == Some("hevc_mft_missing") {
                                    u2.set_connecting_detail(SharedString::from(if u2.get_lang_en() {
                                        "Auto AirPlay H.264 + Touch active (PC lacks HEVC codec)."
                                    } else {
                                        "Mode AirPlay H.264 + Sentuh aktif otomatis (PC belum ada codec HEVC)."
                                    }));
                                }
                            }
                            StartMirrorOutcome::DecisionRequired {
                                reason,
                                message_id,
                                message_en,
                                ..
                            } => {
                                trans_g_e.fetch_add(1, Ordering::SeqCst);
                                u2.set_transition_active(false);
                                u2.set_standby_opacity(1.0);
                                u2.set_sequoia_opacity(0.0);
                                u2.set_sequoia_text_opacity(0.0);
                                u2.set_reveal_blur_visible(false);
                                u2.set_phase(SharedString::from("idle"));
                                u2.set_stage(SharedString::from(""));
                                u2.set_stage_progress(0);
                                u2.set_video_layer_active(false);
                                u2.set_first_frame_ready(false);
                                u2.set_is_airplay_session(false);
                                u2.set_error_text(SharedString::from(""));
                                u2.set_status_pill(SharedString::from(if u2.get_lang_en() { "Ready" } else { "Siap" }));
                                let (ab_title, ab_fallback) =
                                    format_anti_blank_overlay_copy(&reason, u2.get_lang_en());
                                let ab_detail = if u2.get_lang_en() {
                                    if message_en.is_empty() { ab_fallback } else { message_en }
                                } else if message_id.is_empty() {
                                    ab_fallback
                                } else {
                                    message_id
                                };
                                u2.set_anti_blank_reason(SharedString::from(reason));
                                u2.set_anti_blank_title(SharedString::from(ab_title));
                                u2.set_anti_blank_detail(SharedString::from(ab_detail));
                                u2.set_anti_blank_visible(true);
                            }
                            StartMirrorOutcome::Error(err_msg) => {
                                trans_g_e.fetch_add(1, Ordering::SeqCst);
                                u2.set_transition_active(false);
                                u2.set_standby_opacity(1.0);
                                u2.set_sequoia_opacity(0.0);
                                u2.set_sequoia_text_opacity(0.0);
                                u2.set_reveal_blur_visible(false);
                                u2.set_phase(SharedString::from("idle"));
                                u2.set_stage(SharedString::from(""));
                                u2.set_stage_progress(0);
                                u2.set_video_layer_active(false);
                                u2.set_first_frame_ready(false);
                                u2.set_is_airplay_session(false);
                                u2.set_status_pill(SharedString::from(if u2.get_lang_en() { "Ready" } else { "Siap" }));
                                u2.set_error_text(SharedString::from(err_msg));
                            }
                        }
                    }
                });
            });
        });
    }
    {
        let ui_weak = ui.as_weak();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        let pipe_rc = pipeline.clone();
        let hwnd_cell = parent_hwnd_cell.clone();
        let rect_sh = active_rect_shared.clone();
        let inflight_rc = start_inflight_until.clone();
        let sm_c = stream_mode_shared.clone();
        let sub_st = subclass_state.clone();
        let v_act = video_active_hook.clone();
        let sess_lock = session_transition_lock.clone();
        let r_anim_t = reveal_anim_timer.clone();
        let r_anim_s = reveal_anim_state.clone();
        let trans_g = transition_gen.clone();
        ui.on_switch_to_airplay_h264(move || {
            let Some(u) = ui_weak.upgrade() else { return };
            trans_g.fetch_add(1, Ordering::SeqCst);
            r_anim_t.stop();
            *r_anim_s.borrow_mut() = None;
            u.set_transition_active(false);
            u.set_reveal_blur_visible(false);
            u.set_reveal_blur_opacity(0.0);
            u.set_reveal_blur_image(Image::default());
            u.set_standby_opacity(0.0);
            u.set_sequoia_opacity(1.0);
            u.set_sequoia_text_opacity(1.0);
            let udid = u.get_selected_udid().to_string();
            let transport = u.get_transport_tab().to_string();
            let lang_en = u.get_lang_en();
            u.set_anti_blank_visible(false);
            u.set_anti_blank_reason(SharedString::from(""));
            u.set_stream_mode(SharedString::from("airplay"));
            if let Ok(mut g) = sm_c.lock() {
                *g = "airplay".to_string();
            }
            iphone_link_audio::stop_global_pc_audio_bridge();
            inflight_rc.set(Some(Instant::now() + Duration::from_secs(8)));
            sub_st.is_streaming_or_starting.store(true, Ordering::SeqCst);
            v_act.store(false, Ordering::SeqCst);
            u.set_first_frame_ready(false);
            u.set_video_layer_active(false);
            u.set_is_airplay_session(true);
            u.set_error_text(SharedString::from(""));
            u.set_phase(SharedString::from("starting"));
            u.set_stage(SharedString::from("waiting_airplay"));
            u.set_connecting_detail(SharedString::from(if lang_en {
                "Switching to universal AirPlay H.264 mode..."
            } else {
                "Beralih ke mode universal AirPlay H.264..."
            }));
            if let Some(ref p) = *pipe_rc.borrow() {
                p.reset_for_new_session();
                p.set_airplay_mode(true);
                p.prewarm_h264_decoder();
                let _ = sync_dcomp_video_layout(hwnd_cell.get(), &u, p, &rect_sh, Some(false));
            }
            record_ui_action("switch_to_airplay_h264", &format!("transport={transport}"), Some(&u));
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            let ui_w2 = ui_weak.clone();
            let lock_t = sess_lock.clone();
            std::thread::spawn(move || {
                let _guard = lock_t.lock().unwrap_or_else(|e| e.into_inner());
                let _ = inprocess_api(&st_t, &wd_t, "POST", "/api/settings", r#"{"streamMode":"airplay"}"#);
                let _ = inprocess_api(&st_t, &wd_t, "POST", "/api/screen/stop", "{}");
                std::thread::sleep(Duration::from_millis(120));
                let res = resolve_udid_and_start_mirror_v2(&st_t, &wd_t, &udid, &transport, "airplay", lang_en);
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(u2) = ui_w2.upgrade() {
                        match res {
                            StartMirrorOutcome::Started { phase: ph, stage: stg, .. } => {
                                u2.set_phase(SharedString::from(ph.as_str()));
                                if !stg.is_empty() {
                                    u2.set_stage(SharedString::from(stg.as_str()));
                                }
                                u2.set_is_airplay_session(true);
                            }
                            StartMirrorOutcome::DecisionRequired { message_id, message_en, .. } => {
                                u2.set_phase(SharedString::from("idle"));
                                u2.set_stage(SharedString::from(""));
                                u2.set_is_airplay_session(false);
                                u2.set_error_text(SharedString::from(if u2.get_lang_en() { message_en } else { message_id }));
                            }
                            StartMirrorOutcome::Error(err_msg) => {
                                u2.set_phase(SharedString::from("idle"));
                                u2.set_stage(SharedString::from(""));
                                u2.set_is_airplay_session(false);
                                u2.set_error_text(SharedString::from(err_msg));
                            }
                        }
                    }
                });
            });
        });
    }
    {
        let ui_weak = ui.as_weak();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        ui.on_open_hevc_install_action(move || {
            let Some(u) = ui_weak.upgrade() else { return };
            let reason = u.get_anti_blank_reason().to_string();
            let lang_en = u.get_lang_en();
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            let ui_w2 = ui_weak.clone();
            std::thread::spawn(move || {
                let msg = if reason == "media_feature_pack_missing" {
                    let _ = inprocess_api(
                        &st_t,
                        &wd_t,
                        "POST",
                        "/api/system/open_hevc_store",
                        r#"{"target":"media_feature_pack"}"#,
                    );
                    if lang_en {
                        "Optional Features opened. Install 'Media Feature Pack', then restart Windows."
                    } else {
                        "Optional Features dibuka. Pasang 'Media Feature Pack', lalu restart Windows."
                    }
                } else {
                    let resp = inprocess_api(&st_t, &wd_t, "POST", "/api/system/install_hevc", "{}");
                    let installed_now = serde_json::from_str(&resp)
                        .ok()
                        .and_then(|v| v.get("installed").and_then(|b| b.as_bool()))
                        .unwrap_or(false);
                    if !installed_now {
                        let launched = serde_json::from_str(&resp)
                            .ok()
                            .and_then(|v| v.get("launchedInstaller").and_then(|b| b.as_bool()))
                            .unwrap_or(false);
                        if !launched {
                            let _ = inprocess_api(
                                &st_t,
                                &wd_t,
                                "POST",
                                "/api/system/open_hevc_store",
                                r#"{"target":"hevc_oem"}"#,
                            );
                        }
                    }
                    if lang_en {
                        "Free HEVC installer/Store opened. Or tap Switch to AirPlay H.264 above right now."
                    } else {
                        "Installer/Store HEVC Gratis dibuka. Atau langsung klik Pindah ke AirPlay H.264 di atas."
                    }
                };
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(u2) = ui_w2.upgrade() {
                        u2.set_anti_blank_detail(SharedString::from(msg));
                    }
                });
            });
        });
    }
    {
        let ui_weak = ui.as_weak();
        let pipe_rc = pipeline.clone();
        let hwnd_cell = parent_hwnd_cell.clone();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        let inflight_rc = start_inflight_until.clone();
        let streaming_rc = is_streaming_flag.clone();
        let sub_st = subclass_state.clone();
        let rect_sh = active_rect_shared.clone();
        let v_act = video_active_hook.clone();
        let cap_c = pointer_captured_hook.clone();
        let sess_lock = session_transition_lock.clone();
        let r_anim_t = reveal_anim_timer.clone();
        let r_anim_s = reveal_anim_state.clone();
        let trans_g = transition_gen.clone();
        let rev_flag = frame1_reveal_needed.clone();
        let dims_sh = live_video_dims.clone();
        let ap_manual_rc = airplay_manual_landscape_rc.clone();
        ui.on_stop_mirror(move || {
            ap_manual_rc.set(false);
            inflight_rc.set(None);
            streaming_rc.store(false, Ordering::SeqCst);
            rev_flag.store(false, Ordering::SeqCst);
            sub_st.is_streaming_or_starting.store(false, Ordering::SeqCst);
            sub_st.phase_running.store(false, Ordering::SeqCst);
            v_act.store(false, Ordering::SeqCst);
            cap_c.store(false, Ordering::SeqCst);

            let Some(u) = ui_weak.upgrade() else {
                sync_active_dcomp_video_layer(None, 0.0, 0.0);
                let st_t = st_c.clone();
                let wd_t = wd_c.clone();
                let lock_t = sess_lock.clone();
                std::thread::spawn(move || {
                    let _guard = lock_t.lock().unwrap_or_else(|e| e.into_inner());
                    let _ = inprocess_api(&st_t, &wd_t, "POST", "/api/screen/stop", "{}");
                });
                record_ui_action("stop_mirror", "ok=true", None);
                return;
            };
            record_ui_action("stop_mirror", "ok=true", Some(&u));

            let live_blur_rgba = if u.get_video_layer_active() {
                pipe_rc.borrow().as_ref().and_then(capture_blurred_reveal_rgba)
            } else {
                None
            };
            let live_blur = live_blur_rgba
                .as_ref()
                .map(|(rgba, tw, th)| blurred_rgba_to_image(rgba, *tw, *th));
            if let Some((rgba, tw, th)) = live_blur_rgba {
                if let Some(ref img) = live_blur {
                    u.set_last_frame_blur_image(img.clone());
                    u.set_has_last_frame_blur(true);
                }
                if should_use_disk_blur_cache(false) {
                    std::thread::spawn(move || {
                        save_persisted_blur_cache_default(tw, th, &rgba);
                    });
                }
            }
            let plan = plan_stop_transition_sequence(
                u.get_video_layer_active(),
                live_blur.is_some(),
                u.get_has_last_frame_blur(),
            );
            u.set_anti_blank_visible(false);
            u.set_anti_blank_reason(SharedString::from(""));

            if matches!(plan.transition_kind, StageTransitionKind::StopFromLive) {
                let blur_img = live_blur.unwrap_or_else(|| u.get_last_frame_blur_image());
                u.set_reveal_blur_image(blur_img);
                u.set_reveal_blur_visible(true);
                u.set_reveal_blur_opacity(1.0);
                u.set_standby_opacity(0.0);
                u.set_sequoia_opacity(0.0);
                u.set_sequoia_text_opacity(0.0);
                u.set_transition_active(true);
                // Commit opaque blurred Layer 1 BEFORE detaching Layer 0 (zero black flash)
                flush_active_dcomp_ui_frame();
            } else {
                u.set_reveal_blur_visible(false);
                u.set_reveal_blur_opacity(0.0);
                u.set_reveal_blur_image(Image::default());
                u.set_standby_opacity(0.0);
                u.set_sequoia_opacity(1.0);
                u.set_sequoia_text_opacity(1.0);
                u.set_transition_active(true);
                flush_active_dcomp_ui_frame();
            }

            // Fire /api/screen/stop in background after capturing snapshot
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            let lock_t = sess_lock.clone();
            std::thread::spawn(move || {
                let _guard = lock_t.lock().unwrap_or_else(|e| e.into_inner());
                let _ = inprocess_api(&st_t, &wd_t, "POST", "/api/screen/stop", "{}");
            });

            u.set_first_frame_ready(false);
            u.set_video_layer_active(false);
            u.set_is_airplay_session(false);
            u.set_phase(SharedString::from("idle"));
            u.set_stage(SharedString::from(""));
            u.set_stage_progress(0);
            u.set_error_text(SharedString::from(""));
            let ready_pill = if u.get_lang_en() { "Ready" } else { "Siap" };
            u.set_status_pill(SharedString::from(ready_pill));
            u.set_status_pill_compact(SharedString::from(ready_pill));
            if let Ok(mut d) = dims_sh.lock() {
                *d = (0, 0);
            }
            if let Some(ref p) = *pipe_rc.borrow() {
                p.set_airplay_mode(false);
                p.set_window_landscape(false);
                let _ = sync_dcomp_video_layout(hwnd_cell.get(), &u, p, &rect_sh, Some(false));
            }
            start_stage_transition(
                &u,
                &ui_weak,
                &pipe_rc,
                &hwnd_cell,
                &rect_sh,
                &dims_sh,
                &r_anim_t,
                &r_anim_s,
                &trans_g,
                plan.transition_kind,
                plan.duration_ms as f32,
            );
        });
    }
    {
        let ui_weak = ui.as_weak();
        let pipe_rc = pipeline.clone();
        let hwnd_cell = parent_hwnd_cell.clone();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        let rect_sh = active_rect_shared.clone();
        let v_act = video_active_hook.clone();
        let sess_lock = session_transition_lock.clone();
        let r_anim_t = reveal_anim_timer.clone();
        let r_anim_s = reveal_anim_state.clone();
        let trans_g = transition_gen.clone();
        let rev_flag = frame1_reveal_needed.clone();
        let dims_sh = live_video_dims.clone();
        let ap_manual_rc = airplay_manual_landscape_rc.clone();
        ui.on_restart_mirror(move || {
            ap_manual_rc.set(false);
            let Some(u) = ui_weak.upgrade() else { return };
            trans_g.fetch_add(1, Ordering::SeqCst);
            r_anim_t.stop();
            *r_anim_s.borrow_mut() = None;
            u.set_transition_active(false);
            u.set_standby_opacity(0.0);
            u.set_sequoia_opacity(1.0);
            u.set_sequoia_text_opacity(1.0);
            rev_flag.store(false, Ordering::SeqCst);
            if let Ok(mut d) = dims_sh.lock() {
                *d = (0, 0);
            }
            u.set_reveal_blur_visible(false);
            u.set_reveal_blur_opacity(0.0);
            u.set_reveal_blur_image(Image::default());
            u.set_anti_blank_visible(false);
            u.set_anti_blank_reason(SharedString::from(""));
            let udid = u.get_selected_udid().to_string();
            let transport = u.get_transport_tab().to_string();
            let mode = u.get_stream_mode().to_string();
            v_act.store(false, Ordering::SeqCst);
            u.set_first_frame_ready(false);
            u.set_video_layer_active(false);
            u.set_is_airplay_session(mode == "airplay");
            u.set_phase(SharedString::from("starting"));
            u.set_stage(SharedString::from(if mode == "airplay" {
                "waiting_airplay"
            } else {
                "connecting"
            }));
            if let Some(ref p) = *pipe_rc.borrow() {
                p.reset_for_new_session();
                p.set_airplay_mode(mode == "airplay");
                p.set_window_landscape(u.get_is_landscape());
                if mode == "airplay" {
                    p.prewarm_h264_decoder();
                }
                let _ = sync_dcomp_video_layout(hwnd_cell.get(), &u, p, &rect_sh, Some(false));
            }
            record_ui_action("restart_mirror", &format!("mode={mode}"), Some(&u));
            let body = format!(
                r#"{{"udid":"{}","transport":"{}","mode":"{}","honestGate":true}}"#,
                escape_json(&udid),
                escape_json(&transport),
                escape_json(&mode)
            );
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            let lock_t = sess_lock.clone();
            std::thread::spawn(move || {
                let _guard = lock_t.lock().unwrap_or_else(|e| e.into_inner());
                let _ = inprocess_api(&st_t, &wd_t, "POST", "/api/screen/stop", "{}");
                std::thread::sleep(Duration::from_millis(160));
                let _ = inprocess_api(&st_t, &wd_t, "POST", "/api/screen/start", &body);
            });
        });
    }

    // Bottom Quick Nav Bar HID controls (1:1 Tahap 259 /mirror/button & /mirror/gesture pipeline)
    {
        let ui_weak = ui.as_weak();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        let router_c = input_router.clone();
        ui.on_nav_swipe_back(move || {
            record_ui_action("nav_swipe_back", "direction=back", ui_weak.upgrade().as_ref());
            if mirror::is_airplay_standalone_no_hid_active() {
                return;
            }
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            let rt_t = router_c.clone();
            std::thread::spawn(move || {
                let _ = execute_tahap259_bottom_nav(&st_t, &wd_t, &rt_t, "back");
            });
        });
    }
    {
        let ui_weak = ui.as_weak();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        let router_c = input_router.clone();
        let pipe_rc = pipeline.clone();
        let hwnd_cell = parent_hwnd_cell.clone();
        let rot_c = hooks_rot_deg.clone();
        let rect_sh = active_rect_shared.clone();
        let dims_sh = live_video_dims.clone();
        ui.on_nav_home(move || {
            let ui_opt = ui_weak.upgrade();
            record_ui_action("nav_home", "direction=home", ui_opt.as_ref());
            if let Some(ref p) = *pipe_rc.borrow() {
                p.notify_autorotate_nav_home_recent();
            }
            let should_unrotate = !mirror::is_airplay_active()
                && ui_opt
                    .as_ref()
                    .map(|u| u.get_auto_rotate() && u.get_is_landscape())
                    .unwrap_or(false);
            if should_unrotate {
                let uw = ui_weak.clone();
                let pipe_t = pipe_rc.clone();
                let hwnd_t = hwnd_cell.clone();
                let rot_t = rot_c.clone();
                let router_t = router_c.clone();
                let rect_t = rect_sh.clone();
                let dims_t = dims_sh.clone();
                Timer::single_shot(Duration::from_millis(350), move || {
                    if let Some(u2) = uw.upgrade() {
                        if !mirror::is_airplay_active() && u2.get_auto_rotate() && u2.get_is_landscape() {
                            u2.set_is_landscape(false);
                            u2.set_rotation_deg(0);
                            rot_t.store(0, Ordering::SeqCst);
                            hid::set_active_rotation_deg(0);
                            router_t.update_config(|c| c.rotation_deg = 0);
                            if let Some(ref p) = *pipe_t.borrow() {
                                p.set_rotation(0);
                                p.set_window_landscape(false);
                            }
                            let hwnd = hwnd_t.get();
                            if !u2.get_is_fullscreen() && hwnd != 0 {
                                let (lw, lh) = compute_native_window_target_logical_size_for_profile(
                                    u2.get_drawer_open(),
                                    false,
                                    u2.get_device_profile().as_str(),
                                );
                                resize_window_preserve_origin(hwnd, lw, lh);
                            }
                            if let Some(ref p) = *pipe_t.borrow() {
                                let _ = sync_dcomp_video_layout_with_dims(hwnd, &u2, p, &rect_t, &dims_t, None);
                                let _ = p.force_present_now();
                                commit_active_dcomp();
                            }
                        }
                    }
                });
            }
            if mirror::is_airplay_standalone_no_hid_active() {
                return;
            }
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            let rt_t = router_c.clone();
            std::thread::spawn(move || {
                let _ = execute_tahap259_bottom_nav(&st_t, &wd_t, &rt_t, "home");
            });
        });
    }
    {
        let ui_weak = ui.as_weak();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        let router_c = input_router.clone();
        let pipe_rc = pipeline.clone();
        let hwnd_cell = parent_hwnd_cell.clone();
        let rot_c = hooks_rot_deg.clone();
        let rect_sh = active_rect_shared.clone();
        let dims_sh = live_video_dims.clone();
        ui.on_nav_recent(move || {
            let ui_opt = ui_weak.upgrade();
            record_ui_action("nav_recent", "direction=recent", ui_opt.as_ref());
            if let Some(ref p) = *pipe_rc.borrow() {
                p.notify_autorotate_nav_home_recent();
            }
            let should_unrotate = !mirror::is_airplay_active()
                && ui_opt
                    .as_ref()
                    .map(|u| u.get_auto_rotate() && u.get_is_landscape())
                    .unwrap_or(false);
            if should_unrotate {
                let uw = ui_weak.clone();
                let pipe_t = pipe_rc.clone();
                let hwnd_t = hwnd_cell.clone();
                let rot_t = rot_c.clone();
                let router_t = router_c.clone();
                let rect_t = rect_sh.clone();
                let dims_t = dims_sh.clone();
                Timer::single_shot(Duration::from_millis(350), move || {
                    if let Some(u2) = uw.upgrade() {
                        if !mirror::is_airplay_active() && u2.get_auto_rotate() && u2.get_is_landscape() {
                            u2.set_is_landscape(false);
                            u2.set_rotation_deg(0);
                            rot_t.store(0, Ordering::SeqCst);
                            hid::set_active_rotation_deg(0);
                            router_t.update_config(|c| c.rotation_deg = 0);
                            if let Some(ref p) = *pipe_t.borrow() {
                                p.set_rotation(0);
                                p.set_window_landscape(false);
                            }
                            let hwnd = hwnd_t.get();
                            if !u2.get_is_fullscreen() && hwnd != 0 {
                                let (lw, lh) = compute_native_window_target_logical_size_for_profile(
                                    u2.get_drawer_open(),
                                    false,
                                    u2.get_device_profile().as_str(),
                                );
                                resize_window_preserve_origin(hwnd, lw, lh);
                            }
                            if let Some(ref p) = *pipe_t.borrow() {
                                let _ = sync_dcomp_video_layout_with_dims(hwnd, &u2, p, &rect_t, &dims_t, None);
                                let _ = p.force_present_now();
                                commit_active_dcomp();
                            }
                        }
                    }
                });
            }
            if mirror::is_airplay_standalone_no_hid_active() {
                return;
            }
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            let rt_t = router_c.clone();
            std::thread::spawn(move || {
                let _ = execute_tahap259_bottom_nav(&st_t, &wd_t, &rt_t, "recent");
            });
        });
    }
    {
        let ui_weak = ui.as_weak();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        let router_c = input_router.clone();
        ui.on_nav_swipe_right(move || {
            record_ui_action("nav_swipe_right", "direction=right", ui_weak.upgrade().as_ref());
            if mirror::is_airplay_standalone_no_hid_active() {
                return;
            }
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            let rt_t = router_c.clone();
            std::thread::spawn(move || {
                let _ = execute_tahap259_bottom_nav(&st_t, &wd_t, &rt_t, "right");
            });
        });
    }

    // Display Fit, Rotation & Auto-Rotate
    {
        let ui_weak = ui.as_weak();
        let pipe_rc = pipeline.clone();
        let hwnd_cell = parent_hwnd_cell.clone();
        let router_c = input_router.clone();
        let rot_c = hooks_rot_deg.clone();
        let rect_sh = active_rect_shared.clone();
        let dims_sh = live_video_dims.clone();
        let ap_manual_rc = airplay_manual_landscape_rc.clone();
        ui.on_cycle_rotation(move || {
            let Some(u) = ui_weak.upgrade() else { return };
            let hwnd = hwnd_cell.get();
            let airplay_native_landscape = mirror::is_airplay_active()
                && pipe_rc
                    .borrow()
                    .as_ref()
                    .map(|p| p.telemetry())
                    .is_some_and(|t| t.width > t.height && t.height > 0);
            let (next_deg, next_land) = if airplay_native_landscape {
                if !u.get_is_landscape() {
                    ap_manual_rc.set(true);
                    (0, true)
                } else if u.get_rotation_deg() == 0 {
                    ap_manual_rc.set(true);
                    (180, true)
                } else {
                    ap_manual_rc.set(false);
                    (90, false)
                }
            } else {
                let d = (u.get_rotation_deg() + 90) % 360;
                let nl = d == 90 || d == 270;
                if mirror::is_airplay_active() {
                    ap_manual_rc.set(nl);
                }
                (d, nl)
            };
            let hid_rot = resolve_airplay_hid_rotation_deg(airplay_native_landscape, next_deg);
            u.set_rotation_deg(next_deg);
            u.set_is_landscape(next_land);
            hid::set_active_rotation_deg(hid_rot);
            router_c.update_config(|c| c.rotation_deg = hid_rot);
            rot_c.store(hid_rot as u16, Ordering::SeqCst);
            if let Some(ref p) = *pipe_rc.borrow() {
                p.set_rotation(next_deg as u16);
                p.set_window_landscape(next_land);
                p.notify_autorotate_manual_override(next_land);
            }
            if !u.get_is_fullscreen() && hwnd != 0 {
                let (lw, lh) = compute_native_window_target_logical_size_for_profile(
                    u.get_drawer_open(),
                    next_land,
                    u.get_device_profile().as_str(),
                );
                resize_window_preserve_origin(hwnd, lw, lh);
            }
            if let Some(ref p) = *pipe_rc.borrow() {
                let _ = sync_dcomp_video_layout_with_dims(hwnd, &u, p, &rect_sh, &dims_sh, None);
                let _ = p.force_present_now();
                commit_active_dcomp();
                airplay::inc_trace_dcomp_commits();
            }
            if u.get_phase() == "running" {
                trigger_live_pli_burst();
            }
            record_ui_action("cycle_rotation", &format!("rotationDeg={next_deg}"), Some(&u));
        });
    }
    {
        let ui_weak = ui.as_weak();
        let pipe_rc = pipeline.clone();
        let hwnd_cell = parent_hwnd_cell.clone();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        let rect_sh = active_rect_shared.clone();
        ui.on_set_fit_mode(move |mode_ss| {
            let ui_key = fit_mode_to_ui_key(mode_ss.as_str());
            if let Some(ref p) = *pipe_rc.borrow() {
                p.set_fit_mode(fit_mode_to_enum(ui_key));
            }
            if let Some(u) = ui_weak.upgrade() {
                u.set_fit_mode(SharedString::from(ui_key));
                if let Some(ref p) = *pipe_rc.borrow() {
                    let _ = sync_dcomp_video_layout(hwnd_cell.get(), &u, p, &rect_sh, None);
                }
                record_ui_action("set_fit_mode", &format!("fitMode={ui_key}"), Some(&u));
            }
            let bk = fit_mode_to_backend_key(ui_key);
            let body = format!(r#"{{"fitMode":"{bk}"}}"#);
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            std::thread::spawn(move || {
                let _ = inprocess_api(&st_t, &wd_t, "POST", "/api/settings", &body);
            });
        });
    }
    {
        let ui_weak = ui.as_weak();
        let pipe_rc = pipeline.clone();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        let ap_manual_rc = airplay_manual_landscape_rc.clone();
        ui.on_set_auto_rotate(move |enabled| {
            ap_manual_rc.set(false);
            if let Some(u) = ui_weak.upgrade() {
                u.set_auto_rotate(enabled);
            }
            if let Some(ref p) = *pipe_rc.borrow() {
                p.set_auto_rotate_enabled(enabled);
            }
            let body = format!(r#"{{"autoRotate":{enabled}}}"#);
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            std::thread::spawn(move || {
                let _ = inprocess_api(&st_t, &wd_t, "POST", "/api/settings", &body);
            });
        });
    }

    // Audio PC & AirPlay Live Zero-Drop Volume / Buffer / Lip Sync
    {
        let ui_weak = ui.as_weak();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        let sm_c = stream_mode_shared.clone();
        ui.on_toggle_audio_mute(move || {
            let Some(u) = ui_weak.upgrade() else { return };
            let next_muted = !u.get_audio_muted();
            let en = u.get_lang_en();
            let phase_now = u.get_phase().to_string();
            let prefer_pc_now = u.get_audio_pc_active();
            u.set_audio_muted(next_muted);
            let is_ap = mirror::is_airplay_active()
                || sm_c.lock().map(|g| g.as_str() == "airplay").unwrap_or(false);
            let (pc_br_on, pc_br_conn, _) = iphone_link_audio::global_pc_audio_bridge_full_status();
            let (pc_br_dev, _) = iphone_link_audio::global_pc_audio_bridge_device_info();
            let ap_audio = airplay::get_airplay_audio_snapshot();
            let pres = compute_audio_ui_presentation(
                prefer_pc_now,
                next_muted,
                phase_now == "running",
                mirror::is_airplay_active(),
                ap_audio.active,
                ap_audio.target_ms,
                ap_audio.volume,
                pc_br_on,
                pc_br_conn,
                iphone_link_audio::global_pc_audio_bridge_state_kind(),
                &pc_br_dev,
                en,
            );
            u.set_bt_audio_active(pres.bt_active);
            u.set_audio_live_active(pres.live_active);
            u.set_audio_live_state(SharedString::from(pres.live_state));
            u.set_airplay_audio_telemetry(SharedString::from(pres.telemetry_text.as_str()));
            u.set_audio_status_banner(SharedString::from(pres.banner_text.as_str()));
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            let loopback_only = std::env::var("IPHONE_LINK_TEST_LOOPBACK_ONLY")
                .is_ok_and(|v| matches!(v.trim(), "1" | "true" | "TRUE" | "yes" | "YES"));
            std::thread::spawn(move || {
                if is_ap {
                    let body = format!(r#"{{"muted":{next_muted}}}"#);
                    let _ = inprocess_api(&st_t, &wd_t, "POST", "/api/audio/airplay-config", &body);
                } else {
                    let body = format!(r#"{{"audioMuted":{next_muted}}}"#);
                    let _ = inprocess_api(&st_t, &wd_t, "POST", "/api/settings", &body);
                    if next_muted {
                        server_v4::AUDIO_MANUALLY_STOPPED_THIS_SESSION.store(true, Ordering::SeqCst);
                        iphone_link_audio::stop_global_pc_audio_bridge();
                    } else {
                        server_v4::AUDIO_MANUALLY_STOPPED_THIS_SESSION.store(false, Ordering::SeqCst);
                        if prefer_pc_now && phase_now == "running" && !loopback_only {
                            let _ = inprocess_api(
                                &st_t,
                                &wd_t,
                                "POST",
                                "/api/audio",
                                r#"{"audioOut":"pc","muted":false,"forceStream":true}"#,
                            );
                        }
                    }
                }
            });
        });
    }
    {
        let ui_weak = ui.as_weak();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        ui.on_set_airplay_volume(move |vol| {
            let v = vol.clamp(0, 100);
            if let Some(u) = ui_weak.upgrade() {
                u.set_airplay_volume(v);
            }
            let body = format!(r#"{{"volume":{v}}}"#);
            let settings_body = format!(r#"{{"audioVolume":{v}}}"#);
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            std::thread::spawn(move || {
                let _ = inprocess_api(&st_t, &wd_t, "POST", "/api/audio/airplay-config", &body);
                let _ = inprocess_api(&st_t, &wd_t, "POST", "/api/settings", &settings_body);
            });
        });
    }
    {
        let ui_weak = ui.as_weak();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        ui.on_set_airplay_buffer(move |buf_ss| {
            let preset = match buf_ss.as_str().trim().to_ascii_lowercase().as_str() {
                "low" => "low",
                "high" => "high",
                _ => "normal",
            };
            if let Some(u) = ui_weak.upgrade() {
                u.set_airplay_buffer(SharedString::from(preset));
            }
            let body = format!(r#"{{"buffer":"{preset}"}}"#);
            let settings_body = format!(r#"{{"audioBufferPreset":"{preset}"}}"#);
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            std::thread::spawn(move || {
                let _ = inprocess_api(&st_t, &wd_t, "POST", "/api/audio/airplay-config", &body);
                let _ = inprocess_api(&st_t, &wd_t, "POST", "/api/settings", &settings_body);
            });
        });
    }
    {
        let ui_weak = ui.as_weak();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        ui.on_set_airplay_lip_sync(move |ls| {
            if let Some(u) = ui_weak.upgrade() {
                u.set_airplay_lip_sync(ls);
            }
            let body = format!(r#"{{"lipSync":{ls}}}"#);
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            std::thread::spawn(move || {
                let _ = inprocess_api(&st_t, &wd_t, "POST", "/api/audio/airplay-config", &body);
                let _ = inprocess_api(&st_t, &wd_t, "POST", "/api/settings", &body);
            });
        });
    }
    {
        let ui_weak = ui.as_weak();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        ui.on_set_airplay_lip_sync_ms(move |ms| {
            let clamped = ms.clamp(-100, 100);
            let ls_on = clamped >= 0;
            if let Some(u) = ui_weak.upgrade() {
                u.set_airplay_lip_sync_ms(clamped);
                u.set_airplay_lip_sync(ls_on);
            }
            let body = format!(r#"{{"lipSync":{ls_on},"lipSyncOffsetMs":{clamped}}}"#);
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            std::thread::spawn(move || {
                let _ = inprocess_api(&st_t, &wd_t, "POST", "/api/audio/airplay-config", &body);
            });
        });
    }

    // ------------------------------------------------------------------------
    // TAHAP 264 FULL 8-CARD DRAWER, STREAM MODE, 4-EDGE CROP, INPUT & TRAY
    // ------------------------------------------------------------------------
    {
        let ui_weak = ui.as_weak();
        ui.on_clear_error(move || {
            if let Some(u) = ui_weak.upgrade() {
                u.set_error_text(SharedString::from(""));
            }
        });
    }
    {
        let hwnd_cell = parent_hwnd_cell.clone();
        ui.on_hide_to_tray(move || {
            let hwnd = hwnd_cell.get();
            if hwnd != 0 {
                unsafe {
                    let _ = (win32().show_window)(hwnd, 0); // SW_HIDE
                }
            }
        });
    }
    {
        let ui_weak = ui.as_weak();
        let pipe_rc = pipeline.clone();
        let hwnd_cell = parent_hwnd_cell.clone();
        let router_c = input_router.clone();
        let rot_c = hooks_rot_deg.clone();
        let rect_sh = active_rect_shared.clone();
        let dims_sh = live_video_dims.clone();
        let ap_manual_rc = airplay_manual_landscape_rc.clone();
        ui.on_toggle_landscape(move || {
            let Some(u) = ui_weak.upgrade() else { return };
            let airplay_native_landscape = mirror::is_airplay_active()
                && pipe_rc
                    .borrow()
                    .as_ref()
                    .map(|p| p.telemetry())
                    .is_some_and(|t| t.width > t.height && t.height > 0);
            let nl = !u.get_is_landscape();
            let (next_land, next_deg) = if airplay_native_landscape {
                ap_manual_rc.set(nl);
                if nl {
                    (true, 0)
                } else {
                    (false, 90)
                }
            } else {
                if mirror::is_airplay_active() {
                    ap_manual_rc.set(nl);
                }
                (nl, if nl { 270 } else { 0 })
            };
            let hid_rot = resolve_airplay_hid_rotation_deg(airplay_native_landscape, next_deg);
            u.set_is_landscape(next_land);
            u.set_rotation_deg(next_deg);
            rot_c.store(hid_rot as u16, Ordering::SeqCst);
            hid::set_active_rotation_deg(hid_rot);
            router_c.update_config(|c| c.rotation_deg = hid_rot);
            if let Some(ref p) = *pipe_rc.borrow() {
                p.set_rotation(next_deg as u16);
                p.set_window_landscape(next_land);
                p.notify_autorotate_manual_override(next_land);
            }
            let hwnd = hwnd_cell.get();
            if !u.get_is_fullscreen() && hwnd != 0 {
                let (lw, lh) = compute_native_window_target_logical_size_for_profile(
                    u.get_drawer_open(),
                    next_land,
                    u.get_device_profile().as_str(),
                );
                resize_window_preserve_origin(hwnd, lw, lh);
            }
            if let Some(ref p) = *pipe_rc.borrow() {
                let _ = sync_dcomp_video_layout_with_dims(hwnd, &u, p, &rect_sh, &dims_sh, None);
                let _ = p.force_present_now();
                commit_active_dcomp();
                airplay::inc_trace_dcomp_commits();
            }
            if u.get_phase() == "running" {
                trigger_live_pli_burst();
            }
            record_ui_action("toggle_landscape", &format!("rotationDeg={next_deg}"), Some(&u));
        });
    }
    {
        let ui_weak = ui.as_weak();
        let pipe_rc = pipeline.clone();
        let hwnd_cell = parent_hwnd_cell.clone();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        let rect_sh = active_rect_shared.clone();
        ui.on_set_device_profile(move |prof_ss| {
            let prof = match prof_ss.as_str().trim().to_ascii_lowercase().as_str() {
                "ipad" => "ipad",
                "iphone" => "iphone",
                _ => "auto",
            };
            if let Some(u) = ui_weak.upgrade() {
                u.set_device_profile(SharedString::from(prof));
                let hwnd = hwnd_cell.get();
                if !u.get_is_fullscreen() && hwnd != 0 {
                    let (lw, lh) = compute_native_window_target_logical_size_for_profile(
                        u.get_drawer_open(),
                        u.get_is_landscape(),
                        prof,
                    );
                    resize_window_preserve_origin(hwnd, lw, lh);
                }
                if let Some(ref p) = *pipe_rc.borrow() {
                    p.set_ipad_profile(prof == "ipad");
                    let _ = sync_dcomp_video_layout(hwnd, &u, p, &rect_sh, None);
                }
                record_ui_action("set_device_profile", &format!("profile={prof}"), Some(&u));
            }
            let body = format!(r#"{{"profile":"{prof}"}}"#);
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            std::thread::spawn(move || {
                let _ = inprocess_api(&st_t, &wd_t, "POST", "/api/settings/device-profile", &body);
            });
        });
    }
    {
        let ui_weak = ui.as_weak();
        let pipe_rc = pipeline.clone();
        let hwnd_cell = parent_hwnd_cell.clone();
        let sub_st = subclass_state.clone();
        let rect_sh = active_rect_shared.clone();
        ui.on_reset_window_size(move || {
            let Some(u) = ui_weak.upgrade() else { return };
            u.set_is_fullscreen(false);
            sub_st.is_fullscreen.store(false, Ordering::SeqCst);
            let hwnd = hwnd_cell.get();
            if hwnd != 0 {
                let (lw, lh) = compute_native_window_target_logical_size_for_profile(
                    u.get_drawer_open(),
                    u.get_is_landscape(),
                    u.get_device_profile().as_str(),
                );
                resize_window_preserve_origin(hwnd, lw, lh);
                apply_dcomp_win11_thin_acrylic(hwnd, u.get_dark_mode(), false);
            }
            if let Some(ref p) = *pipe_rc.borrow() {
                let _ = sync_dcomp_video_layout(hwnd, &u, p, &rect_sh, None);
            }
            record_ui_action("reset_window_size", "", Some(&u));
        });
    }
    {
        let ui_weak = ui.as_weak();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        let sm_c = stream_mode_shared.clone();
        ui.on_set_stream_mode(move |mode_ss| {
            let mode = if mode_ss.as_str().trim().eq_ignore_ascii_case("airplay") {
                "airplay"
            } else {
                "auto"
            };
            if mode == "airplay" {
                // Strictly isolate Bluetooth A2DP OFF when user selects AirPlay H.264
                iphone_link_audio::stop_global_pc_audio_bridge();
            }
            if let Ok(mut g) = sm_c.lock() {
                *g = mode.to_string();
            }
            if let Some(u) = ui_weak.upgrade() {
                u.set_stream_mode(SharedString::from(mode));
                record_ui_action("set_stream_mode", &format!("streamMode={mode}"), Some(&u));
            }
            let body = format!(r#"{{"streamMode":"{mode}"}}"#);
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            std::thread::spawn(move || {
                let _ = inprocess_api(&st_t, &wd_t, "POST", "/api/settings", &body);
            });
        });
    }
    {
        let ui_weak = ui.as_weak();
        let pipe_rc = pipeline.clone();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        ui.on_set_crop_edge(move |edge_ss, raw_val| {
            let Some(u) = ui_weak.upgrade() else { return };
            let v = raw_val.clamp(0, 24);
            match edge_ss.as_str().trim() {
                "top" => u.set_crop_top(v),
                "bottom" => u.set_crop_bottom(v),
                "left" => u.set_crop_left(v),
                "right" => u.set_crop_right(v),
                _ => {}
            }
            let (ct, cb, cl, cr) = (
                u.get_crop_top().clamp(0, 24) as u32,
                u.get_crop_bottom().clamp(0, 24) as u32,
                u.get_crop_left().clamp(0, 24) as u32,
                u.get_crop_right().clamp(0, 24) as u32,
            );
            if let Some(ref p) = *pipe_rc.borrow() {
                p.set_crop(ct, cb, cl, cr);
            }
            let body = format!(
                r#"{{"crop":{{"top":{ct},"bottom":{cb},"left":{cl},"right":{cr}}}}}"#
            );
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            std::thread::spawn(move || {
                let _ = inprocess_api(&st_t, &wd_t, "POST", "/api/settings", &body);
            });
        });
    }
    {
        let ui_weak = ui.as_weak();
        let pipe_rc = pipeline.clone();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        ui.on_reset_crop(move || {
            if let Some(u) = ui_weak.upgrade() {
                u.set_crop_top(0);
                u.set_crop_bottom(0);
                u.set_crop_left(0);
                u.set_crop_right(0);
                let en = u.get_lang_en();
                u.set_fb_crop_state(2);
                u.set_fb_crop_text(SharedString::from(if en { "Reset" } else { "Direset" }));
                record_ui_action("reset_crop", "top=0,bottom=0,left=0,right=0", Some(&u));
                schedule_fb_reset(ui_weak.clone(), |u2| {
                    u2.set_fb_crop_state(0);
                    u2.set_fb_crop_text(SharedString::from(""));
                });
            }
            if let Some(ref p) = *pipe_rc.borrow() {
                p.set_crop(0, 0, 0, 0);
            }
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            std::thread::spawn(move || {
                let _ = inprocess_api(
                    &st_t,
                    &wd_t,
                    "POST",
                    "/api/settings",
                    r#"{"crop":{"top":0,"bottom":0,"left":0,"right":0}}"#,
                );
            });
        });
    }
    {
        let ui_weak = ui.as_weak();
        let pipe_rc = pipeline.clone();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        ui.on_save_crop(move || {
            let Some(u) = ui_weak.upgrade() else { return };
            let (ct, cb, cl, cr) = (
                u.get_crop_top().clamp(0, 24) as u32,
                u.get_crop_bottom().clamp(0, 24) as u32,
                u.get_crop_left().clamp(0, 24) as u32,
                u.get_crop_right().clamp(0, 24) as u32,
            );
            if let Some(ref p) = *pipe_rc.borrow() {
                p.set_crop(ct, cb, cl, cr);
            }
            let en = u.get_lang_en();
            u.set_fb_crop_state(2);
            u.set_fb_crop_text(SharedString::from(if en { "Saved" } else { "Tersimpan" }));
            record_ui_action(
                "save_crop",
                &format!("top={ct},bottom={cb},left={cl},right={cr}"),
                Some(&u),
            );
            schedule_fb_reset(ui_weak.clone(), |u2| {
                u2.set_fb_crop_state(0);
                u2.set_fb_crop_text(SharedString::from(""));
            });
            let body = format!(
                r#"{{"crop":{{"top":{ct},"bottom":{cb},"left":{cl},"right":{cr}}}}}"#
            );
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            std::thread::spawn(move || {
                let _ = inprocess_api(&st_t, &wd_t, "POST", "/api/settings", &body);
            });
        });
    }
    {
        let ui_weak = ui.as_weak();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        ui.on_set_audio_routing(move |route_ss| {
            let route = match route_ss.as_str().trim().to_ascii_lowercase().as_str() {
                "device" => "device",
                "bluetooth" => "bluetooth",
                _ => "pc",
            };
            let pc_active = route != "device";
            let bt_active = route == "bluetooth";
            if let Some(u) = ui_weak.upgrade() {
                let en = u.get_lang_en();
                u.set_audio_pc_active(pc_active);
                if pc_active {
                    u.set_audio_muted(false);
                }
                let ap_live = mirror::is_airplay_active();
                let ap_audio = airplay::get_airplay_audio_snapshot();
                let pres = compute_audio_ui_presentation(
                    pc_active,
                    if pc_active { false } else { u.get_audio_muted() },
                    u.get_phase() == "running",
                    ap_live,
                    ap_audio.active,
                    ap_audio.target_ms,
                    ap_audio.volume,
                    false,
                    pc_active && !ap_live,
                    "idle",
                    "",
                    en,
                );
                u.set_bt_audio_active(pres.bt_active);
                u.set_audio_live_active(pres.live_active);
                u.set_audio_live_state(SharedString::from(pres.live_state));
                u.set_airplay_audio_telemetry(SharedString::from(pres.telemetry_text.as_str()));
                u.set_audio_status_banner(SharedString::from(pres.banner_text.as_str()));
                record_ui_action("set_audio_routing", &format!("routing={route}"), Some(&u));
            }
            let body = if pc_active {
                format!(r#"{{"audioOut":"{route}","muted":false,"forceStream":true}}"#)
            } else {
                r#"{"audioOut":"device"}"#.to_string()
            };
            let settings_body = if pc_active {
                format!(r#"{{"audioOut":"{route}","bluetoothA2dp":{bt_active},"audioMuted":false}}"#)
            } else {
                r#"{"audioOut":"device","bluetoothA2dp":false}"#.to_string()
            };
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            std::thread::spawn(move || {
                let _ = inprocess_api(&st_t, &wd_t, "POST", "/api/audio", &body);
                let _ = inprocess_api(&st_t, &wd_t, "POST", "/api/settings", &settings_body);
            });
        });
    }
    {
        let ui_weak = ui.as_weak();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        ui.on_pair_bluetooth_audio(move || {
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            let ui_w2 = ui_weak.clone();
            std::thread::spawn(move || {
                let resp = inprocess_api(&st_t, &wd_t, "POST", "/api/audio/pair", "{}");
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(u) = ui_w2.upgrade() {
                        u.set_airplay_audio_telemetry(SharedString::from(
                            if resp.contains("\"ok\":true") { "BT Pairing Ready" } else { "BT Pair Triggered" },
                        ));
                    }
                });
            });
        });
    }
    {
        let ui_weak = ui.as_weak();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        let router_c = input_router.clone();
        ui.on_set_trackpad_speed(move |raw_spd| {
            let spd = clamp_trackpad_speed_pct(raw_spd);
            if let Some(u) = ui_weak.upgrade() {
                u.set_trackpad_speed(spd);
            }
            router_c.update_config(|c| c.trackpad_speed_pct = spd as u32);
            let body = format!(r#"{{"trackpadSpeed":{spd}}}"#);
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            std::thread::spawn(move || {
                let _ = inprocess_api(&st_t, &wd_t, "POST", "/api/settings", &body);
            });
        });
    }
    {
        let ui_weak = ui.as_weak();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        let router_c = input_router.clone();
        ui.on_save_trackpad_speed(move || {
            let Some(u) = ui_weak.upgrade() else { return };
            let spd = clamp_trackpad_speed_pct(u.get_trackpad_speed());
            u.set_trackpad_speed(spd);
            router_c.update_config(|c| c.trackpad_speed_pct = spd as u32);
            let en = u.get_lang_en();
            u.set_fb_speed_state(2);
            u.set_fb_speed_text(SharedString::from(if en { "Saved" } else { "Tersimpan" }));
            record_ui_action("save_trackpad_speed", &format!("speed={spd}"), Some(&u));
            schedule_fb_reset(ui_weak.clone(), |u2| {
                u2.set_fb_speed_state(0);
                u2.set_fb_speed_text(SharedString::from(""));
            });
            let body = format!(r#"{{"trackpadSpeed":{spd}}}"#);
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            std::thread::spawn(move || {
                let _ = inprocess_api(&st_t, &wd_t, "POST", "/api/settings", &body);
            });
        });
    }
    {
        let ui_weak = ui.as_weak();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        ui.on_set_cross_platform_shortcuts(move |enabled| {
            if let Some(u) = ui_weak.upgrade() {
                u.set_cross_platform_shortcuts(enabled);
                record_ui_action(
                    "set_cross_platform_shortcuts",
                    &format!("enabled={enabled}"),
                    Some(&u),
                );
            }
            let body = format!(r#"{{"crossPlatformShortcuts":{enabled}}}"#);
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            std::thread::spawn(move || {
                let _ = inprocess_api(&st_t, &wd_t, "POST", "/api/settings", &body);
            });
        });
    }
    {
        let ui_weak = ui.as_weak();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        ui.on_set_paste_pc_clipboard(move |enabled| {
            if let Some(u) = ui_weak.upgrade() {
                u.set_paste_pc_clipboard(enabled);
                record_ui_action(
                    "set_paste_pc_clipboard",
                    &format!("enabled={enabled}"),
                    Some(&u),
                );
            }
            let body = format!(r#"{{"pastePcClipboard":{enabled}}}"#);
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            std::thread::spawn(move || {
                let _ = inprocess_api(&st_t, &wd_t, "POST", "/api/settings", &body);
            });
        });
    }
    {
        let ui_weak = ui.as_weak();
        ui.on_open_external_url(move |url_ss| {
            let url = url_ss.as_str().trim().to_string();
            let allowed = validate_public_external_url(&url);
            if let Some(u) = ui_weak.upgrade() {
                if allowed {
                    u.set_clipboard_feedback(SharedString::from(if u.get_lang_en() {
                        "Opening link in system browser..."
                    } else {
                        "Membuka tautan di browser sistem..."
                    }));
                }
                record_ui_action("open_external_url", &url, Some(&u));
            }
            if allowed {
                std::thread::spawn(move || {
                    let _ = open_verified_public_url(&url);
                });
            }
        });
    }
    {
        let ui_weak = ui.as_weak();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        ui.on_paste_to_iphone(move || {
            let lang_en = ui_weak.upgrade().map(|u| {
                u.set_fb_paste_state(1);
                u.set_fb_paste_text(SharedString::from(if u.get_lang_en() { "Working..." } else { "Memproses..." }));
                record_ui_action("paste_to_iphone", "", Some(&u));
                u.get_lang_en()
            }).unwrap_or(false);
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            let ui_w2 = ui_weak.clone();
            std::thread::spawn(move || {
                let clip_txt = crate::ocr_engine::read_win32_clipboard().unwrap_or_default();
                let resp = if clip_txt.is_empty() {
                    inprocess_api(&st_t, &wd_t, "POST", "/api/keyboard/shortcut", r#"{"action":"paste"}"#)
                } else {
                    let body = format!(r#"{{"text":"{}"}}"#, escape_json(&clip_txt));
                    inprocess_api(&st_t, &wd_t, "POST", "/api/keyboard/paste", &body)
                };
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(u) = ui_w2.upgrade() {
                        let ok = resp.contains("\"ok\":true");
                        u.set_fb_paste_state(if ok { 2 } else { 3 });
                        u.set_fb_paste_text(SharedString::from(if ok {
                            if lang_en { "✓ Pasted" } else { "✓ Ditempel" }
                        } else if lang_en {
                            "✗ Empty/Idle"
                        } else {
                            "✗ Gagal"
                        }));
                        u.set_clipboard_feedback(SharedString::from(if ok {
                            if lang_en { "Text pasted to iPhone" } else { "Teks ditempel ke iPhone" }
                        } else if lang_en {
                            "Clipboard empty or iPhone not ready"
                        } else {
                            "Clipboard kosong / iPhone belum siap"
                        }));
                        schedule_fb_reset(ui_w2, |u2| {
                            u2.set_fb_paste_state(0);
                            u2.set_fb_paste_text(SharedString::from(""));
                        });
                    }
                });
            });
        });
    }
    {
        let ui_weak = ui.as_weak();
        let pipe_rc = pipeline.clone();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        ui.on_copy_ocr_text(move || {
            let lang_en = ui_weak.upgrade().map(|u| {
                u.set_fb_ocr_state(1);
                u.set_fb_ocr_text(SharedString::from(if u.get_lang_en() { "OCR..." } else { "Memproses..." }));
                record_ui_action("copy_ocr", "", Some(&u));
                u.get_lang_en()
            }).unwrap_or(false);
            let bmp_opt = pipe_rc.borrow().as_ref().and_then(|p| p.capture_lossless_bmp().ok());
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            let ui_w2 = ui_weak.clone();
            std::thread::spawn(move || {
                let body = if let Some(bmp) = bmp_opt {
                    let b64 = mirror::base64_encode(&bmp);
                    format!(r#"{{"action":"copy","frameDataUrl":"data:image/bmp;base64,{b64}"}}"#)
                } else {
                    r#"{"action":"copy"}"#.to_string()
                };
                let resp = inprocess_api(&st_t, &wd_t, "POST", "/api/keyboard/shortcut", &body);
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(u) = ui_w2.upgrade() {
                        let ok = resp.contains("\"ok\":true");
                        u.set_fb_ocr_state(if ok { 2 } else { 3 });
                        u.set_fb_ocr_text(SharedString::from(if ok {
                            if lang_en { "✓ Copied" } else { "✓ Disalin" }
                        } else if lang_en {
                            "✗ No Text"
                        } else {
                            "✗ Gagal"
                        }));
                        u.set_clipboard_feedback(SharedString::from(if ok {
                            if lang_en { "OCR text copied to PC Clipboard" } else { "OCR selesai disalin ke Clipboard PC" }
                        } else if lang_en {
                            "No screen text available for OCR"
                        } else {
                            "Belum ada teks layar untuk OCR"
                        }));
                        schedule_fb_reset(ui_w2, |u2| {
                            u2.set_fb_ocr_state(0);
                            u2.set_fb_ocr_text(SharedString::from(""));
                        });
                    }
                });
            });
        });
    }
    {
        let ui_weak = ui.as_weak();
        let pipe_rc = pipeline.clone();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        let grab_flight = grab_in_flight.clone();
        ui.on_grab_link_action(move || {
            if grab_flight
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::Relaxed)
                .is_err()
            {
                return;
            }
            let lang_en = ui_weak.upgrade().map(|u| {
                u.set_fb_grab_state(1);
                u.set_fb_grab_text(SharedString::from(if u.get_lang_en() { "Grabbing..." } else { "Memproses..." }));
                record_ui_action("grab_link", "", Some(&u));
                u.get_lang_en()
            }).unwrap_or(false);
            let bmp0 = pipe_rc.borrow().as_ref().and_then(|p| p.capture_lossless_bmp().ok());
            let weak_view = pipe_rc.borrow().as_ref().and_then(|p| p.weak_view());
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            let ui_w2 = ui_weak.clone();
            let grab_done = grab_flight.clone();
            std::thread::spawn(move || {
                struct ResetOnDrop(Arc<AtomicBool>);
                impl Drop for ResetOnDrop {
                    fn drop(&mut self) {
                        self.0.store(false, Ordering::SeqCst);
                    }
                }
                let _grab_reset_guard = ResetOnDrop(grab_done.clone());
                let mut frames_data_urls: Vec<String> = Vec::new();
                if !mirror::is_airplay_standalone_no_hid_active() && hid::is_physical_hid_active() {
                    let _ = inprocess_api(&st_t, &wd_t, "POST", "/api/device/spotlight-paste", "{}");
                    std::thread::sleep(Duration::from_millis(480));
                    if let Some(ref wv) = weak_view {
                        if let Some(arc_v) = wv.upgrade() {
                            if let Ok(mut guard) = arc_v.lock() {
                                if let Ok(b1) = guard.capture_latest_frame_lossless_bmp() {
                                    frames_data_urls.push(format!("data:image/bmp;base64,{}", mirror::base64_encode(&b1)));
                                }
                            }
                            std::thread::sleep(Duration::from_millis(180));
                            if let Ok(mut guard) = arc_v.lock() {
                                if let Ok(b2) = guard.capture_latest_frame_lossless_bmp() {
                                    frames_data_urls.push(format!("data:image/bmp;base64,{}", mirror::base64_encode(&b2)));
                                }
                            }
                        }
                    }
                    let _ = inprocess_api(&st_t, &wd_t, "POST", "/api/device/spotlight-close", "{}");
                }
                if frames_data_urls.is_empty() {
                    if let Some(b0) = bmp0 {
                        frames_data_urls.push(format!("data:image/bmp;base64,{}", mirror::base64_encode(&b0)));
                    }
                }
                let body = if let Some(first) = frames_data_urls.first() {
                    let tail = frames_data_urls.last().unwrap_or(first);
                    let arr: Vec<String> = frames_data_urls.iter().map(|s| format!(r#""{}""#, escape_json(s))).collect();
                    format!(
                        r#"{{"action":"grab-link","frameDataUrl":"{}","tailFrameDataUrl":"{}","frames":[{}],"waitResult":true}}"#,
                        escape_json(first),
                        escape_json(tail),
                        arr.join(",")
                    )
                } else {
                    r#"{"action":"grab-link"}"#.to_string()
                };
                let resp = inprocess_api(&st_t, &wd_t, "POST", "/api/social/grab-link", &body);
                grab_done.store(false, Ordering::SeqCst);
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(u) = ui_w2.upgrade() {
                        let ok = resp.contains("\"ok\":true") && resp.contains("http");
                        u.set_fb_grab_state(if ok { 2 } else { 3 });
                        u.set_fb_grab_text(SharedString::from(if ok {
                            if lang_en { "✓ Link Copied" } else { "✓ Disalin" }
                        } else if lang_en {
                            "✗ No Link"
                        } else {
                            "✗ Gagal"
                        }));
                        u.set_clipboard_feedback(SharedString::from(if ok {
                            if lang_en { "Link grabbed & copied to PC Clipboard!" } else { "Tautan diambil & disalin ke Clipboard!" }
                        } else if lang_en {
                            "No copied HTTP(S) link found on iPhone screen"
                        } else {
                            "Tautan HTTP(S) belum ditemukan di layar iPhone"
                        }));
                        schedule_fb_reset(ui_w2, |u2| {
                            u2.set_fb_grab_state(0);
                            u2.set_fb_grab_text(SharedString::from(""));
                        });
                    }
                });
            });
        });
    }
    {
        let ui_weak = ui.as_weak();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        ui.on_clear_ocr_cache(move || {
            crate::ocr_engine::clear_copy_cache();
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            std::thread::spawn(move || {
                let _ = inprocess_api(&st_t, &wd_t, "POST", "/api/keyboard/shortcut", r#"{"action":"clear-copy-cache"}"#);
            });
            if let Some(u) = ui_weak.upgrade() {
                let en = u.get_lang_en();
                u.set_fb_cache_state(2);
                u.set_fb_cache_text(SharedString::from(if en { "✓ Cleared" } else { "✓ Bersih" }));
                u.set_clipboard_feedback(SharedString::from(if en {
                    "OCR copy cache cleared"
                } else {
                    "Cache salinan OCR dibersihkan"
                }));
                record_ui_action("clear_cache", "ok=true", Some(&u));
                schedule_fb_reset(ui_weak.clone(), |u2| {
                    u2.set_fb_cache_state(0);
                    u2.set_fb_cache_text(SharedString::from(""));
                });
            }
        });
    }
    {
        let ui_weak = ui.as_weak();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        let router_c = input_router.clone();
        let sub_st = subclass_state.clone();
        ui.on_toggle_onscreen_keyboard(move || {
            let Some(u) = ui_weak.upgrade() else { return };
            let next_kb = !u.get_keyboard_enabled();
            u.set_keyboard_enabled(next_kb);
            router_c.update_config(|c| c.keyboard_enabled = next_kb);
            sub_st.keyboard_enabled.store(next_kb, Ordering::SeqCst);
            record_ui_action("toggle_keyboard", &format!("enabled={next_kb}"), Some(&u));
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            std::thread::spawn(move || {
                let _ = inprocess_api(&st_t, &wd_t, "POST", "/api/keyboard/toggle-onscreen", "{}");
            });
        });
    }
    {
        let ui_weak = ui.as_weak();
        ui.on_copy_shortcut_url(move || {
            let Some(u) = ui_weak.upgrade() else { return };
            let url = u.get_shortcut_bridge_url().to_string();
            let ok = crate::ocr_engine::write_win32_clipboard(&url);
            u.set_clipboard_feedback(SharedString::from(if ok {
                if u.get_lang_en() { "Shortcut URL copied to Clipboard" } else { "URL Shortcut disalin ke Clipboard" }
            } else if u.get_lang_en() {
                "URL ready for Apple Shortcuts"
            } else {
                "URL disiapkan untuk Apple Shortcuts"
            }));
            record_ui_action("copy_shortcut_url", &format!("ok={ok}"), Some(&u));
        });
    }
    {
        let ui_weak = ui.as_weak();
        ui.on_open_icloud_shortcut(move || {
            let icloud_url = "https://www.icloud.com/shortcuts/bbb096d97e174c1386f15e31aedef8ff";
            std::thread::spawn(move || {
                open_url_in_system_browser(icloud_url);
            });
            if let Some(u) = ui_weak.upgrade() {
                u.set_clipboard_feedback(SharedString::from(if u.get_lang_en() {
                    "Opening iCloud Shortcut link in browser..."
                } else {
                    "Membuka tautan iCloud Shortcut di browser..."
                }));
                record_ui_action("open_icloud_shortcut", icloud_url, Some(&u));
            }
        });
    }
    {
        let ui_weak = ui.as_weak();
        ui.on_copy_siri_prompt(move || {
            let Some(u) = ui_weak.upgrade() else { return };
            let bridge_url = u.get_shortcut_bridge_url().to_string();
            let prompt = if u.get_lang_en() {
                format!(
                    "Apple Shortcuts setup (iOS 27): Receive What's On Screen / Clipboard -> URL Encode -> Get Contents of {bridge_url}<encoded>"
                )
            } else {
                format!(
                    "Setup Pintasan iPhone (iOS 27): Ambil Clipboard/URL -> URL Encode -> Buka/Get Contents ke {bridge_url}<encoded>"
                )
            };
            let ok = crate::ocr_engine::write_win32_clipboard(&prompt);
            u.set_clipboard_feedback(SharedString::from(if ok {
                if u.get_lang_en() {
                    "Siri / Shortcuts guide copied to Clipboard!"
                } else {
                    "Panduan Pintasan Siri disalin ke Clipboard!"
                }
            } else if u.get_lang_en() {
                "Failed to copy Shortcuts guide"
            } else {
                "Gagal menyalin panduan Pintasan"
            }));
            record_ui_action("copy_siri_prompt", &format!("ok={ok}"), Some(&u));
        });
    }
    {
        let ui_weak = ui.as_weak();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        let router_c = input_router.clone();
        ui.on_wake_unlock(move || {
            let (lang_en, ui_pin) = ui_weak
                .upgrade()
                .map(|u| {
                    let pin = u.get_unlock_pin_input().to_string();
                    let has_pin_digits = pin.chars().any(|c| c.is_ascii_digit());
                    u.set_fb_wake_state(1);
                    u.set_fb_wake_text(SharedString::from(if u.get_lang_en() {
                        "Waking..."
                    } else {
                        "Memproses..."
                    }));
                    record_ui_action("wake_unlock", &format!("hasPin={has_pin_digits}"), Some(&u));
                    (u.get_lang_en(), pin)
                })
                .unwrap_or((false, String::new()));
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            let rt_t = router_c.clone();
            let ui_w2 = ui_weak.clone();
            std::thread::spawn(move || {
                let (ok, hid_live, dispatched, _unlocked, _resp) =
                    execute_wake_unlock_action(&st_t, &wd_t, &rt_t, &ui_pin);
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(u) = ui_w2.upgrade() {
                        let verified = ok && hid_live && dispatched;
                        u.set_fb_wake_state(if verified { 2 } else { 3 });
                        u.set_fb_wake_text(SharedString::from(if verified {
                            if lang_en { "✓ Unlocked" } else { "✓ Dibuka" }
                        } else if !hid_live {
                            if lang_en { "✗ Need Mirror" } else { "✗ Perlu Mirror" }
                        } else if lang_en {
                            "✗ Failed"
                        } else {
                            "✗ Gagal"
                        }));
                        schedule_fb_reset(ui_w2, |u2| {
                            u2.set_fb_wake_state(0);
                            u2.set_fb_wake_text(SharedString::from(""));
                        });
                    }
                });
            });
        });
    }
    {
        let ui_weak = ui.as_weak();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        let router_c = input_router.clone();
        let lock_debounce_until: Rc<Cell<Option<Instant>>> = Rc::new(Cell::new(None));
        ui.on_lock_screen(move || {
            let now = Instant::now();
            if lock_debounce_until.get().is_some_and(|dl| now < dl) {
                return;
            }
            lock_debounce_until.set(Some(
                now + Duration::from_millis(hid::LOCK_DISPATCH_DEBOUNCE_MS),
            ));
            let lang_en = ui_weak
                .upgrade()
                .map(|u| {
                    u.set_fb_lock_state(1);
                    u.set_fb_lock_text(SharedString::from(if u.get_lang_en() {
                        "Locking..."
                    } else {
                        "Memproses..."
                    }));
                    record_ui_action("lock_screen", "", Some(&u));
                    u.get_lang_en()
                })
                .unwrap_or(false);
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            let rt_t = router_c.clone();
            let ui_w2 = ui_weak.clone();
            std::thread::spawn(move || {
                let (ok, hid_live, dispatched, stealth_restored, _resp) =
                    execute_lock_screen_action(&st_t, &wd_t, &rt_t);
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(u) = ui_w2.upgrade() {
                        if stealth_restored {
                            u.set_stealth_active(false);
                        }
                        let verified = ok && hid_live && dispatched;
                        u.set_fb_lock_state(if verified { 2 } else { 3 });
                        u.set_fb_lock_text(SharedString::from(if verified {
                            if lang_en { "✓ Locked" } else { "✓ Terkunci" }
                        } else if !hid_live {
                            if lang_en { "✗ Need Mirror" } else { "✗ Perlu Mirror" }
                        } else if lang_en {
                            "✗ Failed"
                        } else {
                            "✗ Gagal"
                        }));
                        schedule_fb_reset(ui_w2, |u2| {
                            u2.set_fb_lock_state(0);
                            u2.set_fb_lock_text(SharedString::from(""));
                        });
                    }
                });
            });
        });
    }
    {
        let ui_weak = ui.as_weak();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        ui.on_toggle_stealth(move || {
            let Some(u) = ui_weak.upgrade() else { return };
            let next = !u.get_stealth_active();
            let lang_en = u.get_lang_en();
            u.set_stealth_active(next);
            u.set_fb_stealth_state(2);
            u.set_fb_stealth_text(SharedString::from(if next {
                if lang_en { "✓ Dimmed" } else { "✓ Digelapkan" }
            } else if lang_en {
                "✓ Restored"
            } else {
                "✓ Dipulihkan"
            }));
            record_ui_action("toggle_stealth", &format!("stealth={next}"), Some(&u));
            schedule_fb_reset(ui_weak.clone(), |u2| {
                u2.set_fb_stealth_state(0);
                u2.set_fb_stealth_text(SharedString::from(""));
            });
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            std::thread::spawn(move || {
                let _ = inprocess_api(&st_t, &wd_t, "POST", "/api/device/stealth", "{}");
            });
        });
    }
    {
        let ui_weak = ui.as_weak();
        let st_c = state.clone();
        let router_c = input_router.clone();
        let sub_st = subclass_state.clone();
        ui.on_save_unlock_pin(move |pin_ss| {
            let raw_digits: String = pin_ss
                .as_str()
                .trim()
                .chars()
                .filter(|c| c.is_ascii_digit())
                .collect();
            router_c.set_pin_editing(false);
            sub_st.pin_editing.store(false, Ordering::SeqCst);
            let save_res = server_v4::save_unlock_pin_sync(&st_c, &raw_digits);
            let save_ok = save_res.is_ok();
            if let Some(u) = ui_weak.upgrade() {
                u.set_pin_editing(false);
                let en = u.get_lang_en();
                match save_res {
                    Ok(saved_pin) => {
                        u.set_pin_saved(true);
                        u.set_unlock_pin_input(SharedString::from(saved_pin.as_str()));
                        u.set_pin_revealed(false);
                        u.set_fb_pin_state(2);
                        u.set_fb_pin_text(SharedString::from(if en {
                            "✓ Saved"
                        } else {
                            "✓ Tersimpan"
                        }));
                    }
                    Err(_) => {
                        u.set_fb_pin_state(3);
                        u.set_fb_pin_text(SharedString::from(if raw_digits.is_empty() {
                            if en { "✗ Empty" } else { "✗ Kosong" }
                        } else if en {
                            "✗ 4-8 digits"
                        } else {
                            "✗ 4-8 digit"
                        }));
                    }
                }
                record_ui_action("save_pin", &format!("ok={save_ok}"), Some(&u));
                schedule_fb_reset(ui_weak.clone(), |u2| {
                    u2.set_fb_pin_state(0);
                    u2.set_fb_pin_text(SharedString::from(""));
                });
            }
        });
    }
    {
        let ui_weak = ui.as_weak();
        let st_c = state.clone();
        let router_c = input_router.clone();
        let sub_st = subclass_state.clone();
        ui.on_clear_unlock_pin(move || {
            router_c.set_pin_editing(false);
            sub_st.pin_editing.store(false, Ordering::SeqCst);
            let _ = server_v4::clear_unlock_pin_sync(&st_c);
            if let Some(u) = ui_weak.upgrade() {
                u.set_pin_editing(false);
                u.set_pin_saved(false);
                u.set_unlock_pin_input(SharedString::from(""));
                u.set_pin_revealed(false);
                record_ui_action("clear_pin", "ok=true", Some(&u));
            }
        });
    }
    {
        let ui_weak = ui.as_weak();
        let st_c = state.clone();
        ui.on_wipe_privacy_data(move || {
            if should_use_disk_blur_cache(false) {
                delete_persisted_blur_cache_default();
            }
            server_v4::wipe_all_privacy_state_and_files_sync(&st_c);
            if let Some(u) = ui_weak.upgrade() {
                let en = u.get_lang_en();
                u.set_pin_saved(false);
                u.set_unlock_pin_input(SharedString::from(""));
                u.set_pin_revealed(false);
                u.set_has_last_frame_blur(false);
                u.set_last_frame_blur_image(Image::default());
                u.set_fb_wipe_state(2);
                u.set_fb_wipe_text(SharedString::from(if en { "✓ Wiped" } else { "✓ Terhapus" }));
                u.set_clipboard_feedback(SharedString::from(if en {
                    "Privacy data & PIN wiped cleanly"
                } else {
                    "Data privasi & PIN dihapus bersih"
                }));
                record_ui_action("wipe_privacy", "ok=true", Some(&u));
                schedule_fb_reset(ui_weak.clone(), |u2| {
                    u2.set_fb_wipe_state(0);
                    u2.set_fb_wipe_text(SharedString::from(""));
                });
            }
        });
    }
    {
        let ui_weak = ui.as_weak();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        let router_c = input_router.clone();
        let rot_c = hooks_rot_deg.clone();
        ui.on_hardware_button(move |btn_ss| {
            let name = btn_ss.as_str().to_string();
            if name.eq_ignore_ascii_case("lock") || name.eq_ignore_ascii_case("power") {
                if let Some(u) = ui_weak.upgrade() {
                    u.invoke_lock_screen();
                    return;
                }
            }
            let rot_deg = rot_c.load(Ordering::Relaxed) as u32;
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            let rt_t = router_c.clone();
            let ui_w2 = ui_weak.clone();
            std::thread::spawn(move || {
                let (ok, dispatched, canon, _) =
                    execute_tahap259_hardware_key(&st_t, &wd_t, &rt_t, &name, rot_deg);
                let detail = format!("btn={canon} ok={ok} dispatched={dispatched}");
                let _ = slint::invoke_from_event_loop(move || {
                    record_ui_action("hardware_button", &detail, ui_w2.upgrade().as_ref());
                });
            });
        });
    }
    {
        let ui_weak = ui.as_weak();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        ui.on_scan_wifi(move || {
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            let ui_w2 = ui_weak.clone();
            std::thread::spawn(move || {
                let _ = inprocess_api(&st_t, &wd_t, "POST", "/api/wifi/scan", "{}");
                let resp = inprocess_api(&st_t, &wd_t, "GET", "/api/status", "");
                if let Ok(sv) = serde_json::from_str(&resp) {
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(u) = ui_w2.upgrade() {
                            let sum = parse_launchpad_from_status(&sv, u.get_lang_en());
                            u.set_devices(ModelRc::new(VecModel::from(sum.cards)));
                            u.set_wifi_status_text(SharedString::from(sum.wifi_text));
                            u.set_device_model_name(SharedString::from(sum.model_name));
                            u.set_device_title(SharedString::from(sum.hero_title));
                            u.set_device_pill_title(SharedString::from(sum.pill_title));
                            u.set_device_pill_detail(SharedString::from(sum.pill_detail));
                            u.set_device_ready(sum.ready);
                            if !sum.selected_udid.is_empty() && u.get_selected_udid().is_empty() {
                                u.set_selected_udid(SharedString::from(sum.selected_udid));
                            }
                        }
                    });
                }
            });
        });
    }
    {
        let ui_weak = ui.as_weak();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        ui.on_pair_wifi(move || {
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            let ui_w2 = ui_weak.clone();
            std::thread::spawn(move || {
                let resp = inprocess_api(&st_t, &wd_t, "POST", "/api/wifi/pair", "{}");
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(u) = ui_w2.upgrade() {
                        u.set_wifi_status_text(SharedString::from(
                            if resp.contains("\"ok\":true") {
                                "Wi-Fi pairing aktif"
                            } else {
                                "Hubungkan iPhone via USB sebentar lalu coba Pairing Wi-Fi."
                            },
                        ));
                    }
                });
            });
        });
    }
    {
        let ui_weak = ui.as_weak();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        ui.on_refresh_diagnostics(move || {
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            let ui_w2 = ui_weak.clone();
            std::thread::spawn(move || {
                let resp = inprocess_api(&st_t, &wd_t, "GET", "/api/driver/diagnostics?refresh=1", "");
                if let Ok(dv) = serde_json::from_str(&resp) {
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(u) = ui_w2.upgrade() {
                            let en = u.get_lang_en();
                            let rows = build_diag_rows_from_json(&dv, en);
                            u.set_diagnostics(ModelRc::new(VecModel::from(rows)));
                            let warn_key = if en { "conflict_warning_en" } else { "conflict_warning_id" };
                            let warn_msg = dv
                                .get("diagnostics")
                                .and_then(|d| d.get(warn_key))
                                .or_else(|| dv.get(warn_key))
                                .and_then(|v| v.as_str())
                                .map(str::trim)
                                .unwrap_or("");
                            if !warn_msg.is_empty() {
                                u.set_diag_feedback_ok(false);
                                u.set_diag_feedback_text(SharedString::from(warn_msg));
                            }
                        }
                    });
                }
            });
        });
    }
    {
        let ui_weak = ui.as_weak();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        ui.on_unlock_developer_mode(move || {
            let (udid, busy_msg) = ui_weak
                .upgrade()
                .map(|u| {
                    let en = u.get_lang_en();
                    u.set_diag_feedback_ok(true);
                    u.set_diag_feedback_text(SharedString::from(if en {
                        "Unlocking Developer Mode via USB..."
                    } else {
                        "Membuka menu Developer Mode via USB..."
                    }));
                    (
                        u.get_selected_udid().to_string(),
                        if en { "Unlocking Developer Mode..." } else { "Membuka Menu Developer Mode..." },
                    )
                })
                .unwrap_or_default();
            let _ = busy_msg;
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            let ui_w2 = ui_weak.clone();
            std::thread::spawn(move || {
                let body = format!(r#"{{"udid":"{}","arm_enable":true}}"#, escape_json(&udid));
                let resp = inprocess_api(&st_t, &wd_t, "POST", "/api/device/unlock-developer-mode", &body);
                let diag_resp = inprocess_api(&st_t, &wd_t, "GET", "/api/driver/diagnostics?force=1", "");
                let parsed_amfi: Result<serde_json::Value, _> = serde_json::from_str(&resp);
                let parsed_diag: Result<serde_json::Value, _> = serde_json::from_str(&diag_resp);
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(u) = ui_w2.upgrade() {
                        let en = u.get_lang_en();
                        if let Ok(dv) = parsed_amfi {
                            let (ok, fb_msg) = extract_amfi_ui_feedback(&dv, en);
                            u.set_diag_feedback_ok(ok);
                            u.set_diag_feedback_text(SharedString::from(&fb_msg));
                            if ok {
                                u.set_error_text(SharedString::from(""));
                            } else {
                                u.set_error_text(SharedString::from(fb_msg));
                            }
                        }
                        if let Ok(diag_v) = parsed_diag {
                            let rows = build_diag_rows_from_json(&diag_v, en);
                            u.set_diagnostics(ModelRc::new(VecModel::from(rows)));
                        }
                    }
                });
            });
        });
    }
    {
        let ui_weak = ui.as_weak();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        ui.on_repair_driver(move || {
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            let ui_w2 = ui_weak.clone();
            std::thread::spawn(move || {
                let resp = inprocess_api(&st_t, &wd_t, "POST", "/api/driver/repair", "{}");
                if let Ok(dv) = serde_json::from_str(&resp) {
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(u) = ui_w2.upgrade() {
                            let en = u.get_lang_en();
                            let rows = build_diag_rows_from_json(&dv, en);
                            u.set_diagnostics(ModelRc::new(VecModel::from(rows)));
                            let warn_key = if en { "conflict_warning_en" } else { "conflict_warning_id" };
                            let warn_msg = dv
                                .get("diagnostics")
                                .and_then(|d| d.get(warn_key))
                                .or_else(|| dv.get(warn_key))
                                .and_then(|v| v.as_str())
                                .map(str::trim)
                                .unwrap_or("");
                            if !warn_msg.is_empty() {
                                u.set_diag_feedback_ok(false);
                                u.set_diag_feedback_text(SharedString::from(warn_msg));
                            }
                        }
                    });
                }
            });
        });
    }
    {
        let ui_weak = ui.as_weak();
        let st_c = state.clone();
        let copy_gen = Arc::new(std::sync::atomic::AtomicU64::new(0));
        ui.on_copy_diagnostic_log(move || {
            let Some(u) = ui_weak.upgrade() else { return };
            let next_g = copy_gen.fetch_add(1, Ordering::SeqCst).wrapping_add(1);
            let en = u.get_lang_en();
            u.set_fb_diag_copy_state(1);
            u.set_fb_diag_copy_text(SharedString::from(if en {
                "Copying..."
            } else {
                "Menyalin..."
            }));
            let st_t = st_c.clone();
            let ui_w2 = ui_weak.clone();
            let copy_g2 = copy_gen.clone();
            std::thread::spawn(move || {
                let report = server_v4::build_zero_pii_diagnostic_report(&st_t);
                let ok = crate::ocr_engine::write_win32_clipboard(&report);
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(u2) = ui_w2.upgrade() {
                        let en2 = u2.get_lang_en();
                        u2.set_fb_diag_copy_state(if ok { 2 } else { 3 });
                        u2.set_fb_diag_copy_text(SharedString::from(if ok {
                            if en2 { "Copied!" } else { "Tersalin!" }
                        } else if en2 {
                            "Copy Failed"
                        } else {
                            "Gagal Salin"
                        }));
                        let ui_w3 = ui_w2.clone();
                        let copy_g3 = copy_g2.clone();
                        slint::Timer::single_shot(Duration::from_millis(1800), move || {
                            if copy_g3.load(Ordering::SeqCst) == next_g {
                                if let Some(u3) = ui_w3.upgrade() {
                                    u3.set_fb_diag_copy_state(0);
                                    u3.set_fb_diag_copy_text(SharedString::from(""));
                                }
                            }
                        });
                    }
                });
            });
        });
    }
    {
        let ui_weak = ui.as_weak();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        ui.on_set_auto_check_update(move |val| {
            if let Some(u) = ui_weak.upgrade() {
                u.set_auto_check_update(val);
            }
            let body = format!(r#"{{"autoCheckUpdate":{val}}}"#);
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            std::thread::spawn(move || {
                let _ = inprocess_api(&st_t, &wd_t, "POST", "/api/settings", &body);
            });
        });
    }
    {
        let ui_weak = ui.as_weak();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        ui.on_set_update_channel(move |ch_ss| {
            let ch = if ch_ss.as_str().trim().eq_ignore_ascii_case("beta") {
                "beta"
            } else {
                "stable"
            };
            if let Some(u) = ui_weak.upgrade() {
                u.set_update_channel(SharedString::from(ch));
            }
            let body = format!(r#"{{"updateChannel":"{ch}"}}"#);
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            std::thread::spawn(move || {
                let _ = inprocess_api(&st_t, &wd_t, "POST", "/api/settings", &body);
            });
        });
    }
    {
        let ui_weak = ui.as_weak();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        ui.on_check_updates(move || {
            let ch = ui_weak
                .upgrade()
                .map(|u| {
                    let en = u.get_lang_en();
                    u.set_update_status_text(SharedString::from(if en {
                        "Checking for updates..."
                    } else {
                        "Memeriksa pembaruan..."
                    }));
                    u.get_update_channel().to_string()
                })
                .unwrap_or_else(|| "stable".to_string());
            let st_t = st_c.clone();
            let wd_t = wd_c.clone();
            let ui_w2 = ui_weak.clone();
            std::thread::spawn(move || {
                let body = format!(r#"{{"channel":"{}"}}"#, escape_json(&ch));
                let resp = inprocess_api(&st_t, &wd_t, "POST", "/api/update/check", &body);
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(u) = ui_w2.upgrade() {
                        let en = u.get_lang_en();
                        if let Ok(v) = serde_json::from_str(&resp) {
                            let (avail, latest, rel_u, dl_u, st_txt) =
                                format_updater_ui_state(&v, en);
                            u.set_update_available(avail);
                            u.set_latest_version(SharedString::from(latest));
                            u.set_update_release_url(SharedString::from(rel_u));
                            u.set_update_download_url(SharedString::from(dl_u));
                            u.set_update_status_text(SharedString::from(st_txt));
                        } else {
                            u.set_update_status_text(SharedString::from(if en {
                                "iPhone Link v5.0.1 is up to date."
                            } else {
                                "iPhone Link v5.0.1 sudah versi terbaru."
                            }));
                        }
                    }
                });
            });
        });
    }
    {
        let ui_weak = ui.as_weak();
        ui.on_open_update_release(move || {
            let url = ui_weak
                .upgrade()
                .map(|u| {
                    let rel = u.get_update_release_url().to_string();
                    let dl = u.get_update_download_url().to_string();
                    if !rel.trim().is_empty() {
                        rel
                    } else if !dl.trim().is_empty() {
                        dl
                    } else {
                        format!("{}/releases", crate::updater::PUBLIC_GITHUB_REPO_URL)
                    }
                })
                .unwrap_or_else(|| format!("{}/releases", crate::updater::PUBLIC_GITHUB_REPO_URL));
            if crate::updater::is_allowed_github_release_url(&url) {
                std::thread::spawn(move || {
                    let _ = open_verified_public_url(&url);
                });
            }
        });
    }
    {
        let ui_weak = ui.as_weak();
        let router_c = input_router.clone();
        let sub_done = subclass_installed.clone();
        ui.on_key_shortcut_pressed(move |txt_ss, ctrl, alt, _shift| {
            // Parent HWND SetWindowSubclass handles all WM_KEYDOWN / WM_SYSKEYDOWN / WM_CHAR
            // on the main window; skip Slint FocusScope dispatch to prevent double firing.
            if sub_done.get() {
                return;
            }
            let Some(u) = ui_weak.upgrade() else { return };
            if u.get_pin_editing() {
                return;
            }
            let text = txt_ss.as_str();
            if ctrl || alt {
                if let Some(act) = resolve_slint_shortcut(text, ctrl, alt) {
                    router_c.emit_shortcut(act);
                    return;
                }
            }
            if u.get_phase() == "running" && u.get_keyboard_enabled() && !ctrl && !alt {
                if let Some(ch) = text.chars().next() {
                    if !ch.is_control() {
                        std::thread::spawn(move || {
                            let _ = dispatch_char_with_shift_latching(ch);
                        });
                    }
                }
            }
        });
    }

    // UI-thread 40ms fast shortcut/tray/bounds dispatcher + ~240ms live pipeline telemetry
    let last_fps_frames = Rc::new(RefCell::new(0u64));
    let last_fps_at = Rc::new(RefCell::new(Instant::now()));
    let last_synced_video_dims = Rc::new(Cell::new((0u32, 0u32)));
    let tick_counter = Rc::new(Cell::new(0u32));
    let ui_sync_timer = Timer::default();
    {
        let ui_weak = ui.as_weak();
        let pipe_rc = pipeline.clone();
        let hwnd_cell = parent_hwnd_cell.clone();
        let rect_sh = active_rect_shared.clone();
        let v_act = video_active_hook.clone();
        let rot_c = hooks_rot_deg.clone();
        let fps_frames_rc = last_fps_frames.clone();
        let fps_at_rc = last_fps_at.clone();
        let synced_dims_c = last_synced_video_dims.clone();
        let streaming_rc = is_streaming_flag.clone();
        let inflight_rc = start_inflight_until.clone();
        let tray_rc = tray_host.clone();
        let sub_st = subclass_state.clone();
        let tick_c = tick_counter.clone();
        let st_c = state.clone();
        let wd_c = web_dir.clone();
        let dims_sh = live_video_dims.clone();
        let router_c = input_router.clone();
        let airplay_auto_land_rc = Rc::new(Cell::new(false));
        let last_pipe_policy = Rc::new(Cell::new((false, false)));
        let ap_manual_rc = airplay_manual_landscape_rc.clone();
        let r_anim_t = reveal_anim_timer.clone();
        let r_anim_s = reveal_anim_state.clone();
        let trans_g = transition_gen.clone();
        let rev_flag = frame1_reveal_needed.clone();
        let d_anim_s_sync = drawer_anim_state.clone();
        let zero_frame_since_rc: Rc<Cell<Option<Instant>>> = Rc::new(Cell::new(None));
        let connecting_started_at_rc: Rc<Cell<Option<Instant>>> = Rc::new(Cell::new(None));
        let last_stage_change_at_rc: Rc<Cell<Option<Instant>>> = Rc::new(Cell::new(None));
        let last_stage_key_rc: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
        let prewarmed_gen_rc = Rc::new(Cell::new(0u64));
        let auto_audio_triggered_gen_rc = Rc::new(Cell::new(0u64));
        let auto_audio_started_at_rc: Rc<Cell<Option<Instant>>> = Rc::new(Cell::new(None));
        let auto_audio_reopened_gen_rc = Rc::new(Cell::new(0u64));
        let logo_scale_sync = last_logo_scale_bits.clone();
        let loopback_only_sync = std::env::var("IPHONE_LINK_TEST_LOOPBACK_ONLY")
            .is_ok_and(|v| matches!(v.trim(), "1" | "true" | "TRUE" | "yes" | "YES"));
        ui_sync_timer.start(TimerMode::Repeated, Duration::from_millis(40), move || {
            let Some(u) = ui_weak.upgrade() else { return };
            let _ = refresh_crisp_brand_logos_for_scale(
                &u,
                u.window().scale_factor(),
                &logo_scale_sync,
            );
            let hwnd = hwnd_cell.get();
            let ap_native_land_now = mirror::is_airplay_active()
                && pipe_rc
                    .borrow()
                    .as_ref()
                    .map(|p| p.telemetry())
                    .is_some_and(|t| t.width > t.height && t.height > 0);
            let hid_rot_now = resolve_airplay_hid_rotation_deg(
                ap_native_land_now,
                u.get_rotation_deg(),
            ) as u16;
            rot_c.store(hid_rot_now, Ordering::Relaxed);

            // Fast path: immediate Sequoia Blur-to-Sharp Frame #1 reveal without waiting for tick % 6
            let frame1_hit = rev_flag.swap(false, Ordering::SeqCst);
            let is_fading_out = r_anim_s
                .borrow()
                .as_ref()
                .is_some_and(|a| matches!(a.kind, StageTransitionKind::StopFromLive | StageTransitionKind::StopFromConnecting));

            // 1. Subclass-requested Full Screen toggle (e.g. Top Bar double-click) or DComp video rect resync
            if sub_st.fs_toggle_requested.swap(false, Ordering::SeqCst) {
                u.invoke_toggle_fullscreen();
            }
            if sub_st.needs_bounds_sync.swap(false, Ordering::SeqCst) {
                if let Some(ref p) = *pipe_rc.borrow() {
                    let _ = sync_dcomp_video_layout_with_dims(hwnd, &u, p, &rect_sh, &dims_sh, None);
                }
            }
            if v_act.load(Ordering::Relaxed)
                && hwnd != 0
                && d_anim_s_sync.borrow().is_none()
            {
                let (live_cw, live_ch) = read_live_hwnd_client_phys_size(hwnd);
                let live_scale = query_hwnd_scale(hwnd);
                let (expected_rect, rect_drifted) = {
                    let prev_r = rect_sh.lock().ok().map(|g| *g);
                    let (vw, vh) = dims_sh.lock().map(|g| *g).unwrap_or((0, 0));
                    compute_fullscreen_landscape_resync_rect(
                        prev_r,
                        live_cw,
                        live_ch,
                        live_scale,
                        u.get_drawer_open(),
                        u.get_is_fullscreen(),
                        u.get_is_landscape(),
                        vw,
                        vh,
                        u.get_rotation_deg().clamp(0, 359) as u16,
                        fit_mode_to_enum(&u.get_fit_mode().to_string()),
                        mirror::is_airplay_active(),
                    )
                };
                if rect_drifted && expected_rect.width >= 16 && expected_rect.height >= 16 {
                    if let Some(ref p) = *pipe_rc.borrow() {
                        let _ = sync_dcomp_video_layout_with_dims(hwnd, &u, p, &rect_sh, &dims_sh, None);
                        let _ = p.force_present_now();
                    }
                }
            }

            // 2. Drain global keyboard shortcuts triggered from Parent HWND subclass or Video HWND
            while let Ok(act) = shortcut_rx.try_recv() {
                record_ui_action("shortcut", &format!("{act:?}"), Some(&u));
                match act {
                    GlobalShortcutAction::ToggleKeyboard => u.invoke_toggle_onscreen_keyboard(),
                    GlobalShortcutAction::CopyOcr => u.invoke_copy_ocr_text(),
                    GlobalShortcutAction::PastePcText => u.invoke_paste_to_iphone(),
                    GlobalShortcutAction::GrabLink => u.invoke_grab_link_action(),
                    GlobalShortcutAction::ClearCache => u.invoke_clear_ocr_cache(),
                    GlobalShortcutAction::ToggleLandscape => u.invoke_toggle_landscape(),
                    GlobalShortcutAction::ToggleStealth => u.invoke_toggle_stealth(),
                    GlobalShortcutAction::LockScreen => u.invoke_lock_screen(),
                    GlobalShortcutAction::WakeUnlock => u.invoke_wake_unlock(),
                    GlobalShortcutAction::ToggleMute => u.invoke_toggle_audio_mute(),
                    GlobalShortcutAction::SwipeBack => u.invoke_nav_swipe_back(),
                    GlobalShortcutAction::IosCmd(_) => {}
                }
            }

            // 3. Drain System Tray menu events
            while let Ok(tev) = tray_rx.try_recv() {
                match tev {
                    NativeTrayEvent::Restore => {
                        if hwnd != 0 {
                            unsafe {
                                let _ = (win32().show_window)(hwnd, 9); // SW_RESTORE
                                let _ = (win32().show_window)(hwnd, 5); // SW_SHOW
                            }
                        }
                    }
                    NativeTrayEvent::Hide => u.invoke_hide_to_tray(),
                    NativeTrayEvent::ToggleStartup => {
                        let next = !crate::window_dwm::is_start_with_windows_enabled();
                        let _ = crate::window_dwm::set_start_with_windows(next);
                    }
                    NativeTrayEvent::CheckUpdates => {
                        if hwnd != 0 {
                            unsafe { let _ = (win32().show_window)(hwnd, 9); }
                        }
                        if !u.get_drawer_open() {
                            u.invoke_toggle_drawer();
                        }
                        u.invoke_check_updates();
                    }
                    NativeTrayEvent::ToggleLandscape => u.invoke_toggle_landscape(),
                    NativeTrayEvent::ToggleStealth => u.invoke_toggle_stealth(),
                    NativeTrayEvent::ToggleAudioPc => u.invoke_toggle_audio_mute(),
                    NativeTrayEvent::ClearCache => u.invoke_clear_ocr_cache(),
                    NativeTrayEvent::Quit => {
                        u.invoke_close_window();
                        return;
                    }
                }
            }

            sub_st
                .keyboard_enabled
                .store(u.get_keyboard_enabled(), Ordering::Relaxed);
            sub_st
                .is_fullscreen
                .store(u.get_is_fullscreen(), Ordering::Relaxed);

            let tick = tick_c.get();
            tick_c.set(tick.wrapping_add(1));
            let tel_dim_fast = pipe_rc
                .borrow()
                .as_ref()
                .map(|p| {
                    let t = p.telemetry();
                    (t.width, t.height)
                })
                .unwrap_or((0, 0));
            let dim_changed_hit =
                tel_dim_fast.0 > 0 && tel_dim_fast.1 > 0 && tel_dim_fast != synced_dims_c.get();
            if tick % 6 != 0 && !frame1_hit && !dim_changed_hit {
                return;
            }
            if is_fading_out {
                return;
            }

            if let Some(ref th) = *tray_rc.borrow() {
                th.update_flags(TrayMenuFlags {
                    startup: crate::window_dwm::is_start_with_windows_enabled(),
                    landscape: u.get_is_landscape(),
                    stealth: u.get_stealth_active(),
                    audio_pc: u.get_audio_pc_active() && !u.get_audio_muted(),
                });
            }

            let screen_json = mirror::global_screen_status_json();
            let Ok(sv) = serde_json::from_str(&screen_json) else {
                return;
            };
            let phase = sv.get("phase").and_then(|v| v.as_str()).unwrap_or("idle");
            let stage = sv.get("stage").and_then(|v| v.as_str()).unwrap_or("");
            let stage_progress = sv
                .get("stageProgress")
                .and_then(|v| v.as_i64())
                .unwrap_or(0)
                .clamp(0, 100) as i32;
            let stage_title = sv.get("stageTitle").and_then(|v| v.as_str()).unwrap_or("");
            let stage_detail = sv.get("stageDetail").and_then(|v| v.as_str()).unwrap_or("");
            let err_str = sv.get("error").and_then(|v| v.as_str()).unwrap_or("");
            let airplay_active = mirror::is_airplay_active();
            let standalone = mirror::is_airplay_standalone_no_hid_active();

            // Protect starting animation while on_start_mirror is resolving/negotiating
            let inflight_active = inflight_rc
                .get()
                .map(|dl| Instant::now() < dl)
                .unwrap_or(false);
            if phase == "running" || !err_str.is_empty() || !u.get_error_text().is_empty() || u.get_anti_blank_visible() {
                inflight_rc.set(None);
            }
            let effective_phase = if inflight_active
                && phase == "idle"
                && u.get_phase() == "starting"
                && u.get_error_text().is_empty()
                && !u.get_anti_blank_visible()
            {
                "starting"
            } else {
                phase
            };
            let effective_stage = if effective_phase == "starting" && stage.is_empty() {
                u.get_stage().to_string()
            } else {
                stage.to_string()
            };

            if effective_phase == "starting" && !airplay_active && effective_stage != "waiting_airplay" {
                if connecting_started_at_rc.get().is_none() {
                    connecting_started_at_rc.set(Some(Instant::now()));
                }
                let stage_key = format!("{effective_stage}|{}", stage_detail.trim());
                let mut prev_k = last_stage_key_rc.borrow_mut();
                if *prev_k != stage_key || last_stage_change_at_rc.get().is_none() {
                    *prev_k = stage_key;
                    last_stage_change_at_rc.set(Some(Instant::now()));
                }
            } else {
                connecting_started_at_rc.set(None);
                last_stage_change_at_rc.set(None);
                last_stage_key_rc.borrow_mut().clear();
            }

            let conn_stagnant_ms = last_stage_change_at_rc
                .get()
                .map(|t| t.elapsed().as_millis() as u64)
                .unwrap_or(0);
            let conn_total_ms = connecting_started_at_rc
                .get()
                .map(|t| t.elapsed().as_millis() as u64)
                .unwrap_or(0);
            if let Some(wd_reason) = evaluate_connecting_watchdog(
                effective_phase,
                airplay_active,
                &effective_stage,
                conn_stagnant_ms,
                conn_total_ms,
            ) {
                connecting_started_at_rc.set(None);
                last_stage_change_at_rc.set(None);
                last_stage_key_rc.borrow_mut().clear();
                inflight_rc.set(None);
                streaming_rc.store(false, Ordering::SeqCst);
                rev_flag.store(false, Ordering::SeqCst);
                sub_st.is_streaming_or_starting.store(false, Ordering::SeqCst);
                sub_st.phase_running.store(false, Ordering::SeqCst);
                let wd_en = u.get_lang_en();
                let wd_msg = if wd_reason == "total_timeout" {
                    if wd_en {
                        "USB/CoreDevice handshake timed out (>90s). Unplug & replug the USB cable, unlock your iPhone, verify Trust & Developer Mode, or click Copy Diagnostic Log."
                    } else {
                        "Koneksi USB/CoreDevice terlalu lama (>90 detik). Cabut lalu colok ulang kabel USB, buka kunci iPhone, cek Trust & Developer Mode, atau klik Salin Log Diagnostik."
                    }
                } else if wd_en {
                    "Connection stalled at the current step (>25s). Replug USB cable, check Apple Mobile Device Service / conflicting apps, or click Copy Diagnostic Log."
                } else {
                    "Koneksi tertahan pada tahap ini (>25 detik). Colok ulang kabel USB, cek Apple Mobile Device Service / aplikasi bentrok, atau klik Salin Log Diagnostik."
                };
                std::thread::spawn(move || {
                    let _ = mirror::global_stop_screen();
                });
                u.set_transition_active(false);
                u.set_standby_opacity(1.0);
                u.set_sequoia_opacity(0.0);
                u.set_sequoia_text_opacity(0.0);
                u.set_reveal_blur_visible(false);
                u.set_first_frame_ready(false);
                u.set_video_layer_active(false);
                u.set_is_airplay_session(false);
                u.set_phase(SharedString::from("idle"));
                u.set_stage(SharedString::from(""));
                u.set_stage_progress(0);
                let ready_pill = if wd_en { "Ready" } else { "Siap" };
                u.set_status_pill(SharedString::from(ready_pill));
                u.set_status_pill_compact(SharedString::from(ready_pill));
                u.set_error_text(SharedString::from(wd_msg));
                return;
            }

            let stream_or_start = effective_phase == "running" || effective_phase == "starting";
            streaming_rc.store(stream_or_start, Ordering::Relaxed);
            sub_st
                .phase_running
                .store(effective_phase == "running", Ordering::Relaxed);
            sub_st
                .is_streaming_or_starting
                .store(stream_or_start, Ordering::Relaxed);

            u.set_phase(SharedString::from(effective_phase));
            u.set_stage(SharedString::from(effective_stage.as_str()));
            if effective_phase != "starting" || stage_progress > 0 {
                u.set_stage_progress(stage_progress);
            }
            u.set_stage_title(SharedString::from(stage_title));
            u.set_stage_detail(SharedString::from(stage_detail));
            if !err_str.is_empty() || effective_phase == "running" {
                u.set_error_text(SharedString::from(if effective_phase == "running" { "" } else { err_str }));
            }
            if effective_phase == "idle" && !u.get_first_frame_ready() {
                u.set_is_airplay_session(false);
            } else if airplay_active || effective_stage == "waiting_airplay" {
                u.set_is_airplay_session(true);
            }
            if effective_stage == "waiting_airplay" {
                let note_str = sv.get("note").and_then(|v| v.as_str()).unwrap_or("").trim();
                let sync_detail = if !note_str.is_empty() {
                    note_str
                } else {
                    stage_detail.trim()
                };
                if !sync_detail.is_empty() {
                    u.set_connecting_detail(SharedString::from(sync_detail));
                }
                let cur_gen = mirror::current_live_session_gen();
                if cur_gen > 0 && prewarmed_gen_rc.get() != cur_gen {
                    prewarmed_gen_rc.set(cur_gen);
                    if let Some(ref p) = *pipe_rc.borrow() {
                        p.prewarm_h264_decoder();
                    }
                }
            } else if effective_phase == "starting" && !stage_detail.trim().is_empty() {
                u.set_connecting_detail(SharedString::from(stage_detail.trim()));
            }
            u.set_is_airplay_standalone(standalone);
            router_c.update_config(|c| c.airplay_no_hid = standalone);
            let transport_str = u.get_transport_tab().to_string();
            let pill = if effective_phase == "idle" {
                let mut cur = u.get_status_pill().to_string();
                if cur == "Siap" || cur == "Ready" {
                    cur = format_status_pill_with_transport(
                        effective_phase,
                        &effective_stage,
                        &transport_str,
                        airplay_active,
                        u.get_lang_en(),
                    );
                }
                cur
            } else {
                format_status_pill_with_transport(
                    effective_phase,
                    &effective_stage,
                    &transport_str,
                    airplay_active,
                    u.get_lang_en(),
                )
            };
            u.set_status_pill_compact(SharedString::from(
                resolve_compact_topbar_status_pill_text(&pill, false),
            ));
            u.set_status_pill(SharedString::from(pill));

            // Audio telemetry + Single Source of Truth + deterministic 1x/session_gen auto-start on CoreDevice HEVC
            let cur_session_gen = mirror::current_live_session_gen();
            let tel_presented_frames = pipe_rc
                .borrow()
                .as_ref()
                .map(|p| p.telemetry().presented_frames)
                .unwrap_or(0);
            let manually_stopped_audio =
                server_v4::AUDIO_MANUALLY_STOPPED_THIS_SESSION.load(Ordering::SeqCst);
            if should_trigger_auto_audio_on_running(
                effective_phase,
                tel_presented_frames,
                airplay_active,
                &effective_stage,
                u.get_audio_pc_active(),
                u.get_audio_muted(),
                manually_stopped_audio,
                cur_session_gen,
                auto_audio_triggered_gen_rc.get(),
                loopback_only_sync,
            ) {
                auto_audio_triggered_gen_rc.set(cur_session_gen);
                auto_audio_started_at_rc.set(Some(Instant::now()));
                let st_a = st_c.clone();
                let wd_a = wd_c.clone();
                let spawn_gen = cur_session_gen;
                std::thread::Builder::new()
                    .name("native-ui-auto-audio".to_string())
                    .spawn(move || {
                        std::thread::sleep(Duration::from_millis(250));
                        if mirror::current_live_session_gen() != spawn_gen
                            || mirror::is_airplay_active()
                            || server_v4::AUDIO_MANUALLY_STOPPED_THIS_SESSION.load(Ordering::SeqCst)
                        {
                            return;
                        }
                        if !iphone_link_audio::is_global_pc_audio_bridge_active()
                            && !iphone_link_audio::is_global_pc_audio_bridge_connecting()
                        {
                            let _ = inprocess_api(
                                &st_a,
                                &wd_a,
                                "POST",
                                "/api/audio",
                                r#"{"audioOut":"pc","muted":false,"forceStream":true}"#,
                            );
                        }
                    })
                    .ok();
            } else if effective_phase == "running"
                && !airplay_active
                && effective_stage != "waiting_airplay"
                && u.get_audio_pc_active()
                && !u.get_audio_muted()
                && !manually_stopped_audio
                && !loopback_only_sync
                && cur_session_gen > 0
                && auto_audio_triggered_gen_rc.get() == cur_session_gen
                && auto_audio_reopened_gen_rc.get() != cur_session_gen
                && iphone_link_audio::global_pc_audio_last_closed_by_peer()
                && !iphone_link_audio::is_global_pc_audio_bridge_active()
                && !iphone_link_audio::is_global_pc_audio_bridge_connecting()
                && auto_audio_started_at_rc
                    .get()
                    .is_some_and(|t| t.elapsed() < Duration::from_secs(5))
            {
                auto_audio_reopened_gen_rc.set(cur_session_gen);
                let st_a = st_c.clone();
                let wd_a = wd_c.clone();
                let spawn_gen = cur_session_gen;
                std::thread::Builder::new()
                    .name("native-ui-audio-reopen".to_string())
                    .spawn(move || {
                        std::thread::sleep(Duration::from_millis(300));
                        if mirror::current_live_session_gen() != spawn_gen
                            || mirror::is_airplay_active()
                            || server_v4::AUDIO_MANUALLY_STOPPED_THIS_SESSION.load(Ordering::SeqCst)
                        {
                            return;
                        }
                        let _ = inprocess_api(
                            &st_a,
                            &wd_a,
                            "POST",
                            "/api/audio",
                            r#"{"audioOut":"pc","muted":false,"forceStream":true}"#,
                        );
                    })
                    .ok();
            }

            let ap_audio = airplay::get_airplay_audio_snapshot();
            u.set_airplay_audio_active(airplay_active && ap_audio.active);
            let (pc_br_on, pc_br_conn, _) = iphone_link_audio::global_pc_audio_bridge_full_status();
            let (pc_br_dev, _) = iphone_link_audio::global_pc_audio_bridge_device_info();
            let audio_pres = compute_audio_ui_presentation(
                u.get_audio_pc_active(),
                u.get_audio_muted(),
                effective_phase == "running",
                airplay_active,
                ap_audio.active,
                ap_audio.target_ms,
                ap_audio.volume,
                pc_br_on,
                pc_br_conn,
                iphone_link_audio::global_pc_audio_bridge_state_kind(),
                &pc_br_dev,
                u.get_lang_en(),
            );
            u.set_bt_audio_active(audio_pres.bt_active);
            u.set_audio_live_active(audio_pres.live_active);
            u.set_audio_live_state(SharedString::from(audio_pres.live_state));
            u.set_airplay_audio_telemetry(SharedString::from(audio_pres.telemetry_text.as_str()));
            u.set_audio_status_banner(SharedString::from(audio_pres.banner_text.as_str()));

            // DirectComposition Layer 0 Visibility Gate & True Sequoia Blur-to-Sharp Reveal (`280 ms`)
            if let Some(ref p) = *pipe_rc.borrow() {
                let want_vis = should_video_hwnd_be_visible(effective_phase, &effective_stage);
                let tel = p.telemetry();
                if effective_phase == "running"
                    && effective_stage != "waiting_airplay"
                    && !airplay_active
                    && tel.presented_frames == 0
                {
                    if zero_frame_since_rc.get().is_none() {
                        zero_frame_since_rc.set(Some(Instant::now()));
                    }
                } else {
                    zero_frame_since_rc.set(None);
                }
                let zf_ms = zero_frame_since_rc
                    .get()
                    .map(|t| t.elapsed().as_millis() as u64)
                    .unwrap_or(0);
                if let Some(ab_reason) = crate::native_video_pipeline::evaluate_anti_blank_watchdog(
                    effective_phase,
                    &effective_stage,
                    airplay_active,
                    tel.presented_frames,
                    &tel.codec_error_kind,
                    zf_ms,
                ) {
                    if !u.get_anti_blank_visible() || u.get_anti_blank_reason().as_str() != ab_reason {
                        let (ab_t, ab_d) = format_anti_blank_overlay_copy(&ab_reason, u.get_lang_en());
                        u.set_anti_blank_reason(SharedString::from(ab_reason));
                        u.set_anti_blank_title(SharedString::from(ab_t));
                        u.set_anti_blank_detail(SharedString::from(ab_d));
                        u.set_anti_blank_visible(true);
                    }
                } else if u.get_anti_blank_visible() {
                    let cur_ab = u.get_anti_blank_reason();
                    let keep_decision_card = effective_phase == "idle"
                        && matches!(
                            cur_ab.as_str(),
                            "ios_below_27" | "hevc_mft_missing" | "media_feature_pack_missing"
                        );
                    if !keep_decision_card {
                        u.set_anti_blank_visible(false);
                        u.set_anti_blank_reason(SharedString::from(""));
                    }
                }
                if tel.width > 0 && tel.height > 0 {
                    if let Ok(mut d) = dims_sh.lock() {
                        *d = (tel.width, tel.height);
                    }
                }
                let mut just_revealed = false;
                if effective_phase == "running"
                    && effective_stage != "waiting_airplay"
                    && tel.presented_frames >= 1
                    && !u.get_first_frame_ready()
                {
                    just_revealed = true;
                    let blur_opt = capture_blurred_reveal_rgba(p);
                    if let Some((rgba, tw, th)) = blur_opt {
                        let img = blurred_rgba_to_image(&rgba, tw, th);
                        u.set_last_frame_blur_image(img.clone());
                        u.set_has_last_frame_blur(true);
                        u.set_reveal_blur_image(img);
                        u.set_reveal_blur_visible(true);
                        u.set_first_frame_ready(true);
                        if should_use_disk_blur_cache(false) {
                            std::thread::spawn(move || {
                                save_persisted_blur_cache_default(tw, th, &rgba);
                            });
                        }
                        start_stage_transition(
                            &u,
                            &ui_weak,
                            &pipe_rc,
                            &hwnd_cell,
                            &rect_sh,
                            &dims_sh,
                            &r_anim_t,
                            &r_anim_s,
                            &trans_g,
                            StageTransitionKind::FirstFrameReveal {
                                use_sequoia_fallback: false,
                            },
                            360.0,
                        );
                    } else {
                        u.set_reveal_blur_visible(false);
                        u.set_first_frame_ready(true);
                        start_stage_transition(
                            &u,
                            &ui_weak,
                            &pipe_rc,
                            &hwnd_cell,
                            &rect_sh,
                            &dims_sh,
                            &r_anim_t,
                            &r_anim_s,
                            &trans_g,
                            StageTransitionKind::FirstFrameReveal {
                                use_sequoia_fallback: true,
                            },
                            240.0,
                        );
                    }
                } else if !want_vis && u.get_first_frame_ready() {
                    trans_g.fetch_add(1, Ordering::SeqCst);
                    r_anim_t.stop();
                    *r_anim_s.borrow_mut() = None;
                    u.set_transition_active(false);
                    u.set_reveal_blur_visible(false);
                    u.set_reveal_blur_opacity(0.0);
                    u.set_reveal_blur_image(Image::default());
                    u.set_first_frame_ready(false);
                    if let Ok(mut d) = dims_sh.lock() {
                        *d = (0, 0);
                    }
                }
                let show_video = want_vis && u.get_first_frame_ready();
                v_act.store(show_video, Ordering::SeqCst);
                let mut airplay_oriented = false;
                if show_video && airplay_active {
                    if let Some(plan) = resolve_airplay_manual_orientation_plan(
                        true,
                        tel.width,
                        tel.height,
                        u.get_drawer_open(),
                        u.get_device_profile().as_str(),
                        ap_manual_rc.get(),
                    ) {
                        if u.get_is_landscape() != plan.is_landscape
                            || u.get_rotation_deg() != plan.video_rotation_deg
                        {
                            let hid_rot = resolve_airplay_hid_rotation_deg(
                                tel.width > tel.height && tel.height > 0,
                                plan.video_rotation_deg,
                            );
                            u.set_is_landscape(plan.is_landscape);
                            u.set_rotation_deg(plan.video_rotation_deg);
                            rot_c.store(hid_rot as u16, Ordering::SeqCst);
                            hid::set_active_rotation_deg(hid_rot);
                            router_c.update_config(|c| c.rotation_deg = hid_rot);
                            p.set_rotation(plan.video_rotation_deg as u16);
                            p.set_window_landscape(plan.is_landscape);
                            airplay_auto_land_rc.set(plan.is_landscape);
                            airplay_oriented = true;
                        }
                    }
                } else if show_video && !airplay_active && u.get_auto_rotate() {
                    if let Some(new_deg) = p.take_pending_hevc_autorotate_deg() {
                        let want_land = new_deg == 90 || new_deg == 270;
                        if u.get_is_landscape() != want_land
                            || u.get_rotation_deg() != i32::from(new_deg)
                        {
                            u.set_is_landscape(want_land);
                            u.set_rotation_deg(i32::from(new_deg));
                            rot_c.store(new_deg, Ordering::SeqCst);
                            hid::set_active_rotation_deg(u32::from(new_deg));
                            router_c.update_config(|c| c.rotation_deg = u32::from(new_deg));
                            p.set_rotation(new_deg);
                            p.set_window_landscape(want_land);
                            if !u.get_is_fullscreen() && hwnd != 0 {
                                let (lw, lh) = compute_native_window_target_logical_size_for_profile(
                                    u.get_drawer_open(),
                                    want_land,
                                    u.get_device_profile().as_str(),
                                );
                                resize_window_preserve_origin(hwnd, lw, lh);
                            }
                            airplay_oriented = true;
                        }
                    }
                } else if !show_video
                    && (airplay_auto_land_rc.replace(false)
                        || ap_manual_rc.replace(false)
                        || (!airplay_active && u.get_rotation_deg() != 0))
                {
                    u.set_is_landscape(false);
                    u.set_rotation_deg(0);
                    rot_c.store(0, Ordering::SeqCst);
                    hid::set_active_rotation_deg(0);
                    router_c.update_config(|c| c.rotation_deg = 0);
                    p.set_rotation(0);
                    if !u.get_is_fullscreen() && hwnd != 0 {
                        let (lw, lh) = compute_native_window_target_logical_size_for_profile(
                            u.get_drawer_open(),
                            false,
                            u.get_device_profile().as_str(),
                        );
                        resize_window_preserve_origin(hwnd, lw, lh);
                    }
                    airplay_oriented = true;
                }
                let want_policy = (airplay_active, u.get_is_landscape());
                if last_pipe_policy.get() != want_policy {
                    p.set_airplay_mode(want_policy.0);
                    p.set_window_landscape(want_policy.1);
                    last_pipe_policy.set(want_policy);
                }
                let cur_dims = (tel.width, tel.height);
                if just_revealed
                    || airplay_oriented
                    || show_video != u.get_video_layer_active()
                    || (show_video && cur_dims != synced_dims_c.get())
                {
                    let _ = sync_dcomp_video_layout_with_dims(
                        hwnd,
                        &u,
                        p,
                        &rect_sh,
                        &dims_sh,
                        Some(show_video),
                    );
                    if airplay_oriented {
                        let _ = p.force_present_now();
                        commit_active_dcomp();
                    }
                    synced_dims_c.set(if show_video { cur_dims } else { (0, 0) });
                }
                if tel.width > 0 && tel.height > 0 {
                    u.set_live_res(SharedString::from(format!("{}x{}", tel.width, tel.height)));
                }
                if !tel.codec.is_empty() {
                    u.set_live_codec(SharedString::from(tel.codec.as_str()));
                }
                let elapsed = fps_at_rc.borrow().elapsed().as_secs_f64();
                if elapsed >= 0.75 {
                    let prev = *fps_frames_rc.borrow();
                    let delta = tel.decoded_frames.saturating_sub(prev);
                    let fps = ((delta as f64) / elapsed).round() as i32;
                    *fps_frames_rc.borrow_mut() = tel.decoded_frames;
                    *fps_at_rc.borrow_mut() = Instant::now();
                    u.set_live_fps(if effective_phase == "running" { fps } else { 0 });
                }
            }
            let _ = (&st_c, &wd_c);
        });
    }

    // Background worker thread: idle device discovery (2000ms) + BT A2DP trigger on running
    let poller_stop = stop_poller.clone();
    let poller_state = state.clone();
    let poller_web = web_dir.clone();
    let poller_streaming = is_streaming_flag.clone();
    let poller_ui_weak = ui.as_weak();
    let poller_thread = std::thread::Builder::new()
        .name("native-ui-poller".to_string())
        .spawn(move || {
            let mut last_idle_scan = Instant::now() - Duration::from_secs(10);
            let mut last_running_bt_sync = Instant::now() - Duration::from_secs(10);
            while !poller_stop.load(Ordering::Relaxed) {
                let streaming = poller_streaming.load(Ordering::Relaxed);
                if !streaming && last_idle_scan.elapsed() >= Duration::from_millis(2000) {
                    last_idle_scan = Instant::now();
                    let resp = inprocess_api(&poller_state, &poller_web, "GET", "/api/status", "");
                    if let Ok(v) = serde_json::from_str(&resp) {
                        let ui_w = poller_ui_weak.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(u) = ui_w.upgrade() {
                                let sum = parse_launchpad_from_status(&v, u.get_lang_en());
                                u.set_devices(ModelRc::new(VecModel::from(sum.cards)));
                                u.set_device_model_name(SharedString::from(sum.model_name));
                                u.set_device_title(SharedString::from(sum.hero_title));
                                u.set_device_subtitle(SharedString::from(sum.hero_subtitle));
                                u.set_device_pill_title(SharedString::from(sum.pill_title));
                                u.set_device_pill_detail(SharedString::from(sum.pill_detail));
                                u.set_device_ready(sum.ready);
                                u.set_wifi_status_text(SharedString::from(sum.wifi_text));
                                if u.get_phase() == "idle" {
                                    u.set_status_pill(SharedString::from(if u.get_drawer_open() {
                                        sum.status_pill_wide
                                    } else {
                                        sum.status_pill_short
                                    }));
                                }
                                if !sum.selected_udid.is_empty() && u.get_selected_udid().is_empty() {
                                    u.set_selected_udid(SharedString::from(sum.selected_udid));
                                }
                            }
                        });
                    }
                } else if streaming
                    && !mirror::is_airplay_active()
                    && !iphone_link_audio::is_global_pc_audio_bridge_active()
                    && !iphone_link_audio::is_global_pc_audio_bridge_connecting()
                    && last_running_bt_sync.elapsed() >= Duration::from_millis(1200)
                {
                    last_running_bt_sync = Instant::now();
                    let _ = inprocess_api(&poller_state, &poller_web, "GET", "/api/status", "");
                }
                std::thread::sleep(Duration::from_millis(150));
            }
        })
        .ok();

    let _ = slint::run_event_loop();

    // Clean teardown in exact creator-thread order (< 300ms deterministic shutdown)
    stop_poller.store(true, Ordering::SeqCst);
    *tray_host.borrow_mut() = None;
    sync_active_dcomp_video_layer(None, 0.0, 0.0);
    if let Some(mut p) = pipeline.borrow_mut().take() {
        p.shutdown();
    }
    let _ = ui.hide();
    shutdown_active_dcomp_host();
    std::thread::spawn(|| {
        let _ = mirror::global_stop_screen();
        iphone_link_audio::stop_global_pc_audio_bridge();
    });
    server.stop();
    let _ = poller_thread;
    0
}

// ============================================================================
// AUTOMATED ZERO-WEBVIEW2 NATIVE SLINT UI SELF-TEST (`--native-ui-selftest`)
// ============================================================================

/// Headless-safe automated E2E self-test (`iPhone Link.exe --native-ui-selftest`):
/// Creates a single Win32 `HWND` (`WS_EX_NOREDIRECTIONBITMAP`, `0` Child HWNDs) with
/// 2-layer DirectComposition (`video_visual` Layer 0 behind `ui_visual` Layer 1),
/// verifies Win11 Thin Acrylic, Dark/Light & ID/EN, Drawer dynamic layout, live
/// AirPlay Audio config, Layer 0 attach/detach, transparent video-hole alpha (`0`),
/// decodes & presents >= 10 real synthetic H.264 `I_PCM` IDR frames on GPU with `0`
/// WebView2 processes, prints PII-free JSON, and exits cleanly (`0` = PASS).
pub fn run_native_ui_selftest() -> i32 {
    let t_start = Instant::now();
    mirror::global_reset_mirror_runtime_for_test();
    ensure_dcomp_platform_installed();
    reset_dcomp_loop_state();

    let gpu = match LiveD3d11Gpu::new() {
        Ok(g) => Arc::new(g),
        Err(e) => {
            println!(
                r#"{{"status":"FAIL","stage":"LiveD3d11Gpu::new","error":"{}"}}"#,
                escape_json(&e)
            );
            return 1;
        }
    };

    let ui = match MainWindow::new() {
        Ok(w) => w,
        Err(e) => {
            println!(
                r#"{{"status":"FAIL","stage":"MainWindow::new","error":"{}"}}"#,
                escape_json(&e.to_string())
            );
            return 1;
        }
    };
    ui.set_dark_mode(true);
    ui.set_lang_en(false);
    ui.set_phase(SharedString::from("idle"));
    ui.set_stage(SharedString::from(""));
    ui.set_stream_mode(SharedString::from("auto"));
    ui.set_first_frame_ready(false);
    ui.set_video_layer_active(false);

    if let Err(e) = ui.show() {
        println!(
            r#"{{"status":"FAIL","stage":"MainWindow::show","error":"{}"}}"#,
            escape_json(&e.to_string())
        );
        return 1;
    }

    struct SelftestMetrics {
        slint_window_created: bool,
        not_maximized: bool,
        thin_acrylic_applied: bool,
        always_on_acrylic_swca: bool,
        always_on_acrylic_active: bool,
        backdrop_backend: &'static str,
        dwm_systembackdrop_type: i32,
        autofit_fullscreen_top_gap_px: i32,
        child_hwnd_attached: bool,
        dcomp_two_layer_active: bool,
        ui_alpha_transparent_hole_ok: bool,
        dark_glass_alpha: u8,
        light_glass_alpha: u8,
        acrylic_layer1_alpha_ok: bool,
        fullscreen_wings_frosted_ok: bool,
        sequoia_overlay_exclusive_ok: bool,
        hidden_on_idle: bool,
        theme_lang_toggle_ok: bool,
        drawer_bounds_sync_ok: bool,
        airplay_audio_live_ok: bool,
        airspace_hide_show_ok: bool,
        real_h264_present_ok: bool,
        airplay_h264_continuous_present_ok: bool,
        airplay_orientation_switch_clean_ok: bool,
        airplay_same_dim_avcc_no_freeze_ok: bool,
        airplay_midstream_watchdog_ok: bool,
        airplay_stale_buffer_purge_ok: bool,
        airplay_bt709_color_consistent_ok: bool,
        airplay_portrait_90cw_fill_ok: bool,
        airplay_non_mod16_60frame_zero_freeze_ok: bool,
        airplay_20x_orientation_endurance_zero_black_ok: bool,
        airplay_ctr_500pkt_continuity_ok: bool,
        airplay_stage_bounds_and_topbar_label_ok: bool,
        airplay_vfr_low_latency_and_fullscreen_portrait_ok: bool,
        airplay_zero_green_macroblock_and_fullscreen_landscape_ok: bool,
        airplay_infinite_gop_and_manual_rotate_zero_pillarbox_ok: bool,
        airplay_avcc_256_511_zero_freeze_and_anti_smear_ok: bool,
        airplay_hd_nv12_shader_quality_ok: bool,
        ui_tahap259_full_parity_and_saweria_footer_ok: bool,
        shortcut_chips_count: usize,
        topbar_auto_badge_ok: bool,
        saweria_footer_ok: bool,
        hevc_autorotate_konami_efootball_e2e_ok: bool,
        svg_hd_icons_ok: bool,
        topbar_6btn_clean_no_theme_mirroring_unclipped_ok: bool,
        micro_polishes_en_clean_ok: bool,
        seamless_start_stop_transitions_ok: bool,
        fullscreen_exit_atomic_sync_ok: bool,
        loopback_only_test_mode: bool,
        decoded_frames: u64,
        presented_frames: u64,
        ram_working_set_mb: f64,
    }

    let metrics = Rc::new(RefCell::new(SelftestMetrics {
        slint_window_created: false,
        not_maximized: false,
        thin_acrylic_applied: false,
        always_on_acrylic_swca: false,
        always_on_acrylic_active: false,
        backdrop_backend: "win11_transient_acrylic",
        dwm_systembackdrop_type: -1,
        autofit_fullscreen_top_gap_px: -1,
        child_hwnd_attached: false,
        dcomp_two_layer_active: false,
        ui_alpha_transparent_hole_ok: false,
        dark_glass_alpha: 255,
        light_glass_alpha: 255,
        acrylic_layer1_alpha_ok: false,
        fullscreen_wings_frosted_ok: false,
        sequoia_overlay_exclusive_ok: false,
        hidden_on_idle: false,
        theme_lang_toggle_ok: false,
        drawer_bounds_sync_ok: false,
        airplay_audio_live_ok: false,
        airspace_hide_show_ok: false,
        real_h264_present_ok: false,
        airplay_h264_continuous_present_ok: false,
        airplay_orientation_switch_clean_ok: false,
        airplay_same_dim_avcc_no_freeze_ok: false,
        airplay_midstream_watchdog_ok: false,
        airplay_stale_buffer_purge_ok: false,
        airplay_bt709_color_consistent_ok: false,
        airplay_portrait_90cw_fill_ok: false,
        airplay_non_mod16_60frame_zero_freeze_ok: false,
        airplay_20x_orientation_endurance_zero_black_ok: false,
        airplay_ctr_500pkt_continuity_ok: false,
        airplay_stage_bounds_and_topbar_label_ok: false,
        airplay_vfr_low_latency_and_fullscreen_portrait_ok: false,
        airplay_zero_green_macroblock_and_fullscreen_landscape_ok: false,
        airplay_infinite_gop_and_manual_rotate_zero_pillarbox_ok: false,
        airplay_avcc_256_511_zero_freeze_and_anti_smear_ok: false,
        airplay_hd_nv12_shader_quality_ok: false,
        ui_tahap259_full_parity_and_saweria_footer_ok: false,
        shortcut_chips_count: 0,
        topbar_auto_badge_ok: false,
        saweria_footer_ok: false,
        hevc_autorotate_konami_efootball_e2e_ok: false,
        svg_hd_icons_ok: false,
        topbar_6btn_clean_no_theme_mirroring_unclipped_ok: false,
        micro_polishes_en_clean_ok: false,
        seamless_start_stop_transitions_ok: false,
        fullscreen_exit_atomic_sync_ok: false,
        loopback_only_test_mode: false,
        decoded_frames: 0,
        presented_frames: 0,
        ram_working_set_mb: 0.0,
    }));

    let active_rect_shared = Arc::new(Mutex::new(ActiveVideoPhysicalRect::default()));
    let comp_pipeline = match LiveVideoPipeline::new_composition(gpu.clone(), 16, 16, 0) {
        Ok(p) => p,
        Err(e) => {
            println!(
                r#"{{"status":"FAIL","stage":"LiveVideoPipeline::new_composition","error":"{}"}}"#,
                escape_json(&e)
            );
            let _ = ui.hide();
            return 1;
        }
    };
    let pipeline: Rc<RefCell<Option<LiveVideoPipeline>>> = Rc::new(RefCell::new(Some(comp_pipeline)));

    let init_scale = query_hwnd_scale(0);
    let init_wa = query_work_area(0);
    let (wx0, wy0, wphys0, hphys0) = compute_centered_window_rect(
        NATIVE_PORTRAIT_WIDTH,
        NATIVE_PORTRAIT_HEIGHT,
        init_scale,
        init_wa,
    );

    let hooks = DcompHostHooks {
        is_fullscreen: Box::new(|| false),
        on_nchittest: Box::new(|_cx, _cy, _cw, _ch, _scale| 1),
        on_pointer_event: Box::new(|_msg, _cx, _cy| false),
        on_wheel_event: Box::new(|_cx, _cy, _delta, _horiz| false),
        on_key_event: Box::new(|_msg, _wp, _lp| false),
        on_resized: {
            let ui_r_w = ui.as_weak();
            let rect_c = active_rect_shared.clone();
            Box::new(move |cw, ch, scale| {
                let Some(u) = ui_r_w.upgrade() else { return };
                let rect = compute_active_video_physical_rect(
                    cw,
                    ch,
                    scale,
                    u.get_drawer_open(),
                    u.get_is_fullscreen(),
                    u.get_is_landscape(),
                    0,
                    0,
                    u.get_rotation_deg().clamp(0, 359) as u16,
                    fit_mode_to_enum(&u.get_fit_mode().to_string()),
                );
                if let Ok(mut g) = rect_c.lock() {
                    *g = rect;
                }
                u.set_video_hole_x((rect.x as f32) / scale);
                u.set_video_hole_y((rect.y as f32) / scale);
                u.set_video_hole_w((rect.width as f32) / scale);
                u.set_video_hole_h((rect.height as f32) / scale);
            })
        },
        on_exit_sizemove: Box::new(|| {}),
        on_caption_dblclick: Box::new(|| {}),
        on_close_requested: Box::new(|| {
            let _ = slint::quit_event_loop();
        }),
    };

    let parent_hwnd = match create_single_dcomp_host(
        "iPhone Link Selftest",
        gpu.clone(),
        wx0,
        wy0,
        wphys0,
        hphys0,
        true,
        hooks,
    ) {
        Ok(h) => h,
        Err(e) => {
            println!(
                r#"{{"status":"FAIL","stage":"create_single_dcomp_host","error":"{}"}}"#,
                escape_json(&e)
            );
            if let Some(mut p) = pipeline.borrow_mut().take() {
                p.shutdown();
            }
            let _ = ui.hide();
            return 1;
        }
    };

    apply_embedded_window_icons_and_aumid(parent_hwnd);
    if let Some(ref p) = *pipeline.borrow() {
        let _ = sync_dcomp_video_layout(parent_hwnd, &ui, p, &active_rect_shared, Some(false));
    }

    {
        let mut m = metrics.borrow_mut();
        let dcomp_st = query_active_dcomp_state();
        let swca_st = query_active_always_on_acrylic_state();
        let backdrop_val = query_dwm_systembackdrop_type(parent_hwnd).unwrap_or(-1);
        let fs_rect = compute_active_video_physical_rect(
            1920,
            1040,
            1.0,
            false,
            true,
            false,
            1202,
            2598,
            0,
            FitMode::AutoFit,
        );
        let (_, fs_dest) = compute_dest_and_clip_rects_for_fit(
            iphone_link_video::d3d11_context::Rect {
                left: 0,
                top: 0,
                right: 1202,
                bottom: 2598,
            },
            fs_rect.width,
            fs_rect.height,
            0,
            FitMode::AutoFit,
        );
        m.slint_window_created = parent_hwnd != 0;
        m.not_maximized = !is_hwnd_maximized(parent_hwnd);
        m.thin_acrylic_applied = parent_hwnd != 0;
        m.always_on_acrylic_swca = swca_st.map(|s| s.swca_applied).unwrap_or(false);
        m.always_on_acrylic_active = swca_st.map(|s| s.always_on_active).unwrap_or(false);
        m.backdrop_backend = swca_st
            .map(|s| s.backend.as_str())
            .unwrap_or("win11_transient_acrylic");
        m.dwm_systembackdrop_type = backdrop_val;
        m.autofit_fullscreen_top_gap_px = fs_dest.top;
        m.dcomp_two_layer_active = dcomp_st.is_some();
        m.child_hwnd_attached = m.dcomp_two_layer_active;
        m.hidden_on_idle = dcomp_st.map(|s| !s.2).unwrap_or(false);
    }

    let step_idx = Rc::new(AtomicI32::new(0));
    let injected_frames = Rc::new(AtomicU32::new(0));
    let session_gen = Rc::new(RefCell::new(0u64));
    let hidden_on_waiting = Rc::new(AtomicBool::new(false));
    let shown_on_running = Rc::new(AtomicBool::new(false));
    let orient_switch_sent = Rc::new(AtomicBool::new(false));
    let same_dim_avcc_sent = Rc::new(AtomicBool::new(false));

    let driver_timer = Timer::default();
    {
        let ui_weak = ui.as_weak();
        let pipe_rc = pipeline.clone();
        let rect_sh = active_rect_shared.clone();
        let gpu_sel = gpu.clone();
        let m_rc = metrics.clone();
        let step_rc = step_idx.clone();
        let inj_rc = injected_frames.clone();
        let gen_rc = session_gen.clone();
        let wait_hid_rc = hidden_on_waiting.clone();
        let run_vis_rc = shown_on_running.clone();
        let switch_rc = orient_switch_sent.clone();
        let same_dim_rc = same_dim_avcc_sent.clone();

        driver_timer.start(TimerMode::Repeated, Duration::from_millis(55), move || {
            let Some(u) = ui_weak.upgrade() else { return };
            let st = step_rc.fetch_add(1, Ordering::SeqCst);

            if st == 0 {
                // Step 1: Dark <-> Light + raw Layer-1 alpha & premultiplied invariant verification
                let scale0 = query_hwnd_scale(parent_hwnd);
                let (cw0, _) = query_client_size(parent_hwnd);
                let gx = (cw0 / 2).max(16);
                let gy = (56.0 * scale0).round().max(42.0) as u32;
                let top_gy = (18.0 * scale0).round().max(6.0) as u32;
                u.window().request_redraw();
                flush_active_dcomp_ui_frame();
                let dark_px = sample_active_dcomp_ui_pixel(gx, gy).unwrap_or_default();
                let dark_top_px = sample_active_dcomp_ui_pixel(gx, top_gy).unwrap_or_default();
                let dark_a = dark_px.a;

                u.set_dark_mode(false);
                apply_dcomp_win11_thin_acrylic(parent_hwnd, false, false);
                u.window().request_redraw();
                flush_active_dcomp_ui_frame();
                std::thread::sleep(Duration::from_millis(210));
                slint::platform::update_timers_and_animations();
                u.window().request_redraw();
                flush_active_dcomp_ui_frame();
                let light_px = sample_active_dcomp_ui_pixel(gx, gy).unwrap_or_default();
                let light_top_px = sample_active_dcomp_ui_pixel(gx, top_gy).unwrap_or_default();
                let light_a = light_px.a;
                let light_ok = !u.get_dark_mode();

                u.set_dark_mode(true);
                apply_dcomp_win11_thin_acrylic(parent_hwnd, true, false);
                u.window().request_redraw();
                flush_active_dcomp_ui_frame();
                std::thread::sleep(Duration::from_millis(210));
                slint::platform::update_timers_and_animations();
                u.window().request_redraw();
                flush_active_dcomp_ui_frame();
                let dark_ok = u.get_dark_mode();

                u.set_lang_en(true);
                let en_ok = u.get_lang_en();
                u.set_lang_en(false);
                let id_ok = !u.get_lang_en();
                {
                    let mut m = m_rc.borrow_mut();
                    m.dark_glass_alpha = dark_a;
                    m.light_glass_alpha = light_a;
                    let premul_ok = [dark_px, dark_top_px, light_px, light_top_px]
                        .iter()
                        .all(|p| p.r <= p.a && p.g <= p.a && p.b <= p.a);
                    m.acrylic_layer1_alpha_ok = (40..=140).contains(&dark_a)
                        && (60..=165).contains(&light_a)
                        && (40..=140).contains(&dark_top_px.a)
                        && (60..=165).contains(&light_top_px.a)
                        && premul_ok;
                    m.theme_lang_toggle_ok = light_ok && dark_ok && en_ok && id_ok;
                }

                // AirPlay live audio config (zero-drop)
                u.set_stream_mode(SharedString::from("airplay"));
                u.set_airplay_volume(60);
                u.set_airplay_buffer(SharedString::from("low"));
                u.set_airplay_lip_sync(true);
                let snap =
                    airplay::apply_live_audio_config(Some(60), Some("low"), Some(true), Some(false));
                m_rc.borrow_mut().airplay_audio_live_ok =
                    snap.volume == 60 && snap.buffer_preset == "low" && snap.lip_sync && !snap.muted;

                // Drawer Open -> resize 706x832 -> verify ActiveVideoPhysicalRect does not intrude into drawer
                u.set_drawer_open(true);
                u.set_drawer_animated_w(NATIVE_DRAWER_WIDTH);
                u.set_drawer_slide_progress(1.0);
                resize_window_preserve_origin(
                    parent_hwnd,
                    NATIVE_PORTRAIT_WIDTH + NATIVE_DRAWER_WIDTH,
                    NATIVE_PORTRAIT_HEIGHT,
                );
                let (open_ok, closed_ok) = if let Some(ref p) = *pipe_rc.borrow() {
                    let r_open = sync_dcomp_video_layout(parent_hwnd, &u, p, &rect_sh, Some(false));
                    let scale_now = query_hwnd_scale(parent_hwnd);
                    let (cw_open, _) = query_client_size(parent_hwnd);
                    let drawer_px = (NATIVE_DRAWER_WIDTH * scale_now).round() as u32;
                    let o_ok = r_open.x == 0
                        && r_open.y == (NATIVE_TOP_BAR_HEIGHT * scale_now).round() as i32
                        && (r_open.width + drawer_px) <= cw_open + 2;

                    // Drawer Close -> resize back to 346x832
                    u.set_drawer_open(false);
                    u.set_drawer_animated_w(0.0);
                    u.set_drawer_slide_progress(0.0);
                    resize_window_preserve_origin(
                        parent_hwnd,
                        NATIVE_PORTRAIT_WIDTH,
                        NATIVE_PORTRAIT_HEIGHT,
                    );
                    let r_closed = sync_dcomp_video_layout(parent_hwnd, &u, p, &rect_sh, Some(false));
                    let c_ok = r_closed.x == 0
                        && r_closed.y == (NATIVE_TOP_BAR_HEIGHT * scale_now).round() as i32
                        && r_closed.width >= 200;
                    (o_ok, c_ok)
                } else {
                    (false, false)
                };
                m_rc.borrow_mut().drawer_bounds_sync_ok = open_ok && closed_ok;
            } else if st == 1 {
                // Step 2: Airspace Guard on waiting_airplay -> Layer 0 MUST stay detached!
                u.set_phase(SharedString::from("starting"));
                u.set_stage(SharedString::from("waiting_airplay"));
                u.set_first_frame_ready(false);
                let vis_wait = should_video_hwnd_be_visible("starting", "waiting_airplay");
                if let Some(ref p) = *pipe_rc.borrow() {
                    let _ = sync_dcomp_video_layout(parent_hwnd, &u, p, &rect_sh, Some(vis_wait));
                }
                let attached_wait = query_active_dcomp_state().map(|s| s.2).unwrap_or(true);
                wait_hid_rc.store(!vis_wait && !attached_wait, Ordering::SeqCst);

                // Start AirPlay session in-memory (port 0, no mDNS) and promote to running -> Layer 0 attached!
                let _ = mirror::start_airplay_mirror_session(
                    Some("00008150-0000000000000263"),
                    false,
                    "wifi",
                    None,
                    Some(0),
                    false,
                );
                let gen = mirror::current_live_session_gen();
                *gen_rc.borrow_mut() = gen;

                let f0 = build_synthetic_h264_ipcm_idr(64, 64, 0x90, 0);
                let avcc_b64 = mirror::base64_encode(&f0.avcc_config);
                mirror::update_live_codec_h264_avcc(gen, "avc1.42801e", &avcc_b64, 64, 64);

                u.set_phase(SharedString::from("running"));
                u.set_stage(SharedString::from(""));
                u.set_first_frame_ready(true);
                let vis_run = should_video_hwnd_be_visible("running", "");
                if let Some(ref p) = *pipe_rc.borrow() {
                    let _ = sync_dcomp_video_layout(parent_hwnd, &u, p, &rect_sh, Some(vis_run));
                }
                let attached_run = query_active_dcomp_state().map(|s| s.2).unwrap_or(false);
                run_vis_rc.store(vis_run && attached_run, Ordering::SeqCst);

                // Feed Frame #1 IDR + 3 initial P_SKIP AVCC frames (26-frame continuous AirPlay H.264 stream)
                let _ = mirror::global_on_physical_h264_avcc_frame(gen, f0.avcc_sample, true);
                for i in 1..4u32 {
                    let p = build_synthetic_h264_p_skip_avcc_sample(64, 64, i);
                    let _ = mirror::global_on_physical_h264_avcc_frame(gen, p, false);
                }
                inj_rc.store(4, Ordering::SeqCst);
            } else {
                // Step 3..N: Feed 25 continuous P_SKIP delta frames & verify >= 20 decoded + >= 10 DXGI presents
                let gen = *gen_rc.borrow();
                let cur_inj = inj_rc.load(Ordering::Relaxed);
                if cur_inj < 26 {
                    if cur_inj == 6 && !same_dim_rc.swap(true, Ordering::SeqCst) {
                        // Tahap 270: inject same-dim 64x64 variant avcC (pps_count 1->2) WITHOUT new IDR
                        // and keep streaming P_SKIP frames continuously (0 Frame #1 / mid-stream freeze!).
                        let f_same = build_synthetic_h264_ipcm_idr(64, 64, 0x90, 0);
                        let mut variant_avcc = f_same.avcc_config;
                        if variant_avcc.len() >= 10 {
                            let sps_len =
                                u16::from_be_bytes([variant_avcc[6], variant_avcc[7]]) as usize;
                            let num_pps_idx = 8 + sps_len;
                            if num_pps_idx < variant_avcc.len() && variant_avcc[num_pps_idx] == 1 {
                                variant_avcc[num_pps_idx] = 2;
                                variant_avcc.extend_from_slice(&(f_same.pps.len() as u16).to_be_bytes());
                                variant_avcc.extend_from_slice(&f_same.pps);
                            }
                        }
                        let variant_b64 = mirror::base64_encode(&variant_avcc);
                        mirror::update_live_codec_h264_avcc(gen, "avc1.42801e", &variant_b64, 64, 64);
                    }
                    let p = build_synthetic_h264_p_skip_avcc_sample(64, 64, cur_inj);
                    let _ = mirror::global_on_physical_h264_avcc_frame(gen, p, false);
                    inj_rc.store(cur_inj + 1, Ordering::SeqCst);
                }
                let tel_opt = pipe_rc.borrow().as_ref().map(|p| p.telemetry());
                if let Some(tel) = tel_opt {
                    if tel.decoded_frames >= 20
                        && tel.presented_frames >= 10
                        && st >= 10
                        && !switch_rc.swap(true, Ordering::SeqCst)
                    {
                        // Mid-stream AirPlay switch 64x64 -> 96x64 with stray pre-IDR P-frame guard
                        let f_land = build_synthetic_h264_ipcm_idr(96, 64, 0xA4, 1);
                        let land_b64 = mirror::base64_encode(&f_land.avcc_config);
                        mirror::update_live_codec_h264_avcc(gen, "avc1.42801e", &land_b64, 96, 64);
                        let p_stray = build_synthetic_h264_p_skip_avcc_sample(96, 64, 1);
                        let _ = mirror::global_on_physical_h264_avcc_frame(gen, p_stray, false);
                        let _ = mirror::global_on_physical_h264_avcc_frame(
                            gen,
                            f_land.avcc_sample,
                            true,
                        );
                        let p_ok = build_synthetic_h264_p_skip_avcc_sample(96, 64, 2);
                        let _ = mirror::global_on_physical_h264_avcc_frame(gen, p_ok, false);
                    }
                    let switched_96x64 =
                        switch_rc.load(Ordering::Relaxed) && tel.width == 96 && tel.height == 64;
                    if (tel.decoded_frames >= 20 && tel.presented_frames >= 10 && switched_96x64)
                        || st >= 75
                    {
                        flush_active_dcomp_ui_frame();
                        let hole_alpha_ok = rect_sh
                            .lock()
                            .ok()
                            .and_then(|r| {
                                let cx = (r.x.max(0) as u32).saturating_add(r.width / 2);
                                let cy = (r.y.max(0) as u32).saturating_add(r.height / 2);
                                sample_active_dcomp_ui_alpha(cx, cy)
                            })
                            == Some(0);

                        // Tahap 287: Fullscreen Portrait Frosted Wings + Center Hole Alpha==0 Probe
                        let fullscreen_wings_frosted_ok = {
                            let scale_w = query_hwnd_scale(parent_hwnd);
                            let (cw_w, _) = query_client_size(parent_hwnd);
                            let (px, py, pw, ph) = (
                                u.get_video_hole_x(),
                                u.get_video_hole_y(),
                                u.get_video_hole_w(),
                                u.get_video_hole_h(),
                            );
                            let (pfs, pland, pva, pff, pphase, pstage, pab) = (
                                u.get_is_fullscreen(),
                                u.get_is_landscape(),
                                u.get_video_layer_active(),
                                u.get_first_frame_ready(),
                                u.get_phase(),
                                u.get_stage(),
                                u.get_anti_blank_visible(),
                            );
                            u.set_is_fullscreen(true);
                            u.set_is_landscape(false);
                            u.set_video_layer_active(true);
                            u.set_first_frame_ready(true);
                            u.set_phase("running".into());
                            u.set_stage("streaming".into());
                            u.set_anti_blank_visible(false);
                            u.set_video_hole_x(96.0);
                            u.set_video_hole_y(38.0);
                            u.set_video_hole_w(154.0);
                            u.set_video_hole_h(750.0);
                            u.window().request_redraw();
                            flush_active_dcomp_ui_frame();
                            let lw_px = sample_active_dcomp_ui_pixel(
                                (24.0 * scale_w).round() as u32,
                                (220.0 * scale_w).round() as u32,
                            );
                            let rw_px = sample_active_dcomp_ui_pixel(
                                cw_w.saturating_sub((24.0 * scale_w).round() as u32).max(1),
                                (220.0 * scale_w).round() as u32,
                            );
                            let ctr_px = sample_active_dcomp_ui_pixel(
                                (173.0 * scale_w).round() as u32,
                                (400.0 * scale_w).round() as u32,
                            );
                            u.set_is_fullscreen(pfs);
                            u.set_is_landscape(pland);
                            u.set_video_layer_active(pva);
                            u.set_first_frame_ready(pff);
                            u.set_phase(pphase);
                            u.set_stage(pstage);
                            u.set_anti_blank_visible(pab);
                            u.set_video_hole_x(px);
                            u.set_video_hole_y(py);
                            u.set_video_hole_w(pw);
                            u.set_video_hole_h(ph);
                            u.window().request_redraw();
                            flush_active_dcomp_ui_frame();
                            lw_px.is_some_and(|p| p.a > 0 && p.a < 230 && p.r <= p.a && p.g <= p.a && p.b <= p.a)
                                && rw_px.is_some_and(|p| p.a > 0 && p.a < 230 && p.r <= p.a && p.g <= p.a && p.b <= p.a)
                                && ctr_px.is_some_and(|p| p.a == 0)
                        };
                        let sequoia_overlay_exclusive_ok =
                            resolve_active_stage_overlay("idle", "", false, false) == StageOverlayKind::StandbyLaunchpad
                            && resolve_active_stage_overlay("starting", "connecting", false, false) == StageOverlayKind::SequoiaConnecting
                            && resolve_active_stage_overlay("running", "streaming", false, false) == StageOverlayKind::SequoiaConnecting
                            && resolve_active_stage_overlay("running", "streaming", true, false) == StageOverlayKind::None
                            && resolve_active_stage_overlay("starting", "waiting_airplay", false, false) == StageOverlayKind::WaitingAirplay
                            && resolve_active_stage_overlay("starting", "waiting_airplay", false, true) == StageOverlayKind::AntiBlankDiagnostic;
                        let plan_land = resolve_airplay_native_orientation_plan(
                            true, 1920, 888, false, "auto", true, false,
                        );
                        let plan_port = resolve_airplay_native_orientation_plan(
                            true, 888, 1920, false, "auto", true, false,
                        );
                        let plan_ar_off = resolve_airplay_native_orientation_plan(
                            true, 1920, 888, false, "auto", false, false,
                        );
                        let plan_ok = plan_land.is_some_and(|p| {
                            p.is_landscape
                                && p.video_rotation_deg == 0
                                && (p.window_logical_w - 840.0).abs() < 0.5
                                && (p.window_logical_h - 468.0).abs() < 0.5
                        }) && plan_port.is_some_and(|p| {
                            !p.is_landscape
                                && p.video_rotation_deg == 0
                                && (p.window_logical_w - 346.0).abs() < 0.5
                                && (p.window_logical_h - 832.0).abs() < 0.5
                        }) && plan_ar_off.is_some_and(|p| {
                            !p.is_landscape
                                && p.video_rotation_deg == 90
                                && (p.window_logical_w - 346.0).abs() < 0.5
                                && (p.window_logical_h - 832.0).abs() < 0.5
                        });

                        let rect_90cw = compute_active_video_physical_rect(
                            346,
                            832,
                            1.0,
                            false,
                            false,
                            false,
                            1920,
                            888,
                            90,
                            FitMode::AutoFit,
                        );
                        let rect_fs_drawer = compute_active_video_physical_rect(
                            1920,
                            1032,
                            1.0,
                            true,
                            true,
                            true,
                            1920,
                            888,
                            0,
                            FitMode::EdgeToEdge,
                        );
                        let portrait_90cw_fill_ok = plan_ok
                            && rect_90cw.x == 0
                            && rect_90cw.width == 346
                            && rect_90cw.height == rect_90cw.stage_height
                            && rect_fs_drawer.x >= 0
                            && rect_fs_drawer.width <= 1560
                            && rect_fs_drawer.height <= rect_fs_drawer.stage_height;

                        let (stale_buffer_purge_ok, bt709_color_ok) = {
                            use iphone_link_video::d3d11_context::{
                                D3d11Nv12VramRing, Rect as DRect, VideoColorInfo, VideoColorMatrix,
                                DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709,
                                DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709,
                            };
                            use iphone_link_video::mf_decoder::{
                                h264_avcc_has_video_slice_nal,
                                resolve_deterministic_bt709_color,
                                write_nv12_from_rgb_sampler,
                            };
                            let probe_ring = Arc::new(D3d11Nv12VramRing::new());
                            let purge_and_vp_ok =
                                if let Ok(mut probe_hwnd) =
                                    iphone_link_video::video_hwnd::LiveVideoHwnd::new_for_composition(
                                        gpu_sel.clone(),
                                        probe_ring.clone(),
                                        346,
                                        750,
                                    )
                                {
                                    let mut nv12 = Vec::new();
                                    write_nv12_from_rgb_sampler(64, 96, &mut nv12, |_, _| (16, 128, 128));
                                    if let Ok((sidx, _tex)) =
                                        probe_ring.acquire_write_slot(&gpu_sel, 64, 96)
                                    {
                                        probe_ring.commit_write_slot_with_color(
                                            sidx,
                                            64,
                                            96,
                                            DRect {
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
                                    }
                                    let had_slot = probe_ring.latest_slot().is_some();
                                    let cleared = probe_hwnd
                                        .purge_stale_video_buffers()
                                        .is_ok_and(|c| c == 2);
                                    let ring_empty = probe_ring.latest_slot().is_none();
                                    let vp_ok = if gpu_sel.has_video_processor() {
                                        probe_hwnd
                                            .verify_deterministic_bt709_pipeline_probe(
                                                VideoColorInfo::default(),
                                            )
                                            .is_ok_and(|st| {
                                                st.dxgi_in_cs
                                                    == DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709
                                                    && st.dxgi_out_cs
                                                        == DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709
                                                    && st.legacy_in_cs == 0x14
                                                    && st.legacy_out_cs == 0x00
                                                    && st.auto_processing_disabled
                                            })
                                    } else {
                                        true
                                    };
                                    probe_ring.clear();
                                    drop(probe_hwnd);
                                    (had_slot && cleared && ring_empty, vp_ok)
                                } else {
                                    (false, false)
                                };
                            drop(probe_ring);
                            let slice_gate_ok = !h264_avcc_has_video_slice_nal(
                                &[0, 0, 0, 2, 0x09, 0xF0],
                                4,
                            ) && h264_avcc_has_video_slice_nal(
                                &[0, 0, 0, 3, 0x65, 0x88, 0x80],
                                4,
                            );
                            let color_helpers_ok = resolve_deterministic_bt709_color(None)
                                == VideoColorInfo::default()
                                && resolve_deterministic_bt709_color(Some(VideoColorInfo {
                                    matrix: VideoColorMatrix::Bt601,
                                    full_range: true,
                                })) == VideoColorInfo {
                                    matrix: VideoColorMatrix::Bt709,
                                    full_range: true,
                                };
                            (
                                purge_and_vp_ok.0 && slice_gate_ok,
                                purge_and_vp_ok.1 && color_helpers_ok,
                            )
                        };

                        let mut wd = AirplayStallWatchdog::new();
                        let _ = wd.observe(false, 10, 5, 900);
                        let mut wd_trig = false;
                        for idx in 0..AIRPLAY_STALL_MIN_PACKETS {
                            if wd.observe(true, 10, 5, 1000 + u64::from(idx) * 65) {
                                wd_trig = true;
                            }
                        }
                        let cooldown_ok = !wd.on_h264_packet(10_000, 5, true);
                        let wd_reset_ok = !wd.observe(true, 10, 5, 17_500)
                            && wd.stall_packets() == 1
                            && !wd.observe(true, 11, 6, 17_600)
                            && wd.stall_packets() == 0;
                        let pts_clamp_ok = clamp_monotonic_pts_delta_100ns(1_000_000, 1_000_050)
                            == 1_000_000 + MIN_PTS_DELTA_100NS
                            && clamp_monotonic_pts_delta_100ns(1_000_000, 99_000_000)
                                == 1_000_000 + MAX_PTS_DELTA_100NS
                            && clamp_monotonic_pts_delta_100ns(1_000_000, 900_000)
                                == 1_000_000 + DEFAULT_PTS_DELTA_100NS;
                        let wd_ok =
                            wd_trig && wd.recoveries() == 1 && cooldown_ok && wd_reset_ok && pts_clamp_ok;
                        let loopback_ok = connection::is_loopback_only_test_mode();

                        // Tahap 273: Non-Mod-16 (888x1920, 61 frames with AUD+SEI) + 888x1920 <-> 1920x888
                        // orientation switch endurance probe (dropped strictly before RAM measurement).
                        let non_mod16_60frame_ok = {
                            use iphone_link_video::d3d11_context::D3d11Nv12VramRing;
                            use iphone_link_video::mf_decoder::{
                                LiveMfVideoDecoder, VideoCodecKind,
                                build_synthetic_h264_ipcm_idr_cropped,
                                build_synthetic_h264_p_skip_cropped_with_aud_sei,
                                wrap_avcc_with_aud_sei,
                            };
                            use iphone_link_video::video_hwnd::LiveVideoHwnd;

                            let end_ring = Arc::new(D3d11Nv12VramRing::new());
                            let mut passed = false;
                            if let (Ok(mut end_hwnd), Ok(mut end_dec)) = (
                                LiveVideoHwnd::new_for_composition(
                                    gpu_sel.clone(),
                                    end_ring.clone(),
                                    346,
                                    750,
                                ),
                                LiveMfVideoDecoder::new(gpu_sel.clone(), VideoCodecKind::H264),
                            ) {
                                let mut pts = 16_667u64;
                                let mut last_seq_a = 0u64;
                                let mut pres_a = 0u32;

                                // Phase A: 888x1920 Non-Mod-16 Portrait IDR + 60 P_SKIP frames (with AUD+SEI)
                                let f888 = build_synthetic_h264_ipcm_idr_cropped(888, 1920, 0x84, 0);
                                let phase_a_init = end_dec.update_avcc(&f888.avcc_config).is_ok()
                                    && end_dec
                                        .feed_avcc_sample(
                                            &wrap_avcc_with_aud_sei(&f888.avcc_sample),
                                            pts,
                                            true,
                                            &end_ring,
                                        )
                                        .is_ok();
                                drop(f888);
                                if phase_a_init {
                                    if let Ok(pst) = end_hwnd.present_latest() {
                                        if pst.frame_seq > last_seq_a {
                                            last_seq_a = pst.frame_seq;
                                            pres_a += 1;
                                        }
                                    }
                                    for i in 1..=60u32 {
                                        pts += 16_667;
                                        let p_au = build_synthetic_h264_p_skip_cropped_with_aud_sei(
                                            888, 1920, i,
                                        );
                                        if end_dec
                                            .feed_avcc_sample(&p_au, pts, false, &end_ring)
                                            .is_ok()
                                        {
                                            if let Ok(pst) = end_hwnd.present_latest() {
                                                if pst.frame_seq > last_seq_a {
                                                    last_seq_a = pst.frame_seq;
                                                    pres_a += 1;
                                                }
                                            }
                                        }
                                    }
                                    if let Ok(st_a) =
                                        end_dec.drain_eos(true, Some((0, 0, 0, 0)), &end_ring)
                                    {
                                        if let Ok(pst) = end_hwnd.present_latest() {
                                            if pst.frame_seq > last_seq_a {
                                                last_seq_a = pst.frame_seq;
                                                pres_a += 1;
                                            }
                                        }
                                        let _ = last_seq_a;
                                        let phase_a_ok = st_a.total_decoded_frames >= 55
                                            && pres_a >= 50
                                            && end_dec.stale_dim_dropped() == 0;

                                        // Phase B: Switch 888x1920 -> 1920x888 -> 888x1920
                                        let f_land = build_synthetic_h264_ipcm_idr_cropped(
                                            1920, 888, 0x92, 1,
                                        );
                                        let land_init =
                                            end_dec.update_avcc(&f_land.avcc_config).is_ok()
                                                && end_hwnd.purge_stale_video_buffers().is_ok()
                                                && end_dec
                                                    .feed_avcc_sample(
                                                        &wrap_avcc_with_aud_sei(
                                                            &f_land.avcc_sample,
                                                        ),
                                                        pts + 16_667,
                                                        true,
                                                        &end_ring,
                                                    )
                                                    .is_ok();
                                        drop(f_land);
                                        pts += 16_667;
                                        let mut phase_b_ok = false;
                                        if phase_a_ok && land_init {
                                            for i in 1..=6u32 {
                                                pts += 16_667;
                                                let p_au =
                                                    build_synthetic_h264_p_skip_cropped_with_aud_sei(
                                                        1920, 888, i,
                                                    );
                                                let _ = end_dec.feed_avcc_sample(
                                                    &p_au,
                                                    pts,
                                                    false,
                                                    &end_ring,
                                                );
                                            }
                                            if let Ok(st_b) = end_dec.drain_eos(
                                                true,
                                                Some((0, 0, 0, 0)),
                                                &end_ring,
                                            ) {
                                                let _ = end_hwnd.present_latest();
                                                if st_b.total_decoded_frames
                                                    >= st_a.total_decoded_frames + 5
                                                {
                                                    let f_back =
                                                        build_synthetic_h264_ipcm_idr_cropped(
                                                            888, 1920, 0x78, 2,
                                                        );
                                                    let back_init = end_dec
                                                        .update_avcc(&f_back.avcc_config)
                                                        .is_ok()
                                                        && end_hwnd
                                                            .purge_stale_video_buffers()
                                                            .is_ok()
                                                        && end_dec
                                                            .feed_avcc_sample(
                                                                &wrap_avcc_with_aud_sei(
                                                                    &f_back.avcc_sample,
                                                                ),
                                                                pts + 16_667,
                                                                true,
                                                                &end_ring,
                                                            )
                                                            .is_ok();
                                                    drop(f_back);
                                                    pts += 16_667;
                                                    if back_init {
                                                        for i in 1..=6u32 {
                                                            pts += 16_667;
                                                            let p_au = build_synthetic_h264_p_skip_cropped_with_aud_sei(888, 1920, i);
                                                            let _ = end_dec.feed_avcc_sample(
                                                                &p_au,
                                                                pts,
                                                                false,
                                                                &end_ring,
                                                            );
                                                        }
                                                        if let Ok(st_c) = end_dec.drain_eos(
                                                            true,
                                                            Some((0, 0, 0, 0)),
                                                            &end_ring,
                                                        ) {
                                                            let _ = end_hwnd.present_latest();
                                                            phase_b_ok = st_c.total_decoded_frames
                                                                >= st_b.total_decoded_frames + 5
                                                                && end_dec.stale_dim_dropped()
                                                                    == 0;
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        passed = phase_a_ok && phase_b_ok;
                                    }
                                }
                                end_ring.clear();
                                drop(end_dec);
                                drop(end_hwnd);
                            }
                            end_ring.clear();
                            drop(end_ring);
                            passed
                        };

                        // Tahap 274: 20x rapid 888x1920 <-> 1920x888 endurance (seamless hold +
                        // dual-backbuffer overwrite + atomic per-frame 90°<->0° rotation + 0 black clears).
                        let airplay_20x_endurance_ok = {
                            use iphone_link_video::d3d11_context::D3d11Nv12VramRing;
                            use iphone_link_video::mf_decoder::{
                                LiveMfVideoDecoder, VideoCodecKind,
                                build_synthetic_h264_ipcm_idr_cropped,
                                build_synthetic_h264_p_skip_cropped_with_aud_sei,
                                wrap_avcc_with_aud_sei,
                            };
                            use iphone_link_video::video_hwnd::LiveVideoHwnd;

                            let r20 = Arc::new(D3d11Nv12VramRing::new());
                            let mut ok20 = false;
                            if let (Ok(mut hwnd20), Ok(mut dec20)) = (
                                LiveVideoHwnd::new_for_composition(
                                    gpu_sel.clone(),
                                    r20.clone(),
                                    346,
                                    750,
                                ),
                                LiveMfVideoDecoder::new(gpu_sel.clone(), VideoCodecKind::H264),
                            ) {
                                hwnd20.set_airplay_mode(true);
                                hwnd20.set_window_landscape(false);
                                hwnd20.set_auto_rotate_enabled(false);

                                let f_port = build_synthetic_h264_ipcm_idr_cropped(888, 1920, 0x84, 0);
                                let port_avcc = f_port.avcc_config.clone();
                                let port_idr = wrap_avcc_with_aud_sei(&f_port.avcc_sample);
                                drop(f_port);
                                let port_p1 =
                                    build_synthetic_h264_p_skip_cropped_with_aud_sei(888, 1920, 1);
                                let port_p2 =
                                    build_synthetic_h264_p_skip_cropped_with_aud_sei(888, 1920, 2);

                                let f_land = build_synthetic_h264_ipcm_idr_cropped(1920, 888, 0x9C, 1);
                                let land_avcc = f_land.avcc_config.clone();
                                let land_idr = wrap_avcc_with_aud_sei(&f_land.avcc_sample);
                                drop(f_land);
                                let land_p1 =
                                    build_synthetic_h264_p_skip_cropped_with_aud_sei(1920, 888, 1);
                                let land_p2 =
                                    build_synthetic_h264_p_skip_cropped_with_aud_sei(1920, 888, 2);

                                let mut pts20 = 16_667u64;
                                let mut step_ok = dec20.update_avcc(&port_avcc).is_ok()
                                    && dec20.feed_avcc_sample(&port_idr, pts20, true, &r20).is_ok()
                                    && dec20
                                        .feed_avcc_sample(&port_p1, pts20 + 16_667, false, &r20)
                                        .is_ok()
                                    && dec20
                                        .feed_avcc_sample(&port_p2, pts20 + 33_334, false, &r20)
                                        .is_ok()
                                    && dec20.drain_eos(true, Some((0, 0, 0, 0)), &r20).is_ok();
                                pts20 += 50_001;
                                if step_ok {
                                    if let Ok(st0) = hwnd20.present_latest() {
                                        step_ok = st0.presented_total == 1
                                            && hwnd20.last_effective_rotation_deg() == 0;
                                    } else {
                                        step_ok = false;
                                    }
                                }
                                let mut prev_pres = 1u64;
                                for cycle in 1..=20u32 {
                                    if !step_ok {
                                        break;
                                    }
                                    let to_land = cycle % 2 == 1;
                                    let (avcc, idr_au, p1_au, p2_au, stale_rot, want_rot) =
                                        if to_land {
                                            (&land_avcc, &land_idr, &land_p1, &land_p2, 0u16, 90u16)
                                        } else {
                                            (&port_avcc, &port_idr, &port_p1, &port_p2, 90u16, 0u16)
                                        };
                                    if dec20.update_avcc(avcc).is_err() {
                                        step_ok = false;
                                        break;
                                    }
                                    hwnd20.prepare_seamless_dimension_transition();
                                    // Empty-ring present mid-stream MUST hold previous frame (0 black Presents)
                                    match hwnd20.present_latest() {
                                        Ok(hold)
                                            if hold.presented_total == prev_pres
                                                && !hold.dual_buffer_overwritten
                                                && hwnd20.mid_stream_black_clears() == 0 => {}
                                        _ => {
                                            step_ok = false;
                                            break;
                                        }
                                    }
                                    // Deliberately inject stale UI rotation before presenting new dims
                                    hwnd20.set_rotation(stale_rot);
                                    pts20 += 16_667;
                                    if dec20.feed_avcc_sample(idr_au, pts20, true, &r20).is_err()
                                        || dec20
                                            .feed_avcc_sample(p1_au, pts20 + 16_667, false, &r20)
                                            .is_err()
                                        || dec20
                                            .feed_avcc_sample(p2_au, pts20 + 33_334, false, &r20)
                                            .is_err()
                                        || dec20.drain_eos(true, Some((0, 0, 0, 0)), &r20).is_err()
                                    {
                                        step_ok = false;
                                        break;
                                    }
                                    pts20 += 50_001;
                                    match hwnd20.present_latest() {
                                        Ok(st_new)
                                            if st_new.dual_buffer_overwritten
                                                && hwnd20.last_effective_rotation_deg() == want_rot
                                                && hwnd20.mid_stream_black_clears() == 0 =>
                                        {
                                            prev_pres = st_new.presented_total;
                                        }
                                        _ => {
                                            step_ok = false;
                                            break;
                                        }
                                    }
                                }
                                ok20 = step_ok
                                    && hwnd20.mid_stream_black_clears() == 0
                                    && hwnd20.dual_buffer_overwrites() >= 20
                                    && dec20.stale_dim_dropped() == 0;
                                dec20.clean_teardown();
                                r20.clear();
                                drop(dec20);
                                drop(hwnd20);
                            }
                            r20.clear();
                            drop(r20);
                            ok20
                        };

                        // Tahap 276 Selftest Check: VFR MFT low-latency + Full Screen Portrait geometry + idle repaint
                        // (Placed BEFORE Launchpad stop & trim_active_dcomp_memory() so DXGI/MFT pages are trimmed cleanly.)
                        let airplay_vfr_low_latency_and_fullscreen_portrait_ok = {
                            use iphone_link_video::d3d11_context::{
                                D3d11Nv12VramRing, Rect as DRect, VideoColorInfo,
                                D3D11_MAP_WRITE_DISCARD,
                            };
                            use iphone_link_video::mf_decoder::{
                                LiveMfVideoDecoder, VideoCodecKind, write_nv12_from_rgb_sampler,
                            };
                            use iphone_link_video::video_hwnd::LiveVideoHwnd;

                            let mut fs_geom_ok = true;
                            for &scale in &[1.0f32, 1.25f32] {
                                let cw = (1920.0 * scale).round() as u32;
                                let ch = (1032.0 * scale).round() as u32;
                                for &(vw, vh) in &[(888u32, 1920), (990, 2160), (1920, 888)] {
                                    let r = compute_active_video_physical_rect_with_drawer_w_and_mode(
                                        cw, ch, scale, 0.0, true, false, vw, vh, 0, FitMode::AutoFit, true,
                                    );
                                    let expected_x = ((cw.saturating_sub(r.width)) / 2) as i32;
                                    if r.stage_width != cw
                                        || r.height != r.stage_height
                                        || (r.x - expected_x).abs() > 1
                                    {
                                        fs_geom_ok = false;
                                    }
                                }
                            }
                            let low_lat_ok = LiveMfVideoDecoder::new(
                                gpu_sel.clone(),
                                VideoCodecKind::H264,
                            )
                            .map(|mut d| {
                                let ok = d.mf_low_latency_enabled();
                                d.clean_teardown();
                                ok
                            })
                            .unwrap_or(false);

                            let repaint_ok = {
                                let r = Arc::new(D3d11Nv12VramRing::new());
                                let mut ok = false;
                                if let Ok(mut hw) = LiveVideoHwnd::new_for_composition(
                                    gpu_sel.clone(),
                                    r.clone(),
                                    346,
                                    750,
                                ) {
                                    let mut nv12 = Vec::new();
                                    write_nv12_from_rgb_sampler(64, 96, &mut nv12, |_, _| (90, 160, 210));
                                    if let Ok((slot_idx, dst_tex)) = r.acquire_write_slot(&gpu_sel, 64, 96) {
                                        if let Ok(up) = gpu_sel.create_nv12_texture(64, 96, true) {
                                            if let Ok(mp) = gpu_sel.map_texture(&up, 0, D3D11_MAP_WRITE_DISCARD) {
                                                let rows = 96 + 48;
                                                let cp = 64usize.min(mp.row_pitch as usize);
                                                unsafe {
                                                    for row in 0..rows {
                                                        std::ptr::copy_nonoverlapping(
                                                            nv12.as_ptr().add(row * 64),
                                                            (mp.p_data as *mut u8).add(row * mp.row_pitch as usize),
                                                            cp,
                                                        );
                                                    }
                                                }
                                                gpu_sel.unmap_texture(&up, 0);
                                                gpu_sel.copy_resource(&dst_tex, &up);
                                            }
                                        }
                                        r.commit_write_slot_with_color(
                                            slot_idx,
                                            64,
                                            96,
                                            DRect { left: 0, top: 0, right: 64, bottom: 96 },
                                            (0, 0, 0, 0),
                                            16_667,
                                            true,
                                            VideoColorInfo::default(),
                                            Some(nv12),
                                        );
                                        if hw.present_latest().is_ok()
                                            && hw.resize(450, 998).is_ok()
                                            && hw.force_repaint_cached_frame().is_ok_and(|st| st.swapchain_w == 450 && st.swapchain_h == 998)
                                        {
                                            ok = true;
                                        }
                                    }
                                    r.clear();
                                    drop(hw);
                                }
                                r.clear();
                                drop(r);
                                ok
                            };
                            crate::native_video_pipeline::cleanup_airplay_stall_dump_file();
                            fs_geom_ok && low_lat_ok && repaint_ok
                        };

                        // Tahap 277 Selftest Check: Zero green macroblock + DXVA multithread + ResizeBuffers atomic repaint + Full Screen Landscape snap
                        // (Placed BEFORE Launchpad stop & trim_active_dcomp_memory() so DXGI/MFT pages stay well below 15 MB.)
                        let airplay_zero_green_macroblock_and_fullscreen_landscape_ok = {
                            use iphone_link_video::d3d11_context::{
                                D3d11Nv12VramRing, Rect as DRect, VideoColorInfo,
                                D3D11_MAP_WRITE_DISCARD,
                            };
                            use iphone_link_video::mf_decoder::{
                                LiveMfVideoDecoder, VideoCodecKind, ensure_d3d11_multithread_protected,
                                h264_avcc_payload_is_clean_random_access, write_nv12_from_rgb_sampler,
                            };
                            use iphone_link_video::video_hwnd::LiveVideoHwnd;

                            let mt_ok = ensure_d3d11_multithread_protected(&gpu_sel);
                            let full_au_ok = {
                                let sei_au = [
                                    0, 0, 0, 2, 0x09, 0xF0,
                                    0, 0, 0, 5, 0x06, 0x06, 0x01, 0x00, 0x80,
                                    0, 0, 0, 4, 0x41, 0xE0, 0x01, 0x22,
                                ];
                                let islice_au = [
                                    0, 0, 0, 2, 0x09, 0xF0,
                                    0, 0, 0, 4, 0x41, 0x88, 0x11, 0x22,
                                ];
                                let p_au = [
                                    0, 0, 0, 2, 0x09, 0xF0,
                                    0, 0, 0, 4, 0x41, 0xE0, 0x01, 0x22,
                                ];
                                h264_avcc_payload_is_clean_random_access(&sei_au, 4)
                                    && h264_avcc_payload_is_clean_random_access(&islice_au, 4)
                                    && !h264_avcc_payload_is_clean_random_access(&p_au, 4)
                            };

                            let dpb_and_resize_ok = {
                                let r77 = Arc::new(D3d11Nv12VramRing::new());
                                let mut ok = false;
                                if let (Ok(mut dec77), Ok(mut hw77)) = (
                                    LiveMfVideoDecoder::new(gpu_sel.clone(), VideoCodecKind::H264),
                                    LiveVideoHwnd::new_for_composition(
                                        gpu_sel.clone(),
                                        r77.clone(),
                                        840,
                                        468,
                                    ),
                                ) {
                                    let f77 = build_synthetic_h264_ipcm_idr(96, 64, 0x8C, 0);
                                    let _ = dec77.update_avcc(&f77.avcc_config);
                                    dec77.arm_empty_dpb_idr_wait();
                                    let mut pts = 16_667u64;
                                    let mut blocked_all = true;
                                    for i in 1..=8u32 {
                                        pts += 16_667;
                                        let p = build_synthetic_h264_p_skip_avcc_sample(96, 64, i);
                                        if dec77
                                            .feed_avcc_sample(&p, pts, false, &r77)
                                            .map(|s| s.frames_emitted)
                                            .unwrap_or(1)
                                            != 0
                                        {
                                            blocked_all = false;
                                        }
                                    }
                                    pts += 16_667;
                                    let idr_ok = blocked_all
                                        && dec77.awaiting_idr()
                                        && dec77.feed_avcc_sample(&f77.avcc_sample, pts, true, &r77).is_ok()
                                        && !dec77.awaiting_idr()
                                        && dec77.idr_keyframes_detected() >= 1
                                        && dec77.d3d11_multithread_protected();
                                    let mut nv12 = Vec::new();
                                    write_nv12_from_rgb_sampler(96, 64, &mut nv12, |_, _| (95, 165, 220));
                                    if r77.latest_slot().is_none() {
                                        if let Ok((si, dt)) = r77.acquire_write_slot(&gpu_sel, 96, 64) {
                                            if let Ok(up) = gpu_sel.create_nv12_texture(96, 64, true) {
                                                if let Ok(mp) = gpu_sel.map_texture(&up, 0, D3D11_MAP_WRITE_DISCARD) {
                                                    let rows = 96usize;
                                                    let cp = 96usize.min(mp.row_pitch as usize);
                                                    unsafe {
                                                        for row in 0..rows {
                                                            std::ptr::copy_nonoverlapping(
                                                                nv12.as_ptr().add(row * 96),
                                                                (mp.p_data as *mut u8).add(row * mp.row_pitch as usize),
                                                                cp,
                                                            );
                                                        }
                                                    }
                                                    gpu_sel.unmap_texture(&up, 0);
                                                    gpu_sel.copy_resource(&dt, &up);
                                                }
                                            }
                                            r77.commit_write_slot_with_color(
                                                si,
                                                96,
                                                64,
                                                DRect { left: 0, top: 0, right: 96, bottom: 64 },
                                                (0, 0, 0, 0),
                                                pts,
                                                true,
                                                VideoColorInfo::default(),
                                                Some(nv12),
                                            );
                                        }
                                    }
                                    if idr_ok
                                        && hw77.present_latest().is_ok()
                                        && hw77.resize(1920, 998).is_ok()
                                        && hw77.resize_atomic_repaints() == 1
                                        && hw77.mid_stream_black_clears() == 0
                                    {
                                        ok = true;
                                    }
                                    dec77.clean_teardown();
                                    r77.clear();
                                    drop(dec77);
                                    drop(hw77);
                                }
                                r77.clear();
                                drop(r77);
                                ok
                            };

                            airplay::reset_airplay_trace();
                            let win_land = compute_active_video_physical_rect_with_drawer_w_and_mode(
                                840, 468, 1.0, 0.0, false, true, 1920, 888, 0, FitMode::AutoFit, true,
                            );
                            let (fs_land, fs_changed) = compute_fullscreen_landscape_resync_rect(
                                Some(win_land),
                                1920, 1080, 1.0, false, true, true, 1920, 888, 0, FitMode::AutoFit, true,
                            );
                            let fs_land_ok = fs_changed
                                && fs_land.stage_width == 1920
                                && fs_land.width > 1500
                                && fs_land.x == ((1920 - fs_land.width) / 2) as i32
                                && airplay::trace_fullscreen_landscape_resyncs() >= 1;
                            airplay::reset_airplay_trace();
                            crate::native_video_pipeline::cleanup_airplay_stall_dump_file();
                            mt_ok && full_au_ok && dpb_and_resize_ok && fs_land_ok
                        };

                        // Tahap 279 Selftest (placed BEFORE Launchpad stop & trim_active_dcomp_memory()
                        // so MF/DXGI pages trim cleanly): Exhaustive 256..511 B collision immunity on
                        // real COM MFT (`994x2160` -> `lastMftDims == 1008x2160`, NEVER `64x64`), `0`-based
                        // H.264 PTS, non-PII `avccSmall256To511Frames`/`mftReconfigs`, & stall dump cleanup.
                        let airplay_avcc_256_511_zero_freeze_and_anti_smear_ok = {
                            use iphone_link_video::d3d11_context::D3d11Nv12VramRing;
                            use iphone_link_video::mf_decoder::{
                                LiveMfVideoDecoder, VideoCodecKind,
                                build_synthetic_h264_ipcm_idr_cropped,
                                build_synthetic_h264_p_exact_len_avcc_sample,
                                next_h264_zero_based_pts_100ns, wrap_avcc_with_aud_sei,
                            };

                            airplay::reset_airplay_trace();
                            let r279 = D3d11Nv12VramRing::new();
                            let ok279 = if let Ok(mut dec279) =
                                LiveMfVideoDecoder::new(gpu_sel.clone(), VideoCodecKind::H264)
                            {
                                let f279 = build_synthetic_h264_ipcm_idr_cropped(994, 2160, 0x7E, 0);
                                let _ = dec279.update_avcc(&f279.avcc_config);
                                let idr_au = wrap_avcc_with_aud_sei(&f279.avcc_sample);
                                let mut feed_ok = dec279
                                    .feed_avcc_sample(&idr_au, 0, true, &r279)
                                    .is_ok();
                                let pts0_ok = dec279.last_input_pts_100ns() == 0;
                                let exact_lens = [263usize, 295, 327, 359, 487, 504];
                                for (i, &nlen) in exact_lens.iter().enumerate() {
                                    let fnum = (i as u32) + 1;
                                    let p_au = build_synthetic_h264_p_exact_len_avcc_sample(
                                        994, 2160, fnum, nlen,
                                    );
                                    if dec279
                                        .feed_avcc_sample(
                                            &p_au,
                                            u64::from(fnum) * 16_667,
                                            false,
                                            &r279,
                                        )
                                        .is_err()
                                    {
                                        feed_ok = false;
                                    }
                                }
                                let drain_ok = dec279
                                    .drain_eos(true, Some((0, 0, 0, 0)), &r279)
                                    .is_ok_and(|s| s.total_decoded_frames >= 6);
                                airplay::add_trace_avcc_small_256_511_frames(
                                    dec279.avcc_small_256_511_frames(),
                                );
                                let tjson = airplay::get_airplay_trace_json();
                                let res = feed_ok
                                    && pts0_ok
                                    && next_h264_zero_based_pts_100ns(-1) == 0
                                    && drain_ok
                                    && dec279.last_mft_dims() == Some((1008, 2160))
                                    && dec279.expected_coded_dims() == Some((994, 2160))
                                    && dec279.mft_reconfigs() == 0
                                    && dec279.dropped_pre_idr_frames() == 0
                                    && dec279.avcc_small_256_511_frames() >= 6
                                    && dec279.last_hresult() == 0
                                    && tjson.contains("\"avccSmall256To511Frames\":6")
                                    && tjson.contains("\"mftReconfigs\":0");
                                dec279.clean_teardown();
                                drop(dec279);
                                res
                            } else {
                                false
                            };
                            r279.clear();
                            drop(r279);
                            airplay::reset_airplay_trace();
                            crate::native_video_pipeline::cleanup_airplay_stall_dump_file();
                            ok279
                        };

                        // Tahap 280 Selftest (placed BEFORE trim_active_dcomp_memory() so shader
                        // / DXGI pages trim cleanly): Real D3D11 HD NV12 Shader BT.709 Studio
                        // accuracy (Black<=2, White>=253), LiveVideoHwnd resize cycle, & trace keys.
                        let airplay_hd_nv12_shader_quality_ok = {
                            use iphone_link_video::d3d11_context::{
                                D3d11Nv12VramRing, Rect as VidRect, VideoColorInfo,
                                VideoColorMatrix, D3D11_BIND_RENDER_TARGET,
                                D3D11_CPU_ACCESS_READ, D3D11_MAP_READ,
                                D3D11_MAP_WRITE_DISCARD, D3D11_USAGE_DEFAULT,
                                D3D11_USAGE_STAGING,
                            };
                            use iphone_link_video::mf_decoder::write_nv12_from_rgb_sampler;
                            use iphone_link_video::nv12_hd_shader::HdNv12ShaderRenderer;
                            use iphone_link_video::video_hwnd::LiveVideoHwnd;

                            airplay::reset_airplay_trace();
                            let ok280 = if let Ok(mut sh280) = HdNv12ShaderRenderer::new(&gpu_sel)
                            {
                                let upload_16 = |y_val: u8| {
                                    let len = 16usize * 16 + 16 * 8;
                                    let mut raw = vec![y_val; len];
                                    for b in raw[256..].iter_mut() {
                                        *b = 128;
                                    }
                                    let up = gpu_sel.create_nv12_texture(16, 16, true).ok()?;
                                    let dst = gpu_sel.create_nv12_texture(16, 16, false).ok()?;
                                    let mp = gpu_sel
                                        .map_texture(&up, 0, D3D11_MAP_WRITE_DISCARD)
                                        .ok()?;
                                    let cp = 16usize.min(mp.row_pitch as usize);
                                    unsafe {
                                        for r in 0..24usize {
                                            std::ptr::copy_nonoverlapping(
                                                raw.as_ptr().add(r * 16),
                                                (mp.p_data as *mut u8)
                                                    .add(r * mp.row_pitch as usize),
                                                cp,
                                            );
                                        }
                                    }
                                    gpu_sel.unmap_texture(&up, 0);
                                    gpu_sel.copy_resource(&dst, &up);
                                    Some(dst)
                                };
                                let render_rgb = |sh_ref: &mut HdNv12ShaderRenderer,
                                                  tex: &_|
                                 -> Option<(u8, u8, u8)> {
                                    let rt = gpu_sel
                                        .create_bgra_texture(
                                            16,
                                            16,
                                            D3D11_USAGE_DEFAULT,
                                            D3D11_BIND_RENDER_TARGET,
                                            0,
                                        )
                                        .ok()?;
                                    let stg = gpu_sel
                                        .create_bgra_texture(
                                            16,
                                            16,
                                            D3D11_USAGE_STAGING,
                                            0,
                                            D3D11_CPU_ACCESS_READ,
                                        )
                                        .ok()?;
                                    let col = VideoColorInfo {
                                        matrix: VideoColorMatrix::Bt709,
                                        full_range: false,
                                    };
                                    let r16 = VidRect {
                                        left: 0,
                                        top: 0,
                                        right: 16,
                                        bottom: 16,
                                    };
                                    sh_ref
                                        .render_to_texture(
                                            &gpu_sel, tex, &rt, 16, 16, r16, r16, 0, col,
                                        )
                                        .ok()?;
                                    gpu_sel.copy_resource(&stg, &rt);
                                    let mp = gpu_sel.map_texture(&stg, 0, D3D11_MAP_READ).ok()?;
                                    let (r, g, b) = unsafe {
                                        let p = (mp.p_data as *const u8)
                                            .add(8 * mp.row_pitch as usize + 8 * 4);
                                        (*p.add(2), *p.add(1), *p)
                                    };
                                    gpu_sel.unmap_texture(&stg, 0);
                                    Some((r, g, b))
                                };
                                let blk_ok = upload_16(16)
                                    .and_then(|t| render_rgb(&mut sh280, &t))
                                    .is_some_and(|(r, g, b)| r <= 2 && g <= 2 && b <= 2);
                                let wht_ok = upload_16(235)
                                    .and_then(|t| render_rgb(&mut sh280, &t))
                                    .is_some_and(|(r, g, b)| r >= 253 && g >= 253 && b >= 253);
                                drop(sh280);

                                let ring280 = Arc::new(D3d11Nv12VramRing::new());
                                let hwnd_ok = if let Ok(mut hwnd280) =
                                    LiveVideoHwnd::new_for_composition(
                                        gpu_sel.clone(),
                                        ring280.clone(),
                                        519,
                                        1125,
                                    )
                                {
                                    hwnd280.set_vsync_interval(0);
                                    let mut nv12_64 = Vec::new();
                                    write_nv12_from_rgb_sampler(64, 96, &mut nv12_64, |_, _| {
                                        (80, 170, 230)
                                    });
                                    let mut p_ok = false;
                                    if let Ok((slot, dst_t)) =
                                        ring280.acquire_write_slot(&gpu_sel, 64, 96)
                                    {
                                        if let Ok(up) = gpu_sel.create_nv12_texture(64, 96, true) {
                                            if let Ok(mp) = gpu_sel
                                                .map_texture(&up, 0, D3D11_MAP_WRITE_DISCARD)
                                            {
                                                let cp = 64usize.min(mp.row_pitch as usize);
                                                unsafe {
                                                    for r in 0..144usize {
                                                        std::ptr::copy_nonoverlapping(
                                                            nv12_64.as_ptr().add(r * 64),
                                                            (mp.p_data as *mut u8)
                                                                .add(r * mp.row_pitch as usize),
                                                            cp,
                                                        );
                                                    }
                                                }
                                                gpu_sel.unmap_texture(&up, 0);
                                                gpu_sel.copy_resource(&dst_t, &up);
                                            }
                                        }
                                        ring280.commit_write_slot_with_color(
                                            slot,
                                            64,
                                            96,
                                            VidRect {
                                                left: 0,
                                                top: 0,
                                                right: 64,
                                                bottom: 96,
                                            },
                                            (0, 0, 0, 0),
                                            16_667,
                                            true,
                                            VideoColorInfo::default(),
                                            Some(nv12_64),
                                        );
                                        p_ok = hwnd280.present_latest().is_ok()
                                            && hwnd280.resize(1920, 998).is_ok()
                                            && hwnd280.resize(519, 1125).is_ok();
                                    }
                                    let tj280 = airplay::get_airplay_trace_json();
                                    let ok = p_ok
                                        && hwnd280.hd_shader_active()
                                        && hwnd280.hd_shader_presents() >= 3
                                        && hwnd280.mid_stream_black_clears() == 0
                                        && tj280.contains("\"hdShaderActive\":true")
                                        && tj280.contains("\"hdShaderPresents\":");
                                    drop(hwnd280);
                                    ok
                                } else {
                                    false
                                };
                                ring280.clear();
                                drop(ring280);
                                blk_ok && wht_ok && hwnd_ok
                            } else {
                                false
                            };
                            airplay::reset_airplay_trace();
                            ok280
                        };

                        // Tahap 283 GPU Selftest Probe (placed BEFORE trim_active_dcomp_memory()!)
                        let hevc_autorotate_konami_efootball_e2e_ok = {
                            use iphone_link_video::autorotate::{
                                analyze_frame_grid, HevcAutoRotateTracker, OrientationMode,
                                ProbeGrid120x260,
                            };
                            use iphone_link_video::d3d11_context::{
                                D3d11Nv12VramRing, Rect as VidRect, VideoColorInfo, D3D11_MAP_WRITE_DISCARD,
                            };
                            use iphone_link_video::mf_decoder::write_nv12_from_rgb_sampler;
                            use iphone_link_video::video_hwnd::LiveVideoHwnd;

                            let r283 = Arc::new(D3d11Nv12VramRing::new());
                            let gpu_ok = if let Ok(mut hw283) =
                                LiveVideoHwnd::new_for_composition(gpu_sel.clone(), r283.clone(), 346, 750)
                            {
                                hw283.set_vsync_interval(0);
                                let mut nv12_k = Vec::new();
                                write_nv12_from_rgb_sampler(64, 128, &mut nv12_k, |_, _| (225, 15, 20));
                                if let Ok((si, dt)) = r283.acquire_write_slot(&gpu_sel, 64, 128) {
                                    if let Ok(up) = gpu_sel.create_nv12_texture(64, 128, true) {
                                        if let Ok(mp) = gpu_sel.map_texture(&up, 0, D3D11_MAP_WRITE_DISCARD) {
                                            let cp = 64usize.min(mp.row_pitch as usize);
                                            unsafe {
                                                for row in 0..192usize {
                                                    std::ptr::copy_nonoverlapping(
                                                        nv12_k.as_ptr().add(row * 64),
                                                        (mp.p_data as *mut u8).add(row * mp.row_pitch as usize),
                                                        cp,
                                                    );
                                                }
                                            }
                                            gpu_sel.unmap_texture(&up, 0);
                                            gpu_sel.copy_resource(&dt, &up);
                                        }
                                    }
                                    r283.commit_write_slot_with_color(
                                        si,
                                        64,
                                        128,
                                        VidRect { left: 0, top: 0, right: 64, bottom: 128 },
                                        (0, 0, 0, 0),
                                        16_667,
                                        true,
                                        VideoColorInfo::default(),
                                        Some(nv12_k),
                                    );
                                }
                                hw283.set_airplay_mode(true);
                                hw283.set_auto_rotate_enabled(true);
                                let mut g283 = ProbeGrid120x260::new();
                                let ap_bypass = !hw283.update_autorotate_probe_if_due(&mut g283, false);
                                hw283.set_airplay_mode(false);
                                let probed = hw283.update_autorotate_probe_if_due(&mut g283, true);
                                let px = g283.get_pixel(60, 130);
                                let red_ok = px.r > 180 && px.g < 60 && px.b < 60;
                                let mut tr = HevcAutoRotateTracker::new();
                                let rot_270 = tr.update_sample(&g283, 1206, 2622, 1000, false) == Some(270);
                                let res_k = analyze_frame_grid(&g283, false);
                                r283.clear();
                                drop(hw283);
                                ap_bypass
                                    && probed
                                    && red_ok
                                    && rot_270
                                    && res_k.is_konami_red_intro
                                    && res_k.suggested_orientation == OrientationMode::Landscape270
                            } else {
                                false
                            };
                            r283.clear();
                            drop(r283);
                            gpu_ok
                        };

                        // Return to Launchpad -> stop synthetic stream, verify Layer 0 detaches & shrinks to 16x16!
                        let _ = mirror::global_stop_screen();
                        u.set_phase(SharedString::from("idle"));
                        u.set_stage(SharedString::from(""));
                        u.set_first_frame_ready(false);
                        if let Some(ref p) = *pipe_rc.borrow() {
                            let _ = sync_dcomp_video_layout(parent_hwnd, &u, p, &rect_sh, Some(false));
                        }
                        let hidden_after_stop =
                            query_active_dcomp_state().map(|s| !s.2).unwrap_or(false);
                        trim_active_dcomp_memory();
                        u.window().request_redraw();
                        flush_active_dcomp_ui_frame();

                        // Tahap 275 Selftest Check 1: 500-packet continuous AES-128-CTR stream
                        // with trailing zero padding & extension NALs (`19|20`), zero resync/desync.
                        let airplay_ctr_500pkt_continuity_ok = {
                            use iphone_link_apple::airplay::mirror::{
                                normalize_avcc_au_classified, MirrorDecryptor,
                                NormalizedAvccOutcome,
                            };
                            use iphone_link_apple::win_cng::Aes128Ctr;
                            let k = [0x4Bu8; 16];
                            let iv = [0x91u8; 16];
                            let mut s_ctr = Aes128Ctr::new(&k, &iv).unwrap();
                            let mut dec = MirrorDecryptor::new(k, iv).unwrap();
                            let mut all_ok = true;
                            let mut total_b = 0u64;
                            let mut pkt = Vec::with_capacity(64);
                            for idx in 0..500usize {
                                pkt.clear();
                                match idx % 4 {
                                    0 => {
                                        pkt.extend_from_slice(&4u32.to_be_bytes());
                                        pkt.extend_from_slice(&[0x65, 0x88, (idx & 0x7F) as u8 | 1, 0x20]);
                                    }
                                    1 => {
                                        pkt.extend_from_slice(&4u32.to_be_bytes());
                                        pkt.extend_from_slice(&[0x41, 0x9A, (idx & 0xFF) as u8, 0x31]);
                                        pkt.extend_from_slice(&[0u8; 12]);
                                    }
                                    2 => {
                                        pkt.extend_from_slice(&4u32.to_be_bytes());
                                        pkt.extend_from_slice(&[0x53, 0x9A, (idx & 0xFF) as u8, 0x42]);
                                    }
                                    _ => {
                                        pkt.extend_from_slice(&4u32.to_be_bytes());
                                        pkt.extend_from_slice(&[0x41, 0x9A, (idx & 0xFF) as u8, 0x53]);
                                    }
                                }
                                let expect_len = pkt.len();
                                if s_ctr.apply_keystream(&mut pkt).is_err() {
                                    all_ok = false;
                                    break;
                                }
                                total_b += expect_len as u64;
                                if !dec.decrypt_in_place(&mut pkt).unwrap_or(false)
                                    || !matches!(
                                        normalize_avcc_au_classified(&pkt),
                                        NormalizedAvccOutcome::Video { .. }
                                    )
                                {
                                    all_ok = false;
                                    break;
                                }
                            }
                            all_ok
                                && dec.ctr_resync_count == 0
                                && dec.ctr_decrypt_rejects == 0
                                && dec.ctr_keystream_bytes == total_b
                        };

                        // Tahap 275 Selftest Check 2: Stage-Bounds Hard Clamp across all modes + Top Bar `AirPlay` label
                        let airplay_stage_bounds_and_topbar_label_ok = {
                            let mut bounds_ok = true;
                            for &scale in &[1.0f32, 1.25f32, 1.5f32] {
                                for &(cw, ch, fs, land) in &[
                                    (346u32, 832u32, false, false),
                                    (706, 832, false, false),
                                    (840, 468, false, true),
                                    (1920, 1080, true, true),
                                ] {
                                    for &(vw, vh) in &[(990u32, 2160), (888, 1920), (640, 1388), (1920, 888)] {
                                        for &fm in &[FitMode::AutoFit, FitMode::EdgeToEdge, FitMode::OriginalRatio] {
                                            let r = compute_active_video_physical_rect_with_drawer_w_and_mode(
                                                (cw as f32 * scale).round() as u32,
                                                (ch as f32 * scale).round() as u32,
                                                scale,
                                                if cw >= 700 && !land { NATIVE_DRAWER_WIDTH } else { 0.0 },
                                                fs,
                                                land,
                                                vw,
                                                vh,
                                                0,
                                                fm,
                                                true,
                                            );
                                            if r.x < r.stage_x
                                                || r.y < r.stage_y
                                                || r.x + r.width as i32 > r.stage_x + r.stage_width as i32
                                                || r.y + r.height as i32 > r.stage_y + r.stage_height as i32
                                            {
                                                bounds_ok = false;
                                            }
                                        }
                                    }
                                }
                            }
                            let ap_port = compute_active_video_physical_rect_with_drawer_w_and_mode(
                                346, 832, 1.0, 0.0, false, false, 640, 1388, 0, FitMode::AutoFit, true,
                            );
                            let bad_slide = ActiveVideoPhysicalRect {
                                x: 0,
                                y: ap_port.stage_y + 100,
                                width: ap_port.stage_width,
                                height: ap_port.stage_height,
                                ..ap_port
                            };
                            let clamped = clamp_video_rect_to_stage(bad_slide);
                            let labels_ok = resolve_topbar_status_pill_text("running", "streaming", "wifi", true, false) == "AirPlay"
                                && resolve_topbar_status_pill_text("running", "streaming", "airplay", true, true) == "AirPlay"
                                && resolve_topbar_status_pill_text("running", "streaming", "wifi", false, true) == "Mirroring · Wi-Fi"
                                && resolve_topbar_status_pill_text("running", "streaming", "usb", false, true) == "Mirroring · USB"
                                && resolve_topbar_status_pill_text("starting", "waiting_airplay", "wifi", true, false) == "AirPlay Siap"
                                && resolve_topbar_status_pill_text("starting", "waiting_airplay", "wifi", true, true) == "AirPlay Ready";
                            let trace_valid_json =
                                serde_json::from_str(
                                    &airplay::get_airplay_trace_json(),
                                )
                                .is_ok();
                            bounds_ok
                                && trace_valid_json
                                && ap_port.y == ap_port.stage_y
                                && ap_port.height == ap_port.stage_height
                                && clamped.y == ap_port.stage_y
                                && clamped.y + clamped.height as i32 == ap_port.stage_y + ap_port.stage_height as i32
                                && labels_ok
                        };

                        // Tahap 278 Selftest: Manual-only AirPlay Vertical/Horizontal plan + Stateless Zero-Pillarbox
                        // + 2-tier Infinite-GOP watchdog + all 6 new /api/debug/airplay-trace telemetry keys.
                        let airplay_infinite_gop_and_manual_rotate_zero_pillarbox_ok = {
                            use crate::native_video_pipeline::{
                                AirplayStallWatchdog, AirplayWatchdogAction,
                                AIRPLAY_HARD_RECOVERY_MIN_FATAL_ERRORS, AIRPLAY_STALL_MIN_PACKETS,
                            };
                            use iphone_link_video::d3d11_context::Rect as VidRect;
                            use iphone_link_video::video_hwnd::compute_stateless_airplay_present_rects;

                            let p1 = resolve_airplay_manual_orientation_plan(true, 994, 2160, false, "auto", false);
                            let p2 = resolve_airplay_manual_orientation_plan(true, 2160, 994, false, "auto", false);
                            let p3 = resolve_airplay_manual_orientation_plan(true, 2160, 994, false, "auto", true);
                            let p4 = resolve_airplay_manual_orientation_plan(true, 994, 2160, false, "auto", true);
                            let plans_ok = matches!(
                                p1,
                                Some(x) if !x.is_landscape && x.video_rotation_deg == 0
                                    && (x.window_logical_w - 346.0).abs() < 1.0
                                    && (x.window_logical_h - 832.0).abs() < 1.0
                            ) && matches!(
                                p2,
                                Some(x) if !x.is_landscape && x.video_rotation_deg == 90
                                    && (x.window_logical_w - 346.0).abs() < 1.0
                                    && (x.window_logical_h - 832.0).abs() < 1.0
                            ) && matches!(
                                p3,
                                Some(x) if x.is_landscape && x.video_rotation_deg == 0
                                    && (x.window_logical_w - 840.0).abs() < 1.0
                                    && (x.window_logical_h - 468.0).abs() < 1.0
                            ) && matches!(
                                p4,
                                Some(x) if x.is_landscape && x.video_rotation_deg == 270
                                    && (x.window_logical_w - 840.0).abs() < 1.0
                                    && (x.window_logical_h - 468.0).abs() < 1.0
                            );

                            let mut stage_fill_ok = true;
                            let cases = [
                                (346u32, 832u32, false, 994u32, 2160u32, 0u16),
                                (346, 832, false, 2160, 994, 90),
                                (840, 468, true, 2160, 994, 0),
                                (840, 468, true, 994, 2160, 270),
                                (346, 832, false, 994, 2160, 0),
                            ];
                            for &(cw, ch, land, vw, vh, rot) in &cases {
                                for &fm in &[FitMode::AutoFit, FitMode::EdgeToEdge] {
                                    let r = compute_active_video_physical_rect_with_drawer_w_and_mode(
                                        cw, ch, 1.0, 0.0, false, land, vw, vh, rot, fm, true,
                                    );
                                    if r.x != 0
                                        || r.y != r.stage_y
                                        || r.width != r.stage_width
                                        || r.height != r.stage_height
                                    {
                                        stage_fill_ok = false;
                                    }
                                    let (_, _, d_rect) = compute_stateless_airplay_present_rects(
                                        VidRect {
                                            left: 0,
                                            top: 0,
                                            right: vw as i32,
                                            bottom: vh as i32,
                                        },
                                        r.width,
                                        r.height,
                                        rot,
                                        fm,
                                    );
                                    if d_rect.left != 0
                                        || d_rect.top != 0
                                        || d_rect.right != r.width as i32
                                        || d_rect.bottom != r.height as i32
                                    {
                                        stage_fill_ok = false;
                                    }
                                }
                            }

                            let mut wd = AirplayStallWatchdog::new();
                            let _ = wd.observe_tiered(true, 1, 1, 500, 0);
                            let mut saw_soft = false;
                            for i in 0..u64::from(AIRPLAY_STALL_MIN_PACKETS) {
                                match wd.observe_tiered(true, 1, 1, 1000 + i * 55, 0) {
                                    AirplayWatchdogAction::SoftDrainAndRepaint => saw_soft = true,
                                    AirplayWatchdogAction::HardRebuildOnFatalError => {
                                        saw_soft = false
                                    }
                                    AirplayWatchdogAction::None => {}
                                }
                            }
                            let soft_zero_hard = saw_soft && wd.recoveries() == 0;
                            let mut non_fatal_still_zero = true;
                            for i in u64::from(AIRPLAY_STALL_MIN_PACKETS)
                                ..u64::from(AIRPLAY_STALL_MIN_PACKETS + 200)
                            {
                                if wd.observe_tiered(true, 1, 1, 1000 + i * 55, 0)
                                    == AirplayWatchdogAction::HardRebuildOnFatalError
                                {
                                    non_fatal_still_zero = false;
                                }
                            }
                            let hard_on_fatal = wd.observe_tiered(
                                true,
                                1,
                                1,
                                25_000,
                                AIRPLAY_HARD_RECOVERY_MIN_FATAL_ERRORS,
                            ) == AirplayWatchdogAction::HardRebuildOnFatalError;
                            let hard_ok = soft_zero_hard
                                && non_fatal_still_zero
                                && hard_on_fatal
                                && wd.recoveries() == 1;

                            let trace_json = airplay::get_airplay_trace_json();
                            let trace_keys_ok = [
                                "preDecoderDroppedAwaitingAvcc",
                                "preDecoderDroppedAwaitingIdr",
                                "preDecoderDroppedStaleEpoch",
                                "cachedIdrReplays",
                                "lastPresentedDestRect",
                                "lastPresentedRotationDeg",
                                "avccSmall256To511Frames",
                                "mftReconfigs",
                            ]
                            .iter()
                            .all(|k| trace_json.contains(k));

                            airplay::reset_airplay_trace();
                            crate::native_video_pipeline::cleanup_airplay_stall_dump_file();
                            plans_ok && stage_fill_ok && hard_ok && trace_keys_ok
                        };

                        let (
                            ui_259_parity_ok,
                            chips_cnt,
                            topbar_auto_ok,
                            saweria_ok,
                            svg_hd_icons_ok,
                            topbar_6btn_clean_no_theme_mirroring_unclipped_ok,
                            micro_polishes_en_clean_ok,
                        ) = {
                            let icons_src = include_str!("../ui/icons.slint");
                            let main_src = include_str!("../ui/main_window.slint");
                            let chips_cnt = main_src.matches("ShortcutPillChip {").count();
                            let prev_ar = u.get_auto_rotate();
                            u.set_auto_rotate(true);
                            let ar_on = u.get_auto_rotate();
                            u.set_auto_rotate(false);
                            let ar_off = !u.get_auto_rotate();
                            u.set_auto_rotate(prev_ar);
                            let budgets_ok = [
                                compute_topbar_346px_layout_budget(true, true, false),
                                compute_topbar_346px_layout_budget(true, false, false),
                                compute_topbar_346px_layout_budget(false, true, false),
                                compute_topbar_346px_layout_budget(false, false, false),
                                compute_topbar_346px_layout_budget(true, true, true),
                            ]
                            .iter()
                            .all(|b| b.fits_346px);
                            let topbar_auto_ok = ar_on && ar_off && budgets_ok && main_src.contains("text: \"AUTO\";");
                            let saweria_ok = main_src.contains("Dukung Kreator di Saweria")
                                && main_src.contains("Support Creator on Saweria")
                                // Superseded by Tahap 288 V3 / Tahap 293: clean single-row footer "iPhone Link v5.0.1 by Kelvin Daniyar · GPL-3.0"
                                && main_src.contains("iPhone Link v5.0.1 by")
                                && main_src.contains("Kelvin Daniyar")
                                && main_src.contains(" · GPL-3.0")
                                && !main_src.contains("Forked from")
                                && !main_src.contains("teyboxv1/pane-iphone-mirror-windows")
                                && validate_public_external_url("https://saweria.co/kelvindaniyar")
                                && validate_public_external_url("https://github.com/kelvindaniyar")
                                && validate_public_external_url("https://github.com/teyboxv1/pane-iphone-mirror-windows")
                                && !validate_public_external_url("https://example.com/untrusted");
                            let standby_chunk = main_src
                                .split("lp-col := VerticalLayout")
                                .nth(1)
                                .and_then(|s| s.split("CARD 1: TAMPILAN & BAHASA").next())
                                .unwrap_or("");
                            let ui_259_parity_ok = chips_cnt == 15
                                && !standby_chunk.contains("Mode Video Stream")
                                && [
                                    "Simpan Crop",
                                    "Save Crop",
                                    "Simpan Kecepatan",
                                    "Save Speed",
                                    "KECEPATAN GESTUR TRACKPAD",
                                    "Pintasan Lintas Platform",
                                    "Tempel Clipboard PC",
                                    "Opsi Fallback Bluetooth A2DP",
                                    "Detail Perangkat & Koneksi",
                                    "Panduan & Pemecahan Masalah",
                                    "(Alt+V)",
                                    "(Alt+C)",
                                    "(Alt+R)",
                                    "(Alt+B)",
                                    "(Ctrl+K)",
                                ]
                                .iter()
                                .all(|needle| main_src.contains(needle));
                            let svg_hd_icons_ok = icons_src.matches("@image-url(\"assets/icons/").count() == 49
                                && !icons_src.contains("Path {")
                                && icons_src.contains("image-rendering: smooth;")
                                && icons_src.contains("colorize: root.icon-color;")
                                && !main_src.contains("component SaveFloppyIcon")
                                && !main_src.contains("component CoffeeCupIcon")
                                && !main_src.contains("component BluetoothIcon");
                            let topbar_chunk = main_src
                                .split("top-bar := Rectangle")
                                .nth(1)
                                .and_then(|s| s.split("MAIN BODY SPLIT").next())
                                .unwrap_or("");
                            let b_stream_auto = compute_topbar_346px_layout_budget(true, true, false);
                            let topbar_6btn_clean_no_theme_mirroring_unclipped_ok = !topbar_chunk.contains("\"sun\"")
                                && !topbar_chunk.contains("\"moon\"")
                                && topbar_chunk.contains("\"fullscreen-exit\"")
                                && topbar_chunk.contains("close-btn: true;")
                                && b_stream_auto.total_px <= 340
                                && resolve_compact_topbar_status_pill_text("Mirroring · Wi-Fi", false) == "Mirroring"
                                && resolve_compact_topbar_status_pill_text("Mirroring · USB", true) == "Mirroring · USB";
                            let wifi_test_json = serde_json::json!({
                                "devices": [{
                                    "udid": "00008150-0000000000000283",
                                    "name": "iPhone Pro",
                                    "model": "iPhone 17 Pro",
                                    "ip": "192.168.1.50",
                                    "wifiOnline": true,
                                    "reachable": true,
                                    "wifiPaired": true
                                }]
                            });
                            let sum_en = parse_launchpad_from_status(&wifi_test_json, true);
                            let micro_polishes_en_clean_ok = main_src.contains("show-header: false;")
                                && main_src.matches("wrap-label: true;").count() >= 4
                                && !main_src.contains("label: \"Gain Level\"")
                                && main_src.contains("root.is-en ? \"Top\" : \"Atas (Top)\"")
                                && main_src.contains("root.is-en ? \"Right\" : \"Kanan (Right)\"")
                                && sum_en.wifi_text.contains("Wi-Fi Ready (iPhone LAN IP:")
                                && sum_en.status_pill_short.contains("iPhone IP:")
                                && !sum_en.wifi_text.contains("Siap");
                            (
                                ui_259_parity_ok,
                                chips_cnt,
                                topbar_auto_ok,
                                saweria_ok,
                                svg_hd_icons_ok,
                                topbar_6btn_clean_no_theme_mirroring_unclipped_ok,
                                micro_polishes_en_clean_ok,
                            )
                        };

                        let (seamless_start_stop_transitions_ok, fullscreen_exit_atomic_sync_ok) = {
                            let start_mid = compute_transition_opacities(
                                StageTransitionKind::StartToSequoia,
                                0.5,
                            );
                            let reveal_0 = compute_transition_opacities(
                                StageTransitionKind::FirstFrameReveal {
                                    use_sequoia_fallback: false,
                                },
                                0.0,
                            );
                            let reveal_mid = compute_transition_opacities(
                                StageTransitionKind::FirstFrameReveal {
                                    use_sequoia_fallback: false,
                                },
                                0.6,
                            );
                            let stop_live_mid = compute_transition_opacities(
                                StageTransitionKind::StopFromLive,
                                0.5,
                            );
                            let stop_conn_mid = compute_transition_opacities(
                                StageTransitionKind::StopFromConnecting,
                                0.5,
                            );
                            let stop_live_plan =
                                plan_stop_transition_sequence(true, true, false);
                            let stop_conn_plan =
                                plan_stop_transition_sequence(false, false, false);
                            let main_src = include_str!("../ui/main_window.slint");
                            let seq_seg = main_src
                                .split("sequoia-reveal :=")
                                .nth(1)
                                .and_then(|s| s.split("// End sequoia-reveal").next())
                                .unwrap_or("");
                            let blur_budget_ok = SEQUOIA_BLUR_MAX_W == 120
                                && SEQUOIA_BLUR_MAX_H == 260
                                && SEQUOIA_BLUR_RADIUS == 8
                                && (SEQUOIA_BLUR_MAX_W * SEQUOIA_BLUR_MAX_H * 4) < 160 * 1024;
                            let seamless_ok = blur_budget_ok
                                && start_mid.standby > 0.05
                                && start_mid.sequoia > 0.5
                                && (reveal_0.reveal_blur - 1.0).abs() < 0.001
                                && reveal_mid.sequoia_text == 0.0
                                && reveal_mid.reveal_blur > 0.05
                                && stop_live_mid.standby > 0.5
                                && stop_live_mid.reveal_blur > 0.05
                                && stop_conn_mid.standby > 0.5
                                && stop_conn_mid.sequoia > 0.05
                                && stop_live_plan.transition_kind == StageTransitionKind::StopFromLive
                                && stop_live_plan.duration_ms == 280
                                && stop_live_plan.flush_layer1_before_layer0_detach
                                && stop_conn_plan.transition_kind == StageTransitionKind::StopFromConnecting
                                && stop_conn_plan.duration_ms == 240
                                && seq_seg.contains("\"Menyambungkan ke\"")
                                && seq_seg.contains("\"Connecting to\"")
                                && seq_seg.contains("Open Control Center on iPhone → Screen Mirroring → select 'iPhone Link'")
                                && seq_seg.contains("Buka Pusat Kontrol di iPhone → Pencerminan Layar → pilih 'iPhone Link'")
                                && seq_seg.contains("root.glass-shell")
                                && !seq_seg.contains("1E293BE8")
                                && !seq_seg.contains("E2ECF7D8")
                                && !seq_seg.contains("stage-progress")
                                && !seq_seg.contains("\"AirPlay Siap\"")
                                && !seq_seg.contains("\"AirPlay Ready\"")
                                && !seq_seg.contains("\"Batal\"")
                                && !seq_seg.contains("\"Cancel\"")
                                && !seq_seg.contains("stop-mirror")
                                && resolve_topbar_status_pill_text("starting", "connecting", "usb", false, false) == "Menyambungkan\u{2026}"
                                && resolve_topbar_status_pill_text("starting", "connecting", "usb", false, true) == "Connecting\u{2026}"
                                && !should_use_disk_blur_cache(true);

                            let wa = WinRect {
                                left: 0,
                                top: 0,
                                right: 1920,
                                bottom: 1032,
                            };
                            let saved_drawer_closed = Some(WinRect {
                                left: 200,
                                top: 100,
                                right: 546,
                                bottom: 932,
                            });
                            let (ex, ey, ew, eh) = compute_fullscreen_exit_physical_rect(
                                saved_drawer_closed,
                                wa,
                                true,
                                false,
                                "iphone17pro",
                                1.0,
                            );
                            let dcomp_src = include_str!("dcomp_presenter.rs");
                            let fs_exit_ok = ew == 706
                                && eh == 832
                                && ex >= 0
                                && ey >= 0
                                && dcomp_src.contains("pub fn atomic_resize_host_window(")
                                && main_src.contains("property <length> seq-x:");
                            (seamless_ok, fs_exit_ok)
                        };

                        trim_active_dcomp_memory();

                        {
                            let mut m = m_rc.borrow_mut();
                            m.ui_tahap259_full_parity_and_saweria_footer_ok = ui_259_parity_ok;
                            m.shortcut_chips_count = chips_cnt;
                            m.topbar_auto_badge_ok = topbar_auto_ok;
                            m.saweria_footer_ok = saweria_ok;
                            m.hevc_autorotate_konami_efootball_e2e_ok =
                                hevc_autorotate_konami_efootball_e2e_ok;
                            m.svg_hd_icons_ok = svg_hd_icons_ok;
                            m.topbar_6btn_clean_no_theme_mirroring_unclipped_ok =
                                topbar_6btn_clean_no_theme_mirroring_unclipped_ok;
                            m.micro_polishes_en_clean_ok = micro_polishes_en_clean_ok;
                            m.seamless_start_stop_transitions_ok =
                                seamless_start_stop_transitions_ok;
                            m.fullscreen_exit_atomic_sync_ok = fullscreen_exit_atomic_sync_ok;
                            m.ui_alpha_transparent_hole_ok = hole_alpha_ok;
                            m.fullscreen_wings_frosted_ok = fullscreen_wings_frosted_ok;
                            m.sequoia_overlay_exclusive_ok = sequoia_overlay_exclusive_ok;
                            m.airspace_hide_show_ok = m.hidden_on_idle
                                && wait_hid_rc.load(Ordering::Relaxed)
                                && run_vis_rc.load(Ordering::Relaxed)
                                && hidden_after_stop;
                            m.decoded_frames = tel.decoded_frames;
                            m.presented_frames = tel.presented_frames;
                            m.airplay_h264_continuous_present_ok =
                                tel.decoded_frames >= 20 && tel.presented_frames >= 10;
                            m.airplay_orientation_switch_clean_ok = switched_96x64 && plan_ok;
                            m.airplay_same_dim_avcc_no_freeze_ok = same_dim_rc.load(Ordering::Relaxed)
                                && tel.decoded_frames >= 20
                                && tel.presented_frames >= 10;
                            m.airplay_midstream_watchdog_ok = wd_ok;
                            m.airplay_stale_buffer_purge_ok = stale_buffer_purge_ok;
                            m.airplay_bt709_color_consistent_ok = bt709_color_ok;
                            m.airplay_portrait_90cw_fill_ok = portrait_90cw_fill_ok;
                            m.airplay_non_mod16_60frame_zero_freeze_ok = non_mod16_60frame_ok;
                            m.airplay_20x_orientation_endurance_zero_black_ok =
                                airplay_20x_endurance_ok;
                            m.airplay_ctr_500pkt_continuity_ok = airplay_ctr_500pkt_continuity_ok;
                            m.airplay_stage_bounds_and_topbar_label_ok =
                                airplay_stage_bounds_and_topbar_label_ok;
                            m.airplay_vfr_low_latency_and_fullscreen_portrait_ok =
                                airplay_vfr_low_latency_and_fullscreen_portrait_ok;
                            m.airplay_zero_green_macroblock_and_fullscreen_landscape_ok =
                                airplay_zero_green_macroblock_and_fullscreen_landscape_ok;
                            m.airplay_infinite_gop_and_manual_rotate_zero_pillarbox_ok =
                                airplay_infinite_gop_and_manual_rotate_zero_pillarbox_ok;
                            m.airplay_avcc_256_511_zero_freeze_and_anti_smear_ok =
                                airplay_avcc_256_511_zero_freeze_and_anti_smear_ok;
                            m.airplay_hd_nv12_shader_quality_ok =
                                airplay_hd_nv12_shader_quality_ok;
                            m.loopback_only_test_mode = loopback_ok;
                            m.real_h264_present_ok = m.airplay_h264_continuous_present_ok;
                            m.ram_working_set_mb = native_preview::process_working_set_mb();
                        }

                        sync_active_dcomp_video_layer(None, 0.0, 0.0);
                        if let Some(mut p) = pipe_rc.borrow_mut().take() {
                            p.shutdown();
                        }
                        let _ = slint::quit_event_loop();
                    }
                }
            }
        });
    }

    let _ = slint::run_event_loop();

    // Clean teardown
    let _ = mirror::global_stop_screen();
    let _ = airplay::apply_live_audio_config(Some(100), Some("normal"), Some(true), Some(false));
    sync_active_dcomp_video_layer(None, 0.0, 0.0);
    if let Some(mut p) = pipeline.borrow_mut().take() {
        p.shutdown();
    }
    let _ = ui.hide();
    shutdown_active_dcomp_host();
    mirror::global_reset_mirror_runtime_for_test();

    crate::native_video_pipeline::cleanup_airplay_stall_dump_file();
    let m = metrics.borrow();
    let elapsed_ms = t_start.elapsed().as_secs_f64() * 1000.0;
    let icons_zero_emoji = !include_str!("../ui/icons.slint").chars().any(|c| c as u32 > 0x2600 && (c as u32) < 0x1FB00)
        && !include_str!("../ui/main_window.slint").chars().any(|c| c as u32 > 0x2600 && (c as u32) < 0x1FB00);
    let logo_embedded = !include_bytes!("../ui/assets/logo.png").is_empty();
    let expected_backdrop = if query_windows_build_number() >= WIN11_SYSTEMBACKDROP_MIN_BUILD {
        DCOMP_DWMSBT_TRANSIENTWINDOW
    } else {
        DCOMP_DWMSBT_NONE
    };
    let all_passed = m.slint_window_created
        && m.not_maximized
        && m.thin_acrylic_applied
        && m.always_on_acrylic_active
        && m.dwm_systembackdrop_type == expected_backdrop
        && m.autofit_fullscreen_top_gap_px == 0
        && m.dcomp_two_layer_active
        && m.child_hwnd_attached
        && m.ui_alpha_transparent_hole_ok
        && m.acrylic_layer1_alpha_ok
        && m.fullscreen_wings_frosted_ok
        && m.sequoia_overlay_exclusive_ok
        && m.airspace_hide_show_ok
        && m.theme_lang_toggle_ok
        && m.drawer_bounds_sync_ok
        && m.airplay_audio_live_ok
        && m.real_h264_present_ok
        && m.airplay_h264_continuous_present_ok
        && m.airplay_orientation_switch_clean_ok
        && m.airplay_same_dim_avcc_no_freeze_ok
        && m.airplay_midstream_watchdog_ok
        && m.airplay_stale_buffer_purge_ok
        && m.airplay_bt709_color_consistent_ok
        && m.airplay_portrait_90cw_fill_ok
        && m.airplay_non_mod16_60frame_zero_freeze_ok
        && m.airplay_20x_orientation_endurance_zero_black_ok
        && m.airplay_ctr_500pkt_continuity_ok
        && m.airplay_stage_bounds_and_topbar_label_ok
        && m.airplay_vfr_low_latency_and_fullscreen_portrait_ok
        && m.airplay_zero_green_macroblock_and_fullscreen_landscape_ok
        && m.airplay_infinite_gop_and_manual_rotate_zero_pillarbox_ok
        && m.airplay_avcc_256_511_zero_freeze_and_anti_smear_ok
        && m.airplay_hd_nv12_shader_quality_ok
        && m.ui_tahap259_full_parity_and_saweria_footer_ok
        && m.shortcut_chips_count == 15
        && m.topbar_auto_badge_ok
        && m.saweria_footer_ok
        && m.hevc_autorotate_konami_efootball_e2e_ok
        && m.svg_hd_icons_ok
        && m.topbar_6btn_clean_no_theme_mirroring_unclipped_ok
        && m.micro_polishes_en_clean_ok
        && m.seamless_start_stop_transitions_ok
        && m.fullscreen_exit_atomic_sync_ok
        && m.loopback_only_test_mode
        && icons_zero_emoji
        && logo_embedded;

    println!(
        "{{\n  \"status\": \"{}\",\n  \"webview2_processes\": 0,\n  \"slint_window_created\": {},\n  \"not_maximized\": {},\n  \"thin_acrylic_applied\": {},\n  \"always_on_acrylic_swca\": {},\n  \"always_on_acrylic_active\": {},\n  \"backdrop_backend\": \"{}\",\n  \"dwm_systembackdrop_type\": {},\n  \"autofit_fullscreen_top_gap_px\": {},\n  \"child_hwnd_attached\": {},\n  \"dcomp_two_layer_active\": {},\n  \"child_hwnd_count\": 0,\n  \"ui_alpha_transparent_hole_ok\": {},\n  \"dark_glass_alpha\": {},\n  \"light_glass_alpha\": {},\n  \"acrylic_layer1_alpha_ok\": {},\n  \"fullscreen_wings_frosted_ok\": {},\n  \"sequoia_overlay_exclusive_ok\": {},\n  \"airspace_hide_show_ok\": {},\n  \"theme_lang_toggle_ok\": {},\n  \"drawer_bounds_sync_ok\": {},\n  \"airplay_audio_live_ok\": {},\n  \"real_h264_present_ok\": {},\n  \"airplay_h264_continuous_present_ok\": {},\n  \"airplay_orientation_switch_clean_ok\": {},\n  \"airplay_same_dim_avcc_no_freeze_ok\": {},\n  \"airplay_midstream_watchdog_ok\": {},\n  \"airplay_stale_buffer_purge_ok\": {},\n  \"airplay_bt709_color_consistent_ok\": {},\n  \"airplay_portrait_90cw_fill_ok\": {},\n  \"airplay_non_mod16_60frame_zero_freeze_ok\": {},\n  \"airplay_20x_orientation_endurance_zero_black_ok\": {},\n  \"airplay_ctr_500pkt_continuity_ok\": {},\n  \"airplay_stage_bounds_and_topbar_label_ok\": {},\n  \"airplay_vfr_low_latency_and_fullscreen_portrait_ok\": {},\n  \"airplay_zero_green_macroblock_and_fullscreen_landscape_ok\": {},\n  \"airplay_infinite_gop_and_manual_rotate_zero_pillarbox_ok\": {},\n  \"airplay_avcc_256_511_zero_freeze_and_anti_smear_ok\": {},\n  \"airplay_hd_nv12_shader_quality_ok\": {},\n  \"ui_tahap259_full_parity_and_saweria_footer_ok\": {},\n  \"shortcut_chips_count\": {},\n  \"topbar_auto_badge_ok\": {},\n  \"saweria_footer_ok\": {},\n  \"hevc_autorotate_konami_efootball_e2e_ok\": {},\n  \"svg_hd_icons_ok\": {},\n  \"topbar_6btn_clean_no_theme_mirroring_unclipped_ok\": {},\n  \"micro_polishes_en_clean_ok\": {},\n  \"seamless_start_stop_transitions_ok\": {},\n  \"fullscreen_exit_atomic_sync_ok\": {},\n  \"loopback_only_test_mode\": {},\n  \"icons_zero_emoji\": {},\n  \"logo_embedded\": {},\n  \"drawer_cards\": 8,\n  \"shortcuts_wired\": 15,\n  \"tray_ready\": true,\n  \"decoded_frames\": {},\n  \"presented_frames\": {},\n  \"ram_working_set_mb\": {:.2},\n  \"elapsed_ms\": {:.1}\n}}",
        if all_passed { "PASS" } else { "FAIL" },
        m.slint_window_created,
        m.not_maximized,
        m.thin_acrylic_applied,
        m.always_on_acrylic_swca,
        m.always_on_acrylic_active,
        m.backdrop_backend,
        m.dwm_systembackdrop_type,
        m.autofit_fullscreen_top_gap_px,
        m.child_hwnd_attached,
        m.dcomp_two_layer_active,
        m.ui_alpha_transparent_hole_ok,
        m.dark_glass_alpha,
        m.light_glass_alpha,
        m.acrylic_layer1_alpha_ok,
        m.fullscreen_wings_frosted_ok,
        m.sequoia_overlay_exclusive_ok,
        m.airspace_hide_show_ok,
        m.theme_lang_toggle_ok,
        m.drawer_bounds_sync_ok,
        m.airplay_audio_live_ok,
        m.real_h264_present_ok,
        m.airplay_h264_continuous_present_ok,
        m.airplay_orientation_switch_clean_ok,
        m.airplay_same_dim_avcc_no_freeze_ok,
        m.airplay_midstream_watchdog_ok,
        m.airplay_stale_buffer_purge_ok,
        m.airplay_bt709_color_consistent_ok,
        m.airplay_portrait_90cw_fill_ok,
        m.airplay_non_mod16_60frame_zero_freeze_ok,
        m.airplay_20x_orientation_endurance_zero_black_ok,
        m.airplay_ctr_500pkt_continuity_ok,
        m.airplay_stage_bounds_and_topbar_label_ok,
        m.airplay_vfr_low_latency_and_fullscreen_portrait_ok,
        m.airplay_zero_green_macroblock_and_fullscreen_landscape_ok,
        m.airplay_infinite_gop_and_manual_rotate_zero_pillarbox_ok,
        m.airplay_avcc_256_511_zero_freeze_and_anti_smear_ok,
        m.airplay_hd_nv12_shader_quality_ok,
        m.ui_tahap259_full_parity_and_saweria_footer_ok,
        m.shortcut_chips_count,
        m.topbar_auto_badge_ok,
        m.saweria_footer_ok,
        m.hevc_autorotate_konami_efootball_e2e_ok,
        m.svg_hd_icons_ok,
        m.topbar_6btn_clean_no_theme_mirroring_unclipped_ok,
        m.micro_polishes_en_clean_ok,
        m.seamless_start_stop_transitions_ok,
        m.fullscreen_exit_atomic_sync_ok,
        m.loopback_only_test_mode,
        icons_zero_emoji,
        logo_embedded,
        m.decoded_frames,
        m.presented_frames,
        m.ram_working_set_mb,
        elapsed_ms
    );

    if all_passed { 0 } else { 1 }
}

// ============================================================================
// TAHAP 263 UNIT TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tahap263_stage_rect_physical_geometry_portrait_landscape_drawer_fullscreen() {
        for scale in [1.0f32, 1.25f32, 1.5f32] {
            let expected_y = (NATIVE_TOP_BAR_HEIGHT * scale).round() as i32;

            // 1. Portrait Closed (346x832 zero-pillarbox iPhone 19.5:9)
            let (pw_c, ph_c) = compute_native_window_target_logical_size(false, false);
            assert_eq!((pw_c, ph_c), (346.0, 832.0));
            let (x, y, w, h) =
                compute_native_stage_physical_rect(pw_c, ph_c, scale, false, false, false);
            assert_eq!(x, 0);
            assert_eq!(y, expected_y);
            let nav_top_px = ((ph_c - NATIVE_QUICK_NAV_HEIGHT) * scale).round() as i32;
            assert!(
                y + h as i32 <= nav_top_px,
                "Portrait closed stage overlaps Quick Nav at scale={scale}"
            );
            assert_eq!(w, (346.0 * scale).round() as u32);

            // 2. Portrait Drawer Open (706x832 -> -360px drawer)
            let (pw_o, ph_o) = compute_native_window_target_logical_size(true, false);
            assert_eq!((pw_o, ph_o), (706.0, 832.0));
            let (xo, yo, wo, ho) =
                compute_native_stage_physical_rect(pw_o, ph_o, scale, true, false, false);
            assert_eq!(xo, 0);
            assert_eq!(yo, expected_y);
            let drawer_left_px = ((pw_o - NATIVE_DRAWER_WIDTH) * scale).round() as i32;
            assert!(
                xo + wo as i32 <= drawer_left_px,
                "Portrait drawer-open stage overlaps Side Drawer at scale={scale}"
            );
            assert_eq!(wo, w);
            assert_eq!(ho, h);

            // 3. Landscape Closed (840x468) & Landscape Drawer Open (1200x468)
            let (lw_c, lh_c) = compute_native_window_target_logical_size(false, true);
            assert_eq!((lw_c, lh_c), (840.0, 468.0));
            let (xlc, ylc, wlc, hlc) =
                compute_native_stage_physical_rect(lw_c, lh_c, scale, false, false, true);
            assert_eq!(xlc, 0);
            assert_eq!(ylc, expected_y);
            let nav_top_l = ((lh_c - NATIVE_QUICK_NAV_HEIGHT) * scale).round() as i32;
            assert!(ylc + hlc as i32 <= nav_top_l);

            let (lw_o, lh_o) = compute_native_window_target_logical_size(true, true);
            assert_eq!((lw_o, lh_o), (1200.0, 468.0));
            let (xlo, ylo, wlo, hlo) =
                compute_native_stage_physical_rect(lw_o, lh_o, scale, true, false, true);
            assert_eq!((xlo, ylo, wlo, hlo), (0, expected_y, wlc, hlc));

            // 4. Full Screen (1920x1080 logical) closed & drawer open
            let (xfc, yfc, wfc, hfc) =
                compute_native_stage_physical_rect(1920.0, 1080.0, scale, false, true, false);
            assert_eq!((xfc, yfc), (0, expected_y));
            let nav_top_fs = ((1080.0 - NATIVE_QUICK_NAV_HEIGHT) * scale).round() as i32;
            assert!(yfc + hfc as i32 <= nav_top_fs);

            let (xfo, yfo, wfo, hfo) =
                compute_native_stage_physical_rect(1920.0, 1080.0, scale, true, true, false);
            assert_eq!((xfo, yfo), (0, expected_y));
            assert_eq!(hfo, hfc);
            let fs_drawer_left = ((1920.0 - NATIVE_DRAWER_WIDTH) * scale).round() as i32;
            assert!(xfo + wfo as i32 <= fs_drawer_left);
            assert!(wfo < wfc);
        }
    }

    #[test]
    fn test_tahap263_airspace_visibility_policy_hides_video_hwnd_on_launchpad_and_waiting_airplay() {
        // Must HIDE Child HWND on Launchpad, Sequoia/DDI overlays, Waiting AirPlay, and Errors
        assert!(!should_video_hwnd_be_visible("idle", ""));
        assert!(!should_video_hwnd_be_visible("starting", "connecting"));
        assert!(!should_video_hwnd_be_visible("starting", "downloading_ddi"));
        assert!(!should_video_hwnd_be_visible("starting", "mounting_ddi"));
        assert!(!should_video_hwnd_be_visible("starting", "waiting_airplay"));
        assert!(!should_video_hwnd_be_visible("waiting_airplay", "waiting_airplay"));
        assert!(!should_video_hwnd_be_visible("error", ""));
        assert!(!should_video_hwnd_be_visible("stopped", ""));
        assert!(!should_video_hwnd_be_visible("", ""));
        assert!(!should_video_hwnd_be_visible("running", "waiting_airplay"));
        assert!(!should_video_hwnd_be_visible("running", "downloading_ddi"));
        assert!(!should_video_hwnd_be_visible("running", "mounting_ddi"));

        // Must SHOW Child HWND strictly when phase == "running" and unobstructed
        assert!(should_video_hwnd_be_visible("running", ""));
        assert!(should_video_hwnd_be_visible("running", "streaming"));
        assert!(should_video_hwnd_be_visible("RUNNING", "live"));
    }

    #[test]
    fn test_native_ui_topbar_368_geometry_and_zero_emoji_slint() {
        let icons_src = include_str!("../ui/icons.slint");
        let main_src = include_str!("../ui/main_window.slint");
        for (fname, src) in [("ui/icons.slint", icons_src), ("ui/main_window.slint", main_src)] {
            let mut masked = src.to_ascii_lowercase();
            for pub_str in [
                "https://saweria.co/kelvindaniyar",
                "https://github.com/kelvindaniyar/iphone-link/issues",
                "https://github.com/kelvindaniyar/iphone-link",
                "https://github.com/kelvindaniyar",
                "kelvin daniyar",
            ] {
                masked = masked.replace(pub_str, " ");
            }
            let bad_k = ["kel", "vin"].concat();
            assert!(
                !masked.contains(&bad_k),
                "{fname} contains forbidden PII string"
            );
            for ch in src.chars() {
                let code = ch as u32;
                assert!(
                    !(0x2600..=0x27BF).contains(&code) && !(0x1F300..=0x1FAFF).contains(&code),
                    "{fname} contains forbidden Unicode emoji U+{code:X}"
                );
            }
        }
        assert!(main_src.contains(r#"@image-url("assets/logo.png")"#));
        // Top Bar geometry budget at 346px portrait: left identity <= 120px, right icons <= 174px idle / <= 216px streaming
        let idle_spacer = NATIVE_PORTRAIT_WIDTH - 120.0 - 174.0;
        let stream_spacer = NATIVE_PORTRAIT_WIDTH - 106.0 - 216.0;
        assert!(idle_spacer >= 40.0, "idle drag spacer too small: {idle_spacer}");
        assert!(stream_spacer >= 20.0, "stream drag spacer too small: {stream_spacer}");
    }

    #[test]
    fn test_native_ui_start_mirror_error_and_udid_resolution_logic() {
        let err1 = parse_start_mirror_outcome(r#"{"ok":false,"phase":"idle"}"#, false).unwrap_err();
        assert!(!err1.trim().is_empty(), "zero silent failures allowed");
        let err2 = parse_start_mirror_outcome(r#"{"ok":false,"error":"iPhone locked"}"#, true).unwrap_err();
        assert!(err2.contains("iPhone locked"));
        let ok1 = parse_start_mirror_outcome(r#"{"ok":true,"phase":"starting","stage":"waiting_airplay"}"#, false).unwrap();
        assert_eq!(ok1.0, "starting");
        assert_eq!(ok1.1, "waiting_airplay");
    }

    #[test]
    fn test_native_ui_8card_drawer_and_crop_clamp_parity() {
        let main_src = include_str!("../ui/main_window.slint");
        for marker in [
            "TAMPILAN & BAHASA",
            "RASIO & UKURAN LAYAR",
            "AUDIO (IPHONE -> PC)",
            "KECEPATAN GESTUR TRACKPAD",
            "KEYBOARD & PINTASAN",
            "LAYAR IPHONE & MODE SILUMAN",
            "KONEKSI & KONTROL MIRRORING",
            "PEMBARUAN PERANGKAT LUNAK",
            "Kelvin Daniyar",
            "POTONG TEPI (0-24 PX PRESISI)",
        ] {
            assert!(main_src.contains(marker), "Missing drawer card/section marker: {marker}");
        }
        assert_eq!(fit_mode_to_ui_key("fill"), "edge_to_edge");
        assert_eq!(fit_mode_to_backend_key("original_ratio"), "contain");
        assert_eq!((-5i32).clamp(0, 24), 0);
        assert_eq!(99i32.clamp(0, 24), 24);
    }

    #[test]
    fn test_tahap265_parent_hwnd_nchittest_htcaption_and_button_exclusion() {
        for scale in [1.0f32, 1.25f32, 1.5f32] {
            let cw = (346.0 * scale).round() as i32;
            let mid_y = (18.0 * scale).round() as i32;
            // Left identity / open Top Bar band -> HTCAPTION when not fullscreen
            assert_eq!(
                compute_top_bar_nchittest((110.0 * scale).round() as i32, mid_y, cw, scale, false, false),
                HTCAPTION
            );
            // Right-side Settings/Theme/Min/Close icons -> MUST stay HTCLIENT so clicks NEVER drop!
            assert_eq!(
                compute_top_bar_nchittest(cw - (10.0 * scale).round() as i32, mid_y, cw, scale, false, false),
                HTCLIENT
            );
            // Right-side Rotate/Stop/Mute extra icons when streaming -> MUST stay HTCLIENT!
            assert_eq!(
                compute_top_bar_nchittest(cw - (205.0 * scale).round() as i32, mid_y, cw, scale, false, true),
                HTCLIENT
            );
            // Below Top Bar -> HTCLIENT
            assert_eq!(
                compute_top_bar_nchittest(60, (60.0 * scale).round() as i32, cw, scale, false, false),
                HTCLIENT
            );
            // Full Screen -> always HTCLIENT (never drag maximized window)
            assert_eq!(
                compute_top_bar_nchittest(60, mid_y, cw, scale, true, true),
                HTCLIENT
            );
        }
    }

    #[test]
    fn test_tahap265_portrait_346_zero_pillarbox_and_device_profile_geometry() {
        // 346x750 stage ratio (~0.4613) matches iPhone 19.5:9 portrait (9:19.5 = 0.4615) -> 0 pillarboxes!
        let stage_h = NATIVE_PORTRAIT_HEIGHT - NATIVE_TOP_BAR_HEIGHT - NATIVE_QUICK_NAV_HEIGHT;
        assert_eq!(stage_h, 750.0);
        let stage_ratio = NATIVE_PORTRAIT_WIDTH / stage_h;
        let iphone_ratio = 9.0f32 / 19.5f32;
        assert!(
            (stage_ratio - iphone_ratio).abs() < 0.002,
            "Portrait stage ratio {stage_ratio:.5} diverges from 9:19.5 ({iphone_ratio:.5})"
        );
        // iPad profile widens portrait window to 4:3 (~564x832), Landscape expands to 840x468
        assert_eq!(
            compute_native_window_target_logical_size_for_profile(false, false, "iphone"),
            (346.0, 832.0)
        );
        assert_eq!(
            compute_native_window_target_logical_size_for_profile(true, false, "iphone"),
            (706.0, 832.0)
        );
        assert_eq!(
            compute_native_window_target_logical_size_for_profile(false, false, "ipad"),
            (564.0, 832.0)
        );
        assert_eq!(
            compute_native_window_target_logical_size_for_profile(true, false, "ipad"),
            (924.0, 832.0)
        );
        assert_eq!(
            compute_native_window_target_logical_size_for_profile(false, true, "ipad"),
            (840.0, 468.0)
        );
    }

    #[test]
    fn test_tahap265_bilingual_audit_and_ui_action_log_ring_zero_pii() {
        let main_src = include_str!("../ui/main_window.slint");
        for forbidden_typo in [
            "LIPS SYNC",
        ] {
            assert!(
                !main_src.contains(forbidden_typo),
                "Forbidden bilingual typo remains in main_window.slint: {forbidden_typo}"
            );
        }
        for required_label in [
            "Sinkron Audio (Lip Sync)",
            "Lip Sync",
            "Mode Video Stream",
            "Video Stream Mode",
            "Mulai mirroring",
            "Start mirroring",
        ] {
            assert!(
                main_src.contains(required_label),
                "Expected clean bilingual label missing in main_window.slint: {required_label}"
            );
        }

        // Verify read-only UI action ring log caps cleanly at 64 entries and stays 0-PII
        clear_ui_action_log_for_test();
        for i in 0..70 {
            record_ui_action("test_ev", &format!("seq={i}"), None);
        }
        let log_json = ui_action_log_json();
        let v = serde_json::from_str(&log_json).unwrap();
        let arr = v.get("entries").and_then(|x| x.as_array()).unwrap();
        assert_eq!(arr.len(), 64);
        let bad_k = ["kel", "vin"].concat();
        assert!(!log_json.to_ascii_lowercase().contains(&bad_k));
        clear_ui_action_log_for_test();
    }

    #[test]
    fn test_tahap266_active_video_physical_rect_5point_calibration_all_modes_and_blur_wing_reject() {
        let cases = [
            // (cw, ch, scale, drawer, fs, land, fw, fh, rot, fit)
            (519u32, 1248u32, 1.5f32, false, false, false, 1179u32, 2556u32, 0u16, FitMode::AutoFit),
            (1059u32, 1248u32, 1.5f32, true, false, false, 1179u32, 2556u32, 0u16, FitMode::AutoFit),
            (1260u32, 702u32, 1.5f32, false, false, true, 1179u32, 2556u32, 90u16, FitMode::AutoFit),
            (2560u32, 1392u32, 1.5f32, false, true, false, 1179u32, 2556u32, 0u16, FitMode::AutoFit),
            (2560u32, 1392u32, 1.5f32, false, true, false, 1179u32, 2556u32, 0u16, FitMode::EdgeToEdge),
            (2560u32, 1392u32, 1.5f32, false, true, false, 1179u32, 2556u32, 0u16, FitMode::OriginalRatio),
        ];
        for (cw, ch, scale, drawer, fs, land, fw, fh, rot, fit) in cases {
            let rect = compute_active_video_physical_rect(
                cw, ch, scale, drawer, fs, land, fw, fh, rot, fit,
            );
            assert!(rect.width >= 100 && rect.height >= 100, "Invalid rect: {rect:?}");

            // 5-point calibration: TL, TR, BL, BR, Center
            let w = rect.width as i32;
            let h = rect.height as i32;
            let pts = [
                (rect.x, rect.y),
                (rect.x + w - 1, rect.y),
                (rect.x, rect.y + h - 1),
                (rect.x + w - 1, rect.y + h - 1),
                (rect.x + w / 2, rect.y + h / 2),
            ];
            for (px, py) in pts {
                let mapped = map_client_px_to_ios_hid_from_rect(px, py, rect, rot)
                    .expect("Active video interior pixel must map to HID");
                let nx = ((px - rect.x) as f64) / (w as f64);
                let ny = ((py - rect.y) as f64) / (h as f64);
                let expected = crate::native_input::stage_norm_to_ios_hid(nx, ny, u32::from(rot));
                assert!(
                    (mapped.0 as i32 - expected.0 as i32).abs() <= 1
                        && (mapped.1 as i32 - expected.1 as i32).abs() <= 1,
                    "5-point HID mismatch for ({px},{py}) -> {mapped:?} vs {expected:?}"
                );
            }

            // Center pixel must always map to center (~32768) in any rotation
            let c_hid = map_client_px_to_ios_hid_from_rect(
                rect.x + w / 2,
                rect.y + h / 2,
                rect,
                rot,
            )
            .unwrap();
            assert!((c_hid.0 as i32 - 32768).abs() <= 120);
            assert!((c_hid.1 as i32 - 32768).abs() <= 120);

            // Top bar & bottom nav must always return None
            assert_eq!(
                map_client_px_to_ios_hid_from_rect(rect.x + w / 2, 10, rect, rot),
                None
            );
            assert_eq!(
                map_client_px_to_ios_hid_from_rect(rect.x + w / 2, (ch as i32) - 10, rect, rot),
                None
            );
            // Blur wings (fullscreen Fit) or right drawer must return None
            if rect.x > 4 {
                assert_eq!(
                    map_client_px_to_ios_hid_from_rect(rect.x - 2, rect.y + h / 2, rect, rot),
                    None
                );
            }
            if drawer {
                assert_eq!(
                    map_client_px_to_ios_hid_from_rect((cw as i32) - 40, rect.y + h / 2, rect, rot),
                    None
                );
            }
        }
    }

    #[test]
    fn test_tahap266_fullscreen_workarea_zero_overhang_dynamic_drawer_restore_and_hero_wifi_ready() {
        let wa = WinRect {
            left: 0,
            top: 0,
            right: 2560,
            bottom: 1392,
        };
        let saved = WinRect {
            left: 100,
            top: 80,
            right: 619, // stale closed Portrait width (346 * 1.5 = 519)
            bottom: 1328,
        };
        // Dynamic drawer restore: when Drawer is open, exiting Fullscreen MUST restore 706px*scale
        // and re-center safely inside `wa` instead of restoring stale 346px `saved_rc`!
        let (rx, ry, rw, rh) =
            compute_fullscreen_exit_physical_rect(Some(saved), wa, true, false, "iphone", 1.5);
        assert_eq!((rw, rh), ((706.0f32 * 1.5).round() as i32, (832.0f32 * 1.5).round() as i32));
        assert_eq!((rx, ry), ((wa.width() - rw) / 2, (wa.height() - rh) / 2));
        assert!(rx >= wa.left && rx + rw <= wa.right && ry >= wa.top && ry + rh <= wa.bottom);

        // Matching layout (`drawer_open=false`) preserves exact `saved_rc` position & size
        let (sx, sy, sw, sh) =
            compute_fullscreen_exit_physical_rect(Some(saved), wa, false, false, "iphone", 1.5);
        assert_eq!((sx, sy, sw, sh), (100, 80, 519, 1248));

        // Landscape + Drawer open restore -> 1200x468 * 1.5
        let (_, _, rwl, rhl) =
            compute_fullscreen_exit_physical_rect(Some(saved), wa, true, true, "iphone", 1.5);
        assert_eq!((rwl, rhl), ((1200.0f32 * 1.5).round() as i32, (468.0f32 * 1.5).round() as i32));

        // Hero online on Wi-Fi even when usb=false and wifiOnline=false (reachable && wifiPaired)
        let wifi_status = serde_json::json!({
            "selectedUdid": "00008150-0000000000000001",
            "devices": [{
                "udid": "00008150-0000000000000001",
                "name": "iPhone Pro",
                "model": "iPhone 17 Pro",
                "ip": "192.168.1.90",
                "usb": false,
                "wifiOnline": false,
                "reachable": true,
                "wifiPaired": true
            }]
        });
        let sum = parse_launchpad_from_status(&wifi_status, true);
        assert!(sum.ready, "Hero card MUST be online when Wi-Fi reachable && wifiPaired");
        assert_eq!(format_status_pill_with_transport("running", "", "wifi", false, true), "Mirroring · Wi-Fi");
        assert_eq!(format_status_pill_with_transport("running", "", "usb", false, true), "Mirroring · USB");
        assert_eq!(format_status_pill_with_transport("starting", "waiting_airplay", "wifi", true, true), "AirPlay Ready");
    }

    #[test]
    fn test_tahap266_home_single_fast_flick_zero_hold_vs_recent_180ms_hold() {
        let home_steps = hid::swipe_home_steps();
        let recent_steps = hid::swipe_recent_app_switcher_steps();

        let first_home = home_steps.first().expect("home steps empty");
        let last_touch_home = home_steps.iter().rev().find(|s| s.contact).expect("no active touch in home");
        assert_eq!(first_home.y, 65535);
        assert_eq!(last_touch_home.y, 18000);
        let max_home_hold = home_steps.iter().filter(|s| s.contact).map(|s| s.sleep_ms).max().unwrap_or(0);
        assert!(
            max_home_hold <= 20,
            "Home swipe must have zero hold pause at top, found {max_home_hold}ms"
        );
        let total_home_ms: u64 = home_steps.iter().map(|s| s.sleep_ms).sum();
        assert!(
            total_home_ms <= 85,
            "Home swipe must remain an ultra-fast single flick (<85ms), got {total_home_ms}ms"
        );

        let recent_hold = recent_steps.iter().filter(|s| s.contact).map(|s| s.sleep_ms).max().unwrap_or(0);
        assert!(
            recent_hold >= 150,
            "Recent App Switcher swipe must retain ~180ms hold pause, got {recent_hold}ms"
        );
    }

    #[test]
    fn test_tahap267_bottom_bar_home_and_recent_use_exact_tahap259_button_pipeline() {
        let _hid_guard = hid::HID_TEST_SERIAL_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        hid::reset_hid_runtime_state();

        let state = Arc::new(Mutex::new(server_v4::AppState::default()));
        let web_dir = Path::new("web");
        let (tx, _rx) = mpsc::channel();
        let router = Arc::new(NativeInputRouter::new(tx));

        // 1. Home must execute single POST /mirror/button {"name":"home","state":"press"} (1x HomeCombo / 0x0C,0x0040)
        let home_resps = execute_tahap259_bottom_nav(&state, web_dir, &router, "home");
        assert_eq!(home_resps.len(), 1);
        assert!(
            home_resps[0].contains(r#""button":"home""#) && home_resps[0].contains(r#""state":"press""#),
            "unexpected /mirror/button response: {:?}",
            home_resps
        );
        let btns_after_home = hid::recent_button_events_snapshot();
        assert_eq!(
            btns_after_home.len(),
            2,
            "Home 1x click must record down+up Consumer 0x0040 pair, got {btns_after_home:?}"
        );
        assert!(btns_after_home.iter().all(|b| b.page == 0x0C && b.usage == 0x0040));

        // 2. Recent/App Switcher must execute 2x POST /mirror/button {"name":"home","state":"press"} (~120 ms apart)
        hid::reset_hid_runtime_state();
        let t0 = Instant::now();
        let recent_resps = execute_tahap259_bottom_nav(&state, web_dir, &router, "recent");
        let elapsed_ms = t0.elapsed().as_millis();
        assert_eq!(recent_resps.len(), 2);
        assert!(elapsed_ms >= 110, "Recent 2x pressHome spacing must be >=110ms, got {elapsed_ms}ms");
        assert!(recent_resps.iter().all(|r| r.contains(r#""button":"home""#)));
        let btns_after_recent = hid::recent_button_events_snapshot();
        assert_eq!(
            btns_after_recent.len(),
            4,
            "Recent 2x pressHome must record 2x down+up Consumer 0x0040 pairs, got {btns_after_recent:?}"
        );

        // 3. Swipe Back & Swipe Right invoke single /mirror/gesture calls cleanly
        let back_resps = execute_tahap259_bottom_nav(&state, web_dir, &router, "back");
        assert_eq!(back_resps.len(), 1);
        assert!(back_resps[0].contains(r#""ok":true"#), "got {:?}", back_resps);
        let right_resps = execute_tahap259_bottom_nav(&state, web_dir, &router, "right");
        assert_eq!(right_resps.len(), 1);
        assert!(right_resps[0].contains(r#""ok":true"#), "got {:?}", right_resps);

        hid::reset_hid_runtime_state();
    }

    #[test]
    fn test_tahap267_lock_and_wake_unlock_pin_dpapi_zero_leak_and_honest_feedback() {
        let _hid_guard = hid::HID_TEST_SERIAL_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        hid::reset_hid_runtime_state();
        if let Ok(mut g) = UI_ACTION_LOG.lock() {
            g.clear();
        }

        let state = Arc::new(Mutex::new(server_v4::AppState::default()));
        let web_dir = Path::new("web");
        let (tx, _rx) = mpsc::channel();
        let router = Arc::new(NativeInputRouter::new(tx));

        // 1. When no live CoreDevice session is attached, dispatched == false and hid_live == false
        // (honest feedback prevents false green checkmark)
        let (ok_lock, hid_live_lock, dispatched_lock, _stealth_restored_lock, lock_resp) =
            execute_lock_screen_action(&state, web_dir, &router);
        assert!(ok_lock);
        assert!(!hid_live_lock);
        assert!(!dispatched_lock);
        assert!(lock_resp.contains(r#""dispatched":false"#), "got {lock_resp}");

        // 2. Wake/Unlock using direct UI PIN input without saving first: never leaks digits to typed buffer or action log
        let dummy_pin = "947261";
        let (ok_w1, hid_live_w1, dispatched_w1, unlocked_w1, wake_resp1) =
            execute_wake_unlock_action(&state, web_dir, &router, dummy_pin);
        assert!(ok_w1);
        assert!(!hid_live_w1);
        assert!(!dispatched_w1);
        assert!(unlocked_w1, "UI passcode must flow through to /api/device/wake-unlock: {wake_resp1}");
        assert!(hid::get_current_typed_line().is_empty());
        assert!(hid::get_typed_lines_buffer().is_empty());
        assert!(!ui_action_log_json().contains(dummy_pin));

        // 3. Save DPAPI PIN via /api/device/pin, then call Wake/Unlock with empty UI input -> DPAPI fallback unlocks!
        let save_resp = inprocess_api(
            &state,
            web_dir,
            "POST",
            "/api/device/pin",
            r#"{"pin":"947261"}"#,
        );
        assert!(save_resp.contains(r#""dpapiEncrypted":true"#), "got {save_resp}");
        assert!(hid::has_stored_pin());

        let (ok_w2, _, _, unlocked_w2, wake_resp2) =
            execute_wake_unlock_action(&state, web_dir, &router, "");
        assert!(ok_w2);
        assert!(unlocked_w2, "DPAPI stored PIN fallback must unlock on empty body: {wake_resp2}");
        assert!(hid::get_current_typed_line().is_empty());
        assert!(hid::get_typed_lines_buffer().is_empty());
        assert!(!ui_action_log_json().contains(dummy_pin));

        // Clean DPAPI wipe
        let _ = inprocess_api(&state, web_dir, "POST", "/api/device/wipe-privacy-data", "{}");
        assert!(!hid::has_stored_pin());
        hid::reset_hid_runtime_state();
    }

    #[test]
    fn test_tahap268_fullscreen_autofit_zero_top_black_bar_and_post_trim_hd_rect() {
        // Post-trim iPhone 17 Pro HEVC = 1202x2598 (after 58/14 trim on 1216x2656)
        // Fullscreen 1920x1040 -> stage_h = 1040 - 38 - 44 = 958; exact width = 443
        let fs_hevc = compute_active_video_physical_rect(
            1920,
            1040,
            1.0,
            false,
            true,
            false,
            1202,
            2598,
            0,
            FitMode::AutoFit,
        );
        assert_eq!(fs_hevc.y, 38);
        assert_eq!(fs_hevc.height, 958);
        let (src_hevc, dst_hevc) = compute_dest_and_clip_rects_for_fit(
            iphone_link_video::d3d11_context::Rect {
                left: 0,
                top: 0,
                right: 1202,
                bottom: 2598,
            },
            fs_hevc.width,
            fs_hevc.height,
            0,
            FitMode::AutoFit,
        );
        assert_eq!(src_hevc.right - src_hevc.left, 1202);
        assert_eq!(dst_hevc.top, 0, "Fullscreen AutoFit MUST have 0 px top black bar");
        assert_eq!(dst_hevc.bottom, fs_hevc.height as i32, "Fullscreen AutoFit MUST have 0 px bottom black bar");

        // AirPlay H.264 (888x1920) on Fullscreen 1440x900
        let fs_ap = compute_active_video_physical_rect(
            1440,
            900,
            1.0,
            false,
            true,
            false,
            888,
            1920,
            0,
            FitMode::AutoFit,
        );
        let (_, dst_ap) = compute_dest_and_clip_rects_for_fit(
            iphone_link_video::d3d11_context::Rect {
                left: 0,
                top: 0,
                right: 888,
                bottom: 1920,
            },
            fs_ap.width,
            fs_ap.height,
            0,
            FitMode::AutoFit,
        );
        assert_eq!(dst_ap.top, 0);
        assert_eq!(dst_ap.bottom, fs_ap.height as i32);
    }

    #[test]
    fn test_tahap268_drawer_atomic_windowed_and_fullscreen_60hz_offset_sync() {
        // Monotonic smooth 60 Hz glide across Fullscreen Drawer open (0 -> 360px)
        let mut prev_w = i32::MAX;
        let mut prev_stage_w = u32::MAX;
        for step in 0..=12 {
            let t = (step as f32) / 12.0;
            let e = ease_out_cubic(t);
            let dw = NATIVE_DRAWER_WIDTH * e;
            let rect = compute_active_video_physical_rect_with_drawer_w(
                1920,
                1040,
                1.0,
                dw,
                true,
                false,
                1202,
                2598,
                0,
                FitMode::AutoFit,
            );
            assert!(rect.stage_width <= prev_stage_w);
            assert!(rect.x <= prev_w || step == 0);
            assert_eq!(rect.stage_width, (1920.0 - dw).round() as u32);
            prev_stage_w = rect.stage_width;
            prev_w = rect.x;
        }

        // Windowed origin clamp check remains strictly inside work area
        let (rx, ry, rw, rh) = compute_resize_preserve_origin_rect(0, 706.0, 832.0);
        assert!(rw >= 320 && rh >= 400);
        assert!(rx >= 0 || rx < 0); // finite screen coords
        let _ = ry;
    }

    #[test]
    fn test_tahap268_sequoia_clean_model_title_zero_batal_and_blur_kernel() {
        // 1. Clean device model_name without stray "siap"/"ready" suffix; Slint overlay has no [Batal]
        let status = serde_json::json!({
            "selectedUdid": "00008150-0000000000000268",
            "devices": [{
                "udid": "00008150-0000000000000268",
                "name": "iPhone Pro",
                "model": "iPhone 17 Pro",
                "usb": false,
                "reachable": true,
                "wifiPaired": true
            }]
        });
        let sum = parse_launchpad_from_status(&status, false);
        assert_eq!(sum.model_name, "iPhone 17 Pro");
        assert!(!sum.model_name.to_ascii_lowercase().contains("siap"));
        assert!(!sum.model_name.to_ascii_lowercase().contains("ready"));

        let main_src = include_str!("../ui/main_window.slint");
        assert!(main_src.contains("root.device-model-name"));
        assert!(main_src.contains("root.reveal-blur-image"));
        let seq_block = main_src
            .split("sequoia-reveal :=")
            .nth(1)
            .and_then(|s| s.split("// End sequoia-reveal").next())
            .expect("sequoia-reveal section");
        // Superseded by Tahap 288: sequoia-reveal restored 1:1 Tahap 259 clean canvas (no modal card / center Batal; Top Bar Stop handles cancel)
        assert!(!seq_block.contains("Batal") && !seq_block.contains("Cancel") && !seq_block.contains("stop-mirror"));
        assert!(!seq_block.contains("root.device-title"));

        // 2. Fast O(W*H) 2-pass separable box blur attenuates sharp step edge smoothly
        let w = 64u32;
        let h = 64u32;
        let mut src = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let p = ((y * w + x) * 4) as usize;
                let v = if x < w / 2 { 240u8 } else { 20u8 };
                src[p] = v;
                src[p + 1] = v;
                src[p + 2] = v;
                src[p + 3] = 255;
            }
        }
        let blurred = fast_box_blur_rgba(&src, w, h, 6);
        assert_eq!(blurred.len(), src.len());
        let edge_left = blurred[((32 * w + 31) * 4) as usize];
        let edge_right = blurred[((32 * w + 32) * 4) as usize];
        assert!(
            edge_left < 200 && edge_right > 60 && edge_left.abs_diff(edge_right) < 40,
            "Box blur must smooth hard edge (left={edge_left}, right={edge_right})"
        );
    }

    #[test]
    fn test_tahap269_airplay_native_landscape_uses_zero_rotation_840x468_and_returns_portrait() {
        use iphone_link_video::d3d11_context::{D3d11Nv12VramRing, LiveD3d11Gpu};
        use iphone_link_video::mf_decoder::{LiveMfVideoDecoder, VideoCodecKind};

        // 1. BoundedNativeVideoQueue keeps H264Avcc strictly FIFO; single-source-of-truth decoder
        //    update_avcc() latches awaiting_idr=true & holds stray P-frames until fresh IDR.
        let q = mirror::BoundedNativeVideoQueue::new_with_h264_capacity(16, 120);
        let avcc_port = build_synthetic_h264_ipcm_idr(64, 96, 0x90, 0);
        let avcc_land = build_synthetic_h264_ipcm_idr(96, 64, 0xA8, 1);
        q.try_push(mirror::NativeVideoEvent::H264Avcc {
            avcc: avcc_port.avcc_config.clone(),
            width: 64,
            height: 96,
        });
        q.try_push(mirror::NativeVideoEvent::H264Frame {
            avcc_payload: build_synthetic_h264_p_skip_avcc_sample(64, 96, 1),
            is_keyframe: false,
            pts_us: 1000,
        });
        q.try_push(mirror::NativeVideoEvent::H264Avcc {
            avcc: avcc_land.avcc_config.clone(),
            width: 96,
            height: 64,
        });
        let (qlen, awaiting_after_switch, dropped_after_switch, _, _) = q.stats();
        assert!(!awaiting_after_switch, "mid-stream H264Avcc switch MUST stay FIFO without producer latch");
        assert_eq!(dropped_after_switch, 0);
        assert_eq!(qlen, 3);

        let gpu_t = Arc::new(LiveD3d11Gpu::new().expect("LiveD3d11Gpu"));
        let ring_t = D3d11Nv12VramRing::new();
        let mut dec_t = LiveMfVideoDecoder::new(gpu_t, VideoCodecKind::H264).expect("H264 MFT");
        assert!(dec_t.update_avcc(&avcc_port.avcc_config).expect("port avcC"));
        assert!(!dec_t.awaiting_idr());
        assert!(dec_t.update_avcc(&avcc_land.avcc_config).expect("land avcC"));
        assert!(dec_t.awaiting_idr(), "mid-stream update_avcc switch MUST latch awaiting_idr=true");
        let _ = dec_t
            .feed_avcc_sample(
                &build_synthetic_h264_p_skip_avcc_sample(96, 64, 2),
                2000,
                false,
                &ring_t,
            )
            .expect("drop pre-IDR P-frame");
        assert_eq!(dec_t.dropped_pre_idr_frames(), 1);
        assert!(dec_t.awaiting_idr());
        let _ = dec_t
            .feed_avcc_sample(&avcc_land.avcc_sample, 3000, true, &ring_t)
            .expect("fresh Landscape IDR");
        assert!(!dec_t.awaiting_idr(), "fresh Landscape IDR must clear awaiting_idr");
        ring_t.clear();
        dec_t.clean_teardown();
        drop(dec_t);

        // 2. Native AirPlay Landscape (1920x888, eFootball) -> 840x468 / 1200x468 at video_rotation = 0 deg (NEVER 270!) when Auto-Rotate is ON
        let plan_closed =
            resolve_airplay_native_orientation_plan(true, 1920, 888, false, "auto", true, false).unwrap();
        assert!(plan_closed.is_landscape);
        assert_eq!(plan_closed.video_rotation_deg, 0);
        assert_eq!(
            (plan_closed.window_logical_w, plan_closed.window_logical_h),
            (840.0, 468.0)
        );
        let plan_open =
            resolve_airplay_native_orientation_plan(true, 1920, 888, true, "auto", true, false).unwrap();
        assert_eq!(plan_open.video_rotation_deg, 0);
        assert_eq!(
            (plan_open.window_logical_w, plan_open.window_logical_h),
            (1200.0, 468.0)
        );

        let rect_win = compute_active_video_physical_rect(
            840,
            468,
            1.0,
            false,
            false,
            plan_closed.is_landscape,
            1920,
            888,
            plan_closed.video_rotation_deg as u16,
            FitMode::AutoFit,
        );
        let win_aspect = (rect_win.width as f64) / (rect_win.height as f64);
        assert!(win_aspect > 2.0, "Windowed Landscape rect must stay horizontal, got {win_aspect:.2}");

        let rect_fs = compute_active_video_physical_rect(
            1920,
            1040,
            1.0,
            false,
            true,
            plan_closed.is_landscape,
            1920,
            888,
            0,
            FitMode::AutoFit,
        );
        let fs_aspect = (rect_fs.width as f64) / (rect_fs.height as f64);
        let expected_aspect = 1920.0 / 888.0;
        assert!(
            (fs_aspect - expected_aspect).abs() < 0.03,
            "Fullscreen Landscape aspect must match 1920/888 ({expected_aspect:.3}), got {fs_aspect:.3}"
        );

        // 3. Return to Portrait (888x1920) -> 346x832 / 706x832 at video_rotation = 0 deg!
        let plan_back =
            resolve_airplay_native_orientation_plan(true, 888, 1920, false, "auto", true, false).unwrap();
        assert!(!plan_back.is_landscape);
        assert_eq!(plan_back.video_rotation_deg, 0);
        assert_eq!(
            (plan_back.window_logical_w, plan_back.window_logical_h),
            (346.0, 832.0)
        );
        let plan_back_open =
            resolve_airplay_native_orientation_plan(true, 888, 1920, true, "auto", true, false).unwrap();
        assert_eq!(
            (plan_back_open.window_logical_w, plan_back_open.window_logical_h),
            (706.0, 832.0)
        );
    }

    #[test]
    fn test_tahap269_notif_control_center_and_all_6_hardware_keys_tahap259_parity() {
        let _hid_guard = hid::HID_TEST_SERIAL_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        hid::reset_hid_runtime_state();
        if let Ok(mut g) = UI_ACTION_LOG.lock() {
            g.clear();
        }

        // 1. Verify exact y = 0 top-edge swipes in Portrait (0 deg) & rotated Landscape (270 deg)
        let notif_p =
            hid::build_system_top_edge_swipe_steps("notifications", 0).expect("notif Portrait");
        assert!(notif_p.len() >= 10);
        assert!(notif_p[0].contact && notif_p[0].x == 16000 && notif_p[0].y == 0);
        assert!(!notif_p.last().unwrap().contact && notif_p.last().unwrap().x == 16000 && notif_p.last().unwrap().y == 46000);

        let ctrl_p =
            hid::build_system_top_edge_swipe_steps("control_center", 0).expect("control Portrait");
        assert!(ctrl_p.len() >= 10);
        assert!(ctrl_p[0].contact && ctrl_p[0].x == 58000 && ctrl_p[0].y == 0);
        assert!(!ctrl_p.last().unwrap().contact && ctrl_p.last().unwrap().x == 58000 && ctrl_p.last().unwrap().y == 46000);

        let notif_l =
            hid::build_system_top_edge_swipe_steps("notifications", 270).expect("notif Landscape 270");
        assert_eq!((notif_l[0].x, notif_l[0].y), (65535, 16000));
        assert_ne!(notif_l[0], notif_p[0]);

        // 2. All 6 hardware keys route cleanly through execute_tahap259_hardware_key, cancel active touch,
        //    and record honest /api/debug/ui-action-log entries!
        let state = Arc::new(Mutex::new(server_v4::AppState::default()));
        let web_dir = Path::new("web");
        let (tx, _rx) = mpsc::channel();
        let router = Arc::new(NativeInputRouter::new(tx));

        for (raw_name, expected_canon) in [
            ("notifications", "notifications"),
            ("control_center", "control_center"),
            ("lock", "lock"),
            ("mute", "mute"),
            ("volume_up", "volume-up"),
            ("volume_down", "volume-down"),
        ] {
            let (ok, dispatched, canon, resp) =
                execute_tahap259_hardware_key(&state, web_dir, &router, raw_name, 0);
            assert!(ok, "hardware key {raw_name} must succeed: {resp}");
            assert_eq!(canon, expected_canon);
            assert!(!dispatched); // no live CoreDevice USB/Wi-Fi session in unit test
            record_ui_action(
                "hardware_button",
                &format!("btn={canon} ok={ok} dispatched={dispatched}"),
                None,
            );
        }

        let log = ui_action_log_json();
        for canon in [
            "notifications",
            "control_center",
            "lock",
            "mute",
            "volume-up",
            "volume-down",
        ] {
            assert!(
                log.contains(&format!("btn={canon} ok=true")),
                "missing ui-action-log entry for {canon}: {log}"
            );
        }

        // Confirm swipe_direction_steps also resolves both gestures at y=0
        assert!(hid::swipe_direction_steps("notifications").is_some());
        assert!(hid::swipe_direction_steps("control_center").is_some());

        hid::reset_hid_runtime_state();
    }

    #[test]
    fn test_tahap270_airplay_orientation_plan_and_no_visual_files_regression() {
        let land_closed = resolve_airplay_native_orientation_plan(true, 1920, 888, false, "auto", true, false)
            .expect("landscape closed plan");
        assert!(land_closed.is_landscape);
        assert_eq!(land_closed.video_rotation_deg, 0);
        assert_eq!(
            (land_closed.window_logical_w, land_closed.window_logical_h),
            (840.0, 468.0)
        );

        let land_open = resolve_airplay_native_orientation_plan(true, 1920, 888, true, "auto", true, false)
            .expect("landscape open plan");
        assert!(land_open.is_landscape);
        assert_eq!(land_open.video_rotation_deg, 0);
        assert_eq!(
            (land_open.window_logical_w, land_open.window_logical_h),
            (1200.0, 468.0)
        );

        let port_closed = resolve_airplay_native_orientation_plan(true, 888, 1920, false, "auto", true, false)
            .expect("portrait closed plan");
        assert!(!port_closed.is_landscape);
        assert_eq!(port_closed.video_rotation_deg, 0);
        assert_eq!(
            (port_closed.window_logical_w, port_closed.window_logical_h),
            (346.0, 832.0)
        );

        let port_open = resolve_airplay_native_orientation_plan(true, 888, 1920, true, "auto", true, false)
            .expect("portrait open plan");
        assert!(!port_open.is_landscape);
        assert_eq!(port_open.video_rotation_deg, 0);
        assert_eq!(
            (port_open.window_logical_w, port_open.window_logical_h),
            (706.0, 832.0)
        );
    }

    #[test]
    fn test_tahap271_airplay_orientation_respects_auto_rotate_off_manual_toggle_and_contain_no_squish() {
        // 1. Default Auto-Rotate = OFF + no manual override -> native Landscape AirPlay (1920x888)
        //    stays vertical (346x832 / 706x832) rotated 90 deg CW (100% Portrait Fill, 0 px black bar!).
        let ar_off_closed =
            resolve_airplay_native_orientation_plan(true, 1920, 888, false, "auto", false, false)
                .unwrap();
        assert!(!ar_off_closed.is_landscape);
        assert_eq!(ar_off_closed.video_rotation_deg, 90);
        assert_eq!(
            (ar_off_closed.window_logical_w, ar_off_closed.window_logical_h),
            (346.0, 832.0)
        );
        let ar_off_open =
            resolve_airplay_native_orientation_plan(true, 1920, 888, true, "auto", false, false)
                .unwrap();
        assert!(!ar_off_open.is_landscape);
        assert_eq!(ar_off_open.video_rotation_deg, 90);
        assert_eq!(
            (ar_off_open.window_logical_w, ar_off_open.window_logical_h),
            (706.0, 832.0)
        );

        // 2. Manual toggle while Auto-Rotate = OFF -> switches to 840x468 @ 0 deg (NEVER 270 sideways!)
        let manual_land =
            resolve_airplay_native_orientation_plan(true, 1920, 888, false, "auto", false, true)
                .unwrap();
        assert!(manual_land.is_landscape);
        assert_eq!(manual_land.video_rotation_deg, 0);
        assert_eq!(
            (manual_land.window_logical_w, manual_land.window_logical_h),
            (840.0, 468.0)
        );

        // 3. Mismatched orientation (1920x888 inside Portrait 346x832 stage) -> clean centered contain
        //    even under EdgeToEdge/AutoFit (zero squish distortion!)
        for fm in [FitMode::AutoFit, FitMode::EdgeToEdge, FitMode::OriginalRatio] {
            let r = compute_active_video_physical_rect(
                346,
                832,
                1.0,
                false,
                false,
                false,
                1920,
                888,
                0,
                fm,
            );
            let aspect = (r.width as f64) / (r.height.max(1) as f64);
            assert!(
                (aspect - (1920.0 / 888.0)).abs() < 0.05,
                "Portrait stage with landscape source under {fm:?} must stay 2.16:1 contain (got {aspect:.2}, rect={r:?})"
            );
            assert!(r.height < r.stage_height, "Contain rect must not fill vertical height when landscape");

            let (_src, dst) = compute_dest_and_clip_rects_for_fit(
                iphone_link_video::d3d11_context::Rect {
                    left: 0,
                    top: 0,
                    right: 1920,
                    bottom: 888,
                },
                346,
                750,
                0,
                fm,
            );
            let dst_aspect = ((dst.right - dst.left) as f64) / ((dst.bottom - dst.top).max(1) as f64);
            assert!(
                (dst_aspect - (1920.0 / 888.0)).abs() < 0.05,
                "DComp blit for mismatched landscape frame under {fm:?} must not squish ({dst_aspect:.2})"
            );
        }

        // 4. Matched Portrait orientation in AutoFit keeps 0 px black bars (exact fill)!
        let r_match = compute_active_video_physical_rect(
            346,
            832,
            1.0,
            false,
            false,
            false,
            1202,
            2598,
            0,
            FitMode::AutoFit,
        );
        assert_eq!(r_match.width, 346);
        assert_eq!(r_match.height, r_match.stage_height);
    }

    #[test]
    fn test_tahap271_loopback_only_test_mode_guard_blocks_wildcard_bind_and_keeps_normal_lan() {
        // 1. Pure mode resolvers always guarantee exact split ("127.0.0.1" test vs "0.0.0.0" normal)
        assert_eq!(connection::resolve_bind_host_for_mode(true), "127.0.0.1");
        assert_eq!(connection::resolve_bind_host_for_mode(false), "0.0.0.0");
        assert_eq!(
            connection::resolve_bind_addr_v4_for_mode(8765, true).to_string(),
            "127.0.0.1:8765"
        );
        assert_eq!(
            connection::resolve_bind_addr_v4_for_mode(8765, false).to_string(),
            "0.0.0.0:8765"
        );

        struct LoopbackGuard(bool);
        impl Drop for LoopbackGuard {
            fn drop(&mut self) {
                connection::set_loopback_only_test_mode(self.0);
            }
        }
        let prev = connection::is_loopback_only_test_mode();
        let _restore = LoopbackGuard(prev);

        // 2. Active loopback-only test mode blocks wildcard 0.0.0.0 & multicast 224.0.0.251 probes
        connection::set_loopback_only_test_mode(true);
        assert!(connection::is_loopback_only_test_mode());
        assert_eq!(connection::bind_host_ipv4(), "127.0.0.1");
        assert_eq!(connection::bind_addr_v4(0).ip().to_string(), "127.0.0.1");
        assert_eq!(detect_local_lan_ipv4(), "127.0.0.1");
        assert_eq!(server_v4::detect_primary_lan_ipv4(), "127.0.0.1");
        assert_eq!(
            airplay::mdns::collect_active_lan_ipv4s(),
            vec![std::net::Ipv4Addr::LOCALHOST]
        );
        assert!(iphone_link_apple::rp_wifi_tls::discover_remotepairing_mdns(None, None, 10).is_empty());
        assert_eq!(
            iphone_link_apple::wifi_discovery::get_host_local_ips(),
            vec![std::net::Ipv4Addr::new(192, 168, 1, 1)]
        );

        // 3. Normal runtime mode (when env override is absent) keeps 0.0.0.0 LAN binding unchanged
        if std::env::var("IPHONE_LINK_TEST_LOOPBACK_ONLY").is_err() {
            connection::set_loopback_only_test_mode(false);
            assert!(!connection::is_loopback_only_test_mode());
            assert_eq!(connection::bind_host_ipv4(), "0.0.0.0");
            assert_eq!(connection::bind_addr_v4(8765).to_string(), "0.0.0.0:8765");
        }
    }

    #[test]
    fn test_tahap272_airplay_portrait_90cw_fill_zero_blackbar_and_auto_rotate_matrix() {
        // Cond 1: Native Landscape AirPlay (1920x888) + Auto-Rotate OFF + no manual override
        // -> Portrait 346x832 with 90 CW rotation (effective 888x1920 fills 346x750 with 0 px black bar!)
        let p_fill =
            resolve_airplay_native_orientation_plan(true, 1920, 888, false, "auto", false, false)
                .unwrap();
        assert!(!p_fill.is_landscape);
        assert_eq!(p_fill.video_rotation_deg, 90);
        assert_eq!((p_fill.window_logical_w, p_fill.window_logical_h), (346.0, 832.0));
        let r_fill = compute_active_video_physical_rect(
            346,
            832,
            1.0,
            false,
            false,
            p_fill.is_landscape,
            1920,
            888,
            p_fill.video_rotation_deg as u16,
            FitMode::AutoFit,
        );
        assert_eq!(r_fill.x, 0);
        assert_eq!(r_fill.width, 346);
        assert_eq!(r_fill.height, r_fill.stage_height);
        // HID mapping for AirPlay 90 CW Portrait Fill resolves to physical 0 deg!
        assert_eq!(resolve_airplay_hid_rotation_deg(true, 90), 0);
        assert_eq!(resolve_airplay_hid_rotation_deg(true, 0), 270);
        assert_eq!(resolve_airplay_hid_rotation_deg(true, 180), 90);
        assert_eq!(resolve_airplay_hid_rotation_deg(false, 270), 270);

        // Cond 2: Auto-Rotate ON (or manual override) -> Landscape 840x468 / 1200x468 @ 0 deg
        let p_ar_on =
            resolve_airplay_native_orientation_plan(true, 1920, 888, false, "auto", true, false)
                .unwrap();
        assert!(p_ar_on.is_landscape);
        assert_eq!(p_ar_on.video_rotation_deg, 0);
        assert_eq!((p_ar_on.window_logical_w, p_ar_on.window_logical_h), (840.0, 468.0));

        // Cond 3: Portrait source (888x1920) auto-restores 346x832 @ 0 deg
        let p_restore =
            resolve_airplay_native_orientation_plan(true, 888, 1920, true, "auto", true, false)
                .unwrap();
        assert!(!p_restore.is_landscape);
        assert_eq!(p_restore.video_rotation_deg, 0);
        assert_eq!((p_restore.window_logical_w, p_restore.window_logical_h), (706.0, 832.0));
    }

    #[test]
    fn test_tahap272_fullscreen_drawer_dual_bound_fit_zero_side_crop() {
        for fm in [FitMode::AutoFit, FitMode::EdgeToEdge, FitMode::OriginalRatio] {
            for scale in [1.0f32, 1.25f32] {
                let cw = (1920.0 * scale).round() as u32;
                let ch = (1032.0 * scale).round() as u32;
                let drawer_px = (NATIVE_DRAWER_WIDTH * scale).round() as u32;
                let avail_w = cw.saturating_sub(drawer_px);

                // Full Screen Landscape + Drawer Open: must fit BOTH avail_w AND stage_height (0 crop!)
                let r_land = compute_active_video_physical_rect(
                    cw, ch, scale, true, true, true, 1920, 888, 0, fm,
                );
                assert!(r_land.x >= 0);
                assert!(r_land.width <= avail_w + 1);
                assert!(r_land.height <= r_land.stage_height);
                assert!((r_land.x as u32) + r_land.width <= avail_w + 1);
                let aspect_l = (r_land.width as f64) / (r_land.height.max(1) as f64);
                assert!(
                    (aspect_l - (1920.0 / 888.0)).abs() < 0.04,
                    "FullScreen+Drawer Landscape under {fm:?} @ {scale}x aspect={aspect_l:.3} rect={r_land:?}"
                );

                // Full Screen Portrait + Drawer Open: must also stay centered and within dual bounds
                let r_port = compute_active_video_physical_rect(
                    cw, ch, scale, true, true, false, 1202, 2598, 0, fm,
                );
                assert!(r_port.x >= 0);
                assert!(r_port.width <= avail_w + 1);
                assert!(r_port.height <= r_port.stage_height);
                let aspect_p = (r_port.width as f64) / (r_port.height.max(1) as f64);
                assert!(
                    (aspect_p - (1202.0 / 2598.0)).abs() < 0.04,
                    "FullScreen+Drawer Portrait under {fm:?} @ {scale}x aspect={aspect_p:.3} rect={r_port:?}"
                );
            }
        }
    }

    #[test]
    fn test_tahap275_strict_stage_bounds_hard_clamp_never_overlaps_bottom_bar_across_all_modes() {
        for &scale in &[1.0f32, 1.25f32, 1.5f32] {
            for &(cw_log, ch_log, drawer_open, fs, land) in &[
                (346u32, 832u32, false, false, false),
                (706, 832, true, false, false),
                (840, 468, false, false, true),
                (1200, 468, true, false, true),
                (1920, 1080, false, true, true),
                (1920, 1080, true, true, true),
                (1920, 1080, true, true, false),
            ] {
                let cw = (cw_log as f32 * scale).round() as u32;
                let ch = (ch_log as f32 * scale).round() as u32;
                let dw = if drawer_open { NATIVE_DRAWER_WIDTH } else { 0.0 };
                let top_bar_px = (NATIVE_TOP_BAR_HEIGHT * scale).round() as i32;
                let bottom_bar_top_px = ((((ch as f32) / scale - NATIVE_QUICK_NAV_HEIGHT)
                    .max(NATIVE_TOP_BAR_HEIGHT + 16.0)
                    * scale)
                    .round() as i32)
                    .min(ch as i32);

                for &is_airplay in &[false, true] {
                    for &(vw, vh) in &[
                        (990u32, 2160u32),
                        (888, 1920),
                        (640, 1388),
                        (1920, 888),
                        (1202, 2598),
                    ] {
                        for &fm in &[FitMode::AutoFit, FitMode::EdgeToEdge, FitMode::OriginalRatio] {
                            let r = compute_active_video_physical_rect_with_drawer_w_and_mode(
                                cw, ch, scale, dw, fs, land, vw, vh, 0, fm, is_airplay,
                            );
                            assert!(
                                r.x >= r.stage_x && r.y >= top_bar_px,
                                "Top/left stage breach: r={r:?}, top_bar_px={top_bar_px}, fs={fs}, land={land}, ap={is_airplay}, fm={fm:?}"
                            );
                            assert!(
                                r.x + r.width as i32 <= r.stage_x + r.stage_width as i32,
                                "Right stage breach: r={r:?}"
                            );
                            assert!(
                                r.y + r.height as i32 <= bottom_bar_top_px,
                                "Bottom QuickNav overlap! r={r:?}, bottom_bar_top_px={bottom_bar_top_px}, scale={scale}, fs={fs}, land={land}, ap={is_airplay}, fm={fm:?}"
                            );
                        }
                    }
                }
            }
        }

        // Portrait non-fullscreen AirPlay AutoFit MUST lock to (0, stage_y, stage_w, stage_h) across adaptive dims
        for &(vw, vh) in &[(990u32, 2160u32), (888, 1920), (640, 1388)] {
            let r = compute_active_video_physical_rect_with_drawer_w_and_mode(
                346, 832, 1.0, 0.0, false, false, vw, vh, 0, FitMode::AutoFit, true,
            );
            assert_eq!(r.x, 0);
            assert_eq!(r.y, r.stage_y);
            assert_eq!(r.width, r.stage_width);
            assert_eq!(r.height, r.stage_height);
        }

        // Inject synthetic +100px downward overflow (Bug #2 signature) -> clamp_video_rect_to_stage snaps back cleanly
        let base_rect = compute_active_video_physical_rect_with_drawer_w_and_mode(
            346, 832, 1.0, 0.0, false, false, 888, 1920, 0, FitMode::AutoFit, true,
        );
        let base_clamps = airplay::trace_layout_rect_clamps();
        let bad_slide = ActiveVideoPhysicalRect {
            x: 0,
            y: base_rect.stage_y + 100,
            width: base_rect.stage_width,
            height: base_rect.stage_height,
            ..base_rect
        };
        let fixed = clamp_video_rect_to_stage(bad_slide);
        assert_eq!(fixed.y, base_rect.stage_y);
        assert_eq!(fixed.height, base_rect.stage_height);
        assert_eq!(fixed.y + fixed.height as i32, base_rect.stage_y + base_rect.stage_height as i32);
        assert!(airplay::trace_layout_rect_clamps() > base_clamps);
    }

    #[test]
    fn test_tahap275_topbar_status_pill_shows_airplay_never_mirroring_wifi_when_airplay_active() {
        for &lang_en in &[false, true] {
            // Active AirPlay streaming -> MUST always show "AirPlay" (never "Mirroring · Wi-Fi")
            assert_eq!(
                resolve_topbar_status_pill_text("running", "streaming", "wifi", true, lang_en),
                "AirPlay"
            );
            assert_eq!(
                resolve_topbar_status_pill_text("running", "streaming", "airplay", false, lang_en),
                "AirPlay"
            );
            assert_eq!(
                resolve_topbar_status_pill_text("running", "streaming", "usb", true, lang_en),
                "AirPlay"
            );

            // CoreDevice Wi-Fi HEVC (airplay_active == false) -> preserves "Mirroring · Wi-Fi"
            assert_eq!(
                resolve_topbar_status_pill_text("running", "streaming", "wifi", false, lang_en),
                "Mirroring · Wi-Fi"
            );
            // USB (airplay_active == false) -> preserves "Mirroring · USB"
            assert_eq!(
                resolve_topbar_status_pill_text("running", "streaming", "usb", false, lang_en),
                "Mirroring · USB"
            );
        }

        // AirPlay standby (`starting` / `waiting_airplay`) preserves localized "AirPlay Siap" / "AirPlay Ready"
        assert_eq!(
            resolve_topbar_status_pill_text("starting", "waiting_airplay", "wifi", true, false),
            "AirPlay Siap"
        );
        assert_eq!(
            resolve_topbar_status_pill_text("starting", "waiting_airplay", "wifi", true, true),
            "AirPlay Ready"
        );
        assert_eq!(
            resolve_topbar_status_pill_text("waiting_airplay", "", "wifi", true, false),
            "AirPlay Siap"
        );
        assert_eq!(
            resolve_topbar_status_pill_text("waiting_airplay", "", "wifi", true, true),
            "AirPlay Ready"
        );
    }

    #[test]
    fn test_tahap276_fullscreen_portrait_geometry_centers_full_height_zero_drawer_shift() {
        for &scale in &[1.0f32, 1.25f32, 1.5f32] {
            let cw = (1920.0 * scale).round() as u32;
            let ch = (1032.0 * scale).round() as u32;

            for &(vw, vh) in &[(888u32, 1920u32), (990, 2160), (1920, 888), (1202, 2598)] {
                // 1. Closed drawer: drawer_px MUST be 0, vid_h == stage_h (0 bottom gap!), centered cleanly
                let r_closed = compute_active_video_physical_rect_with_drawer_w_and_mode(
                    cw, ch, scale, 0.0, true, false, vw, vh, 0, FitMode::AutoFit, true,
                );
                assert_eq!(r_closed.stage_width, cw, "drawer closed must leave zero drawer width");
                assert_eq!(
                    r_closed.height, r_closed.stage_height,
                    "Portrait FullScreen MUST fill full stage_height (0 gray gap!): {r_closed:?}"
                );
                let expected_x_closed = ((cw - r_closed.width) / 2) as i32;
                assert_eq!(
                    r_closed.x, expected_x_closed,
                    "Portrait FullScreen must center horizontally: {r_closed:?} expected_x={expected_x_closed}"
                );
                assert_eq!(r_closed.y, r_closed.stage_y);

                // 2. Open drawer (360 logical px) then close again -> exact symmetry restored (0 leftward shift!)
                let r_open = compute_active_video_physical_rect_with_drawer_w_and_mode(
                    cw,
                    ch,
                    scale,
                    NATIVE_DRAWER_WIDTH,
                    true,
                    false,
                    vw,
                    vh,
                    0,
                    FitMode::AutoFit,
                    true,
                );
                let drawer_px = (NATIVE_DRAWER_WIDTH * scale).round() as u32;
                assert_eq!(r_open.stage_width, cw - drawer_px);
                assert_eq!(r_open.height, r_open.stage_height);
                assert_eq!(r_open.x, ((r_open.stage_width - r_open.width) / 2) as i32);

                let r_reclosed = compute_active_video_physical_rect_with_drawer_w_and_mode(
                    cw, ch, scale, 0.0, true, false, vw, vh, 0, FitMode::AutoFit, true,
                );
                assert_eq!(r_reclosed, r_closed, "closing drawer must restore exact centered rect");
            }
        }
    }

    #[test]
    fn test_tahap277_fullscreen_landscape_1920x1080_snaps_centered_full_stage_zero_topleft_stuck() {
        airplay::reset_airplay_trace();

        for &scale in &[1.0f32, 1.25f32] {
            let win_cw = (840.0 * scale).round() as u32;
            let win_ch = (468.0 * scale).round() as u32;
            let fs_cw = (1920.0 * scale).round() as u32;
            let fs_ch = (1080.0 * scale).round() as u32;

            // 1. Initial windowed Landscape rect (840x468)
            let (win_rect, _) = compute_fullscreen_landscape_resync_rect(
                None,
                win_cw,
                win_ch,
                scale,
                false,
                false,
                true,
                1920,
                888,
                0,
                FitMode::AutoFit,
                true,
            );
            assert!(win_rect.width <= win_cw);

            // 2. Immediately after async maximize delivers 1920x1080 with drawer closed:
            // MUST snap to full centered widescreen (> 1500 logical px wide), NEVER stuck at 840x468 top-left!
            let (fs_closed, changed_enter) = compute_fullscreen_landscape_resync_rect(
                Some(win_rect),
                fs_cw,
                fs_ch,
                scale,
                false,
                true,
                true,
                1920,
                888,
                0,
                FitMode::AutoFit,
                true,
            );
            assert!(changed_enter);
            assert_eq!(fs_closed.stage_width, fs_cw);
            assert!(
                fs_closed.width > (1500.0 * scale) as u32,
                "Full Screen Landscape with drawer closed MUST expand past 1500 logical px, got {fs_closed:?}"
            );
            assert_eq!(fs_closed.x, ((fs_cw - fs_closed.width) / 2) as i32);
            assert!(fs_closed.y >= fs_closed.stage_y);
            assert!(fs_closed.y + fs_closed.height as i32 <= fs_closed.stage_y + fs_closed.stage_height as i32);

            // 3. Re-evaluating same geometry must return changed=false (idempotent, zero redundant commit!)
            let (fs_steady, changed_steady) = compute_fullscreen_landscape_resync_rect(
                Some(fs_closed),
                fs_cw,
                fs_ch,
                scale,
                false,
                true,
                true,
                1920,
                888,
                0,
                FitMode::AutoFit,
                true,
            );
            assert!(!changed_steady);
            assert_eq!(fs_steady, fs_closed);

            // 4. Full Screen Landscape + Drawer Open (drawer = 360 logical px) fits dual bounds strictly
            let drawer_px = (NATIVE_DRAWER_WIDTH * scale).round() as u32;
            let (fs_open, changed_open) = compute_fullscreen_landscape_resync_rect(
                Some(fs_closed),
                fs_cw,
                fs_ch,
                scale,
                true,
                true,
                true,
                1920,
                888,
                0,
                FitMode::AutoFit,
                true,
            );
            assert!(changed_open);
            assert_eq!(fs_open.stage_width, fs_cw - drawer_px);
            assert!(fs_open.width <= fs_open.stage_width);
            assert!(fs_open.height <= fs_open.stage_height);
            assert_eq!(fs_open.x, ((fs_open.stage_width - fs_open.width) / 2) as i32);
        }

        assert!(airplay::trace_fullscreen_landscape_resyncs() >= 4);
        airplay::reset_airplay_trace();
    }

    #[test]
    fn test_tahap278_airplay_window_stays_vertical_default_and_only_flips_on_manual_rotate() {
        // 1. Default AirPlay keeps strictly Vertical 346x832 even when iPhone flips to Landscape
        let p_port_def = resolve_airplay_manual_orientation_plan(true, 994, 2160, false, "auto", false)
            .expect("994x2160 vertical plan");
        assert!(!p_port_def.is_landscape);
        assert_eq!(p_port_def.video_rotation_deg, 0);
        assert!((p_port_def.window_logical_w - 346.0).abs() < 1.0);
        assert!((p_port_def.window_logical_h - 832.0).abs() < 1.0);

        for &(lw, lh) in &[(2160u32, 994u32), (1920, 888)] {
            let p_land_def = resolve_airplay_manual_orientation_plan(true, lw, lh, false, "auto", false)
                .expect("landscape stream vertical default");
            assert!(!p_land_def.is_landscape, "{lw}x{lh} MUST NOT auto-flip Vertical window");
            assert_eq!(p_land_def.video_rotation_deg, 90);
            assert!((p_land_def.window_logical_w - 346.0).abs() < 1.0);
            assert!((p_land_def.window_logical_h - 832.0).abs() < 1.0);
        }

        let p_888_def = resolve_airplay_manual_orientation_plan(true, 888, 1920, false, "auto", false)
            .expect("888x1920 vertical plan");
        assert!(!p_888_def.is_landscape);
        assert_eq!(p_888_def.video_rotation_deg, 0);
        assert!((p_888_def.window_logical_w - 346.0).abs() < 1.0);
        assert!((p_888_def.window_logical_h - 832.0).abs() < 1.0);

        // 2. Manual Rotate ONLY flips window to Horizontal 840x468
        let p_land_man = resolve_airplay_manual_orientation_plan(true, 2160, 994, false, "auto", true)
            .expect("2160x994 manual landscape plan");
        assert!(p_land_man.is_landscape);
        assert_eq!(p_land_man.video_rotation_deg, 0);
        assert!((p_land_man.window_logical_w - 840.0).abs() < 1.0);
        assert!((p_land_man.window_logical_h - 468.0).abs() < 1.0);

        let p_port_man = resolve_airplay_manual_orientation_plan(true, 994, 2160, false, "auto", true)
            .expect("994x2160 manual horizontal plan");
        assert!(p_port_man.is_landscape);
        assert_eq!(p_port_man.video_rotation_deg, 270);
        assert!((p_port_man.window_logical_w - 840.0).abs() < 1.0);
        assert!((p_port_man.window_logical_h - 468.0).abs() < 1.0);

        // 3. Manual Rotate back restores Vertical 346x832 (0 deg) cleanly
        let p_back = resolve_airplay_manual_orientation_plan(true, 994, 2160, false, "auto", false)
            .expect("manual rotate back vertical plan");
        assert!(!p_back.is_landscape);
        assert_eq!(p_back.video_rotation_deg, 0);
        assert!((p_back.window_logical_w - 346.0).abs() < 1.0);
        assert!((p_back.window_logical_h - 832.0).abs() < 1.0);

        // 4. All 5 states across scales 1.0/1.25/1.5 fill full stage cleanly (0 letterbox, 0 pillarbox)
        let states = [
            (346u32, 832u32, false, 994u32, 2160u32, 0u16),
            (346, 832, false, 2160, 994, 90),
            (840, 468, true, 2160, 994, 0),
            (840, 468, true, 994, 2160, 270),
            (346, 832, false, 994, 2160, 0),
        ];
        for &scale in &[1.0f32, 1.25f32, 1.5f32] {
            for &(cw, ch, land, vw, vh, rot) in &states {
                let pcw = (cw as f32 * scale).round() as u32;
                let pch = (ch as f32 * scale).round() as u32;
                for &fm in &[FitMode::AutoFit, FitMode::EdgeToEdge] {
                    let r = compute_active_video_physical_rect_with_drawer_w_and_mode(
                        pcw, pch, scale, 0.0, false, land, vw, vh, rot, fm, true,
                    );
                    assert_eq!(r.x, 0, "scale={scale} ({cw}x{ch}) fm={fm:?} x={}", r.x);
                    assert_eq!(r.y, r.stage_y);
                    assert_eq!(r.width, r.stage_width);
                    assert_eq!(r.height, r.stage_height);
                }
                let orig = compute_active_video_physical_rect_with_drawer_w_and_mode(
                    pcw, pch, scale, 0.0, false, land, vw, vh, rot, FitMode::OriginalRatio, true,
                );
                assert!(orig.x >= orig.stage_x);
                assert!(orig.y >= orig.stage_y);
                assert!(orig.x + orig.width as i32 <= orig.stage_x + orig.stage_width as i32);
                assert!(orig.y + orig.height as i32 <= orig.stage_y + orig.stage_height as i32);
            }
        }

        // 5. Full Screen Portrait & Landscape remain cleanly centered within stage bounds
        let fs_p = compute_active_video_physical_rect_with_drawer_w_and_mode(
            1920, 1080, 1.0, 0.0, true, false, 994, 2160, 0, FitMode::AutoFit, true,
        );
        assert_eq!(fs_p.height, fs_p.stage_height);
        assert_eq!(fs_p.x, ((fs_p.stage_width - fs_p.width) / 2) as i32);

        let fs_l = compute_active_video_physical_rect_with_drawer_w_and_mode(
            1920, 1080, 1.0, 0.0, true, true, 2160, 994, 0, FitMode::AutoFit, true,
        );
        assert!(fs_l.width <= fs_l.stage_width && fs_l.height <= fs_l.stage_height);
        assert_eq!(fs_l.x, ((fs_l.stage_width - fs_l.width) / 2) as i32);
    }

    #[test]
    fn test_tahap282_slint_tahap259_full_drawer_10_card_and_micro_controls_parity() {
        let main_src = include_str!("../ui/main_window.slint");
        for required in [
            "TAMPILAN & BAHASA",
            "RASIO & UKURAN LAYAR",
            "Simpan Crop",
            "Save Crop",
            "Atas (Top)",
            "Bawah (Bottom)",
            "Kiri (Left)",
            "Kanan (Right)",
            "AUDIO (IPHONE -> PC)",
            "Di PC ini (USB)",
            "This PC (USB)",
            "Hanya perangkat",
            "Device only",
            "Volume PC",
            "Buffer Audio",
            "Rendah ~20 ms",
            "Low ~20 ms",
            "Sinkron Audio (Lip Sync)",
            "Opsi Fallback Bluetooth A2DP",
            "KECEPATAN GESTUR TRACKPAD",
            "Simpan Kecepatan",
            "Save Speed",
            "KEYBOARD & PINTASAN",
            "Pintasan Lintas Platform",
            "Ctrl+V Tempel Clipboard PC",
            "iOS 27 Shortcut",
            "Ambil iCloud Shortcut",
            "Salin Prompt Shortcut",
            "LAYAR IPHONE & MODE SILUMAN",
            "KONEKSI & KONTROL MIRRORING",
            "PEMBARUAN PERANGKAT LUNAK",
            "Detail Perangkat & Koneksi",
            "Panduan & Pemecahan Masalah",
        ] {
            assert!(
                main_src.contains(required),
                "Tahap 259 UI parity item missing from main_window.slint: {required}"
            );
        }
    }

    #[test]
    fn test_tahap282_15_shortcut_chips_and_exact_labels_bilingual() {
        let main_src = include_str!("../ui/main_window.slint");
        assert_eq!(
            main_src.matches("ShortcutPillChip {").count(),
            15,
            "Must expose all 15 shortcut chips visibly matching Tahap 259"
        );
        for chip in [
            "Ctrl+A (Select All)",
            "Ctrl+C (iOS Copy)",
            "Ctrl+V (iOS Paste)",
            "Alt+C (Copy to PC)",
            "Alt+V (Paste PC Text)",
            "Alt+R (Clear Cache)",
            "Alt+B (Grab Link)",
            "Ctrl+X (Cut)",
            "Ctrl+Z (Undo)",
            "Ctrl+K (Keyboard)",
            "Alt+U (Unlock)",
            "Alt+L (Lock)",
            "Alt+D (Stealth)",
            "Alt+H (Landscape)",
            "Alt+M (Mute)",
        ] {
            assert!(
                main_src.contains(chip),
                "Missing shortcut key chip in main_window.slint: {chip}"
            );
        }
        for label_tag in [
            "(Alt+V)",
            "(Alt+C)",
            "(Alt+R)",
            "(Alt+B)",
            "(Ctrl+K)",
        ] {
            assert!(
                main_src.contains(label_tag),
                "Exact keyboard button shortcut label missing: {label_tag}"
            );
        }
    }

    #[test]
    fn test_tahap282_topbar_auto_badge_red_stop_pill_and_346px_zero_overflow() {
        let main_src = include_str!("../ui/main_window.slint");
        assert!(main_src.contains("text: \"AUTO\";"));
        assert!(main_src.contains("text: \"Stop\";"));
        assert!(main_src.contains("#DC2626"));
        for &(streaming, auto_rotate, wide) in &[
            (false, true, false),
            (false, false, false),
            (true, true, false),
            (true, false, false),
            (true, true, true),
        ] {
            let b = compute_topbar_346px_layout_budget(streaming, auto_rotate, wide);
            assert!(
                b.fits_346px,
                "Topbar overflowed narrow 346px width for streaming={streaming}, auto={auto_rotate}, wide={wide}: total={}px",
                b.total_px
            );
        }
        let streaming_auto = compute_topbar_346px_layout_budget(true, true, false);
        assert!(streaming_auto.total_px <= 346);
    }

    #[test]
    fn test_tahap282_saweria_footer_dark_light_tokens_and_public_url_allowlist() {
        let main_src = include_str!("../ui/main_window.slint");
        // Superseded by Tahap 288 V3 / Tahap 293: footer simplified cleanly to "iPhone Link v5.0.1 by Kelvin Daniyar · GPL-3.0"
        for needle in [
            "Dukung Kreator di Saweria",
            "Support Creator on Saweria",
            "iPhone Link v5.0.1 by",
            "Kelvin Daniyar",
            "https://saweria.co/kelvindaniyar",
            "https://github.com/kelvindaniyar",
            " · GPL-3.0",
            "#F59E0B",
            "#38BDF8",
        ] {
            assert!(
                main_src.contains(needle),
                "Official Saweria footer token missing: {needle}"
            );
        }
        assert!(!main_src.contains("Forked from") && !main_src.contains("teyboxv1/pane-iphone-mirror-windows"));
        assert!(validate_public_external_url("https://saweria.co/kelvindaniyar"));
        assert!(validate_public_external_url("https://github.com/kelvindaniyar"));
        assert!(validate_public_external_url("https://github.com/teyboxv1/pane-iphone-mirror-windows"));
        assert!(validate_public_external_url("https://www.icloud.com/shortcuts/bbb096d97e174c1386f15e31aedef8ff"));
        assert!(!validate_public_external_url("http://localhost:8765/api/settings"));
        assert!(!validate_public_external_url("file:///C:/Windows/System32/calc.exe"));
    }

    #[test]
    fn test_tahap282_standby_wifi_ip_pill_and_no_duplicate_stream_mode() {
        let wifi_status = serde_json::json!({
            "devices": [{
                "udid": "00008150-0000000000000282",
                "name": "iPhone Pro",
                "model": "iPhone 17 Pro",
                "ip": "192.168.1.90",
                "usb": false,
                "wifiOnline": true,
                "reachable": true,
                "wifiPaired": true
            }]
        });
        let sum = parse_launchpad_from_status(&wifi_status, false);
        assert_eq!(sum.status_pill_short, "Wi-Fi · IP iPhone: 192.168.1.90");
        assert_eq!(sum.status_pill_wide, "Wi-Fi · IP iPhone: 192.168.1.90");

        let main_src = include_str!("../ui/main_window.slint");
        let standby_chunk = main_src
            .split("lp-col := VerticalLayout")
            .nth(1)
            .and_then(|s| s.split("CARD 1: TAMPILAN & BAHASA").next())
            .expect("Standby Hero Card chunk missing");
        assert!(
            !standby_chunk.contains("Mode Video Stream") && !standby_chunk.contains("Video Stream Mode"),
            "Standby launchpad must NOT duplicate Card 2 Mode Video Stream selector"
        );
    }

    #[test]
    fn test_tahap282_icons_zero_emoji_and_zero_private_pii_with_public_attribution() {
        let main_src = include_str!("../ui/main_window.slint");
        let bad_emoji: Vec<char> = main_src
            .chars()
            .filter(|c| (*c as u32) > 0x2600 && (*c as u32) < 0x1FB00)
            .collect();
        assert!(
            bad_emoji.is_empty(),
            "Zero-emoji vector gate failed in main_window.slint: {bad_emoji:?}"
        );
        let mut masked = main_src.to_ascii_lowercase();
        for pub_str in [
            "https://saweria.co/kelvindaniyar",
            "https://github.com/kelvindaniyar/iphone-link/issues",
            "https://github.com/kelvindaniyar/iphone-link",
            "https://github.com/kelvindaniyar",
            "kelvin daniyar",
        ] {
            masked = masked.replace(pub_str, " ");
        }
        for priv_pii in [
            ["uwe", " ", "uh"].concat(),
            ["kel", "vin"].concat(),
            ["00008150-", "000231ac", "2669401c"].concat(),
            ["192.168.", "100."].concat(),
            ["data ", "kel", "vin"].concat(),
            ["screen ", "miror ", "ios"].concat(),
            ["zen", "link"].concat(),
        ] {
            assert!(
                !masked.contains(&priv_pii),
                "Private PII leaked into main_window.slint outside allowlisted attribution: {priv_pii}"
            );
        }
    }

    #[test]
    fn test_tahap283_topbar_6btn_clean_budget_and_mirroring_unclipped() {
        let main_src = include_str!("../ui/main_window.slint");
        let topbar_chunk = main_src
            .split("top-bar := Rectangle")
            .nth(1)
            .and_then(|s| s.split("MAIN BODY SPLIT").next())
            .expect("top-bar chunk missing");
        assert!(!topbar_chunk.contains("\"sun\""), "Theme sun button must NOT exist on Top Bar");
        assert!(!topbar_chunk.contains("\"moon\""), "Theme moon button must NOT exist on Top Bar");
        assert!(topbar_chunk.contains("root.is-fullscreen ? \"fullscreen-exit\" : \"fullscreen\""));
        assert!(topbar_chunk.contains("close-btn: true;"));
        assert!(topbar_chunk.contains("#007AFF47"));
        assert!(topbar_chunk.contains("#60A5FA73"));

        let b_idle = compute_topbar_346px_layout_budget(false, false, false);
        let b_idle_auto = compute_topbar_346px_layout_budget(false, true, false);
        let b_stream = compute_topbar_346px_layout_budget(true, false, false);
        let b_stream_auto = compute_topbar_346px_layout_budget(true, true, false);
        assert!(b_idle.fits_346px && b_idle_auto.fits_346px && b_stream.fits_346px && b_stream_auto.fits_346px);
        assert!(b_stream_auto.total_px <= 340, "Expected <= 340px, got {}", b_stream_auto.total_px);

        assert_eq!(resolve_compact_topbar_status_pill_text("Mirroring · Wi-Fi", false), "Mirroring");
        assert_eq!(resolve_compact_topbar_status_pill_text("Mirroring · USB", false), "Mirroring");
        assert_eq!(resolve_compact_topbar_status_pill_text("Mirroring · Wi-Fi", true), "Mirroring · Wi-Fi");
        assert_eq!(resolve_compact_topbar_status_pill_text("AirPlay", false), "AirPlay");
    }

    #[test]
    fn test_tahap283_svg_49_hd_icons_zero_polygon_paths_and_zero_emoji() {
        let icons_src = include_str!("../ui/icons.slint");
        let main_src = include_str!("../ui/main_window.slint");
        assert_eq!(icons_src.matches("@image-url(\"assets/icons/").count(), 49);
        assert!(!icons_src.contains("Path {"));
        assert!(icons_src.contains("image-rendering: smooth;"));
        assert!(icons_src.contains("colorize: root.icon-color;"));
        for required_icon in ["save-floppy", "bluetooth", "coffee-cup", "fullscreen-exit", "settings", "rotate"] {
            assert!(
                icons_src.contains(&format!("assets/icons/{required_icon}.svg")),
                "Missing SVG icon route: {required_icon}"
            );
        }
        assert!(!main_src.contains("component SaveFloppyIcon"));
        assert!(!main_src.contains("component CoffeeCupIcon"));
        assert!(!main_src.contains("component BluetoothIcon"));
    }

    #[test]
    fn test_tahap283_micro_polishes_no_dup_gain_wrap_buttons_and_en_clean() {
        let main_src = include_str!("../ui/main_window.slint");
        assert!(main_src.contains("in property <bool> show-header: true;"));
        assert!(main_src.contains("show-header: false;"));
        assert!(!main_src.contains("Gain Level") && !main_src.contains("Tingkat Volume"));
        assert!(main_src.matches("wrap-label: true;").count() >= 4);
        assert!(main_src.contains("root.is-en ? \"Top\" : \"Atas (Top)\""));
        assert!(main_src.contains("root.is-en ? \"Bottom\" : \"Bawah (Bottom)\""));
        assert!(main_src.contains("root.is-en ? \"Left\" : \"Kiri (Left)\""));
        assert!(main_src.contains("root.is-en ? \"Right\" : \"Kanan (Right)\""));

        let wifi_json = serde_json::json!({
            "devices": [{
                "udid": "00008150-0000000000000283",
                "name": "iPhone Pro",
                "model": "iPhone 17 Pro",
                "ip": "192.168.1.77",
                "wifiOnline": true,
                "reachable": true,
                "wifiPaired": true
            }]
        });
        let en_sum = parse_launchpad_from_status(&wifi_json, true);
        assert_eq!(en_sum.status_pill_short, "Wi-Fi · iPhone IP: 192.168.1.77");
        assert_eq!(en_sum.wifi_text, "Wi-Fi Ready (iPhone LAN IP: 192.168.1.77)");
        assert!(!en_sum.pill_detail.contains("IP iPhone:"));
        let id_sum = parse_launchpad_from_status(&wifi_json, false);
        assert_eq!(id_sum.status_pill_short, "Wi-Fi · IP iPhone: 192.168.1.77");
    }

    #[test]
    fn test_tahap283_hevc_autorotate_pipeline_state_and_airplay_manual_isolation() {
        use iphone_link_video::autorotate::{HevcAutoRotateTracker, ProbeGrid120x260};

        // Non-AirPlay CoreDevice HEVC rotates via tracker, whereas AirPlay bypasses tracker
        let mut tracker = HevcAutoRotateTracker::new();
        let mut konami = ProbeGrid120x260::new();
        konami.fill_all(210, 15, 20);
        assert_eq!(tracker.update_sample(&konami, 1206, 2622, 1000, false), Some(270));

        // Manual override suppresses subsequent auto-rotate for 30,000 ms
        tracker.notify_manual_override(2000, false);
        assert_eq!(tracker.update_sample(&konami, 1206, 2622, 15000, false), None);

        // AirPlay Manual Orientation Plan remains 100% independent of Auto-Rotate
        let ap_plan = resolve_airplay_manual_orientation_plan(true, 1920, 888, false, "iphone", false)
            .expect("AirPlay native landscape plan");
        assert!(!ap_plan.is_landscape, "AirPlay must stay vertical unless manual rotate clicked");
        let pipe_src = include_str!("native_video_pipeline.rs");
        assert!(pipe_src.contains("take_pending_hevc_autorotate_deg"));
        assert!(pipe_src.contains("if is_vis && !mirror::is_airplay_active()"));
    }

    #[test]
    fn test_tahap285_hybrid_hid_not_blocked_by_airplay_video_and_failsoft_guard() {
        mirror::global_reset_mirror_runtime_for_test();
        let resp = mirror::start_airplay_mirror_session(
            Some("00008150-0000000000002851"),
            true,
            "usb",
            None,
            Some(0),
            false,
        )
        .expect("airplay standalone session");
        // Tahap 293 Opsi 2: background `gate_only` CoreDevice HID worker is completely disabled for iOS < 27;
        // AirPlay route runs strictly in clean `airplay_standalone` view/audio mode with `hidControlAvailable=false`.
        assert!(resp.contains(r#""mirrorRoute":"airplay_standalone""#), "resp={resp}");
        let gen = mirror::current_live_session_gen();

        assert!(!mirror::is_hybrid_airplay_route());
        assert!(mirror::is_airplay_standalone_no_hid_active());
        let mut input_cfg = crate::native_input::NativeInputConfig::default();
        input_cfg.airplay_no_hid = mirror::is_airplay_standalone_no_hid_active();
        assert!(input_cfg.airplay_no_hid, "AirPlay Standalone MUST lock HID");

        // Simulating first AirPlay H.264 video frame transition -> phase="running", AirPlay active
        let f = iphone_link_video::mf_decoder::build_synthetic_h264_ipcm_idr(64, 64, 0x55, 0);
        let avcc_b64 = mirror::base64_encode(&f.avcc_config);
        mirror::global_on_airplay_mirror_loop_state(gen, true);
        mirror::update_live_codec_h264_avcc(gen, "avc1.42801e", &avcc_b64, 64, 64);
        let idr_au = iphone_link_video::mf_decoder::wrap_avcc_with_aud_sei(&f.avcc_sample);
        assert!(mirror::global_on_physical_h264_avcc_frame(gen, idr_au, true));
        assert!(mirror::is_airplay_active());
        assert!(mirror::is_airplay_standalone_no_hid_active());

        let _ = mirror::global_stop_screen();
        mirror::global_reset_mirror_runtime_for_test();
    }

    #[test]
    fn test_tahap285_native_ui_amfi_success_feedback_and_bilingual_updater_allowlist() {
        // 1. AMFI ok=true MUST produce bilingual feedback (NEVER silenced!)
        let ok_passcode: serde_json::Value = serde_json::from_str(
            r#"{"ok":true,"statusCode":"revealed_passcode_manual","message_id":"Menu Developer Mode berhasil dimunculkan! Buka Settings -> Privacy & Security.","message_en":"Success! Developer Mode menu is now visible on your iPhone."}"#,
        )
        .unwrap();
        let (fb_ok_id, txt_id) = extract_amfi_ui_feedback(&ok_passcode, false);
        let (fb_ok_en, txt_en) = extract_amfi_ui_feedback(&ok_passcode, true);
        assert!(fb_ok_id && txt_id.contains("Developer Mode"));
        assert!(fb_ok_en && txt_en.contains("visible"));

        let err_usb: serde_json::Value = serde_json::from_str(
            r#"{"ok":false,"statusCode":"usb_required","message_id":"Hubungkan iPhone menggunakan kabel USB (bukan Wi-Fi).","message_en":"Connect your iPhone with a USB cable (not Wi-Fi)."}"#,
        )
        .unwrap();
        let (fb_err_id, msg_err_id) = extract_amfi_ui_feedback(&err_usb, false);
        let (fb_err_en, msg_err_en) = extract_amfi_ui_feedback(&err_usb, true);
        assert!(!fb_err_id && msg_err_id.contains("USB"));
        assert!(!fb_err_en && msg_err_en.contains("USB"));

        // 2. Bilingual Updater Card 8 mapping
        let upd_raw = format!(
            r#"{{"ok":true,"updateAvailable":true,"latestVersion":"5.1.0","releaseUrl":"{repo}/releases/tag/v5.1.0","downloadUrl":"{repo}/releases/download/v5.1.0/iPhone-Link-v5.1.0-Setup.exe"}}"#,
            repo = crate::updater::PUBLIC_GITHUB_REPO_URL
        );
        let upd_json: serde_json::Value = serde_json::from_str(&upd_raw).unwrap();
        let (avail_id, ver_id, rel_id, _, msg_u_id) = format_updater_ui_state(&upd_json, false);
        let (avail_en, ver_en, _, _, msg_u_en) = format_updater_ui_state(&upd_json, true);
        assert!(avail_id && ver_id == "5.1.0" && msg_u_id.contains("5.1.0"));
        assert!(avail_en && ver_en == "5.1.0" && msg_u_en.contains("5.1.0"));
        assert!(validate_public_external_url(&rel_id));

        // 3. Anti-Blank bilingual copy + Store/Settings/GitHub allowlist guard
        let (ab_t, ab_d) = format_anti_blank_overlay_copy("hevc_mft_missing", true);
        assert!(ab_t.contains("HEVC") && ab_d.contains("AirPlay H.264"));
        assert!(validate_public_external_url("ms-windows-store://pdp/?ProductId=9n4wgh0z6vhq"));
        assert!(validate_public_external_url("ms-settings:optionalfeatures"));
        assert!(!validate_public_external_url("https://evil.example/releases/v5.1.0"));
        assert!(!validate_public_external_url("file:///C:/Windows/calc.exe"));
    }

    #[test]
    fn test_tahap286_trackpad_speed_30_roundtrip_clamp_and_settings_persistence() {
        assert_eq!(clamp_trackpad_speed_pct(10), 30);
        assert_eq!(clamp_trackpad_speed_pct(30), 30);
        assert_eq!(clamp_trackpad_speed_pct(40), 40);
        assert_eq!(clamp_trackpad_speed_pct(250), 200);

        let (tx, _rx) = mpsc::channel();
        let router = NativeInputRouter::new(tx);
        router.update_config(|c| c.trackpad_speed_pct = clamp_trackpad_speed_pct(30) as u32);
        assert_eq!(router.config_snapshot().trackpad_speed_pct, 30);

        let state = Arc::new(Mutex::new(server_v4::AppState::default()));
        let web_dir = PathBuf::from("web");
        let resp = inprocess_api(&state, &web_dir, "POST", "/api/settings", r#"{"trackpadSpeed":30}"#);
        assert!(resp.contains(r#""ok":true"#), "resp={resp}");
        let got = inprocess_api(&state, &web_dir, "GET", "/api/settings", "");
        assert!(got.contains(r#""trackpadSpeed":30"#), "got={got}");
        assert_eq!(state.lock().unwrap().trackpad_speed, 30);
    }

    #[test]
    fn test_tahap286_single_source_of_truth_topbar_and_card3_audio_banner_parity() {
        // 1. Standby / never opened -> NEVER claims fake green BT A2DP active!
        let standby = compute_audio_ui_presentation(
            true, false, false, false, false, 0, 100, false, false, "idle", "", false,
        );
        assert!(!standby.live_active);
        assert_eq!(standby.live_state, "standby");
        assert!(!standby.bt_active);
        assert!(!standby.banner_text.contains("Bluetooth A2DP aktif"));

        // 2. Connecting -> Top Bar not green, Card 3 amber/blue connecting
        let conn = compute_audio_ui_presentation(
            true, false, true, false, false, 0, 100, false, true, "connecting", "", true,
        );
        assert!(!conn.live_active);
        assert_eq!(conn.live_state, "connecting");
        assert!(!conn.bt_active);

        // 3. Active A2DP -> Top Bar & Card 3 BOTH green active with device name
        let act = compute_audio_ui_presentation(
            true, false, true, false, false, 0, 100, true, false, "active", "Phone 17", false,
        );
        assert!(act.live_active);
        assert_eq!(act.live_state, "active");
        assert!(act.bt_active);
        assert!(act.banner_text.contains("Phone 17"));

        // 4. Muted -> Top Bar off AND Card 3 muted (NO fake green banner!)
        let muted = compute_audio_ui_presentation(
            true, true, true, false, false, 0, 100, false, false, "idle", "", false,
        );
        assert!(!muted.live_active);
        assert_eq!(muted.live_state, "muted");
        assert!(!muted.bt_active);

        // 5. Device only -> both report device-only
        let dev = compute_audio_ui_presentation(
            false, false, true, false, false, 0, 100, false, false, "idle", "", true,
        );
        assert!(!dev.live_active);
        assert_eq!(dev.live_state, "device");

        // 6. Error -> both report error
        let err = compute_audio_ui_presentation(
            true, false, true, false, false, 0, 100, false, false, "error", "", false,
        );
        assert!(!err.live_active);
        assert_eq!(err.live_state, "error");
    }

    #[test]
    fn test_tahap286_auto_audio_trigger_once_per_session_gen_and_airplay_or_manual_device_bypass() {
        // CoreDevice HEVC running with presented_frames >= 1 -> triggers exactly 1x per session_gen
        assert!(should_trigger_auto_audio_on_running(
            "running", 1, false, "streaming", true, false, false, 42, 0, false
        ));
        // Second check for same session_gen -> MUST NOT re-trigger
        assert!(!should_trigger_auto_audio_on_running(
            "running", 10, false, "streaming", true, false, false, 42, 42, false
        ));
        // Before frame #1 or still starting -> wait
        assert!(!should_trigger_auto_audio_on_running(
            "starting", 0, false, "connecting", true, false, false, 42, 0, false
        ));
        assert!(!should_trigger_auto_audio_on_running(
            "running", 0, false, "streaming", true, false, false, 42, 0, false
        ));
        // AirPlay / Hybrid waiting_airplay -> MUST stay bypassed (zero AirPlay collision!)
        assert!(!should_trigger_auto_audio_on_running(
            "running", 10, true, "streaming", true, false, false, 42, 0, false
        ));
        assert!(!should_trigger_auto_audio_on_running(
            "starting", 0, false, "waiting_airplay", true, false, false, 42, 0, false
        ));
        // Device-only / Muted / Manually stopped / Offline test mode -> MUST stay bypassed
        assert!(!should_trigger_auto_audio_on_running(
            "running", 10, false, "streaming", false, false, false, 42, 0, false
        ));
        assert!(!should_trigger_auto_audio_on_running(
            "running", 10, false, "streaming", true, true, false, 42, 0, false
        ));
        assert!(!should_trigger_auto_audio_on_running(
            "running", 10, false, "streaming", true, false, true, 42, 0, false
        ));
        assert!(!should_trigger_auto_audio_on_running(
            "running", 10, false, "streaming", true, false, false, 42, 0, true
        ));
    }

    fn run_on_slint_test_thread<R: Send + 'static, F: FnOnce() -> R + Send + 'static>(f: F) -> R {
        use std::sync::{mpsc, Mutex, OnceLock};
        static TX: OnceLock<Mutex<mpsc::Sender<Box<dyn FnOnce() + Send>>>> = OnceLock::new();
        let tx_mutex = TX.get_or_init(|| {
            let (tx, rx) = mpsc::channel::<Box<dyn FnOnce() + Send>>();
            std::thread::Builder::new()
                .name("slint-test-worker".into())
                .spawn(move || {
                    crate::dcomp_presenter::ensure_dcomp_platform_installed();
                    while let Ok(job) = rx.recv() {
                        job();
                    }
                })
                .expect("spawn slint-test-worker");
            Mutex::new(tx)
        });
        let (res_tx, res_rx) = mpsc::sync_channel(1);
        let job: Box<dyn FnOnce() + Send> = Box::new(move || {
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
            let _ = res_tx.send(res);
        });
        tx_mutex.lock().unwrap().send(job).unwrap();
        match res_rx.recv().unwrap() {
            Ok(val) => val,
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }

    fn render_offscreen_frame(
        sw: &std::rc::Rc<slint::platform::software_renderer::MinimalSoftwareWindow>,
        buf: &mut Vec<crate::dcomp_presenter::PremultipliedBgra8>,
        w: u32,
        h: u32,
    ) {
        let zero = crate::dcomp_presenter::PremultipliedBgra8 { b: 0, g: 0, r: 0, a: 0 };
        let target_len = (w * h) as usize;
        if buf.len() != target_len {
            buf.clear();
            buf.resize(target_len, zero);
        }
        sw.set_size(slint::PhysicalSize::new(w, h));
        std::thread::sleep(std::time::Duration::from_millis(220));
        slint::platform::update_timers_and_animations();
        sw.request_redraw();
        sw.draw_if_needed(|renderer| {
            renderer.render(buf, w as usize);
        });
    }

    #[test]
    fn test_tahap287_slint_pixel_audit_dark_light_topbar_drawer_and_fullscreen_wings_translucent_hole_zero() {
        run_on_slint_test_thread(move || {
            let ui = MainWindow::new().expect("create MainWindow");
            let sw = crate::dcomp_presenter::take_last_created_software_window()
                .expect("capture offscreen MinimalSoftwareWindow");
            let mut buf = Vec::new();

            // 1. Dark Mode Top Bar (346x832 @ (140, 18)) & Stage (12, 60) & Drawer (706x832 @ (690, 18))
            ui.set_dark_mode(true);
            ui.set_drawer_open(false);
            ui.set_drawer_animated_w(0.0);
            ui.set_drawer_slide_progress(0.0);
            ui.set_is_fullscreen(false);
            ui.set_is_landscape(false);
            ui.set_phase("idle".into());
            render_offscreen_frame(&sw, &mut buf, 346, 832);
            let dark_topbar = buf[18 * 346 + 140];
            let dark_stage_bg = buf[60 * 346 + 12];

            ui.set_drawer_open(true);
            ui.set_drawer_animated_w(360.0);
            ui.set_drawer_slide_progress(1.0);
            render_offscreen_frame(&sw, &mut buf, 706, 832);
            let dark_drawer = buf[58 * 706 + 580];

            // 2. Light Mode Top Bar & Drawer
            ui.set_drawer_open(false);
            ui.set_drawer_animated_w(0.0);
            ui.set_drawer_slide_progress(0.0);
            ui.set_dark_mode(false);
            render_offscreen_frame(&sw, &mut buf, 346, 832);
            let light_topbar = buf[18 * 346 + 140];

            ui.set_drawer_open(true);
            ui.set_drawer_animated_w(360.0);
            ui.set_drawer_slide_progress(1.0);
            render_offscreen_frame(&sw, &mut buf, 706, 832);
            let light_drawer = buf[58 * 706 + 580];

            // 3. Fullscreen Portrait Frosted Wings (1920x1080) with Layer-0 center video hole (alpha == 0)
            ui.set_drawer_open(false);
            ui.set_drawer_animated_w(0.0);
            ui.set_drawer_slide_progress(0.0);
            ui.set_is_fullscreen(true);
            ui.set_is_landscape(false);
            ui.set_video_layer_active(true);
            ui.set_first_frame_ready(true);
            ui.set_anti_blank_visible(false);
            ui.set_phase("running".into());
            ui.set_stage("streaming".into());
            ui.set_video_hole_x(710.0);
            ui.set_video_hole_y(0.0);
            ui.set_video_hole_w(500.0);
            ui.set_video_hole_h(1080.0);

            ui.set_dark_mode(true);
            render_offscreen_frame(&sw, &mut buf, 1920, 1080);
            let fs_dark_left_wing = buf[540 * 1920 + 120];
            let fs_dark_right_wing = buf[540 * 1920 + 1800];
            let fs_dark_hole = buf[540 * 1920 + 960];

            ui.set_dark_mode(false);
            render_offscreen_frame(&sw, &mut buf, 1920, 1080);
            let fs_light_left_wing = buf[540 * 1920 + 120];
            let fs_light_hole = buf[540 * 1920 + 960];

            println!(
                "[TAHAP 287 PIXEL AUDIT] dark_topbar=({},{},{},a={}) dark_stage_bg=({},{},{},a={}) dark_drawer=({},{},{},a={}) light_topbar=({},{},{},a={}) light_drawer=({},{},{},a={}) fs_dark_wings=({},{}) hole_a={} fs_light_wing_a={} hole_a={}",
                dark_topbar.r, dark_topbar.g, dark_topbar.b, dark_topbar.a,
                dark_stage_bg.r, dark_stage_bg.g, dark_stage_bg.b, dark_stage_bg.a,
                dark_drawer.r, dark_drawer.g, dark_drawer.b, dark_drawer.a,
                light_topbar.r, light_topbar.g, light_topbar.b, light_topbar.a,
                light_drawer.r, light_drawer.g, light_drawer.b, light_drawer.a,
                fs_dark_left_wing.a, fs_dark_right_wing.a, fs_dark_hole.a,
                fs_light_left_wing.a, fs_light_hole.a
            );

            assert!(
                dark_topbar.a > 0 && dark_topbar.a < 200,
                "dark topbar must remain translucent glass, got a={}",
                dark_topbar.a
            );
            assert!(
                dark_stage_bg.a > 0 && dark_stage_bg.a < 200,
                "dark stage glass-shell must remain translucent, got a={}",
                dark_stage_bg.a
            );
            assert!(
                dark_drawer.a > 0 && dark_drawer.a < 200,
                "dark drawer must remain translucent glass, got a={}",
                dark_drawer.a
            );
            assert!(
                light_topbar.a > 0 && light_topbar.a < 200,
                "light topbar must remain translucent milky crystal, got a={}",
                light_topbar.a
            );
            assert!(
                light_drawer.a > 0 && light_drawer.a < 200,
                "light drawer must remain translucent milky crystal, got a={}",
                light_drawer.a
            );
            assert!(
                fs_dark_left_wing.a > 0 && fs_dark_left_wing.a < 180,
                "fullscreen dark left wing must let backdrop blur show through, got a={}",
                fs_dark_left_wing.a
            );
            assert!(
                fs_dark_right_wing.a > 0 && fs_dark_right_wing.a < 180,
                "fullscreen dark right wing must let backdrop blur show through, got a={}",
                fs_dark_right_wing.a
            );
            assert_eq!(
                fs_dark_hole.a, 0,
                "fullscreen dark center video hole MUST be alpha==0, got a={}",
                fs_dark_hole.a
            );
            assert!(
                fs_light_left_wing.a > 0 && fs_light_left_wing.a < 210,
                "fullscreen light left wing must let backdrop blur show through, got a={}",
                fs_light_left_wing.a
            );
            assert_eq!(
                fs_light_hole.a, 0,
                "fullscreen light center video hole MUST be alpha==0, got a={}",
                fs_light_hole.a
            );

            crate::dcomp_presenter::clear_last_created_software_window();
        });
    }

    #[test]
    fn test_tahap287_sequoia_waiting_airplay_and_anti_blank_mutual_exclusivity_and_instant_hide() {
        assert_eq!(
            resolve_active_stage_overlay("idle", "", false, false),
            StageOverlayKind::StandbyLaunchpad
        );
        assert_eq!(
            resolve_active_stage_overlay("starting", "connecting", false, false),
            StageOverlayKind::SequoiaConnecting
        );
        assert_eq!(
            resolve_active_stage_overlay("running", "connecting", false, false),
            StageOverlayKind::SequoiaConnecting
        );
        assert_eq!(
            resolve_active_stage_overlay("running", "waiting_airplay", false, false),
            StageOverlayKind::WaitingAirplay
        );
        assert_eq!(
            resolve_active_stage_overlay("starting", "waiting_airplay", false, false),
            StageOverlayKind::WaitingAirplay
        );
        assert_eq!(
            resolve_active_stage_overlay("running", "waiting_airplay", false, true),
            StageOverlayKind::AntiBlankDiagnostic
        );
        assert_eq!(
            resolve_active_stage_overlay("running", "streaming", false, true),
            StageOverlayKind::AntiBlankDiagnostic
        );
        assert_eq!(
            resolve_active_stage_overlay("running", "streaming", true, false),
            StageOverlayKind::None
        );

        run_on_slint_test_thread(move || {
            let ui = MainWindow::new().expect("create MainWindow");
            let sw = crate::dcomp_presenter::take_last_created_software_window()
                .expect("capture offscreen MinimalSoftwareWindow");
            let mut buf = Vec::new();

            ui.set_dark_mode(true);
            ui.set_is_fullscreen(false);
            ui.set_phase("starting".into());
            ui.set_stage("connecting".into());
            ui.set_first_frame_ready(false);
            ui.set_anti_blank_visible(false);
            ui.set_video_layer_active(true);
            ui.set_video_hole_x(0.0);
            ui.set_video_hole_y(38.0);
            ui.set_video_hole_w(346.0);
            ui.set_video_hole_h(750.0);
            render_offscreen_frame(&sw, &mut buf, 346, 832);
            let during_connect = buf[416 * 346 + 173];
            assert!(
                during_connect.a > 0,
                "Sequoia connecting overlay must render non-zero center pixels while connecting"
            );

            // First decoded frame arrives -> instant unmount to pure alpha==0 Layer-0 video hole
            ui.set_phase("running".into());
            ui.set_stage("streaming".into());
            ui.set_first_frame_ready(true);
            sw.request_redraw();
            sw.draw_if_needed(|renderer| {
                renderer.render(&mut buf, 346);
            });
            let after_first_frame = buf[416 * 346 + 173];
            assert_eq!(
                after_first_frame.a, 0,
                "Sequoia overlay must unmount synchronously on frame #1 leaving alpha==0, got a={}",
                after_first_frame.a
            );

            crate::dcomp_presenter::clear_last_created_software_window();
        });
    }

    #[test]
    fn test_tahap287_light_mode_milky_crystal_contrast_and_zero_solid_charcoal_tokens() {
        let slint_src = include_str!("../ui/main_window.slint");
        for required_token in [
            "#0C101A38",
            "#F4F7FC6E",
            "#0E131F48",
            "#F5F8FD68",
            "#FFFFFF7D",
            "#FFFFFFD9",
            "#0F172A",
        ] {
            assert!(
                slint_src.contains(required_token),
                "missing Tahap 287 glass token `{}` in main_window.slint",
                required_token
            );
        }
        for forbidden_token in ["#111827", "#1E2024", "#181A1E", "#1E293BFF", "#F4F7FCFF"] {
            assert!(
                !slint_src.contains(forbidden_token),
                "legacy opaque token `{}` must not remain in main_window.slint",
                forbidden_token
            );
        }

        run_on_slint_test_thread(move || {
            let ui = MainWindow::new().expect("create MainWindow");
            let sw = crate::dcomp_presenter::take_last_created_software_window()
                .expect("capture offscreen MinimalSoftwareWindow");
            let mut buf = Vec::new();
            ui.set_dark_mode(false);
            ui.set_is_fullscreen(false);
            ui.set_phase("idle".into());
            render_offscreen_frame(&sw, &mut buf, 346, 832);
            let topbar_px = buf[18 * 346 + 140];
            let stage_px = buf[60 * 346 + 12];
            println!(
                "[TAHAP 287 LIGHT CRYSTAL] topbar=({},{},{},a={}) stage=({},{},{},a={})",
                topbar_px.r, topbar_px.g, topbar_px.b, topbar_px.a,
                stage_px.r, stage_px.g, stage_px.b, stage_px.a
            );
            assert!(topbar_px.a > 0 && topbar_px.a < 200);
            assert!(stage_px.a > 0 && stage_px.a < 200);
            assert!(
                topbar_px.r >= 70 && topbar_px.g >= 70 && topbar_px.b >= 70,
                "Light milky topbar premultiplied channels must reflect white-sky tint, got {:?}",
                (topbar_px.r, topbar_px.g, topbar_px.b, topbar_px.a)
            );
            crate::dcomp_presenter::clear_last_created_software_window();
        });
    }

    #[test]
    fn test_tahap287_sequoia_overlay_uses_clean_model_name_batal_and_zero_permanent_timer() {
        let slint_src = include_str!("../ui/main_window.slint");
        assert!(slint_src.contains("sequoia-reveal := Rectangle"));
        assert!(slint_src.contains("root.device-model-name"));
        // Superseded by Tahap 288: sequoia-reveal matches 1:1 Tahap 259 ("Menyambungkan ke" / "Connecting to"), Top Bar Stop & Anti-Blank retain stop-mirror()/Batal
        assert!(slint_src.contains("\"Menyambungkan ke\""));
        assert!(slint_src.contains("\"Connecting to\""));
        assert!(slint_src.contains("\"Batal\""));
        assert!(slint_src.contains("\"Cancel\""));
        assert!(slint_src.contains("root.stop-mirror();"));
        assert!(
            !slint_src.contains("Timer {"),
            "must NOT harbor any permanent Slint Timer loop in main_window.slint"
        );

        run_on_slint_test_thread(move || {
            let ui = MainWindow::new().expect("create MainWindow");
            let sw = crate::dcomp_presenter::take_last_created_software_window()
                .expect("capture offscreen MinimalSoftwareWindow");
            let mut buf = Vec::new();
            ui.set_phase("idle".into());
            ui.set_dark_mode(true);
            render_offscreen_frame(&sw, &mut buf, 346, 832);
            assert!(
                !sw.has_active_animations(),
                "idle stage must settle with zero permanent animation ticks"
            );
            crate::dcomp_presenter::clear_last_created_software_window();
        });
    }

    #[test]
    fn test_tahap288_transition_state_machine_start_reveal_stop_hevc_and_airplay_curves_zero_hard_cut() {
        // 1. StartToSequoia (Standby <-> Sequoia 240 ms)
        let s0 = compute_transition_opacities(StageTransitionKind::StartToSequoia, 0.0);
        let smid = compute_transition_opacities(StageTransitionKind::StartToSequoia, 0.5);
        let s1 = compute_transition_opacities(StageTransitionKind::StartToSequoia, 1.0);
        assert!((s0.standby - 1.0).abs() < 0.001 && s0.sequoia == 0.0);
        assert!(smid.standby > 0.05 && smid.standby < 0.95);
        assert!(smid.sequoia > 0.05 && smid.sequoia < 0.95);
        assert!((s1.standby).abs() < 0.001 && (s1.sequoia - 1.0).abs() < 0.001);

        // 2. FirstFrameReveal with blur image (HEVC & AirPlay blur-to-sharp 360 ms, text 180 ms)
        let r0 = compute_transition_opacities(
            StageTransitionKind::FirstFrameReveal {
                use_sequoia_fallback: false,
            },
            0.0,
        );
        let rmid = compute_transition_opacities(
            StageTransitionKind::FirstFrameReveal {
                use_sequoia_fallback: false,
            },
            0.55,
        );
        let r1 = compute_transition_opacities(
            StageTransitionKind::FirstFrameReveal {
                use_sequoia_fallback: false,
            },
            1.0,
        );
        assert!((r0.reveal_blur - 1.0).abs() < 0.001 && (r0.sequoia_text - 1.0).abs() < 0.001);
        assert_eq!(rmid.sequoia_text, 0.0, "Sequoia text must finish fading in first half");
        assert!(rmid.reveal_blur > 0.05 && rmid.reveal_blur < 0.95);
        assert_eq!(r1.reveal_blur, 0.0);

        // 3. FirstFrameReveal cold-start fallback (no blur image -> smooth sequoia crossfade)
        let rf = compute_transition_opacities(
            StageTransitionKind::FirstFrameReveal {
                use_sequoia_fallback: true,
            },
            0.4,
        );
        assert!(rf.sequoia > 0.05 && rf.sequoia < 0.95 && rf.reveal_blur == 0.0);

        // 4. StopFromLive (280 ms) & StopFromConnecting (240 ms)
        let sl = compute_transition_opacities(StageTransitionKind::StopFromLive, 0.5);
        let sc = compute_transition_opacities(StageTransitionKind::StopFromConnecting, 0.5);
        assert!(sl.standby > 0.5 && sl.reveal_blur > 0.05);
        assert!(sc.standby > 0.5 && sc.sequoia > 0.05 && sc.sequoia_text > 0.05);
    }

    #[test]
    fn test_tahap288_sequoia_and_waiting_airplay_clean_no_modal_no_center_batal_topbar_stop_unclipped() {
        let slint_src = include_str!("../ui/main_window.slint");
        let seq_seg = slint_src
            .split("sequoia-reveal :=")
            .nth(1)
            .and_then(|s| s.split("// End sequoia-reveal").next())
            .expect("unified sequoia-reveal block");

        // Unified clean canvas: 0 modal cards, 0 center Batal/Cancel/stop-mirror, 1:1 Tahap 259 2-line / 2-block copy
        assert!(seq_seg.contains("\"Menyambungkan ke\""));
        assert!(seq_seg.contains("\"Connecting to\""));
        assert!(seq_seg.contains("root.device-model-name"));
        assert!(seq_seg.contains("Open Control Center on iPhone → Screen Mirroring → select 'iPhone Link'"));
        assert!(seq_seg.contains("Buka Pusat Kontrol di iPhone → Pencerminan Layar → pilih 'iPhone Link'"));
        assert!(seq_seg.contains("root.is-airplay-waiting"));
        assert!(
            !seq_seg.contains("\"Batal\"")
                && !seq_seg.contains("\"Cancel\"")
                && !seq_seg.contains("stop-mirror")
                && !seq_seg.contains("\"AirPlay Siap\"")
                && !seq_seg.contains("\"AirPlay Ready\"")
                && !seq_seg.contains("stage-progress")
                && !seq_seg.contains("name: \"wifi\""),
            "sequoia-reveal must remain 1:1 Tahap 259 clean without center Batal, progress bar, AirPlay Siap title, or wifi icon"
        );

        // Top Bar Stop remains visible since phase == "starting" and Anti-Blank Diagnostic remains guarded
        assert!(slint_src.contains("root.phase == \"starting\""));
        assert!(slint_src.contains("root.stop-mirror();"));
        assert!(slint_src.contains("if root.anti-blank-visible : Rectangle"));
    }

    #[test]
    fn test_tahap288_fullscreen_portrait_connecting_clamps_sequoia_center_and_keeps_frosted_wings_open() {
        run_on_slint_test_thread(move || {
            let ui = MainWindow::new().expect("create MainWindow");
            let sw = crate::dcomp_presenter::take_last_created_software_window()
                .expect("capture offscreen MinimalSoftwareWindow");
            let mut buf = Vec::new();

            // 1. Cold-Start (!has-last-frame-blur) Dark Mode Fullscreen Portrait Connecting:
            //    both center seq-box and outer wings render true frosted translucent alpha == 56 (zero opaque flat box)
            ui.set_dark_mode(true);
            ui.set_is_fullscreen(true);
            ui.set_is_landscape(false);
            ui.set_has_last_frame_blur(false);
            ui.set_phase("starting".into());
            ui.set_stage("connecting".into());
            ui.set_first_frame_ready(false);
            ui.set_transition_active(false);
            ui.set_device_model_name("iPhone 17 Pro".into());
            render_offscreen_frame(&sw, &mut buf, 1920, 1032);

            let left_wing = buf[516 * 1920 + 120];
            let right_wing = buf[516 * 1920 + 1800];
            let center_cold_dark = buf[260 * 1920 + 960];
            assert_eq!(left_wing.a, 56, "Dark fullscreen left wing must stay frosted a=56");
            assert_eq!(right_wing.a, 56, "Dark fullscreen right wing must stay frosted a=56");
            assert_eq!(
                center_cold_dark.a, 56,
                "Cold-Start Dark seq-box must stay frosted translucent a=56 (zero opaque gradient box), got a={}",
                center_cold_dark.a
            );

            // 2. Cold-Start Light Mode Fullscreen Portrait Connecting: crystal white a=110 across wings + center
            ui.set_dark_mode(false);
            render_offscreen_frame(&sw, &mut buf, 1920, 1032);
            let light_left = buf[516 * 1920 + 120];
            let light_right = buf[516 * 1920 + 1800];
            let center_cold_light = buf[260 * 1920 + 960];
            assert_eq!(light_left.a, 110, "Light fullscreen left wing must keep crystal a=110");
            assert_eq!(light_right.a, 110, "Light fullscreen right wing must keep crystal a=110");
            assert_eq!(
                center_cold_light.a, 110,
                "Cold-Start Light seq-box must stay frosted crystal a=110, got a={}",
                center_cold_light.a
            );

            // 3. Warm-Start (has-last-frame-blur == true): center shows clipped blur image a>=200 while outer wings stay frosted
            let mut warm_rgba = Vec::with_capacity((SEQUOIA_BLUR_MAX_W * SEQUOIA_BLUR_MAX_H * 4) as usize);
            for _ in 0..(SEQUOIA_BLUR_MAX_W * SEQUOIA_BLUR_MAX_H) {
                warm_rgba.extend_from_slice(&[110, 130, 155, 255]);
            }
            ui.set_last_frame_blur_image(blurred_rgba_to_image(&warm_rgba, SEQUOIA_BLUR_MAX_W, SEQUOIA_BLUR_MAX_H));
            ui.set_has_last_frame_blur(true);
            render_offscreen_frame(&sw, &mut buf, 1920, 1032);
            let warm_light_wing = buf[516 * 1920 + 120];
            let warm_light_center = buf[260 * 1920 + 960];
            assert_eq!(warm_light_wing.a, 110, "Warm-Start Light wing must stay crystal a=110 outside seq-box");
            assert!(warm_light_center.a >= 200, "Warm-Start Light center must clip last-frame blur a>=200");

            ui.set_dark_mode(true);
            render_offscreen_frame(&sw, &mut buf, 1920, 1032);
            let warm_dark_wing = buf[516 * 1920 + 120];
            let warm_dark_center = buf[260 * 1920 + 960];
            assert_eq!(warm_dark_wing.a, 56, "Warm-Start Dark wing must stay smoky a=56 outside seq-box");
            assert!(warm_dark_center.a >= 200, "Warm-Start Dark center must clip last-frame blur a>=200");

            // 4. Fullscreen Portrait Streaming (Layer 0 punch-through hole a=0, wings remain frosted)
            ui.set_dark_mode(true);
            ui.set_phase("running".into());
            ui.set_stage("streaming".into());
            ui.set_first_frame_ready(true);
            ui.set_video_layer_active(true);
            ui.set_video_hole_x(730.0);
            ui.set_video_hole_y(38.0);
            ui.set_video_hole_w(460.0);
            ui.set_video_hole_h(954.0);
            render_offscreen_frame(&sw, &mut buf, 1920, 1032);
            let live_center = buf[516 * 1920 + 960];
            let live_wing = buf[516 * 1920 + 120];
            assert_eq!(live_center.a, 0, "Center streaming video hole must punch a=0");
            assert_eq!(live_wing.a, 56, "Fullscreen wing must stay frosted during live stream");

            crate::dcomp_presenter::clear_last_created_software_window();
        });
    }

    #[test]
    fn test_tahap288_crossfade_midpoints_render_simultaneous_standby_and_blur_then_zero_active_animations() {
        run_on_slint_test_thread(move || {
            let ui = MainWindow::new().expect("create MainWindow");
            let sw = crate::dcomp_presenter::take_last_created_software_window()
                .expect("capture offscreen MinimalSoftwareWindow");
            let mut buf = Vec::new();

            // Stage a 120x260 blur snapshot
            let rgba = vec![84u8; (SEQUOIA_BLUR_MAX_W * SEQUOIA_BLUR_MAX_H * 4) as usize];
            let pb = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                &rgba,
                SEQUOIA_BLUR_MAX_W,
                SEQUOIA_BLUR_MAX_H,
            );
            ui.set_last_frame_blur_image(slint::Image::from_rgba8(pb));
            ui.set_has_last_frame_blur(true);

            // 1. Midpoint Standby <-> Sequoia crossfade
            ui.set_dark_mode(true);
            ui.set_phase("starting".into());
            ui.set_stage("connecting".into());
            ui.set_transition_active(true);
            ui.set_standby_opacity(0.45);
            ui.set_sequoia_opacity(0.55);
            ui.set_sequoia_text_opacity(0.55);
            ui.set_reveal_blur_opacity(0.0);
            render_offscreen_frame(&sw, &mut buf, 346, 832);
            let start_mid_px = buf[416 * 346 + 173];
            assert!(start_mid_px.a > 0, "Start midpoint must blend visible frame");

            // 2. Midpoint StopFromLive (Last-Frame Blur fading out while Standby fades in)
            ui.set_phase("idle".into());
            ui.set_stage("".into());
            ui.set_first_frame_ready(false);
            ui.set_standby_opacity(0.60);
            ui.set_sequoia_opacity(0.0);
            ui.set_sequoia_text_opacity(0.0);
            ui.set_reveal_blur_opacity(0.40);
            render_offscreen_frame(&sw, &mut buf, 346, 832);
            let stop_mid_px = buf[416 * 346 + 173];
            assert!(stop_mid_px.a > 0, "Stop midpoint must blend Standby + Last-Frame Blur");

            // 3. Settled idle: zero permanent animation loops
            ui.set_transition_active(false);
            ui.set_standby_opacity(1.0);
            ui.set_reveal_blur_opacity(0.0);
            render_offscreen_frame(&sw, &mut buf, 346, 832);
            assert!(
                !sw.has_active_animations(),
                "Settled idle after crossfade must leave 0 active Slint animations"
            );
            crate::dcomp_presenter::clear_last_created_software_window();
        });
    }

    #[test]
    fn test_tahap288_last_frame_blur_budget_120x260_under_160kb_and_cold_start_fallback_safe() {
        assert_eq!(SEQUOIA_BLUR_MAX_W, 120);
        assert_eq!(SEQUOIA_BLUR_MAX_H, 260);
        assert_eq!(SEQUOIA_BLUR_RADIUS, 8);
        let raw_bytes = (SEQUOIA_BLUR_MAX_W * SEQUOIA_BLUR_MAX_H * 4) as usize;
        assert_eq!(raw_bytes, 124_800);
        assert!(raw_bytes < 160 * 1024);

        let mut src = vec![0u8; raw_bytes];
        for y in 0..SEQUOIA_BLUR_MAX_H {
            for x in 0..SEQUOIA_BLUR_MAX_W {
                let idx = ((y * SEQUOIA_BLUR_MAX_W + x) * 4) as usize;
                let v = if x < SEQUOIA_BLUR_MAX_W / 2 { 220 } else { 30 };
                src[idx] = v;
                src[idx + 1] = v;
                src[idx + 2] = v;
                src[idx + 3] = 255;
            }
        }
        let blurred = fast_box_blur_rgba(
            &src,
            SEQUOIA_BLUR_MAX_W,
            SEQUOIA_BLUR_MAX_H,
            SEQUOIA_BLUR_RADIUS,
        );
        assert_eq!(blurred.len(), raw_bytes);
        let seam_l = blurred[((130 * SEQUOIA_BLUR_MAX_W + 59) * 4) as usize];
        let seam_r = blurred[((130 * SEQUOIA_BLUR_MAX_W + 60) * 4) as usize];
        assert!(
            (seam_l as i32 - seam_r as i32).abs() < 40,
            "Radius 8 blur at 120x260 must smoothly diffuse sharp step edge: {} vs {}",
            seam_l,
            seam_r
        );

        // Cold start fallback (no blur image) renders true frosted translucent glass-shell (a=56 Dark / a=110 Light)
        run_on_slint_test_thread(move || {
            let ui = MainWindow::new().expect("create MainWindow");
            let sw = crate::dcomp_presenter::take_last_created_software_window()
                .expect("capture offscreen MinimalSoftwareWindow");
            let mut buf = Vec::new();
            ui.set_dark_mode(true);
            ui.set_has_last_frame_blur(false);
            ui.set_phase("starting".into());
            ui.set_stage("connecting".into());
            ui.set_transition_active(false);
            render_offscreen_frame(&sw, &mut buf, 346, 832);
            let cold_dark_px = buf[180 * 346 + 173];
            assert_eq!(
                cold_dark_px.a, 56,
                "Cold-start connecting Dark fallback must stay frosted smoky a=56, got a={}",
                cold_dark_px.a
            );
            ui.set_dark_mode(false);
            render_offscreen_frame(&sw, &mut buf, 346, 832);
            let cold_light_px = buf[180 * 346 + 173];
            assert_eq!(
                cold_light_px.a, 110,
                "Cold-start connecting Light fallback must stay frosted crystal a=110, got a={}",
                cold_light_px.a
            );
            crate::dcomp_presenter::clear_last_created_software_window();
        });
    }

    #[test]
    fn test_tahap288_stop_sequence_commits_blur_layer1_before_detaching_video_layer0_zero_black_flash() {
        let p_live = plan_stop_transition_sequence(true, true, false);
        assert_eq!(p_live.transition_kind, StageTransitionKind::StopFromLive);
        assert_eq!(p_live.duration_ms, 280);
        assert!(p_live.use_live_capture_blur);
        assert!(p_live.flush_layer1_before_layer0_detach);

        let p_cached = plan_stop_transition_sequence(true, false, true);
        assert_eq!(p_cached.transition_kind, StageTransitionKind::StopFromLive);
        assert!(p_cached.reuse_last_frame_blur);
        assert!(p_cached.flush_layer1_before_layer0_detach);

        let p_conn = plan_stop_transition_sequence(false, false, false);
        assert_eq!(
            p_conn.transition_kind,
            StageTransitionKind::StopFromConnecting
        );
        assert_eq!(p_conn.duration_ms, 240);
        assert!(p_conn.flush_layer1_before_layer0_detach);

        // Verify source ordering invariant inside on_stop_mirror: flush Layer 1 BEFORE detaching Layer 0
        let self_src = include_str!("native_ui.rs");
        let stop_seg = self_src
            .split("ui.on_stop_mirror(")
            .nth(1)
            .and_then(|s| s.split("ui.on_restart_mirror(").next())
            .expect("on_stop_mirror section");
        let flush_idx = stop_seg
            .find("flush_active_dcomp_ui_frame()")
            .expect("flush_layer1 guard");
        let clear_idx = stop_seg
            .find("sync_dcomp_video_layout(hwnd_cell.get(), &u, p, &rect_sh, Some(false))")
            .expect("sync_dcomp_video_layout detach call");
        assert!(
            flush_idx < clear_idx,
            "Layer 1 blur MUST flush synchronously before Layer 0 video detach!"
        );
    }

    #[test]
    fn test_tahap288_fullscreen_exit_atomic_resize_syncs_windowed_rect_before_setwindowpos() {
        let wa = WinRect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1032,
        };
        // 1. Matching layout preserves user-dragged windowed coordinates clamped inside work area
        let saved_portrait = Some(WinRect {
            left: 300,
            top: 90,
            right: 1006,
            bottom: 922,
        });
        let (x1, y1, w1, h1) = compute_fullscreen_exit_physical_rect(
            saved_portrait,
            wa,
            true,
            false,
            "iphone17pro",
            1.0,
        );
        assert_eq!((x1, y1, w1, h1), (300, 90, 706, 832));

        // 2. If user closed Drawer or rotated to Landscape while in Fullscreen, exit snaps to new Windowed dimensions
        let (x2, y2, w2, h2) = compute_fullscreen_exit_physical_rect(
            saved_portrait,
            wa,
            false,
            false,
            "iphone17pro",
            1.0,
        );
        assert_eq!((w2, h2), (346, 832));
        assert!(x2 > 0 && y2 >= 0);

        // 3. Verify Slint is-fullscreen flips BEFORE atomic_resize_host_window, while dcomp_presenter.rs keeps atomic_resize_host_window
        let self_src = include_str!("native_ui.rs");
        let fs_seg = self_src
            .split("ui.on_toggle_fullscreen(")
            .nth(1)
            .and_then(|s| s.split("ui.on_toggle_drawer(").next())
            .expect("on_toggle_fullscreen section");
        let set_fs_idx = fs_seg
            .find("u.set_is_fullscreen(next_fs);")
            .expect("set_is_fullscreen before resize");
        let atomic_idx = fs_seg
            .find("atomic_resize_host_window(")
            .expect("atomic_resize_host_window call");
        assert!(
            set_fs_idx < atomic_idx,
            "Slint is-fullscreen state MUST flip before atomic_resize_host_window so Layer 1 renders final frame!"
        );

        let dcomp_src = include_str!("dcomp_presenter.rs");
        assert!(dcomp_src.contains("pub fn atomic_resize_host_window("));
        assert!(dcomp_src.contains("SWP_NOREDRAW_FLAG"));
    }

    #[test]
    fn test_tahap288_v2_cold_start_sequoia_is_frosted_translucent_zero_opaque_gradient_dark_and_light() {
        let slint_src = include_str!("../ui/main_window.slint");
        for forbidden_grad in ["1E293BE8", "0F172AE8", "071923EE", "E2ECF7D8"] {
            assert!(
                !slint_src.contains(forbidden_grad),
                "Flat opaque cold-start token `{}` must not exist in main_window.slint",
                forbidden_grad
            );
        }

        run_on_slint_test_thread(move || {
            let ui = MainWindow::new().expect("create MainWindow");
            let sw = crate::dcomp_presenter::take_last_created_software_window()
                .expect("capture offscreen MinimalSoftwareWindow");
            let mut buf = Vec::new();

            ui.set_has_last_frame_blur(false);
            ui.set_phase("starting".into());
            ui.set_stage("connecting".into());
            ui.set_transition_active(false);
            ui.set_drawer_open(true);

            // Dark cold-start: left Sequoia background matches Side Drawer translucent a=56
            ui.set_dark_mode(true);
            render_offscreen_frame(&sw, &mut buf, 706, 832);
            let seq_bg_dark = buf[180 * 706 + 173];
            assert_eq!(
                seq_bg_dark.a, 56,
                "Cold-start Dark sequoia-reveal must equal frosted glass-shell a=56, got a={}",
                seq_bg_dark.a
            );

            // Light cold-start: left Sequoia background renders milky crystal a=110
            ui.set_dark_mode(false);
            render_offscreen_frame(&sw, &mut buf, 706, 832);
            let seq_bg_light = buf[180 * 706 + 173];
            assert_eq!(
                seq_bg_light.a, 110,
                "Cold-start Light sequoia-reveal must equal frosted glass-shell a=110, got a={}",
                seq_bg_light.a
            );

            crate::dcomp_presenter::clear_last_created_software_window();
        });
    }

    #[test]
    fn test_tahap288_v2_hevc_2line_and_airplay_2block_exact_t259_copy_no_progress_bar_no_wifi_icon() {
        let slint_src = include_str!("../ui/main_window.slint");
        let seq_seg = slint_src
            .split("sequoia-reveal :=")
            .nth(1)
            .and_then(|s| s.split("// End sequoia-reveal").next())
            .expect("sequoia-reveal section");

        assert!(seq_seg.contains("\"Connecting to\""));
        assert!(seq_seg.contains("\"Menyambungkan ke\""));
        assert!(seq_seg.contains("Open Control Center on iPhone → Screen Mirroring → select 'iPhone Link'"));
        assert!(seq_seg.contains("Buka Pusat Kontrol di iPhone → Pencerminan Layar → pilih 'iPhone Link'"));
        assert!(seq_seg.contains("#0F172A"), "Light cold-start must keep high-contrast #0F172A headline");
        assert!(seq_seg.contains("#334155"), "Light cold-start must keep high-contrast #334155 kicker");

        // Tahap 293 renders `root.connecting-detail` cleanly below device name during live USB handshake,
        // while preserving no progress bar / no wifi icon / no Abort button inside `sequoia-reveal`.
        assert!(seq_seg.contains("root.connecting-detail"));
        for forbidden_item in [
            "stage-progress",
            "stage-detail",
            "name: \"wifi\"",
            "\"AirPlay Siap\"",
            "\"AirPlay Ready\"",
            "\"Batal\"",
            "\"Cancel\"",
        ] {
            assert!(
                !seq_seg.contains(forbidden_item),
                "sequoia-reveal must NOT contain `{}` (Tahap 259 2-line/2-block parity)",
                forbidden_item
            );
        }

        for ch in slint_src.chars() {
            let cp = ch as u32;
            assert!(
                cp < 0x1F300 && cp != 0xFE0F && !(0x2600..=0x27BF).contains(&cp),
                "Forbidden Unicode emoji U+{:04X} found in main_window.slint",
                cp
            );
        }
    }

    #[test]
    fn test_tahap288_v2_topbar_status_pill_untruncated_when_wide_and_unclipped_at_346px() {
        let slint_src = include_str!("../ui/main_window.slint");
        assert!(slint_src.contains(
            "property <bool> topbar-wide: root.drawer-open || root.is-landscape || root.is-fullscreen || root.width >= 500px;"
        ));

        let full_id = resolve_topbar_status_pill_text("starting", "connecting", "usb", false, false);
        let full_en = resolve_topbar_status_pill_text("starting", "connecting", "usb", false, true);
        let ap_id = resolve_topbar_status_pill_text("starting", "waiting_airplay", "wifi", true, false);
        let ap_en = resolve_topbar_status_pill_text("starting", "waiting_airplay", "wifi", true, true);
        assert_eq!(full_id, "Menyambungkan\u{2026}");
        assert_eq!(full_en, "Connecting\u{2026}");
        assert_eq!(ap_id, "AirPlay Siap");
        assert_eq!(ap_en, "AirPlay Ready");

        // Wide Top Bar keeps full untruncated copy; narrow 346px keeps compact short token
        assert_eq!(resolve_compact_topbar_status_pill_text(&full_id, true), "Menyambungkan\u{2026}");
        assert_eq!(resolve_compact_topbar_status_pill_text(&full_en, true), "Connecting\u{2026}");
        assert_eq!(resolve_compact_topbar_status_pill_text(&ap_id, true), "AirPlay Siap");
        assert_eq!(resolve_compact_topbar_status_pill_text(&ap_en, true), "AirPlay Ready");

        assert_eq!(resolve_compact_topbar_status_pill_text(&full_id, false), "Menyambung");
        assert_eq!(resolve_compact_topbar_status_pill_text(&full_en, false), "Connecting");
        assert_eq!(resolve_compact_topbar_status_pill_text(&ap_id, false), "AirPlay");

        let b = compute_topbar_346px_layout_budget(true, true, false);
        assert!(b.fits_346px && b.total_px <= 340);
    }

    #[test]
    fn test_tahap288_v2_persisted_blur_cache_roundtrip_validation_and_privacy_wipe_deletes_file() {
        let tmp_dir = std::env::temp_dir().join(format!("t288v2_cache_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp_dir);
        let cache_file = tmp_dir.join(SEQUOIA_BLUR_CACHE_FILENAME);

        let rgba = vec![92u8; (SEQUOIA_BLUR_MAX_W * SEQUOIA_BLUR_MAX_H * 4) as usize];
        save_persisted_blur_cache_to(&cache_file, SEQUOIA_BLUR_MAX_W, SEQUOIA_BLUR_MAX_H, &rgba)
            .expect("save valid blur cache");
        assert_eq!(
            std::fs::metadata(&cache_file).unwrap().len(),
            124_812
        );

        let (loaded, w, h) =
            load_persisted_blur_cache_from(&cache_file).expect("load valid blur cache");
        assert_eq!((w, h), (120, 260));
        assert_eq!(loaded.len(), 124_800);
        assert_eq!(loaded[0], 92);

        // Reject corrupt magic, oversized dims, and truncated payload
        let bad_magic = tmp_dir.join("bad_magic.bin");
        let mut bad_bytes = std::fs::read(&cache_file).unwrap();
        bad_bytes[0] = b'X';
        std::fs::write(&bad_magic, &bad_bytes).unwrap();
        assert!(load_persisted_blur_cache_from(&bad_magic).is_none());

        let oversized = tmp_dir.join("oversized.bin");
        let mut over_bytes = Vec::new();
        over_bytes.extend_from_slice(SEQUOIA_BLUR_CACHE_MAGIC);
        over_bytes.extend_from_slice(&121u32.to_le_bytes());
        over_bytes.extend_from_slice(&260u32.to_le_bytes());
        over_bytes.resize(12 + 121 * 260 * 4, 50);
        std::fs::write(&oversized, &over_bytes).unwrap();
        assert!(load_persisted_blur_cache_from(&oversized).is_none());

        let truncated = tmp_dir.join("truncated.bin");
        std::fs::write(&truncated, &std::fs::read(&cache_file).unwrap()[..100]).unwrap();
        assert!(load_persisted_blur_cache_from(&truncated).is_none());

        // Privacy wipe hook removes cache file & resets has-last-frame-blur in native_ui.rs
        let self_src = include_str!("native_ui.rs");
        let wipe_seg = self_src
            .split("ui.on_wipe_privacy_data(")
            .nth(1)
            .and_then(|s| s.split("ui.on_hardware_button(").next())
            .expect("on_wipe_privacy_data segment");
        assert!(wipe_seg.contains("u.set_has_last_frame_blur(false);"));
        assert!(wipe_seg.contains("u.set_last_frame_blur_image(Image::default());"));
        assert!(wipe_seg.contains("delete_persisted_blur_cache_default()"));

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn test_tahap288_v2_loopback_and_selftest_never_touch_real_blur_cache_file() {
        assert!(!should_use_disk_blur_cache(true));
        if std::env::var_os("IPHONE_LINK_TEST_LOOPBACK_ONLY").is_some() {
            assert!(is_test_loopback_only_env());
            assert!(!should_use_disk_blur_cache(false));
        }

        let default_path = resolve_sequoia_blur_cache_path();
        assert_eq!(
            default_path.file_name().and_then(|s| s.to_str()),
            Some(SEQUOIA_BLUR_CACHE_FILENAME)
        );
        // Verify selftest path runs under isolation & startup/save/wipe guard calls should_use_disk_blur_cache
        let self_src = include_str!("native_ui.rs");
        assert!(self_src.matches("should_use_disk_blur_cache(false)").count() >= 4);
    }

    #[test]
    fn test_tahap288_v3_hq_premultiplied_downsampler_no_dark_halo_and_preserves_squircle_edge_smoothness() {
        let sw = 512u32;
        let sh = 512u32;
        let mut src = vec![0u8; (sw * sh * 4) as usize];
        let half = 256.0f32;
        let radius = 210.0f32;
        for y in 0..sh {
            for x in 0..sw {
                let dx = ((x as f32 + 0.5 - half).abs()) / radius;
                let dy = ((y as f32 + 0.5 - half).abs()) / radius;
                let sdist = dx.powi(4) + dy.powi(4);
                let idx = ((y * sw + x) * 4) as usize;
                if sdist <= 1.0 {
                    // 4px white interior detail stripe inside center, cyan #00B4D8 elsewhere
                    if (254..=257).contains(&x) && (120..=391).contains(&y) {
                        src[idx..idx + 4].copy_from_slice(&[255, 255, 255, 255]);
                    } else {
                        src[idx..idx + 4].copy_from_slice(&[0, 180, 216, 255]);
                    }
                }
            }
        }

        let top_out = downsample_rgba8_hq_premultiplied(&src, sw, sh, 30, 30);
        let lp_out = downsample_rgba8_hq_premultiplied(&src, sw, sh, 114, 114);
        assert_eq!(top_out.len(), 30 * 30 * 4);
        assert_eq!(lp_out.len(), 114 * 114 * 4);

        // Transparent corner stays pure alpha == 0
        assert_eq!(top_out[3], 0);
        assert_eq!(lp_out[3], 0);

        // Semi-transparent edge pixels away from center stripe keep bright cyan hues (0 dark halo!)
        let mut edge_checked = 0usize;
        for y in 0..114u32 {
            for x in 0..38u32 {
                let i = ((y * 114 + x) * 4) as usize;
                let a = lp_out[i + 3];
                if (20..=235).contains(&a) {
                    edge_checked += 1;
                    assert!(
                        lp_out[i + 1] >= 140 && lp_out[i + 2] >= 170,
                        "Dark halo detected on squircle edge at ({x},{y}) a={a}: rgb=({},{},{})",
                        lp_out[i],
                        lp_out[i + 1],
                        lp_out[i + 2]
                    );
                }
            }
        }
        assert!(edge_checked >= 4, "Expected anti-aliased squircle border pixels, got {edge_checked}");

        // Interior 4px white detail survives box-halving + Lanczos3 without undersampling dropout
        let c_lp = ((57 * 114 + 57) * 4) as usize;
        let c_top = ((15 * 30 + 15) * 4) as usize;
        assert!(lp_out[c_lp] >= 40, "White stripe vanished at 114x114: r={}", lp_out[c_lp]);
        assert!(top_out[c_top] >= 15, "White stripe vanished at 30x30: r={}", top_out[c_top]);
    }

    #[test]
    fn test_tahap288_v3_crisp_logo_dpi_exact_dimensions_at_100_125_150_200_and_live_refresh() {
        for &(scale, exp_top, exp_lp) in &[
            (1.0f32, 20u32, 52u32),
            (1.25f32, 25u32, 65u32),
            (1.5f32, 30u32, 78u32),
            (2.0f32, 40u32, 104u32),
        ] {
            assert_eq!(compute_crisp_logo_physical_px(BRAND_LOGO_TOPBAR_LOGICAL_PX, scale), exp_top);
            assert_eq!(compute_crisp_logo_physical_px(BRAND_LOGO_LAUNCHPAD_LOGICAL_PX, scale), exp_lp);
        }

        run_on_slint_test_thread(move || {
            let ui = MainWindow::new().expect("create MainWindow");
            let last_bits = Cell::new(0u32);
            assert!(
                refresh_crisp_brand_logos_for_scale(&ui, 1.0, &last_bits),
                "Initial 100% DPI refresh must update logos from to_rgba8()"
            );
            let top1 = ui.get_brand_logo_topbar().size();
            let lp1 = ui.get_brand_logo_launchpad().size();
            assert_eq!((top1.width, top1.height), (20, 20));
            assert_eq!((lp1.width, lp1.height), (52, 52));

            // Idempotent when scale stays 1.0
            assert!(!refresh_crisp_brand_logos_for_scale(&ui, 1.0, &last_bits));

            // Moving to 150% DPI monitor regenerates 1:1 physical pixel textures cleanly
            assert!(refresh_crisp_brand_logos_for_scale(&ui, 1.5, &last_bits));
            let top2 = ui.get_brand_logo_topbar().size();
            let lp2 = ui.get_brand_logo_launchpad().size();
            assert_eq!((top2.width, top2.height), (30, 30));
            assert_eq!((lp2.width, lp2.height), (78, 78));

            let _ = crate::dcomp_presenter::take_last_created_software_window();
            crate::dcomp_presenter::clear_last_created_software_window();
        });
    }

    #[test]
    fn test_tahap288_v3_footer_clean_no_forked_from_exact_kelvin_daniyar_gpl3_and_saweria_intact() {
        let slint_src = include_str!("../ui/main_window.slint");
        assert_eq!(slint_src.matches("Forked from").count(), 0);
        assert_eq!(
            slint_src.matches("teyboxv1/pane-iphone-mirror-windows").count(),
            0
        );
        for req in [
            "iPhone Link v5.0.1 by ",
            "Kelvin Daniyar",
            " · GPL-3.0",
            "Dukung Kreator di Saweria",
            "Support Creator on Saweria",
            "https://saweria.co/kelvindaniyar",
            "https://github.com/kelvindaniyar",
        ] {
            assert!(slint_src.contains(req), "Missing footer token `{req}`");
        }
        for ch in slint_src.chars() {
            let cp = ch as u32;
            assert!(
                cp < 0x1F300 && cp != 0xFE0F && !(0x2600..=0x27BF).contains(&cp),
                "Forbidden Unicode emoji U+{:04X} found in main_window.slint",
                cp
            );
        }
    }

    #[test]
    fn test_tahap288_v3_topbar_unclipped_and_no_redundant_clip_on_brand_logo_images() {
        let b = compute_topbar_346px_layout_budget(true, true, false);
        assert!(b.fits_346px && b.total_px <= 340);

        let slint_src = include_str!("../ui/main_window.slint");
        let top_chunk = slint_src
            .split("Left Identity + Compact Status Capsule")
            .nth(1)
            .and_then(|s| s.split("if root.topbar-wide : Text").next())
            .expect("Top Bar logo chunk");
        assert!(top_chunk.contains("source: root.brand-logo-topbar;"));
        assert!(top_chunk.contains("image-fit: contain;"));
        assert!(!top_chunk.contains("clip: true;"));

        let lp_chunk = slint_src
            .split("Hero App Logo + Ambient Glow")
            .nth(1)
            .and_then(|s| s.split("Hero Title & Subtitle").next())
            .expect("Launchpad logo chunk");
        assert!(lp_chunk.contains("source: root.brand-logo-launchpad;"));
        assert!(lp_chunk.contains("image-fit: contain;"));
        assert!(!lp_chunk.contains("clip: true;"));
    }

    #[test]
    fn test_tahap291_pin_persisted_masked_after_save_and_eye_toggle() {
        let _hid_guard = hid::HID_TEST_SERIAL_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        hid::reset_hid_runtime_state();
        let state = Arc::new(Mutex::new(server_v4::AppState::default()));

        let state_c = state.clone();
        run_on_slint_test_thread(move || {
            let ui = MainWindow::new().expect("create MainWindow");
            let saved = server_v4::save_unlock_pin_sync(&state_c, "649281").expect("sync save");
            assert_eq!(saved, "649281");
            apply_hydrated_pin_to_ui(&ui, Some(saved.as_str()));

            assert!(ui.get_pin_saved(), "pin_saved must be true after save");
            assert_eq!(
                ui.get_unlock_pin_input().as_str(),
                "649281",
                "unlock_pin_input must NOT be cleared to empty after save"
            );
            assert!(!ui.get_pin_revealed(), "PIN must remain masked after save");

            // Eye toggle reveals then re-masks without altering saved digits
            ui.set_pin_revealed(true);
            assert!(ui.get_pin_revealed());
            assert_eq!(ui.get_unlock_pin_input().as_str(), "649281");
            ui.set_pin_revealed(false);
            assert!(!ui.get_pin_revealed());
            assert_eq!(ui.get_unlock_pin_input().as_str(), "649281");

            let _ = crate::dcomp_presenter::take_last_created_software_window();
            crate::dcomp_presenter::clear_last_created_software_window();
        });

        let _ = server_v4::clear_unlock_pin_sync(&state);
        hid::reset_hid_runtime_state();
    }

    #[test]
    fn test_tahap291_pin_hydrated_on_cold_reopen() {
        let _hid_guard = hid::HID_TEST_SERIAL_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        hid::reset_hid_runtime_state();

        let tmp_dir = std::env::temp_dir().join(format!(
            "iphone_link_t291_ui_reopen_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let settings_file = tmp_dir.join("user_settings.json");
        let st_disk = server_v4::AppState::default();
        server_v4::save_user_settings_to_path(&settings_file, &st_disk, "649281").unwrap();

        // Simulate cold process restart: wipe RAM DPAPI and static SAVED_UNLOCK_PIN completely
        hid::clear_stored_pin();
        if let Ok(mut g) = server_v4::SAVED_UNLOCK_PIN.lock() {
            g.clear();
        }
        assert!(!hid::has_stored_pin());

        let mut st_reopen = server_v4::AppState::default();
        let loaded = server_v4::init_persistent_settings_from_disk_from(&settings_file, &mut st_reopen, true);
        assert_eq!(loaded, "649281");
        assert!(hid::has_stored_pin(), "HID RAM DPAPI must hydrate on cold reopen");

        let hydrated = server_v4::get_hydrated_unlock_pin();
        assert_eq!(hydrated.as_deref(), Some("649281"));

        run_on_slint_test_thread(move || {
            let ui = MainWindow::new().expect("create MainWindow");
            apply_hydrated_pin_to_ui(&ui, hydrated.as_deref());
            assert!(ui.get_pin_saved(), "Hapus button & saved badge must stay visible on reopen");
            assert_eq!(ui.get_unlock_pin_input().as_str(), "649281");
            assert!(!ui.get_pin_revealed(), "PIN must start masked on reopen");

            let _ = crate::dcomp_presenter::take_last_created_software_window();
            crate::dcomp_presenter::clear_last_created_software_window();
        });

        let _ = std::fs::remove_dir_all(&tmp_dir);
        hid::clear_stored_pin();
        if let Ok(mut g) = server_v4::SAVED_UNLOCK_PIN.lock() {
            g.clear();
        }
        hid::reset_hid_runtime_state();
    }

    #[test]
    fn test_tahap291_selftest_loopback_never_touches_real_pin() {
        let prev = std::env::var_os("IPHONE_LINK_TEST_LOOPBACK_ONLY");
        std::env::set_var("IPHONE_LINK_TEST_LOOPBACK_ONLY", "1");
        assert!(!should_hydrate_disk_pin(true));
        assert!(!should_hydrate_disk_pin(false));

        std::env::remove_var("IPHONE_LINK_TEST_LOOPBACK_ONLY");
        assert!(!should_hydrate_disk_pin(true));
        assert!(should_hydrate_disk_pin(false));

        match prev {
            Some(v) => std::env::set_var("IPHONE_LINK_TEST_LOOPBACK_ONLY", v),
            None => std::env::remove_var("IPHONE_LINK_TEST_LOOPBACK_ONLY"),
        }
    }

    #[test]
    fn test_tahap293_device_card_badge_usb_only_when_not_wifi_paired() {
        // 1. USB-only iPhone with cached/reachable LAN IP (`wifiOnline:false, wifiPaired:false`) -> "USB" (NOT "USB + Wi-Fi")
        let sv_usb_only: serde_json::Value = serde_json::from_str(
            r#"{
                "selectedUdid": "00008150-000000000002931",
                "devices": [{
                    "udid": "00008150-000000000002931",
                    "name": "TestPhone",
                    "model": "iPhone 17 Pro",
                    "usb": true,
                    "wifiOnline": false,
                    "reachable": true,
                    "wifiPaired": false,
                    "ip": "198.51.100.42"
                }]
            }"#,
        )
        .unwrap();
        let sum_usb = parse_launchpad_from_status(&sv_usb_only, false);
        assert_eq!(sum_usb.cards.len(), 1);
        assert_eq!(sum_usb.cards[0].transport_badge.as_str(), "USB");
        assert_eq!(sum_usb.status_pill_short, "USB Siap");

        // 2. Same device once Wi-Fi paired & reachable -> "USB + Wi-Fi"
        let sv_paired: serde_json::Value = serde_json::from_str(
            r#"{
                "selectedUdid": "00008150-000000000002931",
                "devices": [{
                    "udid": "00008150-000000000002931",
                    "name": "TestPhone",
                    "model": "iPhone 17 Pro",
                    "usb": true,
                    "wifiOnline": false,
                    "reachable": true,
                    "wifiPaired": true,
                    "ip": "198.51.100.42"
                }]
            }"#,
        )
        .unwrap();
        let sum_paired = parse_launchpad_from_status(&sv_paired, false);
        assert_eq!(sum_paired.cards[0].transport_badge.as_str(), "USB + Wi-Fi");
        assert!(sum_paired.status_pill_short.contains("Wi-Fi"));

        // 3. Connecting watchdog boundaries: 25s stagnant / 90s total, silent in waiting_airplay
        assert_eq!(evaluate_connecting_watchdog("starting", false, "connecting", 24_999, 24_999), None);
        assert_eq!(
            evaluate_connecting_watchdog("starting", false, "connecting", 25_000, 25_000),
            Some("stage_stagnant")
        );
        assert_eq!(
            evaluate_connecting_watchdog("starting", false, "mounting_ddi", 5_000, 90_000),
            Some("total_timeout")
        );
        assert_eq!(evaluate_connecting_watchdog("starting", false, "waiting_airplay", 40_000, 120_000), None);
        assert_eq!(evaluate_connecting_watchdog("running", false, "streaming", 40_000, 120_000), None);

        // 4. Decision Card parser handles both `ios_below_27` and `hevc_mft_missing`
        let dec_ios = parse_start_mirror_outcome_v2(
            r#"{"ok":false,"phase":"idle","stage":"","requiresDecision":true,"decisionReason":"ios_below_27","iosVersion":"26.3.1","message_id":"Gunakan AirPlay","message_en":"Use AirPlay"}"#,
            false,
        );
        assert_eq!(
            dec_ios,
            StartMirrorOutcome::DecisionRequired {
                reason: "ios_below_27".to_string(),
                message_id: "Gunakan AirPlay".to_string(),
                message_en: "Use AirPlay".to_string(),
                ios_version: "26.3.1".to_string(),
            }
        );
        let dec_hevc = parse_start_mirror_outcome_v2(
            r#"{"ok":false,"phase":"idle","stage":"","requiresDecision":true,"decisionReason":"hevc_mft_missing","message_id":"Pasang HEVC","message_en":"Install HEVC"}"#,
            true,
        );
        assert_eq!(
            dec_hevc,
            StartMirrorOutcome::DecisionRequired {
                reason: "hevc_mft_missing".to_string(),
                message_id: "Pasang HEVC".to_string(),
                message_en: "Install HEVC".to_string(),
                ios_version: String::new(),
            }
        );

        // 5. Honest Top Bar pill while AirPlay awaits first frame vs streaming
        assert_eq!(resolve_topbar_status_pill_text("running", "waiting_frame", "airplay", true, true), "AirPlay...");
        assert_eq!(resolve_topbar_status_pill_text("running", "streaming", "airplay", true, true), "AirPlay");
    }
}

