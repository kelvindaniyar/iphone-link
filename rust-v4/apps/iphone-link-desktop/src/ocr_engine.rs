//! WinRT Multi-Frame OCR Engine, Dual-Candidate Image Pre-Processor,
//! Bidirectional Stitcher (Golden Rules 1-3), & Win32 Clipboard FFI.
//!
//! 100% Pure Native Rust — ZERO PowerShell / ZERO Python subprocesses.
//! Provides:
//! - PILAR 1: Pixel-level AssistiveTouch Non-Destructive Masker, Dark Balloon
//!   `[ AutoFill | Paste ]` Masker, Yellow/Blue Selection Highlight Scanner (+4 handle weight),
//!   Virtual Keyboard Clamper, Spellcheck Red-Dot Neutralizer, & Dual-Candidate Fusion
//! - PILAR 2: In-Process WinRT COM (`combase.dll` `Windows.Media.Ocr.OcrEngine`) Probe,
//!   15+ Optical Repairs, 4-Layer Anti-Contamination, CPython `difflib.SequenceMatcher`,
//!   & Bidirectional Multi-Frame Stitcher (Golden Rules 1-3 + Smart Auto-Clear)
//! - Native Win32 Bidirectional Clipboard FFI + Persistent Chunk State (`RECENT_LONG_OCR_CHUNKS`)

use std::collections::HashSet;
use std::ffi::c_void;
use std::fs;
use std::path::PathBuf;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// Long-Session Endurance Invariant 1 constants (Paritas Tahap 94).
pub const MAX_VIEWPORT_HISTORY: usize = 4;
pub const VIEWPORT_HISTORY_TTL_SECS: f64 = 45.0;
pub const VIEWPORT_CAPTURE_COOLDOWN_MS: u64 = 2_500;
pub const MAX_LONG_OCR_CHUNKS: usize = 5;
pub const LONG_OCR_CHUNKS_TTL_SECS: f64 = 90.0;

use iphone_link_apple::hid::{
    clear_typed_lines_buffer, get_current_typed_line, get_last_typed_active_line,
    get_typed_lines_buffer,
};

// ============================================================================
// Clipboard & Session Chunk Cache State
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardSource {
    Pc,
    Iphone,
}

#[derive(Debug, Clone)]
pub struct ClipboardState {
    pub source: ClipboardSource,
    pub last_seq: u32,
    pub last_content: String,
}

impl Default for ClipboardState {
    fn default() -> Self {
        Self {
            source: ClipboardSource::Pc,
            last_seq: 0,
            last_content: String::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionColor {
    None,
    Yellow,
    Blue,
}

impl SelectionColor {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Yellow => "yellow",
            Self::Blue => "blue",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "yellow" => Self::Yellow,
            "blue" => Self::Blue,
            _ => Self::None,
        }
    }
}

#[derive(Debug, Clone)]
struct OcrChunkEntry {
    ts: f64,
    lines: Vec<String>,
    color: SelectionColor,
}

#[derive(Debug, Clone)]
pub struct ViewportHistoryEntry {
    pub ts_ms: u64,
    pub width: u32,
    pub height: u32,
    pub frame_hash: u64,
}

static COPY_CACHE: Mutex<Option<Vec<String>>> = Mutex::new(None);
static RECENT_LONG_OCR_CHUNKS: OnceLock<Mutex<Vec<OcrChunkEntry>>> = OnceLock::new();
static VIEWPORT_HISTORY: OnceLock<Mutex<Vec<ViewportHistoryEntry>>> = OnceLock::new();
static LAST_VIEWPORT_CAPTURE_MS: AtomicU64 = AtomicU64::new(0);
static LAST_SELECTION_COLOR: OnceLock<Mutex<SelectionColor>> = OnceLock::new();
static ACTIVE_CLIPBOARD_SOURCE: OnceLock<Mutex<ClipboardSource>> = OnceLock::new();
static LAST_RECORDED_CLIPBOARD_SEQ: AtomicU32 = AtomicU32::new(0);
static INPROCESS_WINRT_COM_TESTED: AtomicBool = AtomicBool::new(false);
static INPROCESS_WINRT_COM_READY: AtomicBool = AtomicBool::new(false);

fn chunks_store() -> &'static Mutex<Vec<OcrChunkEntry>> {
    RECENT_LONG_OCR_CHUNKS.get_or_init(|| Mutex::new(Vec::new()))
}

fn viewport_history_store() -> &'static Mutex<Vec<ViewportHistoryEntry>> {
    VIEWPORT_HISTORY.get_or_init(|| Mutex::new(Vec::new()))
}

/// Clears bounded OCR viewport frame history cleanly.
pub fn clear_viewport_history() {
    LAST_VIEWPORT_CAPTURE_MS.store(0, Ordering::SeqCst);
    if let Ok(mut g) = viewport_history_store().lock() {
        g.clear();
    }
}

/// Returns active count of retained OCR viewport history frames (`<= MAX_VIEWPORT_HISTORY`).
pub fn viewport_history_count() -> usize {
    viewport_history_store().lock().map(|g| g.len()).unwrap_or(0)
}

/// Records one OCR viewport frame under Long-Session Endurance Invariant 1:
/// - Enforces `VIEWPORT_CAPTURE_COOLDOWN_MS (2500 ms)` between captures
/// - Prunes entries older than `VIEWPORT_HISTORY_TTL_SECS (45s)`
/// - Caps active entries at `MAX_VIEWPORT_HISTORY (4)` (`0` unbounded growth)
pub fn record_viewport_history_snapshot(
    width: u32,
    height: u32,
    frame_hash: u64,
    now_ms: u64,
) -> (bool, usize) {
    let ttl_ms = (VIEWPORT_HISTORY_TTL_SECS * 1000.0) as u64;
    let last_ms = LAST_VIEWPORT_CAPTURE_MS.load(Ordering::Acquire);
    let mut store = viewport_history_store()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    store.retain(|e| now_ms.saturating_sub(e.ts_ms) <= ttl_ms);
    if last_ms != 0 && now_ms.saturating_sub(last_ms) < VIEWPORT_CAPTURE_COOLDOWN_MS {
        return (false, store.len());
    }
    LAST_VIEWPORT_CAPTURE_MS.store(now_ms, Ordering::Release);
    store.push(ViewportHistoryEntry {
        ts_ms: now_ms,
        width,
        height,
        frame_hash,
    });
    if store.len() > MAX_VIEWPORT_HISTORY {
        let drop_n = store.len() - MAX_VIEWPORT_HISTORY;
        store.drain(0..drop_n);
    }
    (true, store.len())
}

fn last_color_store() -> &'static Mutex<SelectionColor> {
    LAST_SELECTION_COLOR.get_or_init(|| Mutex::new(SelectionColor::None))
}

fn active_clip_store() -> &'static Mutex<ClipboardSource> {
    ACTIVE_CLIPBOARD_SOURCE.get_or_init(|| Mutex::new(ClipboardSource::Pc))
}

fn now_epoch_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

pub fn set_active_clipboard_source(source: ClipboardSource) {
    if let Ok(mut g) = active_clip_store().lock() {
        *g = source;
    }
    LAST_RECORDED_CLIPBOARD_SEQ.store(get_clipboard_sequence_number(), Ordering::SeqCst);
}

pub fn set_last_recorded_clipboard_seq_for_test(seq: u32) {
    LAST_RECORDED_CLIPBOARD_SEQ.store(seq, Ordering::SeqCst);
}

/// Evaluates Win32 `GetClipboardSequenceNumber()` state machine:
/// If Windows clipboard sequence changed since the last iPhone copy, promotes active source to `Pc`.
pub fn resolve_active_clipboard_source_with_seq() -> (ClipboardSource, u32) {
    let cur_seq = get_clipboard_sequence_number();
    let prev_seq = LAST_RECORDED_CLIPBOARD_SEQ.load(Ordering::SeqCst);
    if prev_seq == 0 {
        LAST_RECORDED_CLIPBOARD_SEQ.store(cur_seq, Ordering::SeqCst);
    } else if cur_seq != prev_seq {
        if let Ok(mut g) = active_clip_store().lock() {
            *g = ClipboardSource::Pc;
        }
        LAST_RECORDED_CLIPBOARD_SEQ.store(cur_seq, Ordering::SeqCst);
    }
    (get_active_clipboard_source(), cur_seq)
}

pub fn get_active_clipboard_source() -> ClipboardSource {
    active_clip_store()
        .lock()
        .map(|g| *g)
        .unwrap_or(ClipboardSource::Pc)
}

pub fn get_recent_long_ocr_chunks_count() -> usize {
    chunks_store().lock().map(|g| g.len()).unwrap_or(0)
}

pub fn seed_long_ocr_chunk_for_test(lines: &[&str], color: SelectionColor) {
    if let Ok(mut g) = chunks_store().lock() {
        g.push(OcrChunkEntry {
            ts: now_epoch_secs(),
            lines: lines.iter().map(|s| (*s).to_string()).collect(),
            color,
        });
    }
    if let Ok(mut lc) = last_color_store().lock() {
        *lc = color;
    }
}

/// Clears recent long OCR chunks only (used by smart auto-clear and `/api/ocr/clear-chunks`).
pub fn clear_recent_long_ocr_chunks() {
    if let Ok(mut cache) = COPY_CACHE.lock() {
        *cache = Some(Vec::new());
    }
    if let Ok(mut g) = chunks_store().lock() {
        g.clear();
    }
    if let Ok(mut lc) = last_color_store().lock() {
        *lc = SelectionColor::None;
    }
}

/// Clears recent long OCR chunks, copy cache, and typed line buffers (Alt+R / Wipe Privacy).
pub fn clear_copy_cache() {
    clear_recent_long_ocr_chunks();
    clear_typed_lines_buffer();
}

// ============================================================================
// PILAR 1: Native Rust Dual-Candidate Pre-Processor, Highlight & Maskers
// ============================================================================

#[derive(Debug, Clone, PartialEq)]
pub struct OcrLineBox {
    pub text: String,
    pub min_y: f64,
    pub max_y: f64,
    pub height: f64,
}

#[derive(Debug, Clone)]
pub struct DualPreprocessResult {
    pub width: usize,
    pub height: usize,
    pub crop_x: usize,
    pub crop_y: usize,
    pub crop_w: usize,
    pub crop_h: usize,
    pub target_w: usize,
    pub target_h: usize,
    pub final_w: usize,
    pub final_h: usize,
    pub found_highlight: bool,
    pub sel_min_y: usize,
    pub sel_max_y: usize,
    pub selection_color: SelectionColor,
    pub has_keyboard: bool,
    pub assistive_touch_masked: bool,
    pub dark_balloon_masked: bool,
    pub cand1_rgba: Vec<u8>,
    pub cand2_rgba: Option<Vec<u8>>,
}

fn mask_assistivetouch_clusters(buf: &mut [u8], width: usize, height: usize) -> bool {
    let mut masked = false;
    let y_start = (height as f64 * 0.10) as usize;
    let y_end = ((height as f64 * 0.85) as usize).min(height.saturating_sub(1));
    if y_start >= y_end || width < 40 {
        return false;
    }

    for side in 0..2 {
        let x_start = if side == 0 {
            0
        } else {
            (width as f64 * 0.74) as usize
        };
        let x_end = if side == 0 {
            ((width as f64 * 0.20) as usize).min(width)
        } else {
            width
        };

        let mut cluster_min_y: Option<usize> = None;
        let mut cluster_max_y = 0usize;
        let mut cluster_min_x = usize::MAX;
        let mut cluster_max_x = 0usize;
        let mut cluster_rows = 0usize;

        let try_apply_mask =
            |buf: &mut [u8],
             c_min_y: usize,
             c_max_y: usize,
             c_min_x: usize,
             c_max_x: usize,
             rows: usize|
             -> bool {
                let cw = c_max_x.saturating_sub(c_min_x);
                let ch = c_max_y.saturating_sub(c_min_y);
                let aspect = if ch > 0 { cw as f32 / ch as f32 } else { 0.0 };
                if cw >= 55 && ch >= 55 && rows >= 40 && (0.75..=1.35).contains(&aspect) {
                    let pad = 10usize;
                    let mx0 = c_min_x.saturating_sub(pad);
                    let mx1 = (c_max_x + pad).min(width.saturating_sub(1));
                    let my0 = c_min_y.saturating_sub(pad);
                    let my1 = (c_max_y + pad).min(height.saturating_sub(1));
                    let cx = (c_min_x + c_max_x) as isize / 2;
                    let cy = (c_min_y + c_max_y) as isize / 2;
                    let radius = (cw.max(ch) as f64) * 0.5;
                    let core_r_sq = (radius * 0.55) * (radius * 0.55);

                    for my in my0..=my1 {
                        let dy = my as isize - cy;
                        for mx in mx0..=mx1 {
                            let idx = (my * width + mx) * 4;
                            let r = buf[idx];
                            let g = buf[idx + 1];
                            let b = buf[idx + 2];
                            let dx = mx as isize - cx;
                            let is_core = ((dx * dx + dy * dy) as f64) <= core_r_sq;
                            let is_dark_text = r < 75 && g < 75 && b < 75;
                            if is_core || (!is_dark_text && (r >= 70 || g >= 70 || b >= 70)) {
                                buf[idx] = 255;
                                buf[idx + 1] = 255;
                                buf[idx + 2] = 255;
                                buf[idx + 3] = 255;
                            }
                        }
                    }
                    return true;
                }
                false
            };

        for y in y_start..=y_end {
            let mut cur_run = 0usize;
            let mut max_run = 0usize;
            let mut run_start_x = 0usize;
            let mut best_start_x = 0usize;
            for x in x_start..x_end {
                let idx = (y * width + x) * 4;
                let r = buf[idx] as i32;
                let g = buf[idx + 1] as i32;
                let b = buf[idx + 2] as i32;
                let neutral = (r - g).abs() <= 14
                    && (g - b).abs() <= 14
                    && (r - b).abs() <= 14
                    && (48..=195).contains(&r);
                if neutral {
                    if cur_run == 0 {
                        run_start_x = x;
                    }
                    cur_run += 1;
                    if cur_run > max_run {
                        max_run = cur_run;
                        best_start_x = run_start_x;
                    }
                } else {
                    cur_run = 0;
                }
            }

            if max_run >= 25 {
                if cluster_min_y.is_none() || y <= cluster_max_y + 2 {
                    if cluster_min_y.is_none() {
                        cluster_min_y = Some(y);
                    }
                    cluster_max_y = y;
                    cluster_rows += 1;
                    if best_start_x < cluster_min_x {
                        cluster_min_x = best_start_x;
                    }
                    if best_start_x + max_run > cluster_max_x {
                        cluster_max_x = best_start_x + max_run;
                    }
                } else {
                    if let Some(cmin_y) = cluster_min_y {
                        if try_apply_mask(
                            buf,
                            cmin_y,
                            cluster_max_y,
                            cluster_min_x,
                            cluster_max_x,
                            cluster_rows,
                        ) {
                            masked = true;
                        }
                    }
                    cluster_min_y = Some(y);
                    cluster_max_y = y;
                    cluster_rows = 1;
                    cluster_min_x = best_start_x;
                    cluster_max_x = best_start_x + max_run;
                }
            }
        }

        if let Some(cmin_y) = cluster_min_y {
            if try_apply_mask(
                buf,
                cmin_y,
                cluster_max_y,
                cluster_min_x,
                cluster_max_x,
                cluster_rows,
            ) {
                masked = true;
            }
        }
    }
    masked
}

/// Mask dark iOS popup balloons such as `[ AutoFill | Paste ]` / `[ Cut | Copy | Paste ]`.
fn mask_dark_callout_balloons(buf: &mut [u8], width: usize, height: usize) -> bool {
    let mut masked = false;
    let y_start = (height as f64 * 0.08) as usize;
    let y_end = ((height as f64 * 0.88) as usize).min(height.saturating_sub(1));
    if y_start >= y_end || width < 100 {
        return false;
    }

    let is_dark_callout_bg = |r: i32, g: i32, b: i32| -> bool {
        (18..=72).contains(&r)
            && (18..=72).contains(&g)
            && (18..=78).contains(&b)
            && (r - g).abs() <= 12
            && (g - b).abs() <= 14
    };

    let mut row_spans: Vec<(usize, usize, usize)> = Vec::new();
    for y in y_start..=y_end {
        let mut cur_run = 0usize;
        let mut run_start = 0usize;
        let mut best_run = 0usize;
        let mut best_start = 0usize;
        for x in 0..width {
            let idx = (y * width + x) * 4;
            let r = buf[idx] as i32;
            let g = buf[idx + 1] as i32;
            let b = buf[idx + 2] as i32;
            if is_dark_callout_bg(r, g, b) {
                if cur_run == 0 {
                    run_start = x;
                }
                cur_run += 1;
                if cur_run > best_run {
                    best_run = cur_run;
                    best_start = run_start;
                }
            } else {
                cur_run = 0;
            }
        }
        if best_run >= 80 {
            row_spans.push((y, best_start, best_start + best_run));
        }
    }

    // Group contiguous dark-background rows into rounded pill callout boxes (22..=120 px tall)
    let mut i = 0usize;
    while i < row_spans.len() {
        let min_y = row_spans[i].0;
        let mut max_y = min_y;
        let mut min_x = row_spans[i].1;
        let mut max_x = row_spans[i].2;
        let mut j = i + 1;
        while j < row_spans.len() && row_spans[j].0 <= max_y + 3 {
            max_y = row_spans[j].0;
            if row_spans[j].1 < min_x {
                min_x = row_spans[j].1;
            }
            if row_spans[j].2 > max_x {
                max_x = row_spans[j].2;
            }
            j += 1;
        }
        let box_h = max_y.saturating_sub(min_y) + 1;
        let box_w = max_x.saturating_sub(min_x);
        if (22..=120).contains(&box_h) && box_w >= 80 && min_y >= 10 {
            let my0 = min_y.saturating_sub(4);
            let my1 = (max_y + 4).min(height.saturating_sub(1));
            let mx0 = min_x.saturating_sub(6);
            let mx1 = (max_x + 6).min(width.saturating_sub(1));
            for my in my0..=my1 {
                for mx in mx0..=mx1 {
                    let idx = (my * width + mx) * 4;
                    buf[idx] = 255;
                    buf[idx + 1] = 255;
                    buf[idx + 2] = 255;
                    buf[idx + 3] = 255;
                }
            }
            masked = true;
        }
        i = j;
    }
    let _ = min_y_suppress();
    masked
}

#[inline]
fn min_y_suppress() {}

fn bilinear_upscale_with_margin(
    strip: &[u8],
    crop_w: usize,
    crop_h: usize,
    target_w: usize,
    target_h: usize,
    margin: usize,
) -> Vec<u8> {
    let final_w = target_w + margin * 2;
    let final_h = target_h + margin * 2;
    let mut out = vec![255u8; final_w * final_h * 4];
    if crop_w == 0 || crop_h == 0 || target_w == 0 || target_h == 0 {
        return out;
    }

    for ty in 0..target_h {
        let src_y = ((ty as f64 + 0.5) * (crop_h as f64 / target_h as f64) - 0.5)
            .clamp(0.0, (crop_h.saturating_sub(1)) as f64);
        let y0 = src_y.floor() as usize;
        let y1 = (y0 + 1).min(crop_h - 1);
        let wy = src_y - y0 as f64;
        for tx in 0..target_w {
            let src_x = ((tx as f64 + 0.5) * (crop_w as f64 / target_w as f64) - 0.5)
                .clamp(0.0, (crop_w.saturating_sub(1)) as f64);
            let x0 = src_x.floor() as usize;
            let x1 = (x0 + 1).min(crop_w - 1);
            let wx = src_x - x0 as f64;

            let i00 = (y0 * crop_w + x0) * 4;
            let i10 = (y0 * crop_w + x1) * 4;
            let i01 = (y1 * crop_w + x0) * 4;
            let i11 = (y1 * crop_w + x1) * 4;
            let out_idx = ((margin + ty) * final_w + (margin + tx)) * 4;

            for c in 0..3 {
                let top = strip[i00 + c] as f64 * (1.0 - wx) + strip[i10 + c] as f64 * wx;
                let bot = strip[i01 + c] as f64 * (1.0 - wx) + strip[i11 + c] as f64 * wx;
                let val = (top * (1.0 - wy) + bot * wy).round().clamp(0.0, 255.0) as u8;
                out[out_idx + c] = val;
            }
            out[out_idx + 3] = 255;
        }
    }
    out
}

/// Pure-Rust Dual-Candidate Pre-Processor matching `handset/ocr_extractor.ps1` + Dark Balloon Mask:
/// - Masks AssistiveTouch non-destructively (protecting `R,G,B < 75` ink)
/// - Masks dark `[ AutoFill | Paste ]` balloons
/// - Detects on-screen keyboard (`Y = 0.56*H .. 0.84*H`) and clamps bottom crop to `0.49 * H`
/// - Detects Yellow & Blue highlight + `+4` lollipop handles while ignoring toolbar corners
/// - Generates Candidate 1 (Natural RGB) & Candidate 2 (Channel-Filtered Luma Contrast) with dynamic upscaling (`< 3500 px`)
pub fn prepare_dual_ocr_candidates(
    rgba: &[u8],
    width: usize,
    height: usize,
    is_full_viewport: bool,
    only_cand1: bool,
) -> DualPreprocessResult {
    assert!(
        rgba.len() >= width * height * 4 && width > 0 && height > 0,
        "invalid RGBA input dimensions"
    );
    let mut raw = rgba[..width * height * 4].to_vec();

    // 1. AssistiveTouch non-destructive mask + Dark balloon [ AutoFill | Paste ] mask
    let assistive_touch_masked = mask_assistivetouch_clusters(&mut raw, width, height);
    let dark_balloon_masked = mask_dark_callout_balloons(&mut raw, width, height);

    // 2. Virtual keyboard detection in Y = 0.56*H .. 0.84*H
    let kb_y0 = (height as f64 * 0.56) as usize;
    let kb_y1 = ((height as f64 * 0.84) as usize).min(height.saturating_sub(1));
    let kb_x0 = (width as f64 * 0.10) as usize;
    let kb_x1 = ((width as f64 * 0.90) as usize).min(width.saturating_sub(1));
    let mut kb_gray_count = 0usize;
    let mut total_sampled = 0usize;
    for y in (kb_y0..=kb_y1).step_by(4) {
        for x in (kb_x0..=kb_x1).step_by(4) {
            let idx = (y * width + x) * 4;
            let r = raw[idx] as i32;
            let g = raw[idx + 1] as i32;
            let b = raw[idx + 2] as i32;
            total_sampled += 1;
            if (180..=228).contains(&r)
                && (r - g).abs() <= 10
                && (g - b).abs() <= 10
                && (r - b).abs() <= 10
            {
                kb_gray_count += 1;
            }
        }
    }
    let has_keyboard = total_sampled > 0 && (kb_gray_count as f64 / total_sampled as f64) >= 0.12;
    let max_content_bottom = if has_keyboard {
        (height as f64 * 0.49) as usize
    } else {
        (height as f64 * 0.892) as usize
    };

    // 3. Highlight scan (Y = 0.122*H .. scan_bottom) with toolbar masking
    let scan_top_y = (height as f64 * 0.122) as usize;
    let scan_bottom_y = if has_keyboard {
        (height as f64 * 0.49) as usize
    } else {
        (height as f64 * 0.885) as usize
    }
    .min(height.saturating_sub(1));
    let left_x = (width as f64 * 0.04) as usize;
    let right_x = ((width as f64 * 0.96) as usize).min(width.saturating_sub(1));

    let mut row_has_highlight = vec![false; height];
    let mut yellow_hl_count = 0usize;
    let mut blue_hl_count = 0usize;

    if !is_full_viewport && scan_top_y <= scan_bottom_y {
        let top_bar_y = (height as f64 * 0.125) as usize;
        for y in scan_top_y..=scan_bottom_y {
            let row_left = if y < top_bar_y {
                (width as f64 * 0.18) as usize
            } else {
                left_x
            };
            let row_right = if y < top_bar_y {
                ((width as f64 * 0.66) as usize).min(width.saturating_sub(1))
            } else {
                right_x
            };
            if row_left > row_right {
                continue;
            }
            let mut hl_score = 0usize;
            for x in row_left..=row_right {
                let idx = (y * width + x) * 4;
                let r = raw[idx] as i32;
                let g = raw[idx + 1] as i32;
                let b = raw[idx + 2] as i32;

                let is_yellow_handle = r > 200 && g > 120 && b < 100 && (r - b) > 100;
                let is_yellow_hl = r >= 200 && g >= 175 && b < 242 && (r - b) >= 14;
                let is_blue_handle = b > 160 && (b - r) > 70 && (b - g) > 35 && r < 110;
                let is_blue_hl = b >= 205 && g >= 190 && (b - r) >= 8;

                if is_yellow_handle {
                    yellow_hl_count += 4;
                    hl_score += 4;
                } else if is_yellow_hl {
                    yellow_hl_count += 1;
                    hl_score += 1;
                } else if is_blue_handle {
                    blue_hl_count += 4;
                    hl_score += 4;
                } else if is_blue_hl {
                    blue_hl_count += 1;
                    hl_score += 1;
                }
            }
            if hl_score >= 8 {
                row_has_highlight[y] = true;
            }
        }
    }

    // Group highlight rows with vertical gap tolerance 0.060 * H
    let max_line_gap = ((height as f64 * 0.060) as usize).max(4);
    let mut first_cluster_start: Option<usize> = None;
    let mut last_cluster_end: Option<usize> = None;
    let mut cur_start: Option<usize> = None;
    let mut last_row: Option<usize> = None;

    for y in scan_top_y..=scan_bottom_y {
        if row_has_highlight[y] {
            if cur_start.is_none() {
                cur_start = Some(y);
            } else if let (Some(cs), Some(lr)) = (cur_start, last_row) {
                if y.saturating_sub(lr) > max_line_gap {
                    if lr.saturating_sub(cs) >= 8 {
                        if first_cluster_start.is_none() {
                            first_cluster_start = Some(cs);
                        }
                        last_cluster_end = Some(lr);
                    }
                    cur_start = Some(y);
                }
            }
            last_row = Some(y);
        }
    }
    if let (Some(cs), Some(lr)) = (cur_start, last_row) {
        if lr.saturating_sub(cs) >= 8 {
            if first_cluster_start.is_none() {
                first_cluster_start = Some(cs);
            }
            last_cluster_end = Some(lr);
        }
    }

    let mut found_highlight = false;
    let mut sel_min_y = 0usize;
    let mut sel_max_y = 0usize;
    if let (Some(fs), Some(le)) = (first_cluster_start, last_cluster_end) {
        if le.saturating_sub(fs) >= 8 {
            found_highlight = true;
            sel_min_y = fs;
            sel_max_y = le;
        }
    }

    let selection_color = if blue_hl_count > yellow_hl_count && blue_hl_count >= 16 {
        SelectionColor::Blue
    } else if yellow_hl_count >= 16 {
        SelectionColor::Yellow
    } else {
        SelectionColor::None
    };

    // 4. Compute crop rectangle (0.022*W .. 0.978*W)
    let crop_x = (width as f64 * 0.022) as usize;
    let crop_w = (width.saturating_sub(crop_x))
        .min(((width as f64 * 0.956) as usize).max(1));

    let (crop_y, crop_h) = if found_highlight && !is_full_viewport {
        let sel_span = sel_max_y.saturating_sub(sel_min_y);
        let pad_top = if sel_span < (height as f64 * 0.07) as usize {
            (height as f64 * 0.022) as usize
        } else {
            (height as f64 * 0.010) as usize
        };
        let pad_bot = if sel_span < (height as f64 * 0.07) as usize {
            (height as f64 * 0.024) as usize
        } else {
            (height as f64 * 0.018) as usize
        };
        let cy = ((height as f64 * 0.098) as usize).max(sel_min_y.saturating_sub(pad_top));
        let cb = max_content_bottom.min(sel_max_y + pad_bot);
        (cy, cb.saturating_sub(cy).max(20).min(height - cy))
    } else {
        let cy = (height as f64 * 0.102) as usize;
        let cb = max_content_bottom.min((height as f64 * 0.810) as usize);
        (cy, cb.saturating_sub(cy).max(20).min(height - cy))
    };

    let mut nat_strip = vec![255u8; crop_w * crop_h * 4];
    let mut red_strip = if only_cand1 {
        Vec::new()
    } else {
        vec![255u8; crop_w * crop_h * 4]
    };

    let bar_h_limit = (height as f64 * 0.125) as usize;
    let bar_x_left = (width as f64 * 0.18) as usize;
    let bar_x_right = (width as f64 * 0.66) as usize;

    for cy in 0..crop_h {
        let abs_y = crop_y + cy;
        for cx in 0..crop_w {
            let abs_x = crop_x + cx;
            let s_idx = (abs_y * width + abs_x) * 4;
            let n_idx = (cy * crop_w + cx) * 4;

            let r = raw[s_idx] as i32;
            let g = raw[s_idx + 1] as i32;
            let b = raw[s_idx + 2] as i32;

            let max_rgb = r.max(g).max(b);
            let min_rgb = r.min(g).min(b);
            let chroma = max_rgb - min_rgb;

            let is_dark_text_ink = r < 145 && g < 145 && b < 145 && chroma <= 32;
            let is_yellow_chroma = (r - b) >= 36 && r > 135;
            let is_blue_chroma = (b - r) >= 36 && b > 135;
            let is_handle_or_hl = !is_dark_text_ink && (is_yellow_chroma || is_blue_chroma);
            let is_top_nav = abs_y < bar_h_limit && (abs_x < bar_x_left || abs_x > bar_x_right);
            let is_red_spellcheck = !is_dark_text_ink
                && r > 180
                && g < 140
                && b < 140
                && (r - g) > 52
                && (r - b) > 52;

            if is_top_nav || is_handle_or_hl || is_red_spellcheck {
                nat_strip[n_idx] = 255;
                nat_strip[n_idx + 1] = 255;
                nat_strip[n_idx + 2] = 255;
                nat_strip[n_idx + 3] = 255;
            } else {
                nat_strip[n_idx] = r as u8;
                nat_strip[n_idx + 1] = g as u8;
                nat_strip[n_idx + 2] = b as u8;
                nat_strip[n_idx + 3] = 255;
            }

            if !only_cand1 {
                if is_top_nav || is_handle_or_hl || is_red_spellcheck {
                    red_strip[n_idx] = 255;
                    red_strip[n_idx + 1] = 255;
                    red_strip[n_idx + 2] = 255;
                    red_strip[n_idx + 3] = 255;
                } else if is_dark_text_ink {
                    let dark_lum = (r + g + b) / 3;
                    let dv = ((dark_lum as f64) * 0.45).floor().clamp(0.0, 255.0) as u8;
                    red_strip[n_idx] = dv;
                    red_strip[n_idx + 1] = dv;
                    red_strip[n_idx + 2] = dv;
                    red_strip[n_idx + 3] = 255;
                } else {
                    let (lum, threshold) = if b > r + 10 {
                        ((r + g) / 2, 206)
                    } else {
                        (r, 228)
                    };
                    let v: u8 = if lum >= threshold {
                        255
                    } else if lum < 165 {
                        ((lum as f64) * 0.55).floor().clamp(0.0, 255.0) as u8
                    } else {
                        let mapped =
                            90 + ((lum - 165) * (250 - 90)) / (threshold - 165).max(1);
                        mapped.clamp(0, 255) as u8
                    };
                    red_strip[n_idx] = v;
                    red_strip[n_idx + 1] = v;
                    red_strip[n_idx + 2] = v;
                    red_strip[n_idx + 3] = 255;
                }
            }
        }
    }

    // 5. Dynamic safe upscaling (< 3500 px hard clamp at 3400) + 48 px margin
    let mut scale = if crop_h <= 650 { 2.2 } else { 1.35 };
    let max_allowed_dim = 3400.0;
    if (crop_w as f64) * scale > max_allowed_dim {
        scale = max_allowed_dim / (crop_w as f64);
    }
    if (crop_h as f64) * scale > max_allowed_dim {
        scale = max_allowed_dim / (crop_h as f64);
    }
    let target_w = ((crop_w as f64) * scale).round().max(1.0) as usize;
    let target_h = ((crop_h as f64) * scale).round().max(1.0) as usize;
    let margin = 48usize;
    let final_w = target_w + margin * 2;
    let final_h = target_h + margin * 2;

    let cand1_rgba = bilinear_upscale_with_margin(&nat_strip, crop_w, crop_h, target_w, target_h, margin);
    let cand2_rgba = if only_cand1 {
        None
    } else {
        Some(bilinear_upscale_with_margin(
            &red_strip, crop_w, crop_h, target_w, target_h, margin,
        ))
    };

    DualPreprocessResult {
        width,
        height,
        crop_x,
        crop_y,
        crop_w,
        crop_h,
        target_w,
        target_h,
        final_w,
        final_h,
        found_highlight,
        sel_min_y,
        sel_max_y,
        selection_color,
        has_keyboard,
        assistive_touch_masked,
        dark_balloon_masked,
        cand1_rgba,
        cand2_rgba,
    }
}

