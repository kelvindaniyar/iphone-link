//! Native Pure-Rust AirPlay H.264 Receiver Engine (`_airplay._tcp` / `_raop._tcp`) for:
//! - **Hybrid Mode (`iOS 17.4 – 26.x`)**: AirPlay H.264 (`avc1`) video + CoreDevice `gate_only` HID touch/keys
//! - **AirPlay Standalone (`iOS <= 17.3` & PC fallback without HEVC)**: H.264 video without CoreDevice/DDI
//!
//! Zero external crates (`LoadLibraryW` FFI to `playfair.dll` + Windows CNG `Aes128Ctr`/`SHA-512` +
//! pure-Rust `X25519`/`Ed25519` + pure-Rust mDNS responder + pure-std RTSP/1.0 server).

pub mod audio;
pub mod fairplay;
pub mod identity;
pub mod info;
pub mod mdns;
pub mod mirror;
pub mod pair;
pub mod rtsp;
pub mod server;

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::win_cng::sha512_chunks;

pub use audio::{
    apply_live_audio_config, compute_airplay_audio_targets, get_airplay_audio_buffer_preset,
    get_airplay_audio_snapshot, get_airplay_audio_volume, is_airplay_audio_lipsync_enabled,
    is_airplay_audio_muted, mark_airplay_audio_inactive, reset_airplay_audio_stats,
    set_airplay_audio_muted, AirPlayAudioJitterQueue, AirPlayAudioSnapshot, SmoothGainRamp,
};
pub use identity::{AirPlayIdentity, DEFAULT_RECEIVER_NAME};
pub use info::DEFAULT_PORT;
pub use server::AirPlayServer;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AirPlayState {
    Waiting,
    Connected,
    Streaming,
    Stopped,
}

/// Sink receiving decoded AirPlay H.264 `avcC` codec headers, decrypted AVCC frames, and state events.
pub trait AirPlaySink: Send + Sync + 'static {
    fn on_state(&self, state: AirPlayState);
    fn on_codec(&self, codec: &str, avcc_b64: &str, width: u32, height: u32);
    fn on_frame(&self, avcc_payload: Vec<u8>, is_keyframe: bool);
    fn on_decrypt_failure(&self) {}
    fn on_tcp_packet(&self, _kind: u8) {}
    fn on_mirror_loop_state(&self, _alive: bool) {}
}

/// Win64 `WSAPOLLFD` layout (`sizeof == 16` bytes: `usize fd` + `i16 events` + `i16 revents` + 4B pad).
#[cfg(windows)]
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WsaPollFd {
    pub fd: usize,
    pub events: i16,
    pub revents: i16,
}

#[cfg(windows)]
pub const POLLRDNORM: i16 = 0x0100;
#[cfg(windows)]
pub const POLLERR: i16 = 0x0001;
#[cfg(windows)]
pub const POLLHUP: i16 = 0x0002;
#[cfg(windows)]
pub const POLLNVAL: i16 = 0x0004;

/// Waits up to `timeout_ms` for `stream` to become readable using native Win32 `WSAPoll`
/// on a non-blocking TCP socket.
///
/// Tahap 255 Hardening: Immune to `SO_RCVTIMEO` IRP cancellations, immune to `FD_SETSIZE = 64`,
/// immune to Win64 `fd_set` alignment quirks, wakes instantly when bytes arrive, and uses
/// 0% idle CPU while honoring `stop` within `<= 50 ms`.
#[cfg(windows)]
pub(crate) fn wait_tcp_readable(stream: &std::net::TcpStream, timeout_ms: u64) -> bool {
    use std::os::windows::io::AsRawSocket;

    #[link(name = "ws2_32")]
    extern "system" {
        fn WSAPoll(fdarray: *mut WsaPollFd, fds: u32, timeout: i32) -> i32;
    }

    let mut pfd = WsaPollFd {
        fd: stream.as_raw_socket() as usize,
        events: POLLRDNORM,
        revents: 0,
    };
    let rc = unsafe { WSAPoll(&mut pfd, 1, timeout_ms.min(i32::MAX as u64) as i32) };
    rc > 0 && (pfd.revents & (POLLRDNORM | POLLERR | POLLHUP | POLLNVAL)) != 0
}

#[cfg(not(windows))]
pub(crate) fn wait_tcp_readable(_stream: &std::net::TcpStream, timeout_ms: u64) -> bool {
    std::thread::sleep(std::time::Duration::from_millis(timeout_ms.min(10)));
    true
}

/// Waits up to `timeout_ms` for a non-blocking `UdpSocket` to become readable using Win32 `WSAPoll`.
#[cfg(windows)]
pub(crate) fn wait_udp_readable(sock: &std::net::UdpSocket, timeout_ms: u64) -> bool {
    use std::os::windows::io::AsRawSocket;

    #[link(name = "ws2_32")]
    extern "system" {
        fn WSAPoll(fdarray: *mut WsaPollFd, fds: u32, timeout: i32) -> i32;
    }

    let mut pfd = WsaPollFd {
        fd: sock.as_raw_socket() as usize,
        events: POLLRDNORM,
        revents: 0,
    };
    let rc = unsafe { WSAPoll(&mut pfd, 1, timeout_ms.min(i32::MAX as u64) as i32) };
    rc > 0 && (pfd.revents & (POLLRDNORM | POLLERR | POLLHUP | POLLNVAL)) != 0
}

#[cfg(not(windows))]
pub(crate) fn wait_udp_readable(_sock: &std::net::UdpSocket, timeout_ms: u64) -> bool {
    std::thread::sleep(std::time::Duration::from_millis(timeout_ms.min(10)));
    true
}

/// Waits up to `timeout_ms` for EITHER of two non-blocking `UdpSocket`s (e.g. AirPlay audio
/// `dataPort` + `controlPort`) to become readable in a single Win32 `WSAPoll` call (`< 1 ms` wake,
/// 0% idle CPU, zero 20ms latency stall on RTCP Sync `0xD4` when audio data is silent!).
#[cfg(windows)]
pub(crate) fn wait_two_udps_readable(
    sock_a: &std::net::UdpSocket,
    sock_b: &std::net::UdpSocket,
    timeout_ms: u64,
) -> bool {
    use std::os::windows::io::AsRawSocket;

    #[link(name = "ws2_32")]
    extern "system" {
        fn WSAPoll(fdarray: *mut WsaPollFd, fds: u32, timeout: i32) -> i32;
    }

    let mut pfds = [
        WsaPollFd {
            fd: sock_a.as_raw_socket() as usize,
            events: POLLRDNORM,
            revents: 0,
        },
        WsaPollFd {
            fd: sock_b.as_raw_socket() as usize,
            events: POLLRDNORM,
            revents: 0,
        },
    ];
    let rc = unsafe { WSAPoll(pfds.as_mut_ptr(), 2, timeout_ms.min(i32::MAX as u64) as i32) };
    rc > 0
        && pfds
            .iter()
            .any(|p| (p.revents & (POLLRDNORM | POLLERR | POLLHUP | POLLNVAL)) != 0)
}

#[cfg(not(windows))]
pub(crate) fn wait_two_udps_readable(
    _sock_a: &std::net::UdpSocket,
    _sock_b: &std::net::UdpSocket,
    timeout_ms: u64,
) -> bool {
    std::thread::sleep(std::time::Duration::from_millis(timeout_ms.min(10)));
    true
}

/// Disables WinSock2 `SIO_UDP_CONNRESET` (`0x9800_000C`, `WSAECONNRESET = 10054`) so an ICMP Port
/// Unreachable on any AirPlay UDP socket (`timingPort`, audio `dataPort`, audio `controlPort`)
/// NEVER poisons or aborts subsequent `recv_from()` calls on Windows!
#[cfg(windows)]
pub(crate) fn disable_udp_connreset(sock: &std::net::UdpSocket) -> bool {
    use std::os::windows::io::AsRawSocket;

    const SIO_UDP_CONNRESET: u32 = 0x9800_000C;

    #[link(name = "ws2_32")]
    extern "system" {
        fn WSAIoctl(
            s: usize,
            dw_io_control_code: u32,
            lpv_in_buffer: *const std::ffi::c_void,
            cb_in_buffer: u32,
            lpv_out_buffer: *mut std::ffi::c_void,
            cb_out_buffer: u32,
            lpcb_bytes_returned: *mut u32,
            lp_overlapped: *mut std::ffi::c_void,
            lp_completion_routine: *mut std::ffi::c_void,
        ) -> i32;
    }

    let enable: i32 = 0; // FALSE -> disable WSAECONNRESET reporting on UDP
    let mut bytes_returned: u32 = 0;
    let rc = unsafe {
        WSAIoctl(
            sock.as_raw_socket() as usize,
            SIO_UDP_CONNRESET,
            (&enable as *const i32).cast::<std::ffi::c_void>(),
            std::mem::size_of::<i32>() as u32,
            std::ptr::null_mut(),
            0,
            &mut bytes_returned,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    rc == 0
}

#[cfg(not(windows))]
pub(crate) fn disable_udp_connreset(_sock: &std::net::UdpSocket) -> bool {
    true
}

// ============================================================================
// Tahap 255 Pilar 3: In-Memory PII-Free AirPlay Flight Recorder (80 entries)
// Zero disk files, zero IP/UDID/device-name/PIN strings!
// ============================================================================

const MAX_FLIGHT_RECORDER_EVENTS: usize = 80;

#[derive(Clone, Debug)]
struct AirPlayTraceEvent {
    t_ms: u64,
    event: String,
    detail: String,
}

/// Tahap 257/258/259 Pilar 3: Strict PII-free numeric protocol counters exposed alongside `events[]`
/// in `GET /api/debug/airplay-trace` (100% in-memory, 0 disk files, 0 IPs/names/UDIDs).
#[derive(Clone, Debug)]
pub struct AirPlayTraceCounters {
    pub ntp_tx: u64,
    pub ntp_rx: u64,
    pub ntp_last_err: Option<String>,
    pub event_accepts: u64,
    pub event_rx: u64,
    pub audio_data_rx: u64,
    pub audio_ctrl_rx: u64,
    pub audio_decoded: u64,
    pub audio_played_samples: u64,
    pub audio_decode_errors: u64,
    pub audio_dropped_packets: u64,
    pub video_queue_dropped: u64,
    pub tcp_video_packets: u64,
    pub avcc_updates: u64,
    pub queue_dropped: u64,
    pub native_video_sink_packets: u64,
    pub decoder_fed: u64,
    pub mf_decoded_frames: u64,
    pub mf_stream_changes: u64,
    pub mf_not_accepting_drains: u64,
    pub stale_dim_dropped: u64,
    pub vram_published: u64,
    pub dcomp_presented_frames: u64,
    pub present_calls: u64,
    pub present_errors: u64,
    pub last_decoder_hresult: i32,
    pub last_mft_dims: String,
    pub expected_coded_dims: String,
    pub audio_codec: String,
    pub feedback_count: u64,
    pub teardown_count: u64,
    pub airplay_watchdog_recoveries: u64,
    pub mid_stream_black_clears: u64,
    pub dual_buffer_overwrites: u64,
    pub ctr_resync_count: u64,
    pub ctr_decrypt_rejects: u64,
    pub ctr_keystream_bytes: u64,
    pub tcp_disconnects: u64,
    pub layout_rect_clamps: u64,
    pub mft_dim_changes: u64,
    pub last_active_video_rect: String,
    pub mf_low_latency_enabled: bool,
    pub dcomp_commits: u64,
    pub d3d11_multithread_protected: bool,
    pub dpb_preserved_flushes_skipped: u64,
    pub idr_keyframes_detected: u64,
    pub fullscreen_landscape_resyncs: u64,
    pub pre_decoder_dropped_awaiting_avcc: u64,
    pub pre_decoder_dropped_awaiting_idr: u64,
    pub pre_decoder_dropped_stale_epoch: u64,
    pub cached_idr_replays: u64,
    pub last_presented_dest_rect: String,
    pub last_presented_rotation_deg: u32,
    pub avcc_small_256_511_frames: u64,
    pub mft_reconfigs: u64,
    pub hd_shader_active: bool,
    pub hd_shader_presents: u64,
}

impl Default for AirPlayTraceCounters {
    fn default() -> Self {
        Self {
            ntp_tx: 0,
            ntp_rx: 0,
            ntp_last_err: None,
            event_accepts: 0,
            event_rx: 0,
            audio_data_rx: 0,
            audio_ctrl_rx: 0,
            audio_decoded: 0,
            audio_played_samples: 0,
            audio_decode_errors: 0,
            audio_dropped_packets: 0,
            video_queue_dropped: 0,
            tcp_video_packets: 0,
            avcc_updates: 0,
            queue_dropped: 0,
            native_video_sink_packets: 0,
            decoder_fed: 0,
            mf_decoded_frames: 0,
            mf_stream_changes: 0,
            mf_not_accepting_drains: 0,
            stale_dim_dropped: 0,
            vram_published: 0,
            dcomp_presented_frames: 0,
            present_calls: 0,
            present_errors: 0,
            last_decoder_hresult: 0,
            last_mft_dims: "0x0".to_string(),
            expected_coded_dims: "0x0".to_string(),
            audio_codec: "none".to_string(),
            feedback_count: 0,
            teardown_count: 0,
            airplay_watchdog_recoveries: 0,
            mid_stream_black_clears: 0,
            dual_buffer_overwrites: 0,
            ctr_resync_count: 0,
            ctr_decrypt_rejects: 0,
            ctr_keystream_bytes: 0,
            tcp_disconnects: 0,
            layout_rect_clamps: 0,
            mft_dim_changes: 0,
            last_active_video_rect: String::from("0,0,0,0"),
            mf_low_latency_enabled: false,
            dcomp_commits: 0,
            d3d11_multithread_protected: false,
            dpb_preserved_flushes_skipped: 0,
            idr_keyframes_detected: 0,
            fullscreen_landscape_resyncs: 0,
            pre_decoder_dropped_awaiting_avcc: 0,
            pre_decoder_dropped_awaiting_idr: 0,
            pre_decoder_dropped_stale_epoch: 0,
            cached_idr_replays: 0,
            last_presented_dest_rect: String::from("0,0,0,0"),
            last_presented_rotation_deg: 0,
            avcc_small_256_511_frames: 0,
            mft_reconfigs: 0,
            hd_shader_active: false,
            hd_shader_presents: 0,
        }
    }
}

static TRACE_WATCHDOG_RECOVERIES: AtomicU64 = AtomicU64::new(0);
static TRACE_TCP_VIDEO_PACKETS: AtomicU64 = AtomicU64::new(0);
static TRACE_AVCC_UPDATES: AtomicU64 = AtomicU64::new(0);
static TRACE_MID_STREAM_BLACK_CLEARS: AtomicU64 = AtomicU64::new(0);
static TRACE_DUAL_BUFFER_OVERWRITES: AtomicU64 = AtomicU64::new(0);
static TRACE_CTR_RESYNC_COUNT: AtomicU64 = AtomicU64::new(0);
static TRACE_CTR_DECRYPT_REJECTS: AtomicU64 = AtomicU64::new(0);
static TRACE_CTR_KEYSTREAM_BYTES: AtomicU64 = AtomicU64::new(0);
static TRACE_TCP_DISCONNECTS: AtomicU64 = AtomicU64::new(0);
static TRACE_LAYOUT_RECT_CLAMPS: AtomicU64 = AtomicU64::new(0);
static TRACE_MFT_DIM_CHANGES: AtomicU64 = AtomicU64::new(0);
static TRACE_LAST_ACTIVE_VIDEO_RECT: OnceLock<Mutex<String>> = OnceLock::new();
static TRACE_MF_LOW_LATENCY_ENABLED: AtomicBool = AtomicBool::new(false);
static TRACE_DCOMP_COMMITS: AtomicU64 = AtomicU64::new(0);
static TRACE_D3D11_MULTITHREAD_PROTECTED: AtomicBool = AtomicBool::new(false);
static TRACE_DPB_PRESERVED_FLUSHES_SKIPPED: AtomicU64 = AtomicU64::new(0);
static TRACE_IDR_KEYFRAMES_DETECTED: AtomicU64 = AtomicU64::new(0);
static TRACE_FULLSCREEN_LANDSCAPE_RESYNCS: AtomicU64 = AtomicU64::new(0);
static TRACE_PRE_DECODER_DROPPED_AWAITING_AVCC: AtomicU64 = AtomicU64::new(0);
static TRACE_PRE_DECODER_DROPPED_AWAITING_IDR: AtomicU64 = AtomicU64::new(0);
static TRACE_PRE_DECODER_DROPPED_STALE_EPOCH: AtomicU64 = AtomicU64::new(0);
static TRACE_CACHED_IDR_REPLAYS: AtomicU64 = AtomicU64::new(0);
static TRACE_LAST_PRESENTED_DEST_RECT: OnceLock<Mutex<String>> = OnceLock::new();
static TRACE_LAST_PRESENTED_ROTATION_DEG: AtomicU32 = AtomicU32::new(0);
static TRACE_AVCC_SMALL_256_511_FRAMES: AtomicU64 = AtomicU64::new(0);
static TRACE_MFT_RECONFIGS: AtomicU64 = AtomicU64::new(0);
static TRACE_HD_SHADER_ACTIVE: AtomicBool = AtomicBool::new(false);
static TRACE_HD_SHADER_PRESENTS: AtomicU64 = AtomicU64::new(0);

pub fn inc_trace_watchdog_recoveries() {
    TRACE_WATCHDOG_RECOVERIES.fetch_add(1, Ordering::Relaxed);
}

pub fn trace_watchdog_recoveries() -> u64 {
    TRACE_WATCHDOG_RECOVERIES.load(Ordering::Relaxed)
}

pub fn inc_trace_tcp_video_packets() {
    TRACE_TCP_VIDEO_PACKETS.fetch_add(1, Ordering::Relaxed);
}

pub fn inc_trace_avcc_updates() {
    TRACE_AVCC_UPDATES.fetch_add(1, Ordering::Relaxed);
}

pub fn set_trace_mid_stream_black_clears(v: u64) {
    TRACE_MID_STREAM_BLACK_CLEARS.store(v, Ordering::Relaxed);
}

pub fn trace_mid_stream_black_clears() -> u64 {
    TRACE_MID_STREAM_BLACK_CLEARS.load(Ordering::Relaxed)
}

pub fn set_trace_dual_buffer_overwrites(v: u64) {
    TRACE_DUAL_BUFFER_OVERWRITES.store(v, Ordering::Relaxed);
}

pub fn inc_trace_dual_buffer_overwrites() {
    TRACE_DUAL_BUFFER_OVERWRITES.fetch_add(1, Ordering::Relaxed);
}

pub fn trace_dual_buffer_overwrites() -> u64 {
    TRACE_DUAL_BUFFER_OVERWRITES.load(Ordering::Relaxed)
}

pub fn inc_trace_ctr_resync_count() {
    TRACE_CTR_RESYNC_COUNT.fetch_add(1, Ordering::Relaxed);
}

pub fn trace_ctr_resync_count() -> u64 {
    TRACE_CTR_RESYNC_COUNT.load(Ordering::Relaxed)
}

pub fn inc_trace_ctr_decrypt_rejects() {
    TRACE_CTR_DECRYPT_REJECTS.fetch_add(1, Ordering::Relaxed);
}

pub fn trace_ctr_decrypt_rejects() -> u64 {
    TRACE_CTR_DECRYPT_REJECTS.load(Ordering::Relaxed)
}

pub fn add_trace_ctr_keystream_bytes(bytes: u64) {
    TRACE_CTR_KEYSTREAM_BYTES.fetch_add(bytes, Ordering::Relaxed);
}

pub fn trace_ctr_keystream_bytes() -> u64 {
    TRACE_CTR_KEYSTREAM_BYTES.load(Ordering::Relaxed)
}

pub fn inc_trace_tcp_disconnects() {
    TRACE_TCP_DISCONNECTS.fetch_add(1, Ordering::Relaxed);
}

pub fn trace_tcp_disconnects() -> u64 {
    TRACE_TCP_DISCONNECTS.load(Ordering::Relaxed)
}

pub fn inc_trace_layout_rect_clamps() {
    TRACE_LAYOUT_RECT_CLAMPS.fetch_add(1, Ordering::Relaxed);
}

pub fn trace_layout_rect_clamps() -> u64 {
    TRACE_LAYOUT_RECT_CLAMPS.load(Ordering::Relaxed)
}

pub fn inc_trace_mft_dim_changes() {
    TRACE_MFT_DIM_CHANGES.fetch_add(1, Ordering::Relaxed);
}

pub fn trace_mft_dim_changes() -> u64 {
    TRACE_MFT_DIM_CHANGES.load(Ordering::Relaxed)
}

fn last_active_video_rect_lock() -> &'static Mutex<String> {
    TRACE_LAST_ACTIVE_VIDEO_RECT.get_or_init(|| Mutex::new(String::from("0,0,0,0")))
}

pub fn set_trace_last_active_video_rect(x: i32, y: i32, w: i32, h: i32) {
    if let Ok(mut g) = last_active_video_rect_lock().lock() {
        *g = format!("{x},{y},{w},{h}");
    }
}

pub fn trace_last_active_video_rect() -> String {
    last_active_video_rect_lock()
        .lock()
        .map(|g| g.clone())
        .unwrap_or_else(|_| String::from("0,0,0,0"))
}

pub fn set_trace_mf_low_latency_enabled(enabled: bool) {
    TRACE_MF_LOW_LATENCY_ENABLED.store(enabled, Ordering::Relaxed);
}

pub fn trace_mf_low_latency_enabled() -> bool {
    TRACE_MF_LOW_LATENCY_ENABLED.load(Ordering::Relaxed)
}

pub fn inc_trace_dcomp_commits() {
    TRACE_DCOMP_COMMITS.fetch_add(1, Ordering::Relaxed);
}

pub fn trace_dcomp_commits() -> u64 {
    TRACE_DCOMP_COMMITS.load(Ordering::Relaxed)
}

pub fn set_trace_d3d11_multithread_protected(enabled: bool) {
    TRACE_D3D11_MULTITHREAD_PROTECTED.store(enabled, Ordering::Relaxed);
}

pub fn trace_d3d11_multithread_protected() -> bool {
    TRACE_D3D11_MULTITHREAD_PROTECTED.load(Ordering::Relaxed)
}

pub fn set_trace_dpb_preserved_flushes_skipped(v: u64) {
    TRACE_DPB_PRESERVED_FLUSHES_SKIPPED.store(v, Ordering::Relaxed);
}

pub fn inc_trace_dpb_preserved_flushes_skipped() {
    TRACE_DPB_PRESERVED_FLUSHES_SKIPPED.fetch_add(1, Ordering::Relaxed);
}

pub fn trace_dpb_preserved_flushes_skipped() -> u64 {
    TRACE_DPB_PRESERVED_FLUSHES_SKIPPED.load(Ordering::Relaxed)
}

pub fn set_trace_idr_keyframes_detected(v: u64) {
    TRACE_IDR_KEYFRAMES_DETECTED.store(v, Ordering::Relaxed);
}

pub fn inc_trace_idr_keyframes_detected() {
    TRACE_IDR_KEYFRAMES_DETECTED.fetch_add(1, Ordering::Relaxed);
}

pub fn trace_idr_keyframes_detected() -> u64 {
    TRACE_IDR_KEYFRAMES_DETECTED.load(Ordering::Relaxed)
}

pub fn inc_trace_fullscreen_landscape_resyncs() {
    TRACE_FULLSCREEN_LANDSCAPE_RESYNCS.fetch_add(1, Ordering::Relaxed);
}

pub fn trace_fullscreen_landscape_resyncs() -> u64 {
    TRACE_FULLSCREEN_LANDSCAPE_RESYNCS.load(Ordering::Relaxed)
}

pub fn inc_trace_pre_decoder_dropped_awaiting_avcc() {
    TRACE_PRE_DECODER_DROPPED_AWAITING_AVCC.fetch_add(1, Ordering::Relaxed);
}

pub fn trace_pre_decoder_dropped_awaiting_avcc() -> u64 {
    TRACE_PRE_DECODER_DROPPED_AWAITING_AVCC.load(Ordering::Relaxed)
}

pub fn inc_trace_pre_decoder_dropped_awaiting_idr() {
    TRACE_PRE_DECODER_DROPPED_AWAITING_IDR.fetch_add(1, Ordering::Relaxed);
}

pub fn trace_pre_decoder_dropped_awaiting_idr() -> u64 {
    TRACE_PRE_DECODER_DROPPED_AWAITING_IDR.load(Ordering::Relaxed)
}

pub fn inc_trace_pre_decoder_dropped_stale_epoch() {
    TRACE_PRE_DECODER_DROPPED_STALE_EPOCH.fetch_add(1, Ordering::Relaxed);
}

pub fn trace_pre_decoder_dropped_stale_epoch() -> u64 {
    TRACE_PRE_DECODER_DROPPED_STALE_EPOCH.load(Ordering::Relaxed)
}

pub fn inc_trace_cached_idr_replays() {
    TRACE_CACHED_IDR_REPLAYS.fetch_add(1, Ordering::Relaxed);
}

pub fn trace_cached_idr_replays() -> u64 {
    TRACE_CACHED_IDR_REPLAYS.load(Ordering::Relaxed)
}

fn last_presented_dest_rect_lock() -> &'static Mutex<String> {
    TRACE_LAST_PRESENTED_DEST_RECT.get_or_init(|| Mutex::new(String::from("0,0,0,0")))
}

pub fn set_trace_last_presented_dest_rect(x: i32, y: i32, w: i32, h: i32) {
    if let Ok(mut g) = last_presented_dest_rect_lock().lock() {
        *g = format!("{x},{y},{w},{h}");
    }
}

pub fn trace_last_presented_dest_rect() -> String {
    last_presented_dest_rect_lock()
        .lock()
        .map(|g| g.clone())
        .unwrap_or_else(|_| String::from("0,0,0,0"))
}

