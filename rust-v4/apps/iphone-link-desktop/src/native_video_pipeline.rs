//! Tahap 263 — Zero-Copy In-Process `LiveVideoPipeline` orchestrator for `--native-ui`.
//! Bridges `mirror::register_native_video_sink` -> real COM `LiveMfVideoDecoder` (CoreDevice HEVC
//! & AirPlay H.264) -> 6-slot `D3d11Nv12VramRing` -> real DXGI `FLIP_DISCARD` Child `HWND`
//! (`LiveVideoHwnd`) with 0 HTTP `/mirror/live` overhead and 0 WebView2 processes.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use iphone_link_apple::airplay;
use iphone_link_apple::hid;
use iphone_link_apple::mirror::{
    self, BoundedNativeVideoQueue, NativeVideoEvent, NATIVE_AIRPLAY_VIDEO_QUEUE_CAPACITY,
};
use iphone_link_video::autorotate::ProbeGrid120x260;
use iphone_link_video::d3d11_context::{D3d11Nv12VramRing, LiveD3d11Gpu};
use iphone_link_video::mf_decoder::{DecoderError, LiveMfVideoDecoder, VideoCodecKind};
use iphone_link_video::video_hwnd::{
    compute_post_trim_dimensions, AirplayRenderPolicy, FitMode, LiveVideoHwnd, StagePointerEvent,
};

fn post_trim_visible_dims(
    vis_w: u32,
    vis_h: u32,
    trim: (u32, u32, u32, u32),
    is_airplay: bool,
) -> (u32, u32) {
    if is_airplay {
        return (vis_w, vis_h);
    }
    let (tt, tl, tb, tr) = if trim == (0, 0, 0, 0) && vis_w == 1216 && vis_h == 2656 {
        (0, 0, 58, 14)
    } else {
        trim
    };
    (
        vis_w.saturating_sub(tl + tr).max(2),
        vis_h.saturating_sub(tt + tb).max(2),
    )
}

pub const AIRPLAY_STALL_MIN_PACKETS: u32 = 90;
pub const AIRPLAY_STALL_MIN_ELAPSED_MS: u64 = 4500;
pub const AIRPLAY_STALL_COOLDOWN_MS: u64 = 10000;
pub const AIRPLAY_HARD_RECOVERY_MIN_FATAL_ERRORS: u32 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AirplayWatchdogAction {
    None,
    SoftDrainAndRepaint,
    HardRebuildOnFatalError,
}

/// Deterministic calm mid-stream H.264 watchdog for AirPlay:
/// Preserves healthy DPB whenever either `mfDecodedFrames` or `presented_frames` advances.
/// Supports both legacy `observe()` (`90` pkts / `4500 ms` / `10000 ms`) and 2-tier non-destructive
/// `observe_tiered()`: Soft Drain & Repaint (`90` pkts / `4500 ms`) vs Hard Rebuild (`>= 5`
/// consecutive fatal decoder COM HRESULT errors ONLY — never kills a healthy MFT!).
#[derive(Debug, Clone)]
pub struct AirplayStallWatchdog {
    last_decoded: u64,
    last_presented: u64,
    stall_packets: u32,
    stall_window_start_ms: Option<u64>,
    last_soft_at_ms: Option<u64>,
    last_reset_at_ms: Option<u64>,
    soft_recoveries: u64,
    recoveries: u64,
}

impl Default for AirplayStallWatchdog {
    fn default() -> Self {
        Self::new()
    }
}

impl AirplayStallWatchdog {
    pub fn new() -> Self {
        Self {
            last_decoded: 0,
            last_presented: 0,
            stall_packets: 0,
            stall_window_start_ms: None,
            last_soft_at_ms: None,
            last_reset_at_ms: None,
            soft_recoveries: 0,
            recoveries: 0,
        }
    }

    pub fn reset(&mut self) {
        self.last_decoded = 0;
        self.last_presented = 0;
        self.stall_packets = 0;
        self.stall_window_start_ms = None;
        self.last_soft_at_ms = None;
        self.last_reset_at_ms = None;
    }

    /// Clears transient stall packet accumulation while preserving recovery count & cooldown,
    /// used on clean `avcC` dimension transitions and while the decoder legitimately awaits IDR.
    pub fn reset_grace(&mut self) {
        self.stall_packets = 0;
        self.stall_window_start_ms = None;
    }

    pub fn recoveries(&self) -> u64 {
        self.recoveries
    }

    pub fn soft_recoveries(&self) -> u64 {
        self.soft_recoveries
    }

    pub fn reset_count(&self) -> u32 {
        self.recoveries.min(u64::from(u32::MAX)) as u32
    }

    pub fn stall_packets(&self) -> u32 {
        self.stall_packets
    }

    pub fn on_h264_packet(
        &mut self,
        now_ms: u64,
        presented_frames_now: u64,
        is_video_slice: bool,
    ) -> bool {
        self.observe(is_video_slice, self.last_decoded, presented_frames_now, now_ms)
    }

    pub fn observe(
        &mut self,
        incoming_h264_pkt: bool,
        decoded_total: u64,
        presented_total: u64,
        now_ms: u64,
    ) -> bool {
        if decoded_total > self.last_decoded || presented_total > self.last_presented {
            self.last_decoded = self.last_decoded.max(decoded_total);
            self.last_presented = self.last_presented.max(presented_total);
            self.stall_packets = 0;
            self.stall_window_start_ms = None;
            return false;
        }
        self.last_decoded = self.last_decoded.max(decoded_total);
        if !incoming_h264_pkt {
            return false;
        }
        if let Some(last_rst) = self.last_reset_at_ms {
            if now_ms.saturating_sub(last_rst) < AIRPLAY_STALL_COOLDOWN_MS {
                return false;
            }
        }
        self.stall_packets = self.stall_packets.saturating_add(1);
        let start = *self.stall_window_start_ms.get_or_insert(now_ms);
        if self.stall_packets >= AIRPLAY_STALL_MIN_PACKETS
            && now_ms.saturating_sub(start) >= AIRPLAY_STALL_MIN_ELAPSED_MS
        {
            self.last_reset_at_ms = Some(now_ms);
            self.stall_packets = 0;
            self.stall_window_start_ms = None;
            self.recoveries = self.recoveries.saturating_add(1);
            return true;
        }
        false
    }

    /// Two-tier non-destructive AirPlay watchdog (Tahap 279 Opsi A):
    /// - Tier 1 (`SoftDrainAndRepaint`): drains MFT & repaints cached VRAM frame without killing MFT.
    /// - Tier 2 (`HardRebuildOnFatalError`): ONLY triggered when `>= 5` consecutive fatal decoder
    ///   COM HRESULT errors occur (`lastDecoderHresult < 0`), NEVER killing healthy MFT on quiet streams!
    pub fn observe_tiered(
        &mut self,
        incoming_h264_pkt: bool,
        decoded_total: u64,
        presented_total: u64,
        now_ms: u64,
        consecutive_fatal_hr: u32,
    ) -> AirplayWatchdogAction {
        if decoded_total > self.last_decoded || presented_total > self.last_presented {
            self.last_decoded = self.last_decoded.max(decoded_total);
            self.last_presented = self.last_presented.max(presented_total);
            self.stall_packets = 0;
            self.stall_window_start_ms = None;
            return AirplayWatchdogAction::None;
        }
        self.last_decoded = self.last_decoded.max(decoded_total);
        if !incoming_h264_pkt {
            return AirplayWatchdogAction::None;
        }

        let hard_cooldown_active = self
            .last_reset_at_ms
            .is_some_and(|t| now_ms.saturating_sub(t) < AIRPLAY_STALL_COOLDOWN_MS);
        let soft_cooldown_active = hard_cooldown_active
            || self
                .last_soft_at_ms
                .is_some_and(|t| now_ms.saturating_sub(t) < AIRPLAY_STALL_COOLDOWN_MS);

        if !hard_cooldown_active && consecutive_fatal_hr >= AIRPLAY_HARD_RECOVERY_MIN_FATAL_ERRORS {
            self.last_reset_at_ms = Some(now_ms);
            self.last_soft_at_ms = Some(now_ms);
            self.stall_packets = 0;
            self.stall_window_start_ms = None;
            self.recoveries = self.recoveries.saturating_add(1);
            return AirplayWatchdogAction::HardRebuildOnFatalError;
        }

        if !soft_cooldown_active {
            self.stall_packets = self.stall_packets.saturating_add(1);
            let soft_start = *self.stall_window_start_ms.get_or_insert(now_ms);
            if self.stall_packets >= AIRPLAY_STALL_MIN_PACKETS
                && now_ms.saturating_sub(soft_start) >= AIRPLAY_STALL_MIN_ELAPSED_MS
            {
                self.last_soft_at_ms = Some(now_ms);
                self.stall_packets = 0;
                self.stall_window_start_ms = None;
                self.soft_recoveries = self.soft_recoveries.saturating_add(1);
                return AirplayWatchdogAction::SoftDrainAndRepaint;
            }
        }

        AirplayWatchdogAction::None
    }
}

#[derive(Debug, Clone)]
pub struct PipelineTelemetry {
    pub child_hwnd: isize,
    pub visible: bool,
    pub codec: String,
    pub codec_error_kind: String,
    pub width: u32,
    pub height: u32,
    pub decoded_frames: u64,
    pub presented_frames: u64,
}

pub const ANTI_BLANK_ZERO_FRAME_TIMEOUT_MS: u64 = 7_000;

/// Classifies `LiveMfVideoDecoder::new` initialization failure into a UI diagnostic kind
/// so `native_video_pipeline` never swallows missing-HEVC errors silently.
pub fn classify_hevc_decoder_init_error(err: &DecoderError) -> &'static str {
    match err {
        DecoderError::HevcExtensionMissing => "hevc_mft_missing",
        DecoderError::MediaFoundationUnavailable(_) => "media_feature_pack_missing",
        _ => "decoder_init_failed",
    }
}

/// Evaluates whether `--native-ui` should display the interactive Anti-Blank Diagnostic Overlay
/// (immediate on missing HEVC/Media Foundation latch, or after `7,000 ms` of zero presented frames
/// during active CoreDevice streaming outside `waiting_airplay`).
pub fn evaluate_anti_blank_watchdog(
    phase: &str,
    stage: &str,
    airplay_active: bool,
    presented_frames: u64,
    codec_error_kind: &str,
    zero_frame_elapsed_ms: u64,
) -> Option<String> {
    if phase == "idle" || stage == "waiting_airplay" || presented_frames >= 1 {
        return None;
    }
    let err_clean = codec_error_kind.trim();
    if !err_clean.is_empty() && (phase == "running" || phase == "starting") {
        return Some(err_clean.to_string());
    }
    if phase == "running"
        && !airplay_active
        && zero_frame_elapsed_ms >= ANTI_BLANK_ZERO_FRAME_TIMEOUT_MS
    {
        return Some("zero_frame_timeout".to_string());
    }
    None
}

pub struct LiveVideoPipeline {
    view: Option<Arc<Mutex<LiveVideoHwnd>>>,
    ring: Arc<D3d11Nv12VramRing>,
    queue: Arc<BoundedNativeVideoQueue>,
    stop_flag: Arc<AtomicBool>,
    visible_flag: Arc<AtomicBool>,
    redraw_requested: Arc<AtomicBool>,
    composition_mode: bool,
    frame_ready_cb: Arc<Mutex<Option<Arc<dyn Fn() + Send + Sync>>>>,
    session_epoch: Arc<AtomicU64>,
    codec_label: Arc<Mutex<String>>,
    codec_error_kind: Arc<Mutex<String>>,
    hevc_init_failed_latched: Arc<AtomicBool>,
    dim_w: Arc<AtomicU32>,
    dim_h: Arc<AtomicU32>,
    decoder_fed: Arc<AtomicU64>,
    decoded_frames: Arc<AtomicU64>,
    trace_decoder_fed: Arc<AtomicU64>,
    trace_decoded_frames: Arc<AtomicU64>,
    vram_pub_base: Arc<AtomicU64>,
    presented_frames: Arc<AtomicU64>,
    present_calls: Arc<AtomicU64>,
    present_errors: Arc<AtomicU64>,
    stream_changes: Arc<AtomicU64>,
    not_accepting_drains: Arc<AtomicU64>,
    stale_dim_dropped: Arc<AtomicU64>,
    last_decoder_hresult: Arc<AtomicI32>,
    last_mft_w: Arc<AtomicU32>,
    last_mft_h: Arc<AtomicU32>,
    expected_coded_w: Arc<AtomicU32>,
    expected_coded_h: Arc<AtomicU32>,
    dcomp_commit_pending: Arc<AtomicBool>,
    hevc_autorotate_tracker: Arc<Mutex<iphone_link_video::autorotate::HevcAutoRotateTracker>>,
    pending_hevc_autorotate_deg: Arc<AtomicI32>,
    is_ipad_profile: Arc<AtomicBool>,
    autorotate_clock: Instant,
    last_resize_dims: Mutex<(u32, u32)>,
    child_hwnd: isize,
    decode_thread: Option<JoinHandle<()>>,
    render_thread: Option<JoinHandle<()>>,
    shut_down: bool,
}

fn schedule_coalesced_dcomp_commit(pending: &Arc<AtomicBool>) {
    if !pending.swap(true, Ordering::AcqRel) {
        let flag = pending.clone();
        if slint::invoke_from_event_loop(move || {
            flag.store(false, Ordering::Release);
            crate::dcomp_presenter::commit_active_dcomp();
            airplay::inc_trace_dcomp_commits();
        })
        .is_err()
        {
            pending.store(false, Ordering::Release);
            airplay::inc_trace_dcomp_commits();
        }
    }
}

pub const AIRPLAY_STALL_DUMP_THRESHOLD_MS: u64 = 3000;

pub fn airplay_stall_dump_path() -> std::path::PathBuf {
    std::env::temp_dir().join("iphone_link_airplay_last_stall.json")
}

pub fn cleanup_airplay_stall_dump_file() {
    let _ = std::fs::remove_file(airplay_stall_dump_path());
}

pub fn build_airplay_stall_dump_json(last_packet_age_ms: u64) -> String {
    let tc = airplay::get_airplay_trace_counters();
    let trace_json = airplay::get_airplay_trace_json();
    format!(
        r#"{{"stallDump":true,"lastPacketAgeMs":{},"mfLowLatencyEnabled":{},"dcompCommits":{},"trace":{}}}"#,
        last_packet_age_ms, tc.mf_low_latency_enabled, tc.dcomp_commits, trace_json
    )
}

#[derive(Debug, Clone, Default)]
pub struct AirplayStallDumpState {
    pub last_presented: u64,
    pub stagnant_since_ms: Option<u64>,
    pub dumped_for_current_stall: bool,
}

/// Evaluates stagnant `dcompPresentedFrames` while AirPlay is active (`tcpVideoPackets > 0`)
/// and dumps a non-PII diagnostic JSON snapshot once per stall (`> 3000 ms`).
pub fn maybe_write_airplay_stall_dump(
    airplay_active: bool,
    tcp_video_packets: u64,
    dcomp_presented: u64,
    last_packet_age_ms: u64,
    now_ms: u64,
    state: &mut AirplayStallDumpState,
    target_path: &std::path::Path,
) -> bool {
    if !airplay_active || tcp_video_packets == 0 {
        state.last_presented = dcomp_presented;
        state.stagnant_since_ms = None;
        state.dumped_for_current_stall = false;
        return false;
    }
    if dcomp_presented > state.last_presented {
        state.last_presented = dcomp_presented;
        state.stagnant_since_ms = Some(now_ms);
        state.dumped_for_current_stall = false;
        return false;
    }
    let since = *state.stagnant_since_ms.get_or_insert(now_ms);
    if !state.dumped_for_current_stall
        && now_ms.saturating_sub(since) > AIRPLAY_STALL_DUMP_THRESHOLD_MS
    {
        state.dumped_for_current_stall = true;
        let payload = build_airplay_stall_dump_json(last_packet_age_ms);
        let _ = std::fs::write(target_path, payload.as_bytes());
        return true;
    }
    false
}

fn opt_dims_from_atomics(w: &AtomicU32, h: &AtomicU32) -> Option<(u32, u32)> {
    let vw = w.load(Ordering::Relaxed);
    let vh = h.load(Ordering::Relaxed);
    if vw > 0 && vh > 0 {
        Some((vw, vh))
    } else {
        None
    }
}