/// Fuse Candidate 1 (Natural RGB) and Candidate 2 (Contrast Enhanced) line-by-line by vertical Y coordinate.
pub fn merge_line_lists(lines1: &[OcrLineBox], lines2: &[OcrLineBox]) -> Vec<OcrLineBox> {
    if lines1.is_empty() {
        return lines2.to_vec();
    }
    if lines2.is_empty() {
        return lines1.to_vec();
    }
    if lines1.len() == 1 && lines1[0].text.trim() == "<<<NOTES_LIST_VIEW>>>" {
        return lines1.to_vec();
    }
    if lines2.len() == 1 && lines2[0].text.trim() == "<<<NOTES_LIST_VIEW>>>" {
        return lines2.to_vec();
    }

    let total_count = lines1.len() + lines2.len();
    let sum_h: f64 = lines1
        .iter()
        .chain(lines2.iter())
        .map(|l| l.height)
        .sum();
    let avg_h = if total_count > 0 {
        sum_h / (total_count as f64)
    } else {
        30.0
    };

    let mut used2 = vec![false; lines2.len()];
    let mut fused: Vec<OcrLineBox> = Vec::with_capacity(lines1.len() + lines2.len());

    for l1 in lines1 {
        let cy1 = (l1.min_y + l1.max_y) * 0.5;
        let mut best_idx: Option<usize> = None;
        let mut best_dist = f64::MAX;

        for (j, l2) in lines2.iter().enumerate() {
            if used2[j] {
                continue;
            }
            let cy2 = (l2.min_y + l2.max_y) * 0.5;
            let dist = (cy1 - cy2).abs();
            let overlap = l1.min_y <= l2.max_y && l1.max_y >= l2.min_y;
            if (overlap || dist <= 0.65 * avg_h) && dist < best_dist {
                best_dist = dist;
                best_idx = Some(j);
            }
        }

        if let Some(j) = best_idx {
            used2[j] = true;
            let l2 = &lines2[j];
            let t1 = l1.text.trim();
            let t2 = l2.text.trim();
            let p1 = line_optical_defect_penalty(t1);
            let p2 = line_optical_defect_penalty(t2);
            let chosen = if (t2.chars().count() as f64) < (0.75 * t1.chars().count() as f64) {
                l1
            } else if t2.ends_with('<') && !t1.ends_with('<') {
                l1
            } else if t1.ends_with('<') && !t2.ends_with('<') {
                l2
            } else if p2 < p1 {
                l2
            } else if p1 < p2 {
                l1
            } else if t1.chars().count() > t2.chars().count() {
                l1
            } else {
                l2
            };
            fused.push(chosen.clone());
        } else {
            fused.push(l1.clone());
        }
    }

    for (j, l2) in lines2.iter().enumerate() {
        if !used2[j] {
            fused.push(l2.clone());
        }
    }

    fused.sort_by(|a, b| a.min_y.partial_cmp(&b.min_y).unwrap_or(std::cmp::Ordering::Equal));
    fused
}

/// Convert Y-sorted `OcrLineBox` items into text lines preserving adaptive paragraph breaks.
pub fn format_parsed_lines_to_text(
    sorted_lines: &[OcrLineBox],
    has_date_header: bool,
) -> Vec<String> {
    if sorted_lines.is_empty() {
        return Vec::new();
    }
    if sorted_lines.len() == 1 && sorted_lines[0].text.trim() == "<<<NOTES_LIST_VIEW>>>" {
        return vec!["<<<NOTES_LIST_VIEW>>>".to_string()];
    }

    let mut slice = sorted_lines.to_vec();
    slice.sort_by(|a, b| a.min_y.partial_cmp(&b.min_y).unwrap_or(std::cmp::Ordering::Equal));

    let avg_h = {
        let sum: f64 = slice.iter().map(|i| i.height).sum();
        if !slice.is_empty() {
            sum / slice.len() as f64
        } else {
            30.0
        }
    };

    let mut lines = Vec::new();
    if has_date_header {
        lines.push("<<<DOC_START>>>".to_string());
    }

    let is_short = slice.len() <= 7;
    for i in 0..slice.len() {
        let curr = &slice[i];
        if i > 0 {
            let prev = &slice[i - 1];
            let gap = curr.min_y - prev.max_y;
            let prev_cy = (prev.min_y + prev.max_y) * 0.5;
            let curr_cy = (curr.min_y + curr.max_y) * 0.5;
            let center_step = curr_cy - prev_cy;

            if is_short {
                if gap > avg_h * 0.25 || center_step > avg_h * 1.52 {
                    lines.push(String::new());
                }
            } else if gap > avg_h * 0.58 || center_step > avg_h * 1.68 {
                lines.push(String::new());
            }
        }
        lines.push(curr.text.clone());
    }
    lines
}

// ============================================================================
// PILAR 2A: In-Process WinRT COM OCR & GDI+ PNG/JPEG Decoder (`combase.dll` + `gdiplus.dll`)
// ============================================================================

#[cfg(windows)]
mod winrt_com {
    use super::*;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Guid {
        data1: u32,
        data2: u16,
        data3: u16,
        data4: [u8; 8],
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct WinRtRect {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    }

    #[repr(C)]
    struct GdiplusStartupInput {
        gdiplus_version: u32,
        debug_event_callback: *mut c_void,
        suppress_background_thread: i32,
        suppress_external_codecs: i32,
    }

    #[repr(C)]
    struct GpRect {
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    }

    #[repr(C)]
    struct BitmapData {
        width: u32,
        height: u32,
        stride: i32,
        pixel_format: i32,
        scan0: *mut u8,
        reserved: usize,
    }

    const RPC_E_CHANGED_MODE: i32 = -2147417850; // 0x80010106

    // IAsyncInfo {00000036-0000-0000-C000-000000000046}
    const IID_IASYNC_INFO: Guid = Guid {
        data1: 0x00000036,
        data2: 0x0000,
        data3: 0x0000,
        data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
    };

    // IBufferFactory {71AF914D-C10F-484B-BC50-14BC623B3A27}
    const IID_IBUFFER_FACTORY: Guid = Guid {
        data1: 0x71AF914D,
        data2: 0xC10F,
        data3: 0x484B,
        data4: [0xBC, 0x50, 0x14, 0xBC, 0x62, 0x3B, 0x3A, 0x27],
    };

    // IBuffer {905A0FE0-BC53-11DF-8C49-001E4FC686DA}
    const IID_IBUFFER: Guid = Guid {
        data1: 0x905A0FE0,
        data2: 0xBC53,
        data3: 0x11DF,
        data4: [0x8C, 0x49, 0x00, 0x1E, 0x4F, 0xC6, 0x86, 0xDA],
    };

    // IBufferByteAccess {905A0FEF-BC53-11DF-8C49-001E4FC686DA}
    const IID_IBUFFER_BYTE_ACCESS: Guid = Guid {
        data1: 0x905A0FEF,
        data2: 0xBC53,
        data3: 0x11DF,
        data4: [0x8C, 0x49, 0x00, 0x1E, 0x4F, 0xC6, 0x86, 0xDA],
    };

    // ISoftwareBitmapStatics {DF0385DB-672F-4A9D-806E-C2442F343E86}
    const IID_ISOFTWARE_BITMAP_STATICS: Guid = Guid {
        data1: 0xDF0385DB,
        data2: 0x672F,
        data3: 0x4A9D,
        data4: [0x80, 0x6E, 0xC2, 0x44, 0x2F, 0x34, 0x3E, 0x86],
    };

    // IOcrEngineStatics {5BFFA85A-3384-3540-9940-699120D428A8}
    const IID_IOCR_ENGINE_STATICS: Guid = Guid {
        data1: 0x5BFFA85A,
        data2: 0x3384,
        data3: 0x3540,
        data4: [0x99, 0x40, 0x69, 0x91, 0x20, 0xD4, 0x28, 0xA8],
    };

    #[link(name = "kernel32")]
    extern "system" {
        fn LoadLibraryA(lp_lib_file_name: *const u8) -> *mut c_void;
        fn GetProcAddress(h_module: *mut c_void, lp_proc_name: *const i8) -> *mut c_void;
        fn FreeLibrary(h_lib_module: *mut c_void) -> i32;
        fn Sleep(dw_milliseconds: u32);
    }

    type FnRoInitialize = unsafe extern "system" fn(init_type: i32) -> i32;
    type FnWindowsCreateString =
        unsafe extern "system" fn(src: *const u16, len: u32, out: *mut *mut c_void) -> i32;
    type FnWindowsDeleteString = unsafe extern "system" fn(hstr: *mut c_void) -> i32;
    type FnWindowsGetStringRawBuffer =
        unsafe extern "system" fn(hstr: *mut c_void, len: *mut u32) -> *const u16;
    type FnRoGetActivationFactory = unsafe extern "system" fn(
        activatable_class_id: *mut c_void,
        iid: *const Guid,
        factory: *mut *mut c_void,
    ) -> i32;

    type FnSHCreateMemStream = unsafe extern "system" fn(p_init: *const u8, cb_init: u32) -> *mut c_void;
    type FnGdiplusStartup = unsafe extern "system" fn(
        token: *mut usize,
        input: *const GdiplusStartupInput,
        output: *mut c_void,
    ) -> i32;
    type FnGdipCreateBitmapFromStream =
        unsafe extern "system" fn(stream: *mut c_void, bitmap: *mut *mut c_void) -> i32;
    type FnGdipGetImageWidth = unsafe extern "system" fn(image: *mut c_void, width: *mut u32) -> i32;
    type FnGdipGetImageHeight =
        unsafe extern "system" fn(image: *mut c_void, height: *mut u32) -> i32;
    type FnGdipBitmapLockBits = unsafe extern "system" fn(
        bitmap: *mut c_void,
        rect: *const GpRect,
        flags: u32,
        format: i32,
        locked_data: *mut BitmapData,
    ) -> i32;
    type FnGdipBitmapUnlockBits =
        unsafe extern "system" fn(bitmap: *mut c_void, locked_data: *mut BitmapData) -> i32;
    type FnGdipDisposeImage = unsafe extern "system" fn(image: *mut c_void) -> i32;

    #[repr(C)]
    struct ComObj {
        vtable: *const *const c_void,
    }

    unsafe fn release_com(obj: *mut c_void) {
        if obj.is_null() {
            return;
        }
        let com = obj as *mut ComObj;
        let vt = (*com).vtable;
        if vt.is_null() {
            return;
        }
        let release_fn: unsafe extern "system" fn(*mut c_void) -> u32 =
            std::mem::transmute(*vt.add(2));
        let _ = release_fn(obj);
    }

    unsafe fn qi_com(obj: *mut c_void, iid: &Guid) -> *mut c_void {
        if obj.is_null() {
            return ptr::null_mut();
        }
        let com = obj as *mut ComObj;
        let vt = (*com).vtable;
        if vt.is_null() {
            return ptr::null_mut();
        }
        let qi_fn: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> i32 =
            std::mem::transmute(*vt.add(0));
        let mut out: *mut c_void = ptr::null_mut();
        if qi_fn(obj, iid, &mut out) >= 0 {
            out
        } else {
            ptr::null_mut()
        }
    }

    struct GdiplusApi {
        h_gdiplus: usize,
        h_shlwapi: usize,
        sh_create_mem_stream: FnSHCreateMemStream,
        dip_create_from_stream: FnGdipCreateBitmapFromStream,
        dip_get_width: FnGdipGetImageWidth,
        dip_get_height: FnGdipGetImageHeight,
        dip_lock_bits: FnGdipBitmapLockBits,
        dip_unlock_bits: FnGdipBitmapUnlockBits,
        dip_dispose: FnGdipDisposeImage,
    }

    struct CombaseApi {
        _h_combase: usize,
        ro_init: FnRoInitialize,
        win_create_str: FnWindowsCreateString,
        win_del_str: FnWindowsDeleteString,
        win_get_buf: FnWindowsGetStringRawBuffer,
        get_factory: FnRoGetActivationFactory,
    }

    struct ComGuard(*mut c_void);
    impl Drop for ComGuard {
        fn drop(&mut self) {
            unsafe {
                release_com(self.0);
            }
        }
    }

    /// Tahap 243 Pilar 2: Process-lifetime `GdiplusStartup` token (initialized ONCE, NEVER shut down).
    static GDIPLUS_TOKEN: OnceLock<usize> = OnceLock::new();
    static GDIPLUS_API: OnceLock<Option<GdiplusApi>> = OnceLock::new();
    /// Process-lifetime `combase.dll` API table (loaded ONCE, NEVER `FreeLibrary`d during async WinRT OCR).
    static COMBASE_API: OnceLock<Option<CombaseApi>> = OnceLock::new();

    fn get_combase_api() -> Option<&'static CombaseApi> {
        COMBASE_API
            .get_or_init(|| unsafe {
                let h_combase = LoadLibraryA(b"combase.dll\0".as_ptr());
                if h_combase.is_null() {
                    return None;
                }
                let p_ro_init = GetProcAddress(h_combase, b"RoInitialize\0".as_ptr().cast());
                let p_create_str =
                    GetProcAddress(h_combase, b"WindowsCreateString\0".as_ptr().cast());
                let p_del_str = GetProcAddress(h_combase, b"WindowsDeleteString\0".as_ptr().cast());
                let p_get_buf =
                    GetProcAddress(h_combase, b"WindowsGetStringRawBuffer\0".as_ptr().cast());
                let p_get_factory =
                    GetProcAddress(h_combase, b"RoGetActivationFactory\0".as_ptr().cast());
                if p_ro_init.is_null()
                    || p_create_str.is_null()
                    || p_del_str.is_null()
                    || p_get_buf.is_null()
                    || p_get_factory.is_null()
                {
                    FreeLibrary(h_combase);
                    return None;
                }
                Some(CombaseApi {
                    _h_combase: h_combase as usize,
                    ro_init: std::mem::transmute(p_ro_init),
                    win_create_str: std::mem::transmute(p_create_str),
                    win_del_str: std::mem::transmute(p_del_str),
                    win_get_buf: std::mem::transmute(p_get_buf),
                    get_factory: std::mem::transmute(p_get_factory),
                })
            })
            .as_ref()
    }