pub fn set_trace_last_presented_rotation_deg(deg: u32) {
    TRACE_LAST_PRESENTED_ROTATION_DEG.store(deg % 360, Ordering::Relaxed);
}

pub fn trace_last_presented_rotation_deg() -> u32 {
    TRACE_LAST_PRESENTED_ROTATION_DEG.load(Ordering::Relaxed)
}

pub fn add_trace_avcc_small_256_511_frames(n: u64) {
    if n > 0 {
        TRACE_AVCC_SMALL_256_511_FRAMES.fetch_add(n, Ordering::Relaxed);
    }
}

pub fn set_trace_avcc_small_256_511_frames(v: u64) {
    TRACE_AVCC_SMALL_256_511_FRAMES.store(v, Ordering::Relaxed);
}

pub fn trace_avcc_small_256_511_frames() -> u64 {
    TRACE_AVCC_SMALL_256_511_FRAMES.load(Ordering::Relaxed)
}

pub fn inc_trace_mft_reconfigs() {
    TRACE_MFT_RECONFIGS.fetch_add(1, Ordering::Relaxed);
}

pub fn add_trace_mft_reconfigs(n: u64) {
    if n > 0 {
        TRACE_MFT_RECONFIGS.fetch_add(n, Ordering::Relaxed);
    }
}

pub fn set_trace_mft_reconfigs(v: u64) {
    TRACE_MFT_RECONFIGS.store(v, Ordering::Relaxed);
}

pub fn trace_mft_reconfigs() -> u64 {
    TRACE_MFT_RECONFIGS.load(Ordering::Relaxed)
}

pub fn set_trace_hd_shader_active(active: bool) {
    TRACE_HD_SHADER_ACTIVE.store(active, Ordering::Relaxed);
}

pub fn trace_hd_shader_active() -> bool {
    TRACE_HD_SHADER_ACTIVE.load(Ordering::Relaxed)
        || iphone_link_video::video_hwnd::hd_shader_trace_active()
}

pub fn add_trace_hd_shader_presents(n: u64) {
    if n > 0 {
        TRACE_HD_SHADER_PRESENTS.fetch_add(n, Ordering::Relaxed);
    }
}

pub fn set_trace_hd_shader_presents(v: u64) {
    TRACE_HD_SHADER_PRESENTS.store(v, Ordering::Relaxed);
}

pub fn trace_hd_shader_presents() -> u64 {
    TRACE_HD_SHADER_PRESENTS
        .load(Ordering::Relaxed)
        .max(iphone_link_video::video_hwnd::hd_shader_trace_presents())
}

struct AirPlayFlightRecorder {
    started_at: std::time::Instant,
    events: std::collections::VecDeque<AirPlayTraceEvent>,
    counters: AirPlayTraceCounters,
}

static AIRPLAY_FLIGHT_RECORDER: OnceLock<Mutex<AirPlayFlightRecorder>> = OnceLock::new();

fn flight_recorder() -> &'static Mutex<AirPlayFlightRecorder> {
    AIRPLAY_FLIGHT_RECORDER.get_or_init(|| {
        Mutex::new(AirPlayFlightRecorder {
            started_at: std::time::Instant::now(),
            events: std::collections::VecDeque::with_capacity(MAX_FLIGHT_RECORDER_EVENTS),
            counters: AirPlayTraceCounters::default(),
        })
    })
}

fn escape_trace_json(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {}
            c => out.push(c),
        }
    }
    out
}

/// Resets the in-memory AirPlay Flight Recorder (events + counters) for a fresh session.
pub fn reset_airplay_trace() {
    TRACE_WATCHDOG_RECOVERIES.store(0, Ordering::Relaxed);
    TRACE_TCP_VIDEO_PACKETS.store(0, Ordering::Relaxed);
    TRACE_AVCC_UPDATES.store(0, Ordering::Relaxed);
    TRACE_MID_STREAM_BLACK_CLEARS.store(0, Ordering::Relaxed);
    TRACE_DUAL_BUFFER_OVERWRITES.store(0, Ordering::Relaxed);
    TRACE_CTR_RESYNC_COUNT.store(0, Ordering::Relaxed);
    TRACE_CTR_DECRYPT_REJECTS.store(0, Ordering::Relaxed);
    TRACE_CTR_KEYSTREAM_BYTES.store(0, Ordering::Relaxed);
    TRACE_TCP_DISCONNECTS.store(0, Ordering::Relaxed);
    TRACE_LAYOUT_RECT_CLAMPS.store(0, Ordering::Relaxed);
    TRACE_MFT_DIM_CHANGES.store(0, Ordering::Relaxed);
    TRACE_MF_LOW_LATENCY_ENABLED.store(false, Ordering::Relaxed);
    TRACE_DCOMP_COMMITS.store(0, Ordering::Relaxed);
    TRACE_D3D11_MULTITHREAD_PROTECTED.store(false, Ordering::Relaxed);
    TRACE_DPB_PRESERVED_FLUSHES_SKIPPED.store(0, Ordering::Relaxed);
    TRACE_IDR_KEYFRAMES_DETECTED.store(0, Ordering::Relaxed);
    TRACE_FULLSCREEN_LANDSCAPE_RESYNCS.store(0, Ordering::Relaxed);
    TRACE_PRE_DECODER_DROPPED_AWAITING_AVCC.store(0, Ordering::Relaxed);
    TRACE_PRE_DECODER_DROPPED_AWAITING_IDR.store(0, Ordering::Relaxed);
    TRACE_PRE_DECODER_DROPPED_STALE_EPOCH.store(0, Ordering::Relaxed);
    TRACE_CACHED_IDR_REPLAYS.store(0, Ordering::Relaxed);
    TRACE_LAST_PRESENTED_ROTATION_DEG.store(0, Ordering::Relaxed);
    TRACE_AVCC_SMALL_256_511_FRAMES.store(0, Ordering::Relaxed);
    TRACE_MFT_RECONFIGS.store(0, Ordering::Relaxed);
    TRACE_HD_SHADER_ACTIVE.store(false, Ordering::Relaxed);
    TRACE_HD_SHADER_PRESENTS.store(0, Ordering::Relaxed);
    iphone_link_video::video_hwnd::reset_hd_shader_trace_all();
    if let Ok(mut rect_g) = last_active_video_rect_lock().lock() {
        *rect_g = String::from("0,0,0,0");
    }
    if let Ok(mut drect_g) = last_presented_dest_rect_lock().lock() {
        *drect_g = String::from("0,0,0,0");
    }
    if let Ok(mut rec) = flight_recorder().lock() {
        rec.started_at = std::time::Instant::now();
        rec.events.clear();
        rec.counters = AirPlayTraceCounters::default();
    }
}

pub(crate) fn inc_trace_ntp_tx() {
    if let Ok(mut rec) = flight_recorder().lock() {
        rec.counters.ntp_tx = rec.counters.ntp_tx.saturating_add(1);
    }
}

pub(crate) fn inc_trace_ntp_rx() {
    if let Ok(mut rec) = flight_recorder().lock() {
        rec.counters.ntp_rx = rec.counters.ntp_rx.saturating_add(1);
    }
}

pub(crate) fn set_trace_ntp_err(err: impl Into<String>) {
    if let Ok(mut rec) = flight_recorder().lock() {
        let s: String = err
            .into()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | ':'))
            .take(40)
            .collect();
        if !s.is_empty() {
            rec.counters.ntp_last_err = Some(s);
        }
    }
}

pub(crate) fn inc_trace_event_accept() {
    if let Ok(mut rec) = flight_recorder().lock() {
        rec.counters.event_accepts = rec.counters.event_accepts.saturating_add(1);
    }
}

pub(crate) fn inc_trace_event_rx() {
    if let Ok(mut rec) = flight_recorder().lock() {
        rec.counters.event_rx = rec.counters.event_rx.saturating_add(1);
    }
}

pub(crate) fn inc_trace_audio_data_rx() {
    if let Ok(mut rec) = flight_recorder().lock() {
        rec.counters.audio_data_rx = rec.counters.audio_data_rx.saturating_add(1);
    }
}

pub(crate) fn inc_trace_audio_ctrl_rx() {
    if let Ok(mut rec) = flight_recorder().lock() {
        rec.counters.audio_ctrl_rx = rec.counters.audio_ctrl_rx.saturating_add(1);
    }
}

pub(crate) fn inc_trace_audio_decoded() {
    if let Ok(mut rec) = flight_recorder().lock() {
        rec.counters.audio_decoded = rec.counters.audio_decoded.saturating_add(1);
    }
}

pub(crate) fn add_trace_audio_played_samples(n: u64) {
    if let Ok(mut rec) = flight_recorder().lock() {
        rec.counters.audio_played_samples = rec.counters.audio_played_samples.saturating_add(n);
    }
}

pub(crate) fn inc_trace_audio_decode_error() {
    if let Ok(mut rec) = flight_recorder().lock() {
        rec.counters.audio_decode_errors = rec.counters.audio_decode_errors.saturating_add(1);
    }
}

pub(crate) fn add_trace_audio_dropped_packets(n: u64) {
    if let Ok(mut rec) = flight_recorder().lock() {
        rec.counters.audio_dropped_packets = rec.counters.audio_dropped_packets.saturating_add(n);
    }
}

pub(crate) fn add_trace_video_queue_dropped(n: u64) {
    if let Ok(mut rec) = flight_recorder().lock() {
        rec.counters.video_queue_dropped = rec.counters.video_queue_dropped.saturating_add(n);
    }
}

/// Syncs in-process `LiveVideoPipeline` + DComp presentation metrics into `/api/debug/airplay-trace`.
pub fn update_native_video_trace(
    sink_packets: u64,
    mf_decoded: u64,
    stream_changes: u64,
    not_accepting_drains: u64,
    dcomp_presented: u64,
    last_decoder_hresult: i32,
) {
    if let Ok(mut rec) = flight_recorder().lock() {
        rec.counters.native_video_sink_packets = sink_packets;
        rec.counters.mf_decoded_frames = mf_decoded;
        rec.counters.mf_stream_changes = stream_changes;
        rec.counters.mf_not_accepting_drains = not_accepting_drains;
        rec.counters.dcomp_presented_frames = dcomp_presented;
        rec.counters.last_decoder_hresult = last_decoder_hresult;
    }
}

/// Syncs full stage-by-stage native AirPlay H.264 pipeline telemetry into `/api/debug/airplay-trace`.
#[allow(clippy::too_many_arguments)]
pub fn update_native_video_stage_trace(
    sink_packets: u64,
    queue_dropped: u64,
    decoder_fed: u64,
    mf_decoded: u64,
    stream_changes: u64,
    not_accepting_drains: u64,
    stale_dim_dropped: u64,
    vram_published: u64,
    dcomp_presented: u64,
    present_calls: u64,
    present_errors: u64,
    last_decoder_hresult: i32,
    last_mft_dims: Option<(u32, u32)>,
    expected_coded_dims: Option<(u32, u32)>,
) {
    fn format_dims(dims: Option<(u32, u32)>) -> String {
        match dims {
            Some((w, h)) => format!("{w}x{h}"),
            None => "0x0".to_string(),
        }
    }
    if let Ok(mut rec) = flight_recorder().lock() {
        rec.counters.tcp_video_packets = TRACE_TCP_VIDEO_PACKETS.load(Ordering::Relaxed);
        rec.counters.avcc_updates = TRACE_AVCC_UPDATES.load(Ordering::Relaxed);
        rec.counters.queue_dropped = queue_dropped;
        rec.counters.native_video_sink_packets = sink_packets;
        rec.counters.decoder_fed = decoder_fed;
        rec.counters.mf_decoded_frames = mf_decoded;
        rec.counters.mf_stream_changes = stream_changes;
        rec.counters.mf_not_accepting_drains = not_accepting_drains;
        rec.counters.stale_dim_dropped = stale_dim_dropped;
        rec.counters.vram_published = vram_published;
        rec.counters.dcomp_presented_frames = dcomp_presented;
        rec.counters.present_calls = present_calls;
        rec.counters.present_errors = present_errors;
        rec.counters.last_decoder_hresult = last_decoder_hresult;
        rec.counters.last_mft_dims = format_dims(last_mft_dims);
        rec.counters.expected_coded_dims = format_dims(expected_coded_dims);
    }
}

pub(crate) fn set_trace_audio_codec(codec: &str) {
    if let Ok(mut rec) = flight_recorder().lock() {
        let safe: String = codec
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .take(16)
            .collect();
        rec.counters.audio_codec = if safe.is_empty() {
            "none".to_string()
        } else {
            safe
        };
    }
}

pub(crate) fn inc_trace_feedback() {
    if let Ok(mut rec) = flight_recorder().lock() {
        rec.counters.feedback_count = rec.counters.feedback_count.saturating_add(1);
    }
}

pub(crate) fn inc_trace_teardown() {
    if let Ok(mut rec) = flight_recorder().lock() {
        rec.counters.teardown_count = rec.counters.teardown_count.saturating_add(1);
    }
}

pub fn get_airplay_trace_counters() -> AirPlayTraceCounters {
    let mut c = flight_recorder()
        .lock()
        .map(|r| r.counters.clone())
        .unwrap_or_default();
    c.airplay_watchdog_recoveries = trace_watchdog_recoveries();
    c.mid_stream_black_clears = trace_mid_stream_black_clears();
    c.dual_buffer_overwrites = trace_dual_buffer_overwrites();
    c.ctr_resync_count = trace_ctr_resync_count();
    c.ctr_decrypt_rejects = trace_ctr_decrypt_rejects();
    c.ctr_keystream_bytes = trace_ctr_keystream_bytes();
    c.tcp_disconnects = trace_tcp_disconnects();
    c.layout_rect_clamps = trace_layout_rect_clamps();
    c.mft_dim_changes = trace_mft_dim_changes();
    c.last_active_video_rect = trace_last_active_video_rect();
    c.mf_low_latency_enabled = trace_mf_low_latency_enabled();
    c.dcomp_commits = trace_dcomp_commits();
    c.d3d11_multithread_protected = trace_d3d11_multithread_protected();
    c.dpb_preserved_flushes_skipped = trace_dpb_preserved_flushes_skipped();
    c.idr_keyframes_detected = trace_idr_keyframes_detected();
    c.fullscreen_landscape_resyncs = trace_fullscreen_landscape_resyncs();
    c.pre_decoder_dropped_awaiting_avcc = trace_pre_decoder_dropped_awaiting_avcc();
    c.pre_decoder_dropped_awaiting_idr = trace_pre_decoder_dropped_awaiting_idr();
    c.pre_decoder_dropped_stale_epoch = trace_pre_decoder_dropped_stale_epoch();
    c.cached_idr_replays = trace_cached_idr_replays();
    c.last_presented_dest_rect = trace_last_presented_dest_rect();
    c.last_presented_rotation_deg = trace_last_presented_rotation_deg();
    c.avcc_small_256_511_frames = trace_avcc_small_256_511_frames();
    c.mft_reconfigs = trace_mft_reconfigs();
    c.hd_shader_active = trace_hd_shader_active();
    c.hd_shader_presents = trace_hd_shader_presents();
    c
}

/// Appends one sanitized, PII-free technical event to the 80-entry AirPlay Flight Recorder.
pub fn record_airplay_trace(event: &str, detail: impl Into<String>) {
    if let Ok(mut rec) = flight_recorder().lock() {
        let t_ms = rec.started_at.elapsed().as_millis() as u64;
        if rec.events.len() >= MAX_FLIGHT_RECORDER_EVENTS {
            rec.events.pop_front();
        }
        rec.events.push_back(AirPlayTraceEvent {
            t_ms,
            event: event.to_string(),
            detail: detail.into(),
        });
    }
}

/// Returns the in-memory AirPlay Flight Recorder trace as clean PII-free JSON (`GET /api/debug/airplay-trace`).
pub fn get_airplay_trace_json() -> String {
    let (events, c) = flight_recorder()
        .lock()
        .map(|r| (r.events.iter().cloned().collect::<Vec<_>>(), r.counters.clone()))
        .unwrap_or_default();
    let mut items = String::new();
    for (idx, ev) in events.iter().enumerate() {
        if idx > 0 {
            items.push(',');
        }
        items.push_str(&format!(
            r#"{{"tMs":{},"event":"{}","detail":"{}"}}"#,
            ev.t_ms,
            escape_trace_json(&ev.event),
            escape_trace_json(&ev.detail),
        ));
    }
    let ntp_err_json = match c.ntp_last_err {
        Some(ref e) => format!(r#""{}""#, escape_trace_json(e)),
        None => "null".to_string(),
    };
    let ap_audio = get_airplay_audio_snapshot();
    let watchdog_recoveries = trace_watchdog_recoveries();
    let mid_stream_black_clears = trace_mid_stream_black_clears();
    let dual_buffer_overwrites = trace_dual_buffer_overwrites();
    let ctr_resync_count = trace_ctr_resync_count();
    let ctr_decrypt_rejects = trace_ctr_decrypt_rejects();
    let tcp_video_packets = c
        .tcp_video_packets
        .max(TRACE_TCP_VIDEO_PACKETS.load(Ordering::Relaxed));
    let avcc_updates = c
        .avcc_updates
        .max(TRACE_AVCC_UPDATES.load(Ordering::Relaxed));
    format!(
        r#"{{"ok":true,"count":{},"maxEntries":{},"counters":{{"ntpTx":{},"ntpRx":{},"ntpLastErr":{},"eventAccepts":{},"eventRx":{},"audioDataRx":{},"audioCtrlRx":{},"audioDecoded":{},"audioPlayedSamples":{},"audioDecodeErrors":{},"audioDroppedPackets":{},"videoQueueDropped":{},"tcpVideoPackets":{},"avccUpdates":{},"queueDropped":{},"nativeVideoSinkPackets":{},"decoderFed":{},"mfDecodedFrames":{},"mfStreamChanges":{},"mfNotAcceptingDrains":{},"staleDimDropped":{},"vramPublished":{},"dcompPresentedFrames":{},"presentCalls":{},"presentErrors":{},"lastDecoderHresult":{},"lastMftDims":"{}","expectedCodedDims":"{}","audioVolume":{},"audioBufferPreset":"{}","audioLipSync":{},"audioBufferMs":{},"audioTargetMs":{},"audioCodec":"{}","feedbackCount":{},"teardownCount":{},"airplayWatchdogRecoveries":{},"midStreamBlackClears":{},"dualBufferOverwrites":{},"ctrResyncCount":{},"ctrDecryptRejects":{},"ctrKeystreamBytes":{},"tcpDisconnects":{},"layoutRectClamps":{},"mftDimChanges":{},"lastActiveVideoRect":"{}","mfLowLatencyEnabled":{},"dcompCommits":{},"d3d11MultithreadProtected":{},"dpbPreservedFlushesSkipped":{},"idrKeyframesDetected":{},"fullscreenLandscapeResyncs":{},"preDecoderDroppedAwaitingAvcc":{},"preDecoderDroppedAwaitingIdr":{},"preDecoderDroppedStaleEpoch":{},"cachedIdrReplays":{},"lastPresentedDestRect":"{}","lastPresentedRotationDeg":{},"avccSmall256To511Frames":{},"mftReconfigs":{},"hdShaderActive":{},"hdShaderPresents":{}}},"events":[{}]}}"#,
        events.len(),
        MAX_FLIGHT_RECORDER_EVENTS,
        c.ntp_tx,
        c.ntp_rx,
        ntp_err_json,
        c.event_accepts,
        c.event_rx,
        c.audio_data_rx,
        c.audio_ctrl_rx,
        c.audio_decoded,
        c.audio_played_samples,
        c.audio_decode_errors,
        c.audio_dropped_packets.max(ap_audio.dropped_packets),
        c.video_queue_dropped,
        tcp_video_packets,
        avcc_updates,
        c.queue_dropped,
        c.native_video_sink_packets,
        c.decoder_fed,
        c.mf_decoded_frames,
        c.mf_stream_changes,
        c.mf_not_accepting_drains,
        c.stale_dim_dropped,
        c.vram_published,
        c.dcomp_presented_frames,
        c.present_calls,
        c.present_errors,
        c.last_decoder_hresult,
        escape_trace_json(&c.last_mft_dims),
        escape_trace_json(&c.expected_coded_dims),
        ap_audio.volume,
        escape_trace_json(&ap_audio.buffer_preset),
        ap_audio.lip_sync,
        ap_audio.buffer_ms,
        ap_audio.target_ms,
        escape_trace_json(&c.audio_codec),
        c.feedback_count,
        c.teardown_count,
        watchdog_recoveries,
        mid_stream_black_clears,
        dual_buffer_overwrites,
        ctr_resync_count,
        ctr_decrypt_rejects,
        trace_ctr_keystream_bytes(),
        trace_tcp_disconnects(),
        trace_layout_rect_clamps(),
        trace_mft_dim_changes(),
        escape_trace_json(&trace_last_active_video_rect()),
        trace_mf_low_latency_enabled(),
        trace_dcomp_commits(),
        trace_d3d11_multithread_protected(),
        trace_dpb_preserved_flushes_skipped(),
        trace_idr_keyframes_detected(),
        trace_fullscreen_landscape_resyncs(),
        trace_pre_decoder_dropped_awaiting_avcc(),
        trace_pre_decoder_dropped_awaiting_idr(),
        trace_pre_decoder_dropped_stale_epoch(),
        trace_cached_idr_replays(),
        escape_trace_json(&trace_last_presented_dest_rect()),
        trace_last_presented_rotation_deg(),
        trace_avcc_small_256_511_frames(),
        trace_mft_reconfigs(),
        trace_hd_shader_active(),
        trace_hd_shader_presents(),
        items
    )
}

/// Returns the last `limit` sanitized trace entries formatted as short one-line strings.
pub fn get_airplay_trace_tail_lines(limit: usize) -> Vec<String> {
    flight_recorder()
        .lock()
        .map(|r| {
            let skip = r.events.len().saturating_sub(limit);
            r.events
                .iter()
                .skip(skip)
                .map(|e| format!("+{}ms {} {}", e.t_ms, e.event, e.detail))
                .collect()
        })
        .unwrap_or_default()
}

pub fn hex_decode(s: &str) -> Vec<u8> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 2);
    let mut i = 0;
    while i + 1 < bytes.len() {
        let hi = hex_nibble(bytes[i]);
        let lo = hex_nibble(bytes[i + 1]);
        out.push((hi << 4) | lo);
        i += 2;
    }
    out
}

fn hex_nibble(b: u8) -> u8 {
    match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => b - b'a' + 10,
        b'A'..=b'F' => b - b'A' + 10,
        _ => 0,
    }
}

pub fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0F) as usize] as char);
    }
    out
}

/// First 16 bytes of SHA-512 over concatenated chunks (Apple AirPlay KDF primitive).
pub fn sha512_16(parts: &[&[u8]]) -> [u8; 16] {
    let digest = sha512_chunks(parts);
    let mut out = [0u8; 16];
    out.copy_from_slice(&digest[..16]);
    out
}

/// Derives the 16-byte AirPlay session AES key from FairPlay decrypted key + optional Pair-Verify ECDH secret.
pub fn session_key(fp_key: &[u8; 16], ecdh_secret: Option<&[u8; 32]>) -> [u8; 16] {
    match ecdh_secret {
        Some(secret) => sha512_16(&[fp_key, secret]),
        None => *fp_key,
    }
}

/// Derives the per-mirror-stream AES-128-CTR `(key, iv)` from session AES key and `streamConnectionID`.
pub fn stream_key_iv(aeskey: &[u8; 16], stream_conn_id: u64) -> ([u8; 16], [u8; 16]) {
    let k_prefix = format!("AirPlayStreamKey{stream_conn_id}");
    let iv_prefix = format!("AirPlayStreamIV{stream_conn_id}");
    (
        sha512_16(&[k_prefix.as_bytes(), aeskey]),
        sha512_16(&[iv_prefix.as_bytes(), aeskey]),
    )
}

static GLOBAL_AIRPLAY: OnceLock<Mutex<Option<AirPlayServer>>> = OnceLock::new();

fn global_slot() -> &'static Mutex<Option<AirPlayServer>> {
    GLOBAL_AIRPLAY.get_or_init(|| Mutex::new(None))
}

/// Starts (or cleanly replaces) the singleton global AirPlay receiver.
/// Returns `(actual_port, receiver_name)`.
pub fn start_global_airplay_receiver(
    name: Option<&str>,
    bind_port: Option<u16>,
    sink: Arc<dyn AirPlaySink>,
    advertise_mdns: bool,
) -> Result<(u16, String), String> {
    stop_global_airplay_receiver();
    fairplay::ensure_loaded()?;

    let id = AirPlayIdentity::load_default();
    let receiver_name = name
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_RECEIVER_NAME)
        .to_string();

    // Try requested port first (default 7100); if occupied by another receiver on PC, fall back to
    // 7101..=7110 or OS ephemeral port so mirroring never hard-fails on port conflict.
    let srv = if let Some(explicit) = bind_port {
        AirPlayServer::start(&receiver_name, explicit, id, sink, advertise_mdns)?
    } else {
        let mut last_err = String::new();
        let mut started = None;
        for candidate in [DEFAULT_PORT, 7101, 7102, 7103, 0] {
            match AirPlayServer::start(
                &receiver_name,
                candidate,
                id.clone(),
                sink.clone(),
                advertise_mdns,
            ) {
                Ok(s) => {
                    started = Some(s);
                    break;
                }
                Err(e) => last_err = e,
            }
        }
        started.ok_or_else(|| last_err)?
    };

    let port = srv.port();
    let final_name = srv.name().to_string();
    if let Ok(mut guard) = global_slot().lock() {
        *guard = Some(srv);
    }
    Ok((port, final_name))
}

/// Stops the singleton global AirPlay receiver and joins all RTSP/Mirror/mDNS threads cleanly.
pub fn stop_global_airplay_receiver() {
    let taken = global_slot().lock().ok().and_then(|mut g| g.take());
    if let Some(mut srv) = taken {
        srv.stop();
    }
}

pub fn active_airplay_port() -> u16 {
    global_slot()
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|s| s.port()))
        .unwrap_or(0)
}