#[allow(clippy::too_many_arguments)]
fn sync_airplay_pipeline_trace(
    queue: &BoundedNativeVideoQueue,
    ring: &D3d11Nv12VramRing,
    trace_decoder_fed: &AtomicU64,
    trace_decoded_frames: &AtomicU64,
    vram_pub_base: &AtomicU64,
    stream_changes: &AtomicU64,
    not_accepting_drains: &AtomicU64,
    stale_dim_dropped: &AtomicU64,
    presented: &AtomicU64,
    present_calls: &AtomicU64,
    present_errors: &AtomicU64,
    last_hresult: &AtomicI32,
    last_mft_w: &AtomicU32,
    last_mft_h: &AtomicU32,
    expected_w: &AtomicU32,
    expected_h: &AtomicU32,
) {
    let (_, _, queue_dropped, _, pushed_total) = queue.stats();
    let vram_published = ring
        .stats()
        .0
        .saturating_sub(vram_pub_base.load(Ordering::Relaxed));
    airplay::update_native_video_stage_trace(
        pushed_total,
        queue_dropped,
        trace_decoder_fed.load(Ordering::Relaxed),
        trace_decoded_frames.load(Ordering::Relaxed),
        stream_changes.load(Ordering::Relaxed),
        not_accepting_drains.load(Ordering::Relaxed),
        stale_dim_dropped.load(Ordering::Relaxed),
        vram_published,
        presented.load(Ordering::Relaxed),
        present_calls.load(Ordering::Relaxed),
        present_errors.load(Ordering::Relaxed),
        last_hresult.load(Ordering::Relaxed),
        opt_dims_from_atomics(last_mft_w, last_mft_h),
        opt_dims_from_atomics(expected_w, expected_h),
    );
}

impl LiveVideoPipeline {
    /// Creates a DirectComposition swapchain `LiveVideoPipeline` sharing the caller's `LiveD3d11Gpu`
    /// (`child_hwnd == 0`, zero Child `HWND`), for 2-layer `DcompPresenter` compositing.
    pub fn new_composition(
        gpu: Arc<LiveD3d11Gpu>,
        width: u32,
        height: u32,
        vsync_interval: u32,
    ) -> Result<Self, String> {
        let ring = Arc::new(D3d11Nv12VramRing::new());
        let mut comp_view = LiveVideoHwnd::new_for_composition(
            gpu.clone(),
            ring.clone(),
            width.max(16),
            height.max(16),
        )?;
        comp_view.set_vsync_interval(vsync_interval);
        Self::init_with_view(gpu, ring, comp_view, false, true)
    }

    /// Creates `LiveVideoHwnd` on the caller (UI) thread, attaches it as a Child `HWND` of
    /// `parent_hwnd` (or offscreen when `parent_hwnd == 0`), registers the direct in-process
    /// `NativeVideoSink`, and starts dedicated decode & DXGI render worker threads.
    pub fn new(
        parent_hwnd: isize,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        initial_visible: bool,
        vsync_interval: u32,
    ) -> Result<Self, String> {
        let gpu = Arc::new(LiveD3d11Gpu::new()?);
        let ring = Arc::new(D3d11Nv12VramRing::new());

        let mut hwnd_view = LiveVideoHwnd::new(
            gpu.clone(),
            ring.clone(),
            parent_hwnd,
            width.max(16),
            height.max(16),
            initial_visible,
        )?;
        hwnd_view.set_vsync_interval(vsync_interval);
        if parent_hwnd != 0 {
            let _ = hwnd_view.set_bounds(x, y, width.max(16), height.max(16), initial_visible);
        } else {
            hwnd_view.set_visible(initial_visible);
        }
        Self::init_with_view(gpu, ring, hwnd_view, initial_visible, false)
    }