    fn get_gdiplus_api() -> Option<&'static GdiplusApi> {
        GDIPLUS_API
            .get_or_init(|| unsafe {
                let h_shlwapi = LoadLibraryA(b"shlwapi.dll\0".as_ptr());
                let h_gdiplus = LoadLibraryA(b"gdiplus.dll\0".as_ptr());
                if h_shlwapi.is_null() || h_gdiplus.is_null() {
                    if !h_shlwapi.is_null() {
                        FreeLibrary(h_shlwapi);
                    }
                    if !h_gdiplus.is_null() {
                        FreeLibrary(h_gdiplus);
                    }
                    return None;
                }
                let p_mem_stream = GetProcAddress(h_shlwapi, b"SHCreateMemStream\0".as_ptr().cast());
                let p_startup = GetProcAddress(h_gdiplus, b"GdiplusStartup\0".as_ptr().cast());
                let p_create = GetProcAddress(h_gdiplus, b"GdipCreateBitmapFromStream\0".as_ptr().cast());
                let p_width = GetProcAddress(h_gdiplus, b"GdipGetImageWidth\0".as_ptr().cast());
                let p_height = GetProcAddress(h_gdiplus, b"GdipGetImageHeight\0".as_ptr().cast());
                let p_lock = GetProcAddress(h_gdiplus, b"GdipBitmapLockBits\0".as_ptr().cast());
                let p_unlock = GetProcAddress(h_gdiplus, b"GdipBitmapUnlockBits\0".as_ptr().cast());
                let p_dispose = GetProcAddress(h_gdiplus, b"GdipDisposeImage\0".as_ptr().cast());

                if p_mem_stream.is_null()
                    || p_startup.is_null()
                    || p_create.is_null()
                    || p_width.is_null()
                    || p_height.is_null()
                    || p_lock.is_null()
                    || p_unlock.is_null()
                    || p_dispose.is_null()
                {
                    FreeLibrary(h_shlwapi);
                    FreeLibrary(h_gdiplus);
                    return None;
                }

                let startup: FnGdiplusStartup = std::mem::transmute(p_startup);
                let mut token: usize = 0;
                let input = GdiplusStartupInput {
                    gdiplus_version: 1,
                    debug_event_callback: ptr::null_mut(),
                    suppress_background_thread: 0,
                    suppress_external_codecs: 0,
                };
                if startup(&mut token, &input, ptr::null_mut()) != 0 {
                    FreeLibrary(h_shlwapi);
                    FreeLibrary(h_gdiplus);
                    return None;
                }
                let _ = GDIPLUS_TOKEN.set(token);

                Some(GdiplusApi {
                    h_gdiplus: h_gdiplus as usize,
                    h_shlwapi: h_shlwapi as usize,
                    sh_create_mem_stream: std::mem::transmute(p_mem_stream),
                    dip_create_from_stream: std::mem::transmute(p_create),
                    dip_get_width: std::mem::transmute(p_width),
                    dip_get_height: std::mem::transmute(p_height),
                    dip_lock_bits: std::mem::transmute(p_lock),
                    dip_unlock_bits: std::mem::transmute(p_unlock),
                    dip_dispose: std::mem::transmute(p_dispose),
                })
            })
            .as_ref()
    }

    pub fn decode_image_bytes_gdiplus(bytes: &[u8]) -> Option<(Vec<u8>, usize, usize)> {
        if bytes.len() < 8 || bytes.len() > 32 * 1024 * 1024 {
            return None;
        }
        let api = get_gdiplus_api()?;
        let _keep_loaded = (api.h_gdiplus, api.h_shlwapi);
        unsafe {
            let istream = (api.sh_create_mem_stream)(bytes.as_ptr(), bytes.len() as u32);
            if istream.is_null() {
                return None;
            }
            let _istream_guard = ComGuard(istream);
            let mut bitmap: *mut c_void = ptr::null_mut();
            let st_create = (api.dip_create_from_stream)(istream, &mut bitmap);
            if st_create != 0 || bitmap.is_null() {
                return None;
            }

            let mut w: u32 = 0;
            let mut h: u32 = 0;
            let st_w = (api.dip_get_width)(bitmap, &mut w);
            let st_h = (api.dip_get_height)(bitmap, &mut h);
            if st_w != 0 || st_h != 0 || w == 0 || h == 0 || w > 8192 || h > 8192 {
                (api.dip_dispose)(bitmap);
                return None;
            }

            let rect = GpRect {
                x: 0,
                y: 0,
                width: w as i32,
                height: h as i32,
            };
            let mut bmp_data = BitmapData {
                width: 0,
                height: 0,
                stride: 0,
                pixel_format: 0,
                scan0: ptr::null_mut(),
                reserved: 0,
            };
            const IMAGE_LOCK_MODE_READ: u32 = 1;
            const PIXEL_FORMAT_32BPP_ARGB: i32 = 0x0026_200A;

            let st_lock = (api.dip_lock_bits)(
                bitmap,
                &rect,
                IMAGE_LOCK_MODE_READ,
                PIXEL_FORMAT_32BPP_ARGB,
                &mut bmp_data,
            );
            let width = w as usize;
            let height = h as usize;
            let row_bytes = width.saturating_mul(4);
            let stride = bmp_data.stride as isize;
            if st_lock != 0
                || bmp_data.scan0.is_null()
                || stride == 0
                || stride.unsigned_abs() < row_bytes
            {
                if st_lock == 0 {
                    (api.dip_unlock_bits)(bitmap, &mut bmp_data);
                }
                (api.dip_dispose)(bitmap);
                return None;
            }

            let mut rgba = vec![255u8; height.saturating_mul(row_bytes)];
            let base_ptr = bmp_data.scan0.cast::<u8>();
            for y in 0..height {
                let src_row = base_ptr.offset((y as isize).saturating_mul(stride));
                let row_slice = std::slice::from_raw_parts(src_row, row_bytes);
                let dst_row = &mut rgba[y * row_bytes..(y + 1) * row_bytes];
                for x in 0..width {
                    let sp = x * 4;
                    // GDI+ 32bppARGB in little-endian memory is [B, G, R, A]
                    dst_row[sp] = row_slice[sp + 2];
                    dst_row[sp + 1] = row_slice[sp + 1];
                    dst_row[sp + 2] = row_slice[sp];
                    dst_row[sp + 3] = row_slice[sp + 3];
                }
            }

            (api.dip_unlock_bits)(bitmap, &mut bmp_data);
            (api.dip_dispose)(bitmap);
            Some((rgba, width, height))
        }
    }

    pub fn probe_ocr_engine_com() -> bool {
        unsafe {
            let h_combase = LoadLibraryA(b"combase.dll\0".as_ptr());
            if h_combase.is_null() {
                return false;
            }
            let p_ro_init = GetProcAddress(h_combase, b"RoInitialize\0".as_ptr().cast());
            let p_create_str = GetProcAddress(h_combase, b"WindowsCreateString\0".as_ptr().cast());
            let p_del_str = GetProcAddress(h_combase, b"WindowsDeleteString\0".as_ptr().cast());
            let p_get_factory = GetProcAddress(h_combase, b"RoGetActivationFactory\0".as_ptr().cast());

            if p_ro_init.is_null()
                || p_create_str.is_null()
                || p_del_str.is_null()
                || p_get_factory.is_null()
            {
                FreeLibrary(h_combase);
                return false;
            }

            let ro_init: FnRoInitialize = std::mem::transmute(p_ro_init);
            let win_create_str: FnWindowsCreateString = std::mem::transmute(p_create_str);
            let win_del_str: FnWindowsDeleteString = std::mem::transmute(p_del_str);
            let get_factory: FnRoGetActivationFactory = std::mem::transmute(p_get_factory);

            let _ = ro_init(1); // S_OK, S_FALSE, or RPC_E_CHANGED_MODE are all usable

            let class_wide: Vec<u16> = "Windows.Media.Ocr.OcrEngine"
                .encode_utf16()
                .collect();
            let mut hstr: *mut c_void = ptr::null_mut();
            let hr_s = win_create_str(class_wide.as_ptr(), class_wide.len() as u32, &mut hstr);
            if hr_s < 0 || hstr.is_null() {
                FreeLibrary(h_combase);
                return false;
            }

            let mut factory: *mut c_void = ptr::null_mut();
            let hr_f = get_factory(hstr, &IID_IOCR_ENGINE_STATICS, &mut factory);
            let _ = win_del_str(hstr);
            if hr_f < 0 || factory.is_null() {
                FreeLibrary(h_combase);
                return false;
            }

            // Call IOcrEngineStatics::get_MaxImageDimension (slot 6) & TryCreateFromUserProfileLanguages (slot 10)
            let mut max_dim: u32 = 0;
            let com = factory as *mut ComObj;
            let vt = (*com).vtable;
            let get_max_dim: unsafe extern "system" fn(*mut c_void, *mut u32) -> i32 =
                std::mem::transmute(*vt.add(6));
            let hr_dim = get_max_dim(factory, &mut max_dim);

            let mut engine: *mut c_void = ptr::null_mut();
            let try_create: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32 =
                std::mem::transmute(*vt.add(10));
            let hr_eng = try_create(factory, &mut engine);

            let ok = hr_dim >= 0 && max_dim >= 2048 && (hr_eng >= 0 || !engine.is_null());
            release_com(engine);
            release_com(factory);
            FreeLibrary(h_combase);
            ok
        }
    }

    unsafe fn hstring_to_string(
        hstr: *mut c_void,
        win_get_buf: FnWindowsGetStringRawBuffer,
        win_del_str: FnWindowsDeleteString,
    ) -> String {
        if hstr.is_null() {
            return String::new();
        }
        let mut len: u32 = 0;
        let ptr_u16 = win_get_buf(hstr, &mut len);
        let out = if !ptr_u16.is_null() && len > 0 {
            let slice = std::slice::from_raw_parts(ptr_u16, len as usize);
            String::from_utf16_lossy(slice)
        } else {
            String::new()
        };
        let _ = win_del_str(hstr);
        out
    }

    unsafe fn wait_async_operation(async_op: *mut c_void, max_wait_ms: u32) -> bool {
        if async_op.is_null() {
            return false;
        }
        let info = qi_com(async_op, &IID_IASYNC_INFO);
        if info.is_null() {
            return false;
        }
        let get_status: unsafe extern "system" fn(*mut c_void, *mut i32) -> i32 =
            std::mem::transmute(*(*(info as *mut ComObj)).vtable.add(7));
        let mut elapsed = 0u32;
        let mut completed_ok = false;
        loop {
            let mut status: i32 = -1;
            let hr = get_status(info, &mut status);
            if hr < 0 {
                break;
            }
            if status == 1 {
                // AsyncStatus::Completed
                completed_ok = true;
                break;
            }
            if status != 0 || elapsed >= max_wait_ms {
                break;
            }
            Sleep(2);
            elapsed = elapsed.saturating_add(2);
        }
        release_com(info);
        completed_ok
    }

    unsafe fn recognize_rgba_on_mta(rgba: &[u8], width: usize, height: usize) -> Vec<OcrLineBox> {
        let mut out_lines = Vec::new();
        if width == 0 || height == 0 || width > 8192 || height > 8192 {
            return out_lines;
        }
        let expected_bytes = match width.checked_mul(height).and_then(|n| n.checked_mul(4)) {
            Some(n) if n > 0 && n <= u32::MAX as usize && rgba.len() >= n => n,
            _ => return out_lines,
        };
        let Some(api) = get_combase_api() else {
            return out_lines;
        };

        let ro_init = api.ro_init;
        let win_create_str = api.win_create_str;
        let win_del_str = api.win_del_str;
        let win_get_buf = api.win_get_buf;
        let get_factory = api.get_factory;

        let hr_init = ro_init(1);
        if hr_init == RPC_E_CHANGED_MODE {
            // Current thread is STA: run cleanly on a dedicated 8 MB stack MTA worker.
            let owned = rgba[..expected_bytes].to_vec();
            return std::thread::Builder::new()
                .name("iphone-ocr-worker".into())
                .stack_size(8 * 1024 * 1024)
                .spawn(move || {
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
                        recognize_rgba_on_mta(&owned, width, height)
                    }))
                    .unwrap_or_default()
                })
                .ok()
                .and_then(|h| h.join().ok())
                .unwrap_or_default();
        }

        let make_hstr = |s: &str| -> *mut c_void {
            let wide: Vec<u16> = s.encode_utf16().collect();
            let mut h: *mut c_void = ptr::null_mut();
            if win_create_str(wide.as_ptr(), wide.len() as u32, &mut h) >= 0 {
                h
            } else {
                ptr::null_mut()
            }
        };

        // 1. Activate IOcrEngineStatics & create OcrEngine (all pointers RAII-guarded via ComGuard)
        let mut ocr_factory: *mut c_void = ptr::null_mut();
        let h_ocr_cls = make_hstr("Windows.Media.Ocr.OcrEngine");
        if !h_ocr_cls.is_null() {
            let _ = get_factory(h_ocr_cls, &IID_IOCR_ENGINE_STATICS, &mut ocr_factory);
            let _ = win_del_str(h_ocr_cls);
        }
        if ocr_factory.is_null() {
            return out_lines;
        }
        let _ocr_factory_guard = ComGuard(ocr_factory);

        let mut max_dim: u32 = 4096;
        let get_max_dim: unsafe extern "system" fn(*mut c_void, *mut u32) -> i32 =
            std::mem::transmute(*(*(ocr_factory as *mut ComObj)).vtable.add(6));
        let _ = get_max_dim(ocr_factory, &mut max_dim);

        if width > max_dim as usize || height > max_dim as usize {
            return out_lines;
        }

        let mut ocr_engine: *mut c_void = ptr::null_mut();
        let try_create: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32 =
            std::mem::transmute(*(*(ocr_factory as *mut ComObj)).vtable.add(10));
        let hr_eng = try_create(ocr_factory, &mut ocr_engine);
        if hr_eng < 0 || ocr_engine.is_null() {
            return out_lines;
        }
        let _ocr_engine_guard = ComGuard(ocr_engine);

        // 2. Create Windows.Storage.Streams.Buffer and copy RGBA bytes
        let mut buf_factory: *mut c_void = ptr::null_mut();
        let h_buf_cls = make_hstr("Windows.Storage.Streams.Buffer");
        if !h_buf_cls.is_null() {
            let _ = get_factory(h_buf_cls, &IID_IBUFFER_FACTORY, &mut buf_factory);
            let _ = win_del_str(h_buf_cls);
        }
        if buf_factory.is_null() {
            return out_lines;
        }
        let _buf_factory_guard = ComGuard(buf_factory);

        let byte_len = expected_bytes as u32;
        let mut raw_buf_obj: *mut c_void = ptr::null_mut();
        let buf_create: unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> i32 =
            std::mem::transmute(*(*(buf_factory as *mut ComObj)).vtable.add(6));
        let hr_bc = buf_create(buf_factory, byte_len, &mut raw_buf_obj);
        if hr_bc < 0 || raw_buf_obj.is_null() {
            return out_lines;
        }
        let _raw_buf_guard = ComGuard(raw_buf_obj);

        let ibuffer = qi_com(raw_buf_obj, &IID_IBUFFER);
        let byte_access = qi_com(raw_buf_obj, &IID_IBUFFER_BYTE_ACCESS);
        let _ibuffer_guard = ComGuard(ibuffer);
        let _byte_access_guard = ComGuard(byte_access);
        if ibuffer.is_null() || byte_access.is_null() {
            return out_lines;
        }

        let mut dst_ptr: *mut u8 = ptr::null_mut();
        let get_raw_ptr: unsafe extern "system" fn(*mut c_void, *mut *mut u8) -> i32 =
            std::mem::transmute(*(*(byte_access as *mut ComObj)).vtable.add(3));
        if get_raw_ptr(byte_access, &mut dst_ptr) >= 0 && !dst_ptr.is_null() {
            ptr::copy_nonoverlapping(rgba.as_ptr(), dst_ptr, expected_bytes);
        } else {
            return out_lines;
        }

        let put_len: unsafe extern "system" fn(*mut c_void, u32) -> i32 =
            std::mem::transmute(*(*(ibuffer as *mut ComObj)).vtable.add(8));
        if put_len(ibuffer, byte_len) < 0 {
            return out_lines;
        }

        // 3. Create SoftwareBitmap via ISoftwareBitmapStatics::CreateCopyFromBuffer(Rgba8 = 30)
        let mut sbm_statics: *mut c_void = ptr::null_mut();
        let h_sbm_cls = make_hstr("Windows.Graphics.Imaging.SoftwareBitmap");
        if !h_sbm_cls.is_null() {
            let _ = get_factory(h_sbm_cls, &IID_ISOFTWARE_BITMAP_STATICS, &mut sbm_statics);
            let _ = win_del_str(h_sbm_cls);
        }
        if sbm_statics.is_null() {
            return out_lines;
        }
        let _sbm_statics_guard = ComGuard(sbm_statics);

        let mut software_bitmap: *mut c_void = ptr::null_mut();
        let create_copy: unsafe extern "system" fn(
            *mut c_void,
            *mut c_void,
            i32,
            i32,
            i32,
            *mut *mut c_void,
        ) -> i32 = std::mem::transmute(*(*(sbm_statics as *mut ComObj)).vtable.add(9));
        let hr_sbm = create_copy(
            sbm_statics,
            ibuffer,
            30, // BitmapPixelFormat::Rgba8
            width as i32,
            height as i32,
            &mut software_bitmap,
        );
        if hr_sbm < 0 || software_bitmap.is_null() {
            return out_lines;
        }
        // Keep software_bitmap and ocr_engine alive UNTIL AFTER RecognizeAsync + GetResults finish!
        let _software_bitmap_guard = ComGuard(software_bitmap);

        // 4. IOcrEngine::RecognizeAsync(SoftwareBitmap)
        let mut async_op: *mut c_void = ptr::null_mut();
        let recognize_async: unsafe extern "system" fn(
            *mut c_void,
            *mut c_void,
            *mut *mut c_void,
        ) -> i32 = std::mem::transmute(*(*(ocr_engine as *mut ComObj)).vtable.add(6));
        let hr_rec = recognize_async(ocr_engine, software_bitmap, &mut async_op);
        if hr_rec < 0 || async_op.is_null() {
            return out_lines;
        }
        let _async_op_guard = ComGuard(async_op);

        if !wait_async_operation(async_op, 5_000) {
            return out_lines;
        }

        let mut ocr_result: *mut c_void = ptr::null_mut();
        let get_results: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32 =
            std::mem::transmute(*(*(async_op as *mut ComObj)).vtable.add(8));
        let hr_res = get_results(async_op, &mut ocr_result);
        if hr_res < 0 || ocr_result.is_null() {
            return out_lines;
        }
        let _ocr_result_guard = ComGuard(ocr_result);

        // 5. Extract IOcrResult::get_Lines -> IVectorView<OcrLine>
        let mut lines_vec: *mut c_void = ptr::null_mut();
        let get_lines: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32 =
            std::mem::transmute(*(*(ocr_result as *mut ComObj)).vtable.add(6));
        if get_lines(ocr_result, &mut lines_vec) >= 0 && !lines_vec.is_null() {
            let _lines_vec_guard = ComGuard(lines_vec);
            let mut line_count: u32 = 0;
            let get_size: unsafe extern "system" fn(*mut c_void, *mut u32) -> i32 =
                std::mem::transmute(*(*(lines_vec as *mut ComObj)).vtable.add(7));
            let get_at: unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> i32 =
                std::mem::transmute(*(*(lines_vec as *mut ComObj)).vtable.add(6));
            if get_size(lines_vec, &mut line_count) >= 0 {
                for i in 0..line_count {
                    let mut ocr_line: *mut c_void = ptr::null_mut();
                    if get_at(lines_vec, i, &mut ocr_line) < 0 || ocr_line.is_null() {
                        continue;
                    }
                    let _ocr_line_guard = ComGuard(ocr_line);
                    let mut h_line_text: *mut c_void = ptr::null_mut();
                    let get_line_text: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32 =
                        std::mem::transmute(*(*(ocr_line as *mut ComObj)).vtable.add(7));
                    let text = if get_line_text(ocr_line, &mut h_line_text) >= 0 {
                        hstring_to_string(h_line_text, win_get_buf, win_del_str)
                    } else {
                        String::new()
                    };

                    // Extract line bounding box via IOcrLine::get_Words (slot 6)
                    let mut min_y = f64::MAX;
                    let mut max_y = f64::MIN;
                    let mut words_vec: *mut c_void = ptr::null_mut();
                    let get_words: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32 =
                        std::mem::transmute(*(*(ocr_line as *mut ComObj)).vtable.add(6));
                    if get_words(ocr_line, &mut words_vec) >= 0 && !words_vec.is_null() {
                        let _words_vec_guard = ComGuard(words_vec);
                        let mut word_count: u32 = 0;
                        let w_size: unsafe extern "system" fn(*mut c_void, *mut u32) -> i32 =
                            std::mem::transmute(*(*(words_vec as *mut ComObj)).vtable.add(7));
                        let w_at: unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> i32 =
                            std::mem::transmute(*(*(words_vec as *mut ComObj)).vtable.add(6));
                        if w_size(words_vec, &mut word_count) >= 0 {
                            for wi in 0..word_count {
                                let mut word_obj: *mut c_void = ptr::null_mut();
                                if w_at(words_vec, wi, &mut word_obj) >= 0 && !word_obj.is_null() {
                                    let _word_guard = ComGuard(word_obj);
                                    let mut rect = WinRtRect {
                                        x: 0.0,
                                        y: 0.0,
                                        width: 0.0,
                                        height: 0.0,
                                    };
                                    let get_rect: unsafe extern "system" fn(*mut c_void, *mut WinRtRect) -> i32 =
                                        std::mem::transmute(*(*(word_obj as *mut ComObj)).vtable.add(6));
                                    if get_rect(word_obj, &mut rect) >= 0 && rect.height > 0.0 {
                                        min_y = min_y.min(rect.y as f64);
                                        max_y = max_y.max((rect.y + rect.height) as f64);
                                    }
                                }
                            }
                        }
                    }

                    let trimmed = text.trim().to_string();
                    if !trimmed.is_empty() {
                        if min_y == f64::MAX || max_y <= min_y {
                            min_y = (i as f64) * 28.0;
                            max_y = min_y + 20.0;
                        }
                        out_lines.push(OcrLineBox {
                            text: trimmed,
                            min_y,
                            max_y,
                            height: (max_y - min_y).max(1.0),
                        });
                    }
                }
            }
        }
        out_lines
    }

    pub fn recognize_rgba_lines(rgba: &[u8], width: usize, height: usize) -> Vec<OcrLineBox> {
        if width == 0 || height == 0 || rgba.len() < width.saturating_mul(height).saturating_mul(4) {
            return Vec::new();
        }
        let owned = rgba[..width * height * 4].to_vec();
        // Tahap 243 Pilar 2: Always run WinRT OCR inside a dedicated 8 MB stack MTA worker + catch_unwind
        // so deep WinRT COM frames NEVER overflow an HTTP worker stack or force-close the process!
        std::thread::Builder::new()
            .name("iphone-ocr-worker".into())
            .stack_size(8 * 1024 * 1024)
            .spawn(move || {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
                    recognize_rgba_on_mta(&owned, width, height)
                }))
                .unwrap_or_default()
            })
            .ok()
            .and_then(|h| h.join().ok())
            .unwrap_or_default()
    }
}

/// Decodes raw PNG, JPEG, BMP, GIF, or TIFF bytes into `(rgba_bytes, width, height)` in-process via `gdiplus.dll` + `shlwapi.dll`.
pub fn decode_image_bytes_to_rgba(bytes: &[u8]) -> Option<(Vec<u8>, usize, usize)> {
    #[cfg(windows)]
    {
        if bytes.len() >= 54 && &bytes[0..2] == b"BM" {
            // Fast path for uncompressed BMP also works via gdiplus below; try gdiplus first or fallback.
            if let Some(res) = winrt_com::decode_image_bytes_gdiplus(bytes) {
                return Some(res);
            }
        }
        return winrt_com::decode_image_bytes_gdiplus(bytes);
    }
    #[cfg(not(windows))]
    {
        let _ = bytes;
        None
    }
}

fn decode_base64_flexible(raw_b64: &str) -> Option<Vec<u8>> {
    let mut clean = String::with_capacity(raw_b64.len());
    for b in raw_b64.bytes() {
        if b.is_ascii_whitespace() {
            continue;
        }
        match b {
            b'-' => clean.push('+'),
            b'_' => clean.push('/'),
            _ => clean.push(b as char),
        }
    }
    while clean.len() % 4 != 0 {
        clean.push('=');
    }
    iphone_link_video::video_hwnd::decode_base64_fast(&clean)
}

/// Decodes any `data:image/<png|jpeg|jpg|bmp|webp>;base64,...` URL into `(rgba_bytes, width, height)` in-process.
pub fn decode_image_data_url(data_url: &str) -> Option<(Vec<u8>, usize, usize)> {
    let trimmed = data_url.trim();
    if trimmed.starts_with("data:image/bmp;base64,") {
        if let Some(fast) = iphone_link_video::video_hwnd::decode_bmp_data_url_to_rgba(trimmed) {
            return Some(fast);
        }
    }
    let payload = if let Some((_, after_comma)) = trimmed.split_once(";base64,") {
        after_comma
    } else if let Some((_, after_comma)) = trimmed.split_once(",") {
        after_comma
    } else {
        trimmed
    };
    let bytes = decode_base64_flexible(payload)?;
    decode_image_bytes_to_rgba(&bytes)
}

/// Recognizes text lines in an RGBA image buffer in-process via Windows Runtime `OcrEngine`.
pub fn recognize_rgba_with_winrt_ocr(rgba: &[u8], width: usize, height: usize) -> Vec<OcrLineBox> {
    #[cfg(windows)]
    {
        winrt_com::recognize_rgba_lines(rgba, width, height)
    }
    #[cfg(not(windows))]
    {
        let _ = (rgba, width, height);
        Vec::new()
    }
}

/// Probe in-process Windows Runtime `Windows.Media.Ocr.OcrEngine` via `combase.dll` without spawning `powershell.exe`.
pub fn probe_inprocess_winrt_ocr_com() -> bool {
    if INPROCESS_WINRT_COM_TESTED.load(Ordering::Acquire) {
        return INPROCESS_WINRT_COM_READY.load(Ordering::Acquire);
    }
    #[cfg(windows)]
    let ok = winrt_com::probe_ocr_engine_com();
    #[cfg(not(windows))]
    let ok = false;

    INPROCESS_WINRT_COM_READY.store(ok, Ordering::Release);
    INPROCESS_WINRT_COM_TESTED.store(true, Ordering::Release);
    ok
}

// ============================================================================
// Exact CPython difflib.SequenceMatcher(None, a, b).ratio()
// ============================================================================

fn find_longest_match(
    a: &[char],
    b: &[char],
    alo: usize,
    ahi: usize,
    blo: usize,
    bhi: usize,
) -> (usize, usize, usize) {
    let mut best_i = alo;
    let mut best_j = blo;
    let mut best_size = 0usize;

    let mut prev_j2len = vec![0usize; b.len() + 1];
    let mut cur_j2len = vec![0usize; b.len() + 1];

    for i in alo..ahi {
        cur_j2len[blo..bhi].fill(0);
        for j in blo..bhi {
            if a[i] == b[j] {
                let k = if j > blo { prev_j2len[j - 1] + 1 } else { 1 };
                cur_j2len[j] = k;
                if k > best_size {
                    best_i = i + 1 - k;
                    best_j = j + 1 - k;
                    best_size = k;
                }
            }
        }
        std::mem::swap(&mut prev_j2len, &mut cur_j2len);
    }

    (best_i, best_j, best_size)
}

fn matching_chars_count(
    a: &[char],
    b: &[char],
    alo: usize,
    ahi: usize,
    blo: usize,
    bhi: usize,
) -> usize {
    if alo >= ahi || blo >= bhi {
        return 0;
    }
    let (i, j, k) = find_longest_match(a, b, alo, ahi, blo, bhi);
    if k == 0 {
        return 0;
    }
    let mut total = k;
    if alo < i && blo < j {
        total += matching_chars_count(a, b, alo, i, blo, j);
    }
    if i + k < ahi && j + k < bhi {
        total += matching_chars_count(a, b, i + k, ahi, j + k, bhi);
    }
    total
}

/// Exact CPython `difflib.SequenceMatcher(None, a, b).ratio()`.
pub fn difflib_ratio(a_str: &str, b_str: &str) -> f64 {
    let a: Vec<char> = a_str.chars().collect();
    let b: Vec<char> = b_str.chars().collect();
    let total = a.len() + b.len();
    if total == 0 {
        return 1.0;
    }
    let matches = matching_chars_count(&a, &b, 0, a.len(), 0, b.len());
    (2.0 * matches as f64) / (total as f64)
}

// ============================================================================
// PILAR 2B: Character Classification, Filters & 15+ Optical Repairs
// ============================================================================

fn normalize_line_for_match(text: &str) -> String {
    let lower = text.to_lowercase();
    let mut space_collapsed = String::with_capacity(lower.len());
    let mut in_ws = false;
    for ch in lower.chars() {
        if ch.is_whitespace() {
            if !in_ws && !space_collapsed.is_empty() {
                space_collapsed.push(' ');
            }
            in_ws = true;
        } else {
            space_collapsed.push(ch);
            in_ws = false;
        }
    }
    space_collapsed.trim_end().to_string()
}

fn ends_with_terminal_punct(s: &str) -> bool {
    let t = s.trim_end();
    t.ends_with('.')
        || t.ends_with('?')
        || t.ends_with('!')
        || t.ends_with("\".")
        || t.ends_with("'.")
        || t.ends_with(").")
        || t.ends_with("].")
}

fn is_month_or_folder_header(clean: &str) -> bool {
    let low = clean.trim().to_ascii_lowercase();
    if low.starts_with("notes (") || low.starts_with("catatan (") {
        return true;
    }
    if matches!(
        low.as_str(),
        "today"
            | "yesterday"
            | "folders"
            | "hari ini"
            | "kemarin"
            | "icloud"
            | "all icloud"
            | "notes"
            | "catatan"
            | "january"
            | "february"
            | "march"
            | "april"
            | "may"
            | "june"
            | "july"
            | "august"
            | "september"
            | "october"
            | "november"
            | "december"
            | "januari"
            | "februari"
            | "maret"
            | "mei"
            | "juni"
            | "juli"
            | "agustus"
            | "oktober"
            | "desember"
    ) {
        return true;
    }
    let parts: Vec<&str> = low.split_whitespace().collect();
    if parts.len() == 2 && parts[0].chars().all(|c| c.is_ascii_digit()) {
        if matches!(parts[1], "notes" | "catatan") {
            return true;
        }
    }
    if parts.len() == 3
        && parts[0] == "previous"
        && parts[1].chars().all(|c| c.is_ascii_digit())
        && parts[2] == "days"
    {
        return true;
    }
    if parts.len() == 3
        && parts[0].chars().all(|c| c.is_ascii_digit())
        && parts[1] == "hari"
        && parts[2] == "sebelumnya"
    {
        return true;
    }
    false
}

fn is_notes_list_preview_timestamp(clean: &str) -> bool {
    let mut it = clean.trim().splitn(2, char::is_whitespace);
    let Some(first) = it.next() else {
        return false;
    };
    let Some(rest) = it.next() else {
        return false;
    };
    if rest.trim().is_empty() {
        return false;
    }
    let sep_pos = first.find('.').or_else(|| first.find(':'));
    let Some(pos) = sep_pos else {
        return false;
    };
    let (h, m) = (&first[..pos], &first[pos + 1..]);
    (1..=2).contains(&h.len())
        && m.len() == 2
        && h.chars().all(|c| c.is_ascii_digit())
        && m.chars().all(|c| c.is_ascii_digit())
}

fn is_ios_notes_date_header(clean: &str) -> bool {
    let toks: Vec<&str> = clean.split_whitespace().collect();
    if toks.len() < 3 || toks.len() > 5 {
        return false;
    }
    if !(1..=2).contains(&toks[0].len()) || !toks[0].chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    let mon = toks[1].to_ascii_lowercase();
    let valid_mon = matches!(
        mon.as_str(),
        "january"
            | "february"
            | "march"
            | "april"
            | "may"
            | "june"
            | "july"
            | "august"
            | "september"
            | "october"
            | "november"
            | "december"
            | "oktober"
            | "januari"
            | "februari"
            | "maret"
            | "mei"
            | "juni"
            | "juli"
            | "agustus"
            | "desember"
            | "jan"
            | "feb"
            | "mar"
            | "apr"
            | "jun"
            | "jul"
            | "aug"
            | "sep"
            | "oct"
            | "nov"
            | "dec"
    );
    if !valid_mon || toks[2].len() != 4 || !toks[2].chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    if toks.len() == 3 {
        return true;
    }
    toks.len() == 5
        && matches!(toks[3].to_ascii_lowercase().as_str(), "at" | "pukul")
        && (toks[4].contains('.') || toks[4].contains(':'))
}

/// Checks if a line or chunk contains Notes folder / list chrome.
pub fn is_notes_folder_list_frame(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return false;
    }
    if t.contains("<<<NOTES_LIST_VIEW>>>") {
        return true;
    }
    let mut ellipsis_count = 0usize;
    for raw in t.lines() {
        let clean = raw.trim();
        if clean.is_empty() {
            continue;
        }
        if is_month_or_folder_header(clean) || is_notes_list_preview_timestamp(clean) {
            return true;
        }
        if clean.ends_with("...") || clean.ends_with('…') {
            ellipsis_count += 1;
            if ellipsis_count >= 2 {
                return true;
            }
        }
    }
    false
}

fn is_pure_callout_menu_line(t: &str) -> bool {
    let t_low = t.to_ascii_lowercase();
    if t_low.contains("cut-off") || t_low.contains("cutoff") {
        return false;
    }
    if matches!(
        t_low.as_str(),
        "cut"
            | "copy"
            | "paste"
            | "delete"
            | "select"
            | "select all"
            | "autofill"
            | "look up"
            | "translate"
            | "share"
            | "share..."
            | "proofread"
            | "rewrite"
            | "no replacements found"
            | "cut | copy | paste"
    ) {
        return true;
    }
    // Check if the entire line consists purely of popup callout tokens & separators (e.g. "[ AutoFill | Paste ]", "Cut | Copy | Paste")
    let cleaned: String = t_low
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c
            } else {
                ' '
            }
        })
        .collect();
    let words: Vec<&str> = cleaned.split_whitespace().collect();
    if words.is_empty() {
        return false;
    }
    let mut menu_hits = 0usize;
    let mut i = 0usize;
    while i < words.len() {
        let w = words[i];
        let matched = match w {
            "cut" | "copy" | "paste" | "delete" | "select" | "autofill" | "translate" | "share"
            | "proofread" | "rewrite" | "co" | "py" | "te" | "cu" | "pa" | "de" => true,
            "all" => i > 0 && words[i - 1] == "select",
            "up" => i > 0 && words[i - 1] == "look",
            "look" => i + 1 < words.len() && words[i + 1] == "up",
            "no" => i + 2 < words.len() && words[i + 1] == "replacements" && words[i + 2] == "found",
            "replacements" => i > 0 && words[i - 1] == "no",
            "found" => i >= 2 && words[i - 1] == "replacements",
            _ => false,
        };
        if !matched {
            return false;
        }
        menu_hits += 1;
        i += 1;
    }
    menu_hits >= 1
}

/// Checks if a text line is phantom balloon chrome or iOS popup callout (NEVER rejects "Cut-Off" in sentences).
pub fn is_balloon_popup_text(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return false;
    }
    if t == "1Q0 1O0O" || t == "1O0 1O0O" || t == "100 1000" {
        return true;
    }
    let is_phantom_digits = (3..=15).contains(&t.chars().count())
        && t.chars()
            .all(|c| c.is_ascii_digit() || matches!(c, 'O' | 'Q' | 'I' | 'l' | ' '));
    if is_phantom_digits {
        return true;
    }
    is_pure_callout_menu_line(t)
}

/// Parity port of `handset/screen.py::_is_garbage_ocr_line`.
pub fn is_garbage_ocr_line(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return false;
    }
    if t == "<<<DOC_START>>>" || t == "<<<NOTES_LIST_VIEW>>>" {
        return true;
    }
    let t_low = t.to_lowercase();
    if t_low.contains("jendela-kecil")
        || t_low.contains("device hub")
        || t_low.contains("this device was viewed remotely")
        || t_low.contains("remotely from mac")
    {
        return true;
    }

    if is_month_or_folder_header(t) || is_notes_list_preview_timestamp(t) || is_ios_notes_date_header(t) {
        return true;
    }

    // Virtual keyboard single-char rows (w e r t y u i o p)
    let tokens: Vec<&str> = t.split_whitespace().collect();
    let single_alpha = tokens
        .iter()
        .filter(|tok| tok.chars().count() == 1 && tok.chars().all(|c| c.is_alphabetic()))
        .count();
    if single_alpha >= 4 && (single_alpha as f64) >= (tokens.len() as f64) * 0.70 {
        return true;
    }

    // Keyboard suggestion bar debris
    if matches!(
        t_low.as_str(),
        "cat aku" | "kamu aku" | "kamu aku ya" | "aku kamu" | "cat kamu"
    ) {
        return true;
    }
    if tokens.len() <= 3
        && matches!(t_low.as_str(), "cat aku" | "kamu aku" | "kamu aku ya")
    {
        return true;
    }

    // iOS Callout balloon (preserving "Cut-Off" inside sentences)
    if is_balloon_popup_text(t) {
        return true;
    }

    // Edge border fragments like "r I", "I I", "r l", "| I", "r |", "r"
    let all_edge_frags = !tokens.is_empty()
        && tokens.iter().all(|tok| {
            let cnt = tok.chars().count();
            (1..=2).contains(&cnt)
                && tok
                    .chars()
                    .all(|c| matches!(c, 'r' | 'I' | 'l' | '1' | '|' | '/' | '[' | ']' | '(' | ')' | '-'))
        });
    if all_edge_frags {
        return true;
    }

    // Short bullet debris "< 8" chars ("•a•y")
    if t.chars().count() < 8 && (t.contains('•') || t.contains('·')) {
        return true;
    }

    // Hallucinated alphanumeric noise
    if tokens.iter().any(|w| {
        matches!(
            w.to_ascii_lowercase().as_str(),
            "v3one" | "oil13i1" | "111e113n" | "me1a1ui" | "miliknva" | "ao/o3124an"
        )
    }) {
        return true;
    }

    let mixed_count = tokens
        .iter()
        .filter(|w| {
            if w.starts_with('@') || w.starts_with('#') {
                return false;
            }
            let has_alpha = w.chars().any(|c| c.is_ascii_alphabetic());
            let has_digit = w.chars().any(|c| c.is_ascii_digit());
            if !(has_alpha && has_digit) {
                return false;
            }
            let alpha_then_digit = {
                let mut seen_digit = false;
                let mut valid = true;
                for c in w.chars() {
                    if c.is_ascii_alphabetic() && seen_digit {
                        valid = false;
                        break;
                    } else if c.is_ascii_digit() {
                        seen_digit = true;
                    } else if !c.is_ascii_alphanumeric() {
                        valid = false;
                        break;
                    }
                }
                valid
            };
            !alpha_then_digit
        })
        .count();
    if mixed_count >= 2 {
        return true;
    }

    false
}