pub fn is_global_airplay_running() -> bool {
    global_slot()
        .lock()
        .ok()
        .map(|g| g.is_some())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::{Duration, Instant};

    use plist::{Dictionary, Value};

    use crate::hid::map_airplay_touch_to_ios_digitizer;
    use crate::link::{resolve_ios_mirror_route, IosMirrorRoute};
    use crate::physical_stream::{evaluate_stream_liveness_for_mode, StreamLivenessAction};
    use crate::rp_wifi_tls::{x25519, x25519_public};
    use crate::win_cng::Aes128Ctr;

    fn send_rtsp(
        stream: &mut TcpStream,
        method: &str,
        path: &str,
        cseq: u32,
        content_type: Option<&str>,
        body: &[u8],
    ) -> (u16, Vec<u8>) {
        let mut req = format!("{method} {path} RTSP/1.0\r\nCSeq: {cseq}\r\n");
        if let Some(ct) = content_type {
            req.push_str(&format!("Content-Type: {ct}\r\n"));
        }
        req.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
        stream.write_all(req.as_bytes()).expect("write rtsp head");
        if !body.is_empty() {
            stream.write_all(body).expect("write rtsp body");
        }
        stream.flush().expect("flush rtsp");

        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        let header_end = loop {
            if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break pos;
            }
            let n = stream.read(&mut tmp).expect("read rtsp resp head");
            assert!(n > 0, "unexpected EOF on RTSP response");
            buf.extend_from_slice(&tmp[..n]);
        };
        let head_str = String::from_utf8_lossy(&buf[..header_end]).to_string();
        let status_code = head_str
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|c| c.parse::<u16>().ok())
            .unwrap_or(0);
        let content_len = head_str
            .lines()
            .find_map(|l| {
                let (k, v) = l.split_once(':')?;
                if k.trim().eq_ignore_ascii_case("content-length") {
                    v.trim().parse::<usize>().ok()
                } else {
                    None
                }
            })
            .unwrap_or(0);
        let mut resp_body = buf[header_end + 4..].to_vec();
        while resp_body.len() < content_len {
            let n = stream.read(&mut tmp).expect("read rtsp resp body");
            assert!(n > 0, "unexpected EOF on RTSP body");
            resp_body.extend_from_slice(&tmp[..n]);
        }
        resp_body.truncate(content_len);
        (status_code, resp_body)
    }

    #[test]
    fn test_session_and_stream_key_derivation_vectors() {
        let fp_key = [0x11u8; 16];
        let secret = [0x22u8; 32];
        let sk = session_key(&fp_key, Some(&secret));
        assert_ne!(sk, fp_key);
        assert_eq!(session_key(&fp_key, None), fp_key);

        let (k1, iv1) = stream_key_iv(&sk, 1234567890123456789u64);
        let (k2, iv2) = stream_key_iv(&sk, 1234567890123456790u64);
        assert_ne!(k1, k2);
        assert_ne!(iv1, iv2);
        assert_ne!(k1, iv1);
    }

    /// Mock Test M-1: iOS 16.x / 17.3 without CoreDevice routes to AirPlay Standalone, completes full
    /// real RTSP + Pair-Verify + FairPlay v3 (`playfair.dll`) + `Aes128Ctr` H.264 stream over TCP,
    /// strips SEI, advances Honest Gate to `running`, and publishes `avc1.*` with `trim == 0`.
    #[test]
    fn test_tahap252_m1_ios16_and_17_3_no_coredevice_routes_to_airplay_standalone() {
        assert_eq!(
            resolve_ios_mirror_route("16.7.10"),
            IosMirrorRoute::AirPlayStandalone
        );
        assert_eq!(
            resolve_ios_mirror_route("17.3.1"),
            IosMirrorRoute::AirPlayStandalone
        );

        let start_json = crate::mirror::start_airplay_mirror_session(
            Some("00000000-0000000000001670"),
            false,
            "wifi",
            None,
            Some(0),
            false,
        )
        .expect("start AirPlay standalone session");
        assert!(start_json.contains(r#""phase":"starting""#));
        assert!(start_json.contains(r#""stage":"waiting_airplay""#));
        assert!(start_json.contains(r#""mirrorRoute":"airplay_standalone""#));
        assert!(start_json.contains(r#""airplayActive":true"#));
        assert!(start_json.contains(r#""hidControlAvailable":false"#));

        let port = active_airplay_port();
        assert!(port > 0, "AirPlay ephemeral port must be bound");

        let mut rtsp = TcpStream::connect(("127.0.0.1", port)).expect("connect AirPlay RTSP");
        rtsp.set_read_timeout(Some(Duration::from_secs(3))).unwrap();

        // 1. GET /info -> binary plist with explicit hevc=false and bit 42 = 0
        let (code, info_bp) = send_rtsp(&mut rtsp, "GET", "/info", 1, None, &[]);
        assert_eq!(code, 200);
        assert!(info_bp.starts_with(b"bplist00"));
        let info_val = plist::from_bytes(&info_bp).expect("decode /info bplist00");
        let info_dict = info_val.as_dictionary().expect("/info dictionary");
        assert_eq!(info_dict.get("hevc").and_then(|v| v.as_boolean()), Some(false));
        assert_eq!(info_dict.get("name").and_then(|v| v.as_string()), Some("iPhone Link"));
        let feat = info_dict
            .get("features")
            .and_then(|v| v.as_unsigned_integer())
            .expect("features u64");
        assert_eq!((feat >> 32) & 0x400, 0, "HEVC feature bit 42 must be 0");

        // 2. POST /pair-setup -> 32-byte Ed25519 pk
        let (code, pk) = send_rtsp(&mut rtsp, "POST", "/pair-setup", 2, None, &[]);
        assert_eq!(code, 200);
        assert_eq!(pk.len(), 32);

        // 3. POST /pair-verify Step 1 -> derive real X25519 ECDH secret
        let client_x_priv = [0x42u8; 32];
        let client_x_pub = x25519_public(&client_x_priv);
        let mut pv1 = vec![1u8, 0, 0, 0];
        pv1.extend_from_slice(&client_x_pub);
        pv1.extend_from_slice(&[0x55u8; 32]);
        let (code, pv1_resp) = send_rtsp(
            &mut rtsp,
            "POST",
            "/pair-verify",
            3,
            Some("application/octet-stream"),
            &pv1,
        );
        assert_eq!(code, 200);
        assert_eq!(pv1_resp.len(), 96);
        let server_x_pub: [u8; 32] = pv1_resp[..32].try_into().unwrap();
        let ecdh_secret = x25519(&client_x_priv, &server_x_pub);

        // 4. POST /fp-setup Stage 1 (16 B -> 142 B) & Stage 2 (164 B -> 32 B)
        let fp1_req = [
            0x46, 0x50, 0x4C, 0x59, 0x03, 0x01, 0x01, 0x00,
            0x00, 0x00, 0x00, 0x04, 0x02, 0x00, 0x02, 0xBB,
        ];
        let (code, fp1_resp) = send_rtsp(
            &mut rtsp,
            "POST",
            "/fp-setup",
            4,
            Some("application/octet-stream"),
            &fp1_req,
        );
        assert_eq!(code, 200);
        assert_eq!(fp1_resp.len(), 142);

        let mut fp2_req = vec![0x46, 0x50, 0x4C, 0x59, 0x03];
        fp2_req.resize(164, 0);
        let (code, fp2_resp) = send_rtsp(
            &mut rtsp,
            "POST",
            "/fp-setup",
            5,
            Some("application/octet-stream"),
            &fp2_req,
        );
        assert_eq!(code, 200);
        assert_eq!(fp2_resp.len(), 32);

        // 5. SETUP Stage 1 (72-byte ekey decrypted via real playfair.dll + 16-byte eiv)
        let ekey: Vec<u8> = (0u8..72u8).collect();
        let expected_fp_key: [u8; 16] = hex_decode("299464065e68f7ed2125fcdd9a06f983")
            .try_into()
            .unwrap();
        let mut s1 = Dictionary::new();
        s1.insert("ekey".into(), Value::Data(ekey));
        s1.insert("eiv".into(), Value::Data(vec![0x77u8; 16]));
        let s1_bp = plist::to_bytes_bplist00(&Value::Dictionary(s1)).unwrap();
        let (code, s1_resp) = send_rtsp(
            &mut rtsp,
            "SETUP",
            "rtsp://127.0.0.1/1",
            6,
            Some("application/x-apple-binary-plist"),
            &s1_bp,
        );
        assert_eq!(code, 200);
        let s1_val = plist::from_bytes(&s1_resp).expect("decode SETUP1 resp");
        let timing_port = s1_val
            .as_dictionary()
            .and_then(|d| d.get("timingPort"))
            .and_then(|v| v.as_unsigned_integer())
            .unwrap_or(0);
        assert!(timing_port > 0, "timingPort must be allocated");

        // 6. SETUP Stage 2 (type=110 mirror video -> returns dataPort)
        let stream_conn_id: u64 = 9_876_543_210;
        let mut st_desc = Dictionary::new();
        st_desc.insert("type".into(), Value::from(110u64));
        st_desc.insert("streamConnectionID".into(), Value::from(stream_conn_id));
        let mut s2 = Dictionary::new();
        s2.insert("streams".into(), Value::Array(vec![Value::Dictionary(st_desc)]));
        let s2_bp = plist::to_bytes_bplist00(&Value::Dictionary(s2)).unwrap();
        let (code, s2_resp) = send_rtsp(
            &mut rtsp,
            "SETUP",
            "rtsp://127.0.0.1/1",
            7,
            Some("application/x-apple-binary-plist"),
            &s2_bp,
        );
        assert_eq!(code, 200);
        let s2_val = plist::from_bytes(&s2_resp).expect("decode SETUP2 resp");
        let data_port = s2_val
            .as_dictionary()
            .and_then(|d| d.get("streams"))
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|v| v.as_dictionary())
            .and_then(|d| d.get("dataPort"))
            .and_then(|v| v.as_unsigned_integer())
            .unwrap_or(0) as u16;
        assert!(data_port > 0, "mirror dataPort must be bound");

        // 7. Connect mirror TCP, send payloadType=1 (avcC 1280x720) + encrypted payloadType=0 (SEI+IDR)
        let mut mirror_tcp = TcpStream::connect(("127.0.0.1", data_port)).expect("connect mirror");
        let sps = vec![0x67, 0x42, 0xC0, 0x1F, 0xF4, 0x02, 0x80, 0x2D, 0xD0];
        let pps = vec![0x68, 0xCE, 0x38, 0x80];
        let mut avcc = vec![0x01, 0x42, 0xC0, 0x1F, 0xFF, 0xE1, 0x00, sps.len() as u8];
        avcc.extend_from_slice(&sps);
        avcc.push(0x01);
        avcc.extend_from_slice(&[0x00, pps.len() as u8]);
        avcc.extend_from_slice(&pps);

        let mut hdr1 = [0u8; 128];
        hdr1[0..4].copy_from_slice(&(avcc.len() as u32).to_le_bytes());
        hdr1[4..6].copy_from_slice(&1u16.to_le_bytes());
        mirror_tcp.write_all(&hdr1).unwrap();
        mirror_tcp.write_all(&avcc).unwrap();

        let sei = [0x06u8, 0x05, 0x02, 0x11, 0x22];
        let idr = [0x65u8, 0x88, 0x84, 0x21, 0xA0, 0x33, 0x44];
        let mut plain_au = Vec::new();
        plain_au.extend_from_slice(&(sei.len() as u32).to_be_bytes());
        plain_au.extend_from_slice(&sei);
        plain_au.extend_from_slice(&(idr.len() as u32).to_be_bytes());
        plain_au.extend_from_slice(&idr);

        let sess_key = session_key(&expected_fp_key, Some(&ecdh_secret));
        let (skey, siv) = stream_key_iv(&sess_key, stream_conn_id);
        let mut enc_au = plain_au.clone();
        let mut enc = Aes128Ctr::new(&skey, &siv).unwrap();
        enc.apply_keystream(&mut enc_au).unwrap();

        let mut hdr0 = [0u8; 128];
        hdr0[0..4].copy_from_slice(&(enc_au.len() as u32).to_le_bytes());
        hdr0[4..6].copy_from_slice(&0u16.to_le_bytes());
        mirror_tcp.write_all(&hdr0).unwrap();
        mirror_tcp.write_all(&enc_au).unwrap();
        mirror_tcp.flush().unwrap();

        // Wait up to 2s for Honest Gate promotion to phase=="running"
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut final_status = String::new();
        while Instant::now() < deadline {
            final_status = crate::mirror::global_screen_status_json();
            if final_status.contains(r#""phase":"running""#) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            final_status.contains(r#""phase":"running""#),
            "Honest Gate must advance to running after decrypted H.264 IDR: {final_status}"
        );
        assert!(final_status.contains(r#""url":"/mirror.html""#));

        let codec_json = crate::mirror::global_codec_json();
        assert!(codec_json.contains(r#""codec":"avc1.42C01F""#), "codec={codec_json}");
        assert!(codec_json.contains(r#""isAirplay":true"#), "codec={codec_json}");
        assert!(codec_json.contains(r#""width":1280"#), "codec={codec_json}");
        assert!(codec_json.contains(r#""height":720"#), "codec={codec_json}");
        assert!(
            codec_json.contains(r#""trim":{"top":0,"left":0,"bottom":0,"right":0}"#),
            "AirPlay trim must be zero: {codec_json}"
        );

        // Verify live broadcast chunk contains stripped SEI -> IDR only
        let mut last_seq = 0u64;
        let mut sent_init = false;
        let chunks = crate::mirror::global_wait_next_live_chunks(&mut last_seq, &mut sent_init, 500)
            .expect("live chunks must be active");
        assert!(!chunks.is_empty(), "live broadcast must hold decrypted IDR chunk");
        let chunk = &chunks[0];
        // [4B BE len][1B type=0][4B BE nal_len][idr...]
        assert_eq!(chunk[4], 0, "first chunk must be keyframe (type 0)");
        assert_eq!(&chunk[9..], &idr, "SEI must be stripped leaving exact IDR NALU");

        drop(mirror_tcp);
        drop(rtsp);
        let stopped = crate::mirror::global_stop_screen();
        assert!(stopped.json.contains(r#""phase":"idle""#));
        assert!(!is_global_airplay_running());
    }

    /// Mock Test M-2: iOS 17.4–26.x Hybrid Mode (`gate_only = true` with 0 video RTP > 15s),
    /// fail-soft picture-only guarantee, inverse touch `device_point` across 0°/90°/180°/270°,
    /// and rapid Stop -> Restart #2 on the exact same port with zero `AddrInUse`.
    #[test]
    fn test_tahap252_m2_ios17_4_to_26_hybrid_gate_only_zero_video_rtp_and_device_point() {
        // 1. ProductVersion router tier verification (Tahap 293 Opsi 2: iOS < 27 -> AirPlayStandalone)
        assert_eq!(
            resolve_ios_mirror_route("17.4"),
            IosMirrorRoute::AirPlayStandalone
        );
        assert_eq!(
            resolve_ios_mirror_route("18.5"),
            IosMirrorRoute::AirPlayStandalone
        );
        assert_eq!(
            resolve_ios_mirror_route("26.2"),
            IosMirrorRoute::AirPlayStandalone
        );
        assert_eq!(
            resolve_ios_mirror_route("27.0"),
            IosMirrorRoute::CoreDeviceNativeHevc
        );
        assert_eq!(
            resolve_ios_mirror_route("27.2"),
            IosMirrorRoute::CoreDeviceNativeHevc
        );
        assert_eq!(
            resolve_ios_mirror_route(""),
            IosMirrorRoute::CoreDeviceNativeHevc
        );

        // 2. Hybrid `gate_only = true` MUST keep tunnel alive even with 20s zero video RTP
        assert_eq!(
            evaluate_stream_liveness_for_mode(
                Duration::from_secs(20),
                Duration::from_secs(20),
                Some(Duration::from_secs(3)),
                false,
                false,
                false,
                true,
            ),
            StreamLivenessAction::Healthy,
            "Hybrid gate_only mode must NEVER disconnect on 0 video RTP frames!"
        );
        // iOS 27+ `gate_only = false` MUST preserve 12s dead-tunnel HardReconnect
        assert_eq!(
            evaluate_stream_liveness_for_mode(
                Duration::from_secs(20),
                Duration::from_secs(20),
                None,
                false,
                true,
                false,
                false,
            ),
            StreamLivenessAction::HardReconnect
        );

        // 3. Inverse touch `map_airplay_touch_to_ios_digitizer` (1:1 zenlink_core::api::device_point)
        assert_eq!(map_airplay_touch_to_ios_digitizer(0.0, 0.0, 0), (0, 0));
        assert_eq!(map_airplay_touch_to_ios_digitizer(1.0, 0.0, 0), (65535, 0));
        assert_eq!(map_airplay_touch_to_ios_digitizer(0.25, 0.75, 0), (16384, 49151));
        assert_eq!(map_airplay_touch_to_ios_digitizer(1.0, 0.0, 90), (0, 0));
        assert_eq!(map_airplay_touch_to_ios_digitizer(0.0, 1.0, 90), (65535, 65535));
        assert_eq!(map_airplay_touch_to_ios_digitizer(1.0, 1.0, 180), (0, 0));
        assert_eq!(map_airplay_touch_to_ios_digitizer(0.25, 0.75, 180), (49151, 16384));
        assert_eq!(map_airplay_touch_to_ios_digitizer(0.0, 1.0, 270), (0, 0));
        assert_eq!(map_airplay_touch_to_ios_digitizer(1.0, 0.0, 270), (65535, 65535));

        // 4. AirPlay Standalone guarantee (Tahap 293 Opsi 2)
        let _start_json = crate::mirror::start_airplay_mirror_session(
            Some("00000000-0000000000001850"),
            false,
            "wifi",
            Some("192.0.2.55"),
            Some(0),
            false,
        )
        .expect("start AirPlay Standalone session");
        let port1 = active_airplay_port();
        assert!(port1 > 0);
        let gen1 = crate::mirror::current_live_session_gen();
        let idr_avcc = vec![0, 0, 0, 5, 0x65, 0x88, 0x84, 0x21, 0xA0];
        assert!(crate::mirror::global_on_physical_h264_avcc_frame(
            gen1, idr_avcc, true
        ));
        crate::mirror::set_hybrid_hid_available(gen1, false);
        let st_failsoft = crate::mirror::global_screen_status_json();
        assert!(
            st_failsoft.contains(r#""phase":"running""#),
            "AirPlay video MUST stay running: {st_failsoft}"
        );
        assert!(st_failsoft.contains(r#""mirrorRoute":"airplay_standalone""#));
        assert!(st_failsoft.contains(r#""hidControlAvailable":false"#));

        // 5. Rapid Stop -> Restart #2 on the EXACT same port1 with ZERO AddrInUse
        let _ = crate::mirror::global_stop_screen();
        assert!(!is_global_airplay_running());
        let restart_json = crate::mirror::start_airplay_mirror_session(
            Some("00000000-0000000000001850"),
            false,
            "wifi",
            None,
            Some(port1),
            false,
        )
        .expect("Restart #2 on exact same port must succeed with zero AddrInUse");
        assert!(restart_json.contains(r#""airplayActive":true"#));
        assert_eq!(active_airplay_port(), port1);

        let _ = crate::mirror::global_stop_screen();
        assert!(!is_global_airplay_running());
    }

    #[test]
    fn test_tahap253_airplay_late_joiner_receives_init_idr_plus_all_trailing_deltas_without_pli() {
        let _ = crate::mirror::start_airplay_mirror_session(
            Some("00000000-TEST-DEV-A"),
            false,
            "wifi",
            None,
            Some(0),
            false,
        )
        .expect("start AirPlay standalone for late-joiner test");

        let gen = crate::mirror::current_live_session_gen();
        crate::mirror::clear_live_pli_request();

        // Push 1 IDR (seq 1) + 20 trailing delta P-frames (seq 2..21) BEFORE any /mirror/live client connects
        let idr = vec![0, 0, 0, 5, 0x65, 0x88, 0x84, 0x21, 0xA0];
        assert!(crate::mirror::global_on_physical_h264_avcc_frame(gen, idr, true));
        for i in 1..=20u8 {
            let delta = vec![0, 0, 0, 4, 0x41, 0x9A, 0x10, i];
            assert!(crate::mirror::global_on_physical_h264_avcc_frame(gen, delta, false));
        }

        // Simulate late-joining /mirror/live client (e.g., mirror.html opening 500ms after phase=="running")
        let mut last_seq = 0u64;
        let mut sent_init = false;
        let chunks = crate::mirror::global_wait_next_live_chunks(&mut last_seq, &mut sent_init, 500)
            .expect("live broadcast active");
        assert!(sent_init);
        assert_eq!(
            chunks.len(),
            21,
            "AirPlay late-joiner MUST receive IDR seq 1 + ALL 20 trailing deltas (never truncated at 8!)"
        );
        assert_eq!(last_seq, 21);
        assert_eq!(chunks[0][4], 0, "chunk 0 must be IDR keyframe (type 0)");
        for idx in 1..21 {
            assert_eq!(chunks[idx][4], 1, "chunk {idx} must be delta (type 1)");
            assert_eq!(chunks[idx][12], idx as u8, "delta ordering must be contiguous");
        }
        assert!(
            !crate::mirror::is_live_pli_requested(),
            "Zero-PLI AirPlay must NOT trigger RTCP PLI on late-joiner"
        );

        let _ = crate::mirror::global_stop_screen();
    }

    #[test]
    fn test_tahap253_airplay_30_consecutive_encrypted_frames_zero_decrypt_failure_and_avcc_normalize() {
        let _ = crate::mirror::start_airplay_mirror_session(
            Some("00000000-TEST-DEV-A"),
            false,
            "wifi",
            None,
            Some(0),
            false,
        )
        .expect("start AirPlay session");
        let port = active_airplay_port();
        let mut rtsp = TcpStream::connect(("127.0.0.1", port)).expect("connect RTSP");
        rtsp.set_read_timeout(Some(Duration::from_secs(3))).unwrap();

        // Pair-Verify + FairPlay v3 (playfair.dll) + SETUP Stage 1 & 2
        let client_x_priv = [0x33u8; 32];
        let client_x_pub = x25519_public(&client_x_priv);
        let mut pv1 = vec![1u8, 0, 0, 0];
        pv1.extend_from_slice(&client_x_pub);
        pv1.extend_from_slice(&[0x44u8; 32]);
        let (code, pv1_resp) = send_rtsp(&mut rtsp, "POST", "/pair-verify", 1, Some("application/octet-stream"), &pv1);
        assert_eq!(code, 200);
        let server_x_pub: [u8; 32] = pv1_resp[..32].try_into().unwrap();
        let ecdh_secret = x25519(&client_x_priv, &server_x_pub);

        let fp1_req = [
            0x46, 0x50, 0x4C, 0x59, 0x03, 0x01, 0x01, 0x00,
            0x00, 0x00, 0x00, 0x04, 0x02, 0x00, 0x02, 0xBB,
        ];
        assert_eq!(send_rtsp(&mut rtsp, "POST", "/fp-setup", 2, Some("application/octet-stream"), &fp1_req).0, 200);
        let mut fp2_req = vec![0x46, 0x50, 0x4C, 0x59, 0x03];
        fp2_req.resize(164, 0);
        assert_eq!(send_rtsp(&mut rtsp, "POST", "/fp-setup", 3, Some("application/octet-stream"), &fp2_req).0, 200);

        let ekey: Vec<u8> = (0u8..72u8).collect();
        let expected_fp_key: [u8; 16] = hex_decode("299464065e68f7ed2125fcdd9a06f983").try_into().unwrap();
        let mut s1 = Dictionary::new();
        s1.insert("ekey".into(), Value::Data(ekey));
        s1.insert("eiv".into(), Value::Data(vec![0x66u8; 16]));
        let s1_bp = plist::to_bytes_bplist00(&Value::Dictionary(s1)).unwrap();
        assert_eq!(send_rtsp(&mut rtsp, "SETUP", "rtsp://127.0.0.1/1", 4, Some("application/x-apple-binary-plist"), &s1_bp).0, 200);

        let stream_conn_id: u64 = 1_234_567_890_123;
        let mut st_desc = Dictionary::new();
        st_desc.insert("type".into(), Value::from(110u64));
        st_desc.insert("streamConnectionID".into(), Value::from(stream_conn_id));
        let mut s2 = Dictionary::new();
        s2.insert("streams".into(), Value::Array(vec![Value::Dictionary(st_desc)]));
        let s2_bp = plist::to_bytes_bplist00(&Value::Dictionary(s2)).unwrap();
        let (code, s2_resp) = send_rtsp(&mut rtsp, "SETUP", "rtsp://127.0.0.1/1", 5, Some("application/x-apple-binary-plist"), &s2_bp);
        assert_eq!(code, 200);
        let s2_val = plist::from_bytes(&s2_resp).unwrap();
        let data_port = s2_val
            .as_dictionary().unwrap()
            .get("streams").unwrap().as_array().unwrap()[0]
            .as_dictionary().unwrap()
            .get("dataPort").unwrap().as_unsigned_integer().unwrap() as u16;

        let mut mirror_tcp = TcpStream::connect(("127.0.0.1", data_port)).expect("connect mirror");
        let sps = vec![0x67, 0x42, 0xC0, 0x1F, 0xF4, 0x02, 0x80, 0x2D, 0xD0];
        let pps = vec![0x68, 0xCE, 0x38, 0x80];
        let mut avcc = vec![0x01, 0x42, 0xC0, 0x1F, 0xFF, 0xE1, 0x00, sps.len() as u8];
        avcc.extend_from_slice(&sps);
        avcc.push(0x01);
        avcc.extend_from_slice(&[0x00, pps.len() as u8]);
        avcc.extend_from_slice(&pps);

        let mut hdr1 = [0u8; 128];
        hdr1[0..4].copy_from_slice(&(avcc.len() as u32).to_le_bytes());
        hdr1[4] = 1;
        mirror_tcp.write_all(&hdr1).unwrap();
        mirror_tcp.write_all(&avcc).unwrap();

        let sess_key = session_key(&expected_fp_key, Some(&ecdh_secret));
        let (skey, siv) = stream_key_iv(&sess_key, stream_conn_id);
        let mut enc = Aes128Ctr::new(&skey, &siv).unwrap();

        // Send 1 IDR + 29 encrypted delta frames interleaved with kind=2 heartbeats and AUD(9)+SEI(6)
        for frame_idx in 0..30u8 {
            if frame_idx % 5 == 0 {
                // Heartbeat packet (kind = 2) with 16-byte dummy body — MUST NOT advance CTR keystream!
                let mut hb_hdr = [0u8; 128];
                hb_hdr[0..4].copy_from_slice(&16u32.to_le_bytes());
                hb_hdr[4] = 2;
                mirror_tcp.write_all(&hb_hdr).unwrap();
                mirror_tcp.write_all(&[0xAAu8; 16]).unwrap();
            }

            let mut au = Vec::new();
            let aud = [0x09u8, 0x10];
            let sei = [0x06u8, 0x05, 0x01, frame_idx];
            let vcl: Vec<u8> = if frame_idx == 0 {
                vec![0x65, 0x88, 0x84, 0x21, 0xA0, 0x01, 0x02, 0x03]
            } else {
                vec![0x41, 0x9A, 0x22, frame_idx, 0x55, 0x66]
            };
            au.extend_from_slice(&(aud.len() as u32).to_be_bytes());
            au.extend_from_slice(&aud);
            au.extend_from_slice(&(sei.len() as u32).to_be_bytes());
            au.extend_from_slice(&sei);
            au.extend_from_slice(&(vcl.len() as u32).to_be_bytes());
            au.extend_from_slice(&vcl);

            enc.apply_keystream(&mut au).unwrap();
            let mut hdr0 = [0u8; 128];
            hdr0[0..4].copy_from_slice(&(au.len() as u32).to_le_bytes());
            hdr0[4] = 0;
            mirror_tcp.write_all(&hdr0).unwrap();
            mirror_tcp.write_all(&au).unwrap();
        }
        mirror_tcp.flush().unwrap();

        let deadline = Instant::now() + Duration::from_secs(3);
        let mut st = String::new();
        while Instant::now() < deadline {
            st = crate::mirror::global_screen_status_json();
            if st.contains(r#""airplayVideoFrames":30"#) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(st.contains(r#""phase":"running""#), "st={st}");
        assert!(st.contains(r#""airplayAvccCount":1"#), "st={st}");
        assert!(st.contains(r#""airplayIdrCount":1"#), "st={st}");
        assert!(st.contains(r#""airplayVideoFrames":30"#), "st={st}");
        assert!(st.contains(r#""airplayDecryptFailures":0"#), "st={st}");

        // Verify /mirror/codec exposes both isAirplay and is_airplay
        let codec_json = crate::mirror::global_codec_json();
        assert!(codec_json.contains(r#""isAirplay":true"#), "codec={codec_json}");
        assert!(codec_json.contains(r#""is_airplay":true"#), "codec={codec_json}");

        drop(mirror_tcp);
        drop(rtsp);
        let _ = crate::mirror::global_stop_screen();
    }

    #[test]
    fn test_tahap253_airplay_rtsp_event_port_and_ntp_timing_keepalive() {
        let _ = crate::mirror::start_airplay_mirror_session(
            Some("00000000-TEST-DEV-A"),
            false,
            "wifi",
            None,
            Some(0),
            false,
        )
        .expect("start AirPlay session");
        let port = active_airplay_port();
        let mut rtsp = TcpStream::connect(("127.0.0.1", port)).expect("connect RTSP");
        rtsp.set_read_timeout(Some(Duration::from_secs(3))).unwrap();

        // FairPlay v3 Stage 1 & 2
        let fp1_req = [
            0x46, 0x50, 0x4C, 0x59, 0x03, 0x01, 0x01, 0x00,
            0x00, 0x00, 0x00, 0x04, 0x02, 0x00, 0x02, 0xBB,
        ];
        assert_eq!(send_rtsp(&mut rtsp, "POST", "/fp-setup", 1, Some("application/octet-stream"), &fp1_req).0, 200);
        let mut fp2_req = vec![0x46, 0x50, 0x4C, 0x59, 0x03];
        fp2_req.resize(164, 0);
        assert_eq!(send_rtsp(&mut rtsp, "POST", "/fp-setup", 2, Some("application/octet-stream"), &fp2_req).0, 200);

        // SETUP Stage 1 -> check eventPort > 0 and timingPort > 0
        let mut s1 = Dictionary::new();
        s1.insert("ekey".into(), Value::Data((0u8..72u8).collect()));
        s1.insert("eiv".into(), Value::Data(vec![0x55u8; 16]));
        let s1_bp = plist::to_bytes_bplist00(&Value::Dictionary(s1)).unwrap();
        let (code, s1_resp) = send_rtsp(&mut rtsp, "SETUP", "rtsp://127.0.0.1/1", 3, Some("application/x-apple-binary-plist"), &s1_bp);
        assert_eq!(code, 200);
        let s1_val = plist::from_bytes(&s1_resp).unwrap();
        let s1_dict = s1_val.as_dictionary().unwrap();
        let timing_port = s1_dict.get("timingPort").unwrap().as_unsigned_integer().unwrap() as u16;
        let event_port = s1_dict.get("eventPort").unwrap().as_unsigned_integer().unwrap() as u16;
        assert!(timing_port > 0, "timingPort must be allocated");
        assert!(event_port > 0, "eventPort must be allocated");

        // 1. Verify eventPort TCP accepts connection, drains payload, and stays open
        let mut ev_tcp = TcpStream::connect(("127.0.0.1", event_port)).expect("connect eventPort");
        ev_tcp.set_read_timeout(Some(Duration::from_millis(120))).unwrap();
        ev_tcp.write_all(b"POST /event HTTP/1.1\r\nContent-Length: 0\r\n\r\n").unwrap();
        std::thread::sleep(Duration::from_millis(60));
        let mut probe_buf = [0u8; 16];
        match ev_tcp.read(&mut probe_buf) {
            Ok(0) => panic!("eventPort MUST stay open and never close prematurely!"),
            _ => {}
        }

        // 2. Verify timingPort UDP responds to 0xD2 NTP Timing Request with exact 0xD3 reply
        let udp = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        udp.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let mut ntp_req = [0u8; 32];
        ntp_req[0] = 0x80;
        ntp_req[1] = 0xD2;
        ntp_req[2..4].copy_from_slice(&[0x00, 0x07]);
        ntp_req[24..32].copy_from_slice(&[0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88]);
        udp.send_to(&ntp_req, ("127.0.0.1", timing_port)).unwrap();
        let mut ntp_resp = [0u8; 64];
        let (n, _) = udp.recv_from(&mut ntp_resp).expect("receive NTP 0xD3 response");
        assert_eq!(n, 32);
        assert_eq!(ntp_resp[0], 0x80);
        assert_eq!(ntp_resp[1], 0xD3);
        assert_eq!(&ntp_resp[8..16], &ntp_req[24..32], "reference timestamp must echo client send ts");

        // 3. SETUP Stage 2 (video 110 + audio 96) + RECORD + GET_PARAMETER + POST /feedback
        let mut st_v = Dictionary::new();
        st_v.insert("type".into(), Value::from(110u64));
        st_v.insert("streamConnectionID".into(), Value::from(424242u64));
        let mut st_a = Dictionary::new();
        st_a.insert("type".into(), Value::from(96u64));
        let mut s2 = Dictionary::new();
        s2.insert("streams".into(), Value::Array(vec![Value::Dictionary(st_v), Value::Dictionary(st_a)]));
        let s2_bp = plist::to_bytes_bplist00(&Value::Dictionary(s2)).unwrap();
        assert_eq!(send_rtsp(&mut rtsp, "SETUP", "rtsp://127.0.0.1/1", 4, Some("application/x-apple-binary-plist"), &s2_bp).0, 200);
        assert_eq!(send_rtsp(&mut rtsp, "RECORD", "rtsp://127.0.0.1/1", 5, None, &[]).0, 200);
        let (code, vol_body) = send_rtsp(&mut rtsp, "GET_PARAMETER", "rtsp://127.0.0.1/1", 6, Some("text/parameters"), b"volume\r\n");
        assert_eq!(code, 200);
        assert!(String::from_utf8_lossy(&vol_body).contains("volume:"));
        assert_eq!(send_rtsp(&mut rtsp, "POST", "/feedback", 7, None, &[]).0, 200);

        // 4. Per-stream TEARDOWN of audio (type=96) MUST keep video (type=110) and RTSP alive!
        let mut td_a = Dictionary::new();
        td_a.insert("type".into(), Value::from(96u64));
        let mut td = Dictionary::new();
        td.insert("streams".into(), Value::Array(vec![Value::Dictionary(td_a)]));
        let td_bp = plist::to_bytes_bplist00(&Value::Dictionary(td)).unwrap();
        assert_eq!(send_rtsp(&mut rtsp, "TEARDOWN", "rtsp://127.0.0.1/1", 8, Some("application/x-apple-binary-plist"), &td_bp).0, 200);

        // Subsequent POST /feedback on the EXACT same RTSP TCP socket MUST still succeed (no Click-#1 drop!)
        assert_eq!(send_rtsp(&mut rtsp, "POST", "/feedback", 9, None, &[]).0, 200);

        drop(ev_tcp);
        drop(rtsp);
        let _ = crate::mirror::global_stop_screen();
    }

    #[test]
    fn test_tahap254_airplay_mirror_keeps_ctr_sync_across_odd_frames_kind5_and_idle_timeout() {
        let _ = crate::mirror::global_stop_screen();

        let status_json = crate::mirror::start_airplay_mirror_session(
            Some("00008150-TEST-DEV-A"),
            false,
            "wifi",
            Some("198.51.100.14"),
            Some(0),
            false,
        )
        .expect("AirPlay standalone session must start");
        let rtsp_port: u16 = status_json
            .split(r#""airplayPort":"#)
            .nth(1)
            .and_then(|s| s.split([',', '}']).next())
            .and_then(|n| n.trim().parse().ok())
            .expect("extract airplayPort");

        let mut rtsp = TcpStream::connect(("127.0.0.1", rtsp_port)).expect("connect RTSP");
        rtsp.set_read_timeout(Some(Duration::from_secs(3))).unwrap();

        // FairPlay v3 Stage 1 & 2
        let fp1 = [
            0x46, 0x50, 0x4C, 0x59, 0x03, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x04, 0x02, 0x00,
            0x02, 0xBB,
        ];
        assert_eq!(
            send_rtsp(&mut rtsp, "POST", "/fp-setup", 1, Some("application/octet-stream"), &fp1).0,
            200
        );
        let mut fp2 = vec![0x46, 0x50, 0x4C, 0x59, 0x03];
        fp2.resize(164, 0);
        assert_eq!(
            send_rtsp(&mut rtsp, "POST", "/fp-setup", 2, Some("application/octet-stream"), &fp2).0,
            200
        );

        let ekey: Vec<u8> = (0u8..72u8).collect();
        let mut s1 = Dictionary::new();
        s1.insert("ekey".into(), Value::Data(ekey.clone()));
        s1.insert("eiv".into(), Value::Data(vec![0x55u8; 16]));
        let s1_bp = plist::to_bytes_bplist00(&Value::Dictionary(s1)).unwrap();
        assert_eq!(
            send_rtsp(
                &mut rtsp,
                "SETUP",
                "rtsp://127.0.0.1/1",
                3,
                Some("application/x-apple-binary-plist"),
                &s1_bp,
            )
            .0,
            200
        );

        let conn_id: u64 = 0x2233_4455_6677_8899;
        let mut st_v = Dictionary::new();
        st_v.insert("type".into(), Value::from(110u64));
        st_v.insert("streamConnectionID".into(), Value::from(conn_id));
        let mut s2 = Dictionary::new();
        s2.insert("streams".into(), Value::Array(vec![Value::Dictionary(st_v)]));
        let s2_bp = plist::to_bytes_bplist00(&Value::Dictionary(s2)).unwrap();
        let (code, s2_resp) = send_rtsp(
            &mut rtsp,
            "SETUP",
            "rtsp://127.0.0.1/1",
            4,
            Some("application/x-apple-binary-plist"),
            &s2_bp,
        );
        assert_eq!(code, 200);
        let s2_val = plist::from_bytes(&s2_resp).unwrap();
        let data_port = s2_val
            .as_dictionary()
            .unwrap()
            .get("streams")
            .unwrap()
            .as_array()
            .unwrap()[0]
            .as_dictionary()
            .unwrap()
            .get("dataPort")
            .unwrap()
            .as_unsigned_integer()
            .unwrap() as u16;

        let fp_key: [u8; 16] = hex_decode("299464065e68f7ed2125fcdd9a06f983")
            .try_into()
            .unwrap();
        let aes = session_key(&fp_key, None);
        let (skey, siv) = stream_key_iv(&aes, conn_id);
        let mut sender_ctr = crate::win_cng::Aes128Ctr::new(&skey, &siv).unwrap();

        let mut vid = TcpStream::connect(("127.0.0.1", data_port)).expect("connect dataPort");
        let write_pkt = |s: &mut TcpStream, kind: u8, body: &[u8]| {
            let mut hdr = [0u8; 128];
            hdr[0..4].copy_from_slice(&(body.len() as u32).to_le_bytes());
            hdr[4] = kind;
            if kind == 1 {
                hdr[56..60].copy_from_slice(&(1170.0f32).to_le_bytes());
                hdr[60..64].copy_from_slice(&(2532.0f32).to_le_bytes());
            }
            s.write_all(&hdr).unwrap();
            if !body.is_empty() {
                s.write_all(body).unwrap();
            }
            s.flush().unwrap();
        };

        // 1. Send unencrypted avcC (kind=1)
        let sps = vec![0x67u8, 0x42, 0xC0, 0x1F, 0xF4, 0x02, 0x80, 0x2D, 0xD0];
        let pps = vec![0x68u8, 0xCE, 0x38, 0x80];
        let mut avcc = vec![0x01, 0x42, 0xC0, 0x1F, 0xFF, 0xE1, 0x00, sps.len() as u8];
        avcc.extend_from_slice(&sps);
        avcc.push(0x01);
        avcc.extend_from_slice(&[0x00, pps.len() as u8]);
        avcc.extend_from_slice(&pps);
        write_pkt(&mut vid, 1, &avcc);

        // 2. Send odd-length (193 B!) encrypted IDR (kind=0)
        let mut idr_nal = vec![0x65u8, 0x88, 0x84, 0x00];
        idr_nal.resize(189, 0xAB); // 4 + 189 = 193 bytes (193 % 16 == 1!)
        let mut idr_au = Vec::with_capacity(193);
        idr_au.extend_from_slice(&(idr_nal.len() as u32).to_be_bytes());
        idr_au.extend_from_slice(&idr_nal);
        assert_eq!(idr_au.len(), 193);
        sender_ctr.apply_keystream(&mut idr_au).unwrap();
        write_pkt(&mut vid, 0, &idr_au);

        // 3. Real idle pause (350 ms) — forces multiple 50ms select() wakeups on Windows!
        std::thread::sleep(Duration::from_millis(350));

        // 4. Send unencrypted kind=5 streaming report (64 B) + kind=2 heartbeat (0 B) — MUST NOT shift CTR!
        write_pkt(&mut vid, 5, &[0x55u8; 64]);
        write_pkt(&mut vid, 2, &[]);

        // 5. Send 25 odd-length delta frames (len % 16 != 0) mixing SEI (6), AUD (9), Filler (12), Non-IDR (1), & 0x00 tail
        for idx in 1u8..=25u8 {
            let mut au = Vec::new();
            if idx % 3 == 0 {
                let aud = [0x09u8, 0x30];
                au.extend_from_slice(&(aud.len() as u32).to_be_bytes());
                au.extend_from_slice(&aud);
            }
            if idx % 4 == 0 {
                let sei = [0x06u8, 0x05, 0x02, idx, 0x80];
                au.extend_from_slice(&(sei.len() as u32).to_be_bytes());
                au.extend_from_slice(&sei);
            }
            if idx % 5 == 0 {
                let filler = [0x0Cu8, 0xFF, 0xFF, 0x80];
                au.extend_from_slice(&(filler.len() as u32).to_be_bytes());
                au.extend_from_slice(&filler);
            }
            // Choose odd slice length so total AU length is NOT a multiple of 16
            let mut slice_len = 25usize + (idx as usize * 7) % 103;
            let mut slice_nal = vec![0x41u8, 0x9A, idx];
            slice_nal.resize(slice_len, idx.wrapping_mul(19));
            au.extend_from_slice(&(slice_nal.len() as u32).to_be_bytes());
            au.extend_from_slice(&slice_nal);
            if au.len() % 16 == 0 {
                // Append 1 extra byte to slice so len % 16 != 0
                let pos = au.len() - slice_nal.len() - 4;
                slice_len += 1;
                au[pos..pos + 4].copy_from_slice(&(slice_len as u32).to_be_bytes());
                au.push(0x33);
            }
            if idx % 7 == 0 {
                // Append trailing zero-padding bytes (tolerated by valid_avcc_au) while keeping len % 16 != 0
                let pad_n = if (au.len() + 2) % 16 == 0 { 3 } else { 2 };
                au.extend(std::iter::repeat(0u8).take(pad_n));
            }
            assert_ne!(au.len() % 16, 0, "delta #{idx} must have odd/unaligned length");
            sender_ctr.apply_keystream(&mut au).unwrap();
            write_pkt(&mut vid, 0, &au);
        }

        // Wait up to 2s for all 26 video frames to process cleanly
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let (vframes, dframes, dfails, pkts, alive) = loop {
            let snap = {
                let g = crate::mirror::mirror_runtime().lock().unwrap();
                (
                    g.airplay_video_frames,
                    g.airplay_delta_frames,
                    g.airplay_decrypt_failures,
                    g.airplay_tcp_packets_read,
                    g.airplay_mirror_loop_count > 0,
                )
            };
            if snap.0 >= 26 || std::time::Instant::now() >= deadline {
                break snap;
            }
            std::thread::sleep(Duration::from_millis(15));
        };

        assert_eq!(vframes, 26, "Expected 1 IDR + 25 odd deltas = 26 frames, got {vframes}");
        assert_eq!(dframes, 25, "Expected 25 delta frames, got {dframes}");
        assert_eq!(dfails, 0, "Expected 0 decrypt failures across odd CTR + idle + kind=5, got {dfails}");
        assert!(alive, "airplayMirrorLoopAlive must remain true while TCP socket is open!");
        assert!(pkts >= 29, "Expected >= 29 TCP packets (avcC+IDR+kind5+kind2+25 deltas), got {pkts}");

        drop(vid);
        drop(rtsp);
        let _ = crate::mirror::global_stop_screen();
    }

    #[test]
    fn test_tahap254_live_stream_delivers_realtime_post_connect_deltas_after_late_joiner() {
        let _ = crate::mirror::global_stop_screen();

        let _ = crate::mirror::start_airplay_mirror_session(
            Some("00008150-TEST-DEV-A"),
            false,
            "wifi",
            Some("198.51.100.15"),
            Some(0),
            false,
        )
        .expect("AirPlay session must start");
        let gen = crate::mirror::current_live_session_gen();

        // 1. Push 1 IDR + 4 deltas BEFORE late-joining `/mirror/live` client connects
        assert!(crate::mirror::global_on_physical_h264_avcc_frame(
            gen,
            vec![0, 0, 0, 5, 0x65, 0x88, 0x01, 0x02, 0x03],
            true,
        ));
        for d in 1u8..=4u8 {
            assert!(crate::mirror::global_on_physical_h264_avcc_frame(
                gen,
                vec![0, 0, 0, 5, 0x41, 0x9A, d, 0x11, 0x22],
                false,
            ));
        }

        // 2. Late-joining client receives initial batch (5 frames: IDR + 4 deltas)
        let mut last_seq = 0u64;
        let mut sent_init = false;
        let init_batch =
            crate::mirror::global_wait_next_live_chunks(&mut last_seq, &mut sent_init, 500)
                .expect("init batch must arrive");
        assert_eq!(init_batch.len(), 5, "Late-joiner must receive IDR + 4 pre-connect deltas");
        assert_eq!(init_batch[0][4], 0, "Chunk #0 must be IDR");

        // 3. Producer thread pushes 15 LIVE post-connect deltas every 25ms + 1 mid-session avcC re-send
        let producer = std::thread::spawn(move || {
            for d in 5u8..=19u8 {
                std::thread::sleep(Duration::from_millis(25));
                if d == 11 {
                    // Simulate iOS re-sending avcC mid-stream (closing Control Center / rotating)
                    crate::mirror::update_live_codec_h264_avcc(
                        gen,
                        "avc1.640028",
                        "AQIDBAUGBw==",
                        1170,
                        2532,
                    );
                }
                assert!(crate::mirror::global_on_physical_h264_avcc_frame(
                    gen,
                    vec![0, 0, 0, 5, 0x41, 0x9A, d, 0x33, 0x44],
                    false,
                ));
            }
        });

        // 4. Reader loop receives all 15 post-connect deltas in real time (total 20/20!)
        let mut total_received = init_batch.len();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while total_received < 20 && std::time::Instant::now() < deadline {
            let batch =
                crate::mirror::global_wait_next_live_chunks(&mut last_seq, &mut sent_init, 150)
                    .expect("live broadcast must stay active across mid-session avcC re-send");
            for chunk in batch {
                assert_eq!(chunk[4], 1, "Post-connect chunks must be delta (type=1)");
                total_received += 1;
            }
        }
        producer.join().unwrap();

        assert_eq!(
            total_received, 20,
            "Live `/mirror/live` reader must receive ALL 20/20 frames (5 init + 15 live post-connect), got {total_received}"
        );
        let _ = crate::mirror::global_stop_screen();
    }

    #[test]
    fn test_tahap255_gate_only_never_corrupts_airplay_live_broadcast_or_codec_when_rtp_arrives() {
        let _ = crate::mirror::global_stop_screen();

        let _ = crate::mirror::start_airplay_mirror_session(
            Some("00008150-TEST-DEV-G"),
            false,
            "wifi",
            None,
            Some(0),
            false,
        )
        .expect("start AirPlay session");
        let gen = crate::mirror::current_live_session_gen();

        // 1. AirPlay registers avc1.640028 + IDR frame #1
        crate::mirror::update_live_codec_h264_avcc(
            gen,
            "avc1.640028",
            "AWQAKP/hAAlnQsAf9AKALdABAARozo=",
            1170,
            2532,
        );
        assert!(crate::mirror::global_on_physical_h264_avcc_frame(
            gen,
            vec![0, 0, 0, 5, 0x65, 0x88, 0x01, 0x02, 0x03],
            true,
        ));

        // 2. Simulate 20 unexpected HEVC RTP packets (VPS/SPS/PPS + IDR) arriving on a `gate_only = true` worker
        let mut depack = idevice::core_device::HevcDepacketizer::new();
        let mut au = Vec::new();
        let mut codec_ready = false;
        let mut sent_first_key = false;
        let fake_vps = [0x40u8, 0x01, 0x0C, 0x01, 0xFF, 0xFF, 0x01, 0x60];
        let fake_sps = [0x42u8, 0x01, 0x01, 0x01, 0x60, 0x00, 0x00, 0x03];
        let fake_pps = [0x44u8, 0x01, 0xC1, 0x72, 0xB4, 0x62, 0x40];
        let fake_idr = [0x26u8, 0x01, 0xAF, 0x08, 0x01, 0x02, 0x03, 0x04];
        for i in 0u16..20u16 {
            let payload: &[u8] = match i % 4 {
                0 => &fake_vps,
                1 => &fake_sps,
                2 => &fake_pps,
                _ => &fake_idr,
            };
            let pushed = crate::physical_stream::process_rtp_video_packet_guarded(
                true,
                gen,
                &mut depack,
                &mut au,
                &mut codec_ready,
                &mut sent_first_key,
                1000 + i,
                90000 + (i as u32) * 1500,
                payload,
                i % 4 == 3,
            );
            assert!(!pushed, "gate_only MUST silent-drain RTP packet #{i} and never push HEVC frames");
        }
        assert!(!codec_ready, "gate_only MUST NEVER populate HEVC codec_ready");
        assert!(!sent_first_key, "gate_only MUST NEVER touch sent_first_key");
        assert!(depack.sps.is_none(), "gate_only MUST NEVER parse HEVC SPS");

        // 3. Verify CodecHeader is STILL strictly avc1.640028 (NEVER overwritten to hev1!)
        let codec_json = crate::mirror::global_codec_json();
        assert!(
            codec_json.contains(r#""codec":"avc1.640028""#),
            "CodecHeader must remain avc1.640028, got {codec_json}"
        );
        assert!(
            codec_json.contains(r#""isAirplay":true"#),
            "isAirplay must remain true, got {codec_json}"
        );
        assert!(
            !codec_json.contains("hev1") && !codec_json.contains("hvc1"),
            "gate_only MUST NEVER leak hev1/hvc1 into CodecHeader: {codec_json}"
        );

        // 4. Verify LIVE_BROADCAST generation is untouched and delivers subsequent AirPlay deltas cleanly
        assert_eq!(crate::mirror::current_live_session_gen(), gen);
        let mut last_seq = 0u64;
        let mut sent_init = false;
        let init_batch =
            crate::mirror::global_wait_next_live_chunks(&mut last_seq, &mut sent_init, 300)
                .expect("init IDR must arrive");
        assert_eq!(init_batch.len(), 1);
        for d in 1u8..=5u8 {
            assert!(crate::mirror::global_on_physical_h264_avcc_frame(
                gen,
                vec![0, 0, 0, 5, 0x41, 0x9A, d, 0x10, 0x20],
                false,
            ));
        }
        let delta_batch =
            crate::mirror::global_wait_next_live_chunks(&mut last_seq, &mut sent_init, 300)
                .expect("post-gate_only AirPlay deltas must still flow cleanly");
        assert_eq!(delta_batch.len(), 5, "All 5 AirPlay deltas must be delivered");

        let _ = crate::mirror::global_stop_screen();
    }

    #[test]
    fn test_tahap255_wsapoll_layout_and_100_fragmented_multikb_airplay_frames_zero_drop() {
        let _ = crate::mirror::global_stop_screen();

        #[cfg(windows)]
        {
            assert_eq!(
                std::mem::size_of::<WsaPollFd>(),
                16,
                "Win64 WSAPOLLFD struct size must be exactly 16 bytes"
            );
            assert_eq!(POLLRDNORM, 0x0100);
        }

        let _ = crate::mirror::start_airplay_mirror_session(
            Some("00008150-TEST-DEV-W"),
            false,
            "wifi",
            Some("198.51.100.33"),
            Some(0),
            false,
        )
        .expect("start AirPlay session");
        let port = active_airplay_port();
        assert!(port > 0);

        let mut rtsp = TcpStream::connect(("127.0.0.1", port)).expect("connect RTSP");
        rtsp.set_read_timeout(Some(Duration::from_secs(4))).unwrap();

        // FairPlay v3 Stage 1 & 2 + SETUP Stage 1 & 2
        let fp1_req = [
            0x46, 0x50, 0x4C, 0x59, 0x03, 0x01, 0x01, 0x00,
            0x00, 0x00, 0x00, 0x04, 0x02, 0x00, 0x02, 0xBB,
        ];
        assert_eq!(
            send_rtsp(&mut rtsp, "POST", "/fp-setup", 1, Some("application/octet-stream"), &fp1_req).0,
            200
        );
        let mut fp2_req = vec![0x46, 0x50, 0x4C, 0x59, 0x03];
        fp2_req.resize(164, 0);
        assert_eq!(
            send_rtsp(&mut rtsp, "POST", "/fp-setup", 2, Some("application/octet-stream"), &fp2_req).0,
            200
        );

        let mut s1 = Dictionary::new();
        s1.insert("ekey".into(), Value::Data((0u8..72u8).collect()));
        s1.insert("eiv".into(), Value::Data(vec![0x77u8; 16]));
        let s1_bp = plist::to_bytes_bplist00(&Value::Dictionary(s1)).unwrap();
        assert_eq!(
            send_rtsp(&mut rtsp, "SETUP", "rtsp://198.51.100.33/99", 3, Some("application/x-apple-binary-plist"), &s1_bp).0,
            200
        );

        let conn_id: u64 = 555_444_333_222_111;
        let mut st_v = Dictionary::new();
        st_v.insert("type".into(), Value::from(110u64));
        st_v.insert("streamConnectionID".into(), Value::from(conn_id));
        let mut s2 = Dictionary::new();
        s2.insert("streams".into(), Value::Array(vec![Value::Dictionary(st_v)]));
        let s2_bp = plist::to_bytes_bplist00(&Value::Dictionary(s2)).unwrap();
        let (code, s2_resp) = send_rtsp(
            &mut rtsp,
            "SETUP",
            "rtsp://198.51.100.33/99",
            4,
            Some("application/x-apple-binary-plist"),
            &s2_bp,
        );
        assert_eq!(code, 200);
        let s2_val = plist::from_bytes(&s2_resp).unwrap();
        let data_port = s2_val
            .as_dictionary().unwrap()
            .get("streams").unwrap().as_array().unwrap()[0]
            .as_dictionary().unwrap()
            .get("dataPort").unwrap().as_unsigned_integer().unwrap() as u16;

        let fp_key: [u8; 16] = hex_decode("299464065e68f7ed2125fcdd9a06f983")
            .try_into()
            .unwrap();
        let aes = session_key(&fp_key, None);
        let (skey, siv) = stream_key_iv(&aes, conn_id);
        let mut sender_ctr = crate::win_cng::Aes128Ctr::new(&skey, &siv).unwrap();

        let mut vid = TcpStream::connect(("127.0.0.1", data_port)).expect("connect dataPort");
        vid.set_nodelay(true).ok();

        let write_fragmented = |s: &mut TcpStream, raw: &[u8], frag_size: usize| {
            let mut off = 0usize;
            while off < raw.len() {
                let end = (off + frag_size).min(raw.len());
                s.write_all(&raw[off..end]).unwrap();
                off = end;
            }
        };

        // 1. Send avcC (kind=1)
        let sps = vec![0x67u8, 0x64, 0x00, 0x28, 0xAC, 0xD9, 0x40, 0x78, 0x02, 0x27];
        let pps = vec![0x68u8, 0xEE, 0x3C, 0x80];
        let mut avcc = vec![0x01, 0x64, 0x00, 0x28, 0xFF, 0xE1, 0x00, sps.len() as u8];
        avcc.extend_from_slice(&sps);
        avcc.push(0x01);
        avcc.extend_from_slice(&[0x00, pps.len() as u8]);
        avcc.extend_from_slice(&pps);
        let mut hdr1 = [0u8; 128];
        hdr1[0..4].copy_from_slice(&(avcc.len() as u32).to_le_bytes());
        hdr1[4] = 1;
        hdr1[56..60].copy_from_slice(&(1170.0f32).to_le_bytes());
        hdr1[60..64].copy_from_slice(&(2532.0f32).to_le_bytes());
        write_fragmented(&mut vid, &hdr1, 37);
        write_fragmented(&mut vid, &avcc, 13);

        // 2. Send 100 multi-KB odd-length encrypted H.264 frames (4 KB .. 48 KB) split into small TCP chunks
        let frag_sizes = [137usize, 509usize, 1021usize];
        for idx in 0usize..100usize {
            if idx == 40 {
                // Mid-stream idle pause (> 250 ms) forcing multiple WSAPoll timeouts + kind=5 & kind=2 keepalive
                std::thread::sleep(Duration::from_millis(260));
                let mut h5 = [0u8; 128];
                h5[0..4].copy_from_slice(&48u32.to_le_bytes());
                h5[4] = 5;
                vid.write_all(&h5).unwrap();
                vid.write_all(&[0x42u8; 48]).unwrap();
                let mut h2 = [0u8; 128];
                h2[4] = 2;
                vid.write_all(&h2).unwrap();
            }

            let is_idr = idx == 0 || idx == 50;
            // Odd payload length between 4 KB and 48 KB (never a multiple of 16!)
            let mut slice_len = 4099 + (idx * 439) % 44000;
            if (slice_len + 4) % 16 == 0 {
                slice_len += 3;
            }
            let mut nal = Vec::with_capacity(slice_len);
            nal.push(if is_idr { 0x65 } else { 0x41 });
            nal.push(0x9A);
            nal.push((idx & 0xFF) as u8);
            nal.resize(slice_len, (idx.wrapping_mul(29) & 0xFF) as u8);

            let mut au = Vec::with_capacity(4 + nal.len());
            au.extend_from_slice(&(nal.len() as u32).to_be_bytes());
            au.extend_from_slice(&nal);
            assert_ne!(au.len() % 16, 0);

            sender_ctr.apply_keystream(&mut au).unwrap();
            let mut hdr0 = [0u8; 128];
            hdr0[0..4].copy_from_slice(&(au.len() as u32).to_le_bytes());
            hdr0[4] = 0;

            let fsz = frag_sizes[idx % frag_sizes.len()];
            write_fragmented(&mut vid, &hdr0, 41);
            write_fragmented(&mut vid, &au, fsz);
        }
        vid.flush().unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let (vframes, dframes, dfails, alive) = loop {
            let snap = {
                let g = crate::mirror::mirror_runtime().lock().unwrap();
                (
                    g.airplay_video_frames,
                    g.airplay_delta_frames,
                    g.airplay_decrypt_failures,
                    g.airplay_mirror_loop_count > 0,
                )
            };
            if snap.0 >= 100 || std::time::Instant::now() >= deadline {
                break snap;
            }
            std::thread::sleep(Duration::from_millis(15));
        };

        assert_eq!(vframes, 100, "Must receive ALL 100/100 fragmented multi-KB frames, got {vframes}");
        assert_eq!(dframes, 98, "Must receive 98 delta frames (idx 0 & 50 are IDRs), got {dframes}");
        assert_eq!(dfails, 0, "Must have ZERO decrypt failures across 100 multi-KB fragmented frames, got {dfails}");
        assert!(alive, "airplayMirrorLoopAlive must remain true while TCP socket is open");

        // 3. Verify In-Memory Flight Recorder contains sanitized events & ZERO PII (no IP/UDID!)
        let trace_json = get_airplay_trace_json();
        assert!(trace_json.contains(r#""ok":true"#), "trace={trace_json}");
        assert!(trace_json.contains("pkt_k0"), "trace must record kind=0 video frames: {trace_json}");
        assert!(trace_json.contains("pkt_k1") || trace_json.contains("live_push_seq"), "trace={trace_json}");
        assert!(
            !trace_json.contains("198.51.100.33") && !trace_json.contains("00008150-TEST-DEV-W"),
            "Flight Recorder MUST contain ZERO IP/UDID PII: {trace_json}"
        );

        drop(vid);
        drop(rtsp);
        let _ = crate::mirror::global_stop_screen();
    }

    #[test]
    fn test_tahap256_airplay_setup_type96_audio_and_teardown_type96_preserves_video_and_rtsp() {
        let _ = crate::mirror::global_stop_screen();
        let _ = crate::mirror::start_airplay_mirror_session(
            Some("00008150-TEST-DEV-A"),
            false,
            "wifi",
            Some("198.51.100.51"),
            Some(0),
            false,
        )
        .expect("start AirPlay session");
        let port = active_airplay_port();
        let mut rtsp = TcpStream::connect(("127.0.0.1", port)).expect("connect RTSP");
        rtsp.set_read_timeout(Some(Duration::from_secs(4))).unwrap();

        // Exact iOS 27.2 sequence: FairPlay -> SETUP s1 -> GET /info -> RECORD -> SETUP 110 -> SETUP 96
        let fp1 = [
            0x46, 0x50, 0x4C, 0x59, 0x03, 0x01, 0x01, 0x00,
            0x00, 0x00, 0x00, 0x04, 0x02, 0x00, 0x02, 0xBB,
        ];
        assert_eq!(send_rtsp(&mut rtsp, "POST", "/fp-setup", 1, Some("application/octet-stream"), &fp1).0, 200);
        let mut fp2 = vec![0x46, 0x50, 0x4C, 0x59, 0x03];
        fp2.resize(164, 0);
        assert_eq!(send_rtsp(&mut rtsp, "POST", "/fp-setup", 2, Some("application/octet-stream"), &fp2).0, 200);

        let client_ntp = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let client_ntp_port = client_ntp.local_addr().unwrap().port();
        let mut s1 = Dictionary::new();
        s1.insert("ekey".into(), Value::Data((0u8..72u8).collect()));
        s1.insert("eiv".into(), Value::Data(vec![0x55u8; 16]));
        s1.insert("timingPort".into(), Value::from(client_ntp_port as u64));
        s1.insert("timingProtocol".into(), Value::from("NTP"));
        let (code, s1_resp) = send_rtsp(
            &mut rtsp,
            "SETUP",
            "rtsp://198.51.100.51/1",
            3,
            Some("application/x-apple-binary-plist"),
            &plist::to_bytes_bplist00(&Value::Dictionary(s1)).unwrap(),
        );
        assert_eq!(code, 200);
        let s1_d = plist::from_bytes(&s1_resp).unwrap().into_dictionary().unwrap();
        let timing_port = s1_d.get("timingPort").unwrap().as_unsigned_integer().unwrap() as u16;
        let event_port = s1_d.get("eventPort").unwrap().as_unsigned_integer().unwrap() as u16;
        assert!(timing_port > 0 && event_port > 0);

        let mut ev_tcp = TcpStream::connect(("127.0.0.1", event_port)).expect("connect eventPort");
        ev_tcp.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        ev_tcp
            .write_all(b"POST /event RTSP/1.0\r\nCSeq: 1\r\nContent-Length: 0\r\n\r\n")
            .unwrap();
        let mut ev_resp = [0u8; 256];
        let ev_n = ev_tcp.read(&mut ev_resp).expect("eventPort must reply 200 OK to framed RTSP request");
        assert!(
            String::from_utf8_lossy(&ev_resp[..ev_n]).starts_with("RTSP/1.0 200 OK"),
            "unexpected eventPort reply"
        );

        assert_eq!(send_rtsp(&mut rtsp, "GET", "/info", 4, None, &[]).0, 200);
        assert_eq!(send_rtsp(&mut rtsp, "RECORD", "rtsp://198.51.100.51/1", 5, None, &[]).0, 200);

        // SETUP type=110 (video)
        let conn_id: u64 = 999_888_777_666;
        let mut st_v = Dictionary::new();
        st_v.insert("type".into(), Value::from(110u64));
        st_v.insert("streamConnectionID".into(), Value::from(conn_id));
        let mut s2v = Dictionary::new();
        s2v.insert("streams".into(), Value::Array(vec![Value::Dictionary(st_v)]));
        let (code, s2v_resp) = send_rtsp(
            &mut rtsp,
            "SETUP",
            "rtsp://198.51.100.51/1",
            6,
            Some("application/x-apple-binary-plist"),
            &plist::to_bytes_bplist00(&Value::Dictionary(s2v)).unwrap(),
        );
        assert_eq!(code, 200);
        let video_port = plist::from_bytes(&s2v_resp)
            .unwrap()
            .as_dictionary().unwrap()
            .get("streams").unwrap().as_array().unwrap()[0]
            .as_dictionary().unwrap()
            .get("dataPort").unwrap().as_unsigned_integer().unwrap() as u16;

        // SETUP type=96 (audio) -> returns both dataPort & controlPort, both UDP sockets open
        let mut st_a = Dictionary::new();
        st_a.insert("type".into(), Value::from(96u64));
        st_a.insert("ct".into(), Value::from(8u64));
        st_a.insert("spf".into(), Value::from(480u64));
        let mut s2a = Dictionary::new();
        s2a.insert("streams".into(), Value::Array(vec![Value::Dictionary(st_a)]));
        let (code, s2a_resp) = send_rtsp(
            &mut rtsp,
            "SETUP",
            "rtsp://198.51.100.51/1",
            7,
            Some("application/x-apple-binary-plist"),
            &plist::to_bytes_bplist00(&Value::Dictionary(s2a)).unwrap(),
        );
        assert_eq!(code, 200);
        let s2a_st = plist::from_bytes(&s2a_resp)
            .unwrap()
            .as_dictionary().unwrap()
            .get("streams").unwrap().as_array().unwrap()[0]
            .as_dictionary().unwrap()
            .clone();
        let audio_dp = s2a_st.get("dataPort").unwrap().as_unsigned_integer().unwrap() as u16;
        let audio_cp = s2a_st.get("controlPort").unwrap().as_unsigned_integer().unwrap() as u16;
        assert!(audio_dp > 0 && audio_cp > 0);
        // Send 1 audio RTP + 1 control UDP packet to verify drain & trace
        let aud_sender = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let _ = aud_sender.send_to(&[0x80u8; 32], ("127.0.0.1", audio_dp));
        let _ = aud_sender.send_to(&[0x80u8, 0xD4, 0, 0, 0, 0, 0, 0], ("127.0.0.1", audio_cp));

        // Connect video TCP and send avcC + 10 encrypted video frames
        let fp_key: [u8; 16] = hex_decode("299464065e68f7ed2125fcdd9a06f983").try_into().unwrap();
        let aes = session_key(&fp_key, None);
        let (skey, siv) = stream_key_iv(&aes, conn_id);
        let mut enc = crate::win_cng::Aes128Ctr::new(&skey, &siv).unwrap();

        let mut vid = TcpStream::connect(("127.0.0.1", video_port)).expect("connect video dataPort");
        let sps = vec![0x67u8, 0x64, 0x00, 0x20, 0xAC, 0xD9, 0x40, 0x78];
        let pps = vec![0x68u8, 0xEE, 0x3C, 0x80];
        let mut avcc = vec![0x01, 0x64, 0x00, 0x20, 0xFF, 0xE1, 0x00, sps.len() as u8];
        avcc.extend_from_slice(&sps);
        avcc.push(0x01);
        avcc.extend_from_slice(&[0x00, pps.len() as u8]);
        avcc.extend_from_slice(&pps);
        let mut h1 = [0u8; 128];
        h1[0..4].copy_from_slice(&(avcc.len() as u32).to_le_bytes());
        h1[4] = 1;
        vid.write_all(&h1).unwrap();
        vid.write_all(&avcc).unwrap();

        let send_vid_frame = |vid: &mut TcpStream, enc: &mut crate::win_cng::Aes128Ctr, idx: u8, idr: bool| {
            let nal = vec![if idr { 0x65 } else { 0x41 }, 0x9A, idx, 0x11, 0x22, 0x33, 0x44];
            let mut au = Vec::new();
            au.extend_from_slice(&(nal.len() as u32).to_be_bytes());
            au.extend_from_slice(&nal);
            enc.apply_keystream(&mut au).unwrap();
            let mut h0 = [0u8; 128];
            h0[0..4].copy_from_slice(&(au.len() as u32).to_le_bytes());
            h0[4] = 0;
            vid.write_all(&h0).unwrap();
            vid.write_all(&au).unwrap();
        };
        for i in 0u8..10u8 {
            send_vid_frame(&mut vid, &mut enc, i, i == 0);
        }
        vid.flush().unwrap();

        // Send TEARDOWN {"streams":[{"type":96}]} (exact iOS 27.2 tMs=11405 event!)
        let mut td_a = Dictionary::new();
        td_a.insert("type".into(), Value::from(96u64));
        let mut td_body = Dictionary::new();
        td_body.insert("streams".into(), Value::Array(vec![Value::Dictionary(td_a)]));
        assert_eq!(
            send_rtsp(
                &mut rtsp,
                "TEARDOWN",
                "rtsp://198.51.100.51/1",
                8,
                Some("application/x-apple-binary-plist"),
                &plist::to_bytes_bplist00(&Value::Dictionary(td_body)).unwrap(),
            )
            .0,
            200
        );

        // RTSP, eventPort, timingPort, & video MUST ALL stay alive!
        assert_eq!(send_rtsp(&mut rtsp, "POST", "/feedback", 9, None, &[]).0, 200);
        ev_tcp.set_read_timeout(Some(Duration::from_millis(80))).unwrap();
        let mut probe = [0u8; 8];
        if let Ok(0) = ev_tcp.read(&mut probe) {
            panic!("eventPort MUST NOT close on TEARDOWN type=96!");
        }

        // Send 15 more video frames -> total 25/25 must flow cleanly with 0 failures!
        for i in 10u8..25u8 {
            send_vid_frame(&mut vid, &mut enc, i, false);
        }
        vid.flush().unwrap();

        let deadline = Instant::now() + Duration::from_secs(3);
        let (vframes, dfails, alive) = loop {
            let snap = {
                let g = crate::mirror::mirror_runtime().lock().unwrap();
                (g.airplay_video_frames, g.airplay_decrypt_failures, g.airplay_mirror_loop_count > 0)
            };
            if snap.0 >= 25 || Instant::now() >= deadline {
                break snap;
            }
            std::thread::sleep(Duration::from_millis(15));
        };
        assert_eq!(vframes, 25, "Video must keep flowing 25/25 after TEARDOWN type=96");
        assert_eq!(dfails, 0);
        assert!(alive, "airplayMirrorLoopAlive must stay true");

        let trace = get_airplay_trace_json();
        assert!(trace.contains("setup_s2_audio"), "missing setup_s2_audio: {trace}");
        assert!(trace.contains("event_accept"), "missing event_accept: {trace}");
        assert!(trace.contains("teardown_parsed"), "missing teardown_parsed: {trace}");
        assert!(trace.contains("rtsp_resp"), "missing rtsp_resp: {trace}");
        assert!(!trace.contains("198.51.100.51") && !trace.contains("00008150-TEST-DEV-A"));

        drop(ev_tcp);
        drop(vid);
        drop(rtsp);
        let _ = crate::mirror::global_stop_screen();
    }

    #[test]
    fn test_tahap256_airplay_ntp_timing_immediate_probe_reply_and_udp_connreset_immunity() {
        let _ = crate::mirror::global_stop_screen();
        let _ = crate::mirror::start_airplay_mirror_session(
            Some("00008150-TEST-DEV-N"),
            false,
            "wifi",
            None,
            Some(0),
            false,
        )
        .expect("start AirPlay session");
        let port = active_airplay_port();
        let mut rtsp = TcpStream::connect(("127.0.0.1", port)).expect("connect RTSP");
        rtsp.set_read_timeout(Some(Duration::from_secs(3))).unwrap();

        // Bind a client UDP socket to receive receiver's immediate NTP 0x80 0xD2 probe (< 250ms)
        let client_ntp = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let client_ntp_port = client_ntp.local_addr().unwrap().port();
        client_ntp.set_read_timeout(Some(Duration::from_millis(400))).unwrap();

        let fp1 = [
            0x46, 0x50, 0x4C, 0x59, 0x03, 0x01, 0x01, 0x00,
            0x00, 0x00, 0x00, 0x04, 0x02, 0x00, 0x02, 0xBB,
        ];
        assert_eq!(send_rtsp(&mut rtsp, "POST", "/fp-setup", 1, Some("application/octet-stream"), &fp1).0, 200);
        let mut fp2 = vec![0x46, 0x50, 0x4C, 0x59, 0x03];
        fp2.resize(164, 0);
        assert_eq!(send_rtsp(&mut rtsp, "POST", "/fp-setup", 2, Some("application/octet-stream"), &fp2).0, 200);

        let t0 = Instant::now();
        let mut s1 = Dictionary::new();
        s1.insert("ekey".into(), Value::Data((0u8..72u8).collect()));
        s1.insert("eiv".into(), Value::Data(vec![0x66u8; 16]));
        s1.insert("timingPort".into(), Value::from(client_ntp_port as u64));
        let (code, s1_resp) = send_rtsp(
            &mut rtsp,
            "SETUP",
            "rtsp://127.0.0.1/1",
            3,
            Some("application/x-apple-binary-plist"),
            &plist::to_bytes_bplist00(&Value::Dictionary(s1)).unwrap(),
        );
        assert_eq!(code, 200);
        let timing_port = plist::from_bytes(&s1_resp)
            .unwrap()
            .as_dictionary().unwrap()
            .get("timingPort").unwrap().as_unsigned_integer().unwrap() as u16;

        // 1. Receiver MUST send proactive NTP 0x80 0xD2 probe within < 250ms!
        let mut probe_buf = [0u8; 64];
        let (n, _) = client_ntp
            .recv_from(&mut probe_buf)
            .expect("receiver must proactively send 0x80 0xD2 NTP probe within 250ms");
        let elapsed_ms = t0.elapsed().as_millis();
        assert!(elapsed_ms < 350, "proactive NTP probe took too long: {elapsed_ms}ms");
        assert_eq!(n, 32);
        assert_eq!(&probe_buf[0..4], &[0x80, 0xD2, 0x00, 0x07]);
        let probe_secs = u32::from_be_bytes(probe_buf[24..28].try_into().unwrap());
        assert!(probe_secs > 3_800_000_000, "NTP epoch 1900 secs expected, got {probe_secs}");

        // 2. Verify disable_udp_connreset succeeds & receiver replies 0x80 0xD3 with copied originate ts
        assert!(disable_udp_connreset(&client_ntp));
        let mut req = [0u8; 32];
        req[0..4].copy_from_slice(&[0x80, 0xD2, 0x00, 0x07]);
        req[24..32].copy_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD, 0x11, 0x22, 0x33, 0x44]);
        client_ntp.send_to(&req, ("127.0.0.1", timing_port)).unwrap();

        let mut resp = [0u8; 64];
        let mut got_reply = false;
        for _ in 0..4 {
            if let Ok((rn, _)) = client_ntp.recv_from(&mut resp) {
                if rn == 32 && resp[1] == 0xD3 {
                    assert_eq!(&resp[0..4], &[0x80, 0xD3, 0x00, 0x07]);
                    assert_eq!(&resp[8..16], &req[24..32]);
                    let rx_secs = u32::from_be_bytes(resp[16..20].try_into().unwrap());
                    let tx_secs = u32::from_be_bytes(resp[24..28].try_into().unwrap());
                    assert!(rx_secs > 3_800_000_000 && tx_secs >= rx_secs);
                    got_reply = true;
                    break;
                }
            }
        }
        assert!(got_reply, "timingPort must reply 0x80 0xD3 to client NTP 0xD2 request");

        let trace = get_airplay_trace_json();
        assert!(trace.contains("ntp_tx") && trace.contains("ntp_rx"), "trace={trace}");

        drop(rtsp);
        let _ = crate::mirror::global_stop_screen();
    }

    #[test]
    fn test_tahap256_airplay_full_teardown_transitions_honestly_to_waiting_airplay_and_updates_note() {
        let _ = crate::mirror::global_stop_screen();
        let _ = crate::mirror::start_airplay_mirror_session(
            Some("00008150-TEST-DEV-T"),
            false,
            "wifi",
            None,
            Some(0),
            false,
        )
        .expect("start AirPlay session");
        let port = active_airplay_port();

        let mut rtsp = TcpStream::connect(("127.0.0.1", port)).expect("connect RTSP #1");
        rtsp.set_read_timeout(Some(Duration::from_secs(3))).unwrap();

        let fp1 = [
            0x46, 0x50, 0x4C, 0x59, 0x03, 0x01, 0x01, 0x00,
            0x00, 0x00, 0x00, 0x04, 0x02, 0x00, 0x02, 0xBB,
        ];
        assert_eq!(send_rtsp(&mut rtsp, "POST", "/fp-setup", 1, Some("application/octet-stream"), &fp1).0, 200);
        let mut fp2 = vec![0x46, 0x50, 0x4C, 0x59, 0x03];
        fp2.resize(164, 0);
        assert_eq!(send_rtsp(&mut rtsp, "POST", "/fp-setup", 2, Some("application/octet-stream"), &fp2).0, 200);

        let mut s1 = Dictionary::new();
        s1.insert("ekey".into(), Value::Data((0u8..72u8).collect()));
        s1.insert("eiv".into(), Value::Data(vec![0x77u8; 16]));
        assert_eq!(
            send_rtsp(&mut rtsp, "SETUP", "rtsp://127.0.0.1/1", 3, Some("application/x-apple-binary-plist"), &plist::to_bytes_bplist00(&Value::Dictionary(s1)).unwrap()).0,
            200
        );

        let conn_id: u64 = 777_666_555;
        let mut st_v = Dictionary::new();
        st_v.insert("type".into(), Value::from(110u64));
        st_v.insert("streamConnectionID".into(), Value::from(conn_id));
        let mut s2 = Dictionary::new();
        s2.insert("streams".into(), Value::Array(vec![Value::Dictionary(st_v)]));
        let (code, s2_resp) = send_rtsp(
            &mut rtsp,
            "SETUP",
            "rtsp://127.0.0.1/1",
            4,
            Some("application/x-apple-binary-plist"),
            &plist::to_bytes_bplist00(&Value::Dictionary(s2)).unwrap(),
        );
        assert_eq!(code, 200);
        let data_port = plist::from_bytes(&s2_resp)
            .unwrap()
            .as_dictionary().unwrap()
            .get("streams").unwrap().as_array().unwrap()[0]
            .as_dictionary().unwrap()
            .get("dataPort").unwrap().as_unsigned_integer().unwrap() as u16;

        // Send avcC + IDR Frame #1 -> verify phase=="running" AND note updated (NOT stale "Menunggu frame pertama...")!
        let fp_key: [u8; 16] = hex_decode("299464065e68f7ed2125fcdd9a06f983").try_into().unwrap();
        let aes = session_key(&fp_key, None);
        let (skey, siv) = stream_key_iv(&aes, conn_id);
        let mut enc = crate::win_cng::Aes128Ctr::new(&skey, &siv).unwrap();

        let mut vid = TcpStream::connect(("127.0.0.1", data_port)).expect("connect video");
        let sps = vec![0x67u8, 0x64, 0x00, 0x20, 0xAC, 0xD9, 0x40, 0x78];
        let pps = vec![0x68u8, 0xEE, 0x3C, 0x80];
        let mut avcc = vec![0x01, 0x64, 0x00, 0x20, 0xFF, 0xE1, 0x00, sps.len() as u8];
        avcc.extend_from_slice(&sps);
        avcc.push(0x01);
        avcc.extend_from_slice(&[0x00, pps.len() as u8]);
        avcc.extend_from_slice(&pps);
        let mut h1 = [0u8; 128];
        h1[0..4].copy_from_slice(&(avcc.len() as u32).to_le_bytes());
        h1[4] = 1;
        vid.write_all(&h1).unwrap();
        vid.write_all(&avcc).unwrap();

        let mut au = vec![0, 0, 0, 5, 0x65, 0x88, 0x01, 0x02, 0x03];
        enc.apply_keystream(&mut au).unwrap();
        let mut h0 = [0u8; 128];
        h0[0..4].copy_from_slice(&(au.len() as u32).to_le_bytes());
        h0[4] = 0;
        vid.write_all(&h0).unwrap();
        vid.write_all(&au).unwrap();
        vid.flush().unwrap();

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut st_run = String::new();
        while Instant::now() < deadline {
            st_run = crate::mirror::global_screen_status_json();
            if st_run.contains(r#""phase":"running""#) {
                break;
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        assert!(st_run.contains(r#""phase":"running""#), "st_run={st_run}");
        assert!(
            !st_run.contains("Menunggu frame pertama"),
            "Running AirPlay note MUST NOT stay stale: {st_run}"
        );
        assert!(st_run.contains("Streaming AirPlay H.264 (60fps)"), "st_run={st_run}");

        // Send full TEARDOWN ({} = 42B bplist00!) -> state must honestly transition back to waiting_airplay!
        let empty_bp = plist::to_bytes_bplist00(&Value::Dictionary(Dictionary::new())).unwrap();
        assert_eq!(empty_bp.len(), 42);
        assert_eq!(
            send_rtsp(&mut rtsp, "TEARDOWN", "rtsp://127.0.0.1/1", 5, Some("application/x-apple-binary-plist"), &empty_bp).0,
            200
        );
        drop(vid);
        drop(rtsp);

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut st_wait = String::new();
        while Instant::now() < deadline {
            st_wait = crate::mirror::global_screen_status_json();
            if st_wait.contains(r#""stage":"waiting_airplay""#) {
                break;
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        assert!(st_wait.contains(r#""phase":"starting""#), "Expected phase=starting after full TEARDOWN: {st_wait}");
        assert!(st_wait.contains(r#""stage":"waiting_airplay""#), "Expected stage=waiting_airplay: {st_wait}");
        assert!(st_wait.contains(r#""airplayActive":true"#), "AirPlay receiver must stay alive for reconnect: {st_wait}");

        // Verify RTSP server IMMEDIATELY accepts reconnect session #2 without Stop/Start on PC!
        let mut rtsp2 = TcpStream::connect(("127.0.0.1", port)).expect("connect RTSP #2 after full TEARDOWN");
        rtsp2.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        assert_eq!(send_rtsp(&mut rtsp2, "GET", "/info", 1, None, &[]).0, 200);

        drop(rtsp2);
        let _ = crate::mirror::global_stop_screen();
    }

    #[test]
    fn test_tahap257_airplay_mdns_and_info_features_flags_strict_consistency_no_hevc_no_ptp() {
        use std::net::Ipv4Addr;

        let id = AirPlayIdentity::from_seed_and_mac([0x42u8; 32], "02:11:22:33:44:55".into());
        let receiver = info::ReceiverInfo {
            id: &id,
            name: "iPhone Link",
            hevc: false,
        };

        // 1. Strict Apple TV 3 H.264 NTP-only feature mask:
        //    bit 42 (HEVC = 0x400 in hi) == 0, bit 40/41 (AirPlay 2 BufferedAudio/PTP) == 0!
        let (lo, hi) = info::feature_bits(false);
        assert_eq!(lo, 0x5A7F_FEE6);
        assert_eq!(hi, 0);
        let feat_u64 = ((hi as u64) << 32) | (lo as u64);
        assert_eq!(feat_u64 & (1u64 << 42), 0, "HEVC bit 42 must be 0");
        assert_eq!(feat_u64 & (1u64 << 40), 0, "BufferedAudio bit 40 must be 0");
        assert_eq!(feat_u64 & (1u64 << 41), 0, "PTP bit 41 must be 0");
        assert_eq!(receiver.features_str(), "0x5A7FFEE6,0x0");

        // 2. Verify mDNS TXT records (_airplay & _raop) match zenlink-airplay 1:1
        let ap_txt: std::collections::HashMap<_, _> = receiver.txt_airplay().into_iter().collect();
        assert_eq!(ap_txt.get("deviceid").map(String::as_str), Some("02:11:22:33:44:55"));
        assert_eq!(ap_txt.get("features").map(String::as_str), Some("0x5A7FFEE6,0x0"));
        assert_eq!(ap_txt.get("flags").map(String::as_str), Some("0x4"));
        assert_eq!(ap_txt.get("model").map(String::as_str), Some("AppleTV3,2"));
        assert_eq!(ap_txt.get("srcvers").map(String::as_str), Some("220.68"));
        assert_eq!(ap_txt.get("vv").map(String::as_str), Some("2"));
        assert_eq!(ap_txt.get("pi").map(String::as_str), Some(info::PI));
        assert_eq!(ap_txt.get("pk").map(String::len), Some(64));

        let raop_txt: std::collections::HashMap<_, _> = receiver.txt_raop().into_iter().collect();
        assert_eq!(raop_txt.get("ft").map(String::as_str), Some("0x5A7FFEE6,0x0"));
        assert_eq!(raop_txt.get("sf").map(String::as_str), Some("0x4"));
        assert_eq!(raop_txt.get("am").map(String::as_str), Some("AppleTV3,2"));
        assert_eq!(raop_txt.get("vs").map(String::as_str), Some("220.68"));
        assert_eq!(raop_txt.get("tp").map(String::as_str), Some("UDP"));
        assert_eq!(raop_txt.get("sr").map(String::as_str), Some("44100"));
        assert_eq!(raop_txt.get("ss").map(String::as_str), Some("16"));
        assert_eq!(raop_txt.get("ch").map(String::as_str), Some("2"));
        assert_eq!(raop_txt.get("cn").map(String::as_str), Some("0,1,2,3"));
        assert_eq!(raop_txt.get("pk"), ap_txt.get("pk"));

        // 3. Verify GET /info bplist00 consistency with mDNS TXT
        let bp = receiver.build_info_bplist(None, &[]);
        let val = plist::from_bytes(&bp).expect("valid GET /info bplist00");
        let d = val.as_dictionary().expect("info dict");
        assert_eq!(d.get("features").and_then(|v| v.as_unsigned_integer()), Some(feat_u64));
        assert_eq!(d.get("statusFlags").and_then(|v| v.as_unsigned_integer()), Some(68));
        assert_eq!(d.get("sourceVersion").and_then(|v| v.as_string()), Some("220.68"));
        assert_eq!(d.get("model").and_then(|v| v.as_string()), Some("AppleTV3,2"));
        assert_eq!(d.get("vv").and_then(|v| v.as_unsigned_integer()), Some(2));
        assert_eq!(d.get("hevc").and_then(|v| v.as_boolean()), Some(false));

        // 4. Verify mDNS announcement packet carries BOTH _airplay._tcp & _raop._tcp + neutral hostname
        let pkt = mdns::build_announcement_packet(&receiver, 7100, &[Ipv4Addr::new(192, 0, 2, 20)], false);
        assert_eq!(&pkt[2..4], &[0x84, 0x00]); // QR=1, AA=1
        assert!(pkt.windows(8).any(|w| w == b"_airplay"));
        assert!(pkt.windows(5).any(|w| w == b"_raop"));
        assert!(pkt.windows(11).any(|w| w == b"iphone-link"));
    }

    #[test]
    fn test_tahap257_airplay_setup_s1_parses_signed_unsigned_timing_port_and_sends_ntp_to_peer() {
        use plist::{Dictionary, Value};
        use std::net::{TcpStream, UdpSocket};
        use std::time::{Duration, Instant};

        struct NullSink;
        impl AirPlaySink for NullSink {
            fn on_state(&self, _: AirPlayState) {}
            fn on_codec(&self, _: &str, _: &str, _: u32, _: u32) {}
            fn on_frame(&self, _: Vec<u8>, _: bool) {}
        }

        reset_airplay_trace();

        let id = AirPlayIdentity::from_seed_and_mac([0x51u8; 32], "02:AA:BB:CC:DD:57".into());
        let sink: Arc<dyn AirPlaySink> = Arc::new(NullSink);
        let mut srv = AirPlayServer::start("iPhone Link", 0, id, sink, false).expect("start AirPlay");
        let port = srv.port();

        // Bind a real peer UDP socket to catch receiver's proactive 0x80 0xD2 NTP query!
        let peer_udp = UdpSocket::bind("127.0.0.1:0").expect("bind peer ntp udp");
        let peer_ntp_port = peer_udp.local_addr().unwrap().port();
        peer_udp.set_read_timeout(Some(Duration::from_millis(800))).unwrap();

        let mut rtsp = TcpStream::connect(("127.0.0.1", port)).expect("connect RTSP");
        rtsp.set_read_timeout(Some(Duration::from_secs(2))).unwrap();

        // Pair-verify + FairPlay v3 handshake
        let pv_body1 = [vec![1u8, 0, 0, 0], vec![0x33u8; 32], vec![0x44u8; 32]].concat();
        assert_eq!(send_rtsp(&mut rtsp, "POST", "/pair-verify", 1, Some("application/octet-stream"), &pv_body1).0, 200);
        assert_eq!(send_rtsp(&mut rtsp, "POST", "/pair-verify", 2, Some("application/octet-stream"), &[0u8; 68]).0, 200);
        let mut fp1 = [0u8; 16];
        fp1[..4].copy_from_slice(b"FPLY");
        fp1[4] = 3;
        fp1[5] = 1;
        fp1[14] = 0x02;
        assert_eq!(send_rtsp(&mut rtsp, "POST", "/fp-setup", 3, Some("application/octet-stream"), &fp1).0, 200);
        let mut fp2 = vec![0u8; 164];
        fp2[..4].copy_from_slice(b"FPLY");
        fp2[4] = 3;
        fp2[5] = 1;
        fp2[6] = 3;
        assert_eq!(send_rtsp(&mut rtsp, "POST", "/fp-setup", 4, Some("application/octet-stream"), &fp2).0, 200);

        // Build SETUP Stage 1 with SIGNED integer timingPort (just as Apple bplist encoders emit!)
        let mut s1 = Dictionary::new();
        s1.insert("ekey".into(), Value::Data(vec![0x11u8; 72]));
        s1.insert("eiv".into(), Value::Data(vec![0x22u8; 16]));
        s1.insert("timingProtocol".into(), Value::String("NTP".into()));
        s1.insert("timingPort".into(), Value::Integer(plist::Integer::Signed(peer_ntp_port as i64)));
        s1.insert("deviceID".into(), Value::String("SYNTH-HIDDEN-VALUE".into()));
        let s1_bp = plist::to_bytes_bplist00(&Value::Dictionary(s1)).unwrap();
        let (sc, body) = send_rtsp(&mut rtsp, "SETUP", "rtsp://127.0.0.1/1", 5, Some("application/x-apple-binary-plist"), &s1_bp);
        assert_eq!(sc, 200);
        let resp_val = plist::from_bytes(&body).unwrap();
        let resp_d = resp_val.as_dictionary().unwrap();
        let our_timing = resp_d.get("timingPort").and_then(|v| v.as_unsigned_integer()).unwrap() as u16;
        assert!(our_timing > 0);

        // 1. Receiver MUST proactively send 0x80 0xD2 NTP query to our peer_ntp_port!
        let mut ntp_buf = [0u8; 64];
        let (n, src) = peer_udp.recv_from(&mut ntp_buf).expect("receiver must proactively send 0x80 0xD2 NTP query");
        assert_eq!(n, 32);
        assert_eq!(ntp_buf[0], 0x80);
        assert_eq!(ntp_buf[1], 0xD2);
        // Reply with 0x80 0xD3 so receiver records ntpRx!
        let mut d3 = [0u8; 32];
        d3[0] = 0x80;
        d3[1] = 0xD3;
        d3[8..16].copy_from_slice(&ntp_buf[24..32]);
        peer_udp.send_to(&d3, src).unwrap();

        // 2. Also send incoming 0x80 0xD2 to receiver's timingPort -> verify 0x80 0xD3 reply!
        let mut q = [0u8; 32];
        q[0] = 0x80;
        q[1] = 0xD2;
        q[2] = 0x00;
        q[3] = 0x07;
        q[24..32].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF, 0x01, 0x02, 0x03, 0x04]);
        peer_udp.send_to(&q, ("127.0.0.1", our_timing)).unwrap();
        let mut got_reply = false;
        let dl = Instant::now() + Duration::from_secs(1);
        while Instant::now() < dl {
            if let Ok((rn, _)) = peer_udp.recv_from(&mut ntp_buf) {
                if rn == 32 && ntp_buf[1] == 0xD3 && &ntp_buf[8..16] == &q[24..32] {
                    got_reply = true;
                    break;
                }
            }
        }
        assert!(got_reply, "timingPort must answer incoming 0x80 0xD2 with 0x80 0xD3");

        // 3. Re-send SETUP s1 with UNSIGNED timingPort and confirm parser handles both!
        let mut s1u = Dictionary::new();
        s1u.insert("ekey".into(), Value::Data(vec![0x11u8; 72]));
        s1u.insert("eiv".into(), Value::Data(vec![0x22u8; 16]));
        s1u.insert("timingProtocol".into(), Value::String("NTP".into()));
        s1u.insert("timingPort".into(), Value::Integer(plist::Integer::Unsigned(peer_ntp_port as u64)));
        let s1u_bp = plist::to_bytes_bplist00(&Value::Dictionary(s1u)).unwrap();
        assert_eq!(send_rtsp(&mut rtsp, "SETUP", "rtsp://127.0.0.1/1", 6, Some("application/x-apple-binary-plist"), &s1u_bp).0, 200);

        std::thread::sleep(Duration::from_millis(60));
        let trace_json = get_airplay_trace_json();
        assert!(trace_json.contains("setup_s1_parsed"), "trace={trace_json}");
        assert!(trace_json.contains("proto=NTP"), "trace={trace_json}");
        assert!(trace_json.contains(&format!("theirTiming={peer_ntp_port}")), "trace={trace_json}");
        assert!(!trace_json.contains("SYNTH-HIDDEN-VALUE"), "Must NEVER log plist values (PII)! trace={trace_json}");
        let c = get_airplay_trace_counters();
        assert!(c.ntp_tx >= 2, "Expected ntp_tx >= 2, got {:?}", c);
        assert!(c.ntp_rx >= 2, "Expected ntp_rx >= 2, got {:?}", c);
        assert!(c.ntp_last_err.is_none(), "Expected no NTP error, got {:?}", c);

        drop(rtsp);
        srv.stop();
    }

    #[test]
    fn test_tahap257_airplay_audio_type96_control_port_rtcp_sync_and_feedback_keepalive() {
        use plist::{Dictionary, Value};
        use std::io::Write;
        use std::net::{TcpStream, UdpSocket};
        use std::time::Duration;

        struct NullSink;
        impl AirPlaySink for NullSink {
            fn on_state(&self, _: AirPlayState) {}
            fn on_codec(&self, _: &str, _: &str, _: u32, _: u32) {}
            fn on_frame(&self, _: Vec<u8>, _: bool) {}
        }

        reset_airplay_trace();

        let id = AirPlayIdentity::from_seed_and_mac([0x57u8; 32], "02:AA:BB:CC:DD:58".into());
        let sink: Arc<dyn AirPlaySink> = Arc::new(NullSink);
        let mut srv = AirPlayServer::start("iPhone Link", 0, id, sink, false).expect("start AirPlay");
        let port = srv.port();

        let mut rtsp = TcpStream::connect(("127.0.0.1", port)).expect("connect RTSP");
        rtsp.set_read_timeout(Some(Duration::from_secs(2))).unwrap();

        // Pair-verify + FairPlay v3 + SETUP s1
        let pv1 = [vec![1u8, 0, 0, 0], vec![0x33u8; 32], vec![0x44u8; 32]].concat();
        assert_eq!(send_rtsp(&mut rtsp, "POST", "/pair-verify", 1, Some("application/octet-stream"), &pv1).0, 200);
        assert_eq!(send_rtsp(&mut rtsp, "POST", "/pair-verify", 2, Some("application/octet-stream"), &[0u8; 68]).0, 200);
        let mut fp1 = [0u8; 16];
        fp1[..4].copy_from_slice(b"FPLY");
        fp1[4] = 3;
        fp1[5] = 1;
        fp1[14] = 0x02;
        assert_eq!(send_rtsp(&mut rtsp, "POST", "/fp-setup", 3, Some("application/octet-stream"), &fp1).0, 200);
        let mut fp2 = vec![0u8; 164];
        fp2[..4].copy_from_slice(b"FPLY");
        fp2[4] = 3;
        fp2[5] = 1;
        fp2[6] = 3;
        assert_eq!(send_rtsp(&mut rtsp, "POST", "/fp-setup", 4, Some("application/octet-stream"), &fp2).0, 200);

        let mut s1 = Dictionary::new();
        s1.insert("ekey".into(), Value::Data(vec![0x11u8; 72]));
        s1.insert("eiv".into(), Value::Data(vec![0x22u8; 16]));
        s1.insert("timingPort".into(), Value::from(0u64));
        let (_, s1_body) = send_rtsp(
            &mut rtsp,
            "SETUP",
            "rtsp://127.0.0.1/1",
            5,
            Some("application/x-apple-binary-plist"),
            &plist::to_bytes_bplist00(&Value::Dictionary(s1)).unwrap(),
        );
        let event_port = plist::from_bytes(&s1_body)
            .unwrap()
            .as_dictionary()
            .unwrap()
            .get("eventPort")
            .unwrap()
            .as_unsigned_integer()
            .unwrap() as u16;
        assert!(event_port > 0);

        // Connect eventPort & send one framed RTSP event -> verify eventAccepts & eventRx counters
        let mut ev_tcp = TcpStream::connect(("127.0.0.1", event_port)).unwrap();
        ev_tcp.write_all(b"POST /event RTSP/1.0\r\nCSeq: 1\r\nContent-Length: 0\r\n\r\n").unwrap();

        // SETUP s2 video (type=110) + audio (type=96 with ct=8, sr=44100, controlPort=50002)
        let mut s2_vid = Dictionary::new();
        s2_vid.insert("type".into(), Value::from(110u64));
        s2_vid.insert("streamConnectionID".into(), Value::from(123456789u64));
        let mut s2_aud = Dictionary::new();
        s2_aud.insert("type".into(), Value::from(96u64));
        s2_aud.insert("ct".into(), Value::from(8u64));
        s2_aud.insert("sr".into(), Value::from(44100u64));
        s2_aud.insert("controlPort".into(), Value::from(50002u64));
        let mut s2 = Dictionary::new();
        s2.insert("streams".into(), Value::Array(vec![Value::Dictionary(s2_vid), Value::Dictionary(s2_aud)]));
        let (sc2, s2_body) = send_rtsp(
            &mut rtsp,
            "SETUP",
            "rtsp://127.0.0.1/1",
            6,
            Some("application/x-apple-binary-plist"),
            &plist::to_bytes_bplist00(&Value::Dictionary(s2)).unwrap(),
        );
        assert_eq!(sc2, 200);
        let s2_val = plist::from_bytes(&s2_body).unwrap();
        let s2_streams = s2_val
            .as_dictionary()
            .unwrap()
            .get("streams")
            .unwrap()
            .as_array()
            .unwrap();
        let aud_entry = s2_streams
            .iter()
            .find_map(|v| {
                let d = v.as_dictionary()?;
                if d.get("type")?.as_unsigned_integer()? == 96 {
                    Some(d)
                } else {
                    None
                }
            })
            .expect("type=96 audio entry");
        let aud_data_port = aud_entry.get("dataPort").unwrap().as_unsigned_integer().unwrap() as u16;
        let aud_ctrl_port = aud_entry.get("controlPort").unwrap().as_unsigned_integer().unwrap() as u16;
        assert!(aud_data_port > 0 && aud_ctrl_port > 0);

        // Send 3x RTP audio packets + 3x RTCP Sync (0x80 0xD4) packets on controlPort!
        let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
        for seq in 0u16..3 {
            let rtp = [0x80, 0x60, (seq >> 8) as u8, seq as u8, 0, 0, 0, 1, 0, 0, 0, 1, 0x8C, 0x01, 0x02, 0x03];
            udp.send_to(&rtp, ("127.0.0.1", aud_data_port)).unwrap();
            let rtcp_sync = [0x80, 0xD4, 0x00, 0x04, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0, 4];
            udp.send_to(&rtcp_sync, ("127.0.0.1", aud_ctrl_port)).unwrap();
        }

        // Send 3x POST /feedback, SET_PARAMETER, POST /command, POST /audioMode, + 1x unknown POST /custom-probe
        for seq in 7..10 {
            assert_eq!(send_rtsp(&mut rtsp, "POST", "/feedback", seq, None, &[]).0, 200);
        }
        assert_eq!(send_rtsp(&mut rtsp, "SET_PARAMETER", "rtsp://127.0.0.1/1", 10, Some("text/parameters"), b"volume: -5.0\r\n").0, 200);
        assert_eq!(send_rtsp(&mut rtsp, "POST", "/command", 11, None, &[]).0, 200);
        assert_eq!(send_rtsp(&mut rtsp, "POST", "/audioMode", 12, None, &[]).0, 200);
        assert_eq!(send_rtsp(&mut rtsp, "POST", "/custom-probe", 13, None, b"probe").0, 200);

        std::thread::sleep(Duration::from_millis(80));
        let c = get_airplay_trace_counters();
        assert!(c.audio_data_rx >= 3, "audio_data_rx={:?}", c);
        assert!(c.audio_ctrl_rx >= 3, "audio_ctrl_rx={:?}", c);
        assert_eq!(c.feedback_count, 3, "feedback_count={:?}", c);
        assert!(c.event_accepts >= 1 && c.event_rx >= 1, "event counters={:?}", c);
        assert_eq!(c.teardown_count, 0);

        let trace_json = get_airplay_trace_json();
        assert!(trace_json.contains("setup_s2_parsed"), "trace={trace_json}");
        assert!(trace_json.contains("ct=8") && trace_json.contains("sr=44100"), "trace={trace_json}");
        assert!(trace_json.contains("audio_ctrl_rx") && trace_json.contains("pt=0xD4"), "trace={trace_json}");
        assert!(trace_json.contains("rtsp_method_unhandled") && trace_json.contains("POST /custom-probe"), "trace={trace_json}");

        // Send TEARDOWN type=96 (audio only) -> teardownCount == 1, RTSP stays open!
        let mut td96_s = Dictionary::new();
        td96_s.insert("type".into(), Value::from(96u64));
        let mut td96 = Dictionary::new();
        td96.insert("streams".into(), Value::Array(vec![Value::Dictionary(td96_s)]));
        assert_eq!(
            send_rtsp(&mut rtsp, "TEARDOWN", "rtsp://127.0.0.1/1", 14, Some("application/x-apple-binary-plist"), &plist::to_bytes_bplist00(&Value::Dictionary(td96)).unwrap()).0,
            200
        );
        assert_eq!(send_rtsp(&mut rtsp, "POST", "/feedback", 15, None, &[]).0, 200);
        assert_eq!(get_airplay_trace_counters().teardown_count, 1);
        assert_eq!(get_airplay_trace_counters().feedback_count, 4);

        // Full TEARDOWN {} -> teardownCount == 2
        let empty_td = plist::to_bytes_bplist00(&Value::Dictionary(Dictionary::new())).unwrap();
        assert_eq!(
            send_rtsp(&mut rtsp, "TEARDOWN", "rtsp://127.0.0.1/1", 16, Some("application/x-apple-binary-plist"), &empty_td).0,
            200
        );
        assert_eq!(get_airplay_trace_counters().teardown_count, 2);

        drop(ev_tcp);
        drop(rtsp);
        srv.stop();
    }

    #[test]
    fn test_tahap258_airplay_info_advertises_hd_portrait_ready_display_without_hevc() {
        let id = AirPlayIdentity::from_seed_and_mac([0x58u8; 32], "02:AA:BB:CC:DD:59".into());
        let receiver = info::ReceiverInfo {
            id: &id,
            name: "iPhone Link",
            hevc: false,
        };
        let bp = receiver.build_info_bplist(None, &[]);
        let val = plist::from_bytes(&bp).expect("valid GET /info bplist00");
        let d = val.as_dictionary().expect("info dict");

        // Strict no-HEVC & no-PTP bits preserved
        assert_eq!(d.get("hevc").and_then(|v| v.as_boolean()), Some(false));
        let feat = d.get("features").and_then(|v| v.as_unsigned_integer()).unwrap();
        assert_eq!(feat & (1u64 << 42), 0, "HEVC bit 42 must stay 0");
        assert_eq!(feat & (1u64 << 40), 0, "BufferedAudio bit 40 must stay 0");
        assert_eq!(feat & (1u64 << 41), 0, "PTP bit 41 must stay 0");

        let disp = d
            .get("displays")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|v| v.as_dictionary())
            .expect("displays[0] dict");
        let wp = disp.get("widthPixels").and_then(|v| v.as_unsigned_integer()).unwrap();
        let hp = disp.get("heightPixels").and_then(|v| v.as_unsigned_integer()).unwrap();
        let w = disp.get("width").and_then(|v| v.as_unsigned_integer()).unwrap();
        let h = disp.get("height").and_then(|v| v.as_unsigned_integer()).unwrap();
        let max_fps = disp.get("maxFPS").and_then(|v| v.as_unsigned_integer()).unwrap();
        let overscanned = disp.get("overscanned").and_then(|v| v.as_boolean()).unwrap();

        assert!(wp >= 2160 && hp >= 2160, "Expected HD+ portrait-ready >= 2160x2160, got {wp}x{hp}");
        assert_eq!((w, h), (wp, hp));
        assert_eq!(max_fps, 60);
        assert!(!overscanned);
    }

    #[test]
    fn test_tahap258_airplay_audio_pipeline_decrypts_decodes_handles_flush_mute_and_teardown96() {
        use crate::win_cng::Aes128CbcKey;
        use plist::{Dictionary, Value};
        use std::net::{TcpStream, UdpSocket};
        use std::time::{Duration, Instant};

        // 1. Verify offline decoders for ALL supported codecs (LPCM, ALAC uncompressed, AAC-ELD/AAC-LC via zlaac.dll)
        let mut lpcm_dec = audio::AirPlayAudioDecoder::open(1, 44_100, 352).expect("open LPCM");
        let lpcm_out = lpcm_dec.decode_payload(&[0x12, 0x34, 0xFE, 0xDC]).expect("decode LPCM");
        assert_eq!(lpcm_out, vec![0x1234i16, -292i16]);

        // Construct valid uncompressed ALAC CPE frame (3b tag=1, 4b inst=0, 12b unused=0, 1b has_size=1,
        // 2b shift=0, 1b uncompressed=1, 32b num_frames=4, then 4 stereo pairs of 16-bit BE samples).
        let mut alac_bits: u64 = 0;
        let mut bit_buf: Vec<u8> = Vec::new();
        let mut bit_len = 0usize;
        let mut push_bits = |val: u32, n: usize| {
            for k in (0..n).rev() {
                alac_bits = (alac_bits << 1) | (((val >> k) & 1) as u64);
                bit_len += 1;
                if bit_len == 8 {
                    bit_buf.push(alac_bits as u8);
                    alac_bits = 0;
                    bit_len = 0;
                }
            }
        };
        push_bits(1, 3); // tag = CPE (stereo)
        push_bits(0, 4); // elem_inst
        push_bits(0, 12); // unused
        push_bits(1, 1); // has_size
        push_bits(0, 2); // unused_bytes
        push_bits(1, 1); // is_uncompressed
        push_bits(4, 32); // 4 stereo frames
        for s in [1000i16, -1000i16, 2000, -2000, 3000, -3000, 4000, -4000] {
            push_bits(s as u16 as u32, 16);
        }
        if bit_len > 0 {
            bit_buf.push((alac_bits << (8 - bit_len)) as u8);
        }
        let mut alac_dec = audio::AirPlayAudioDecoder::open(2, 44_100, 352).expect("open ALAC");
        let alac_out = alac_dec.decode_payload(&bit_buf).expect("decode ALAC");
        assert_eq!(alac_out, vec![1000, -1000, 2000, -2000, 3000, -3000, 4000, -4000]);

        // Verify bundled zlaac.dll opens BOTH AAC-ELD (ct=8) & AAC-LC (ct=4) cleanly!
        let _eld_dec = audio::AirPlayAudioDecoder::open(8, 44_100, 480).expect("open AAC-ELD via zlaac.dll");
        let _lc_dec = audio::AirPlayAudioDecoder::open(4, 44_100, 1024).expect("open AAC-LC via zlaac.dll");

        // 2. Full live AirPlayServer E2E: encrypted AES-128-CBC RTP audio -> waveOut + FLUSH + Mute + TEARDOWN type=96
        struct NullSink;
        impl AirPlaySink for NullSink {
            fn on_state(&self, _: AirPlayState) {}
            fn on_codec(&self, _: &str, _: &str, _: u32, _: u32) {}
            fn on_frame(&self, _: Vec<u8>, _: bool) {}
        }

        reset_airplay_trace();
        reset_airplay_audio_stats();
        set_airplay_audio_muted(false);

        let id = AirPlayIdentity::from_seed_and_mac([0x58u8; 32], "02:AA:BB:CC:DD:60".into());
        let sink: Arc<dyn AirPlaySink> = Arc::new(NullSink);
        let mut srv = AirPlayServer::start("iPhone Link", 0, id, sink, false).expect("start AirPlay");
        let port = srv.port();

        let mut rtsp = TcpStream::connect(("127.0.0.1", port)).expect("connect RTSP");
        rtsp.set_read_timeout(Some(Duration::from_secs(2))).unwrap();

        // Real X25519 Pair-Verify + FairPlay v3 so client & server share exact AES-128-CBC audio key!
        let cli_x_priv = [0x37u8; 32];
        let cli_x_pub = x25519_public(&cli_x_priv);
        let pv1 = [vec![1u8, 0, 0, 0], cli_x_pub.to_vec(), vec![0x44u8; 32]].concat();
        let (sc, pv1_resp) = send_rtsp(&mut rtsp, "POST", "/pair-verify", 1, Some("application/octet-stream"), &pv1);
        assert_eq!(sc, 200);
        let srv_x_pub: [u8; 32] = pv1_resp[..32].try_into().unwrap();
        let shared_secret = x25519(&cli_x_priv, &srv_x_pub);
        assert_eq!(send_rtsp(&mut rtsp, "POST", "/pair-verify", 2, Some("application/octet-stream"), &[0u8; 68]).0, 200);

        let mut fp1 = [0u8; 16];
        fp1[..4].copy_from_slice(b"FPLY");
        fp1[4] = 3;
        fp1[5] = 1;
        fp1[14] = 0x02;
        assert_eq!(send_rtsp(&mut rtsp, "POST", "/fp-setup", 3, Some("application/octet-stream"), &fp1).0, 200);
        let mut fp2 = vec![0u8; 164];
        fp2[..4].copy_from_slice(b"FPLY");
        fp2[4] = 3;
        fp2[5] = 1;
        fp2[6] = 3;
        assert_eq!(send_rtsp(&mut rtsp, "POST", "/fp-setup", 4, Some("application/octet-stream"), &fp2).0, 200);

        let ekey = vec![0x29u8; 72];
        let eiv = [0x61u8; 16];
        let mut fp_cli = fairplay::FairPlay::default();
        assert!(fp_cli.handle(&fp1).is_some());
        assert!(fp_cli.handle(&fp2).is_some());
        let fp_key = fp_cli.decrypt(&ekey).expect("fairplay decrypt ekey");
        let audio_key = session_key(&fp_key, Some(&shared_secret));

        let mut s1 = Dictionary::new();
        s1.insert("ekey".into(), Value::Data(ekey));
        s1.insert("eiv".into(), Value::Data(eiv.to_vec()));
        s1.insert("timingPort".into(), Value::from(0u64));
        assert_eq!(
            send_rtsp(&mut rtsp, "SETUP", "rtsp://127.0.0.1/1", 5, Some("application/x-apple-binary-plist"), &plist::to_bytes_bplist00(&Value::Dictionary(s1)).unwrap()).0,
            200
        );

        // SETUP video (type=110) + audio (type=96, ct=1 LPCM 44.1 kHz 16-bit stereo so sine PCM is 100% deterministic)
        let mut s2_vid = Dictionary::new();
        s2_vid.insert("type".into(), Value::from(110u64));
        s2_vid.insert("streamConnectionID".into(), Value::from(555666777u64));
        let mut s2_aud = Dictionary::new();
        s2_aud.insert("type".into(), Value::from(96u64));
        s2_aud.insert("ct".into(), Value::from(1u64));
        s2_aud.insert("sr".into(), Value::from(44_100u64));
        s2_aud.insert("spf".into(), Value::from(352u64));
        let mut s2 = Dictionary::new();
        s2.insert("streams".into(), Value::Array(vec![Value::Dictionary(s2_vid), Value::Dictionary(s2_aud)]));
        let (sc2, s2_body) = send_rtsp(
            &mut rtsp,
            "SETUP",
            "rtsp://127.0.0.1/1",
            6,
            Some("application/x-apple-binary-plist"),
            &plist::to_bytes_bplist00(&Value::Dictionary(s2)).unwrap(),
        );
        assert_eq!(sc2, 200);
        let s2_val = plist::from_bytes(&s2_body).unwrap();
        let aud_entry = s2_val
            .as_dictionary()
            .unwrap()
            .get("streams")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .find_map(|v| {
                let d = v.as_dictionary()?;
                if d.get("type")?.as_unsigned_integer()? == 96 { Some(d) } else { None }
            })
            .expect("type=96 entry");
        let aud_data_port = aud_entry.get("dataPort").unwrap().as_unsigned_integer().unwrap() as u16;
        assert!(aud_data_port > 0);

        // Encrypt & send 10 valid 440 Hz stereo RTP LPCM packets (352 frames = 1408B payload + 4B unencrypted tail test)
        let cbc = Aes128CbcKey::new(&audio_key).expect("cbc enc");
        let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
        for seq in 0u16..10 {
            let mut payload = Vec::with_capacity(352 * 4);
            for f in 0..352usize {
                let t = ((seq as usize * 352 + f) as f32) * 2.0 * std::f32::consts::PI * 440.0 / 44_100.0;
                let sample = (t.sin() * 8000.0) as i16;
                payload.extend_from_slice(&sample.to_be_bytes());
                payload.extend_from_slice(&sample.to_be_bytes());
            }
            cbc.encrypt_in_place(&eiv, &mut payload).expect("cbc encrypt audio");
            let mut pkt = vec![0x80, 0x60, (seq >> 8) as u8, seq as u8, 0, 0, 0, 1, 0, 0, 0, 1];
            pkt.extend_from_slice(&payload);
            udp.send_to(&pkt, ("127.0.0.1", aud_data_port)).unwrap();
        }

        let dl = Instant::now() + Duration::from_secs(2);
        while Instant::now() < dl {
            let snap = get_airplay_audio_snapshot();
            if snap.decoded_packets >= 10 && snap.played_samples > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        let snap = get_airplay_audio_snapshot();
        assert!(snap.active, "airplayAudioActive must be true: {snap:?}");
        assert_eq!(snap.codec.as_deref(), Some("pcm"));
        assert!(snap.decoded_packets >= 10, "decoded_packets={snap:?}");
        assert!(snap.played_samples > 0, "played_samples must be > 0: {snap:?}");
        assert_eq!(snap.decode_errors, 0, "decode_errors={snap:?}");

        let cnt = get_airplay_trace_counters();
        assert!(cnt.audio_decoded >= 10 && cnt.audio_played_samples > 0 && cnt.audio_decode_errors == 0, "{cnt:?}");
        assert_eq!(cnt.audio_codec, "pcm");

        // Test atomic Mute / Unmute + RTSP FLUSH
        set_airplay_audio_muted(true);
        assert!(is_airplay_audio_muted());
        set_airplay_audio_muted(false);
        assert!(!is_airplay_audio_muted());
        assert_eq!(send_rtsp(&mut rtsp, "FLUSH", "rtsp://127.0.0.1/1", 7, None, &[]).0, 200);
        assert!(get_airplay_trace_json().contains("audio_flush"));

        // Send TEARDOWN type=96 -> audio closes cleanly (< 20ms) while RTSP + video type=110 stay alive!
        let mut td96_s = Dictionary::new();
        td96_s.insert("type".into(), Value::from(96u64));
        let mut td96 = Dictionary::new();
        td96.insert("streams".into(), Value::Array(vec![Value::Dictionary(td96_s)]));
        assert_eq!(
            send_rtsp(&mut rtsp, "TEARDOWN", "rtsp://127.0.0.1/1", 8, Some("application/x-apple-binary-plist"), &plist::to_bytes_bplist00(&Value::Dictionary(td96)).unwrap()).0,
            200
        );
        assert!(!get_airplay_audio_snapshot().active, "TEARDOWN type=96 must cleanly deactivate AirPlay audio!");
        assert_eq!(send_rtsp(&mut rtsp, "POST", "/feedback", 9, None, &[]).0, 200, "RTSP & video 110 must stay alive!");

        drop(rtsp);
        srv.stop();
    }

    #[test]
    fn test_tahap259_airplay_audio_smooth_volume_ramp_and_hot_reload_presets_zero_drop() {
        // 1. Verify SmoothGainRamp exact 0% silence, proportional 50% (zero clipping), & exact 100% unity
        let orig_frame: Vec<i16> = (0..480)
            .flat_map(|i| {
                let s = (((i as f32) * 0.1).sin() * 16_000.0) as i16;
                [s, -s]
            })
            .collect();

        let mut ramp = SmoothGainRamp::new(0, false);
        let mut buf0 = orig_frame.clone();
        ramp.apply_stereo_in_place(&mut buf0, 0, false);
        assert!(buf0.iter().all(|&s| s == 0), "0% volume must produce exact silence");

        // Ramp 0% -> 50% across 2 frames (~21.7 ms > ~5 ms ramp); tail of frame 2 must be steady ~50%
        let mut buf50_a = orig_frame.clone();
        ramp.apply_stereo_in_place(&mut buf50_a, 50, false);
        let mut buf50_b = orig_frame.clone();
        ramp.apply_stereo_in_place(&mut buf50_b, 50, false);
        for (&orig, &scaled) in orig_frame.iter().zip(buf50_b.iter()) {
            let expected = (orig as i32) / 2;
            assert!(
                (scaled as i32 - expected).abs() <= 2,
                "50% gain expected ~{expected}, got {scaled} (orig={orig})"
            );
        }

        // Ramp 50% -> 100%; after ramp completes, samples must be exact unity (0 distortion)
        let mut buf100_a = orig_frame.clone();
        ramp.apply_stereo_in_place(&mut buf100_a, 100, false);
        let mut buf100_b = orig_frame.clone();
        ramp.apply_stereo_in_place(&mut buf100_b, 100, false);
        assert_eq!(buf100_b, orig_frame, "100% volume must produce exact unity samples");

        // Mute forces exact silence after ramp
        let mut buf_m1 = orig_frame.clone();
        ramp.apply_stereo_in_place(&mut buf_m1, 100, true);
        let mut buf_m2 = orig_frame.clone();
        ramp.apply_stereo_in_place(&mut buf_m2, 100, true);
        assert!(buf_m2.iter().all(|&s| s == 0), "muted audio must produce exact silence");

        // 2. Verify live atomic hot-reload presets & Lip Sync targets without resetting/dropping active state
        let s_low_on = apply_live_audio_config(Some(40), Some("low"), Some(true), Some(false));
        assert_eq!(s_low_on.volume, 40);
        assert_eq!(s_low_on.buffer_preset, "low");
        assert!(s_low_on.lip_sync);
        assert_eq!(s_low_on.target_ms, 22 + 55);

        let s_low_off = apply_live_audio_config(None, None, Some(false), None);
        assert_eq!(s_low_off.volume, 40, "partial update must preserve volume");
        assert_eq!(s_low_off.buffer_preset, "low");
        assert!(!s_low_off.lip_sync);
        assert_eq!(s_low_off.target_ms, 22);

        let s_norm_on = apply_live_audio_config(Some(100), Some("normal"), Some(true), Some(false));
        assert_eq!(s_norm_on.target_ms, 45 + 55);
        assert_eq!(compute_airplay_audio_targets(1, false), (45, 95));
        assert_eq!(compute_airplay_audio_targets(2, false), (90, 175));
        assert_eq!(compute_airplay_audio_targets(2, true), (90 + 55, 175 + 55));
    }

    #[test]
    fn test_tahap259_airplay_audio_smooth_drift_trimming_caps_backlog_under_burst() {
        let _ = apply_live_audio_config(Some(100), Some("low"), Some(false), Some(false));
        let (target_ms, cap_ms) = compute_airplay_audio_targets(0, false);
        assert_eq!((target_ms, cap_ms), (22, 55));

        let mut q = AirPlayAudioJitterQueue::new(audio::AIRPLAY_AUDIO_RATE);
        let now = Instant::now();
        let pkt_template = vec![4000i16; 960]; // 480 stereo frames = ~10.88 ms per packet
        let mut last_buf_ms = 0u32;
        let mut total_dropped = 0u64;

        // Simulate a heavy Wi-Fi burst of 40 consecutive RTP audio packets (~435 ms!) arriving at once
        for _ in 0..40 {
            let (dropped_now, buf_ms, t_ms) = q.push_packet(pkt_template.clone(), 0, now);
            total_dropped += dropped_now;
            last_buf_ms = buf_ms;
            assert_eq!(t_ms, target_ms);
        }

        assert!(
            total_dropped >= 1 && q.total_dropped_packets >= 1,
            "Smooth drift trimming must drop excess burst packets (dropped={total_dropped})"
        );
        assert!(
            last_buf_ms <= cap_ms + 25,
            "Queued buffer_ms ({last_buf_ms} ms) must stay capped <= {} ms",
            cap_ms + 25
        );

        // Drain after prefill satisfied -> blocks must still come out intact & non-empty
        let (blocks, _d2, post_buf_ms, _t2) = q.drain_ready_blocks(0, 12, now);
        assert!(!blocks.is_empty(), "Queue must still play smoothly after drift trimming");
        assert!(post_buf_ms <= cap_ms + 25);

        // Restore defaults cleanly
        let _ = apply_live_audio_config(Some(100), Some("normal"), Some(true), Some(false));
    }

    #[test]
    fn test_tahap259_airplay_display_sweetspot_2160_and_live_idr_fast_forward() {
        // 1. Verify GET /info advertises exact sweet-spot HD+ Retina 2160x2160 @ 60fps, hevc=false, bit42=0
        let id = AirPlayIdentity::from_seed_and_mac([0x59u8; 32], "02:AA:BB:CC:DD:61".into());
        let receiver = info::ReceiverInfo {
            id: &id,
            name: "iPhone Link",
            hevc: false,
        };
        let bp = receiver.build_info_bplist(None, &[]);
        let val = plist::from_bytes(&bp).expect("valid GET /info bplist00");
        let d = val.as_dictionary().expect("info dict");
        assert_eq!(d.get("hevc").and_then(|v| v.as_boolean()), Some(false));
        let feat = d.get("features").and_then(|v| v.as_unsigned_integer()).unwrap();
        assert_eq!(feat & (1u64 << 42), 0, "HEVC bit 42 must stay 0");
        let disp0 = d
            .get("displays")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|v| v.as_dictionary())
            .expect("displays[0]");
        assert_eq!(disp0.get("widthPixels").and_then(|v| v.as_unsigned_integer()), Some(2160));
        assert_eq!(disp0.get("heightPixels").and_then(|v| v.as_unsigned_integer()), Some(2160));
        assert_eq!(disp0.get("maxFPS").and_then(|v| v.as_unsigned_integer()), Some(60));
        assert_eq!(disp0.get("overscanned").and_then(|v| v.as_boolean()), Some(false));

        // 2. Verify LIVE_BROADCAST IDR Fast-Forward jumps cleanly to newest IDR when client lags > 12 frames
        let _ = crate::mirror::start_airplay_mirror_session(
            Some("00008150-TEST-DEV-A"),
            false,
            "wifi",
            None,
            Some(0),
            false,
        )
        .expect("start AirPlay test session");
        let sgen = crate::mirror::current_live_session_gen();

        // Push initial IDR (#1) + 2 deltas (#2, #3) so client syncs at seq=3
        assert!(crate::mirror::global_on_physical_h264_avcc_frame(sgen, vec![0, 0, 0, 2, 0x65, 0x01], true));
        assert!(crate::mirror::global_on_physical_h264_avcc_frame(sgen, vec![0, 0, 0, 2, 0x41, 0x02], false));
        assert!(crate::mirror::global_on_physical_h264_avcc_frame(sgen, vec![0, 0, 0, 2, 0x41, 0x03], false));

        let mut last_seq = 0u64;
        let mut sent_init = false;
        let first_batch = crate::mirror::global_wait_next_live_chunks(&mut last_seq, &mut sent_init, 50)
            .expect("first batch");
        assert!(sent_init && last_seq == 3 && first_batch.len() == 3);

        // Now simulate client lagging behind by 20 frames (15 stale deltas + fresh IDR #19 + 4 new deltas #20..#23)
        for i in 0u8..15 {
            assert!(crate::mirror::global_on_physical_h264_avcc_frame(sgen, vec![0, 0, 0, 2, 0x41, 0x10 + i], false));
        }
        assert!(crate::mirror::global_on_physical_h264_avcc_frame(sgen, vec![0, 0, 0, 2, 0x65, 0xAA], true)); // seq=19 (IDR!)
        for i in 0u8..4 {
            assert!(crate::mirror::global_on_physical_h264_avcc_frame(sgen, vec![0, 0, 0, 2, 0x41, 0x80 + i], false));
        }

        let catchup = crate::mirror::global_wait_next_live_chunks(&mut last_seq, &mut sent_init, 50)
            .expect("catchup batch");
        // Must have skipped the 15 stale deltas and started cleanly on IDR seq=19 (type=0) + 4 trailing deltas = 5 chunks!
        assert_eq!(catchup.len(), 5, "Expected IDR + 4 trailing deltas, got {}", catchup.len());
        assert_eq!(catchup[0][4], 0, "First emitted chunk after catch-up MUST be IDR (type=0), never mid-GOP delta!");
        assert_eq!(last_seq, 23);
        assert!(
            crate::mirror::global_airplay_video_queue_dropped() >= 15,
            "airplayVideoQueueDropped must record skipped stale pre-IDR frames"
        );

        let _ = crate::mirror::global_stop_screen();
    }

    #[test]
    fn test_tahap273_airplay_trace_json_exposes_all_stage_counters_zero_pii() {
        reset_airplay_trace();
        inc_trace_tcp_video_packets();
        inc_trace_tcp_video_packets();
        inc_trace_tcp_video_packets();
        inc_trace_avcc_updates();
        inc_trace_avcc_updates();
        inc_trace_watchdog_recoveries();

        update_native_video_stage_trace(
            42,
            3,
            40,
            39,
            1,
            2,
            0,
            39,
            38,
            38,
            0,
            -1072875852,
            Some((888, 1920)),
            Some((888, 1920)),
        );

        let json = get_airplay_trace_json();
        for expected in [
            r#""tcpVideoPackets":3"#,
            r#""avccUpdates":2"#,
            r#""videoQueueDropped":0"#,
            r#""queueDropped":3"#,
            r#""nativeVideoSinkPackets":42"#,
            r#""decoderFed":40"#,
            r#""mfDecodedFrames":39"#,
            r#""mfStreamChanges":1"#,
            r#""mfNotAcceptingDrains":2"#,
            r#""staleDimDropped":0"#,
            r#""vramPublished":39"#,
            r#""dcompPresentedFrames":38"#,
            r#""presentCalls":38"#,
            r#""presentErrors":0"#,
            r#""lastDecoderHresult":-1072875852"#,
            r#""lastMftDims":"888x1920""#,
            r#""expectedCodedDims":"888x1920""#,
            r#""airplayWatchdogRecoveries":1"#,
        ] {
            assert!(json.contains(expected), "missing `{expected}` in trace JSON: {json}");
        }
        assert!(
            !json.contains("C:\\\\")
                && !json.contains("/Users/")
                && !json.contains("192.168.")
                && !json.contains("00008")
        );

        reset_airplay_trace();
        let after_reset = get_airplay_trace_json();
        assert!(after_reset.contains(r#""tcpVideoPackets":0"#));
        assert!(after_reset.contains(r#""lastMftDims":"0x0""#));
    }

    #[test]
    fn test_tahap274_strict_avcc_validation_and_safe_ctr_resync_prevents_false_counter0_desync() {
        use super::mirror::{
            avcc_au_has_keyframe_or_params, valid_avcc_au_strict, MirrorDecryptor,
        };
        use crate::win_cng::Aes128Ctr;

        // 1. Verify valid_avcc_au_strict validation rules (4-byte & 2-byte length prefixes)
        assert!(!valid_avcc_au_strict(&[], 4));
        assert!(!valid_avcc_au_strict(&[0, 0, 0, 1], 4));
        assert!(!valid_avcc_au_strict(&[0, 0, 0, 0, 0x65], 4)); // zero length
        assert!(!valid_avcc_au_strict(&[0, 0, 0, 4, 0x65, 0x88], 4)); // overrun
        assert!(!valid_avcc_au_strict(&[0, 0, 0, 2, 0x65, 0x88, 0xFF], 4)); // trailing non-zero
        assert!(!valid_avcc_au_strict(&[0, 0, 0, 2, 0x65, 0x88, 0x00, 0x00], 4)); // trailing zero
        assert!(!valid_avcc_au_strict(&[0, 0, 0, 2, 0xE5, 0x88], 4)); // forbidden_zero_bit == 1
        assert!(!valid_avcc_au_strict(&[0, 0, 0, 2, 0x00, 0x88], 4)); // nal_type == 0
        assert!(!valid_avcc_au_strict(&[0, 0, 0, 2, 0x1F, 0x88], 4)); // nal_type > 12
        assert!(!valid_avcc_au_strict(&[0, 0, 0, 2, 0x0C, 0xFF], 4)); // filler-only

        let pack4 = |nals: &[&[u8]]| -> Vec<u8> {
            let mut out = Vec::new();
            for n in nals {
                out.extend_from_slice(&(n.len() as u32).to_be_bytes());
                out.extend_from_slice(n);
            }
            out
        };
        let pack2 = |nals: &[&[u8]]| -> Vec<u8> {
            let mut out = Vec::new();
            for n in nals {
                out.extend_from_slice(&(n.len() as u16).to_be_bytes());
                out.extend_from_slice(n);
            }
            out
        };

        let multi_idr = pack4(&[&[0x09, 0xF0], &[0x06, 0x05, 0x80], &[0x65, 0x88, 0x84]]);
        let p_only = pack4(&[&[0x41, 0x9A, 0x22, 0x33]]);
        assert!(valid_avcc_au_strict(&multi_idr, 4));
        assert!(avcc_au_has_keyframe_or_params(&multi_idr, 4));
        assert!(valid_avcc_au_strict(&p_only, 4));
        assert!(!avcc_au_has_keyframe_or_params(&p_only, 4));
        assert!(valid_avcc_au_strict(&pack2(&[&[0x09, 0xF0], &[0x41, 0x9A]]), 2));

        // 2. Verify MirrorDecryptor safe mid-stream CTR resync guard
        reset_airplay_trace();
        let key = [0x3Au8; 16];
        let iv = [0x91u8; 16];
        let mut sender_ctr = Aes128Ctr::new(&key, &iv).unwrap();
        let mut dec = MirrorDecryptor::new(key, iv).unwrap();

        let idr1 = pack4(&[&[0x65, 0x88, 0x84, 0x21, 0xA0]]);
        let mut pkt1 = idr1.clone();
        sender_ctr.apply_keystream(&mut pkt1).unwrap();
        assert!(dec.decrypt_in_place(&mut pkt1).unwrap());
        assert_eq!(pkt1, idr1);

        let p2 = pack4(&[&[0x41, 0x9A, 0x10, 0x20, 0x30]]);
        let mut pkt2 = p2.clone();
        sender_ctr.apply_keystream(&mut pkt2).unwrap();
        assert!(dec.decrypt_in_place(&mut pkt2).unwrap());
        assert_eq!(pkt2, p2);

        // Inject a counter-0 P-frame trap (no SPS/PPS/IDR) while running CTR stays alive:
        // must be REJECTED without overwriting self.ctr!
        let trap_plain = pack4(&[&[0x41, 0x9A, 0x55, 0x66, 0x77, 0x88]]);
        let mut trap_ct = trap_plain.clone();
        let mut rogue_ctr0 = Aes128Ctr::new(&key, &iv).unwrap();
        rogue_ctr0.apply_keystream(&mut trap_ct).unwrap();
        // Advance legitimate sender by same byte count (mirroring running CTR advancement on trial)
        let mut advance_dummy = vec![0u8; trap_ct.len()];
        sender_ctr.apply_keystream(&mut advance_dummy).unwrap();

        assert!(!dec.decrypt_in_place(&mut trap_ct).unwrap());
        assert_eq!(dec.ctr_decrypt_rejects, 1);
        assert_eq!(dec.ctr_resync_count, 0);
        assert_eq!(dec.consecutive_bad_ctr, 1);

        // Next legitimate frame from original running CTR MUST still decrypt cleanly!
        let p3 = pack4(&[&[0x41, 0x9A, 0x44, 0x55, 0x66]]);
        let mut pkt3 = p3.clone();
        sender_ctr.apply_keystream(&mut pkt3).unwrap();
        assert!(dec.decrypt_in_place(&mut pkt3).unwrap());
        assert_eq!(pkt3, p3);
        assert_eq!(dec.consecutive_bad_ctr, 0);

        // Under Tahap 275 3-miss hysteresis guard, inject 3 consecutive decrypt misses before
        // genuine counter-0 restart carrying SPS(7)+PPS(8)+IDR(5) resyncs cleanly.
        for tag in 0x11u8..=0x13u8 {
            let mut miss = [0xFA, 0xCE, 0xB0, 0x0C, tag, 0x22, 0x33, 0x44];
            assert!(!dec.decrypt_in_place(&mut miss).unwrap());
        }
        assert_eq!(dec.consecutive_bad_ctr, 3);

        // Genuine counter-0 restart carrying SPS(7)+PPS(8)+IDR(5) MUST resync cleanly!
        let mut restarted_ctr = Aes128Ctr::new(&key, &iv).unwrap();
        let resync_idr = pack4(&[&[0x67, 0x42, 0xC0, 0x1E], &[0x68, 0xCE, 0x38], &[0x65, 0x88, 0x01]]);
        let mut resync_ct = resync_idr.clone();
        restarted_ctr.apply_keystream(&mut resync_ct).unwrap();
        assert!(dec.decrypt_in_place(&mut resync_ct).unwrap());
        assert_eq!(resync_ct, resync_idr);
        assert_eq!(dec.ctr_resync_count, 1);

        // Subsequent P-frame from restarted CTR decrypts cleanly
        let p4 = pack4(&[&[0x41, 0x9A, 0x77, 0x88, 0x99]]);
        let mut pkt4 = p4.clone();
        restarted_ctr.apply_keystream(&mut pkt4).unwrap();
        assert!(dec.decrypt_in_place(&mut pkt4).unwrap());
        assert_eq!(pkt4, p4);

        let trace_json = get_airplay_trace_json();
        assert!(trace_json.contains(r#""ctrResyncCount":1"#), "{trace_json}");
        assert!(trace_json.contains(r#""ctrDecryptRejects":4"#), "{trace_json}");
        assert!(trace_json.contains(r#""midStreamBlackClears":0"#), "{trace_json}");
        assert!(trace_json.contains(r#""dualBufferOverwrites":0"#), "{trace_json}");
        reset_airplay_trace();
    }

    #[test]
    fn test_tahap275_continuous_ctr_500_packets_with_trailing_padding_zero_desync_and_3miss_resync_guard() {
        use crate::airplay::mirror::{
            normalize_avcc_au_classified, strip_trailing_zero_padding_avcc,
            valid_avcc_au_relaxed, MirrorDecryptor, NormalizedAvccOutcome,
        };
        use crate::win_cng::Aes128Ctr;

        reset_airplay_trace();
        let key = [0x3Au8; 16];
        let iv = [0x7Fu8; 16];
        let mut sender_ctr = Aes128Ctr::new(&key, &iv).unwrap();
        let mut dec = MirrorDecryptor::new(key, iv).unwrap();

        let pack4 = |nals: &[&[u8]]| -> Vec<u8> {
            let mut out = Vec::new();
            for n in nals {
                out.extend_from_slice(&(n.len() as u32).to_be_bytes());
                out.extend_from_slice(n);
            }
            out
        };

        let mut total_bytes = 0u64;
        for i in 0..500usize {
            let plain = match i % 5 {
                0 => pack4(&[
                    &[0x67, 0x42, 0xC0, 0x1E],
                    &[0x68, 0xCE, 0x38],
                    &[0x65, 0x88, (i & 0x7F) as u8 | 0x01, 0x22],
                ]),
                1 => {
                    // P-slice followed by 4..=16 trailing 0x00 alignment padding bytes
                    let mut v = pack4(&[&[0x41, 0x9A, (i & 0xFF) as u8, 0x33, 0x44]]);
                    let pad_len = [4usize, 8, 12, 16][(i / 5) % 4];
                    v.extend(std::iter::repeat_n(0u8, pad_len));
                    v
                }
                2 => {
                    // Extension NAL 19 / 20
                    let hdr = if (i % 2) == 0 { 0x53 } else { 0x54 };
                    pack4(&[&[hdr, 0x9A, (i & 0xFF) as u8, 0x55]])
                }
                3 => {
                    // SEI(6) + AUD(9) + Filler(12) + P(1) + 3-byte trailing pad
                    let mut v = pack4(&[
                        &[0x09, 0xF0],
                        &[0x06, 0x05, 0xAA],
                        &[0x0C, 0xFF],
                        &[0x41, 0x9A, (i & 0xFF) as u8, 0x66],
                    ]);
                    v.extend_from_slice(&[0u8; 3]);
                    v
                }
                _ => pack4(&[&[0x41, 0x9A, (i & 0xFF) as u8, 0x77, 0x88, 0x99]]),
            };

            let mut ct = plain.clone();
            sender_ctr.apply_keystream(&mut ct).unwrap();
            total_bytes += ct.len() as u64;

            assert!(
                dec.decrypt_in_place(&mut ct).unwrap(),
                "Packet #{i} failed continuous CTR decrypt (len={})",
                plain.len()
            );
            assert_eq!(ct, plain, "Plaintext mismatch at packet #{i}");
            assert!(valid_avcc_au_relaxed(&ct, 4));
            let stripped = strip_trailing_zero_padding_avcc(&ct, 4);
            assert!(matches!(
                normalize_avcc_au_classified(stripped),
                NormalizedAvccOutcome::Video { .. }
            ));
        }

        assert_eq!(dec.ctr_resync_count, 0);
        assert_eq!(dec.ctr_decrypt_rejects, 0);
        assert_eq!(dec.ctr_keystream_bytes, total_bytes);
        assert_eq!(trace_ctr_keystream_bytes(), total_bytes);

        // Verify 3-miss hysteresis guard: 2 misses (< 3) NEVER false-reset CTR even on counter-0 IDR
        for step in 0..2u8 {
            let mut bad = vec![0xDE, 0xAD, 0xBE, 0xEF, step, 0xAA];
            total_bytes += bad.len() as u64;
            assert!(!dec.decrypt_in_place(&mut bad).unwrap());
        }
        assert_eq!(dec.consecutive_bad_ctr, 2);
        assert_eq!(dec.ctr_resync_count, 0);

        let resync_idr = pack4(&[&[0x67, 0x42, 0xC0, 0x1E], &[0x68, 0xCE, 0x38], &[0x65, 0x88, 0x09]]);
        let mut early_resync = resync_idr.clone();
        Aes128Ctr::new(&key, &iv).unwrap().apply_keystream(&mut early_resync).unwrap();
        total_bytes += early_resync.len() as u64;
        // Miss #3: still rejects early_resync because prior consecutive_bad_ctr was 2 (< 3)
        assert!(!dec.decrypt_in_place(&mut early_resync).unwrap());
        assert_eq!(dec.consecutive_bad_ctr, 3);
        assert_eq!(dec.ctr_resync_count, 0);

        // Next genuine counter-0 SPS+PPS+IDR after consecutive_bad_ctr >= 3 MUST resync cleanly!
        let mut genuine_resync = resync_idr.clone();
        Aes128Ctr::new(&key, &iv).unwrap().apply_keystream(&mut genuine_resync).unwrap();
        total_bytes += genuine_resync.len() as u64;
        assert!(dec.decrypt_in_place(&mut genuine_resync).unwrap());
        assert_eq!(genuine_resync, resync_idr);
        assert_eq!(dec.ctr_resync_count, 1);
        assert_eq!(dec.ctr_keystream_bytes, total_bytes);
        let trace_json = get_airplay_trace_json();
        assert!(
            trace_json.contains(&format!(r#""ctrKeystreamBytes":{total_bytes}"#)),
            "{trace_json}"
        );
        reset_airplay_trace();
    }

    #[test]
    fn test_tahap275_airplay_tcp_eof_immediately_emits_disconnect_and_stopped_event() {
        use std::io::Write;
        use std::net::{TcpListener, TcpStream};
        use std::sync::atomic::AtomicBool;
        use std::time::{Duration, Instant};

        use crate::airplay::mirror::run_mirror_listener;
        use crate::win_cng::Aes128Ctr;

        let _hid_guard = crate::hid::HID_TEST_SERIAL_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        reset_airplay_trace();

        struct LoopTrackSink {
            events: Arc<Mutex<Vec<bool>>>,
        }
        impl AirPlaySink for LoopTrackSink {
            fn on_state(&self, _: AirPlayState) {}
            fn on_codec(&self, _: &str, _: &str, _: u32, _: u32) {}
            fn on_frame(&self, _: Vec<u8>, _: bool) {}
            fn on_mirror_loop_state(&self, alive: bool) {
                if let Ok(mut g) = self.events.lock() {
                    g.push(alive);
                }
            }
        }

        // Part A: Real loopback mirror TCP EOF while stop == false increments `tcpDisconnects`
        // and calls `on_mirror_loop_state(false)` cleanly in < 500 ms.
        let loop_events = Arc::new(Mutex::new(Vec::<bool>::new()));
        let sink: Arc<dyn AirPlaySink> = Arc::new(LoopTrackSink {
            events: loop_events.clone(),
        });
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback mirror");
        let port = listener.local_addr().unwrap().port();
        let key = [0x22u8; 16];
        let iv = [0x44u8; 16];
        let stop = Arc::new(AtomicBool::new(false));
        let stop_srv = stop.clone();
        let jh = std::thread::spawn(move || {
            run_mirror_listener(listener, key, iv, sink, stop_srv);
        });

        let mut cli = TcpStream::connect(("127.0.0.1", port)).expect("connect mirror tcp");
        let idr = [0x00, 0x00, 0x00, 0x05, 0x65, 0x88, 0x01, 0x02, 0x03];
        let mut idr_ct = idr.to_vec();
        Aes128Ctr::new(&key, &iv).unwrap().apply_keystream(&mut idr_ct).unwrap();
        let mut hdr = [0u8; 128];
        hdr[0..4].copy_from_slice(&(idr_ct.len() as u32).to_le_bytes());
        hdr[4..6].copy_from_slice(&0u16.to_le_bytes());
        cli.write_all(&hdr).unwrap();
        cli.write_all(&idr_ct).unwrap();
        cli.flush().unwrap();

        let t_wait = Instant::now();
        while t_wait.elapsed() < Duration::from_millis(500) {
            if loop_events.lock().map(|g| g.contains(&true)).unwrap_or(false) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(loop_events.lock().unwrap().contains(&true));
        assert_eq!(trace_tcp_disconnects(), 0);

        drop(cli); // Sudden TCP disconnect while RTSP/server remains up
        let t_eof = Instant::now();
        let mut saw_false = false;
        while t_eof.elapsed() < Duration::from_millis(500) {
            if trace_tcp_disconnects() >= 1
                && loop_events.lock().map(|g| g.last().copied() == Some(false)).unwrap_or(false)
            {
                saw_false = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(saw_false, "TCP EOF did not notify disconnect within 500 ms");
        assert_eq!(trace_tcp_disconnects(), 1);
        stop.store(true, Ordering::SeqCst);
        let _ = jh.join();

        // Part B: Mirror runtime transition (`running` -> TCP loop drop to 0 -> `NativeVideoEvent::Stopped` < 500 ms)
        crate::mirror::global_reset_mirror_runtime_for_test();
        let _ = crate::mirror::start_airplay_mirror_session(
            Some("00008150-TEST-275"),
            false,
            "wifi",
            Some("127.0.0.1"),
            Some(0),
            false,
        )
        .expect("start loopback airplay session");
        let gen = crate::mirror::current_live_session_gen();
        let q = Arc::new(crate::mirror::BoundedNativeVideoQueue::new(32));
        crate::mirror::register_native_video_sink(Some(q.clone()));

        crate::mirror::global_on_airplay_mirror_loop_state(gen, true);
        crate::mirror::update_live_codec_h264_avcc(
            gen,
            "avc1.42801e",
            "AUKAHv/hAAJnQgEAAmjO",
            888,
            1920,
        );
        assert!(crate::mirror::global_on_physical_h264_avcc_frame(
            gen,
            vec![0, 0, 0, 4, 0x65, 0x88, 0x01, 0x02],
            true,
        ));
        while q.try_pop().is_some() {}
        let running_json = crate::mirror::global_screen_status_json();
        assert!(running_json.contains(r#""phase":"running""#), "{running_json}");

        let t_disc = Instant::now();
        crate::mirror::global_on_airplay_mirror_loop_state(gen, false);
        assert!(t_disc.elapsed() < Duration::from_millis(500));

        let mut saw_stopped = false;
        while let Some(ev) = q.try_pop() {
            if matches!(ev, crate::mirror::NativeVideoEvent::Stopped) {
                saw_stopped = true;
            }
        }
        assert!(saw_stopped, "Expected NativeVideoEvent::Stopped on last mirror TCP disconnect");
        let after_json = crate::mirror::global_screen_status_json();
        assert!(after_json.contains(r#""phase":"starting""#), "{after_json}");
        assert!(after_json.contains(r#""stage":"waiting_airplay""#), "{after_json}");

        let _ = crate::mirror::global_stop_screen();
        crate::mirror::register_native_video_sink(None);
        crate::mirror::global_reset_mirror_runtime_for_test();
        reset_airplay_trace();
    }

    #[test]
    fn test_tahap275_airplay_trace_json_exposes_ctr_bytes_disconnects_and_layout_clamps() {
        reset_airplay_trace();
        add_trace_ctr_keystream_bytes(65_432);
        inc_trace_tcp_disconnects();
        inc_trace_layout_rect_clamps();
        inc_trace_layout_rect_clamps();
        inc_trace_mft_dim_changes();
        set_trace_last_active_video_rect(0, 48, 346, 750);

        let c = get_airplay_trace_counters();
        assert_eq!(c.ctr_keystream_bytes, 65_432);
        assert_eq!(c.tcp_disconnects, 1);
        assert_eq!(c.layout_rect_clamps, 2);
        assert_eq!(c.mft_dim_changes, 1);
        assert_eq!(c.last_active_video_rect, "0,48,346,750");

        let json = get_airplay_trace_json();
        assert!(json.contains(r#""ctrKeystreamBytes":65432"#), "{json}");
        assert!(json.contains(r#""tcpDisconnects":1"#), "{json}");
        assert!(json.contains(r#""layoutRectClamps":2"#), "{json}");
        assert!(json.contains(r#""mftDimChanges":1"#), "{json}");
        assert!(json.contains(r#""lastActiveVideoRect":"0,48,346,750""#), "{json}");
        assert!(!json.contains("192.168."), "Zero PII allowed in trace: {json}");
        assert!(!json.contains("000081"), "Zero UDID PII allowed in trace: {json}");
        reset_airplay_trace();
    }

    #[test]
    fn test_tahap276_airplay_trace_exposes_low_latency_and_dcomp_commits_counters() {
        reset_airplay_trace();
        assert!(!trace_mf_low_latency_enabled());
        assert_eq!(trace_dcomp_commits(), 0);

        set_trace_mf_low_latency_enabled(true);
        inc_trace_dcomp_commits();
        inc_trace_dcomp_commits();
        inc_trace_dcomp_commits();
        inc_trace_tcp_video_packets();
        inc_trace_tcp_video_packets();
        update_native_video_stage_trace(
            2,
            0,
            2,
            2,
            1,
            0,
            0,
            2,
            2,
            2,
            0,
            0,
            Some((888, 1920)),
            Some((888, 1920)),
        );

        let c = get_airplay_trace_counters();
        assert!(c.mf_low_latency_enabled);
        assert_eq!(c.dcomp_commits, 3);
        assert_eq!(c.tcp_video_packets, 2);
        assert_eq!(c.decoder_fed, 2);
        assert_eq!(c.mf_decoded_frames, 2);
        assert_eq!(c.vram_published, 2);
        assert_eq!(c.dcomp_presented_frames, 2);

        let json = get_airplay_trace_json();
        assert!(json.contains(r#""mfLowLatencyEnabled":true"#), "{json}");
        assert!(json.contains(r#""dcompCommits":3"#), "{json}");
        assert!(json.contains(r#""tcpVideoPackets":2"#), "{json}");
        assert!(json.contains(r#""decoderFed":2"#), "{json}");
        assert!(json.contains(r#""mfDecodedFrames":2"#), "{json}");
        assert!(json.contains(r#""vramPublished":2"#), "{json}");
        assert!(json.contains(r#""dcompPresentedFrames":2"#), "{json}");
        assert!(json.contains(r#""events":["#), "{json}");
        assert!(!json.contains("192.168."), "Zero PII in trace JSON: {json}");
        assert!(!json.contains("000081"), "Zero UDID in trace JSON: {json}");
        reset_airplay_trace();
    }

    #[test]
    fn test_tahap278_airplay_trace_exposes_pre_decoder_drops_cached_idr_and_presented_rect() {
        reset_airplay_trace();
        assert_eq!(trace_pre_decoder_dropped_awaiting_avcc(), 0);
        assert_eq!(trace_pre_decoder_dropped_awaiting_idr(), 0);
        assert_eq!(trace_pre_decoder_dropped_stale_epoch(), 0);
        assert_eq!(trace_cached_idr_replays(), 0);
        assert_eq!(trace_last_presented_dest_rect(), "0,0,0,0");
        assert_eq!(trace_last_presented_rotation_deg(), 0);

        inc_trace_pre_decoder_dropped_awaiting_avcc();
        inc_trace_pre_decoder_dropped_awaiting_idr();
        inc_trace_pre_decoder_dropped_awaiting_idr();
        inc_trace_pre_decoder_dropped_stale_epoch();
        inc_trace_pre_decoder_dropped_stale_epoch();
        inc_trace_pre_decoder_dropped_stale_epoch();
        inc_trace_cached_idr_replays();
        set_trace_last_presented_dest_rect(0, 0, 519, 1125);
        set_trace_last_presented_rotation_deg(90);

        let c = get_airplay_trace_counters();
        assert_eq!(c.pre_decoder_dropped_awaiting_avcc, 1);
        assert_eq!(c.pre_decoder_dropped_awaiting_idr, 2);
        assert_eq!(c.pre_decoder_dropped_stale_epoch, 3);
        assert_eq!(c.cached_idr_replays, 1);
        assert_eq!(c.last_presented_dest_rect, "0,0,519,1125");
        assert_eq!(c.last_presented_rotation_deg, 90);

        let json = get_airplay_trace_json();
        assert!(json.contains(r#""preDecoderDroppedAwaitingAvcc":1"#), "{json}");
        assert!(json.contains(r#""preDecoderDroppedAwaitingIdr":2"#), "{json}");
        assert!(json.contains(r#""preDecoderDroppedStaleEpoch":3"#), "{json}");
        assert!(json.contains(r#""cachedIdrReplays":1"#), "{json}");
        assert!(json.contains(r#""lastPresentedDestRect":"0,0,519,1125""#), "{json}");
        assert!(json.contains(r#""lastPresentedRotationDeg":90"#), "{json}");
        assert!(!json.contains("192.168."), "Zero PII in trace JSON: {json}");
        assert!(!json.contains("000081"), "Zero UDID in trace JSON: {json}");
        reset_airplay_trace();
    }

    #[test]
    fn test_tahap279_airplay_trace_exposes_avcc_small_256_511_and_mft_reconfigs_zero_pii() {
        reset_airplay_trace();
        assert_eq!(trace_avcc_small_256_511_frames(), 0);
        assert_eq!(trace_mft_reconfigs(), 0);

        add_trace_avcc_small_256_511_frames(7);
        inc_trace_mft_reconfigs();
        add_trace_mft_reconfigs(2);
        set_trace_hd_shader_active(true);
        add_trace_hd_shader_presents(12);
        record_airplay_trace("mft_reconfig", "reason=avcc_dim_change dims=2160x994");

        let c = get_airplay_trace_counters();
        assert_eq!(c.avcc_small_256_511_frames, 7);
        assert_eq!(c.mft_reconfigs, 3);
        assert!(c.hd_shader_active);
        assert_eq!(c.hd_shader_presents, 12);

        let json = get_airplay_trace_json();
        assert!(json.contains(r#""avccSmall256To511Frames":7"#), "{json}");
        assert!(json.contains(r#""mftReconfigs":3"#), "{json}");
        assert!(json.contains(r#""hdShaderActive":true"#), "{json}");
        assert!(json.contains(r#""hdShaderPresents":12"#), "{json}");
        assert!(json.contains(r#""cachedIdrReplays":0"#), "{json}");
        assert!(json.contains(r#""staleDimDropped":0"#), "{json}");
        assert!(json.contains(r#""preDecoderDroppedAwaitingIdr":0"#), "{json}");
        assert!(json.contains(r#""lastPresentedDestRect":"0,0,0,0""#), "{json}");
        assert!(json.contains(r#""lastPresentedRotationDeg":0"#), "{json}");
        assert!(json.contains(r#""event":"mft_reconfig""#), "{json}");
        assert!(!json.contains("192.168."), "Zero PII in trace JSON: {json}");
        assert!(!json.contains("000081"), "Zero UDID in trace JSON: {json}");
        assert!(!json.contains("C:\\Users\\"), "Zero path PII in trace JSON: {json}");

        reset_airplay_trace();
        assert_eq!(trace_avcc_small_256_511_frames(), 0);
        assert_eq!(trace_mft_reconfigs(), 0);
    }
}