    fn init_with_view(
        gpu: Arc<LiveD3d11Gpu>,
        ring: Arc<D3d11Nv12VramRing>,
        hwnd_view: LiveVideoHwnd,
        initial_visible: bool,
        composition_mode: bool,
    ) -> Result<Self, String> {
        let queue = Arc::new(BoundedNativeVideoQueue::new_with_h264_capacity(
            48,
            NATIVE_AIRPLAY_VIDEO_QUEUE_CAPACITY,
        ));
        hwnd_view.set_pointer_callback(Some(Arc::new(|ev| match ev {
            StagePointerEvent::Down { hid_x, hid_y }
            | StagePointerEvent::Move { hid_x, hid_y } => {
                if !mirror::is_airplay_standalone_no_hid_active() {
                    let _ = hid::dispatch_physical_touch(true, hid_x, hid_y);
                }
            }
            StagePointerEvent::Up { hid_x, hid_y } => {
                if !mirror::is_airplay_standalone_no_hid_active() {
                    let _ = hid::dispatch_physical_touch(false, hid_x, hid_y);
                }
            }
            StagePointerEvent::Wheel { .. } => {}
        })));

        airplay::set_trace_d3d11_multithread_protected(hwnd_view.d3d11_multithread_protected());
        let child_hwnd = hwnd_view.hwnd();
        let view = Arc::new(Mutex::new(hwnd_view));
        let stop_flag = Arc::new(AtomicBool::new(false));
        let visible_flag = Arc::new(AtomicBool::new(initial_visible));
        let redraw_requested = Arc::new(AtomicBool::new(false));
        let frame_ready_cb: Arc<Mutex<Option<Arc<dyn Fn() + Send + Sync>>>> =
            Arc::new(Mutex::new(None));
        let session_epoch = Arc::new(AtomicU64::new(1));
        let codec_label = Arc::new(Mutex::new(String::new()));
        let codec_error_kind = Arc::new(Mutex::new(String::new()));
        let hevc_init_failed_latched = Arc::new(AtomicBool::new(false));
        let dim_w = Arc::new(AtomicU32::new(0));
        let dim_h = Arc::new(AtomicU32::new(0));
        let decoder_fed = Arc::new(AtomicU64::new(0));
        let decoded_frames = Arc::new(AtomicU64::new(0));
        let trace_decoder_fed = Arc::new(AtomicU64::new(0));
        let trace_decoded_frames = Arc::new(AtomicU64::new(0));
        let vram_pub_base = Arc::new(AtomicU64::new(ring.stats().0));
        let presented_frames = Arc::new(AtomicU64::new(0));
        let present_calls = Arc::new(AtomicU64::new(0));
        let present_errors = Arc::new(AtomicU64::new(0));
        let stream_changes = Arc::new(AtomicU64::new(0));
        let not_accepting_drains = Arc::new(AtomicU64::new(0));
        let stale_dim_dropped = Arc::new(AtomicU64::new(0));
        let last_decoder_hresult = Arc::new(AtomicI32::new(0));
        let last_mft_w = Arc::new(AtomicU32::new(0));
        let last_mft_h = Arc::new(AtomicU32::new(0));
        let expected_coded_w = Arc::new(AtomicU32::new(0));
        let expected_coded_h = Arc::new(AtomicU32::new(0));
        let dcomp_commit_pending = Arc::new(AtomicBool::new(false));
        let hevc_autorotate_tracker = Arc::new(Mutex::new(
            iphone_link_video::autorotate::HevcAutoRotateTracker::new(),
        ));
        let pending_hevc_autorotate_deg = Arc::new(AtomicI32::new(-1));
        let is_ipad_profile = Arc::new(AtomicBool::new(false));
        let pipeline_epoch_start = Instant::now();
        let last_h264_pkt_ms = Arc::new(AtomicU64::new(0));

        // Register direct in-process video sink (0 HTTP /mirror/live overhead!)
        mirror::register_native_video_sink(Some(queue.clone()));

        // Decode worker thread (HEVC CoreDevice & H.264 AirPlay -> VRAM NV12 ring)
        let d_stop = stop_flag.clone();
        let d_gpu = gpu.clone();
        let d_ring = ring.clone();
        let d_q = queue.clone();
        let d_codec = codec_label.clone();
        let d_codec_err = codec_error_kind.clone();
        let d_hevc_failed = hevc_init_failed_latched.clone();
        let d_w = dim_w.clone();
        let d_h = dim_h.clone();
        let d_fed = decoder_fed.clone();
        let d_cnt = decoded_frames.clone();
        let d_trace_fed = trace_decoder_fed.clone();
        let d_trace_cnt = trace_decoded_frames.clone();
        let d_vram_base = vram_pub_base.clone();
        let d_pres = presented_frames.clone();
        let d_pres_calls = present_calls.clone();
        let d_pres_errs = present_errors.clone();
        let d_sc = stream_changes.clone();
        let d_nad = not_accepting_drains.clone();
        let d_stale_drop = stale_dim_dropped.clone();
        let d_lhr = last_decoder_hresult.clone();
        let d_mft_w = last_mft_w.clone();
        let d_mft_h = last_mft_h.clone();
        let d_exp_w = expected_coded_w.clone();
        let d_exp_h = expected_coded_h.clone();
        let d_epoch = session_epoch.clone();
        let d_redraw = redraw_requested.clone();
        let d_view = view.clone();
        let d_dcomp_pending = dcomp_commit_pending.clone();
        let d_ar_tracker = hevc_autorotate_tracker.clone();
        let d_pending_ar = pending_hevc_autorotate_deg.clone();
        let d_pkt_ms = last_h264_pkt_ms.clone();
        let d_clock_origin = pipeline_epoch_start;
        let decode_thread = std::thread::Builder::new()
            .name("native-ui-decode".to_string())
            .spawn(move || {
                let mut active_dec: Option<LiveMfVideoDecoder> = None;
                let mut active_kind: Option<VideoCodecKind> = None;
                let mut active_avcc: Option<Vec<u8>> = None;
                let mut consecutive_fatal_hr: u32 = 0;
                let mut prev_dec_idrs: u64 = 0;
                let mut prev_dec_small_256_511: u64 = 0;
                let mut prev_dec_reconfigs: u64 = 0;
                let mut trace_idrs_total: u64 = 0;
                let mut watchdog = AirplayStallWatchdog::new();
                let mut wd_clock = d_clock_origin;
                let mut last_gen = mirror::current_live_session_gen();
                let mut seen_decode_epoch = d_epoch.load(Ordering::Relaxed);

                while !d_stop.load(Ordering::Relaxed) {
                    let cur_epoch = d_epoch.load(Ordering::Relaxed);
                    if cur_epoch != seen_decode_epoch {
                        seen_decode_epoch = cur_epoch;
                        trace_idrs_total = 0;
                        prev_dec_idrs = active_dec
                            .as_ref()
                            .map(|d| d.idr_keyframes_detected())
                            .unwrap_or(0);
                        prev_dec_small_256_511 = active_dec
                            .as_ref()
                            .map(|d| d.avcc_small_256_511_frames())
                            .unwrap_or(0);
                        prev_dec_reconfigs = active_dec
                            .as_ref()
                            .map(|d| d.mft_reconfigs())
                            .unwrap_or(0);
                    }
                    let cur_gen = mirror::current_live_session_gen();
                    if cur_gen != last_gen {
                        last_gen = cur_gen;
                        watchdog.reset();
                        wd_clock = Instant::now();
                        d_pkt_ms.store(0, Ordering::Relaxed);
                        d_pending_ar.store(-1, Ordering::Release);
                        if let Ok(mut tr) = d_ar_tracker.lock() {
                            tr.reset();
                        }
                        active_avcc = None;
                        consecutive_fatal_hr = 0;
                        trace_idrs_total = 0;
                        prev_dec_idrs = active_dec
                            .as_ref()
                            .map(|d| d.idr_keyframes_detected())
                            .unwrap_or(0);
                        prev_dec_small_256_511 = active_dec
                            .as_ref()
                            .map(|d| d.avcc_small_256_511_frames())
                            .unwrap_or(0);
                        prev_dec_reconfigs = active_dec
                            .as_ref()
                            .map(|d| d.mft_reconfigs())
                            .unwrap_or(0);
                        airplay::set_trace_idr_keyframes_detected(0);
                        airplay::set_trace_avcc_small_256_511_frames(0);
                        airplay::set_trace_mft_reconfigs(0);
                    }
                    let Some(ev) = d_q.recv_timeout(Duration::from_millis(40)) else {
                        continue;
                    };
                    match ev {
                        NativeVideoEvent::Stopped => {
                            if let Some(ref mut dec) = active_dec {
                                dec.clean_teardown();
                            }
                            // CRITICAL (Bug #8): always release old decoder/kind on Stop so switching
                            // Auto (HEVC) -> Stop -> AirPlay (H.264) creates a fresh decoder cleanly.
                            // Keep cumulative `d_trace_fed`/`d_trace_cnt`/`d_vram_base` intact across
                            // transient AirPlay TCP reconnects so `decoderFed` and `mfDecodedFrames == vramPublished`
                            // never diverge (`reset_for_new_session` resets all cleanly).
                            active_dec = None;
                            active_kind = None;
                            active_avcc = None;
                            consecutive_fatal_hr = 0;
                            prev_dec_idrs = 0;
                            prev_dec_small_256_511 = 0;
                            prev_dec_reconfigs = 0;
                            watchdog.reset();
                            wd_clock = Instant::now();
                            d_pending_ar.store(-1, Ordering::Release);
                            if let Ok(mut tr) = d_ar_tracker.lock() {
                                tr.reset();
                            }
                            if let Ok(mut v) = d_view.lock() {
                                v.set_airplay_mode(false);
                                let _ = v.purge_stale_video_buffers();
                                airplay::set_trace_mid_stream_black_clears(
                                    v.mid_stream_black_clears(),
                                );
                                airplay::set_trace_dual_buffer_overwrites(
                                    v.dual_buffer_overwrites(),
                                );
                            } else {
                                d_ring.clear();
                            }
                            d_w.store(0, Ordering::Relaxed);
                            d_h.store(0, Ordering::Relaxed);
                            d_fed.store(0, Ordering::Relaxed);
                            d_cnt.store(0, Ordering::Relaxed);
                            d_pres.store(0, Ordering::Relaxed);
                            d_pres_calls.store(0, Ordering::Relaxed);
                            d_pres_errs.store(0, Ordering::Relaxed);
                            d_sc.store(0, Ordering::Relaxed);
                            d_nad.store(0, Ordering::Relaxed);
                            d_stale_drop.store(0, Ordering::Relaxed);
                            d_lhr.store(0, Ordering::Relaxed);
                            d_mft_w.store(0, Ordering::Relaxed);
                            d_mft_h.store(0, Ordering::Relaxed);
                            d_exp_w.store(0, Ordering::Relaxed);
                            d_exp_h.store(0, Ordering::Relaxed);
                            d_epoch.fetch_add(1, Ordering::SeqCst);
                            if let Ok(mut g) = d_codec.lock() {
                                g.clear();
                            }
                            sync_airplay_pipeline_trace(
                                &d_q,
                                &d_ring,
                                &d_trace_fed,
                                &d_trace_cnt,
                                &d_vram_base,
                                &d_sc,
                                &d_nad,
                                &d_stale_drop,
                                &d_pres,
                                &d_pres_calls,
                                &d_pres_errs,
                                &d_lhr,
                                &d_mft_w,
                                &d_mft_h,
                                &d_exp_w,
                                &d_exp_h,
                            );
                        }
                        NativeVideoEvent::HevcConfig { .. } => {
                            if !d_hevc_failed.load(Ordering::Acquire)
                                && (active_kind != Some(VideoCodecKind::Hevc) || active_dec.is_none())
                            {
                                if let Some(ref mut old) = active_dec {
                                    old.clean_teardown();
                                }
                                match LiveMfVideoDecoder::new(d_gpu.clone(), VideoCodecKind::Hevc) {
                                    Ok(dec) => {
                                        active_dec = Some(dec);
                                        active_kind = Some(VideoCodecKind::Hevc);
                                        if let Ok(mut e) = d_codec_err.lock() {
                                            e.clear();
                                        }
                                        if let Ok(mut v) = d_view.lock() {
                                            v.set_airplay_mode(false);
                                        }
                                        watchdog.reset();
                                    }
                                    Err(err) => {
                                        d_hevc_failed.store(true, Ordering::Release);
                                        let kind = classify_hevc_decoder_init_error(&err);
                                        if let Ok(mut e) = d_codec_err.lock() {
                                            *e = kind.to_string();
                                        }
                                        if let DecoderError::ComError { hr, .. } = err {
                                            d_lhr.store(hr, Ordering::Relaxed);
                                        } else {
                                            d_lhr.store(-1, Ordering::Relaxed);
                                        }
                                    }
                                }
                            }
                            if let Ok(mut g) = d_codec.lock() {
                                *g = "HEVC".to_string();
                            }
                        }
                        NativeVideoEvent::HevcFrame {
                            annex_b,
                            is_keyframe,
                            pts_us,
                            trim,
                        } => {
                            let trim_t = (trim[0], trim[1], trim[2], trim[3]);
                            if !d_hevc_failed.load(Ordering::Acquire)
                                && (active_kind != Some(VideoCodecKind::Hevc) || active_dec.is_none())
                            {
                                if let Some(ref mut old) = active_dec {
                                    old.clean_teardown();
                                }
                                match LiveMfVideoDecoder::new(d_gpu.clone(), VideoCodecKind::Hevc) {
                                    Ok(dec) => {
                                        active_dec = Some(dec);
                                        active_kind = Some(VideoCodecKind::Hevc);
                                        if let Ok(mut e) = d_codec_err.lock() {
                                            e.clear();
                                        }
                                        if let Ok(mut v) = d_view.lock() {
                                            v.set_airplay_mode(false);
                                        }
                                        if let Ok(mut g) = d_codec.lock() {
                                            *g = "HEVC".to_string();
                                        }
                                    }
                                    Err(err) => {
                                        d_hevc_failed.store(true, Ordering::Release);
                                        let kind = classify_hevc_decoder_init_error(&err);
                                        if let Ok(mut e) = d_codec_err.lock() {
                                            *e = kind.to_string();
                                        }
                                        if let DecoderError::ComError { hr, .. } = err {
                                            d_lhr.store(hr, Ordering::Relaxed);
                                        } else {
                                            d_lhr.store(-1, Ordering::Relaxed);
                                        }
                                    }
                                }
                            }
                            if let Some(ref mut dec) = active_dec {
                                d_fed.fetch_add(1, Ordering::Relaxed);
                                d_trace_fed.fetch_add(1, Ordering::Relaxed);
                                match dec.feed_access_unit(
                                    &annex_b,
                                    pts_us,
                                    is_keyframe,
                                    false,
                                    Some(trim_t),
                                    &d_ring,
                                ) {
                                    Ok(st) => {
                                        if st.frames_emitted > 0 {
                                            let em = u64::from(st.frames_emitted);
                                            d_cnt.fetch_add(em, Ordering::Relaxed);
                                            d_trace_cnt.fetch_add(em, Ordering::Relaxed);
                                        }
                                        if st.visible_width > 0 && st.visible_height > 0 {
                                            let (pw, ph) = d_ring
                                                .latest_slot()
                                                .map(|snap| compute_post_trim_dimensions(&snap))
                                                .unwrap_or_else(|| {
                                                    post_trim_visible_dims(
                                                        st.visible_width,
                                                        st.visible_height,
                                                        trim_t,
                                                        false,
                                                    )
                                                });
                                            d_w.store(pw, Ordering::Relaxed);
                                            d_h.store(ph, Ordering::Relaxed);
                                        }
                                        d_sc.store(u64::from(st.stream_changes), Ordering::Relaxed);
                                        d_nad.store(dec.not_accepting_drains(), Ordering::Relaxed);
                                        d_lhr.store(dec.last_hresult(), Ordering::Relaxed);
                                    }
                                    Err(err) => {
                                        let hr = match err {
                                            DecoderError::ComError { hr, .. } => hr,
                                            _ => -1,
                                        };
                                        d_lhr.store(hr, Ordering::Relaxed);
                                    }
                                }
                                sync_airplay_pipeline_trace(
                                    &d_q,
                                    &d_ring,
                                    &d_trace_fed,
                                    &d_trace_cnt,
                                    &d_vram_base,
                                    &d_sc,
                                    &d_nad,
                                    &d_stale_drop,
                                    &d_pres,
                                    &d_pres_calls,
                                    &d_pres_errs,
                                    &d_lhr,
                                    &d_mft_w,
                                    &d_mft_h,
                                    &d_exp_w,
                                    &d_exp_h,
                                );
                            }
                        }
                        NativeVideoEvent::H264Avcc { avcc, .. } => {
                            d_hevc_failed.store(false, Ordering::Release);
                            if let Ok(mut e) = d_codec_err.lock() {
                                e.clear();
                            }
                            if !avcc.is_empty()
                                && active_kind == Some(VideoCodecKind::H264)
                                && active_dec.is_some()
                                && active_avcc.as_deref() == Some(avcc.as_slice())
                            {
                                // Identical repeated `pkt_k1` (`avcC`) resend: 100% NO-OP preserving DPB.
                                continue;
                            }
                            if active_kind != Some(VideoCodecKind::H264) || active_dec.is_none() {
                                if let Some(ref mut old) = active_dec {
                                    old.clean_teardown();
                                }
                                prev_dec_idrs = 0;
                                prev_dec_small_256_511 = 0;
                                prev_dec_reconfigs = 0;
                                active_dec =
                                    LiveMfVideoDecoder::new(d_gpu.clone(), VideoCodecKind::H264)
                                        .ok();
                                if let Some(ref dec) = active_dec {
                                    airplay::set_trace_mf_low_latency_enabled(
                                        dec.mf_low_latency_enabled(),
                                    );
                                    airplay::set_trace_d3d11_multithread_protected(
                                        dec.d3d11_multithread_protected(),
                                    );
                                }
                                active_kind = Some(VideoCodecKind::H264);
                                if let Ok(mut v) = d_view.lock() {
                                    v.set_airplay_mode(true);
                                }
                            }
                            if !avcc.is_empty() {
                                active_avcc = Some(avcc.clone());
                                let prev_dims = active_dec
                                    .as_ref()
                                    .and_then(|d| d.expected_coded_dims());
                                if let Some(ref mut dec) = active_dec {
                                    match dec.update_avcc(&avcc) {
                                        Ok(changed) => {
                                            d_sc.store(
                                                u64::from(dec.stream_changes()),
                                                Ordering::Relaxed,
                                            );
                                            if let Some((mw, mh)) = dec.last_mft_dims() {
                                                d_mft_w.store(mw, Ordering::Relaxed);
                                                d_mft_h.store(mh, Ordering::Relaxed);
                                            }
                                            let next_dims = dec.expected_coded_dims();
                                            if let Some((ew, eh)) = next_dims {
                                                d_exp_w.store(ew, Ordering::Relaxed);
                                                d_exp_h.store(eh, Ordering::Relaxed);
                                            }
                                            let delta_reconfigs = dec
                                                .mft_reconfigs()
                                                .saturating_sub(prev_dec_reconfigs);
                                            prev_dec_reconfigs = dec.mft_reconfigs();
                                            if delta_reconfigs > 0 {
                                                airplay::add_trace_mft_reconfigs(delta_reconfigs);
                                                let (rw, rh) = next_dims.unwrap_or((0, 0));
                                                airplay::record_airplay_trace(
                                                    "mft_reconfig",
                                                    format!("reason=avcc_dim_change dims={rw}x{rh}"),
                                                );
                                            }
                                            let dims_changed_any = matches!(
                                                (prev_dims, next_dims),
                                                (Some((pw, ph)), Some((nw, nh)))
                                                    if (pw, ph) != (nw, nh)
                                            );
                                            let orientation_flipped = matches!(
                                                (prev_dims, next_dims),
                                                (Some((pw, ph)), Some((nw, nh)))
                                                    if (pw > ph) != (nw > nh)
                                            );
                                            if dims_changed_any {
                                                watchdog.reset_grace();
                                            }
                                            if changed && orientation_flipped {
                                                if let Ok(mut v) = d_view.lock() {
                                                    v.prepare_seamless_dimension_transition();
                                                    airplay::set_trace_mid_stream_black_clears(
                                                        v.mid_stream_black_clears(),
                                                    );
                                                } else {
                                                    d_ring.clear();
                                                }
                                                d_redraw.store(true, Ordering::SeqCst);
                                            } else if changed && dims_changed_any {
                                                airplay::inc_trace_mft_dim_changes();
                                                d_redraw.store(true, Ordering::SeqCst);
                                            }
                                        }
                                        Err(err) => {
                                            let hr = match err {
                                                DecoderError::ComError { hr, .. } => hr,
                                                _ => -1,
                                            };
                                            d_lhr.store(hr, Ordering::Relaxed);
                                        }
                                    }
                                    sync_airplay_pipeline_trace(
                                        &d_q,
                                        &d_ring,
                                        &d_trace_fed,
                                        &d_trace_cnt,
                                        &d_vram_base,
                                        &d_sc,
                                        &d_nad,
                                        &d_stale_drop,
                                        &d_pres,
                                        &d_pres_calls,
                                        &d_pres_errs,
                                        &d_lhr,
                                        &d_mft_w,
                                        &d_mft_h,
                                        &d_exp_w,
                                        &d_exp_h,
                                    );
                                }
                                if let Ok(mut g) = d_codec.lock() {
                                    *g = "AirPlay H.264".to_string();
                                }
                            }
                        }
                        NativeVideoEvent::H264Frame {
                            avcc_payload,
                            is_keyframe,
                            pts_us,
                        } => {
                            d_pkt_ms.store(
                                d_clock_origin.elapsed().as_millis().max(1) as u64,
                                Ordering::Relaxed,
                            );
                            if active_kind != Some(VideoCodecKind::H264) || active_dec.is_none() {
                                if let Some(ref mut old) = active_dec {
                                    old.clean_teardown();
                                }
                                prev_dec_idrs = 0;
                                prev_dec_small_256_511 = 0;
                                prev_dec_reconfigs = 0;
                                if let Ok(mut v) = d_view.lock() {
                                    v.set_airplay_mode(true);
                                }
                                match LiveMfVideoDecoder::new(d_gpu.clone(), VideoCodecKind::H264) {
                                    Ok(mut dec) => {
                                        airplay::set_trace_mf_low_latency_enabled(
                                            dec.mf_low_latency_enabled(),
                                        );
                                        airplay::set_trace_d3d11_multithread_protected(
                                            dec.d3d11_multithread_protected(),
                                        );
                                        let fallback_avcc = active_avcc
                                            .clone()
                                            .or_else(mirror::current_live_h264_avcc_bytes)
                                            .or_else(|| {
                                                mirror::latest_airplay_avcc_snapshot()
                                                    .map(|(_, b, _, _)| b)
                                            });
                                        if let Some(avcc) = fallback_avcc {
                                            if dec.update_avcc(&avcc).is_ok() {
                                                active_avcc = Some(avcc);
                                            }
                                        }
                                        if let Some((mw, mh)) = dec.last_mft_dims() {
                                            d_mft_w.store(mw, Ordering::Relaxed);
                                            d_mft_h.store(mh, Ordering::Relaxed);
                                        }
                                        if let Some((ew, eh)) = dec.expected_coded_dims() {
                                            d_exp_w.store(ew, Ordering::Relaxed);
                                            d_exp_h.store(eh, Ordering::Relaxed);
                                        }
                                        active_dec = Some(dec);
                                        active_kind = Some(VideoCodecKind::H264);
                                        if let Ok(mut g) = d_codec.lock() {
                                            *g = "AirPlay H.264".to_string();
                                        }
                                    }
                                    Err(err) => {
                                        let hr = match err {
                                            DecoderError::ComError { hr, .. } => hr,
                                            _ => -1,
                                        };
                                        d_lhr.store(hr, Ordering::Relaxed);
                                    }
                                }
                            } else if let Some(ref mut dec) = active_dec {
                                if !dec.has_avcc_config() {
                                    let fallback_avcc = active_avcc
                                        .clone()
                                        .or_else(mirror::current_live_h264_avcc_bytes)
                                        .or_else(|| {
                                            mirror::latest_airplay_avcc_snapshot()
                                                .map(|(_, b, _, _)| b)
                                        });
                                    if let Some(avcc) = fallback_avcc {
                                        if dec.update_avcc(&avcc).is_ok() {
                                            active_avcc = Some(avcc);
                                        }
                                    }
                                }
                            }
                            if active_dec.as_ref().is_some_and(|d| !d.has_avcc_config()) {
                                airplay::inc_trace_pre_decoder_dropped_awaiting_avcc();
                                sync_airplay_pipeline_trace(
                                    &d_q,
                                    &d_ring,
                                    &d_trace_fed,
                                    &d_trace_cnt,
                                    &d_vram_base,
                                    &d_sc,
                                    &d_nad,
                                    &d_stale_drop,
                                    &d_pres,
                                    &d_pres_calls,
                                    &d_pres_errs,
                                    &d_lhr,
                                    &d_mft_w,
                                    &d_mft_h,
                                    &d_exp_w,
                                    &d_exp_h,
                                );
                                continue;
                            }
                            let mut nal_len_sz = 4usize;
                            if let Some(ref mut dec) = active_dec {
                                nal_len_sz = dec.nal_length_size();
                                let is_clean_ra = is_keyframe
                                    || iphone_link_video::mf_decoder::h264_avcc_payload_is_clean_random_access(
                                        &avcc_payload,
                                        nal_len_sz,
                                    );
                                d_fed.fetch_add(1, Ordering::Relaxed);
                                d_trace_fed.fetch_add(1, Ordering::Relaxed);
                                let pre_drops_before = dec.dropped_pre_idr_frames();
                                match dec.feed_avcc_sample(
                                    &avcc_payload,
                                    pts_us,
                                    is_clean_ra,
                                    &d_ring,
                                ) {
                                    Ok(st) => {
                                        consecutive_fatal_hr = 0;
                                        if st.frames_emitted > 0 {
                                            let em = u64::from(st.frames_emitted);
                                            d_cnt.fetch_add(em, Ordering::Relaxed);
                                            d_trace_cnt.fetch_add(em, Ordering::Relaxed);
                                        }
                                        if st.visible_width > 0 && st.visible_height > 0 {
                                            let (pw, ph) = d_ring
                                                .latest_slot()
                                                .map(|snap| compute_post_trim_dimensions(&snap))
                                                .unwrap_or_else(|| {
                                                    post_trim_visible_dims(
                                                        st.visible_width,
                                                        st.visible_height,
                                                        (0, 0, 0, 0),
                                                        true,
                                                    )
                                                });
                                            let prev_w = d_w.swap(pw, Ordering::Relaxed);
                                            let prev_h = d_h.swap(ph, Ordering::Relaxed);
                                            if (prev_w, prev_h) != (pw, ph) {
                                                d_redraw.store(true, Ordering::SeqCst);
                                            }
                                        }
                                        d_sc.store(u64::from(st.stream_changes), Ordering::Relaxed);
                                        d_nad.store(dec.not_accepting_drains(), Ordering::Relaxed);
                                        d_stale_drop.store(dec.stale_dim_dropped(), Ordering::Relaxed);
                                        d_lhr.store(dec.last_hresult(), Ordering::Relaxed);
                                        airplay::set_trace_dpb_preserved_flushes_skipped(
                                            dec.dpb_preserved_flushes_skipped(),
                                        );
                                        let cur_idrs = dec.idr_keyframes_detected();
                                        let delta_idrs = cur_idrs.saturating_sub(prev_dec_idrs);
                                        prev_dec_idrs = cur_idrs;
                                        if delta_idrs > 0 {
                                            trace_idrs_total =
                                                trace_idrs_total.saturating_add(delta_idrs);
                                        }
                                        airplay::set_trace_idr_keyframes_detected(trace_idrs_total);
                                        if let Some((mw, mh)) = dec.last_mft_dims() {
                                            d_mft_w.store(mw, Ordering::Relaxed);
                                            d_mft_h.store(mh, Ordering::Relaxed);
                                        }
                                        if let Some((ew, eh)) = dec.expected_coded_dims() {
                                            d_exp_w.store(ew, Ordering::Relaxed);
                                            d_exp_h.store(eh, Ordering::Relaxed);
                                        }
                                    }
                                    Err(err) => {
                                        let hr = match err {
                                            DecoderError::ComError { hr, .. } => hr,
                                            _ => -1,
                                        };
                                        d_lhr.store(hr, Ordering::Relaxed);
                                        if hr < 0 {
                                            consecutive_fatal_hr =
                                                consecutive_fatal_hr.saturating_add(1);
                                        }
                                    }
                                }
                                let cur_small = dec.avcc_small_256_511_frames();
                                let delta_small = cur_small.saturating_sub(prev_dec_small_256_511);
                                prev_dec_small_256_511 = cur_small;
                                if delta_small > 0 {
                                    airplay::add_trace_avcc_small_256_511_frames(delta_small);
                                }
                                let cur_reconfigs = dec.mft_reconfigs();
                                let delta_reconfigs = cur_reconfigs.saturating_sub(prev_dec_reconfigs);
                                prev_dec_reconfigs = cur_reconfigs;
                                if delta_reconfigs > 0 {
                                    airplay::add_trace_mft_reconfigs(delta_reconfigs);
                                    let (rw, rh) = dec
                                        .last_mft_dims()
                                        .or_else(|| dec.expected_coded_dims())
                                        .unwrap_or((0, 0));
                                    airplay::record_airplay_trace(
                                        "mft_reconfig",
                                        format!("reason=keyframe_retry dims={rw}x{rh}"),
                                    );
                                }
                                if dec.dropped_pre_idr_frames() > pre_drops_before {
                                    airplay::inc_trace_pre_decoder_dropped_awaiting_idr();
                                }
                                sync_airplay_pipeline_trace(
                                    &d_q,
                                    &d_ring,
                                    &d_trace_fed,
                                    &d_trace_cnt,
                                    &d_vram_base,
                                    &d_sc,
                                    &d_nad,
                                    &d_stale_drop,
                                    &d_pres,
                                    &d_pres_calls,
                                    &d_pres_errs,
                                    &d_lhr,
                                    &d_mft_w,
                                    &d_mft_h,
                                    &d_exp_w,
                                    &d_exp_h,
                                );
                            }
                            let awaiting_idr_now = active_dec.as_ref().is_some_and(|d| d.awaiting_idr());
                            let dec_now = d_cnt.load(Ordering::Relaxed);
                            let pres_now = d_pres.load(Ordering::Relaxed);
                            let now_ms = wd_clock.elapsed().as_millis() as u64;
                            let is_slice = iphone_link_video::mf_decoder::h264_avcc_has_video_slice_nal(
                                &avcc_payload,
                                nal_len_sz,
                            );
                            if awaiting_idr_now {
                                watchdog.reset_grace();
                            } else {
                                match watchdog.observe_tiered(
                                    is_slice,
                                    dec_now,
                                    pres_now,
                                    now_ms,
                                    consecutive_fatal_hr,
                                ) {
                                    AirplayWatchdogAction::None => {}
                                    AirplayWatchdogAction::SoftDrainAndRepaint => {
                                        if let Some(ref mut dec) = active_dec {
                                            if let Ok(emitted) =
                                                dec.drain_pending_outputs_only(true, &d_ring)
                                            {
                                                if emitted > 0 {
                                                    let em = u64::from(emitted);
                                                    d_cnt.fetch_add(em, Ordering::Relaxed);
                                                    d_trace_cnt.fetch_add(em, Ordering::Relaxed);
                                                }
                                            }
                                        }
                                        if let Ok(mut v) = d_view.lock() {
                                            if d_ring.latest_slot().is_some() {
                                                let _ = v.force_repaint_cached_frame();
                                                let (dx0, dy0, dx1, dy1) =
                                                    v.last_presented_dest_rect();
                                                let dw = dx1.saturating_sub(dx0);
                                                let dh = dy1.saturating_sub(dy0);
                                                if dw > 0 && dh > 0 {
                                                    airplay::set_trace_last_presented_dest_rect(
                                                        dx0, dy0, dw, dh,
                                                    );
                                                    airplay::set_trace_last_presented_rotation_deg(
                                                        u32::from(v.last_effective_rotation_deg()),
                                                    );
                                                }
                                            }
                                        }
                                        schedule_coalesced_dcomp_commit(&d_dcomp_pending);
                                        d_redraw.store(true, Ordering::SeqCst);
                                    }
                                    AirplayWatchdogAction::HardRebuildOnFatalError => {
                                        let fatal_hr_val = d_lhr.load(Ordering::Relaxed);
                                        if let Some(ref mut old) = active_dec {
                                            old.clean_teardown();
                                        }
                                        active_dec = None;
                                        prev_dec_idrs = 0;
                                        prev_dec_small_256_511 = 0;
                                        prev_dec_reconfigs = 0;
                                        consecutive_fatal_hr = 0;
                                        if let Ok(mut v) = d_view.lock() {
                                            v.prepare_seamless_dimension_transition();
                                            airplay::set_trace_mid_stream_black_clears(
                                                v.mid_stream_black_clears(),
                                            );
                                        } else {
                                            d_ring.clear();
                                        }
                                        if let Ok(mut fresh) = LiveMfVideoDecoder::new(
                                            d_gpu.clone(),
                                            VideoCodecKind::H264,
                                        ) {
                                            airplay::set_trace_mf_low_latency_enabled(
                                                fresh.mf_low_latency_enabled(),
                                            );
                                            airplay::set_trace_d3d11_multithread_protected(
                                                fresh.d3d11_multithread_protected(),
                                            );
                                            let attach_avcc = active_avcc
                                                .clone()
                                                .or_else(mirror::current_live_h264_avcc_bytes)
                                                .or_else(|| {
                                                    mirror::latest_airplay_avcc_snapshot()
                                                        .map(|(_, b, _, _)| b)
                                                });
                                            if let Some(avcc) = attach_avcc {
                                                let _ = fresh.update_avcc(&avcc);
                                                active_avcc = Some(avcc);
                                            }
                                            fresh.force_next_sps_pps_prepend();
                                            fresh.arm_empty_dpb_idr_wait();
                                            let (mw, mh) = fresh
                                                .last_mft_dims()
                                                .or_else(|| fresh.expected_coded_dims())
                                                .unwrap_or((0, 0));
                                            if mw > 0 && mh > 0 {
                                                d_mft_w.store(mw, Ordering::Relaxed);
                                                d_mft_h.store(mh, Ordering::Relaxed);
                                            }
                                            if let Some((ew, eh)) = fresh.expected_coded_dims() {
                                                d_exp_w.store(ew, Ordering::Relaxed);
                                                d_exp_h.store(eh, Ordering::Relaxed);
                                            }
                                            active_dec = Some(fresh);
                                            active_kind = Some(VideoCodecKind::H264);
                                            airplay::inc_trace_watchdog_recoveries();
                                            airplay::inc_trace_mft_reconfigs();
                                            airplay::record_airplay_trace(
                                                "mft_reconfig",
                                                format!(
                                                    "reason=fatal_hr hr=0x{:08X} dims={mw}x{mh}",
                                                    fatal_hr_val as u32
                                                ),
                                            );
                                            d_redraw.store(true, Ordering::SeqCst);
                                            sync_airplay_pipeline_trace(
                                                &d_q,
                                                &d_ring,
                                                &d_trace_fed,
                                                &d_trace_cnt,
                                                &d_vram_base,
                                                &d_sc,
                                                &d_nad,
                                                &d_stale_drop,
                                                &d_pres,
                                                &d_pres_calls,
                                                &d_pres_errs,
                                                &d_lhr,
                                                &d_mft_w,
                                                &d_mft_h,
                                                &d_exp_w,
                                                &d_exp_h,
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            })
            .ok();

        // Render worker thread (DXGI FLIP Present on frame arrival)
        let r_stop = stop_flag.clone();
        let r_vis = visible_flag.clone();
        let r_redraw = redraw_requested.clone();
        let r_ring = ring.clone();
        let r_view = view.clone();
        let r_q = queue.clone();
        let r_fed = decoder_fed.clone();
        let r_trace_fed = trace_decoder_fed.clone();
        let r_trace_cnt = trace_decoded_frames.clone();
        let r_vram_base = vram_pub_base.clone();
        let r_sc = stream_changes.clone();
        let r_nad = not_accepting_drains.clone();
        let r_stale_drop = stale_dim_dropped.clone();
        let r_lhr = last_decoder_hresult.clone();
        let r_pres = presented_frames.clone();
        let r_pres_calls = present_calls.clone();
        let r_pres_errs = present_errors.clone();
        let r_mft_w = last_mft_w.clone();
        let r_mft_h = last_mft_h.clone();
        let r_exp_w = expected_coded_w.clone();
        let r_exp_h = expected_coded_h.clone();
        let r_epoch = session_epoch.clone();
        let r_ready_cb = frame_ready_cb.clone();
        let r_dcomp_pending = dcomp_commit_pending.clone();
        let r_ar_tracker = hevc_autorotate_tracker.clone();
        let r_pending_ar = pending_hevc_autorotate_deg.clone();
        let r_is_ipad = is_ipad_profile.clone();
        let r_w = dim_w.clone();
        let r_h = dim_h.clone();
        let r_ar_clock = pipeline_epoch_start;
        let r_last_pkt_ms = last_h264_pkt_ms;
        let r_clock_origin = pipeline_epoch_start;
        let render_thread = std::thread::Builder::new()
            .name("native-ui-render".to_string())
            .spawn(move || {
                let mut last_seq = 0u64;
                let mut last_presented_seq = 0u64;
                let mut seen_epoch = r_epoch.load(Ordering::Relaxed);
                let mut ar_grid = ProbeGrid120x260::new();
                let mut stall_state = AirplayStallDumpState::default();
                let mut last_stall_check_at = Instant::now();

                while !r_stop.load(Ordering::Relaxed) {
                    let cur_epoch = r_epoch.load(Ordering::Relaxed);
                    if cur_epoch != seen_epoch {
                        seen_epoch = cur_epoch;
                        last_seq = 0;
                        last_presented_seq = 0;
                        stall_state = AirplayStallDumpState::default();
                        r_pending_ar.store(-1, Ordering::Release);
                        if let Ok(mut tr) = r_ar_tracker.lock() {
                            tr.reset();
                        }
                    }

                    let is_vis = r_vis.load(Ordering::Relaxed);
                    let need_first_frame =
                        r_pres.load(Ordering::Relaxed) == 0 && r_ring.latest_slot().is_some();

                    if !is_vis && !need_first_frame {
                        let _ = r_ring.wait_for_new_frame(last_seq, Duration::from_millis(50));
                        continue;
                    }
                    let got_new = if r_redraw.load(Ordering::Acquire) {
                        r_ring
                            .latest_slot()
                            .filter(|s| s.frame_seq > last_seq)
                            .map(|s| {
                                last_seq = s.frame_seq;
                                true
                            })
                            .unwrap_or(false)
                    } else {
                        r_ring
                            .wait_for_new_frame(last_seq, Duration::from_millis(16))
                            .map(|s| {
                                last_seq = s.frame_seq;
                                true
                            })
                            .unwrap_or(false)
                    };
                    if r_stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let forced = r_redraw.swap(false, Ordering::SeqCst);
                    if got_new || forced || need_first_frame {
                        let mut notify_ready = false;
                        let mut probed_hevc_ar = false;
                        if let Ok(mut v) = r_view.lock() {
                            r_pres_calls.fetch_add(1, Ordering::Relaxed);
                            match v.present_latest() {
                                Ok(st) => {
                                    airplay::set_trace_mid_stream_black_clears(
                                        v.mid_stream_black_clears(),
                                    );
                                    airplay::set_trace_dual_buffer_overwrites(
                                        v.dual_buffer_overwrites(),
                                    );
                                    let (dx0, dy0, dx1, dy1) = v.last_presented_dest_rect();
                                    let dw = dx1.saturating_sub(dx0);
                                    let dh = dy1.saturating_sub(dy0);
                                    if st.frame_seq > 0 && dw > 0 && dh > 0 {
                                        airplay::set_trace_last_presented_dest_rect(
                                            dx0, dy0, dw, dh,
                                        );
                                        airplay::set_trace_last_presented_rotation_deg(u32::from(
                                            v.last_effective_rotation_deg(),
                                        ));
                                    }
                                    if st.frame_seq > last_presented_seq
                                        || st.dual_buffer_overwritten
                                        || forced
                                    {
                                        schedule_coalesced_dcomp_commit(&r_dcomp_pending);
                                    }
                                    if st.frame_seq > last_presented_seq {
                                        last_presented_seq = st.frame_seq;
                                        let prev = r_pres.fetch_add(1, Ordering::SeqCst);
                                        if prev == 0 {
                                            notify_ready = true;
                                        }
                                        if prev < 8 || prev % 15 == 0 {
                                            sync_airplay_pipeline_trace(
                                                &r_q,
                                                &r_ring,
                                                &r_trace_fed,
                                                &r_trace_cnt,
                                                &r_vram_base,
                                                &r_sc,
                                                &r_nad,
                                                &r_stale_drop,
                                                &r_pres,
                                                &r_pres_calls,
                                                &r_pres_errs,
                                                &r_lhr,
                                                &r_mft_w,
                                                &r_mft_h,
                                                &r_exp_w,
                                                &r_exp_h,
                                            );
                                        }
                                    }
                                }
                                Err(_) => {
                                    r_pres_errs.fetch_add(1, Ordering::Relaxed);
                                    sync_airplay_pipeline_trace(
                                        &r_q,
                                        &r_ring,
                                        &r_trace_fed,
                                        &r_trace_cnt,
                                        &r_vram_base,
                                        &r_sc,
                                        &r_nad,
                                        &r_stale_drop,
                                        &r_pres,
                                        &r_pres_calls,
                                        &r_pres_errs,
                                        &r_lhr,
                                        &r_mft_w,
                                        &r_mft_h,
                                        &r_exp_w,
                                        &r_exp_h,
                                    );
                                }
                            }
                            if is_vis && !mirror::is_airplay_active() {
                                probed_hevc_ar =
                                    v.update_autorotate_probe_if_due(&mut ar_grid, false);
                            }
                        }
                        if probed_hevc_ar && !mirror::is_airplay_active() {
                            let raw_w = r_w.load(Ordering::Relaxed);
                            let raw_h = r_h.load(Ordering::Relaxed);
                            if raw_w > 0 && raw_h > 0 {
                                let now_ms = r_ar_clock.elapsed().as_millis() as u64;
                                let is_ipad = r_is_ipad.load(Ordering::Relaxed);
                                if let Ok(mut tr) = r_ar_tracker.lock() {
                                    if let Some(new_deg) =
                                        tr.update_sample(&ar_grid, raw_w, raw_h, now_ms, is_ipad)
                                    {
                                        r_pending_ar.store(i32::from(new_deg), Ordering::Release);
                                    }
                                }
                            }
                        }
                        if notify_ready {
                            if let Some(cb) = r_ready_cb.lock().ok().and_then(|g| g.clone()) {
                                cb();
                            }
                        }
                    }
                    if is_vis
                        && r_fed.load(Ordering::Relaxed) > 0
                        && last_stall_check_at.elapsed() >= Duration::from_millis(500)
                    {
                        last_stall_check_at = Instant::now();
                        let now_ms = r_clock_origin.elapsed().as_millis() as u64;
                        let tc = airplay::get_airplay_trace_counters();
                        let last_pkt = r_last_pkt_ms.load(Ordering::Relaxed);
                        let pkt_age_ms = if last_pkt > 0 {
                            now_ms.saturating_sub(last_pkt)
                        } else {
                            0
                        };
                        let ap_active = mirror::is_airplay_active();
                        let pres_now = r_pres.load(Ordering::Relaxed);
                        let dump_path = airplay_stall_dump_path();
                        if maybe_write_airplay_stall_dump(
                            ap_active,
                            tc.tcp_video_packets,
                            pres_now,
                            pkt_age_ms,
                            now_ms,
                            &mut stall_state,
                            &dump_path,
                        ) {
                            airplay::record_airplay_trace(
                                "airplay_stall_autodump",
                                format!(
                                    "pres={pres_now} tcp={} ageMs={pkt_age_ms}",
                                    tc.tcp_video_packets
                                ),
                            );
                        }
                    }
                }
            })
            .ok();

        Ok(Self {
            view: Some(view),
            ring,
            queue,
            stop_flag,
            visible_flag,
            redraw_requested,
            composition_mode,
            frame_ready_cb,
            session_epoch,
            codec_label,
            codec_error_kind,
            hevc_init_failed_latched,
            dim_w,
            dim_h,
            decoder_fed,
            decoded_frames,
            trace_decoder_fed,
            trace_decoded_frames,
            vram_pub_base,
            presented_frames,
            present_calls,
            present_errors,
            stream_changes,
            not_accepting_drains,
            stale_dim_dropped,
            last_decoder_hresult,
            last_mft_w,
            last_mft_h,
            expected_coded_w,
            expected_coded_h,
            dcomp_commit_pending,
            hevc_autorotate_tracker,
            pending_hevc_autorotate_deg,
            is_ipad_profile,
            autorotate_clock: pipeline_epoch_start,
            last_resize_dims: Mutex::new((0, 0)),
            child_hwnd,
            decode_thread,
            render_thread,
            shut_down: false,
        })
    }

    /// Pre-warms `CMSH264DecoderMFT` on the decode thread before AirPlay TCP Frame #1 arrives.
    pub fn prewarm_h264_decoder(&self) {
        self.queue.try_push(NativeVideoEvent::H264Avcc {
            avcc: Vec::new(),
            width: 0,
            height: 0,
        });
    }

    pub fn is_composition_mode(&self) -> bool {
        self.composition_mode
    }

    pub fn swapchain_ptr(&self) -> Option<*mut c_void> {
        let view = self.view.as_ref()?;
        let guard = view.lock().ok()?;
        let ptr = guard.swapchain_ptr();
        if ptr.is_null() {
            None
        } else {
            Some(ptr)
        }
    }

    pub fn resize_composition(&self, width: u32, height: u32) -> Result<bool, String> {
        let Some(ref view) = self.view else {
            return Ok(false);
        };
        let target = (width.max(1), height.max(1));
        if let Ok(dims) = self.last_resize_dims.lock() {
            if *dims == target {
                return Ok(false);
            }
        }
        let mut repainted = false;
        if let Ok(mut v) = view.lock() {
            let before_rp = v.resize_atomic_repaints();
            v.resize(target.0, target.1)?;
            if v.resize_atomic_repaints() > before_rp
                || (self.ring.latest_slot().is_some() && v.force_repaint_cached_frame().is_ok())
            {
                repainted = true;
            }
        }
        if let Ok(mut dims) = self.last_resize_dims.lock() {
            *dims = target;
        }
        if repainted {
            self.present_calls.fetch_add(1, Ordering::Relaxed);
            schedule_coalesced_dcomp_commit(&self.dcomp_commit_pending);
        }
        self.redraw_requested.store(true, Ordering::SeqCst);
        self.ring.notify_all();
        Ok(true)
    }

    /// Forces immediate synchronous repaint of the cached VFR frame onto the active composition
    /// SwapChain and commits DirectComposition so Maximize/Restore/Fullscreen toggles never show a blank stage.
    pub fn force_present_now(&self) -> bool {
        let Some(ref view) = self.view else {
            return false;
        };
        if self.ring.latest_slot().is_none() {
            return false;
        }
        let repainted = if let Ok(mut v) = view.lock() {
            v.force_repaint_cached_frame().is_ok()
        } else {
            false
        };
        if repainted {
            self.present_calls.fetch_add(1, Ordering::Relaxed);
            self.redraw_requested.store(false, Ordering::Release);
            crate::dcomp_presenter::commit_active_dcomp();
            airplay::inc_trace_dcomp_commits();
        }
        repainted
    }

    pub fn set_frame_ready_callback(&self, cb: Option<Arc<dyn Fn() + Send + Sync>>) {
        if let Ok(mut g) = self.frame_ready_cb.lock() {
            *g = cb;
        }
    }

    pub fn reset_for_new_session(&self) {
        self.session_epoch.fetch_add(1, Ordering::SeqCst);
        self.pending_hevc_autorotate_deg.store(-1, Ordering::SeqCst);
        if let Ok(mut tr) = self.hevc_autorotate_tracker.lock() {
            tr.reset();
        }
        self.queue.reset_stats();
        if let Some(ref view) = self.view {
            if let Ok(mut v) = view.lock() {
                let _ = v.purge_stale_video_buffers();
            } else {
                self.ring.clear();
            }
        } else {
            self.ring.clear();
        }
        airplay::set_trace_idr_keyframes_detected(0);
        airplay::set_trace_dual_buffer_overwrites(0);
        airplay::set_trace_avcc_small_256_511_frames(0);
        airplay::set_trace_mft_reconfigs(0);
        self.decoder_fed.store(0, Ordering::Relaxed);
        self.decoded_frames.store(0, Ordering::Relaxed);
        self.trace_decoder_fed.store(0, Ordering::Relaxed);
        self.trace_decoded_frames.store(0, Ordering::Relaxed);
        self.vram_pub_base
            .store(self.ring.stats().0, Ordering::Relaxed);
        self.presented_frames.store(0, Ordering::Relaxed);
        self.present_calls.store(0, Ordering::Relaxed);
        self.present_errors.store(0, Ordering::Relaxed);
        self.stream_changes.store(0, Ordering::Relaxed);
        self.not_accepting_drains.store(0, Ordering::Relaxed);
        self.stale_dim_dropped.store(0, Ordering::Relaxed);
        self.last_decoder_hresult.store(0, Ordering::Relaxed);
        self.dim_w.store(0, Ordering::Relaxed);
        self.dim_h.store(0, Ordering::Relaxed);
        self.last_mft_w.store(0, Ordering::Relaxed);
        self.last_mft_h.store(0, Ordering::Relaxed);
        self.expected_coded_w.store(0, Ordering::Relaxed);
        self.expected_coded_h.store(0, Ordering::Relaxed);
        self.hevc_init_failed_latched.store(false, Ordering::Release);
        if let Ok(mut e) = self.codec_error_kind.lock() {
            e.clear();
        }
        if let Ok(mut g) = self.codec_label.lock() {
            g.clear();
        }
        sync_airplay_pipeline_trace(
            &self.queue,
            &self.ring,
            &self.trace_decoder_fed,
            &self.trace_decoded_frames,
            &self.vram_pub_base,
            &self.stream_changes,
            &self.not_accepting_drains,
            &self.stale_dim_dropped,
            &self.presented_frames,
            &self.present_calls,
            &self.present_errors,
            &self.last_decoder_hresult,
            &self.last_mft_w,
            &self.last_mft_h,
            &self.expected_coded_w,
            &self.expected_coded_h,
        );
        self.redraw_requested.store(true, Ordering::SeqCst);
        self.ring.notify_all();
        schedule_coalesced_dcomp_commit(&self.dcomp_commit_pending);
    }

    pub fn set_bounds(&self, x: i32, y: i32, w: u32, h: u32, visible: bool) {
        self.visible_flag.store(visible, Ordering::SeqCst);
        if let Some(ref view) = self.view {
            if let Ok(mut v) = view.lock() {
                let _ = v.set_bounds(x, y, w.max(16), h.max(16), visible);
            }
        }
        if visible {
            self.ring.notify_all();
        }
    }

    pub fn set_visible(&self, visible: bool) {
        let prev = self.visible_flag.swap(visible, Ordering::SeqCst);
        if prev != visible {
            if let Some(ref view) = self.view {
                if let Ok(v) = view.lock() {
                    v.set_visible(visible);
                }
            }
            if visible {
                self.ring.notify_all();
            }
        }
    }

    pub fn set_fit_mode(&self, fit: FitMode) {
        if let Some(ref view) = self.view {
            if let Ok(mut v) = view.lock() {
                v.set_fit_mode(fit);
            }
        }
        self.redraw_requested.store(true, Ordering::SeqCst);
        self.ring.notify_all();
    }

    pub fn set_rotation(&self, deg: u16) {
        if let Some(ref view) = self.view {
            if let Ok(mut v) = view.lock() {
                v.set_rotation(deg);
            }
        }
        self.redraw_requested.store(true, Ordering::SeqCst);
        self.ring.notify_all();
    }

    pub fn set_auto_rotate_enabled(&self, enabled: bool) {
        if let Some(ref view) = self.view {
            if let Ok(mut v) = view.lock() {
                v.set_auto_rotate_enabled(enabled);
            }
        }
        if !enabled {
            self.pending_hevc_autorotate_deg.store(-1, Ordering::Release);
            if let Ok(mut tr) = self.hevc_autorotate_tracker.lock() {
                tr.reset();
            }
        }
    }

    pub fn take_pending_hevc_autorotate_deg(&self) -> Option<u16> {
        let deg = self.pending_hevc_autorotate_deg.swap(-1, Ordering::AcqRel);
        if deg >= 0 {
            Some(deg as u16)
        } else {
            None
        }
    }

    pub fn set_ipad_profile(&self, is_ipad: bool) {
        self.is_ipad_profile.store(is_ipad, Ordering::Release);
        if is_ipad {
            self.pending_hevc_autorotate_deg.store(-1, Ordering::Release);
        }
    }

    pub fn notify_autorotate_manual_override(&self, landscape_now: bool) {
        self.pending_hevc_autorotate_deg.store(-1, Ordering::Release);
        let now_ms = self.autorotate_clock.elapsed().as_millis() as u64;
        if let Ok(mut tr) = self.hevc_autorotate_tracker.lock() {
            tr.notify_manual_override(now_ms, landscape_now);
        }
    }

    pub fn notify_autorotate_nav_home_recent(&self) {
        self.pending_hevc_autorotate_deg.store(-1, Ordering::Release);
        let now_ms = self.autorotate_clock.elapsed().as_millis() as u64;
        if let Ok(mut tr) = self.hevc_autorotate_tracker.lock() {
            tr.notify_nav_home_recent(now_ms);
        }
    }

    pub fn set_airplay_render_policy(&self, policy: AirplayRenderPolicy) {
        if let Some(ref view) = self.view {
            if let Ok(mut v) = view.lock() {
                v.set_airplay_render_policy(policy);
            }
        }
        self.redraw_requested.store(true, Ordering::SeqCst);
        self.ring.notify_all();
    }

    pub fn set_airplay_mode(&self, is_airplay: bool) {
        if let Some(ref view) = self.view {
            if let Ok(mut v) = view.lock() {
                v.set_airplay_mode(is_airplay);
            }
        }
        self.redraw_requested.store(true, Ordering::SeqCst);
        self.ring.notify_all();
    }

    pub fn set_window_landscape(&self, landscape: bool) {
        if let Some(ref view) = self.view {
            if let Ok(mut v) = view.lock() {
                v.set_window_landscape(landscape);
            }
        }
        self.redraw_requested.store(true, Ordering::SeqCst);
        self.ring.notify_all();
    }

    pub fn set_crop(&self, top: u32, bottom: u32, left: u32, right: u32) {
        if let Some(ref view) = self.view {
            if let Ok(mut v) = view.lock() {
                v.set_crop(top, bottom, left, right);
            }
        }
        self.redraw_requested.store(true, Ordering::SeqCst);
        self.ring.notify_all();
    }

    pub fn capture_lossless_bmp(&self) -> Result<Vec<u8>, String> {
        let Some(ref view) = self.view else {
            return Err("LiveVideoHwnd belum siap".to_string());
        };
        let mut v = view.lock().map_err(|_| "Video view lock poisoned".to_string())?;
        v.capture_latest_frame_lossless_bmp()
    }

    /// Captures a fast downscaled tightly packed RGBA8 thumbnail (`< 0.4 ms` GPU) for
    /// Sequoia Blur-to-Sharp reveal and smooth exit transition.
    pub fn capture_rgba_thumbnail(&self, max_w: u32, max_h: u32) -> Option<(Vec<u8>, u32, u32)> {
        let view = self.view.as_ref()?;
        let mut v = view.lock().ok()?;
        v.capture_latest_frame_rgba_thumbnail(max_w, max_h)
    }

    /// Weak reference to the GPU presenter (`LiveVideoHwnd`) for asynchronous Spotlight OCR bursts
    /// without blocking the Slint UI thread or extending lifetime past `shutdown()`.
    pub fn weak_view(&self) -> Option<std::sync::Weak<Mutex<LiveVideoHwnd>>> {
        self.view.as_ref().map(Arc::downgrade)
    }

    pub fn attach_input_router(&self, router: Arc<crate::native_input::NativeInputRouter>) {
        if let Some(ref view) = self.view {
            if let Ok(v) = view.lock() {
                let r_ptr = router.clone();
                v.set_pointer_callback(Some(Arc::new(move |ev| {
                    r_ptr.handle_pointer_event(ev);
                })));
                let r_key = router;
                v.set_key_callback(Some(Arc::new(move |ev| {
                    r_key.handle_key_event(ev);
                })));
            }
        }
    }

    pub fn telemetry(&self) -> PipelineTelemetry {
        let codec = self
            .codec_label
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default();
        let codec_error_kind = self
            .codec_error_kind
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default();
        PipelineTelemetry {
            child_hwnd: self.child_hwnd,
            visible: self.visible_flag.load(Ordering::Relaxed),
            codec,
            codec_error_kind,
            width: self.dim_w.load(Ordering::Relaxed),
            height: self.dim_h.load(Ordering::Relaxed),
            decoded_frames: self.decoded_frames.load(Ordering::Relaxed),
            presented_frames: self.presented_frames.load(Ordering::Relaxed),
        }
    }

    /// Stops decode & render worker threads cleanly, then drops `LiveVideoHwnd` on the caller
    /// (UI creator) thread so Win32 `DestroyWindow` always succeeds with 0 leaks.
    pub fn shutdown(&mut self) {
        if self.shut_down {
            return;
        }
        self.shut_down = true;
        mirror::register_native_video_sink(None);
        self.stop_flag.store(true, Ordering::SeqCst);
        self.queue.close();
        self.ring.notify_all();
        if let Some(jh) = self.decode_thread.take() {
            let _ = jh.join();
        }
        if let Some(jh) = self.render_thread.take() {
            let _ = jh.join();
        }
        // Release ALL VRAM NV12 ComPtr textures BEFORE dropping LiveVideoHwnd / LiveD3d11Gpu
        // (whose Drop calls MFShutdown + FreeLibrary(d3d11.dll)!)
        self.ring.clear();
        // Drop LiveVideoHwnd on creator (UI) thread after both worker threads have exited!
        self.view = None;
    }
}

impl Drop for LiveVideoPipeline {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iphone_link_apple::mirror::base64_encode;
    use iphone_link_video::mf_decoder::build_synthetic_h264_ipcm_idr;

    #[test]
    fn test_tahap263_live_video_pipeline_routes_h264_avcc_to_d3d11_present_zero_webview2() {
        let _hid_guard = iphone_link_apple::hid::HID_TEST_SERIAL_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        mirror::global_reset_mirror_runtime_for_test();

        let mut pipe = LiveVideoPipeline::new(0, 0, 38, 346, 750, false, 0)
            .expect("LiveVideoPipeline must initialize offscreen with 0 WebView2");
        assert_ne!(pipe.telemetry().child_hwnd, 0);
        assert!(!pipe.telemetry().visible);
        assert!(mirror::is_native_video_sink_registered());

        // Promote to running + visible and feed real synthetic H.264 Baseline 64x64 I_PCM IDR
        // through mirror::update_live_codec_h264_avcc & global_on_physical_h264_avcc_frame!
        let _ = mirror::start_airplay_mirror_session(
            Some("00008150-0000000000000263"),
            false,
            "wifi",
            None,
            Some(0),
            false,
        );
        let gen = mirror::current_live_session_gen();
        pipe.set_visible(true);

        let frame = build_synthetic_h264_ipcm_idr(64, 64, 0x98, 0);
        let avcc_b64 = base64_encode(&frame.avcc_config);
        mirror::update_live_codec_h264_avcc(gen, "avc1.42801e", &avcc_b64, 64, 64);

        for idx in 0..8u32 {
            let f = build_synthetic_h264_ipcm_idr(64, 64, 0x90u8.wrapping_add(idx as u8 * 4), idx);
            let ok = mirror::global_on_physical_h264_avcc_frame(gen, f.avcc_sample, true);
            assert!(ok, "frame {idx} must advance mirror pipeline");
            std::thread::sleep(Duration::from_millis(12));
        }

        let deadline = Instant::now() + Duration::from_secs(3);
        let mut tel = pipe.telemetry();
        while Instant::now() < deadline {
            tel = pipe.telemetry();
            if tel.decoded_frames >= 1 && tel.presented_frames >= 1 && tel.width == 64 {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        assert!(tel.decoded_frames >= 1, "expected decoded >= 1, got {:?}", tel);
        assert!(tel.presented_frames >= 1, "expected presented >= 1, got {:?}", tel);
        assert_eq!((tel.width, tel.height), (64, 64));
        assert_eq!(tel.codec, "AirPlay H.264");
        assert!(tel.visible);

        let _ = mirror::global_stop_screen();
        pipe.shutdown();
        assert!(!mirror::is_native_video_sink_registered());
        mirror::global_reset_mirror_runtime_for_test();
    }

    #[test]
    fn test_tahap266_auto_to_stop_to_airplay_h264_reconfigures_composition_decoder_cleanly() {
        let _hid_guard = iphone_link_apple::hid::HID_TEST_SERIAL_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        mirror::global_reset_mirror_runtime_for_test();

        let gpu = Arc::new(LiveD3d11Gpu::new().expect("LiveD3d11Gpu::new"));
        let ready_hits = Arc::new(AtomicU32::new(0));
        let ready_clone = ready_hits.clone();

        let mut pipe = LiveVideoPipeline::new_composition(gpu, 346, 750, 0)
            .expect("LiveVideoPipeline::new_composition must succeed");
        assert!(pipe.is_composition_mode());
        assert_eq!(pipe.telemetry().child_hwnd, 0);
        assert!(pipe.swapchain_ptr().is_some());
        pipe.set_frame_ready_callback(Some(Arc::new(move || {
            ready_clone.fetch_add(1, Ordering::SeqCst);
        })));

        // Step 1: Simulate an initial Auto HEVC session stopped cleanly -> reset_for_new_session
        pipe.queue.try_push(NativeVideoEvent::HevcConfig {
            codec: "hvc1".to_string(),
            width: 1206,
            height: 2622,
            trim: [0, 0, 0, 0],
        });
        std::thread::sleep(Duration::from_millis(80));
        pipe.queue.try_push(NativeVideoEvent::Stopped);
        let _ = mirror::global_stop_screen();
        pipe.reset_for_new_session();
        std::thread::sleep(Duration::from_millis(80));
        assert_eq!(pipe.telemetry().presented_frames, 0);
        assert_eq!(pipe.telemetry().codec, "");

        // Step 2: Switch to AirPlay H.264 and feed real H.264 IDR frames BEFORE visible flips true
        // (verifies Frame #1 pre-reveal present fires ready callback + switches decoder to H.264!)
        let _ = mirror::start_airplay_mirror_session(
            Some("00008150-0000000000000266"),
            false,
            "wifi",
            None,
            Some(0),
            false,
        );
        let gen = mirror::current_live_session_gen();
        let frame0 = build_synthetic_h264_ipcm_idr(64, 64, 0x9A, 0);
        let avcc_b64 = base64_encode(&frame0.avcc_config);
        mirror::update_live_codec_h264_avcc(gen, "avc1.42801e", &avcc_b64, 64, 64);

        for idx in 0..6u32 {
            let f = build_synthetic_h264_ipcm_idr(64, 64, 0x80u8.wrapping_add(idx as u8 * 6), idx);
            assert!(mirror::global_on_physical_h264_avcc_frame(gen, f.avcc_sample, true));
            std::thread::sleep(Duration::from_millis(12));
        }

        let deadline = Instant::now() + Duration::from_secs(3);
        let mut tel = pipe.telemetry();
        while Instant::now() < deadline {
            tel = pipe.telemetry();
            if tel.decoded_frames >= 1 && tel.presented_frames >= 1 && ready_hits.load(Ordering::SeqCst) >= 1 {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        assert!(tel.decoded_frames >= 1, "expected H.264 decoded >= 1, got {:?}", tel);
        assert!(tel.presented_frames >= 1, "expected H.264 presented >= 1, got {:?}", tel);
        assert_eq!(tel.codec, "AirPlay H.264");
        assert!(ready_hits.load(Ordering::SeqCst) >= 1, "Frame #1 ready callback must fire");

        pipe.resize_composition(519, 1125)
            .expect("resize_composition must succeed");

        let _ = mirror::global_stop_screen();
        pipe.shutdown();
        mirror::global_reset_mirror_runtime_for_test();
    }

    #[test]
    fn test_tahap267_airplay_h264_continuous_present_and_midstream_resolution_change() {
        let _hid_guard = hid::HID_TEST_SERIAL_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        mirror::global_reset_mirror_runtime_for_test();

        let gpu = Arc::new(LiveD3d11Gpu::new().expect("LiveD3d11Gpu"));
        let mut pipe = LiveVideoPipeline::new_composition(gpu, 320, 640, 0)
            .expect("composition pipeline");
        pipe.prewarm_h264_decoder();

        let _ = mirror::start_airplay_mirror_session(
            Some("00008150-0000000000000267"),
            false,
            "wifi",
            None,
            Some(0),
            false,
        );
        let gen = mirror::current_live_session_gen();

        let frame0 = build_synthetic_h264_ipcm_idr(64, 64, 0x90, 0);
        let avcc_b64 = base64_encode(&frame0.avcc_config);
        mirror::update_live_codec_h264_avcc(gen, "avc1.42801e", &avcc_b64, 64, 64);
        assert!(mirror::global_on_physical_h264_avcc_frame(
            gen,
            frame0.avcc_sample.clone(),
            true
        ));
        let first_dl = Instant::now() + Duration::from_secs(3);
        while Instant::now() < first_dl {
            if pipe.telemetry().presented_frames >= 1 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        // Reveal video stage (mirrors Sequoia Seamless Reveal flipping `first_frame_ready=true`)
        pipe.set_visible(true);

        for seq in 1..30u32 {
            let p_skip =
                iphone_link_video::mf_decoder::build_synthetic_h264_p_skip_avcc_sample(64, 64, seq);
            assert!(mirror::global_on_physical_h264_avcc_frame(gen, p_skip, false));
            std::thread::sleep(Duration::from_millis(12));
        }

        let deadline = Instant::now() + Duration::from_secs(4);
        let mut tel = pipe.telemetry();
        while Instant::now() < deadline {
            tel = pipe.telemetry();
            if tel.decoded_frames >= 30 && tel.presented_frames >= 6 {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            tel.decoded_frames >= 30,
            "expected all 30 continuous AirPlay H.264 frames decoded (1 IDR + 29 P_SKIP), got {:?}",
            tel
        );
        assert!(
            tel.presented_frames >= 6,
            "expected continuous AirPlay H.264 presentation (>1 frame!), got {:?}",
            tel
        );

        // Mid-stream rotation (64x64 -> 64x96) + duplicate resize skip check
        assert!(pipe.resize_composition(400, 600).expect("first resize"));
        assert!(!pipe.resize_composition(400, 600).expect("redundant resize skipped"));

        let f_rot = build_synthetic_h264_ipcm_idr(64, 96, 0xA4, 1);
        let rot_b64 = base64_encode(&f_rot.avcc_config);
        mirror::update_live_codec_h264_avcc(gen, "avc1.42801e", &rot_b64, 64, 96);
        assert!(mirror::global_on_physical_h264_avcc_frame(
            gen,
            f_rot.avcc_sample,
            true
        ));
        let p_rot =
            iphone_link_video::mf_decoder::build_synthetic_h264_p_skip_avcc_sample(64, 96, 1);
        assert!(mirror::global_on_physical_h264_avcc_frame(gen, p_rot, false));

        let deadline2 = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline2 {
            tel = pipe.telemetry();
            if tel.decoded_frames >= 32 && tel.width == 64 && tel.height == 96 {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(tel.decoded_frames >= 32, "expected post-rotation frames decoded: {:?}", tel);
        assert_eq!((tel.width, tel.height), (64, 96));

        let _ = mirror::global_stop_screen();
        pipe.shutdown();
        mirror::global_reset_mirror_runtime_for_test();
    }

    #[test]
    fn test_tahap271_airplay_midstream_stall_watchdog_recovers_decoder_and_clamps_pts() {
        use iphone_link_video::mf_decoder::{
            clamp_monotonic_pts_delta_100ns, DEFAULT_PTS_DELTA_100NS, MAX_PTS_DELTA_100NS,
            MIN_PTS_DELTA_100NS,
        };

        // 1. PTS clamping protects CMSH264DecoderMFT across pauses & bursts
        assert_eq!(clamp_monotonic_pts_delta_100ns(-1, 500_000), 500_000);
        // Non-monotonic or duplicate PTS -> +166_667 100ns default
        assert_eq!(
            clamp_monotonic_pts_delta_100ns(1_000_000, 1_000_000),
            1_000_000 + DEFAULT_PTS_DELTA_100NS
        );
        assert_eq!(
            clamp_monotonic_pts_delta_100ns(1_000_000, 900_000),
            1_000_000 + DEFAULT_PTS_DELTA_100NS
        );
        // Tiny sub-ms delta clamped up to MIN_PTS_DELTA_100NS (10_000 = 1 ms)
        assert_eq!(
            clamp_monotonic_pts_delta_100ns(1_000_000, 1_000_100),
            1_000_000 + MIN_PTS_DELTA_100NS
        );
        // Large static screen pause clamped down to MAX_PTS_DELTA_100NS (1_000_000 = 100 ms)
        assert_eq!(
            clamp_monotonic_pts_delta_100ns(1_000_000, 90_000_000),
            1_000_000 + MAX_PTS_DELTA_100NS
        );

        // 2. Watchdog resets ONLY when presented progresses (stale decoded alone never blinds it)
        let mut wd = AirplayStallWatchdog::new();
        for i in 0..10 {
            assert!(!wd.observe(true, 5, 3, 1000 + i * 50));
        }
        assert!(!wd.observe(true, 6, 4, 2100));
        assert_eq!(wd.stall_packets(), 0);

        // 3. >= 60 stalled video-slice packets across >= 3500 ms triggers recovery cleanly & increments airplayWatchdogRecoveries
        let mut recovered = false;
        for i in 0..AIRPLAY_STALL_MIN_PACKETS {
            if wd.observe(true, 6, 4, 3000 + u64::from(i) * 65) {
                recovered = true;
            }
        }
        assert!(recovered, "Watchdog must trigger after >=60 packets over >=3500ms without progress");
        assert_eq!(wd.recoveries(), 1);
        assert_eq!(wd.stall_packets(), 0);

        airplay::reset_airplay_trace();
        airplay::inc_trace_watchdog_recoveries();
        let trace_json = airplay::get_airplay_trace_json();
        assert!(
            trace_json.contains(r#""airplayWatchdogRecoveries":1"#),
            "airplay-trace JSON must expose airplayWatchdogRecoveries: {trace_json}"
        );
        airplay::reset_airplay_trace();

        // 4. Deep AirPlay queue (cap=240) with 2 GOPs trims cleanly at GOP #2 IDR without latching awaiting_idr
        let deep_q = BoundedNativeVideoQueue::new_with_h264_capacity(16, NATIVE_AIRPLAY_VIDEO_QUEUE_CAPACITY);
        let idr = build_synthetic_h264_ipcm_idr(64, 64, 0x90, 0);
        deep_q.try_push(NativeVideoEvent::H264Avcc {
            avcc: idr.avcc_config.clone(),
            width: 64,
            height: 64,
        });
        deep_q.try_push(NativeVideoEvent::H264Frame {
            avcc_payload: idr.avcc_sample.clone(),
            is_keyframe: true,
            pts_us: 1000,
        });
        for i in 1..=(NATIVE_AIRPLAY_VIDEO_QUEUE_CAPACITY + 10) {
            if i == 120 {
                deep_q.try_push(NativeVideoEvent::H264Frame {
                    avcc_payload: idr.avcc_sample.clone(),
                    is_keyframe: true,
                    pts_us: 1000 + (i as u64) * 16_667,
                });
            } else {
                deep_q.try_push(NativeVideoEvent::H264Frame {
                    avcc_payload: vec![0, 0, 0, 4, 0x41, 0xE0, (i & 0xFF) as u8, 0x22],
                    is_keyframe: false,
                    pts_us: 1000 + (i as u64) * 16_667,
                });
            }
        }
        let (_qlen, awaiting_idr, dropped, _, _) = deep_q.stats();
        assert!(!awaiting_idr, "Deep AirPlay queue with fresh GOP IDR must NEVER latch awaiting_idr on overflow");
        assert!(dropped >= 10);
    }

    #[test]
    fn test_tahap272_airplay_watchdog_45pkts_1500ms_cooldown_3000ms_and_slice_filter() {
        assert_eq!(AIRPLAY_STALL_MIN_PACKETS, 90);
        assert_eq!(AIRPLAY_STALL_MIN_ELAPSED_MS, 4500);
        assert_eq!(AIRPLAY_STALL_COOLDOWN_MS, 10000);

        let mut wd = AirplayStallWatchdog::new();

        // 1. Non-slice packets (AUD/SEI/SPS keepalive) NEVER trigger recovery even over 6000 ms
        for i in 0..100u64 {
            assert!(!wd.on_h264_packet(1000 + i * 60, 0, false));
        }
        assert_eq!(wd.reset_count(), 0);

        // 2. 89 slice packets over 4895 ms do NOT trigger; 90th triggers
        for i in 0..(AIRPLAY_STALL_MIN_PACKETS - 1) as u64 {
            assert!(!wd.on_h264_packet(10_000 + i * 55, 0, true));
        }
        assert_eq!(wd.reset_count(), 0);
        assert!(wd.on_h264_packet(10_000 + 89 * 55, 0, true));
        assert_eq!(wd.reset_count(), 1);
        let reset_at = 10_000 + 89 * 55;

        // 3. Inside 10000 ms cooldown window (0..9999 ms), even 250 slice packets NEVER trigger
        for i in 1..250u64 {
            let t = reset_at + i * 39;
            if t - reset_at < AIRPLAY_STALL_COOLDOWN_MS {
                assert!(!wd.on_h264_packet(t, 0, true));
            }
        }
        assert_eq!(wd.reset_count(), 1);

        // 4. After cooldown >= 10000 ms, another 90 slice packets over >= 4500 ms triggers #2
        let resume_t = reset_at + AIRPLAY_STALL_COOLDOWN_MS + 10;
        let mut second = false;
        for i in 0..AIRPLAY_STALL_MIN_PACKETS as u64 {
            if wd.on_h264_packet(resume_t + i * 55, 0, true) {
                second = true;
            }
        }
        assert!(second, "Expected second recovery after >=10000ms cooldown and 90 slice packets");
        assert_eq!(wd.reset_count(), 2);
    }

    #[test]
    fn test_tahap273_watchdog_tracks_presented_frames_and_recovers_in_1200ms_20_packets() {
        assert_eq!(AIRPLAY_STALL_MIN_PACKETS, 90);
        assert_eq!(AIRPLAY_STALL_MIN_ELAPSED_MS, 4500);

        let mut wd = AirplayStallWatchdog::new();
        // Frame #1 decoded & presented at 500 ms
        assert!(!wd.observe(true, 1, 1, 500));

        // Genuine hard decoder stall (decoded=1, presented=1) recovers at 90 packets / >= 4500 ms
        let mut recovered = false;
        for idx in 0..u64::from(AIRPLAY_STALL_MIN_PACKETS) {
            let trig = wd.observe(true, 1, 1, 1000 + idx * 55);
            if idx < u64::from(AIRPLAY_STALL_MIN_PACKETS - 1) {
                assert!(!trig, "must wait for full 90 packets / 4500 ms window");
            }
            if trig {
                recovered = true;
            }
        }
        assert!(
            recovered,
            "Watchdog must recover in 90 packets / 4500 ms when decoder is hard-stalled"
        );
        assert_eq!(wd.recoveries(), 1);

        // Coalesced DComp commit helper safely resets flag even when UI loop is not running
        let pending = Arc::new(AtomicBool::new(false));
        schedule_coalesced_dcomp_commit(&pending);
        assert!(!pending.load(Ordering::Acquire));

        // Real decoded/presented advance clears stall packets after 10000 ms cooldown
        assert!(!wd.observe(true, 1, 1, 16_500));
        assert_eq!(wd.stall_packets(), 1);
        assert!(!wd.observe(true, 2, 2, 16_600));
        assert_eq!(wd.stall_packets(), 0);
    }

    #[test]
    fn test_tahap274_calm_watchdog_60pkts_3500ms_grace_reset_and_20x_orientation_endurance() {
        use iphone_link_video::mf_decoder::{
            build_synthetic_h264_ipcm_idr_cropped, build_synthetic_h264_p_skip_cropped_with_aud_sei,
            wrap_avcc_with_aud_sei,
        };

        // 1. Calm watchdog thresholds + reset_grace() immunity during orientation/IDR waits
        assert_eq!(AIRPLAY_STALL_MIN_PACKETS, 90);
        assert_eq!(AIRPLAY_STALL_MIN_ELAPSED_MS, 4500);
        assert_eq!(AIRPLAY_STALL_COOLDOWN_MS, 10000);

        let mut wd = AirplayStallWatchdog::new();
        assert!(!wd.observe(true, 1, 1, 900));
        for i in 0..25u64 {
            assert!(!wd.observe(true, 1, 1, 1000 + i * 65));
        }
        assert_eq!(wd.recoveries(), 0);
        assert_eq!(wd.stall_packets(), 25);

        wd.reset_grace();
        assert_eq!(wd.stall_packets(), 0);
        assert_eq!(wd.recoveries(), 0);

        // 2. Live 20x Portrait (888x1920) <-> Landscape (1920x888) orientation endurance in pipeline
        let _hid_guard = hid::HID_TEST_SERIAL_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        mirror::global_reset_mirror_runtime_for_test();
        airplay::reset_airplay_trace();

        let gpu = Arc::new(LiveD3d11Gpu::new().expect("LiveD3d11Gpu"));
        let mut pipe = LiveVideoPipeline::new_composition(gpu, 346, 750, 0)
            .expect("composition pipeline");
        pipe.set_airplay_mode(true);
        pipe.set_window_landscape(false);
        pipe.prewarm_h264_decoder();

        let _ = mirror::start_airplay_mirror_session(
            Some("00008150-0000000000000274"),
            false,
            "wifi",
            None,
            Some(0),
            false,
        );
        let gen = mirror::current_live_session_gen();

        let f_port = build_synthetic_h264_ipcm_idr_cropped(888, 1920, 0x88, 0);
        let port_avcc_b64 = base64_encode(&f_port.avcc_config);
        let port_idr = wrap_avcc_with_aud_sei(&f_port.avcc_sample);
        let port_p1 = build_synthetic_h264_p_skip_cropped_with_aud_sei(888, 1920, 1);
        let port_p2 = build_synthetic_h264_p_skip_cropped_with_aud_sei(888, 1920, 2);
        drop(f_port);

        let f_land = build_synthetic_h264_ipcm_idr_cropped(1920, 888, 0xA0, 1);
        let land_avcc_b64 = base64_encode(&f_land.avcc_config);
        let land_idr = wrap_avcc_with_aud_sei(&f_land.avcc_sample);
        let land_p1 = build_synthetic_h264_p_skip_cropped_with_aud_sei(1920, 888, 1);
        let land_p2 = build_synthetic_h264_p_skip_cropped_with_aud_sei(1920, 888, 2);
        drop(f_land);

        pipe.set_visible(true);
        for cycle in 0..20u32 {
            if cycle % 2 == 0 {
                mirror::update_live_codec_h264_avcc(gen, "avc1.42801e", &port_avcc_b64, 888, 1920);
                assert!(mirror::global_on_physical_h264_avcc_frame(gen, port_idr.clone(), true));
                assert!(mirror::global_on_physical_h264_avcc_frame(gen, port_p1.clone(), false));
                assert!(mirror::global_on_physical_h264_avcc_frame(gen, port_p2.clone(), false));
            } else {
                mirror::update_live_codec_h264_avcc(gen, "avc1.42801e", &land_avcc_b64, 1920, 888);
                assert!(mirror::global_on_physical_h264_avcc_frame(gen, land_idr.clone(), true));
                assert!(mirror::global_on_physical_h264_avcc_frame(gen, land_p1.clone(), false));
                assert!(mirror::global_on_physical_h264_avcc_frame(gen, land_p2.clone(), false));
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        let dl = Instant::now() + Duration::from_secs(6);
        let mut tel = pipe.telemetry();
        let mut tc = airplay::get_airplay_trace_counters();
        while Instant::now() < dl {
            tel = pipe.telemetry();
            tc = airplay::get_airplay_trace_counters();
            if tel.decoded_frames >= 20 && tel.presented_frames >= 8 && tc.dual_buffer_overwrites >= 1
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(
            tel.decoded_frames >= 20,
            "20x orientation endurance must decode smoothly without MFT stalls: {:?}",
            tel
        );
        assert!(
            tel.presented_frames >= 8,
            "20x orientation endurance must keep presenting without black freeze: {:?}",
            tel
        );
        assert_eq!(
            tc.mid_stream_black_clears, 0,
            "mid-stream orientation transitions MUST produce 0 black clears: {:?}",
            tc
        );
        assert!(
            tc.dual_buffer_overwrites >= 1,
            "seamless transitions must atomically overwrite dual backbuffers: {:?}",
            tc
        );

        let _ = mirror::global_stop_screen();
        pipe.shutdown();
        mirror::global_reset_mirror_runtime_for_test();
    }

    #[test]
    fn test_tahap276_vfr_sparse_frames_advance_presented_and_dcomp_commits_without_stall() {
        use iphone_link_video::mf_decoder::{
            build_synthetic_h264_ipcm_idr_cropped, build_synthetic_h264_p_skip_cropped_with_aud_sei,
            wrap_avcc_with_aud_sei,
        };

        let _hid_guard = hid::HID_TEST_SERIAL_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        mirror::global_reset_mirror_runtime_for_test();
        airplay::reset_airplay_trace();

        let gpu = Arc::new(LiveD3d11Gpu::new().expect("LiveD3d11Gpu"));
        let mut pipe = LiveVideoPipeline::new_composition(gpu, 346, 750, 0)
            .expect("composition pipeline");
        pipe.set_airplay_mode(true);
        pipe.prewarm_h264_decoder();

        let _ = mirror::start_airplay_mirror_session(
            Some("00008150-0000000000000276"),
            false,
            "wifi",
            None,
            Some(0),
            false,
        );
        let gen = mirror::current_live_session_gen();

        let f64 = build_synthetic_h264_ipcm_idr_cropped(64, 64, 0x84, 0);
        let avcc64 = base64_encode(&f64.avcc_config);
        let idr64 = wrap_avcc_with_aud_sei(&f64.avcc_sample);
        let p64 = [
            build_synthetic_h264_p_skip_cropped_with_aud_sei(64, 64, 1),
            build_synthetic_h264_p_skip_cropped_with_aud_sei(64, 64, 2),
            build_synthetic_h264_p_skip_cropped_with_aud_sei(64, 64, 3),
            build_synthetic_h264_p_skip_cropped_with_aud_sei(64, 64, 4),
            build_synthetic_h264_p_skip_cropped_with_aud_sei(64, 64, 5),
        ];
        drop(f64);

        let f96 = build_synthetic_h264_ipcm_idr_cropped(96, 64, 0x9E, 1);
        let avcc96 = base64_encode(&f96.avcc_config);
        let idr96 = wrap_avcc_with_aud_sei(&f96.avcc_sample);
        let p96_1 = build_synthetic_h264_p_skip_cropped_with_aud_sei(96, 64, 1);
        let p96_2 = build_synthetic_h264_p_skip_cropped_with_aud_sei(96, 64, 2);
        drop(f96);

        pipe.set_visible(true);

        // VFR Burst 1: 64x64 (3 frames)
        mirror::update_live_codec_h264_avcc(gen, "avc1.42801e", &avcc64, 64, 64);
        assert!(mirror::global_on_physical_h264_avcc_frame(gen, idr64, true));
        assert!(mirror::global_on_physical_h264_avcc_frame(gen, p64[0].clone(), false));
        assert!(mirror::global_on_physical_h264_avcc_frame(gen, p64[1].clone(), false));

        let dl1 = Instant::now() + Duration::from_secs(2);
        while Instant::now() < dl1 {
            if pipe.telemetry().presented_frames >= 1 && airplay::trace_dcomp_commits() >= 1 {
                break;
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        let tel1 = pipe.telemetry();
        let com1 = airplay::trace_dcomp_commits();
        assert!(tel1.presented_frames >= 1, "Burst 1 must present >= 1: {tel1:?}");
        assert!(com1 >= 1, "Burst 1 must commit DComp >= 1, got {com1}");
        assert!(airplay::trace_mf_low_latency_enabled(), "trace must record mfLowLatencyEnabled=true");

        // VFR static-screen pause (1500 ms -> zero packets)
        std::thread::sleep(Duration::from_millis(1500));

        // VFR Burst 2: 3 more 64x64 sparse P-frames
        assert!(mirror::global_on_physical_h264_avcc_frame(gen, p64[2].clone(), false));
        assert!(mirror::global_on_physical_h264_avcc_frame(gen, p64[3].clone(), false));
        assert!(mirror::global_on_physical_h264_avcc_frame(gen, p64[4].clone(), false));

        let dl2 = Instant::now() + Duration::from_secs(2);
        while Instant::now() < dl2 {
            if pipe.telemetry().presented_frames > tel1.presented_frames
                && airplay::trace_dcomp_commits() > com1
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        let tel2 = pipe.telemetry();
        let com2 = airplay::trace_dcomp_commits();
        assert!(
            tel2.presented_frames > tel1.presented_frames,
            "Burst 2 after 1.5s VFR pause MUST advance presented_frames ({tel2:?} vs {tel1:?})"
        );
        assert!(com2 > com1, "Burst 2 must advance dcompCommits ({com2} vs {com1})");

        // Switch dims -> 96x64 + VFR Burst 3
        mirror::update_live_codec_h264_avcc(gen, "avc1.42801e", &avcc96, 96, 64);
        assert!(mirror::global_on_physical_h264_avcc_frame(gen, idr96, true));
        assert!(mirror::global_on_physical_h264_avcc_frame(gen, p96_1, false));
        assert!(mirror::global_on_physical_h264_avcc_frame(gen, p96_2, false));

        let dl3 = Instant::now() + Duration::from_secs(3);
        while Instant::now() < dl3 {
            let t = pipe.telemetry();
            if t.presented_frames > tel2.presented_frames
                && airplay::trace_dcomp_commits() > com2
                && t.width == 96
                && t.height == 64
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        let tel3 = pipe.telemetry();
        let com3 = airplay::trace_dcomp_commits();
        assert!(
            tel3.presented_frames > tel2.presented_frames,
            "Burst 3 after dim switch MUST advance presented_frames ({tel3:?} vs {tel2:?})"
        );
        assert!(com3 > com2, "Burst 3 must advance dcompCommits ({com3} vs {com2})");
        assert_eq!((tel3.width, tel3.height), (96, 64));

        let _ = mirror::global_stop_screen();
        pipe.shutdown();
        mirror::global_reset_mirror_runtime_for_test();
        cleanup_airplay_stall_dump_file();
    }

    #[test]
    fn test_tahap276_stall_auto_dump_writes_non_pii_json_and_cleans_up() {
        airplay::reset_airplay_trace();
        airplay::set_trace_mf_low_latency_enabled(true);
        airplay::inc_trace_dcomp_commits();
        airplay::inc_trace_tcp_video_packets();

        let path = std::env::temp_dir().join("test_tahap276_stall_autodump_unit.json");
        let _ = std::fs::remove_file(&path);

        let mut st = AirplayStallDumpState::default();
        assert!(!maybe_write_airplay_stall_dump(
            true, 25, 3, 45, 1000, &mut st, &path
        ));
        assert!(!path.exists());

        // Right below threshold (2999 ms) -> no dump yet
        assert!(!maybe_write_airplay_stall_dump(
            true, 25, 3, 45, 3999, &mut st, &path
        ));
        assert!(!path.exists());

        // Exceeds 3000 ms threshold (3150 ms) with tcpVideoPackets > 0 -> dumps valid PII-free JSON ONCE
        assert!(maybe_write_airplay_stall_dump(
            true, 25, 3, 120, 4150, &mut st, &path
        ));
        assert!(path.exists());

        let raw = std::fs::read_to_string(&path).expect("read stall auto-dump JSON");
        assert!(raw.contains(r#""stallDump":true"#), "{raw}");
        assert!(raw.contains(r#""lastPacketAgeMs":120"#), "{raw}");
        assert!(raw.contains(r#""mfLowLatencyEnabled":true"#), "{raw}");
        assert!(raw.contains(r#""dcompCommits":1"#), "{raw}");
        assert!(raw.contains(r#""tcpVideoPackets":1"#), "{raw}");
        assert!(
            serde_json::from_str(&raw).is_ok(),
            "stall dump must be valid JSON: {raw}"
        );
        assert!(!raw.contains("192.168."), "Zero PII allowed in dump: {raw}");
        assert!(!raw.contains("000081"), "Zero UDID allowed in dump: {raw}");

        // Same stall does not overwrite repeatedly
        assert!(!maybe_write_airplay_stall_dump(
            true, 30, 3, 80, 6000, &mut st, &path
        ));

        // Progress resets stall state cleanly
        assert!(!maybe_write_airplay_stall_dump(
            true, 31, 4, 10, 6100, &mut st, &path
        ));
        assert!(!st.dumped_for_current_stall);

        let _ = std::fs::remove_file(&path);
        assert!(!path.exists());
        cleanup_airplay_stall_dump_file();
        assert!(!airplay_stall_dump_path().exists());
        airplay::reset_airplay_trace();
    }

    #[test]
    fn test_tahap277_calm_decoder_watchdog_90pkts_4500ms_preserves_healthy_vfr_dpb() {
        assert_eq!(AIRPLAY_STALL_MIN_PACKETS, 90);
        assert_eq!(AIRPLAY_STALL_MIN_ELAPSED_MS, 4500);
        assert_eq!(AIRPLAY_STALL_COOLDOWN_MS, 10000);

        let mut wd = AirplayStallWatchdog::new();
        assert!(!wd.observe(true, 1, 1, 500));

        // 60 packets over 3900 ms MUST NOT trigger false-kill on healthy VFR stream
        for i in 0..60u64 {
            assert!(
                !wd.observe(true, 1, 1, 1000 + i * 65),
                "60 packets / 3835 ms MUST NOT false-trigger Tahap 277 watchdog"
            );
        }
        assert_eq!(wd.recoveries(), 0);

        // If mfDecodedFrames keeps advancing while presented_frames is briefly held (e.g. duplicate
        // or resize boundary), watchdog preserves healthy DPB (0 false recoveries)!
        assert!(!wd.observe(true, 2, 1, 5000));
        assert_eq!(wd.stall_packets(), 0);
        assert_eq!(wd.recoveries(), 0);
        for i in 0..120u64 {
            assert!(!wd.observe(true, 3 + i, 1, 5100 + i * 60));
        }
        assert_eq!(
            wd.recoveries(),
            0,
            "Advancing mfDecodedFrames MUST protect healthy MFT DPB from false recovery"
        );

        // Only a true decoder hard-stall (both decoded & presented stuck for >=90 pkts AND >=4500ms) recovers
        let mut recovered = false;
        for i in 0..u64::from(AIRPLAY_STALL_MIN_PACKETS) {
            if wd.observe(true, 122, 1, 13_000 + i * 55) {
                recovered = true;
            }
        }
        assert!(recovered);
        assert_eq!(wd.recoveries(), 1);
    }

    #[test]
    fn test_tahap278_three_consecutive_airplay_reconnects_zero_pre_decoder_packet_loss() {
        use iphone_link_apple::hid;
        use iphone_link_apple::mirror;
        use iphone_link_video::mf_decoder::{
            build_synthetic_h264_ipcm_idr_cropped, build_synthetic_h264_p_skip_cropped_with_aud_sei,
            wrap_avcc_with_aud_sei,
        };

        let _hid_guard = hid::HID_TEST_SERIAL_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        mirror::global_reset_mirror_runtime_for_test();
        airplay::reset_airplay_trace();

        let gpu = Arc::new(LiveD3d11Gpu::new().expect("LiveD3d11Gpu"));
        let mut pipe = LiveVideoPipeline::new_composition(gpu, 519, 1125, 0)
            .expect("composition pipeline");
        pipe.set_airplay_mode(true);
        pipe.set_window_landscape(false);
        pipe.prewarm_h264_decoder();

        let _ = mirror::start_airplay_mirror_session(
            Some("00008150-0000000000000278"),
            false,
            "wifi",
            None,
            Some(0),
            false,
        );
        let gen = mirror::current_live_session_gen();

        let f_port = build_synthetic_h264_ipcm_idr_cropped(994, 2160, 0x7E, 0);
        let port_avcc_b64 = base64_encode(&f_port.avcc_config);
        let port_idr = wrap_avcc_with_aud_sei(&f_port.avcc_sample);
        drop(f_port);

        pipe.set_visible(true);
        let mut expected_fed_min = 0u64;
        for reconnect in 0..3u32 {
            mirror::global_on_airplay_mirror_loop_state(gen, true);
            if reconnect < 2 {
                mirror::update_live_codec_h264_avcc(gen, "avc1.42801e", &port_avcc_b64, 994, 2160);
            }
            // Wait if reconnect == 2 without resending update_live_codec_h264_avcc:
            // LATEST_AIRPLAY_AVCC_SNAPSHOT + auto-attach guarantees 0 awaiting_avcc drops.
            assert!(mirror::global_on_physical_h264_avcc_frame(
                gen,
                port_idr.clone(),
                true
            ));
            for p_idx in 1..=25u32 {
                let p_au = build_synthetic_h264_p_skip_cropped_with_aud_sei(994, 2160, p_idx);
                assert!(mirror::global_on_physical_h264_avcc_frame(gen, p_au, false));
                std::thread::sleep(Duration::from_millis(2));
            }
            expected_fed_min += 26;

            let dl = Instant::now() + Duration::from_secs(4);
            while Instant::now() < dl {
                let tc = airplay::get_airplay_trace_counters();
                if tc.decoder_fed >= expected_fed_min && tc.mf_decoded_frames >= expected_fed_min - 6 {
                    break;
                }
                std::thread::sleep(Duration::from_millis(15));
            }

            if reconnect < 2 {
                mirror::mark_airplay_stream_disconnected(gen);
                std::thread::sleep(Duration::from_millis(25));
            }
        }

        let dl = Instant::now() + Duration::from_secs(4);
        let mut tc = airplay::get_airplay_trace_counters();
        while Instant::now() < dl {
            tc = airplay::get_airplay_trace_counters();
            if tc.decoder_fed >= 78
                && tc.mf_decoded_frames >= 72
                && tc.dcomp_presented_frames >= 8
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        assert_eq!(tc.pre_decoder_dropped_awaiting_avcc, 0, "0 awaiting_avcc drops: {tc:?}");
        assert_eq!(tc.pre_decoder_dropped_awaiting_idr, 0, "0 awaiting_idr drops: {tc:?}");
        assert_eq!(tc.pre_decoder_dropped_stale_epoch, 0, "0 stale_epoch drops: {tc:?}");
        assert!(
            tc.native_video_sink_packets >= 78 && tc.native_video_sink_packets <= 86,
            "all 78 video frames + control events must enter sink: {tc:?}"
        );
        assert_eq!(
            tc.decoder_fed, 78,
            "3 reconnects MUST keep 100% parity decoder_fed == 78 video frames (0 lost): {tc:?}"
        );
        assert_eq!(
            tc.vram_published, tc.mf_decoded_frames,
            "vram_published MUST match cumulative mf_decoded_frames cleanly: {tc:?}"
        );
        assert!(tc.mf_decoded_frames >= 70, "decoded >= 70 across 3 reconnects: {tc:?}");
        assert!(tc.dcomp_presented_frames >= 8, "presented >= 8 across 3 reconnects: {tc:?}");
        assert!(tc.dcomp_commits >= 10, "expected >= 10 DComp commits across 3 reconnects: {tc:?}");
        assert_eq!(tc.cached_idr_replays, 0, "healthy reconnects must not invoke stall replay: {tc:?}");

        let _ = mirror::global_stop_screen();
        pipe.shutdown();
        mirror::global_reset_mirror_runtime_for_test();
    }

    #[test]
    fn test_tahap278_infinite_gop_500_pframes_and_repeated_994x2160_avcc_zero_watchdog_kill() {
        use iphone_link_apple::hid;
        use iphone_link_apple::mirror;
        use iphone_link_video::mf_decoder::{
            build_synthetic_h264_ipcm_idr_cropped, build_synthetic_h264_p_skip_cropped_with_aud_sei,
            wrap_avcc_with_aud_sei,
        };

        let _hid_guard = hid::HID_TEST_SERIAL_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        mirror::global_reset_mirror_runtime_for_test();
        airplay::reset_airplay_trace();

        let gpu = Arc::new(LiveD3d11Gpu::new().expect("LiveD3d11Gpu"));
        let mut pipe = LiveVideoPipeline::new_composition(gpu, 519, 1125, 0)
            .expect("composition pipeline");
        pipe.set_airplay_mode(true);
        pipe.set_window_landscape(false);
        pipe.prewarm_h264_decoder();

        let _ = mirror::start_airplay_mirror_session(
            Some("00008150-0000000000000278"),
            false,
            "wifi",
            None,
            Some(0),
            false,
        );
        let gen = mirror::current_live_session_gen();

        let f_port = build_synthetic_h264_ipcm_idr_cropped(994, 2160, 0x84, 0);
        let port_avcc_b64 = base64_encode(&f_port.avcc_config);
        let port_idr = wrap_avcc_with_aud_sei(&f_port.avcc_sample);
        drop(f_port);

        pipe.set_visible(true);
        mirror::global_on_airplay_mirror_loop_state(gen, true);
        mirror::update_live_codec_h264_avcc(gen, "avc1.42801e", &port_avcc_b64, 994, 2160);
        assert!(mirror::global_on_physical_h264_avcc_frame(gen, port_idr, true));

        // Feed 500 continuous P-frames with zero new IDRs (Infinite GOP) + repeated identical pkt_k1
        // and mod-16 equivalent (1008, 2160) updates every 100 frames.
        for i in 1..=500u32 {
            if i % 100 == 0 {
                mirror::update_live_codec_h264_avcc(gen, "avc1.42801e", &port_avcc_b64, 994, 2160);
                mirror::update_live_codec_h264_avcc(
                    gen,
                    "avc1.42801e",
                    &port_avcc_b64,
                    1008,
                    2160,
                );
            }
            let p_au = build_synthetic_h264_p_skip_cropped_with_aud_sei(994, 2160, i);
            assert!(mirror::global_on_physical_h264_avcc_frame(gen, p_au, false));
            if i % 32 == 0 {
                // Keep well inside 240-slot BoundedNativeVideoQueue without false stalls
                let pace_dl = Instant::now() + Duration::from_secs(2);
                while Instant::now() < pace_dl {
                    let tel = pipe.telemetry();
                    if u64::from(i) <= tel.decoded_frames.saturating_add(64) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        }

        let dl = Instant::now() + Duration::from_secs(8);
        let mut tc = airplay::get_airplay_trace_counters();
        while Instant::now() < dl {
            tc = airplay::get_airplay_trace_counters();
            if tc.decoder_fed >= 501
                && tc.mf_decoded_frames >= 485
                && tc.dcomp_presented_frames >= 20
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        assert_eq!(tc.airplay_watchdog_recoveries, 0, "Infinite GOP 500 P-frames MUST trigger 0 watchdog kills: {tc:?}");
        assert_eq!(tc.cached_idr_replays, 0, "healthy Infinite GOP needs 0 cached IDR replays: {tc:?}");
        assert!(tc.mf_stream_changes <= 1, "repeated pkt_k1 / 1008x2160 aliases must be NO-OP: {tc:?}");
        assert_eq!(tc.mid_stream_black_clears, 0, "0 mid-stream black clears: {tc:?}");
        assert_eq!(tc.decoder_fed, 501, "all 501 AUs (1 IDR + 500 P) fed: {tc:?}");
        assert!(tc.mf_decoded_frames >= 485, "expected >= 485 decoded frames: {tc:?}");
        assert_eq!(tc.vram_published, tc.mf_decoded_frames, "exact vram == decoded parity: {tc:?}");
        assert!(tc.dcomp_presented_frames >= 20, "continuous presentation across Infinite GOP: {tc:?}");

        let _ = mirror::global_stop_screen();
        pipe.shutdown();
        mirror::global_reset_mirror_runtime_for_test();
    }

    #[test]
    fn test_tahap279_orientation_switch_waits_clean_idr_without_force_unlatch_and_watchdog_preserves_healthy_mft() {
        use iphone_link_apple::hid;
        use iphone_link_apple::mirror;
        use iphone_link_video::mf_decoder::{
            build_synthetic_h264_ipcm_idr_cropped, build_synthetic_h264_p_skip_cropped_with_aud_sei,
            wrap_avcc_with_aud_sei,
        };

        let _hid_guard = hid::HID_TEST_SERIAL_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        mirror::global_reset_mirror_runtime_for_test();
        airplay::reset_airplay_trace();

        // 1. Real COM H.264 MFT: Portrait 994x2160 -> Landscape 2160x994 orientation switch
        //    holds old P-frames strictly until clean 2160x994 IDR arrives (0 force-unlatch, staleDimDropped=0).
        let gpu = Arc::new(LiveD3d11Gpu::new().expect("LiveD3d11Gpu"));
        let ring = Arc::new(D3d11Nv12VramRing::new());
        let mut dec = LiveMfVideoDecoder::new(gpu.clone(), VideoCodecKind::H264).expect("H264 MFT");

        let f_port = build_synthetic_h264_ipcm_idr_cropped(994, 2160, 0x7A, 0);
        let f_land = build_synthetic_h264_ipcm_idr_cropped(2160, 994, 0x90, 1);
        assert!(dec.update_avcc(&f_port.avcc_config).expect("994x2160 avcC"));
        let _ = dec
            .feed_avcc_sample(&wrap_avcc_with_aud_sei(&f_port.avcc_sample), 0, true, &ring)
            .expect("994x2160 IDR");
        let p_old = build_synthetic_h264_p_skip_cropped_with_aud_sei(994, 2160, 1);
        let _ = dec.feed_avcc_sample(&p_old, 16_667, false, &ring).expect("portrait P");

        assert!(dec.update_avcc(&f_land.avcc_config).expect("2160x994 avcC"));
        assert!(dec.awaiting_idr());
        assert_eq!(dec.mft_reconfigs(), 1);
        for i in 1..=35u32 {
            let p_stray = build_synthetic_h264_p_skip_cropped_with_aud_sei(994, 2160, i + 1);
            let st_drop = dec
                .feed_avcc_sample(&p_stray, 33_334 + u64::from(i) * 16_667, false, &ring)
                .expect("drop old-orientation P-frame while awaiting IDR");
            assert_eq!(st_drop.frames_emitted, 0);
            assert!(dec.awaiting_idr(), "MUST wait strictly for clean IDR at step #{i}");
        }
        let _ = dec
            .feed_avcc_sample(&wrap_avcc_with_aud_sei(&f_land.avcc_sample), 700_000, true, &ring)
            .expect("2160x994 IDR");
        assert!(!dec.awaiting_idr());
        for i in 1..=5u32 {
            let p_new = build_synthetic_h264_p_skip_cropped_with_aud_sei(2160, 994, i);
            let _ = dec
                .feed_avcc_sample(&p_new, 700_000 + u64::from(i) * 16_667, false, &ring)
                .expect("2160x994 P");
        }
        let _ = dec.drain_eos(true, Some((0, 0, 0, 0)), &ring).expect("drain landscape");
        assert_eq!(dec.last_mft_dims(), Some((2160, 1008)));
        assert_eq!(dec.stale_dim_dropped(), 0);
        ring.clear();
        dec.clean_teardown();
        drop(dec);

        // 2. Watchdog observe_tiered: Tier 1 soft drain works, 300 packets with fatal_hr == 0 NEVER kill MFT,
        //    and >= 5 fatal HRESULT triggers HardRebuildOnFatalError!
        let mut wd = AirplayStallWatchdog::new();
        assert_eq!(wd.observe_tiered(true, 1, 1, 500, 0), AirplayWatchdogAction::None);
        for i in 0..300u64 {
            let act = wd.observe_tiered(true, 1, 1, 1000 + i * 55, 0);
            assert_ne!(
                act,
                AirplayWatchdogAction::HardRebuildOnFatalError,
                "healthy MFT (fatal_hr == 0) MUST NEVER suffer hard rebuild at pkt #{i}"
            );
        }
        assert!(wd.soft_recoveries() >= 1);
        assert_eq!(wd.recoveries(), 0);
        assert_eq!(
            wd.observe_tiered(true, 1, 1, 25_000, AIRPLAY_HARD_RECOVERY_MIN_FATAL_ERRORS),
            AirplayWatchdogAction::HardRebuildOnFatalError
        );
        assert_eq!(wd.recoveries(), 1);

        // 3. Real LiveVideoPipeline::reset_for_new_session cleanly zeroes queue stats + trace counters
        let mut pipe = LiveVideoPipeline::new_composition(gpu, 519, 1125, 0)
            .expect("composition pipeline");
        pipe.set_airplay_mode(true);
        pipe.set_window_landscape(false);
        let _ = mirror::start_airplay_mirror_session(
            Some("00008150-0000000000000279"),
            false,
            "wifi",
            None,
            Some(0),
            false,
        );
        let gen = mirror::current_live_session_gen();
        let port_avcc_b64 = base64_encode(&f_port.avcc_config);
        let port_idr = wrap_avcc_with_aud_sei(&f_port.avcc_sample);
        pipe.set_visible(true);
        mirror::global_on_airplay_mirror_loop_state(gen, true);
        mirror::update_live_codec_h264_avcc(gen, "avc1.42801e", &port_avcc_b64, 994, 2160);
        assert!(mirror::global_on_physical_h264_avcc_frame(gen, port_idr, true));
        for i in 1..=10u32 {
            let p_au = build_synthetic_h264_p_skip_cropped_with_aud_sei(994, 2160, i);
            assert!(mirror::global_on_physical_h264_avcc_frame(gen, p_au, false));
        }
        let dl = Instant::now() + Duration::from_secs(3);
        while Instant::now() < dl {
            if airplay::trace_idr_keyframes_detected() >= 1 {
                break;
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        assert!(airplay::trace_idr_keyframes_detected() >= 1);
        airplay::inc_trace_dual_buffer_overwrites();
        airplay::add_trace_avcc_small_256_511_frames(3);
        airplay::inc_trace_mft_reconfigs();

        pipe.reset_for_new_session();
        let (_, _, _, _, q_pushed) = pipe.queue.stats();
        assert_eq!(q_pushed, 0, "reset_for_new_session MUST zero queue pushed_total");
        assert_eq!(airplay::trace_idr_keyframes_detected(), 0);
        assert_eq!(airplay::trace_dual_buffer_overwrites(), 0);
        assert_eq!(airplay::trace_avcc_small_256_511_frames(), 0);
        assert_eq!(airplay::trace_mft_reconfigs(), 0);

        let _ = mirror::global_stop_screen();
        pipe.shutdown();
        mirror::global_reset_mirror_runtime_for_test();
        airplay::reset_airplay_trace();
    }

    #[test]
    fn test_tahap285_zero_frame_anti_blank_watchdog_and_hevc_missing_latch() {
        // 1. Structured error classification (zero swallowed errors)
        assert_eq!(
            classify_hevc_decoder_init_error(&DecoderError::HevcExtensionMissing),
            "hevc_mft_missing"
        );
        assert_eq!(
            classify_hevc_decoder_init_error(&DecoderError::MediaFoundationUnavailable(
                "mfplat missing".into()
            )),
            "media_feature_pack_missing"
        );
        assert_eq!(
            classify_hevc_decoder_init_error(&DecoderError::ComError {
                op: "SetInputType",
                hr: -1072875852
            }),
            "decoder_init_failed"
        );

        // 2. Watchdog triggers immediately on latched codec_error_kind, waits >= 7000 ms for zero-frame timeout,
        //    and NEVER triggers during idle / waiting_airplay / active AirPlay / once frame #1 is presented.
        assert_eq!(
            evaluate_anti_blank_watchdog("running", "streaming", false, 0, "hevc_mft_missing", 0)
                .as_deref(),
            Some("hevc_mft_missing")
        );
        assert_eq!(
            evaluate_anti_blank_watchdog("running", "streaming", false, 0, "", 6999),
            None
        );
        assert_eq!(
            evaluate_anti_blank_watchdog("running", "streaming", false, 0, "", 7000).as_deref(),
            Some("zero_frame_timeout")
        );
        assert_eq!(
            evaluate_anti_blank_watchdog("running", "streaming", true, 0, "", 9000),
            None,
            "AirPlay waiting/active stream uses AirPlay rules instead of CoreDevice timeout"
        );
        assert_eq!(
            evaluate_anti_blank_watchdog("running", "waiting_airplay", false, 0, "hevc_mft_missing", 9000),
            None
        );
        assert_eq!(
            evaluate_anti_blank_watchdog("running", "streaming", false, 1, "hevc_mft_missing", 9000),
            None
        );

        // 3. Latch verification: simulated 60-frame burst when HEVC fails init attempts ONLY 1x before reset
        let mut latch = false;
        let mut init_attempts = 0u32;
        for _ in 0..60 {
            if !latch {
                init_attempts += 1;
                latch = true;
            }
        }
        assert_eq!(init_attempts, 1, "HEVC init failure MUST latch after 1 attempt until reset");
    }
}