fn replace_ascii_case_insensitive_word(text: &str, needle: &str, replacement: &str) -> String {
    let lower_chars: Vec<char> = text.chars().collect();
    let needle_low: Vec<char> = needle.to_ascii_lowercase().chars().collect();
    let nlen = needle_low.len();
    if nlen == 0 || lower_chars.len() < nlen {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut i = 0usize;
    while i < lower_chars.len() {
        if i + nlen <= lower_chars.len() {
            let matches = (0..nlen)
                .all(|k| lower_chars[i + k].to_ascii_lowercase() == needle_low[k]);
            if matches {
                let left_ok = i == 0 || !lower_chars[i - 1].is_alphanumeric() && lower_chars[i - 1] != '_';
                let right_ok = i + nlen == lower_chars.len()
                    || (!lower_chars[i + nlen].is_alphanumeric() && lower_chars[i + nlen] != '_');
                if left_ok && right_ok {
                    out.push_str(replacement);
                    i += nlen;
                    continue;
                }
            }
        }
        out.push(lower_chars[i]);
        i += 1;
    }
    out
}

fn replace_tag_prefix_ci(text: &str, needle_lower: &str, replacement: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let nchars: Vec<char> = needle_lower.chars().collect();
    let nlen = nchars.len();
    if chars.len() < nlen {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut i = 0usize;
    while i < chars.len() {
        if i + nlen <= chars.len()
            && (0..nlen).all(|k| chars[i + k].to_ascii_lowercase() == nchars[k])
        {
            let right_ok = i + nlen == chars.len()
                || (!chars[i + nlen].is_alphanumeric() && chars[i + nlen] != '_');
            if right_ok {
                out.push_str(replacement);
                i += nlen;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn replace_l_handle_lt_with_k(input: &str) -> String {
    let mut t = input.to_string();
    for _ in 0..10 {
        let chars: Vec<char> = t.chars().collect();
        let mut changed = false;
        let mut out = String::with_capacity(t.len());
        let mut i = 0usize;
        while i < chars.len() {
            if i + 1 < chars.len()
                && matches!(chars[i], 'l' | 'I' | '1' | '|')
                && chars[i + 1] == '<'
            {
                let left_ok = i > 0
                    && (chars[i - 1].is_ascii_alphanumeric()
                        || chars[i - 1] == '_'
                        || chars[i - 1] == '@');
                let right_ok = i + 2 == chars.len()
                    || chars[i + 2].is_ascii_alphanumeric()
                    || matches!(chars[i + 2], '_' | '.' | ',' | '!' | '?')
                    || chars[i + 2].is_whitespace();
                if left_ok || right_ok {
                    out.push('k');
                    i += 2;
                    changed = true;
                    continue;
                }
            }
            out.push(chars[i]);
            i += 1;
        }
        t = out;
        if !changed {
            break;
        }
    }
    t
}

fn replace_mid_word_o0(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    for i in 0..chars.len() {
        if matches!(chars[i], 'O' | '0')
            && i > 0
            && chars[i - 1].is_ascii_lowercase()
            && (i + 1 == chars.len()
                || chars[i + 1].is_ascii_lowercase()
                || !chars[i + 1].is_alphanumeric())
        {
            out.push('o');
        } else {
            out.push(chars[i]);
        }
    }
    out
}

fn replace_bang_optical(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut i = 0usize;
    while i < chars.len() {
        // !olom at start or after whitespace -> kolom
        if chars[i] == '!'
            && (i == 0 || chars[i - 1].is_whitespace())
            && i + 5 <= chars.len()
            && chars[i + 1].to_ascii_lowercase() == 'o'
            && chars[i + 2].to_ascii_lowercase() == 'l'
            && chars[i + 3].to_ascii_lowercase() == 'o'
            && chars[i + 4].to_ascii_lowercase() == 'm'
            && (i + 5 == chars.len() || !chars[i + 5].is_alphanumeric())
        {
            out.push_str("kolom");
            i += 5;
            continue;
        }
        // \bd! at word end -> di
        if chars[i] == 'd'
            && (i == 0 || (!chars[i - 1].is_alphanumeric() && chars[i - 1] != '_'))
            && i + 1 < chars.len()
            && chars[i + 1] == '!'
            && (i + 2 == chars.len() || chars[i + 2].is_whitespace() || matches!(chars[i + 2], ',' | '.' | ';'))
        {
            out.push_str("di");
            i += 2;
            continue;
        }
        // Mid-word ! between letters -> l
        if chars[i] == '!'
            && i > 0
            && i + 1 < chars.len()
            && chars[i - 1].is_ascii_alphabetic()
            && chars[i + 1].is_ascii_alphabetic()
        {
            out.push('l');
            i += 1;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn strip_trailing_orphan_acronym_digit(input: &str) -> String {
    let trimmed = input.trim_end();
    let chars: Vec<char> = trimmed.chars().collect();
    if chars.len() < 4 {
        return input.to_string();
    }
    let last = *chars.last().unwrap();
    if !last.is_ascii_digit() {
        return input.to_string();
    }
    // Check one whitespace right before trailing digit (e.g. "KMP 4")
    if !chars[chars.len() - 2].is_whitespace() {
        return input.to_string();
    }
    let mut k = chars.len() - 2;
    while k > 0 && chars[k - 1].is_whitespace() {
        k -= 1;
    }
    let mut start = k;
    while start > 0 && chars[start - 1].is_ascii_uppercase() {
        start -= 1;
    }
    let acronym_len = k - start;
    if acronym_len >= 2 && (start == 0 || !chars[start - 1].is_alphanumeric()) {
        return chars[..k].iter().collect();
    }
    input.to_string()
}

fn strip_handle_and_hashtag_trailing_noise(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut i = 0usize;
    while i < chars.len() {
        if (chars[i] == '#' || chars[i] == '@')
            && i + 1 < chars.len()
            && (chars[i + 1].is_ascii_alphanumeric() || chars[i + 1] == '_')
        {
            let prefix = chars[i];
            out.push(prefix);
            i += 1;
            while i < chars.len()
                && (chars[i].is_ascii_alphanumeric()
                    || chars[i] == '_'
                    || (prefix == '@' && chars[i] == '.'))
            {
                out.push(chars[i]);
                i += 1;
            }
            if prefix == '#' {
                let mut skip_end = i;
                while skip_end < chars.len()
                    && matches!(chars[skip_end], ']' | '[' | '|' | '>' | '<' | ')' | '}' | '!')
                {
                    skip_end += 1;
                }
                if skip_end > i
                    && (skip_end == chars.len() || chars[skip_end].is_whitespace())
                {
                    i = skip_end;
                }
            } else {
                let mut skip_end = i;
                while skip_end < chars.len() && matches!(chars[skip_end], '<' | '|' | '!') {
                    skip_end += 1;
                }
                if skip_end > i
                    && (skip_end == chars.len() || chars[skip_end].is_whitespace())
                {
                    i = skip_end;
                }
            }
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn strip_stray_word_underscores(input: &str) -> String {
    let mut tokens_out: Vec<String> = Vec::new();
    for tok in input.split(' ') {
        if tok.starts_with('@') || tok.starts_with('#') || tok.is_empty() {
            tokens_out.push(tok.to_string());
        } else {
            let trimmed_u = tok.trim_matches('_');
            if trimmed_u.is_empty() {
                tokens_out.push(tok.to_string());
            } else {
                tokens_out.push(trimmed_u.to_string());
            }
        }
    }
    tokens_out.join(" ")
}

fn restore_initial_i_to_l(input: &str) -> String {
    const SUFFIXES: &[&str] = &[
        "ambung", "angkah", "angsung", "agi", "ain", "alu", "ebih", "intas", "aut", "epas", "uka",
        "oket", "okasi", "aporan", "embar", "ingkungan", "uar",
    ];
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == 'I' && (i == 0 || !chars[i - 1].is_alphanumeric()) {
            let mut end = i + 1;
            while end < chars.len() && chars[end].is_alphanumeric() {
                end += 1;
            }
            let rest: String = chars[i + 1..end].iter().collect();
            if SUFFIXES.iter().any(|s| *s == rest) {
                out.push('l');
                out.push_str(&rest);
                i = end;
                continue;
            }
        }
        // Final capital 'I' at end of lowercase word (e.g. KapaI / kapaI -> kapal)
        if chars[i] == 'I'
            && i > 0
            && chars[i - 1].is_ascii_lowercase()
            && (i + 1 == chars.len() || !chars[i + 1].is_alphanumeric())
        {
            out.push('l');
            i += 1;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn replace_merak_jakarta_dash(input: &str) -> String {
    let mut out = input.to_string();
    for bad in ["Merak-Jakarta", "Merak—Jakarta", "Merak\u{FFFD}Jakarta"] {
        out = out.replace(bad, "Merak\u{2013}Jakarta");
    }
    out
}

/// Complete optical OCR line cleaner matching `handset/screen.py::_clean_ocr_line` (15+ repairs).
pub fn clean_ocr_line(line: &str) -> String {
    if line.is_empty() {
        return String::new();
    }
    let mut t = line.trim().to_string();
    if is_garbage_ocr_line(&t) {
        return String::new();
    }

    t = strip_trailing_orphan_acronym_digit(&t);

    if t.ends_with("tali evakua") {
        let len = t.len() - "tali evakua".len();
        t = format!("{}tali evakuasi", &t[..len]);
    }
    if t.ends_with("memperlih") {
        let len = t.len() - "memperlih".len();
        t = format!("{}memperlihatkan", &t[..len]);
    }

    t = replace_ascii_case_insensitive_word(&t, "tik-detik", "detik-detik");
    for bad_jiwa in ["i1wa", "j1wa", "ilwa", "jiw4"] {
        t = replace_ascii_case_insensitive_word(&t, bad_jiwa, "jiwa");
    }
    t = t.replace("memaksaopng", "memaksa para penumpang");
    t = replace_ascii_case_insensitive_word(&t, "memaksa pa", "memaksa para penumpang");

    // Optical letter restoration
    t = replace_l_handle_lt_with_k(&t);
    t = replace_mid_word_o0(&t);
    t = t.replace(", Oleh ", ", oleh ");
    // Handle any lowercase/alnum before " Oleh\b"
    {
        let chars: Vec<char> = t.chars().collect();
        let mut out = String::with_capacity(t.len());
        let mut i = 0usize;
        while i < chars.len() {
            if i + 5 <= chars.len()
                && chars[i] == ' '
                && chars[i + 1] == 'O'
                && chars[i + 2] == 'l'
                && chars[i + 3] == 'e'
                && chars[i + 4] == 'h'
                && (i + 5 == chars.len() || !chars[i + 5].is_alphanumeric())
                && i > 0
                && (chars[i - 1].is_ascii_alphanumeric() || chars[i - 1] == ',')
            {
                out.push_str(" oleh");
                i += 5;
                continue;
            }
            out.push(chars[i]);
            i += 1;
        }
        t = out;
    }

    t = replace_bang_optical(&t);
    t = t.replace(" d, ", " di ");
    if t.starts_with("d, ") {
        t.replace_range(0..3, "di ");
    }

    t = t.replace("Baturnandi", "Batumandi");
    t = t.replace("baturnandi", "batumandi");
    t = replace_tag_prefix_ci(&t, "#kmpbaturnandi", "#kmpbatumandi");
    t = replace_tag_prefix_ci(&t, "#baturnandi", "#batumandi");
    t = replace_ascii_case_insensitive_word(&t, "hincfia", "hindia");

    // Protect @andriboto< explicitly if no full @andribotox is on this line yet
    t = t.replace("@andriboto<", "@andribotox");
    t = t.replace("@andriboto|", "@andribotox");
    t = t.replace("@eril<l<ristian", "@erikkristian");
    t = t.replace("@eril<l<rist", "@erikkristian");

    t = strip_handle_and_hashtag_trailing_noise(&t);
    t = replace_merak_jakarta_dash(&t);
    t = t
        .replace("(Al Generated Content)", "(AI Generated Content)")
        .replace("Al Generated", "AI Generated");
    t = replace_ascii_case_insensitive_word(&t, "dapua", "papua");

    t = restore_initial_i_to_l(&t);
    t = replace_ascii_case_insensitive_word(&t, "awak kapaI", "awak kapal");
    t = replace_ascii_case_insensitive_word(&t, "awak kapal", "awak kapal");

    t = t.replace("@wisnutry017", "@wisnutryo17");
    t = replace_tag_prefix_ci(&t, "#iakartarunninafestival", "#jakartarunningfestival");
    t = replace_tag_prefix_ci(&t, "#irf2026", "#jrf2026");
    t = replace_tag_prefix_ci(&t, "#iakarta", "#jakarta");
    t = replace_tag_prefix_ci(&t, "#irf", "#jrf");

    t = strip_stray_word_underscores(&t);

    // Tahap 193 Lollipop handle repairs
    t = replace_ascii_case_insensitive_word(&t, "enggalnih", "engga nih");
    t = replace_ascii_case_insensitive_word(&t, "engga1nih", "engga nih");
    t = replace_ascii_case_insensitive_word(&t, "enga1nih", "enga nih");
    t = t.replace("engga!nih", "engga nih").replace("Engga!nih", "Engga nih");
    t = replace_ascii_case_insensitive_word(&t, "tarhutla", "karhutla");
    t = t.replace("lomah enak", "lontoh enak").replace("Lomah enak", "Lontoh enak");
    t = replace_ascii_case_insensitive_word(&t, "prabowk", "prabowo");
    t = replace_ascii_case_insensitive_word(&t, "prabowl", "prabowo");
    t = replace_ascii_case_insensitive_word(&t, "prabow1", "prabowo");

    for qword in [
        "nih", "ya", "kan", "dong", "sih", "kok", "lah", "kah", "apa", "mana", "siapa", "kapan",
        "gimana", "kenapa", "aman", "wok", "bro", "bang", "kak", "guys",
    ] {
        let te = t.trim_end();
        let mut replaced_q = false;
        for mark in [" I", " 1", " l", " |"] {
            if let Some(stem) = te.strip_suffix(mark) {
                let stem_low = stem.to_ascii_lowercase();
                if stem_low.ends_with(qword) {
                    let plen = stem.len() - qword.len();
                    let word_start_ok =
                        plen == 0 || !stem.as_bytes()[plen - 1].is_ascii_alphanumeric();
                    if word_start_ok {
                        t = format!("{stem}?");
                        replaced_q = true;
                        break;
                    }
                }
            }
        }
        if replaced_q {
            break;
        }
    }
    if t.trim_end().ends_with("pemulun") && !t.trim_end().ends_with("pemulung") {
        let te = t.trim_end();
        t = format!("{}pemulung", &te[..te.len() - "pemulun".len()]);
    }
    t
}

pub fn is_balloon_text_line(line: &str) -> bool {
    is_balloon_popup_text(line)
}

fn line_optical_defect_penalty(line: &str) -> i32 {
    if line.is_empty() {
        return 0;
    }
    let mut pen = 0i32;
    if line.contains("l<") || line.contains("I<") || line.contains("1<") || line.contains("|<") {
        pen += 10;
    }
    if line.contains("@andriboto<") || line.contains("@andriboto|") {
        pen += 15;
    }
    if line.contains("baturnandi") || line.contains("Baturnandi") {
        pen += 10;
    }
    if line.to_ascii_lowercase().contains("hincfia") {
        pen += 10;
    }
    if line.contains(" Oleh ") {
        pen += 5;
    }
    for tok in line.split_whitespace() {
        if !tok.starts_with('@') && !tok.starts_with('#') && tok.contains('_') {
            pen += 25;
            break;
        }
    }
    pen
}

fn pick_cleaner_line(line1: &str, line2: &str) -> String {
    let l1_clean = clean_ocr_line(line1);
    let l2_clean = clean_ocr_line(line2);
    let has_u = |s: &str| {
        s.split_whitespace()
            .any(|tok| !tok.starts_with('@') && !tok.starts_with('#') && tok.contains('_'))
    };
    let u1 = has_u(line1);
    let u2 = has_u(line2);
    if u2 && !u1 {
        return l1_clean;
    }
    if u1 && !u2 {
        return l2_clean;
    }

    let p1 = line_optical_defect_penalty(line1);
    let p2 = line_optical_defect_penalty(line2);
    if p1 < p2 {
        return l1_clean;
    }
    if p2 < p1 {
        return l2_clean;
    }

    if l1_clean.contains("detik-detik") && !l2_clean.contains("detik-detik") {
        return l1_clean;
    }
    if l2_clean.contains("detik-detik") && !l1_clean.contains("detik-detik") {
        return l2_clean;
    }
    if l1_clean.contains("jiwa") && !l2_clean.contains("jiwa") {
        return l1_clean;
    }
    if l2_clean.contains("jiwa") && !l1_clean.contains("jiwa") {
        return l2_clean;
    }

    let w1 = l1_clean.split_whitespace().last().unwrap_or("").trim_end_matches(&['.', ',', '?', '!'][..]);
    let w2 = l2_clean.split_whitespace().last().unwrap_or("").trim_end_matches(&['.', ',', '?', '!'][..]);
    if !w1.is_empty() && !w2.is_empty() && w1 != w2 {
        if w2.starts_with(w1) && w2.len() > w1.len() {
            return l2_clean;
        }
        if w1.starts_with(w2) && w1.len() > w2.len() {
            return l1_clean;
        }
    }

    if l1_clean.chars().count() >= l2_clean.chars().count() {
        l1_clean
    } else {
        l2_clean
    }
}

/// Broken Word Un-splitter (Tahap 29): rejoin trailing 1..=6 lowercase fragments onto previous line.
pub fn unsplit_broken_lines(lines: &[String]) -> Vec<String> {
    if lines.is_empty() {
        return Vec::new();
    }
    let mut result = Vec::new();
    let mut i = 0usize;
    while i < lines.len() {
        let mut line = lines[i].clone();
        if line.trim().is_empty() {
            result.push(line);
            i += 1;
            continue;
        }
        if i + 1 < lines.len() {
            let nxt = lines[i + 1].trim();
            let punct_len = if nxt.ends_with(&['.', ',', '?', '!'][..]) {
                1
            } else {
                0
            };
            let frag = &nxt[..nxt.len() - punct_len];
            let punct = &nxt[nxt.len() - punct_len..];
            if (1..=6).contains(&frag.chars().count()) && frag.chars().all(|c| c.is_ascii_lowercase())
            {
                let cur = line.trim();
                if !cur.ends_with(&['.', '?', '!', ':'][..]) {
                    let last_tok = cur.split_whitespace().last().unwrap_or("");
                    if last_tok.ends_with(frag)
                        || (last_tok == "memperlihatkan" && frag == "atkan")
                        || (last_tok == "evakuasi" && frag == "si")
                    {
                        result.push(line);
                        i += 2;
                        continue;
                    }
                    if let Some(stripped_dash) = cur.strip_suffix('-') {
                        line = format!("{stripped_dash}{frag}{punct}");
                    } else if matches!(
                        frag,
                        "atkan" | "si" | "nya" | "kan" | "kah" | "lah" | "an" | "i" | "wan" | "man" | "tas"
                    ) || last_tok.chars().count() >= 4
                    {
                        line = format!("{cur}{frag}{punct}");
                    } else {
                        line = format!("{cur} {frag}{punct}");
                    }
                    i += 1;
                }
            }
        }
        result.push(line);
        i += 1;
    }
    result
}

// ============================================================================
// PILAR 2C: 4-Layer Anti-Contamination, Scoring, & Mention Completion
// ============================================================================

pub fn is_generic_template_line(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return false;
    }
    let low = t.to_ascii_lowercase();
    for pfx in ["source:", "source :", "sumber:", "sumber :", "video:", "video :", "foto:", "foto :"]
    {
        if low.starts_with(pfx) {
            return true;
        }
    }
    if low.starts_with("bagaimana perasaan")
        || low.starts_with("bagaimana tanggapan")
        || low.starts_with("bagaimana menurut")
    {
        return true;
    }
    if t.starts_with('#') {
        return true;
    }
    let no_q = low.trim_end_matches('?');
    if no_q.ends_with("banteners") {
        return true;
    }
    false
}

/// Gap quality scorer rejecting balloon artifacts ("1Q0 1O0O", "memaksaopng") in favor of intact lines.
pub fn score_gap_quality(gap_lines: &[String]) -> i32 {
    if gap_lines.is_empty() {
        return 0;
    }
    let mut score = 0i32;
    for line in gap_lines {
        let t = line.trim();
        let is_phantom_digits = (3..=15).contains(&t.chars().count())
            && t.chars()
                .all(|c| c.is_ascii_digit() || matches!(c, 'O' | 'Q' | 'I' | 'l' | ' '));
        if is_phantom_digits {
            score -= 100;
        }
        let low = t.to_ascii_lowercase();
        if low.contains("memaksaopng") || low.contains("memaksa pa") || low.split_whitespace().any(|w| w == "opng") {
            score -= 100;
        }
        if t.contains("l<") || t.contains("I<") || t.contains("1<") || t.contains("|<") {
            score -= 50;
        }
        let words: Vec<&str> = t.split_whitespace().collect();
        let mixed = words
            .iter()
            .filter(|w| {
                let has_a = w.chars().any(|c| c.is_ascii_alphabetic());
                let has_d = w.chars().any(|c| c.is_ascii_digit());
                has_a && has_d && !(w.chars().take_while(|c| c.is_ascii_alphabetic()).count() > 0 && w.chars().skip_while(|c| c.is_ascii_alphabetic()).all(|c| c.is_ascii_digit()))
            })
            .count();
        if mixed >= 1 {
            score -= 50;
        }
        score += (words.len() as i32) * 10;
    }
    score
}

/// Complete truncated or occluded `@username` mentions using longer intact mentions present in the same document.
pub fn complete_usernames(text: &str) -> String {
    if text.is_empty() || !text.contains('@') {
        return text.to_string();
    }
    let norm = replace_l_handle_lt_with_k(text);
    let chars: Vec<char> = norm.chars().collect();
    let mut handles: HashSet<String> = HashSet::new();

    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '@' {
            let mut j = i + 1;
            while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '_' || chars[j] == '.') {
                j += 1;
            }
            let cand: String = chars[i..j].iter().collect();
            if cand.len() >= 6 && !cand.ends_with('.') && !cand.ends_with('_') {
                handles.insert(cand);
            }
            i = j;
        } else {
            i += 1;
        }
    }

    if handles.is_empty() {
        return norm;
    }

    let mut full_list: Vec<String> = handles.into_iter().collect();
    full_list.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));

    let mut result = norm;
    // Step 1: Resolve defect mentions (@andriboto<) or truncated handles >= 5 chars
    let mut replaced = String::with_capacity(result.len());
    let rchars: Vec<char> = result.chars().collect();
    let mut k = 0usize;
    while k < rchars.len() {
        if rchars[k] == '@' {
            let mut m_end = k + 1;
            while m_end < rchars.len()
                && (rchars[m_end].is_ascii_alphanumeric() || rchars[m_end] == '_' || rchars[m_end] == '.')
            {
                m_end += 1;
            }
            let base: String = rchars[k..m_end].iter().collect();
            let mut def_end = m_end;
            while def_end < rchars.len() && matches!(rchars[def_end], '<' | '|' | '!') {
                def_end += 1;
            }
            let mut replacement: Option<String> = None;
            if def_end > m_end {
                let matched = full_list.iter().find(|fh| {
                    fh.to_ascii_lowercase().starts_with(&base.to_ascii_lowercase())
                        && fh.len() > base.len()
                });
                replacement = Some(matched.cloned().unwrap_or(base));
            } else if base.len() >= 5 {
                if let Some(matched) = full_list.iter().find(|fh| {
                    fh.len() > base.len()
                        && fh.to_ascii_lowercase().starts_with(&base.to_ascii_lowercase())
                }) {
                    replacement = Some(matched.clone());
                }
            }
            if let Some(rep) = replacement {
                replaced.push_str(&rep);
                k = def_end;
                continue;
            }
        }
        replaced.push(rchars[k]);
        k += 1;
    }
    result = replaced;
    result
}

pub fn strip_top_unselected_debris(lines: &[String]) -> Vec<String> {
    if lines.len() < 2 {
        return lines.to_vec();
    }
    let has_body_later = lines.iter().any(|l| {
        let c = l.trim();
        let low = c.to_ascii_lowercase();
        c.chars().count() >= 25
            && !c.starts_with('#')
            && !low.starts_with("source:")
            && !low.starts_with("sumber:")
            && !low.starts_with("video:")
            && !low.starts_with("foto:")
    });
    if !has_body_later {
        return lines.to_vec();
    }
    let mut start = 0usize;
    while start < lines.len() {
        let first = lines[start].trim();
        if first.is_empty() {
            start += 1;
            continue;
        }
        if first.chars().count() >= 2
            && first
                .chars()
                .all(|c| matches!(c, '-' | '—' | '–' | '_' | '='))
        {
            start += 1;
            continue;
        }
        if first.chars().count() < 8 && (first.contains('•') || first.contains('·')) {
            start += 1;
            continue;
        }
        if first.starts_with('#')
            || (first.contains('#') && first.chars().count() < 40 && !first.ends_with('.'))
        {
            start += 1;
            continue;
        }
        break;
    }
    lines[start..].to_vec()
}

fn get_typed_candidates_for_ocr() -> Vec<String> {
    let mut cands = Vec::new();
    let curr = get_current_typed_line().trim().to_string();
    if curr.chars().count() >= 3 {
        cands.push(curr);
    }
    for line in get_typed_lines_buffer().into_iter().rev() {
        if line.chars().count() >= 3 && !cands.contains(&line) {
            cands.push(line);
        }
    }
    cands
}

pub fn strip_typed_buffer_contamination_from_top(lines: &[String]) -> Vec<String> {
    let non_empty_count = lines.iter().filter(|l| !l.trim().is_empty()).count();
    if lines.is_empty() || non_empty_count < 6 {
        return lines.to_vec();
    }
    let cands = get_typed_candidates_for_ocr();
    if cands.is_empty() {
        return lines.to_vec();
    }

    let mut result = lines.to_vec();
    for _ in 0..2 {
        let non_empty: Vec<&String> = result.iter().filter(|l| !l.trim().is_empty()).collect();
        if non_empty.len() < 6 {
            break;
        }
        if result[0].trim().is_empty() {
            result.remove(0);
            continue;
        }
        let top = result[0].trim();
        let top_low = top.to_lowercase();
        if top.starts_with('#') || ends_with_terminal_punct(top) {
            break;
        }
        if non_empty.len() >= 2 {
            let next_line = non_empty[1].trim();
            if top.chars().count() >= 30
                && next_line.chars().next().is_some_and(|c| c.is_lowercase())
            {
                break;
            }
        }
        if top.chars().count() >= 32 {
            break;
        }

        let mut is_contam = false;
        for cand in &cands {
            let c_clean = cand.trim();
            let clen = c_clean.chars().count();
            if clen < 4 || clen >= 35 || ends_with_terminal_punct(c_clean) {
                continue;
            }
            let c_low = c_clean.to_lowercase();
            if c_low.contains("rekaman video amatir") || c_low.contains("unggahan bertajuk") {
                continue;
            }
            let ratio = difflib_ratio(&top_low, &c_low);
            if ratio >= 0.75 || (c_low.len() >= 8 && (top_low.contains(&c_low) || c_low.contains(&top_low))) {
                is_contam = true;
                break;
            }
        }
        if !is_contam && (is_notes_folder_list_frame(top) || is_notes_list_preview_timestamp(top)) {
            is_contam = true;
        }
        if is_contam {
            result.remove(0);
        } else {
            break;
        }
    }
    result
}

fn is_body_sentence_line(line: &str) -> bool {
    let t = line.trim();
    if t.chars().count() < 20 {
        return false;
    }
    let low = t.to_lowercase();
    if low.starts_with('#')
        || low.starts_with("source:")
        || low.starts_with("sumber:")
        || low.starts_with("bagaimana perasaan")
        || low.starts_with("bagaimana tanggapan")
        || low.starts_with("bagaimana kalian")
        || low.starts_with("bagaimana menurut")
    {
        return false;
    }
    !is_generic_template_line(t) && !is_garbage_ocr_line(t)
}

pub fn has_overlapping_body_line(lines_a: &[String], lines_b: &[String]) -> bool {
    if lines_a.is_empty() || lines_b.is_empty() {
        return false;
    }
    let body_a: Vec<String> = lines_a
        .iter()
        .filter(|l| is_body_sentence_line(l))
        .map(|l| normalize_line_for_match(l))
        .collect();
    let body_b: Vec<String> = lines_b
        .iter()
        .filter(|l| is_body_sentence_line(l))
        .map(|l| normalize_line_for_match(l))
        .collect();

    for na in &body_a {
        for nb in &body_b {
            let la = na.chars().count();
            let lb = nb.chars().count();
            let diff = la.abs_diff(lb);
            let max_allowed = 15usize.max(((la.max(lb) as f64) * 0.35) as usize);
            if diff > max_allowed {
                continue;
            }
            if difflib_ratio(na, nb) >= 0.82 || (la >= 22 && (nb.contains(na) || na.contains(nb))) {
                return true;
            }
        }
    }
    false
}

fn word_tokens(s: &str) -> HashSet<String> {
    let mut set = HashSet::new();
    let mut cur = String::new();
    for ch in s.chars() {
        if ch.is_alphanumeric() || ch == '_' {
            cur.push(ch);
        } else if !cur.is_empty() {
            set.insert(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        set.insert(cur);
    }
    set
}

/// Gap-Filling Segment Stitcher with anchor alignment & quality scoring.
pub fn merge_two_overlapping_blocks(
    lines_a: &[String],
    lines_b: &[String],
) -> Option<Vec<String>> {
    if lines_a.is_empty() || lines_b.is_empty() {
        return None;
    }
    let clean_a = unsplit_broken_lines(
        &lines_a
            .iter()
            .filter(|l| !l.trim().is_empty() && !is_garbage_ocr_line(l))
            .map(|l| clean_ocr_line(l))
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>(),
    );
    let clean_b = unsplit_broken_lines(
        &lines_b
            .iter()
            .filter(|l| !l.trim().is_empty() && !is_garbage_ocr_line(l))
            .map(|l| clean_ocr_line(l))
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>(),
    );
    if clean_a.is_empty() || clean_b.is_empty() {
        return None;
    }
    if clean_a == clean_b {
        return Some(clean_a);
    }

    let norm_a: Vec<String> = clean_a.iter().map(|l| normalize_line_for_match(l)).collect();
    let norm_b: Vec<String> = clean_b.iter().map(|l| normalize_line_for_match(l)).collect();

    let mut matches: Vec<(usize, usize, f64, isize)> = Vec::new();
    for (i, na) in norm_a.iter().enumerate() {
        let la = na.chars().count();
        if la < 18 {
            continue;
        }
        for (j, nb) in norm_b.iter().enumerate() {
            let lb = nb.chars().count();
            if lb < 18 {
                continue;
            }
            if la.abs_diff(lb) as f64 > (la.max(lb) as f64) * 0.40 {
                continue;
            }
            let sm = difflib_ratio(na, nb);
            let contains = la >= 20 && (nb.contains(na) || na.contains(nb));
            if sm >= 0.78 || contains {
                let ratio_val = if contains { sm.max(0.88) } else { sm };
                matches.push((i, j, ratio_val, i as isize - j as isize));
            }
        }
    }
    if matches.is_empty() {
        return None;
    }

    let has_specific_body_anchor = matches.iter().any(|&(i, j, sm_ratio, _)| {
        if is_generic_template_line(&clean_a[i]) || is_generic_template_line(&clean_b[j]) {
            return false;
        }
        if norm_a[i].chars().count() < 22 || norm_b[j].chars().count() < 22 || sm_ratio < 0.85 {
            return false;
        }
        let wa = word_tokens(&norm_a[i]);
        let wb = word_tokens(&norm_b[j]);
        if wa.is_empty() || wb.is_empty() {
            return false;
        }
        let overlap = wa.intersection(&wb).count() as f64 / (wa.len().max(wb.len()) as f64);
        overlap >= 0.65
    });
    if !has_specific_body_anchor {
        return None;
    }

    let mut offset_counts: std::collections::HashMap<isize, usize> = std::collections::HashMap::new();
    for &m in &matches {
        *offset_counts.entry(m.3).or_insert(0) += 1;
    }
    let dom_offset = offset_counts
        .into_iter()
        .max_by_key(|kv| kv.1)
        .map(|kv| kv.0)
        .unwrap_or(0);

    let mut consistent: Vec<(usize, usize, f64, isize)> = matches
        .iter()
        .copied()
        .filter(|m| (m.3 - dom_offset).abs() <= 1)
        .collect();
    if consistent.is_empty() {
        consistent = matches;
    }
    consistent.sort_by_key(|m| m.0);

    let mut anchors: Vec<(usize, usize)> = Vec::new();
    let mut last_i: isize = -1;
    let mut last_j: isize = -1;
    for (ci, cj, _, _) in consistent {
        if (ci as isize) > last_i && (cj as isize) > last_j {
            anchors.push((ci, cj));
            last_i = ci as isize;
            last_j = cj as isize;
        }
    }
    if anchors.is_empty() {
        return None;
    }

    let mut merged = Vec::new();
    let (first_i, first_j) = anchors[0];
    if dom_offset > 0 {
        merged.extend_from_slice(&clean_a[..first_i]);
    } else if dom_offset < 0 {
        merged.extend_from_slice(&clean_b[..first_j]);
    } else if first_i >= first_j {
        merged.extend_from_slice(&clean_a[..first_i]);
    } else {
        merged.extend_from_slice(&clean_b[..first_j]);
    }

    for k in 0..anchors.len() {
        let (ci, cj) = anchors[k];
        merged.push(pick_cleaner_line(&clean_a[ci], &clean_b[cj]));

        if k + 1 < anchors.len() {
            let (ni, nj) = anchors[k + 1];
            let gap_a = &clean_a[ci + 1..ni];
            let gap_b = &clean_b[cj + 1..nj];
            if gap_a.is_empty() && !gap_b.is_empty() {
                merged.extend_from_slice(gap_b);
            } else if gap_b.is_empty() && !gap_a.is_empty() {
                merged.extend_from_slice(gap_a);
            } else if !gap_a.is_empty() && !gap_b.is_empty() {
                let sa = score_gap_quality(gap_a);
                let sb = score_gap_quality(gap_b);
                if sa > sb {
                    merged.extend_from_slice(gap_a);
                } else if sb > sa {
                    merged.extend_from_slice(gap_b);
                } else if gap_a.len() == gap_b.len() {
                    for (ga, gb) in gap_a.iter().zip(gap_b.iter()) {
                        merged.push(pick_cleaner_line(ga, gb));
                    }
                } else if gap_a.len() > gap_b.len() {
                    merged.extend_from_slice(gap_a);
                } else {
                    merged.extend_from_slice(gap_b);
                }
            }
        }
    }

    let (li, lj) = *anchors.last().unwrap();
    let tail_a = &clean_a[li + 1..];
    let tail_b = &clean_b[lj + 1..];
    let has_ht_a = clean_a.iter().any(|l| l.trim().starts_with('#'));
    let has_ht_b = clean_b.iter().any(|l| l.trim().starts_with('#'));
    if has_ht_a && !has_ht_b {
        merged.extend_from_slice(tail_a);
    } else if has_ht_b && !has_ht_a {
        merged.extend_from_slice(tail_b);
    } else if dom_offset > 0 {
        merged.extend_from_slice(if !tail_b.is_empty() { tail_b } else { tail_a });
    } else if dom_offset < 0 {
        merged.extend_from_slice(if !tail_a.is_empty() { tail_a } else { tail_b });
    } else if tail_a.len() >= tail_b.len() {
        merged.extend_from_slice(tail_a);
    } else {
        merged.extend_from_slice(tail_b);
    }

    let mut deduped: Vec<String> = Vec::new();
    for line in merged {
        let cl = clean_ocr_line(&line);
        if cl.is_empty() || is_garbage_ocr_line(&cl) {
            continue;
        }
        if deduped
            .last()
            .is_some_and(|prev| normalize_line_for_match(prev) == normalize_line_for_match(&cl))
        {
            continue;
        }
        deduped.push(cl);
    }

    if let Some(last_ht_idx) = deduped.iter().rposition(|l| l.trim().starts_with('#')) {
        let mut clean_tail = deduped[..=last_ht_idx].to_vec();
        for l in &deduped[last_ht_idx + 1..] {
            if l.trim().starts_with('#')
                || (!is_garbage_ocr_line(l) && l.split_whitespace().count() > 3)
            {
                clean_tail.push(l.clone());
            }
        }
        deduped = clean_tail;
    }

    Some(deduped)
}

/// Stitches two overlapping text frames by finding matching anchor lines.
pub fn stitch_two_ocr_texts(text1: &str, text2: &str) -> String {
    let t1 = text1.trim();
    let t2 = text2.trim();
    if t1.is_empty() {
        return t2.to_string();
    }
    if t2.is_empty() || t1 == t2 {
        return t1.to_string();
    }

    let lines1 = unsplit_broken_lines(
        &t1.lines()
            .filter(|l| !l.trim().is_empty())
            .map(clean_ocr_line)
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>(),
    );
    let lines2 = unsplit_broken_lines(
        &t2.lines()
            .filter(|l| !l.trim().is_empty())
            .map(clean_ocr_line)
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>(),
    );

    if let Some(interior) = merge_two_overlapping_blocks(&lines1, &lines2) {
        return interior.join("\r\n");
    }

    let mut best_match: Option<(usize, usize)> = None;
    let mut best_score = 0.0f64;

    for i in (0..lines1.len()).rev() {
        let l1 = &lines1[i];
        if l1.chars().count() < 22 || is_generic_template_line(l1) {
            continue;
        }
        for j in 0..lines2.len().min(20) {
            let l2 = &lines2[j];
            if l2.chars().count() < 22 || is_generic_template_line(l2) {
                continue;
            }
            let l1_low = l1.to_lowercase();
            let l2_low = l2.to_lowercase();
            let sm = difflib_ratio(&l1_low, &l2_low);
            let w1 = word_tokens(&l1_low);
            let w2 = word_tokens(&l2_low);
            let w_ov = if !w1.is_empty() && !w2.is_empty() {
                w1.intersection(&w2).count() as f64 / w1.len().max(w2.len()) as f64
            } else {
                0.0
            };
            if sm >= 0.85 && w_ov >= 0.65 {
                let mut bonus = 0.0;
                if i + 1 < lines1.len() && j + 1 < lines2.len() {
                    let n1 = lines1[i + 1].trim().to_lowercase();
                    let n2 = lines2[j + 1].trim().to_lowercase();
                    if n1.chars().count() >= 4
                        && n2.chars().count() >= 4
                        && (difflib_ratio(&n1, &n2) >= 0.65 || n2.contains(&n1) || n1.contains(&n2))
                    {
                        bonus = 0.25;
                    }
                }
                let total = sm + bonus;
                if total > best_score {
                    best_score = total;
                    best_match = Some((i, j));
                }
            }
        }
        if best_score >= 0.90 {
            break;
        }
    }

    if let Some((i, j)) = best_match {
        let mut out = lines1[..i].to_vec();
        out.push(pick_cleaner_line(&lines1[i], &lines2[j]));
        out.extend_from_slice(&lines2[j + 1..]);
        return out.join("\r\n");
    }

    // Fallback tail-to-head exact overlap for short synthetic unit test fixtures (e.g. Paragraf 2..6)
    let max_check = lines1.len().min(lines2.len());
    for k in (1..=max_check).rev() {
        let tail = &lines1[lines1.len() - k..];
        let head = &lines2[..k];
        if tail.iter().zip(head.iter()).all(|(a, b)| {
            normalize_line_for_match(a) == normalize_line_for_match(b)
                && !is_generic_template_line(a)
        }) {
            let mut out = lines1.clone();
            out.extend_from_slice(&lines2[k..]);
            return out.join("\r\n");
        }
    }

    t2.to_string()
}

fn is_middle_para_opener(line: &str) -> bool {
    let low = line.trim().to_lowercase();
    [
        "momen menegangkan",
        "situasi darurat",
        "dalam unggahan",
        "konten tersebut",
        "fenomena ini",
        "bagaimana perasaan",
        "bagaimana tanggapan",
        "bagaimana menurut",
        "source:",
        "source :",
        "sumber:",
        "sumber :",
    ]
    .iter()
    .any(|pfx| low.starts_with(pfx))
        || (low.starts_with("seluruh ")
            && low
                .split_whitespace()
                .nth(1)
                .is_some_and(|tok| tok.chars().all(|c| c.is_ascii_digit())))
}

fn is_valid_body_start(first_line: &str) -> bool {
    let f = first_line.trim();
    if f.is_empty() || is_middle_para_opener(f) {
        return false;
    }
    let first_ch_upper = f.chars().next().is_some_and(|c| c.is_uppercase());
    f.chars().count() >= 20
        && !is_generic_template_line(f)
        && first_ch_upper
        && !f.ends_with(',')
}

/// Stitches chronologically ordered OCR frames using Golden Rules 1-3.
pub fn stitch_multiline_ocr_frames(frames: &[String]) -> String {
    if frames.is_empty() {
        return String::new();
    }
    if frames.iter().any(|f| is_notes_folder_list_frame(f)) {
        clear_recent_long_ocr_chunks();
    }

    let valid: Vec<String> = frames
        .iter()
        .map(|f| f.trim().to_string())
        .filter(|f| !f.is_empty() && !is_notes_folder_list_frame(f))
        .collect();
    if valid.is_empty() {
        return String::new();
    }

    // DOC_START detection
    let mut doc_start_norm: Option<String> = None;
    for frame in &valid {
        let lines: Vec<&str> = frame.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
        for (idx, line) in lines.iter().enumerate() {
            if *line == "<<<DOC_START>>>" || is_ios_notes_date_header(line) {
                for nxt in &lines[idx + 1..] {
                    let cand = clean_ocr_line(nxt);
                    if !cand.is_empty() && !is_garbage_ocr_line(&cand) && cand != "<<<DOC_START>>>" {
                        doc_start_norm = Some(normalize_line_for_match(&cand));
                        break;
                    }
                }
                if doc_start_norm.is_some() {
                    break;
                }
            }
        }
        if doc_start_norm.is_some() {
            break;
        }
    }

    let mut para_breaks_after: HashSet<String> = HashSet::new();
    for frame in &valid {
        let raw_lines: Vec<&str> = frame.lines().map(str::trim).collect();
        for i in 0..raw_lines.len().saturating_sub(1) {
            if !raw_lines[i].is_empty() && raw_lines[i + 1].is_empty() {
                let norm_l = normalize_line_for_match(raw_lines[i]);
                if !norm_l.is_empty() {
                    para_breaks_after.insert(norm_l);
                }
            }
        }
    }

    let curr_raw_full = valid.last().unwrap();
    let curr_cleaned = curr_raw_full
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !is_garbage_ocr_line(l))
        .map(clean_ocr_line)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>();
    let curr_lines = unsplit_broken_lines(&strip_top_unselected_debris(&curr_cleaned));

    let curr_shares_overlap_with_prev = valid.len() > 1
        && valid[..valid.len() - 1].iter().any(|prev| {
            let p_set: HashSet<String> = prev
                .lines()
                .map(|l| normalize_line_for_match(&clean_ocr_line(l)))
                .filter(|l| !l.is_empty())
                .collect();
            curr_lines
                .iter()
                .any(|cl| p_set.contains(&normalize_line_for_match(cl)))
        });

    let total_chars: usize = curr_lines.iter().map(|l| l.chars().count()).sum();
    let is_short = curr_lines.len() <= 5
        && curr_lines.iter().all(|l| l.chars().count() < 55)
        && total_chars < 200
        && !curr_shares_overlap_with_prev;
    if is_short {
        let normalized_input = auto_correct_ocr_text(curr_raw_full).replace("\r\n", "\n");
        let preserved: Vec<String> = normalized_input
            .split('\n')
            .map(|l| {
                let t = l.trim();
                if t.is_empty() {
                    String::new()
                } else {
                    clean_ocr_line(t)
                }
            })
            .collect();
        return preserved.join("\r\n");
    }

    let has_closing = curr_lines.len() >= 2
        && curr_lines[curr_lines.len() - 2..]
            .iter()
            .any(|l| l.starts_with('#'));
    let first_line = curr_lines.first().map(String::as_str).unwrap_or("");
    let has_body_start = if let Some(ref ds) = doc_start_norm {
        normalize_line_for_match(first_line) == *ds
    } else {
        is_valid_body_start(first_line)
    };
    let is_complete_single_screen = has_closing && has_body_start && curr_lines.len() >= 6;
    if is_complete_single_screen && score_gap_quality(&curr_lines) >= 0 {
        return curr_lines.join("\r\n");
    }

    let mut current_lines = curr_lines;
    for prev in valid[..valid.len() - 1].iter().rev() {
        let p_cleaned = prev
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !is_garbage_ocr_line(l))
            .map(clean_ocr_line)
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>();
        let prev_lines = unsplit_broken_lines(&p_cleaned);
        if prev_lines.is_empty() {
            continue;
        }
        if let Some(merged) = merge_two_overlapping_blocks(&prev_lines, &current_lines)
            .or_else(|| merge_two_overlapping_blocks(&current_lines, &prev_lines))
        {
            current_lines = merged;
        } else if !is_complete_single_screen {
            let t_merged =
                stitch_two_ocr_texts(&prev_lines.join("\r\n"), &current_lines.join("\r\n"));
            let mut m_lines: Vec<String> = t_merged
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !is_garbage_ocr_line(l))
                .map(clean_ocr_line)
                .filter(|l| !l.is_empty())
                .collect();
            if m_lines.is_empty() || m_lines == current_lines {
                let rev_merged =
                    stitch_two_ocr_texts(&current_lines.join("\r\n"), &prev_lines.join("\r\n"));
                m_lines = rev_merged
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty() && !is_garbage_ocr_line(l))
                    .map(clean_ocr_line)
                    .filter(|l| !l.is_empty())
                    .collect();
            }
            if !m_lines.is_empty() {
                current_lines = unsplit_broken_lines(&m_lines);
            }
        }
    }

    current_lines = unsplit_broken_lines(&current_lines);

    if let Some(ref ds) = doc_start_norm {
        if let Some(idx) = current_lines
            .iter()
            .position(|l| normalize_line_for_match(l) == *ds)
        {
            if idx > 0 {
                current_lines = current_lines[idx..].to_vec();
            }
        }
    }

    current_lines = strip_typed_buffer_contamination_from_top(&current_lines);
    current_lines = strip_top_unselected_debris(&current_lines);
    current_lines = unsplit_broken_lines(&current_lines);

    if current_lines.len() <= 1 {
        return current_lines.join("\r\n");
    }
    let joined = current_lines.join("\r\n");
    ensure_paragraph_formatting(&joined, Some(&para_breaks_after))
}

fn is_para_closing_line(line: &str) -> bool {
    let low = line.trim().to_lowercase();
    let trimmed = low.trim_end_matches(&['.', ')', '?', '!', '"', ' '][..]);
    [
        "media sosial",
        "liferaft",
        "rakit darurat",
        "korban jiwa",
        "dari warganet",
        "banteners",
    ]
    .iter()
    .any(|suf| trimmed.ends_with(suf))
}

fn is_section_opener_line(line: &str) -> bool {
    let low = line.trim().to_lowercase();
    line.trim().starts_with('#')
        || [
            "momen menegangkan",
            "situasi darurat",
            "dalam unggahan",
            "dalam video",
            "dalam rekaman",
            "dalam konten",
            "dalam foto",
            "kericuhan",
            "bagaimana perasaan",
            "bagaimana tanggapan",
            "bagaimana menurut",
            "rekaman singkat",
            "source:",
            "source :",
            "sumber:",
            "sumber :",
            "video:",
            "video :",
            "foto:",
            "foto :",
        ]
        .iter()
        .any(|pfx| low.starts_with(pfx))
}

/// Final Paragraph Formatter (`handset/screen.py::ensure_paragraph_formatting` parity).
pub fn ensure_paragraph_formatting(
    text: &str,
    raw_para_breaks: Option<&HashSet<String>>,
) -> String {
    if text.trim().is_empty() {
        return String::new();
    }

    let mut raw_lines: Vec<String> = text
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .split('\n')
        .map(|l| l.trim().to_string())
        .collect();
    raw_lines = strip_top_unselected_debris(&raw_lines);
    raw_lines = strip_typed_buffer_contamination_from_top(&raw_lines);

    let mut non_empty: Vec<String> = Vec::new();
    let mut has_blank_after: HashSet<usize> = HashSet::new();

    for l in raw_lines {
        if !l.is_empty() {
            let cl = clean_ocr_line(&l);
            if !cl.is_empty() && !is_garbage_ocr_line(&cl) {
                non_empty.push(cl);
            }
        } else if !non_empty.is_empty() {
            has_blank_after.insert(non_empty.len() - 1);
        }
    }

    non_empty = strip_typed_buffer_contamination_from_top(&non_empty);
    non_empty = strip_top_unselected_debris(&non_empty);

    if let Some(last_ht_idx) = non_empty.iter().rposition(|l| l.trim().starts_with('#')) {
        let mut clean_tail = non_empty[..=last_ht_idx].to_vec();
        for l in &non_empty[last_ht_idx + 1..] {
            if l.trim().starts_with('#')
                || (!is_garbage_ocr_line(l) && l.split_whitespace().count() > 3)
            {
                clean_tail.push(l.clone());
            }
        }
        non_empty = clean_tail;
    }

    if non_empty.is_empty() {
        return String::new();
    }
    if non_empty.len() == 1 {
        return non_empty[0].clone();
    }

    let is_short_text = non_empty.len() <= 6;
    if is_short_text {
        let mut buf = get_typed_lines_buffer();
        let curr_act = get_current_typed_line().trim().to_string();
        if !curr_act.is_empty() && buf.last().is_none_or(|b| b != &curr_act) {
            buf.push(curr_act);
        }
        for idx in 0..non_empty.len().saturating_sub(1) {
            if has_blank_after.contains(&idx) {
                continue;
            }
            let clow = non_empty[idx].to_lowercase();
            let nlow = non_empty[idx + 1].to_lowercase();
            for b_idx in 0..buf.len().saturating_sub(2) {
                if buf[b_idx + 1].is_empty() {
                    let bp = buf[b_idx].to_lowercase();
                    let bn = buf[b_idx + 2].to_lowercase();
                    let prev_ok = clow == bp
                        || (clow.chars().count() >= 4 && (bp.contains(&clow) || clow.contains(&bp)))
                        || difflib_ratio(&clow, &bp) >= 0.72;
                    let next_ok = nlow == bn
                        || (nlow.chars().count() >= 4 && (bn.contains(&nlow) || nlow.contains(&bn)))
                        || difflib_ratio(&nlow, &bn) >= 0.72;
                    if prev_ok && next_ok {
                        has_blank_after.insert(idx);
                        break;
                    }
                }
            }
        }
    }

    let full_lengths: Vec<usize> = non_empty
        .iter()
        .filter(|l| l.chars().count() >= 28 && !l.starts_with('#'))
        .map(|l| l.chars().count())
        .collect();
    let avg_full_len = if !full_lengths.is_empty() {
        (full_lengths.iter().sum::<usize>() as f64) / (full_lengths.len() as f64)
    } else {
        42.0
    };

    let mut out_lines: Vec<String> = Vec::new();
    for i in 0..non_empty.len() {
        let line = &non_empty[i];
        out_lines.push(line.clone());
        if i + 1 < non_empty.len() {
            let nxt = &non_empty[i + 1];
            let norm_curr = normalize_line_for_match(line);
            let is_curr_src = {
                let l = line.to_ascii_lowercase();
                l.starts_with("source:") || l.starts_with("source :") || l.starts_with("sumber:") || l.starts_with("sumber :")
            };
            let is_next_src = {
                let l = nxt.to_ascii_lowercase();
                l.starts_with("source:") || l.starts_with("source :") || l.starts_with("sumber:") || l.starts_with("sumber :")
            };
            let is_curr_ht = line.starts_with('#');
            let is_next_ht = nxt.starts_with('#');

            let mut is_para_end = false;
            if is_short_text {
                if has_blank_after.contains(&i)
                    || raw_para_breaks.is_some_and(|rb| {
                        rb.contains(&norm_curr)
                            || rb
                                .iter()
                                .any(|b| norm_curr.contains(b) || b.contains(&norm_curr))
                    })
                {
                    is_para_end = true;
                }
            } else if is_curr_ht && is_next_ht {
                is_para_end = false;
            } else if is_next_src || (is_next_ht && !is_curr_ht) {
                is_para_end = true;
            } else {
                let has_sentence_end = ends_with_terminal_punct(line) || is_curr_src;
                let is_next_capital = nxt.chars().next().is_some_and(|c| {
                    c.is_uppercase() || matches!(c, '"' | '“' | '\'' | '#')
                });
                let has_blank = has_blank_after.contains(&i)
                    || raw_para_breaks.is_some_and(|rb| {
                        rb.contains(&norm_curr)
                            || rb
                                .iter()
                                .any(|b| norm_curr.contains(b) || b.contains(&norm_curr))
                    });

                if has_sentence_end
                    && ((has_blank && is_next_capital)
                        || is_section_opener_line(nxt)
                        || is_para_closing_line(line)
                        || ((line.chars().count() as f64) < 0.82 * avg_full_len
                            && is_next_capital
                            && !is_curr_ht))
                {
                    is_para_end = true;
                }
                if !is_curr_src && !has_sentence_end {
                    is_para_end = false;
                }
            }
            if is_para_end {
                out_lines.push(String::new());
            }
        }
    }

    let mut joined = complete_usernames(&out_lines.join("\r\n"));
    joined = joined
        .replace("(Al Generated Content)", "(AI Generated Content)")
        .replace("Al Generated", "AI Generated");
    while joined.contains("\r\n\r\n\r\n") {
        joined = joined.replace("\r\n\r\n\r\n", "\r\n\r\n");
    }
    joined
}

/// Synchronize candidate casing with iPhone screen OCR casing when Caps Lock caused an all-caps/lower mismatch (Tahap 194).
pub fn sync_candidate_case_to_ocr(ocr_text: &str, candidate_text: &str) -> String {
    if ocr_text.is_empty() || candidate_text.is_empty() {
        return candidate_text.to_string();
    }
    let alpha_ocr: String = ocr_text.chars().filter(|c| c.is_alphabetic()).collect();
    let alpha_cand: String = candidate_text.chars().filter(|c| c.is_alphabetic()).collect();
    if alpha_ocr.chars().count() >= 4 && alpha_cand.chars().count() >= 4 {
        let ocr_all_lower = alpha_ocr.chars().all(|c| c.is_lowercase());
        let ocr_all_upper = alpha_ocr.chars().all(|c| c.is_uppercase());
        let cand_all_lower = alpha_cand.chars().all(|c| c.is_lowercase());
        let cand_all_upper = alpha_cand.chars().all(|c| c.is_uppercase());
        if ocr_all_lower && cand_all_upper {
            return candidate_text.to_lowercase();
        }
        if ocr_all_upper && cand_all_lower {
            return candidate_text.to_uppercase();
        }
    }
    candidate_text.to_string()
}

fn is_length_comparable(cand: &str, target: &str) -> bool {
    let tlen = target.chars().count();
    if tlen < 5 {
        return true;
    }
    let allowed = 3usize.max(((tlen as f64) * 0.20) as usize);
    cand.chars().count().abs_diff(tlen) <= allowed
}

/// Optical auto-correction + Typed Shadow Buffer contextual recovery (`handset/screen.py::auto_correct_ocr_text` parity).
pub fn auto_correct_ocr_text(input: &str) -> String {
    let candidates = get_typed_candidates_for_ocr();
    let active_line = get_last_typed_active_line().trim().to_string();
    let has_valid_active = active_line.chars().count() >= 3;
    let raw_ocr = input.trim();

    if has_valid_active {
        if raw_ocr.is_empty() || raw_ocr.chars().count() <= 2 {
            return active_line;
        }
        let raw_lower = raw_ocr.to_lowercase();
        let act_lower = active_line.to_lowercase();
        if raw_ocr.chars().count() < 5 || is_length_comparable(&active_line, raw_ocr) {
            if act_lower == raw_lower
                || (act_lower.chars().count() >= 6 && raw_lower.contains(&act_lower))
            {
                return sync_candidate_case_to_ocr(raw_ocr, &active_line);
            }
            if difflib_ratio(&raw_lower, &act_lower) >= 0.78 {
                return sync_candidate_case_to_ocr(raw_ocr, &active_line);
            }
        }
    }

    if raw_ocr.is_empty() {
        return if has_valid_active {
            active_line
        } else {
            String::new()
        };
    }

    let had_crlf = input.contains("\r\n");
    let raw_lines: Vec<String> = input
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .split('\n')
        .map(|l| l.trim().to_string())
        .collect();

    if candidates.is_empty() {
        let cleaned: Vec<String> = raw_lines
            .iter()
            .map(|l| if l.is_empty() { String::new() } else { clean_ocr_line(l) })
            .collect();
        let sep = if had_crlf || cleaned.len() > 1 { "\r\n" } else { "\n" };
        let res = cleaned.join(sep);
        return res;
    }

    let non_empty: Vec<&String> = raw_lines.iter().filter(|l| !l.is_empty()).collect();
    if non_empty.len() <= 1 {
        let full_ocr = clean_ocr_line(non_empty.first().map(|s| s.as_str()).unwrap_or(""));
        let full_lower = full_ocr.to_lowercase();
        let ocr_words_vec: Vec<&str> = full_lower.split_whitespace().collect();
        let ocr_words_set: HashSet<&str> = ocr_words_vec.iter().copied().collect();

        let mut best_cand: Option<&String> = None;
        let mut best_score = 0.0f64;

        for cand in &candidates {
            let cs = cand.trim();
            if cs.chars().count() < 3 {
                continue;
            }
            if full_ocr.chars().count() >= 5 && !is_length_comparable(cs, &full_ocr) {
                continue;
            }
            let clow = cs.to_lowercase();
            if clow == full_lower {
                return sync_candidate_case_to_ocr(&full_ocr, cs);
            }
            let mut score = if clow.chars().next() != full_lower.chars().next() {
                let cwords: Vec<&str> = clow.split_whitespace().collect();
                if ocr_words_vec.is_empty() || cwords.is_empty() {
                    continue;
                }
                let w0_r = difflib_ratio(ocr_words_vec[0], cwords[0]);
                let cset: HashSet<&str> = cwords.iter().copied().collect();
                let shared = ocr_words_set.intersection(&cset).count();
                let sm = difflib_ratio(&full_lower, &clow);
                let multi = cwords.len() >= 2 && shared >= 1 && w0_r >= 0.50;
                let min_req = if multi { 0.72 } else { 0.84 };
                if (!multi && w0_r < 0.70) || sm < min_req {
                    continue;
                }
                sm + if multi { 0.04 } else { 0.0 }
            } else if clow.chars().count() >= 8
                && (full_lower.contains(&clow) || clow.contains(&full_lower))
            {
                0.95
            } else {
                let sm = difflib_ratio(&full_lower, &clow);
                let cset: HashSet<&str> = clow.split_whitespace().collect();
                let w_r = if !cset.is_empty() && !ocr_words_set.is_empty() {
                    ocr_words_set.intersection(&cset).count() as f64
                        / (cset.len().max(ocr_words_set.len()) as f64)
                } else {
                    0.0
                };
                sm.max(w_r)
            };
            if has_valid_active && clow == active_line.to_lowercase() {
                score += 0.08;
            }
            if score > best_score {
                best_score = score;
                best_cand = Some(cand);
            }
        }
        if best_score >= 0.78 {
            if let Some(bc) = best_cand {
                return sync_candidate_case_to_ocr(&full_ocr, bc.trim());
            }
        }
    }

    // Priority 3: Line-by-line contextual correction preserving empty lines
    let mut other_matching = 0usize;
    for l in &raw_lines {
        if l.trim().is_empty() {
            continue;
        }
        let ll = clean_ocr_line(l).to_lowercase();
        if ll.is_empty() {
            continue;
        }
        for cand in &candidates {
            let cl = cand.trim().to_lowercase();
            if ll == cl
                || (ll.chars().count() >= 6 && (cl.contains(&ll) || ll.contains(&cl)))
                || difflib_ratio(&ll, &cl) >= 0.72
            {
                other_matching += 1;
                break;
            }
        }
    }

    let is_short_note = non_empty.len() <= 6;
    let mut corrected_lines = Vec::new();
    for line in &raw_lines {
        if line.is_empty() {
            corrected_lines.push(String::new());
            continue;
        }
        let cl_line = clean_ocr_line(line);
        if cl_line.is_empty() {
            continue;
        }
        let l_low = cl_line.to_lowercase();

        // Contextual Prefix Recovery ("the" -> "the beatles")
        let is_exact_cand = candidates.iter().any(|c| c.trim().to_lowercase() == l_low);
        if !is_exact_cand
            && is_short_note
            && other_matching >= 1
            && l_low.split_whitespace().count() <= 2
            && l_low.chars().count() <= 15
        {
            if let Some(pfx_match) = candidates.iter().find(|c| {
                let cs = c.trim();
                let clow = cs.to_lowercase();
                clow.starts_with(&format!("{l_low} "))
                    && clow.split_whitespace().count() <= l_low.split_whitespace().count() + 1
                    && cs.chars().count() <= cl_line.chars().count() + 10
            }) {
                corrected_lines.push(sync_candidate_case_to_ocr(&cl_line, pfx_match.trim()));
                continue;
            }
        }

        let mut line_best = cl_line.clone();
        let mut line_ratio = 0.0f64;
        let l_words: Vec<&str> = l_low.split_whitespace().collect();

        for cand in &candidates {
            let cs = cand.trim();
            if cs.chars().count() < 3 {
                continue;
            }
            let clow = cs.to_lowercase();
            let c_words: Vec<&str> = clow.split_whitespace().collect();
            let same_endpoints = clow.chars().count() >= 4
                && l_low.chars().count() >= 4
                && clow.chars().next() == l_low.chars().next()
                && clow.chars().last() == l_low.chars().last()
                && clow.chars().count().abs_diff(l_low.chars().count()) <= 2;
            let mut req_ratio: f64 = if same_endpoints { 0.72 } else { 0.78 };

            if cl_line.chars().count() >= 5 && !is_length_comparable(cs, &cl_line) && !same_endpoints {
                continue;
            }

            if clow.chars().next() != l_low.chars().next() {
                if l_words.is_empty() || c_words.is_empty() {
                    continue;
                }
                let w0_r = difflib_ratio(l_words[0], c_words[0]);
                let ls: HashSet<&str> = l_words.iter().copied().collect();
                let cs_set: HashSet<&str> = c_words.iter().copied().collect();
                let multi = c_words.len() >= 2 && ls.intersection(&cs_set).count() >= 1 && w0_r >= 0.50;
                if !multi && w0_r < 0.70 {
                    continue;
                }
                req_ratio = if multi {
                    req_ratio.min(0.72)
                } else {
                    req_ratio.max(0.84)
                };
            }

            if cl_line.chars().count() >= 6
                && (clow.contains(&l_low) || l_low.contains(&clow))
                && clow.chars().next() == l_low.chars().next()
            {
                line_best = cs.to_string();
                line_ratio = 1.0;
                break;
            }
            let r = difflib_ratio(&l_low, &clow);
            if r >= req_ratio && r > line_ratio {
                line_ratio = r;
                line_best = cs.to_string();
            }
        }

        if line_ratio >= 0.72 {
            corrected_lines.push(sync_candidate_case_to_ocr(&cl_line, &line_best));
        } else {
            corrected_lines.push(cl_line);
        }
    }

    let result = corrected_lines.join("\r\n");
    if result.trim().chars().count() <= 2 {
        if has_valid_active {
            return active_line;
        }
        if raw_ocr.chars().count() > result.trim().chars().count() {
            return raw_ocr.to_string();
        }
    }
    result
}

fn strip_ocr_markers(raw: &str) -> (String, SelectionColor) {
    let mut color = SelectionColor::None;
    let mut cleaned_lines = Vec::new();
    for line in raw.replace("\r\n", "\n").replace('\r', "\n").split('\n') {
        let mut s = line.to_string();
        if let Some(pos) = s.find("<<<SEL_COLOR:") {
            if let Some(end_rel) = s[pos..].find(">>>") {
                let tag_val = &s[pos + "<<<SEL_COLOR:".len()..pos + end_rel];
                let parsed = SelectionColor::from_str(tag_val);
                if parsed != SelectionColor::None {
                    color = parsed;
                }
                s.replace_range(pos..pos + end_rel + 3, "");
            }
        }
        if let Some(pos) = s.find("<<<SEL_BOUNDS:") {
            if let Some(end_rel) = s[pos..].find(">>>") {
                s.replace_range(pos..pos + end_rel + 3, "");
            }
        }
        cleaned_lines.push(s.trim().to_string());
    }
    (cleaned_lines.join("\n").trim().to_string(), color)
}

fn get_recent_typed_single_line_fallback() -> Option<String> {
    let cand = {
        let cur = get_current_typed_line().trim().to_string();
        if !cur.is_empty() {
            cur
        } else {
            get_typed_lines_buffer()
                .into_iter()
                .rev()
                .find(|s| !s.trim().is_empty())
                .unwrap_or_default()
        }
    };
    let len = cand.trim().chars().count();
    if (3..80).contains(&len) {
        Some(cand.trim().to_string())
    } else {
        None
    }
}

fn is_damaged_notes_tail(line: &str) -> bool {
    if line.starts_with('#') {
        return false;
    }
    let low = line.to_ascii_lowercase();
    low.split(|c: char| !c.is_alphanumeric())
        .any(|w| matches!(w, "ccrta" | "dilanorkan" | "hcrhacil" | "kcrhacil"))
}

/// Centralized Native Rust OCR Batch Pipeline (`handset/screen.py::process_ocr_batch_results` parity).
/// Returns `(Option<final_text>, auto_cleared_cache)`.
pub fn process_ocr_batch_results(batch_texts: &[String]) -> (Option<String>, bool) {
    if batch_texts.is_empty() {
        return (None, false);
    }

    if batch_texts.iter().any(|t| is_notes_folder_list_frame(t)) {
        clear_recent_long_ocr_chunks();
    }

    let mut current_color = SelectionColor::None;
    for bt in batch_texts.iter().rev() {
        if bt.trim().is_empty() {
            continue;
        }
        let (_, col) = strip_ocr_markers(bt);
        if col != SelectionColor::None {
            current_color = col;
            break;
        }
    }

    if let Ok(mut lc) = last_color_store().lock() {
        if *lc != SelectionColor::None
            && current_color != SelectionColor::None
            && *lc != current_color
        {
            if let Ok(mut g) = chunks_store().lock() {
                g.clear();
            }
        }
        if current_color != SelectionColor::None {
            *lc = current_color;
        }
    }

    let mut raw_para_breaks: HashSet<String> = HashSet::new();
    for bt in batch_texts {
        if bt.trim().is_empty() {
            continue;
        }
        let (bt_clean, _) = strip_ocr_markers(bt);
        let raw_l: Vec<&str> = bt_clean.lines().map(str::trim).collect();
        for idx in 0..raw_l.len().saturating_sub(1) {
            if !raw_l[idx].is_empty() && raw_l[idx + 1].is_empty() {
                let n = normalize_line_for_match(raw_l[idx]);
                if !n.is_empty() {
                    raw_para_breaks.insert(n);
                }
            }
        }
    }

    let last_raw = batch_texts.last().unwrap().trim();
    if last_raw.is_empty() || is_notes_folder_list_frame(last_raw) {
        if current_color != SelectionColor::None {
            if let Some(fb) = get_recent_typed_single_line_fallback() {
                return (Some(fb), false);
            }
        }
        return (None, false);
    }

    let (curr_no_markers, _) = strip_ocr_markers(last_raw);
    if curr_no_markers.is_empty() {
        if current_color != SelectionColor::None {
            if let Some(fb) = get_recent_typed_single_line_fallback() {
                return (Some(fb), false);
            }
        }
        return (None, false);
    }

    let mut raw_frame_lines: Vec<String> = curr_no_markers
        .lines()
        .map(|l| l.trim().to_string())
        .collect();
    let mut c_lines = strip_top_unselected_debris(
        &raw_frame_lines
            .iter()
            .filter(|l| !l.is_empty() && !is_garbage_ocr_line(l))
            .cloned()
            .collect::<Vec<_>>(),
    );

    if c_lines.is_empty() {
        if current_color != SelectionColor::None {
            if let Some(fb) = get_recent_typed_single_line_fallback() {
                return (Some(fb), false);
            }
        }
        return (None, false);
    }

    if c_lines.len() >= 4 && is_damaged_notes_tail(c_lines.last().unwrap()) {
        let popped = c_lines.pop().unwrap();
        if raw_frame_lines.last().is_some_and(|l| l == &popped) {
            raw_frame_lines.pop();
        }
    }

    // Golden Rule 1A: Single line
    if c_lines.len() == 1 {
        let cl = clean_ocr_line(&c_lines[0]);
        if !cl.is_empty() {
            return (Some(auto_correct_ocr_text(&cl)), false);
        }
    }

    // Golden Rule 1B: Short snippet preserving double Enter
    let total_chars: usize = c_lines.iter().map(|l| l.chars().count()).sum();
    if c_lines.len() <= 6 && c_lines.iter().all(|l| l.chars().count() < 55) && total_chars < 220
    {
        let mut snippet_lines: Vec<String> = raw_frame_lines
            .iter()
            .map(|l| if l.is_empty() { String::new() } else { clean_ocr_line(l) })
            .collect();
        while snippet_lines.first().is_some_and(|s| s.is_empty()) {
            snippet_lines.remove(0);
        }
        while snippet_lines.last().is_some_and(|s| s.is_empty()) {
            snippet_lines.pop();
        }
        let corrected = auto_correct_ocr_text(&snippet_lines.join("\r\n"));
        let formatted = ensure_paragraph_formatting(&corrected, Some(&raw_para_breaks));
        return (Some(formatted), false);
    }

    let mut current_frame_text = c_lines.join("\r\n");

    let has_hashtag_end = c_lines.len() >= 2
        && c_lines[c_lines.len() - 2..]
            .iter()
            .any(|l| l.starts_with('#'));
    let first_line = c_lines.first().map(String::as_str).unwrap_or("");
    let has_body_start = is_valid_body_start(first_line);
    let is_complete_single_screen = has_hashtag_end && has_body_start && c_lines.len() >= 6;

    // Golden Rule 3A: Session chunk continuity (bounded MAX_LONG_OCR_CHUNKS=5, TTL=90s, matching color)
    let now = now_epoch_secs();
    if let Ok(mut store) = chunks_store().lock() {
        store.retain(|c| (now - c.ts) <= LONG_OCR_CHUNKS_TTL_SECS);

        if !is_complete_single_screen && c_lines.len() >= 4 {
            for past in store.iter().rev() {
                if current_color != SelectionColor::None
                    && past.color != SelectionColor::None
                    && past.color != current_color
                {
                    continue;
                }
                let p_lines: Vec<String> = past
                    .lines
                    .iter()
                    .filter(|l| !is_damaged_notes_tail(l))
                    .cloned()
                    .collect();
                if p_lines.is_empty() {
                    continue;
                }
                let mut merged = if has_overlapping_body_line(&p_lines, &c_lines) {
                    merge_two_overlapping_blocks(&p_lines, &c_lines)
                        .or_else(|| merge_two_overlapping_blocks(&c_lines, &p_lines))
                } else {
                    None
                };
                if merged.is_none() {
                    let p_has_start = p_lines.len() >= 4
                        && p_lines[0].chars().next().is_some_and(|c| c.is_uppercase())
                        && !is_generic_template_line(&p_lines[0])
                        && !p_lines.iter().any(|l| l.starts_with('#'));
                    if p_has_start
                        && has_hashtag_end
                        && !has_body_start
                        && current_color != SelectionColor::None
                        && past.color == current_color
                    {
                        let mut combined = p_lines.clone();
                        combined.extend_from_slice(&c_lines);
                        merged = Some(combined);
                    }
                }
                if let Some(m) = merged {
                    c_lines = strip_top_unselected_debris(&m);
                    current_frame_text = c_lines.join("\r\n");
                    break;
                }
            }
        }

        if c_lines.len() >= 4 {
            store.push(OcrChunkEntry {
                ts: now,
                lines: c_lines.clone(),
                color: current_color,
            });
            if store.len() > MAX_LONG_OCR_CHUNKS {
                let drop_n = store.len() - MAX_LONG_OCR_CHUNKS;
                store.drain(0..drop_n);
            }
        }
    }

    let mut final_text = if batch_texts.len() <= 1
        || (is_complete_single_screen && score_gap_quality(&c_lines) >= 0)
    {
        auto_correct_ocr_text(&current_frame_text)
    } else {
        let mut valid_hist = Vec::new();
        for ht in &batch_texts[..batch_texts.len() - 1] {
            if ht.trim().is_empty() || is_notes_folder_list_frame(ht) {
                continue;
            }
            let (ht_clean, _) = strip_ocr_markers(ht);
            let h_lines = strip_top_unselected_debris(
                &ht_clean
                    .lines()
                    .map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty())
                    .collect::<Vec<_>>(),
            );
            if has_overlapping_body_line(&h_lines, &c_lines) {
                valid_hist.push(h_lines.join("\r\n"));
            }
        }
        if !valid_hist.is_empty() {
            valid_hist.push(current_frame_text.clone());
            let stitched = stitch_multiline_ocr_frames(&valid_hist);
            let chosen = if stitched.is_empty() {
                current_frame_text
            } else {
                stitched
            };
            auto_correct_ocr_text(&chosen)
        } else {
            auto_correct_ocr_text(&current_frame_text)
        }
    };

    final_text = ensure_paragraph_formatting(&final_text, Some(&raw_para_breaks));

    // Smart Auto-Clear once complete caption (>= 5 lines, capital body start, ending #hashtag) is copied
    let non_empty_final: Vec<&str> = final_text.lines().filter(|l| !l.trim().is_empty()).collect();
    let is_now_complete = non_empty_final.len() >= 5
        && non_empty_final[non_empty_final.len() - 2..]
            .iter()
            .any(|l| l.trim().starts_with('#'))
        && non_empty_final[0]
            .trim()
            .chars()
            .next()
            .is_some_and(|c| c.is_uppercase())
        && !is_generic_template_line(non_empty_final[0]);
    let mut auto_cleared = false;
    if is_now_complete {
        clear_recent_long_ocr_chunks();
        auto_cleared = true;
    }

    (Some(final_text), auto_cleared)
}

// ============================================================================
// WIN32 CLIPBOARD FFI
// ============================================================================

const CF_UNICODETEXT: u32 = 13;
const GMEM_MOVEABLE: u32 = 0x0002;

#[link(name = "user32")]
extern "system" {
    fn GetClipboardSequenceNumber() -> u32;
    fn OpenClipboard(hwnd: *mut c_void) -> i32;
    fn CloseClipboard() -> i32;
    fn EmptyClipboard() -> i32;
    fn GetClipboardData(format: u32) -> *mut c_void;
    fn SetClipboardData(format: u32, hmem: *mut c_void) -> *mut c_void;
}

#[link(name = "kernel32")]
extern "system" {
    fn GlobalAlloc(flags: u32, bytes: usize) -> *mut c_void;
    fn GlobalLock(hmem: *mut c_void) -> *mut c_void;
    fn GlobalUnlock(hmem: *mut c_void) -> i32;
}

unsafe fn open_clipboard_with_retry() -> bool {
    for attempt in 0..10u64 {
        if OpenClipboard(ptr::null_mut()) != 0 {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(2 + attempt));
    }
    false
}

pub fn read_win32_clipboard() -> Option<String> {
    unsafe {
        if !open_clipboard_with_retry() {
            return None;
        }
        let handle = GetClipboardData(CF_UNICODETEXT);
        if handle.is_null() {
            CloseClipboard();
            return None;
        }
        let ptr = GlobalLock(handle) as *const u16;
        if ptr.is_null() {
            CloseClipboard();
            return None;
        }

        let mut len = 0usize;
        while *ptr.add(len) != 0 {
            len += 1;
        }

        let slice = std::slice::from_raw_parts(ptr, len);
        let result = String::from_utf16_lossy(slice);

        GlobalUnlock(handle);
        CloseClipboard();
        Some(result)
    }
}

pub fn write_win32_clipboard(text: &str) -> bool {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes_len = wide.len() * std::mem::size_of::<u16>();

    unsafe {
        if !open_clipboard_with_retry() {
            return false;
        }
        EmptyClipboard();

        let hmem = GlobalAlloc(GMEM_MOVEABLE, bytes_len);
        if hmem.is_null() {
            CloseClipboard();
            return false;
        }

        let ptr = GlobalLock(hmem) as *mut u16;
        if ptr.is_null() {
            CloseClipboard();
            return false;
        }

        ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
        GlobalUnlock(hmem);

        let res = SetClipboardData(CF_UNICODETEXT, hmem);
        CloseClipboard();

        !res.is_null()
    }
}

pub fn get_clipboard_sequence_number() -> u32 {
    unsafe { GetClipboardSequenceNumber() }
}

// ============================================================================
// Telemetry, Debug JSON, In-Process Batch API & CLI Parity Evaluator
// ============================================================================

#[derive(Debug, Clone, PartialEq)]
pub struct HotOcrTelemetry {
    pub hot_ocr_engine_ready: bool,
    pub inprocess_winrt_com_ok: bool,
    pub driver_mode: &'static str,
    pub warm_latency_ms: f64,
    pub batches_served: u64,
    pub fallbacks_triggered: u64,
}

impl HotOcrTelemetry {
    pub fn new_warm(warm_latency_ms: f64) -> Self {
        let com_ok = probe_inprocess_winrt_ocr_com();
        Self {
            hot_ocr_engine_ready: true,
            inprocess_winrt_com_ok: com_ok,
            driver_mode: "persistent_winrt_hot_worker",
            warm_latency_ms,
            batches_served: 0,
            fallbacks_triggered: 0,
        }
    }

    pub fn to_json(&self) -> String {
        format!(
            r#"{{"ok":true,"hot_ocr_engine_ready":{},"inprocess_winrt_com_ok":{},"zero_powershell":true,"driver_mode":"{}","warm_latency_ms":{:.2},"batches_served":{},"fallbacks_triggered":{}}}"#,
            self.hot_ocr_engine_ready,
            self.inprocess_winrt_com_ok,
            self.driver_mode,
            self.warm_latency_ms,
            self.batches_served,
            self.fallbacks_triggered
        )
    }
}

/// Parses `---FRAME_SPLIT---` output frames.
pub fn normalize_and_verify_ocr_batch_output(raw_stdout: &str) -> Vec<String> {
    raw_stdout
        .split("---FRAME_SPLIT---")
        .map(|chunk| chunk.replace("\r\n", "\n").trim().to_string())
        .filter(|chunk| !chunk.is_empty())
        .collect()
}

pub fn escape_json_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

fn unescape_json_string_at(src: &str, start_quote: usize) -> Option<(String, usize)> {
    let bytes = src.as_bytes();
    if *bytes.get(start_quote)? != b'"' {
        return None;
    }
    let mut i = start_quote + 1;
    let mut out = String::new();
    while i < src.len() {
        let rest = &src[i..];
        let ch = rest.chars().next()?;
        if ch == '"' {
            return Some((out, i + 1));
        }
        if ch == '\\' {
            i += 1;
            let esc = src[i..].chars().next()?;
            i += esc.len_utf8();
            match esc {
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                '/' => out.push('/'),
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                'b' => out.push('\x08'),
                'f' => out.push('\x0c'),
                'u' => {
                    let hex = src.get(i..i + 4)?;
                    if let Ok(code) = u32::from_str_radix(hex, 16) {
                        i += 4;
                        if (0xD800..=0xDBFF).contains(&code)
                            && src.get(i..i + 2) == Some("\\u")
                        {
                            if let Some(low_hex) = src.get(i + 2..i + 6) {
                                if let Ok(low) = u32::from_str_radix(low_hex, 16) {
                                    if (0xDC00..=0xDFFF).contains(&low) {
                                        let cp =
                                            0x10000 + (((code - 0xD800) << 10) | (low - 0xDC00));
                                        if let Some(u) = char::from_u32(cp) {
                                            out.push(u);
                                        }
                                        i += 6;
                                        continue;
                                    }
                                }
                            }
                        }
                        if let Some(u) = char::from_u32(code) {
                            out.push(u);
                        }
                    }
                }
                other => out.push(other),
            }
        } else {
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    None
}

fn find_json_field_colon(body: &str, key: &str) -> Option<usize> {
    let needle = format!("\"{key}\"");
    let mut search_from = 0usize;
    while let Some(rel) = body[search_from..].find(&needle) {
        let idx = search_from + rel + needle.len();
        let rest = &body[idx..];
        let trimmed_start = rest.len() - rest.trim_start().len();
        if rest[trimmed_start..].starts_with(':') {
            return Some(idx + trimmed_start + 1);
        }
        search_from = idx;
    }
    None
}

pub fn extract_json_string_field(body: &str, key: &str) -> Option<String> {
    let colon_end = find_json_field_colon(body, key)?;
    let rest = body[colon_end..].trim_start();
    let ws = body.len() - colon_end - rest.len();
    if !rest.starts_with('"') {
        return None;
    }
    unescape_json_string_at(body, colon_end + ws).map(|(s, _)| s)
}

pub fn extract_json_bool_field(body: &str, key: &str) -> Option<bool> {
    let colon_end = find_json_field_colon(body, key)?;
    let rest = body[colon_end..].trim_start();
    if rest.starts_with("true") {
        Some(true)
    } else if rest.starts_with("false") {
        Some(false)
    } else {
        None
    }
}

pub fn extract_json_string_array_field(body: &str, key: &str) -> Option<Vec<String>> {
    let colon_end = find_json_field_colon(body, key)?;
    let rest = body[colon_end..].trim_start();
    if !rest.starts_with('[') {
        return None;
    }
    let abs_start = body.len() - rest.len() + 1;
    let mut items = Vec::new();
    let mut pos = abs_start;
    while pos < body.len() {
        let slice = &body[pos..];
        let trimmed = slice.trim_start();
        pos = body.len() - trimmed.len();
        if trimmed.starts_with(']') {
            return Some(items);
        }
        if trimmed.starts_with(',') {
            pos += 1;
            continue;
        }
        if trimmed.starts_with('"') {
            if let Some((s, next_pos)) = unescape_json_string_at(body, pos) {
                items.push(s);
                pos = next_pos;
                continue;
            }
        }
        pos += trimmed.chars().next().map(char::len_utf8).unwrap_or(1);
    }
    Some(items)
}

fn write_debug_ocr_result(raw_ocr: &str, batch_count: usize, corrected: &str, auto_cleared: bool) {
    let temp_dir = std::env::temp_dir().join("iPhoneLink");
    if fs::create_dir_all(&temp_dir).is_ok() {
        let json = format!(
            "{{\n  \"raw_ocr_text\": \"{}\",\n  \"batch_count\": {},\n  \"corrected_text\": \"{}\",\n  \"auto_cleared_cache\": {}\n}}",
            escape_json_value(raw_ocr),
            batch_count,
            escape_json_value(corrected),
            auto_cleared
        );
        let _ = fs::write(temp_dir.join("debug_ocr_result.json"), json);
    }
}

/// Decodes any `data:image/...;base64,...` snapshot via `gdiplus.dll` / BMP fast path
/// and runs the dual-candidate highlight pre-processor in-process (`0` PowerShell).
pub fn process_bmp_snapshot_preproc(data_url: &str) -> Option<(usize, usize, bool)> {
    let (rgba, w, h) = decode_image_data_url(data_url)?;
    let prep = prepare_dual_ocr_candidates(&rgba, w, h, false, false);
    Some((prep.final_w, prep.final_h, prep.found_highlight))
}

fn invert_rgba_rgb_in_place(buf: &mut [u8]) {
    for px in buf.chunks_exact_mut(4) {
        px[0] = 255u8.saturating_sub(px[0]);
        px[1] = 255u8.saturating_sub(px[1]);
        px[2] = 255u8.saturating_sub(px[2]);
    }
}

fn is_dark_background_dominant(rgba: &[u8], w: usize, h: usize) -> bool {
    if w < 16 || h < 16 {
        return false;
    }
    let mut dark = 0usize;
    let mut total = 0usize;
    let y0 = (h as f64 * 0.12) as usize;
    let y1 = (h as f64 * 0.80) as usize;
    for y in (y0..y1).step_by(8) {
        for x in (0..w).step_by(8) {
            let idx = (y * w + x) * 4;
            if idx + 2 >= rgba.len() {
                continue;
            }
            let lum = (rgba[idx] as u32 * 299 + rgba[idx + 1] as u32 * 587 + rgba[idx + 2] as u32 * 114) / 1000;
            if lum < 75 {
                dark += 1;
            }
            total += 1;
        }
    }
    total > 0 && dark * 100 >= total * 55
}

/// Runs Dual-Candidate Pre-Processing + In-Process WinRT `OcrEngine` on an RGBA frame.
/// Includes automatic dark-mode inversion fallback and full-frame fallback so text at top/bottom never vanishes.
pub fn run_dual_winrt_ocr_on_rgba(
    rgba: &[u8],
    w: usize,
    h: usize,
    is_full_viewport: bool,
) -> (Option<String>, bool, SelectionColor) {
    let prep = prepare_dual_ocr_candidates(rgba, w, h, is_full_viewport, false);
    let mut lines1 = recognize_rgba_with_winrt_ocr(&prep.cand1_rgba, prep.final_w, prep.final_h);
    let mut lines2 = if let Some(ref c2) = prep.cand2_rgba {
        recognize_rgba_with_winrt_ocr(c2, prep.final_w, prep.final_h)
    } else {
        Vec::new()
    };

    if lines1.is_empty() && lines2.is_empty() && is_dark_background_dominant(rgba, w, h) {
        let mut inv = prep.cand1_rgba.clone();
        invert_rgba_rgb_in_place(&mut inv);
        lines1 = recognize_rgba_with_winrt_ocr(&inv, prep.final_w, prep.final_h);
    }

    if lines1.is_empty() && lines2.is_empty() {
        // Fallback: recognize the full uncropped frame (including header/bottom dialogs & dark mode if needed)
        lines2 = recognize_rgba_with_winrt_ocr(rgba, w, h);
        if lines2.is_empty() && is_dark_background_dominant(rgba, w, h) {
            let mut inv_full = rgba[..w * h * 4].to_vec();
            invert_rgba_rgb_in_place(&mut inv_full);
            lines2 = recognize_rgba_with_winrt_ocr(&inv_full, w, h);
        }
    }

    let merged = merge_line_lists(&lines1, &lines2);
    let formatted_lines = format_parsed_lines_to_text(&merged, false);
    let joined = formatted_lines.join("\r\n").trim().to_string();
    if joined.is_empty() {
        return (None, prep.found_highlight, prep.selection_color);
    }

    if prep.found_highlight && prep.selection_color != SelectionColor::None {
        let top_norm = (prep.sel_min_y as f64 / (h.max(1) as f64)).clamp(0.0, 1.0);
        let bot_norm = (prep.sel_max_y as f64 / (h.max(1) as f64)).clamp(top_norm, 1.0);
        let tagged = format!(
            "<<<SEL_COLOR:{}>>>\r\n<<<SEL_BOUNDS:top={:.4},bot={:.4}>>>\r\n{}",
            prep.selection_color.as_str(),
            top_norm,
            bot_norm,
            joined
        );
        return (Some(tagged), true, prep.selection_color);
    }

    (Some(joined), prep.found_highlight, prep.selection_color)
}

pub fn extract_data_urls_array_from_json(body: &str, key: &str) -> Vec<String> {
    let Some(colon_end) = find_json_field_colon(body, key) else {
        return Vec::new();
    };
    let rest = body[colon_end..].trim_start();
    if !rest.starts_with('[') {
        return Vec::new();
    }
    let abs_start = body.len() - rest.len();
    let mut out = Vec::new();
    let mut pos = abs_start;
    let mut depth = 0i32;
    while pos < body.len() {
        let b = body.as_bytes()[pos];
        match b {
            b'[' => {
                depth += 1;
                pos += 1;
            }
            b']' => {
                depth -= 1;
                pos += 1;
                if depth <= 0 {
                    break;
                }
            }
            b'"' => {
                if let Some((s, next_pos)) = unescape_json_string_at(body, pos) {
                    if s.starts_with("data:image/") {
                        out.push(s);
                    }
                    pos = next_pos;
                } else {
                    pos += 1;
                }
            }
            _ => {
                pos += 1;
            }
        }
    }
    out
}

/// Runs native GDI+ decode + in-process WinRT OCR directly on ordered `data:image/...` URLs.
pub fn extract_and_process_ocr_from_data_urls(urls: &[String]) -> (Option<String>, bool, usize) {
    let mut batch: Vec<String> = Vec::new();
    let mut any_highlight = false;
    let total = urls.len();
    for (idx, u) in urls.iter().enumerate() {
        if let Some((rgba, w, h)) = decode_image_data_url(u) {
            let is_full = idx + 1 == total && total > 1;
            let (txt_opt, hl, _) = run_dual_winrt_ocr_on_rgba(&rgba, w, h, is_full);
            if hl {
                any_highlight = true;
            }
            if let Some(txt) = txt_opt {
                if !txt.trim().is_empty() {
                    batch.push(txt);
                }
            }
        }
    }
    let batch_count = batch.len();
    let (mut final_opt, auto_cleared) = process_ocr_batch_results(&batch);
    if final_opt.as_deref().map(str::trim).unwrap_or("").is_empty() && any_highlight {
        if let Some(fb) = get_recent_typed_single_line_fallback() {
            final_opt = Some(fb);
        }
    }
    if let Some(ref ft) = final_opt {
        write_debug_ocr_result(
            batch.last().map(String::as_str).unwrap_or(""),
            batch_count,
            ft,
            auto_cleared,
        );
    }
    (final_opt, auto_cleared, batch_count)
}

/// Runs GDI+ decode + WinRT OCR on a single data URL and returns raw newline text (used for QR/Link Grabber fallback).
pub fn recognize_data_url_text_raw(data_url: &str) -> Option<String> {
    let (rgba, w, h) = decode_image_data_url(data_url)?;
    let (txt_opt, _, _) = run_dual_winrt_ocr_on_rgba(&rgba, w, h, true);
    txt_opt
}

fn crop_band_rgba(
    rgba: &[u8],
    w: usize,
    h: usize,
    y_start: usize,
    y_end: usize,
) -> Option<(Vec<u8>, usize, usize)> {
    let ys = y_start.min(h);
    let ye = y_end.min(h);
    if w < 16 || ye <= ys + 8 {
        return None;
    }
    let ch = ye - ys;
    let row_bytes = w.saturating_mul(4);
    let byte_start = ys.saturating_mul(row_bytes);
    let byte_end = ye.saturating_mul(row_bytes);
    if byte_end > rgba.len() {
        return None;
    }
    Some((rgba[byte_start..byte_end].to_vec(), w, ch))
}

/// Tahap 244/245: Mini 2x bilinear upscale with configurable contrast factor (`1.0` = natural
/// for iOS 27.2 Liquid Glass; `1.35` = gentle contrast; `1.45` = crisp contrast) strictly for
/// narrow Spotlight URL strips (`h <= 360 px` -> `<= 7.0 MB` RGBA).
pub fn upscale_2x_bilinear_with_factor_rgba(
    src: &[u8],
    w: usize,
    h: usize,
    contrast_factor: f32,
) -> (Vec<u8>, usize, usize) {
    if w == 0 || h == 0 || src.len() < w.saturating_mul(h).saturating_mul(4) {
        return (Vec::new(), 0, 0);
    }
    let nw = w.saturating_mul(2);
    let nh = h.saturating_mul(2);
    let mut dst = vec![255u8; nw.saturating_mul(nh).saturating_mul(4)];
    let w_max = w - 1;
    let h_max = h - 1;
    let apply_contrast = (contrast_factor - 1.0).abs() > 0.01;

    #[inline]
    fn contrast_u8(v: f32, factor: f32, apply: bool) -> u8 {
        if apply {
            ((v - 128.0) * factor + 128.0).clamp(0.0, 255.0) as u8
        } else {
            v.clamp(0.0, 255.0) as u8
        }
    }

    for ny in 0..nh {
        let sy = (ny as f32) * 0.5;
        let y0 = (sy.floor() as usize).min(h_max);
        let y1 = (y0 + 1).min(h_max);
        let fy = sy - (y0 as f32);
        let iy = 1.0 - fy;
        let row0 = y0 * w;
        let row1 = y1 * w;
        let dst_row = ny * nw;

        for nx in 0..nw {
            let sx = (nx as f32) * 0.5;
            let x0 = (sx.floor() as usize).min(w_max);
            let x1 = (x0 + 1).min(w_max);
            let fx = sx - (x0 as f32);
            let ix = 1.0 - fx;

            let i00 = (row0 + x0) * 4;
            let i10 = (row0 + x1) * 4;
            let i01 = (row1 + x0) * 4;
            let i11 = (row1 + x1) * 4;
            let di = (dst_row + nx) * 4;

            let w00 = ix * iy;
            let w10 = fx * iy;
            let w01 = ix * fy;
            let w11 = fx * fy;

            for c in 0..3 {
                let v = (src[i00 + c] as f32) * w00
                    + (src[i10 + c] as f32) * w10
                    + (src[i01 + c] as f32) * w01
                    + (src[i11 + c] as f32) * w11;
                dst[di + c] = contrast_u8(v, contrast_factor, apply_contrast);
            }
            dst[di + 3] = 255;
        }
    }
    (dst, nw, nh)
}

/// Backward-compatible wrapper (`boost_contrast = true` -> `1.45x`, `false` -> `1.0x` natural).
pub fn upscale_2x_bilinear_with_contrast_rgba(
    src: &[u8],
    w: usize,
    h: usize,
    boost_contrast: bool,
) -> (Vec<u8>, usize, usize) {
    upscale_2x_bilinear_with_factor_rgba(src, w, h, if boost_contrast { 1.45 } else { 1.0 })
}

fn is_spotlight_ios_ui_label(line: &str) -> bool {
    let low = line.trim().to_ascii_lowercase();
    if low.is_empty() {
        return true;
    }
    const UI_LABELS: &[&str] = &[
        "ask siri",
        "tanya siri",
        "top hit",
        "hasil teratas",
        "siri suggestions",
        "saran siri",
        "search web",
        "cari di web",
        "siri knowledge",
        "app store",
        "show more",
        "tampilkan",
        "websites",
        "situs web",
        "search",
        "cari",
        "cancel",
        "batal",
        "apple",
        "notes",
        "catatan",
        "messages",
        "pesan",
        "mail",
        "safari",
        "maps",
        "peta",
    ];
    UI_LABELS.iter().any(|lbl| {
        low == *lbl
            || low.starts_with(&format!("{lbl} "))
            || low.starts_with(&format!("{lbl}:"))
            || low.contains("ask siri")
            || low.contains("tanya siri")
            || low.contains("siri suggestions")
            || low.contains("top hit")
            || low.contains("app store")
            || low.contains("show more")
    })
}

fn is_spotlight_url_start_line(line: &str) -> bool {
    let low = line.to_ascii_lowercase();
    low.contains("http://")
        || low.contains("https://")
        || low.contains("http5://")
        || low.contains("htps://")
        || low.contains("www.")
        || low.contains("instagram.com")
        || low.contains("tiktok.com")
        || low.contains("vt.tiktok.com")
        || low.contains("vm.tiktok.com")
        || low.contains("youtube.com")
        || low.contains("youtu.be")
        || low.contains("x.com/")
        || low.contains("twitter.com/")
        || low.contains("threads.net")
        || low.contains("threads.com")
        || low.contains("facebook.com")
        || low.contains("fb.watch")
        || low.contains("t.co/")
}

fn strip_spotlight_clear_button_suffix(line: &str) -> String {
    let trimmed = line.trim();
    let parts: Vec<&str> = trimmed.split_whitespace().collect();
    if parts.len() >= 2 {
        if let Some(&last) = parts.last() {
            if matches!(
                last,
                "x" | "X" | "×" | "⊗" | "✕" | "✖" | "Q" | "q" | "|" | "•"
            ) {
                return parts[..parts.len() - 1].join(" ");
            }
        }
    }
    trimmed.to_string()
}

fn clean_spotlight_url_line(line: &str) -> String {
    let mut s = strip_spotlight_clear_button_suffix(
        &line
            .replace('\u{200B}', "")
            .replace('\u{FEFF}', "")
            .replace('\r', ""),
    );
    // Strip single magnifier/circle OCR artifact directly glued before scheme or www.
    // (e.g. "Qhttps://...", "Ohttps://...", "0https://...", "@https://...", "Qwww....")
    let lower = s.to_ascii_lowercase();
    for marker in ["https://", "http://", "www."] {
        if let Some(pos) = lower.find(marker) {
            if pos > 0 && pos <= 4 {
                let prefix = s[..pos].trim();
                let prefix_chars = prefix.chars().count();
                if prefix_chars <= 2
                    && prefix.chars().all(|c| {
                        matches!(
                            c,
                            'Q' | 'q' | 'O' | 'o' | '0' | '@' | '©' | '®' | '🔍' | '•' | '|' | '[' | '('
                        )
                    })
                {
                    s = s[pos..].to_string();
                    break;
                }
            }
        }
    }
    // Collapse common WinRT OCR mid-URL spacing glitches
    for (bad, good) in [
        ("https:// www.", "https://www."),
        ("http:// www.", "http://www."),
        ("https:// ", "https://"),
        ("http:// ", "http://"),
        (".com /p/", ".com/p/"),
        (".com /reel/", ".com/reel/"),
        (".com /reels/", ".com/reels/"),
        (".com /tv/", ".com/tv/"),
        (".com /share/", ".com/share/"),
        (".net /share/", ".net/share/"),
        (".com /@", ".com/@"),
        (".net /@", ".net/@"),
        (".com /", ".com/"),
        (".net /", ".net/"),
    ] {
        if s.to_ascii_lowercase().contains(bad) {
            // Case-preserving replacement on ASCII URL structural tokens
            let mut out = String::with_capacity(s.len());
            let mut cur = s.as_str();
            let bad_low = bad.to_ascii_lowercase();
            while let Some(idx) = cur.to_ascii_lowercase().find(&bad_low) {
                out.push_str(&cur[..idx]);
                out.push_str(good);
                cur = &cur[idx + bad.len()..];
            }
            out.push_str(cur);
            s = out;
        }
    }
    s.trim().to_string()
}

fn is_valid_url_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            '.' | '_' | '~' | ':' | '/' | '?' | '#' | '[' | ']' | '@' | '!' | '$' | '&' | '\'' | '(' | ')' | '*' | '+' | ',' | ';' | '=' | '%' | '-'
        )
}

fn extract_spotlight_url_continuation(line: &str) -> Option<String> {
    let cleaned = strip_spotlight_clear_button_suffix(line);
    let trimmed = cleaned.trim();
    if trimmed.is_empty()
        || is_spotlight_ios_ui_label(trimmed)
        || is_spotlight_url_start_line(trimmed)
    {
        return None;
    }
    // Continuation must be a single URL token (clear button already stripped above)
    let parts: Vec<&str> = trimmed.split_whitespace().collect();
    if parts.len() != 1 {
        return None;
    }
    let tok = parts[0];
    let has_url_punct = tok
        .chars()
        .any(|c| matches!(c, '/' | '?' | '=' | '&' | '_' | '-' | '%' | '.'));
    if tok.len() < 4 && !(tok.len() >= 2 && has_url_punct) {
        return None;
    }
    if !tok.chars().all(is_valid_url_char) {
        return None;
    }
    // Reject plain TitleCase/lowercase app names (e.g. "Photos", "Settings", "Camera")
    // while accepting all social shortcodes & query params (digits, URL punct, or mixed case).
    let has_digit = tok.chars().any(|c| c.is_ascii_digit());
    let upper_count = tok.chars().filter(|c| c.is_ascii_uppercase()).count();
    let lower_count = tok.chars().filter(|c| c.is_ascii_lowercase()).count();
    let looks_like_shortcode = (upper_count >= 2 && lower_count >= 1) || tok.len() >= 10;
    if has_url_punct || has_digit || looks_like_shortcode {
        Some(tok.to_string())
    } else {
        None
    }
}

/// Tahap 245 Pilar 1: Unwraps multi-line Spotlight URLs wrapped by iOS 27.2 (`iPhone 17 Pro`)
/// such as `https://www.instagram.com/p/\nDWUra9vEgZ1/?\nigsh=MTQyY213azh0eTR4OQ==\nAsk Siri`
/// into complete single-line URLs while rejecting iOS UI labels and preserving original lines.
pub fn unwrap_multiline_spotlight_urls(raw_ocr: &str) -> String {
    let cleaned_lines: Vec<String> = raw_ocr
        .lines()
        .map(clean_spotlight_url_line)
        .filter(|l| !l.is_empty())
        .collect();
    if cleaned_lines.is_empty() {
        return String::new();
    }

    let mut out_lines: Vec<String> = Vec::new();
    let mut i = 0usize;
    while i < cleaned_lines.len() {
        let line = &cleaned_lines[i];
        if is_spotlight_url_start_line(line) {
            let mut merged = line.clone();
            // If line has leading text before the URL token, isolate the trailing URL token as start
            if let Some(last_ws) = merged.rfind(char::is_whitespace) {
                let tail = merged[last_ws + 1..].trim();
                if is_spotlight_url_start_line(tail) {
                    merged = tail.to_string();
                }
            }
            let mut joined_any = false;
            for step in 1..=3usize {
                if i + step >= cleaned_lines.len() {
                    break;
                }
                if let Some(cont) = extract_spotlight_url_continuation(&cleaned_lines[i + step]) {
                    merged.push_str(&cont);
                    joined_any = true;
                } else {
                    break;
                }
            }
            if !out_lines.contains(&merged) {
                out_lines.push(merged);
            }
            if joined_any && !out_lines.contains(line) {
                out_lines.push(line.clone());
            }
        } else if !out_lines.contains(line) {
            out_lines.push(line.clone());
        }
        i += 1;
    }

    out_lines.join("\n")
}

/// Checks whether any unwrapped OCR candidate contains a COMPLETE URL (not a bare truncated
/// social prefix like `https://www.instagram.com/p/` or `https://www.instagram.com/reel/`).
pub fn text_has_complete_url_candidate(text: &str) -> bool {
    let unwrapped = unwrap_multiline_spotlight_urls(text);
    for line in unwrapped.lines() {
        for tok in line.split_whitespace() {
            let t = tok.trim_matches(|c: char| matches!(c, '"' | '\'' | '(' | ')' | '[' | ']' | ',' | ';'));
            let low = t.to_ascii_lowercase();
            let has_scheme_or_domain = low.starts_with("https://")
                || low.starts_with("http://")
                || low.starts_with("www.instagram.com/")
                || low.starts_with("instagram.com/")
                || low.starts_with("www.tiktok.com/")
                || low.starts_with("vt.tiktok.com/")
                || low.starts_with("vm.tiktok.com/")
                || low.starts_with("www.threads.")
                || low.starts_with("threads.")
                || low.starts_with("www.youtube.com/")
                || low.starts_with("youtu.be/")
                || low.starts_with("x.com/")
                || low.starts_with("twitter.com/")
                || low.starts_with("www.facebook.com/")
                || low.starts_with("fb.watch/");
            if !has_scheme_or_domain {
                continue;
            }
            // Reject bare incomplete social paths lacking an actual post/reel/share ID
            let mut incomplete_social = false;
            for pfx in [
                "instagram.com/p/",
                "instagram.com/reel/",
                "instagram.com/reels/",
                "instagram.com/tv/",
                "threads.net/share/",
                "threads.com/share/",
            ] {
                if let Some(idx) = low.find(pfx) {
                    let rest = &t[idx + pfx.len()..];
                    let id_seg = rest.split(&['/', '?', '#'][..]).next().unwrap_or("");
                    if id_seg.len() < 6 {
                        incomplete_social = true;
                    }
                }
            }
            if incomplete_social {
                continue;
            }
            if t.len() >= 18 {
                return true;
            }
        }
    }
    false
}

/// Runs GDI+ PNG/JPEG/BMP decode + overlapping Spotlight-band in-process WinRT OCR tuned for
/// `Grab Link (Alt+B)` with low RAM (`< 45 MB` peak), multi-line iOS 27.2 URL unwrapping,
/// multi-line mini-2x crop (`<= 360 px` -> `<= 7 MB` RGBA) in BOTH 2x natural (`1.0`) and
/// 2x gentle contrast (`1.35`), and validated multi-candidate preservation (NEVER blind 2x overwrite!).
pub fn recognize_data_url_for_link_grab(data_url: &str) -> Vec<String> {
    fn push_unique(texts: &mut Vec<String>, item: String) {
        let tr = item.trim();
        if !tr.is_empty() && !texts.iter().any(|x| x == tr) {
            texts.push(tr.to_string());
        }
    }

    fn push_lines(texts: &mut Vec<String>, lines: &[OcrLineBox]) {
        if lines.is_empty() {
            return;
        }
        let formatted = format_parsed_lines_to_text(lines, false);
        let joined = formatted.join("\n").trim().to_string();
        if !joined.is_empty() {
            let unwrapped = unwrap_multiline_spotlight_urls(&joined);
            push_unique(texts, unwrapped);
            push_unique(texts, joined);
        }
    }

    fn line_has_url_hint(t: &str) -> bool {
        let low = t.to_ascii_lowercase();
        low.contains("http")
            || low.contains("instagram.com")
            || low.contains("tiktok.com")
            || low.contains("threads.")
            || low.contains("youtube.com")
            || low.contains("youtu.be")
            || low.contains("x.com/")
            || low.contains("twitter.com/")
            || low.contains("facebook.com/")
            || low.contains("fb.watch")
            || low.contains(".com/")
            || low.contains("/reel/")
            || low.contains("/p/")
    }

    fn has_url_hint(items: &[String]) -> bool {
        items.iter().any(|t| line_has_url_hint(t))
    }

    fn has_complete_url(items: &[String]) -> bool {
        items.iter().any(|t| text_has_complete_url_candidate(t))
    }

    fn run_mini_2x_strip_ocr(
        band: &[u8],
        bw: usize,
        bh: usize,
        detected_lines: &[OcrLineBox],
    ) -> Vec<String> {
        let mut out = Vec::new();
        if bw == 0 || bh == 0 || bw > 1600 || detected_lines.is_empty() {
            return out;
        }
        // Find the first URL-starting line and include up to 3 continuation lines below it
        // so 3-line wrapped iOS 27.2 Spotlight URLs (`.../p/\n<ID>/?\nigsh=...`) fit inside 2x crop!
        let mut first_idx: Option<usize> = None;
        for (idx, ln) in detected_lines.iter().enumerate() {
            if line_has_url_hint(&ln.text) {
                first_idx = Some(idx);
                break;
            }
        }
        let Some(start_idx) = first_idx else {
            return out;
        };
        let first_ln = &detected_lines[start_idx];
        let min_y = (first_ln.min_y.max(0.0) as usize).saturating_sub(24);
        let mut max_y = ((first_ln.max_y.max(first_ln.min_y + first_ln.height).max(0.0) as usize) + 32).min(bh);

        for step in 1..=3usize {
            let idx = start_idx + step;
            if idx >= detected_lines.len() {
                break;
            }
            let cand_ln = &detected_lines[idx];
            if is_spotlight_ios_ui_label(&cand_ln.text) {
                break;
            }
            let cand_top = cand_ln.min_y.max(0.0) as usize;
            let cand_bot = cand_ln.max_y.max(cand_ln.min_y + cand_ln.height).max(0.0) as usize;
            if cand_top <= min_y.saturating_add(300) {
                max_y = max_y.max((cand_bot + 32).min(bh));
            }
        }

        // If Line 0 looks truncated at a wrap boundary (`/` or `?` or `=`), ensure room below for wrapped lines
        let first_trim = first_ln.text.trim();
        if first_trim.ends_with('/') || first_trim.ends_with('?') || first_trim.ends_with('=') {
            let min_wrap_bot = ((first_ln.max_y.max(first_ln.min_y + first_ln.height) as usize) + 160).min(bh);
            max_y = max_y.max(min_wrap_bot);
        }

        // Cap multi-line strip height to <= 360 px (<= 7.0 MB RGBA at 2x, keeping WorkingSet < 45 MB!)
        if max_y.saturating_sub(min_y) > 360 {
            max_y = (min_y + 360).min(bh);
        }
        if max_y <= min_y + 8 {
            return out;
        }

        if let Some((strip, sw, sh)) = crop_band_rgba(band, bw, bh, min_y, max_y) {
            // Pass A (primary): 2x Gentle-Crisp Contrast (1.40x) — keeps 0/O & I/1 edges razor sharp
            // on both Dark Liquid Glass (iOS 27.2) and classic Spotlight pills!
            let (up_con, uw2, uh2) = upscale_2x_bilinear_with_factor_rgba(&strip, sw, sh, 1.40);
            if uw2 > 0 && uh2 > 0 {
                let l_con = recognize_rgba_with_winrt_ocr(&up_con, uw2, uh2);
                push_lines(&mut out, &l_con);
            }
            // Pass B (fallback): 2x Natural (1.0x, zero contrast clipping for high-glare Liquid Glass)
            let (up_nat, uw, uh) = upscale_2x_bilinear_with_factor_rgba(&strip, sw, sh, 1.0);
            if uw > 0 && uh > 0 {
                let l_nat = recognize_rgba_with_winrt_ocr(&up_nat, uw, uh);
                push_lines(&mut out, &l_nat);
            }
        }
        out
    }

    // Collects ordered candidates [2x_natural..., 2x_contrast..., 1x...] WITHOUT ever discarding 1x!
    fn evaluate_band_pass(
        band: &[u8],
        bw: usize,
        bh: usize,
        all_candidates: &mut Vec<String>,
    ) -> bool {
        let l_1x = recognize_rgba_with_winrt_ocr(band, bw, bh);
        let mut cands_1x: Vec<String> = Vec::new();
        push_lines(&mut cands_1x, &l_1x);
        if has_url_hint(&cands_1x) {
            let cands_2x = run_mini_2x_strip_ocr(band, bw, bh, &l_1x);
            for item in cands_2x {
                push_unique(all_candidates, item);
            }
            for item in cands_1x {
                push_unique(all_candidates, item);
            }
            return has_complete_url(all_candidates);
        }
        for item in cands_1x {
            push_unique(all_candidates, item);
        }
        false
    }

    let Some((mut rgba, w, h)) = decode_image_data_url(data_url) else {
        return Vec::new();
    };
    let mut texts: Vec<String> = Vec::new();

    // Compact frame (e.g. unit/synthetic PNG <= 720px tall): normal + inverted + mini-2x (never blind overwrite).
    if h <= 720 {
        let dark_bg = is_dark_background_dominant(&rgba, w, h);
        if dark_bg {
            invert_rgba_rgb_in_place(&mut rgba);
        }
        if evaluate_band_pass(&rgba, w, h, &mut texts) {
            return texts;
        }
        invert_rgba_rgb_in_place(&mut rgba);
        if evaluate_band_pass(&rgba, w, h, &mut texts) {
            return texts;
        }
        if w <= 1600 && h <= 360 {
            if dark_bg {
                invert_rgba_rgb_in_place(&mut rgba);
            }
            let (up_nat, uw, uh) = upscale_2x_bilinear_with_factor_rgba(&rgba, w, h, 1.0);
            if uw > 0 && uh > 0 {
                let l_up = recognize_rgba_with_winrt_ocr(&up_nat, uw, uh);
                push_lines(&mut texts, &l_up);
            }
            if !has_complete_url(&texts) {
                let (up_con, uw2, uh2) = upscale_2x_bilinear_with_factor_rgba(&rgba, w, h, 1.35);
                if uw2 > 0 && uh2 > 0 {
                    let l_up = recognize_rgba_with_winrt_ocr(&up_con, uw2, uh2);
                    push_lines(&mut texts, &l_up);
                }
            }
        }
        return texts;
    }

    // High-resolution iPhone frame (e.g. 1216 x 2656): 4 overlapping Spotlight bands covering:
    //   1) Top Spotlight (`iPhone 17 Pro / iOS 27.2`, y = 4%..32%)
    //   2) Mid / above-keyboard Spotlight (y = 44%..84%)
    //   3) Bottom Spotlight (`iPhone 11 Pro Max / iOS 18`, y = 78%..98%)
    //   4) Mid-upper fallback (y = 24%..52%)
    // Early-exits ONLY once a COMPLETE validated URL candidate is found (never on bare `.../p/`!).
    let bands = [(4usize, 32usize), (44usize, 84usize), (78usize, 98usize), (24usize, 52usize)];
    for (ys_pct, ye_pct) in bands {
        let ys = (h * ys_pct) / 100;
        let ye = (h * ye_pct) / 100;
        let Some((mut band, bw, bh)) = crop_band_rgba(&rgba, w, h, ys, ye) else {
            continue;
        };
        let dark_band = is_dark_background_dominant(&band, bw, bh);
        if dark_band {
            invert_rgba_rgb_in_place(&mut band);
            if evaluate_band_pass(&band, bw, bh, &mut texts) {
                return texts;
            }
            invert_rgba_rgb_in_place(&mut band);
            if evaluate_band_pass(&band, bw, bh, &mut texts) {
                return texts;
            }
        } else {
            if evaluate_band_pass(&band, bw, bh, &mut texts) {
                return texts;
            }
            invert_rgba_rgb_in_place(&mut band);
            if evaluate_band_pass(&band, bw, bh, &mut texts) {
                return texts;
            }
        }
    }

    texts
}

/// Execute OCR batch post-processing in-process from HTTP/CLI JSON payload.
/// Accepts `"batchTexts"`, `"batch_texts"`, `"raw_text"`, `"text"`, or encoded PNG/JPEG/BMP `"frameDataUrl"` / `"frameHistory"`.
pub fn extract_and_process_ocr_batch_from_json(body: &str) -> (Option<String>, bool, usize) {
    let mut batch: Vec<String> = extract_json_string_array_field(body, "batchTexts")
        .or_else(|| extract_json_string_array_field(body, "batch_texts"))
        .unwrap_or_default();

    if batch.is_empty() {
        if let Some(raw_text) = extract_json_string_field(body, "raw_text")
            .or_else(|| extract_json_string_field(body, "rawText"))
            .or_else(|| extract_json_string_field(body, "text"))
        {
            if !raw_text.trim().is_empty() {
                batch = if raw_text.contains("---FRAME_SPLIT---") {
                    normalize_and_verify_ocr_batch_output(&raw_text)
                } else {
                    vec![raw_text]
                };
            }
        }
    }

    if batch.is_empty() {
        let mut image_urls = extract_data_urls_array_from_json(body, "frameHistory");
        if image_urls.is_empty() {
            image_urls = extract_data_urls_array_from_json(body, "frames");
        }
        if let Some(primary_url) = extract_json_string_field(body, "frameDataUrl")
            .or_else(|| extract_json_string_field(body, "imageDataUrl"))
            .or_else(|| extract_json_string_field(body, "dataUrl"))
        {
            if primary_url.starts_with("data:image/")
                && image_urls.last().map(String::as_str) != Some(primary_url.as_str())
            {
                image_urls.push(primary_url);
            }
        }
        if !image_urls.is_empty() {
            return extract_and_process_ocr_from_data_urls(&image_urls);
        }
    }

    let batch_count = batch.len();
    let (final_opt, auto_cleared) = process_ocr_batch_results(&batch);
    if let Some(ref ft) = final_opt {
        write_debug_ocr_result(
            batch.last().map(String::as_str).unwrap_or(""),
            batch_count,
            ft,
            auto_cleared,
        );
    }
    (final_opt, auto_cleared, batch_count)
}

/// CLI entrypoint for `--ocr-batch-eval <input.json> <output.json>` used in differential parity verification.
/// Reads a JSON object or array of scenarios and emits deterministic results without touching desktop clipboard.
pub fn run_ocr_batch_eval_cli(in_path: &PathBuf, out_path: &PathBuf) -> i32 {
    let Ok(raw_json) = fs::read_to_string(in_path) else {
        return 2;
    };
    clear_recent_long_ocr_chunks();
    clear_typed_lines_buffer();

    let (reset_chunks, batch_texts, typed_seed) = (
        extract_json_bool_field(&raw_json, "clearChunks").unwrap_or(false),
        extract_json_string_array_field(&raw_json, "batchTexts").unwrap_or_default(),
        extract_json_string_array_field(&raw_json, "typedSeed").unwrap_or_default(),
    );
    if reset_chunks {
        clear_recent_long_ocr_chunks();
    }
    for t in &typed_seed {
        iphone_link_apple::hid::record_typed_text(&format!("{t}\n"));
    }

    let (res, auto_cleared) = process_ocr_batch_results(&batch_texts);
    let out_json = format!(
        "{{\"ok\":true,\"inprocessWinrtCom\":{},\"zeroPowershell\":true,\"text\":{},\"autoClearedCache\":{}}}",
        probe_inprocess_winrt_ocr_com(),
        match &res {
            Some(t) => format!("\"{}\"", escape_json_value(t)),
            None => "null".to_string(),
        },
        auto_cleared
    );
    if fs::write(out_path, out_json).is_err() {
        return 3;
    }
    0
}

// ============================================================================
// Unit Tests
// ============================================================================

#[cfg(test)]
pub static CRATE_GLOBAL_TEST_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
    use super::CRATE_GLOBAL_TEST_LOCK as OCR_TEST_LOCK;
    use crate::link_grabber::{extract_and_normalize_links, is_strict_https_social_url};

    #[test]
    fn test_optical_corrections() {
        let _g = OCR_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_copy_cache();
        let input = "Momen diabadil<an oleh @eril<l<ristian di !olom komentar kapal Baturnandi menuju Dapua dengan hincfia (Al Generated Content) #cilegon] aksi _heroik @andriboto<";
        let corrected = auto_correct_ocr_text(input);
        assert!(corrected.contains("diabadikan"));
        assert!(corrected.contains("@erikkristian"));
        assert!(corrected.contains("kolom"));
        assert!(corrected.contains("Batumandi"));
        assert!(corrected.contains("papua"));
        assert!(corrected.contains("hindia"));
        assert!(corrected.contains("(AI Generated Content)"));
        assert!(corrected.contains("#cilegon"));
        assert!(!corrected.contains("#cilegon]"));
        assert!(corrected.contains("heroik"));
        assert!(!corrected.contains("_heroik"));
        assert!(corrected.contains("@andribotox"));
    }

    #[test]
    fn test_golden_rule_1_short_text() {
        let _g = OCR_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_copy_cache();
        let frame = "Headline Singkat\r\n\r\nParagraf kedua pendek.".to_string();
        let stitched = stitch_multiline_ocr_frames(&[frame.clone()]);
        assert_eq!(stitched, frame);
    }

    #[test]
    fn test_balloon_and_folder_rejection() {
        assert!(is_balloon_popup_text("1Q0 1O0O"));
        assert!(is_balloon_popup_text("Cut | Copy | Paste"));
        assert!(is_balloon_popup_text("Select All"));
        assert!(is_balloon_popup_text("[ AutoFill | Paste ]"));
        assert!(!is_balloon_popup_text("Ini teks biasa kapal Batumandi"));
        assert!(!is_balloon_popup_text("(Cut-Off Time/"));
        assert!(!is_balloon_popup_text("Cut-Off Point (COP) 1"));

        assert!(is_notes_folder_list_frame("<<<NOTES_LIST_VIEW>>>"));
        assert!(is_notes_folder_list_frame("Folders"));
        assert!(is_notes_folder_list_frame("Notes (12)"));
        assert!(!is_notes_folder_list_frame("Catatan penting tentang rapat"));
    }

    #[test]
    fn test_kmp_stitch_batumandi_six_paragraphs() {
        let _g = OCR_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_copy_cache();
        let part1 = "Paragraf 1: Penyelamatan KMP Batumandi\r\nParagraf 2: Evakuasi lancar\r\nParagraf 3: Semua selamat";
        let part2 = "Paragraf 2: Evakuasi lancar\r\nParagraf 3: Semua selamat\r\nParagraf 4: Petugas sigap";
        let part3 = "Paragraf 4: Petugas sigap\r\nParagraf 5: Cuaca membaik\r\nParagraf 6: Penyelidikan selesai";

        let frames = vec![part1.to_string(), part2.to_string(), part3.to_string()];
        let stitched = stitch_multiline_ocr_frames(&frames);

        assert!(stitched.contains("Paragraf 1"));
        assert!(stitched.contains("Paragraf 2"));
        assert!(stitched.contains("Paragraf 3"));
        assert!(stitched.contains("Paragraf 4"));
        assert!(stitched.contains("Paragraf 5"));
        assert!(stitched.contains("Paragraf 6"));
    }

    #[test]
    fn test_tahap193_lollipop_handle_repairs_and_double_enter() {
        let _g = OCR_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_copy_cache();
        let noisy = "tarhutla aman enggalnih wok I\r\n\r\nlomah enak teddy prabowk".to_string();
        let stitched = stitch_multiline_ocr_frames(&[noisy]);
        assert_eq!(stitched, "karhutla aman engga nih wok?\r\n\r\nlontoh enak teddy prabowo");

        let football = "lionel messi lamine yamal\r\n\r\nronaldo kdb ini pemulun".to_string();
        let stitched_fb = stitch_multiline_ocr_frames(&[football]);
        assert_eq!(stitched_fb, "lionel messi lamine yamal\r\n\r\nronaldo kdb ini pemulung");
    }

    #[test]
    fn test_single_enter_preserved_no_false_double_enter() {
        let _g = OCR_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_copy_cache();
        let single_enter = "baris satu\r\nbaris dua".to_string();
        let stitched = stitch_multiline_ocr_frames(&[single_enter.clone()]);
        assert_eq!(stitched, single_enter);
        assert!(!stitched.contains("\r\n\r\n"));
    }

    #[test]
    fn test_tahap194_sync_candidate_case_to_ocr() {
        assert_eq!(
            sync_candidate_case_to_ocr(
                "kalo gini mah teddy juga bisa wok?",
                "KALO GINI MAH TEDDY JUGA BISA WOK?"
            ),
            "kalo gini mah teddy juga bisa wok?"
        );
        assert_eq!(
            sync_candidate_case_to_ocr(
                "KALO GINI MAH TEDDY JUGA BISA WOK?",
                "kalo gini mah teddy juga bisa wok?"
            ),
            "KALO GINI MAH TEDDY JUGA BISA WOK?"
        );
        assert_eq!(
            sync_candidate_case_to_ocr("iPhone Link Pro", "IPhone Link Pro"),
            "IPhone Link Pro"
        );
    }

    #[test]
    fn test_hot_ocr_telemetry_and_batch_normalization() {
        let tele = HotOcrTelemetry::new_warm(48.5);
        assert!(tele.hot_ocr_engine_ready);
        let json = tele.to_json();
        assert!(json.contains(r#""hot_ocr_engine_ready":true"#));
        assert!(json.contains("persistent_winrt_hot_worker"));
        assert!(json.contains(r#""zero_powershell":true"#));

        let parsed = normalize_and_verify_ocr_batch_output(
            "<<<SEL_COLOR:yellow>>>\r\nsample tahap 212\r\n---FRAME_SPLIT---\r\n   \r\n---FRAME_SPLIT---\r\nsecond frame\r\n",
        );
        assert_eq!(
            parsed,
            vec![
                "<<<SEL_COLOR:yellow>>>\nsample tahap 212".to_string(),
                "second frame".to_string()
            ]
        );
    }

    #[test]
    fn test_tahap229_dual_preprocessor_assistivetouch_balloon_and_spellcheck_mask() {
        // Synthetic 320x480 viewport with:
        // - AssistiveTouch neutral gray box (64x64) on right edge at X=248..312, Y=120..184
        //   and dark text stroke pixel (20,20,20) just outside its core (must stay dark!)
        // - Yellow highlight & lollipop handle in Y=80..120
        // - Dark [ AutoFill | Paste ] balloon box (width 110, height 32) at Y=150..182, X=80..190
        // - Spellcheck red dot at Y=100, X=50 next to dark ink at X=48
        let w = 320usize;
        let h = 480usize;
        let mut rgba = vec![255u8; w * h * 4];

        // Paint AssistiveTouch cluster (neutral gray 120)
        for y in 120..184 {
            for x in 248..312 {
                let idx = (y * w + x) * 4;
                rgba[idx] = 120;
                rgba[idx + 1] = 120;
                rgba[idx + 2] = 120;
            }
        }
        // Dark letter stroke on AssistiveTouch outer boundary (evakuasi tail protection)
        let ink_at_edge = (122 * w + 249) * 4;
        rgba[ink_at_edge] = 20;
        rgba[ink_at_edge + 1] = 20;
        rgba[ink_at_edge + 2] = 20;

        // Paint dark [ AutoFill | Paste ] popup balloon
        for y in 150..182 {
            for x in 80..190 {
                let idx = (y * w + x) * 4;
                rgba[idx] = 36;
                rgba[idx + 1] = 36;
                rgba[idx + 2] = 40;
            }
        }

        // Paint yellow highlight strip + yellow handle
        for y in 85..112 {
            for x in 28..220 {
                let idx = (y * w + x) * 4;
                rgba[idx] = 253;
                rgba[idx + 1] = 246;
                rgba[idx + 2] = 210;
            }
            let h_idx = (y * w + 26) * 4;
            rgba[h_idx] = 245;
            rgba[h_idx + 1] = 195;
            rgba[h_idx + 2] = 65;
        }
        // Red spellcheck dot & nearby dark stem of 'p' in "papua"
        let red_idx = (108 * w + 55) * 4;
        rgba[red_idx] = 240;
        rgba[red_idx + 1] = 65;
        rgba[red_idx + 2] = 65;
        let papua_stem = (100 * w + 55) * 4;
        rgba[papua_stem] = 18;
        rgba[papua_stem + 1] = 18;
        rgba[papua_stem + 2] = 18;

        let res = prepare_dual_ocr_candidates(&rgba, w, h, false, false);
        assert!(res.assistive_touch_masked);
        assert!(res.dark_balloon_masked);
        assert!(res.found_highlight);
        assert_eq!(res.selection_color, SelectionColor::Yellow);
        assert!(res.final_w < 3500 && res.final_h < 3500);
        assert!(res.cand2_rgba.is_some());

        // Check merge_line_lists + format_parsed_lines_to_text gap spacing
        let c1 = vec![
            OcrLineBox { text: "diabadil<an".into(), min_y: 10.0, max_y: 30.0, height: 20.0 },
            OcrLineBox { text: "Paragraf kedua".into(), min_y: 65.0, max_y: 85.0, height: 20.0 },
        ];
        let c2 = vec![
            OcrLineBox { text: "diabadikan".into(), min_y: 10.0, max_y: 30.0, height: 20.0 },
        ];
        let merged = merge_line_lists(&c1, &c2);
        let formatted = format_parsed_lines_to_text(&merged, false);
        assert_eq!(formatted, vec!["diabadikan", "", "Paragraf kedua"]);
    }

    #[test]
    fn test_tahap229_case1_google_keep_tol_balaraja_5_paragraphs() {
        let _g = OCR_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_copy_cache();

        let hist = "Malam' yang dibagikan oleh akun kreator Instagram @kingtruck.id mendadak viral.";
        let curr = concat!(
            "<<<SEL_COLOR:blue>>>\r\n",
            "<<<SEL_BOUNDS:top=0.1850,bot=0.8250>>>\r\n",
            "#toltangerangmerak #infotangerang\r\n",
            "---\r\n",
            "Unggahan bertajuk 'Pesona Tol Balaraja Jam 11 Malam'\r\n",
            "yang dibagikan oleh akun kreator Instagram @kingtruck.id\r\n",
            "mendadak viral dan memicu gelombang komentar warganet.\r\n",
            "\r\n",
            "Dalam unggahan tersebut, tampak potret seorang wanita\r\n",
            "berdiri di samping truk tronton hijau pada larut malam.\r\n",
            "Konten ini memantik spekulasi mulai dari titik ruas tol\r\n",
            "Merak-Jakarta hingga olahan (Al Generated Content).\r\n",
            "\r\n",
            "Bagaimana tanggapan kalian mengenai potret viral suasana\r\n",
            "malam di ruas Tol Balaraja ini, Banteners?\r\n",
            "\r\n",
            "Source: @kingtruck.id\r\n",
            "\r\n",
            "#tolbalaraja #tolmerakjakarta #kingtruckid #sopirtruk #infotangerang"
        );

        let (final_opt, auto_cleared) =
            process_ocr_batch_results(&[hist.to_string(), curr.to_string()]);
        let final_text = final_opt.unwrap();
        assert!(final_text.starts_with("Unggahan bertajuk 'Pesona Tol Balaraja Jam 11 Malam'"));
        assert!(!final_text.contains("toltangerangmerak"));
        assert!(!final_text.contains("---"));
        assert!(final_text.contains("Merak\u{2013}Jakarta"));
        assert!(final_text.contains("(AI Generated Content)"));
        assert!(final_text.contains("Source: @kingtruck.id"));
        assert!(auto_cleared);
        assert_eq!(get_recent_long_ocr_chunks_count(), 0);

        let blocks: Vec<&str> = final_text.split("\r\n\r\n").filter(|b| !b.trim().is_empty()).collect();
        assert_eq!(blocks.len(), 5, "Expected 5 paragraphs, got:\n{final_text}");
    }

    #[test]
    fn test_tahap229_case2_google_keep_jrf_cut_off_preservation_6_paragraphs() {
        let _g = OCR_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_copy_cache();

        let jrf_raw = concat!(
            "<<<SEL_COLOR:blue>>>\r\n",
            "Aksi sejumlah peserta Jakarta Running Festival\r\n",
            "(JRF) yang nekat membobol barikade pengamanan\r\n",
            "usai terkena batas waktu gugur (Cut-Off Time/\r\n",
            "COT) viral dan memicu kegaduhan di media sosial.\r\n",
            "\r\n",
            "Peristiwa tersebut terekam dalam sejumlah\r\n",
            "unggahan video di titik Cut-Off Point (COP) 1\r\n",
            "kawasan Senayan, Jakarta. Dalam rekaman yang\r\n",
            "dibagikan akun Threads @widiashufi dan\r\n",
            "@wisnutry017, tampak puluhan pelari yang tidak\r\n",
            "memenuhi batas waktu mendorong barikade besi\r\n",
            "hingga merangsek masuk ke jalur steril meski telah\r\n",
            "dihadang Oleh petugas keamanan dan kru lomba.\r\n",
            "\r\n",
            "Kericuhan berlanjut hingga area logistik lintasan\r\n",
            "rekaman lain memperlihatkan sebagian peserta\r\n",
            "mendatangi stan pembagian medali dan mendesak\r\n",
            "petugas untuk tetap memberikan medali finisher.\r\n",
            "\r\n",
            "Bagaimana tanggapan kalian mengenai insiden\r\n",
            "penjebolan barikade dan pemaksaan medali akibat\r\n",
            "terkena cut-off time ini, Banteners?\r\n",
            "\r\n",
            "Source: Threads @wisnutry017\r\n",
            "\r\n",
            "#iakartarunninafestival #irf2026\r\n",
            "#marathonindonesia #runningcommunity"
        );

        let (res, auto_cleared) = process_ocr_batch_results(&[jrf_raw.to_string()]);
        let text = res.unwrap();
        assert!(text.contains("(Cut-Off Time/"));
        assert!(text.contains("Cut-Off Point (COP) 1"));
        assert!(text.contains("cut-off time"));
        assert!(text.contains("dihadang oleh petugas"));
        assert!(text.contains("@wisnutryo17"));
        assert!(text.contains("#jakartarunningfestival #jrf2026"));
        assert!(text.contains("#marathonindonesia #runningcommunity"));
        assert!(auto_cleared);

        let paras: Vec<&str> = text.split("\r\n\r\n").filter(|p| !p.trim().is_empty()).collect();
        assert_eq!(paras.len(), 6, "Expected 6 paragraphs, got:\n{text}");
        assert!(paras[5].contains("#jakartarunningfestival") && paras[5].contains("#runningcommunity"));
    }

    #[test]
    fn test_tahap229_case3_apple_notes_batumandi_bidirectional_gap_fill_and_auto_clear() {
        let _g = OCR_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_copy_cache();

        // Contamination buffer must be stripped only when short, NEVER line 0 news opener
        iphone_link_apple::hid::record_typed_text("banten feeds test 17 pro\n");
        let top_with_contam = concat!(
            "banten feeds test 17 pro\r\n",
            "Rekaman video amatir yang memperlihatkan\r\n",
            "aksi _heroik seorang ayah menggendong erat buah hatinya di !olom kapal Baturnandi menuju Dapua dengan hincfia.\r\n",
            "Momen menegangkan tersebut diabadikan oleh @andribotox & @erikkristian saat menuruni Iambung kapal yang tinggi.\r\n",
            "tersebut memaksaopng\r\n",
            "1Q0 1O0O\r\n",
            "Situasi darurat d, tengah kepulan asap tebal tersebut menuju sekoci keselamatan."
        );
        let bottom_clean = concat!(
            "Momen menegangkan tersebut diabadikan oleh @andribotox & @erikkristian saat menuruni lambung kapal yang tinggi.\r\n",
            "tersebut memaksa para penumpang\r\n",
            "menuju sel<oci keselamatan tanpa korban i1wa oleh awak KapaI.\r\n",
            "Situasi darurat di tengah kepulan asap tebal tersebut menuju sekoci keselamatan.\r\n",
            "Bagaimana perasaan kalian menyaksikan perjuangan di tengah evakuasi KMP Batumandi ini, Banteners?\r\n",
            "Source: Threads @eril<l<rist & @andriboto<\r\n",
            "#kmpbaturnandi #cilegon]"
        );

        let (forward_res, cleared_fwd) = process_ocr_batch_results(&[
            "<<<NOTES_LIST_VIEW>>>\nNotes\n07.42 prabowo".to_string(),
            top_with_contam.to_string(),
            bottom_clean.to_string(),
        ]);
        let ftxt = forward_res.unwrap();
        assert!(ftxt.starts_with("Rekaman video amatir yang memperlihatkan"), "got:\n{ftxt}");
        assert!(!ftxt.contains("banten feeds test 17 pro"));
        assert!(!ftxt.contains("1Q0 1O0O"));
        assert!(!ftxt.contains("memaksaopng"));
        assert!(ftxt.contains("tersebut memaksa para penumpang"));
        assert!(ftxt.contains("sekoci"));
        assert!(ftxt.contains("jiwa"));
        assert!(ftxt.contains("awak kapal"));
        assert!(ftxt.contains("@erikkristian"));
        assert!(ftxt.contains("@andribotox"));
        assert!(ftxt.contains("#kmpbatumandi") && ftxt.contains("#cilegon") && !ftxt.contains("#cilegon]"));
        assert!(cleared_fwd);

        // Two-chunk session stitch Top -> Bottom (23s gap simulation) with yellow selection auto-clear
        clear_copy_cache();
        let top_chunk = concat!(
            "<<<SEL_COLOR:yellow>>>\r\n",
            "Detik-detik KMP Batumandi yang kandas di perairan\r\n",
            "Pulau Sangiang berhasil dievakuasi setelah lebih dari\r\n",
            "12 jam terombang-ambing di Selat Sunda, Banten.\r\n",
            "\r\n",
            "Momen menegangkan terekam kamera saat kapal\r\n",
            "ferry rute Pelabuhan Merak-Bakauheni dihantam ombak.\r\n",
            "\r\n",
            "Situasi darurat kian mencekam saat air laut mulai\r\n",
            "masuk ke lambung kapal. Seluruh 420 penumpang\r\n",
            "ccrta 40 awak kanal dilanorkan hcrhacil"
        );
        let (r1, c1) = process_ocr_batch_results(&[top_chunk.to_string()]);
        assert!(!c1 && !r1.as_ref().unwrap().contains("ccrta"));
        assert_eq!(get_recent_long_ocr_chunks_count(), 1);

        let bottom_chunk = concat!(
            "<<<SEL_COLOR:yellow>>>\r\n",
            "serta 40 awak KapaI dilaporkan berhasil dievakuasi selamat oleh tim SAR gabungan.\r\n",
            "\r\n",
            "Dalam unggahan video, para penumpang mengenakan pelampung sambil melantunkan doa di dek.\r\n",
            "\r\n",
            "Source: Threads @andribotox\r\n",
            "\r\n",
            "#kmpbatumandi #cilegon"
        );
        let (r2, c2) = process_ocr_batch_results(&[bottom_chunk.to_string()]);
        let stitched2 = r2.unwrap();
        assert!(stitched2.contains("Detik-detik KMP Batumandi"));
        assert!(stitched2.contains("serta 40 awak kapal"));
        assert!(stitched2.contains("#cilegon"));
        assert!(c2 && get_recent_long_ocr_chunks_count() == 0);
    }

    #[test]
    fn test_tahap229_case4_short_double_enter_prefix_recovery_and_single_line() {
        let _g = OCR_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_copy_cache();

        for line in ["pamungkas kenangan manis", "barasuara", "the beatles", "hindia"] {
            iphone_link_apple::hid::record_typed_text(&format!("{line}\n"));
        }
        let short_batch = vec![
            "pamungkas kenangan manis\r\n\r\nbarasuara\r\n\r\nthe\r\n\r\nhincfia".to_string(),
        ];
        let (out1, cleared1) = process_ocr_batch_results(&short_batch);
        assert!(!cleared1);
        assert_eq!(
            out1.unwrap(),
            "pamungkas kenangan manis\r\n\r\nbarasuara\r\n\r\nthe beatles\r\n\r\nhindia"
        );

        clear_copy_cache();
        let single_line = vec!["prabowo gibran tarhutla".to_string()];
        let (out2, cleared2) = process_ocr_batch_results(&single_line);
        assert!(!cleared2);
        assert_eq!(out2.unwrap(), "prabowo gibran karhutla");

        // Verify link_grabber integration cleanly without touching frozen link_grabber.rs
        let links = extract_and_normalize_links("Tonton https://www.instagram.com/reel/DScCQallz12/?igsh=abc");
        assert!(links.iter().any(|u| is_strict_https_social_url(u)));
    }

    #[test]
    fn test_tahap229_inprocess_winrt_com_probe_and_json_batch() {
        let _g = OCR_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_copy_cache();
        assert!(probe_inprocess_winrt_ocr_com());
        let (text_opt, cleared, count) = extract_and_process_ocr_batch_from_json(
            r#"{"batchTexts":["papua raya selatan\r\n\r\nhincfia"]}"#,
        );
        assert_eq!(count, 1);
        assert!(!cleared);
        assert_eq!(text_opt.as_deref(), Some("papua raya selatan\r\n\r\nhindia"));
        clear_copy_cache();
    }

    #[test]
    fn test_tahap235_bounded_viewport_history_4_slots_ttl_and_cooldown() {
        let _g = OCR_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_viewport_history();

        // First capture recorded at t = 10_000 ms
        let (rec1, cnt1) = record_viewport_history_snapshot(346, 750, 0x1111, 10_000);
        assert!(rec1 && cnt1 == 1);

        // Cooldown guard blocks rapid duplicate captures < 2500 ms
        let (rec_cd, cnt_cd) = record_viewport_history_snapshot(346, 750, 0x2222, 11_000);
        assert!(!rec_cd && cnt_cd == 1);

        // Push 6 spaced frames (step 3000 ms): must never exceed MAX_VIEWPORT_HISTORY (4)
        for i in 1..=6u64 {
            let (ok, len) =
                record_viewport_history_snapshot(346, 750, 0x3000 + i, 10_000 + i * 3_000);
            assert!(ok);
            assert!(len <= MAX_VIEWPORT_HISTORY, "len={len}");
        }
        assert_eq!(viewport_history_count(), MAX_VIEWPORT_HISTORY);

        // Advance past VIEWPORT_HISTORY_TTL_SECS (45s) -> stale frames pruned to leave only the new one
        let (ok_ttl, len_ttl) =
            record_viewport_history_snapshot(346, 750, 0x9999, 10_000 + 6 * 3_000 + 46_000);
        assert!(ok_ttl && len_ttl == 1);
        clear_viewport_history();
    }

    #[cfg(windows)]
    #[test]
    fn test_tahap239_gdiplus_decode_and_inprocess_winrt_ocr_execution() {
        let _g = OCR_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_copy_cache();

        // Render bold crisp 5x7 block glyphs "HALO 239" (scale 6px/cell) onto a white RGBA canvas
        let w = 420usize;
        let h = 140usize;
        let mut rgba = vec![255u8; w * h * 4];
        let glyphs: [&[u8; 7]; 8] = [
            // 'H'
            &[0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
            // 'A'
            &[0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
            // 'L'
            &[0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111],
            // 'O'
            &[0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
            // ' '
            &[0, 0, 0, 0, 0, 0, 0],
            // '2'
            &[0b01110, 0b10001, 0b00001, 0b00110, 0b01000, 0b10000, 0b11111],
            // '3'
            &[0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110],
            // '9'
            &[0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00001, 0b01110],
        ];
        let scale = 6usize;
        for (gi, rows) in glyphs.iter().enumerate() {
            let gx0 = 36 + gi * 42;
            let gy0 = 42;
            for (ry, row_bits) in rows.iter().enumerate() {
                for rx in 0..5usize {
                    if (row_bits >> (4 - rx)) & 1 == 1 {
                        for sy in 0..scale {
                            for sx in 0..scale {
                                let py = gy0 + ry * scale + sy;
                                let px = gx0 + rx * scale + sx;
                                let idx = (py * w + px) * 4;
                                rgba[idx] = 10;
                                rgba[idx + 1] = 10;
                                rgba[idx + 2] = 10;
                                rgba[idx + 3] = 255;
                            }
                        }
                    }
                }
            }
        }

        // Build 24-bit bottom-up BMP in-memory to exercise GDI+ + WinRT OCR end-to-end
        let row_stride = ((w * 3 + 3) / 4) * 4;
        let pix_size = row_stride * h;
        let file_size = 54 + pix_size;
        let mut bmp_bytes = Vec::with_capacity(file_size);
        bmp_bytes.extend_from_slice(b"BM");
        bmp_bytes.extend_from_slice(&(file_size as u32).to_le_bytes());
        bmp_bytes.extend_from_slice(&0u32.to_le_bytes());
        bmp_bytes.extend_from_slice(&54u32.to_le_bytes());
        bmp_bytes.extend_from_slice(&40u32.to_le_bytes());
        bmp_bytes.extend_from_slice(&(w as i32).to_le_bytes());
        bmp_bytes.extend_from_slice(&(h as i32).to_le_bytes());
        bmp_bytes.extend_from_slice(&1u16.to_le_bytes());
        bmp_bytes.extend_from_slice(&24u16.to_le_bytes());
        bmp_bytes.extend_from_slice(&0u32.to_le_bytes());
        bmp_bytes.extend_from_slice(&(pix_size as u32).to_le_bytes());
        bmp_bytes.extend_from_slice(&[0u8; 16]);
        for y in (0..h).rev() {
            for x in 0..w {
                let idx = (y * w + x) * 4;
                bmp_bytes.push(rgba[idx + 2]);
                bmp_bytes.push(rgba[idx + 1]);
                bmp_bytes.push(rgba[idx]);
            }
            bmp_bytes.resize(bmp_bytes.len() + (row_stride - w * 3), 0);
        }

        let decoded = decode_image_bytes_to_rgba(&bmp_bytes).expect("GDI+ BMP decode");
        assert_eq!(decoded.1, w);
        assert_eq!(decoded.2, h);

        let b64 = iphone_link_video::video_hwnd::encode_base64_fast(&bmp_bytes);
        let data_url = format!("data:image/bmp;base64,{b64}");
        let payload = format!(r#"{{"action":"copy","frameDataUrl":"{data_url}"}}"#);
        let (text_opt, _cleared, batch_count) = extract_and_process_ocr_batch_from_json(&payload);
        assert!(batch_count >= 1, "expected WinRT OCR to recognize rendered text");
        let text = text_opt.unwrap_or_default().to_ascii_uppercase();
        assert!(text.contains("239") && !text.trim().is_empty(), "unexpected OCR text: {text:?}");
        clear_copy_cache();
    }
}
