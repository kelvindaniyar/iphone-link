//! Native AirPlay `SETUP type=96` Audio Receiver, Offline Multi-Codec Decoder, & Win32 `winmm::waveOut` Player.
//!
//! Pipeline (100% Pure Native Rust + bundled `zlaac.dll`, zero external crates, zero Bluetooth A2DP):
//! 1. Dual-socket non-blocking UDP receiver (`dataPort` RTP + `controlPort` RTCP Sync `0x80 0xD4`)
//!    with WinSock2 `SIO_UDP_CONNRESET = FALSE`, 2-fd `WSAPoll`, & RAII `winmm::timeBeginPeriod(1)`.
//! 2. Sequence reorder queue (`DEPTH = 3`, ~32 ms max wait) handling Wi-Fi jitter at ultra-low latency.
//! 3. Per-packet Windows CNG `AES-128-CBC` decryption (`sess.aeskey` + `sess.eiv`, whole 16B blocks only,
//!    trailing `< 16B` untouched — 1:1 `zenlink-airplay/src/audio.rs:48-56`).
//! 4. Offline Multi-Codec Decoder -> 44.1 kHz 16-bit interleaved stereo PCM:
//!    - `ct = 1`: Raw LPCM 16-bit big-endian stereo (pure Rust)
//!    - `ct = 2`: Apple Lossless ALAC 16-bit stereo (pure Rust uncompressed + compressed Rice/LPC)
//!    - `ct = 4`: AAC-LC 44.1 kHz stereo (bundled `zlaac.dll`, `ASC = [0x12, 0x10]`)
//!    - `ct = 8`: AAC-ELD 44.1 kHz stereo (bundled `zlaac.dll`, `ASC = [0xF8, 0xE8, 0x50, 0x00]`)
//! 5. Native Win32 `winmm::waveOut` 12-slot pinned `WAVEHDR` ring player (`44100 Hz`, 2ch, 16-bit PCM)
//!    with live zero-drop atomic hot-reload (`Volume PC` zipper-free ~5 ms ramp, `Buffer Audio`
//!    `low`/`normal`/`high`, `Lip Sync` adaptive alignment +55 ms), smooth drift trimming,
//!    RTSP `FLUSH` (`waveOutReset`), and `< 20 ms` leak-free teardown on `TEARDOWN type=96` / Stop.

use std::collections::{HashMap, VecDeque};
use std::ffi::c_void;
use std::io;
use std::net::UdpSocket;
use std::path::PathBuf;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::win_cng::Aes128CbcKey;

use super::{
    add_trace_audio_dropped_packets, add_trace_audio_played_samples, disable_udp_connreset,
    inc_trace_audio_ctrl_rx, inc_trace_audio_data_rx, inc_trace_audio_decode_error,
    inc_trace_audio_decoded, record_airplay_trace, set_trace_audio_codec, wait_two_udps_readable,
};

/// Output sample rate for AirPlay `waveOut` playback (matches iOS AirPlay 44.1 kHz standard).
pub const AIRPLAY_AUDIO_RATE: u32 = 44_100;
/// AudioSpecificConfig for iOS AirPlay AAC-ELD 44.1 kHz stereo (480-sample frames, `frameLengthFlag=1`).
pub const ASC_AAC_ELD_44K: [u8; 4] = [0xF8, 0xE8, 0x50, 0x00];
/// AudioSpecificConfig for AAC-LC 44.1 kHz stereo (AOT=2, freqIdx=4, ch=2).
pub const ASC_AAC_LC_44K: [u8; 2] = [0x12, 0x10];

/// Tahap 259 Pilar 2: Tight reorder depth (`3` packets = ~32 ms max wait vs 65 ms previously).
pub const REORDER_DEPTH: usize = 3;
/// Adaptive Lip Sync offset added to AirPlay audio target when Lip Sync is ON (`+55 ms`) so
/// audio aligns tightly with hardware WebCodecs `optimizeForLatency: true` H.264 video (~80-110 ms).
pub const AIRPLAY_LIPSYNC_OFFSET_MS: u32 = 55;
const ZLAAC_CAP_FRAMES: usize = 8192;
const WAVEOUT_SLOTS: usize = 12;
/// Stereo frames required for a full 0% -> 100% zipper-free smooth volume ramp (~5 ms @ 44.1 kHz).
const GAIN_RAMP_FRAMES: i32 = 220;
const GAIN_UNITY_Q15: i32 = 32_768;
const GAIN_STEP_Q15: i32 = GAIN_UNITY_Q15 / GAIN_RAMP_FRAMES;

// ============================================================================
// Global Atomic Live Hot-Reload Controls & Non-PII AirPlay Audio Telemetry
// ============================================================================

static AIRPLAY_AUDIO_MUTED: AtomicBool = AtomicBool::new(false);
static AIRPLAY_AUDIO_VOLUME_PCT: AtomicU32 = AtomicU32::new(100);
/// `0 = "low"`, `1 = "normal"`, `2 = "high"`.
static AIRPLAY_AUDIO_BUFFER_PRESET: AtomicU8 = AtomicU8::new(1);
static AIRPLAY_AUDIO_LIPSYNC_ENABLED: AtomicBool = AtomicBool::new(true);
static AIRPLAY_AUDIO_BUFFER_MS: AtomicU32 = AtomicU32::new(0);
static AIRPLAY_AUDIO_DROPPED_PACKETS: AtomicU64 = AtomicU64::new(0);

pub fn parse_buffer_preset_code(preset: &str) -> u8 {
    match preset.trim().to_ascii_lowercase().as_str() {
        "low" | "rendah" => 0,
        "high" | "tinggi" => 2,
        _ => 1,
    }
}

pub fn buffer_preset_label(code: u8) -> &'static str {
    match code {
        0 => "low",
        2 => "high",
        _ => "normal",
    }
}

/// Computes `(effective_target_ms, effective_cap_ms)` from live atomic preset & Lip Sync toggle:
/// - `"low"` (`0`): base `22 ms`, cap `55 ms`
/// - `"normal"` (`1`): base `45 ms`, cap `95 ms`
/// - `"high"` (`2`): base `90 ms`, cap `175 ms`
/// - `lip_sync == true`: adds `+55 ms` alignment offset to both target and cap.
pub fn compute_airplay_audio_targets(preset_code: u8, lip_sync: bool) -> (u32, u32) {
    let (base_target, base_cap) = match preset_code {
        0 => (22u32, 55u32),
        2 => (90u32, 175u32),
        _ => (45u32, 95u32),
    };
    let offset = if lip_sync { AIRPLAY_LIPSYNC_OFFSET_MS } else { 0 };
    (base_target + offset, base_cap + offset)
}

pub fn current_airplay_audio_targets() -> (u32, u32) {
    compute_airplay_audio_targets(
        AIRPLAY_AUDIO_BUFFER_PRESET.load(Ordering::Relaxed),
        AIRPLAY_AUDIO_LIPSYNC_ENABLED.load(Ordering::Relaxed),
    )
}

#[derive(Clone, Debug)]
pub struct AirPlayAudioSnapshot {
    pub active: bool,
    pub muted: bool,
    pub codec: Option<String>,
    pub sample_rate: u32,
    pub decoded_packets: u64,
    pub played_samples: u64,
    pub decode_errors: u64,
    pub volume: u32,
    pub buffer_preset: String,
    pub lip_sync: bool,
    pub buffer_ms: u32,
    pub target_ms: u32,
    pub dropped_packets: u64,
}

impl Default for AirPlayAudioSnapshot {
    fn default() -> Self {
        let (target_ms, _) = compute_airplay_audio_targets(1, true);
        Self {
            active: false,
            muted: false,
            codec: None,
            sample_rate: 0,
            decoded_packets: 0,
            played_samples: 0,
            decode_errors: 0,
            volume: 100,
            buffer_preset: "normal".to_string(),
            lip_sync: true,
            buffer_ms: 0,
            target_ms,
            dropped_packets: 0,
        }
    }
}

static AIRPLAY_AUDIO_STATE: OnceLock<Mutex<AirPlayAudioSnapshot>> = OnceLock::new();

fn audio_state() -> &'static Mutex<AirPlayAudioSnapshot> {
    AIRPLAY_AUDIO_STATE.get_or_init(|| Mutex::new(AirPlayAudioSnapshot::default()))
}

pub fn set_airplay_audio_muted(muted: bool) {
    AIRPLAY_AUDIO_MUTED.store(muted, Ordering::SeqCst);
    if let Ok(mut st) = audio_state().lock() {
        st.muted = muted;
    }
}

pub fn is_airplay_audio_muted() -> bool {
    AIRPLAY_AUDIO_MUTED.load(Ordering::SeqCst)
}

pub fn get_airplay_audio_volume() -> u32 {
    AIRPLAY_AUDIO_VOLUME_PCT.load(Ordering::SeqCst).min(100)
}

pub fn get_airplay_audio_buffer_preset() -> &'static str {
    buffer_preset_label(AIRPLAY_AUDIO_BUFFER_PRESET.load(Ordering::SeqCst))
}

pub fn is_airplay_audio_lipsync_enabled() -> bool {
    AIRPLAY_AUDIO_LIPSYNC_ENABLED.load(Ordering::SeqCst)
}

/// Live atomic hot-reload of AirPlay audio parameters (`0 ms` dropout, zero worker restart,
/// zero UDP socket close, zero `waveOut` close, and 100% isolated from Bluetooth A2DP!).
pub fn apply_live_audio_config(
    volume: Option<u32>,
    buffer_preset: Option<&str>,
    lip_sync: Option<bool>,
    muted: Option<bool>,
) -> AirPlayAudioSnapshot {
    if let Some(v) = volume {
        AIRPLAY_AUDIO_VOLUME_PCT.store(v.min(100), Ordering::SeqCst);
    }
    if let Some(p) = buffer_preset {
        let code = parse_buffer_preset_code(p);
        AIRPLAY_AUDIO_BUFFER_PRESET.store(code, Ordering::SeqCst);
    }
    if let Some(ls) = lip_sync {
        AIRPLAY_AUDIO_LIPSYNC_ENABLED.store(ls, Ordering::SeqCst);
    }
    if let Some(m) = muted {
        AIRPLAY_AUDIO_MUTED.store(m, Ordering::SeqCst);
    }
    let vol_now = AIRPLAY_AUDIO_VOLUME_PCT.load(Ordering::SeqCst).min(100);
    let preset_code = AIRPLAY_AUDIO_BUFFER_PRESET.load(Ordering::SeqCst);
    let preset_str = buffer_preset_label(preset_code).to_string();
    let ls_now = AIRPLAY_AUDIO_LIPSYNC_ENABLED.load(Ordering::SeqCst);
    let muted_now = AIRPLAY_AUDIO_MUTED.load(Ordering::SeqCst);
    let (target_ms, _) = compute_airplay_audio_targets(preset_code, ls_now);
    let buf_ms = AIRPLAY_AUDIO_BUFFER_MS.load(Ordering::Relaxed);
    let dropped = AIRPLAY_AUDIO_DROPPED_PACKETS.load(Ordering::Relaxed);

    if let Ok(mut st) = audio_state().lock() {
        st.volume = vol_now;
        st.buffer_preset = preset_str.clone();
        st.lip_sync = ls_now;
        st.muted = muted_now;
        st.target_ms = target_ms;
        st.buffer_ms = buf_ms;
        st.dropped_packets = dropped;
        st.clone()
    } else {
        AirPlayAudioSnapshot {
            muted: muted_now,
            volume: vol_now,
            buffer_preset: preset_str,
            lip_sync: ls_now,
            buffer_ms: buf_ms,
            target_ms,
            dropped_packets: dropped,
            ..Default::default()
        }
    }
}

pub fn reset_airplay_audio_stats() {
    let muted = AIRPLAY_AUDIO_MUTED.load(Ordering::SeqCst);
    let vol = AIRPLAY_AUDIO_VOLUME_PCT.load(Ordering::SeqCst).min(100);
    let preset_code = AIRPLAY_AUDIO_BUFFER_PRESET.load(Ordering::SeqCst);
    let ls = AIRPLAY_AUDIO_LIPSYNC_ENABLED.load(Ordering::SeqCst);
    let (target_ms, _) = compute_airplay_audio_targets(preset_code, ls);
    AIRPLAY_AUDIO_BUFFER_MS.store(0, Ordering::SeqCst);
    AIRPLAY_AUDIO_DROPPED_PACKETS.store(0, Ordering::SeqCst);
    if let Ok(mut st) = audio_state().lock() {
        *st = AirPlayAudioSnapshot {
            active: false,
            muted,
            codec: None,
            sample_rate: 0,
            decoded_packets: 0,
            played_samples: 0,
            decode_errors: 0,
            volume: vol,
            buffer_preset: buffer_preset_label(preset_code).to_string(),
            lip_sync: ls,
            buffer_ms: 0,
            target_ms,
            dropped_packets: 0,
        };
    }
}

pub fn mark_airplay_audio_inactive() {
    AIRPLAY_AUDIO_BUFFER_MS.store(0, Ordering::Relaxed);
    if let Ok(mut st) = audio_state().lock() {
        st.active = false;
        st.buffer_ms = 0;
    }
}

pub fn get_airplay_audio_snapshot() -> AirPlayAudioSnapshot {
    let muted = AIRPLAY_AUDIO_MUTED.load(Ordering::SeqCst);
    let vol = AIRPLAY_AUDIO_VOLUME_PCT.load(Ordering::SeqCst).min(100);
    let preset_code = AIRPLAY_AUDIO_BUFFER_PRESET.load(Ordering::SeqCst);
    let preset_str = buffer_preset_label(preset_code).to_string();
    let ls = AIRPLAY_AUDIO_LIPSYNC_ENABLED.load(Ordering::SeqCst);
    let (target_ms, _) = compute_airplay_audio_targets(preset_code, ls);
    let buf_ms = AIRPLAY_AUDIO_BUFFER_MS.load(Ordering::Relaxed);
    let dropped = AIRPLAY_AUDIO_DROPPED_PACKETS.load(Ordering::Relaxed);
    audio_state()
        .lock()
        .map(|s| {
            let mut snap = s.clone();
            snap.muted = muted;
            snap.volume = vol;
            snap.buffer_preset = preset_str.clone();
            snap.lip_sync = ls;
            snap.target_ms = target_ms;
            snap.buffer_ms = if snap.active { buf_ms } else { 0 };
            snap.dropped_packets = dropped;
            snap
        })
        .unwrap_or(AirPlayAudioSnapshot {
            muted,
            volume: vol,
            buffer_preset: preset_str,
            lip_sync: ls,
            buffer_ms: 0,
            target_ms,
            dropped_packets: dropped,
            ..Default::default()
        })
}

fn codec_label_for_ct(ct: u64) -> &'static str {
    match ct {
        1 => "pcm",
        2 => "alac",
        4 => "aac-lc",
        8 => "aac-eld",
        _ => "aac-eld",
    }
}

// ============================================================================
// RTP Sequence Reorder Buffer (`DEPTH = 3`, ~32 ms max wait)
// ============================================================================

#[derive(Default)]
struct RtpReorderQueue {
    next: Option<u16>,
    pending: HashMap<u16, Vec<u8>>,
}

impl RtpReorderQueue {
    fn clear(&mut self) {
        self.next = None;
        self.pending.clear();
    }

    fn push(&mut self, seq: u16, packet: Vec<u8>) -> Vec<Vec<u8>> {
        let next = *self.next.get_or_insert(seq);
        if seq.wrapping_sub(next) > 0x8000 {
            return Vec::new();
        }
        self.pending.insert(seq, packet);
        let mut out = Vec::new();
        loop {
            let n = self.next.unwrap();
            if let Some(p) = self.pending.remove(&n) {
                out.push(p);
            } else if self.pending.len() <= REORDER_DEPTH {
                return out;
            }
            self.next = Some(n.wrapping_add(1));
        }
    }
}

// ============================================================================
// Bundled zlaac.dll Dynamic Loader (AAC-ELD & AAC-LC Offline Decoder)
// ============================================================================

type ZlAacOpenFn = unsafe extern "C" fn(*const u8, i32, i32, i32, i32) -> *mut c_void;
type ZlAacDecodeFn = unsafe extern "C" fn(*mut c_void, *const u8, i32, *mut i16, i32) -> i32;
type ZlAacCloseFn = unsafe extern "C" fn(*mut c_void);

#[cfg(windows)]
struct ZlAacApi {
    _h_mod: *mut c_void,
    open: ZlAacOpenFn,
    decode: ZlAacDecodeFn,
    close: ZlAacCloseFn,
}

#[cfg(windows)]
unsafe impl Send for ZlAacApi {}
#[cfg(windows)]
unsafe impl Sync for ZlAacApi {}

#[cfg(windows)]
static ZLAAC_API: OnceLock<Result<ZlAacApi, String>> = OnceLock::new();

#[cfg(windows)]
fn resolve_zlaac_dll_path() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("zlaac.dll"));
            candidates.push(dir.join("bin").join("zlaac.dll"));
        }
        for anc in exe.ancestors().skip(1).take(6) {
            candidates.push(anc.join("bins").join("zlaac.dll"));
            candidates.push(anc.join("rust-v4").join("bins").join("zlaac.dll"));
            candidates.push(anc.join("handset").join("bin").join("zlaac.dll"));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("zlaac.dll"));
        candidates.push(cwd.join("rust-v4").join("bins").join("zlaac.dll"));
        candidates.push(cwd.join("handset").join("bin").join("zlaac.dll"));
        for anc in cwd.ancestors().skip(1).take(4) {
            candidates.push(anc.join("bins").join("zlaac.dll"));
            candidates.push(anc.join("rust-v4").join("bins").join("zlaac.dll"));
            candidates.push(anc.join("handset").join("bin").join("zlaac.dll"));
        }
    }
    candidates.into_iter().find(|p| p.is_file())
}

#[cfg(windows)]
fn load_zlaac_api() -> Result<&'static ZlAacApi, String> {
    let res = ZLAAC_API.get_or_init(|| {
        use std::os::windows::ffi::OsStrExt;

        #[link(name = "kernel32")]
        extern "system" {
            fn LoadLibraryW(lp_lib_file_name: *const u16) -> *mut c_void;
            fn GetProcAddress(h_module: *mut c_void, lp_proc_name: *const u8) -> *const c_void;
        }

        let dll = resolve_zlaac_dll_path().ok_or_else(|| "zlaac.dll not found".to_string())?;
        let wide: Vec<u16> = dll
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let h_mod = unsafe { LoadLibraryW(wide.as_ptr()) };
        if h_mod.is_null() {
            return Err("LoadLibraryW(zlaac.dll) failed".to_string());
        }
        unsafe {
            let p_open = GetProcAddress(h_mod, b"zl_aac_open\0".as_ptr());
            let p_dec = GetProcAddress(h_mod, b"zl_aac_decode\0".as_ptr());
            let p_close = GetProcAddress(h_mod, b"zl_aac_close\0".as_ptr());
            if p_open.is_null() || p_dec.is_null() || p_close.is_null() {
                return Err("zlaac.dll missing expected exports".to_string());
            }
            Ok(ZlAacApi {
                _h_mod: h_mod,
                open: std::mem::transmute::<*const c_void, ZlAacOpenFn>(p_open),
                decode: std::mem::transmute::<*const c_void, ZlAacDecodeFn>(p_dec),
                close: std::mem::transmute::<*const c_void, ZlAacCloseFn>(p_close),
            })
        }
    });
    res.as_ref().map_err(|e| e.clone())
}

#[cfg(windows)]
pub(crate) struct ZlAacHandle {
    api: &'static ZlAacApi,
    ctx: *mut c_void,
    out_buf: Vec<i16>,
}

#[cfg(windows)]
impl ZlAacHandle {
    fn open(asc: &[u8], in_rate: u32, out_rate: u32) -> Result<Self, String> {
        let api = load_zlaac_api()?;
        let ctx = unsafe {
            (api.open)(
                asc.as_ptr(),
                asc.len() as i32,
                in_rate as i32,
                2,
                out_rate as i32,
            )
        };
        if ctx.is_null() {
            return Err("zl_aac_open returned null".to_string());
        }
        Ok(Self {
            api,
            ctx,
            out_buf: vec![0i16; ZLAAC_CAP_FRAMES * 2],
        })
    }

    fn decode(&mut self, au: &[u8]) -> Result<Vec<i16>, String> {
        if au.is_empty() {
            return Ok(Vec::new());
        }
        // zlaac.c: returns stereo FRAMES written (each stereo frame = 2 interleaved i16 samples!).
        let frames = unsafe {
            (self.api.decode)(
                self.ctx,
                au.as_ptr(),
                au.len() as i32,
                self.out_buf.as_mut_ptr(),
                ZLAAC_CAP_FRAMES as i32,
            )
        };
        if frames < 0 {
            return Err(format!("zl_aac_decode error {frames}"));
        }
        let n_samples = ((frames as usize).min(ZLAAC_CAP_FRAMES)) * 2;
        Ok(self.out_buf[..n_samples].to_vec())
    }
}

#[cfg(windows)]
impl Drop for ZlAacHandle {
    fn drop(&mut self) {
        if !self.ctx.is_null() {
            unsafe { (self.api.close)(self.ctx) };
            self.ctx = ptr::null_mut();
        }
    }
}

// ============================================================================
// Pure-Rust ALAC 16-bit Stereo Frame Decoder (ct = 2: Uncompressed + Compressed)
// ============================================================================

struct BitReader<'a> {
    data: &'a [u8],
    bit_pos: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, bit_pos: 0 }
    }

    fn bits_left(&self) -> usize {
        self.data.len().saturating_mul(8).saturating_sub(self.bit_pos)
    }

    fn read_bits(&mut self, count: usize) -> Option<u32> {
        if count == 0 {
            return Some(0);
        }
        if count > 32 || self.bits_left() < count {
            return None;
        }
        let mut val = 0u32;
        for _ in 0..count {
            let byte = self.data[self.bit_pos >> 3];
            let shift = 7 - (self.bit_pos & 7);
            val = (val << 1) | (((byte >> shift) & 1) as u32);
            self.bit_pos += 1;
        }
        Some(val)
    }

    fn read_signed_16(&mut self) -> Option<i16> {
        self.read_bits(16).map(|v| v as u16 as i16)
    }
}

/// Decodes one Apple Lossless (ALAC) 16-bit stereo frame (uncompressed escape or compressed
/// Rice/LPC stereo decorrelated frame) into interleaved little-endian stereo `i16` PCM.
pub fn decode_alac_stereo_frame(frame: &[u8], default_spf: usize) -> Result<Vec<i16>, String> {
    if frame.len() < 4 {
        return Err("ALAC frame too short".to_string());
    }
    let mut br = BitReader::new(frame);
    let tag = br.read_bits(3).ok_or("ALAC tag EOF")?;
    if tag != 1 && tag != 0 {
        return Err(format!("ALAC unsupported element tag {tag}"));
    }
    let _elem_inst = br.read_bits(4).ok_or("ALAC inst EOF")?;
    let _unused = br.read_bits(12).ok_or("ALAC header EOF")?;
    let has_size = br.read_bits(1).ok_or("ALAC has_size EOF")? != 0;
    let unused_bytes = br.read_bits(2).ok_or("ALAC shift EOF")? as usize;
    let is_uncompressed = br.read_bits(1).ok_or("ALAC escape EOF")? != 0;
    let num_frames = if has_size {
        br.read_bits(32).ok_or("ALAC sample count EOF")? as usize
    } else {
        default_spf.max(1)
    };
    if num_frames == 0 || num_frames > 16_384 {
        return Err(format!("ALAC invalid frame count {num_frames}"));
    }

    let channels = if tag == 1 { 2usize } else { 1usize };
    if is_uncompressed {
        let mut out = Vec::with_capacity(num_frames * 2);
        for _ in 0..num_frames {
            if channels == 2 {
                let l = br.read_signed_16().ok_or("ALAC raw L EOF")?;
                let r = br.read_signed_16().ok_or("ALAC raw R EOF")?;
                out.push(l);
                out.push(r);
            } else {
                let m = br.read_signed_16().ok_or("ALAC raw M EOF")?;
                out.push(m);
                out.push(m);
            }
        }
        return Ok(out);
    }

    // Compressed ALAC stereo/mono path (Apple ALAC spec: interlacing + adaptive Rice + FIR LPC)
    let _shift = unused_bytes * 8;
    let (interlacing_shift, interlacing_leftweight) = if channels == 2 {
        (
            br.read_bits(8).ok_or("ALAC il_shift EOF")? as u8,
            br.read_bits(8).ok_or("ALAC il_weight EOF")? as u8,
        )
    } else {
        (0u8, 0u8)
    };

    let mut ch_preds: Vec<(u8, u8, u8, Vec<i16>)> = Vec::with_capacity(channels);
    for _ in 0..channels {
        let _pred_type = br.read_bits(4).ok_or("ALAC pred_type EOF")?;
        let pred_quant = br.read_bits(4).ok_or("ALAC pred_quant EOF")? as u8;
        let rice_mod = br.read_bits(3).ok_or("ALAC rice_mod EOF")? as u8;
        let order = br.read_bits(5).ok_or("ALAC order EOF")? as usize;
        let mut coefs = Vec::with_capacity(order);
        for _ in 0..order {
            coefs.push(br.read_signed_16().ok_or("ALAC coef EOF")?);
        }
        ch_preds.push((pred_quant, rice_mod, order as u8, coefs));
    }

    let mut ch_samples: Vec<Vec<i32>> = vec![vec![0i32; num_frames]; channels];
    let sample_bits = if channels == 2 && interlacing_leftweight != 0 {
        17usize
    } else {
        16usize
    };

    for ch in 0..channels {
        let mut hist: u32 = 10; // initial history (Apple ALAC default kb=10, mb=10, pb=40)
        let mut sign_mod: i32 = 0;
        let mut i = 0usize;
        while i < num_frames {
            let k = (31u32.saturating_sub((hist >> 9).saturating_add(3).leading_zeros())).min(14);
            // Count leading 1-bits (unary quotient up to 9 -> escape)
            let mut q = 0u32;
            while q < 9 {
                let bit = br.read_bits(1).ok_or("ALAC rice EOF")?;
                if bit == 0 {
                    break;
                }
                q += 1;
            }
            let val_u = if q >= 9 {
                br.read_bits(sample_bits).ok_or("ALAC escape bits EOF")?
            } else if k > 1 {
                let extra = br.read_bits(k as usize).ok_or("ALAC rice rem EOF")?;
                let thresh = (1u32 << k) - 1;
                if extra > 0 {
                    // Standard ALAC truncated binary coding
                    (q * thresh) + extra - 1
                } else {
                    q * thresh
                }
            } else {
                q
            };
            let val_adj = (val_u as i32) + sign_mod;
            sign_mod = 0;
            let signed_val = if (val_adj & 1) != 0 {
                -((val_adj + 1) >> 1)
            } else {
                val_adj >> 1
            };
            ch_samples[ch][i] = signed_val;
            hist = hist
                .saturating_add((val_u.saturating_mul(40)).saturating_sub((hist.saturating_mul(40)) >> 9))
                .min(0xFFFF_0000);
            i += 1;
        }

        // Apply FIR LPC reconstruction in place
        let (quant, _rice_mod, order_u8, ref coefs) = ch_preds[ch];
        let order = order_u8 as usize;
        if order > 0 && num_frames > 1 {
            let mut c = coefs.clone();
            let s = &mut ch_samples[ch];
            let mut prev = s[0];
            for idx in 1..num_frames.min(order + 1) {
                prev = prev.wrapping_add(s[idx]);
                s[idx] = prev;
            }
            for idx in (order + 1)..num_frames {
                let err = s[idx];
                let base = s[idx - order - 1];
                let mut sum: i64 = 0;
                for j in 0..order {
                    sum += (s[idx - 1 - j] - base) as i64 * (c[j] as i64);
                }
                let pred = base + (((sum + (1i64 << (quant.saturating_sub(1)))) >> quant) as i32);
                let out_val = pred.wrapping_add(err);
                s[idx] = out_val;
                if err != 0 {
                    let sign = if err > 0 { 1 } else { -1 };
                    for j in 0..order {
                        let d = s[idx - 1 - j] - base;
                        if d != 0 {
                            let adj = if d > 0 { sign } else { -sign };
                            c[j] = c[j].saturating_add(adj as i16);
                        }
                    }
                }
            }
        }
    }

    let mut out = Vec::with_capacity(num_frames * 2);
    if channels == 2 {
        for i in 0..num_frames {
            let (l, r) = if interlacing_leftweight != 0 {
                let m = ch_samples[0][i];
                let s = ch_samples[1][i];
                let r_val = m - ((s * (interlacing_leftweight as i32)) >> interlacing_shift);
                let l_val = r_val + s;
                (l_val, r_val)
            } else {
                (ch_samples[0][i], ch_samples[1][i])
            };
            out.push(l.clamp(i16::MIN as i32, i16::MAX as i32) as i16);
            out.push(r.clamp(i16::MIN as i32, i16::MAX as i32) as i16);
        }
    } else {
        for i in 0..num_frames {
            let m = ch_samples[0][i].clamp(i16::MIN as i32, i16::MAX as i32) as i16;
            out.push(m);
            out.push(m);
        }
    }
    Ok(out)
}

// ============================================================================
// Unified Multi-Codec Offline AirPlay Audio Decoder
// ============================================================================

pub(crate) enum AirPlayAudioDecoder {
    Lpcm,
    Alac { default_spf: usize },
    #[cfg(windows)]
    AacLc(ZlAacHandle),
    #[cfg(windows)]
    AacEld(ZlAacHandle),
    #[cfg(not(windows))]
    Unsupported,
}

impl AirPlayAudioDecoder {
    pub fn open(ct: u64, sample_rate: u32, spf: usize) -> Result<Self, String> {
        let in_rate = if sample_rate == 0 {
            AIRPLAY_AUDIO_RATE
        } else {
            sample_rate
        };
        match ct {
            1 => Ok(Self::Lpcm),
            2 => Ok(Self::Alac {
                default_spf: if spf == 0 { 352 } else { spf },
            }),
            #[cfg(windows)]
            4 => Ok(Self::AacLc(ZlAacHandle::open(
                &ASC_AAC_LC_44K,
                in_rate,
                AIRPLAY_AUDIO_RATE,
            )?)),
            #[cfg(windows)]
            8 => Ok(Self::AacEld(ZlAacHandle::open(
                &ASC_AAC_ELD_44K,
                in_rate,
                AIRPLAY_AUDIO_RATE,
            )?)),
            #[cfg(windows)]
            _ => Ok(Self::AacEld(ZlAacHandle::open(
                &ASC_AAC_ELD_44K,
                in_rate,
                AIRPLAY_AUDIO_RATE,
            )?)),
            #[cfg(not(windows))]
            _ => Ok(Self::Unsupported),
        }
    }

    /// Decodes one decrypted RTP audio payload into interleaved 44.1 kHz 16-bit stereo PCM (`Vec<i16>`).
    pub fn decode_payload(&mut self, payload: &[u8]) -> Result<Vec<i16>, String> {
        if payload.is_empty() {
            return Ok(Vec::new());
        }
        match self {
            Self::Lpcm => {
                // Big-endian signed 16-bit interleaved stereo LPCM -> native little-endian i16
                let pairs = payload.len() / 2;
                let mut out = Vec::with_capacity(pairs);
                for chunk in payload.chunks_exact(2) {
                    out.push(i16::from_be_bytes([chunk[0], chunk[1]]));
                }
                Ok(out)
            }
            Self::Alac { default_spf } => decode_alac_stereo_frame(payload, *default_spf),
            #[cfg(windows)]
            Self::AacLc(h) | Self::AacEld(h) => h.decode(payload),
            #[cfg(not(windows))]
            Self::Unsupported => Err("AAC decoder unavailable on non-Windows host".to_string()),
        }
    }
}

// ============================================================================
// Zipper-Free Per-Sample Smooth Gain Ramp (~5 ms @ 44.1 kHz, Q15 i32 Math)
// ============================================================================

/// Smoothly ramps stereo `i16` gain over `~5 ms` (`220` stereo frames) using exact Q15 `i32`
/// arithmetic so live slider moves (`0% <-> 100%`) and Mute/Unmute never produce clicks/pops.
#[derive(Clone, Debug)]
pub struct SmoothGainRamp {
    current_q15: i32,
}

impl SmoothGainRamp {
    pub fn new(initial_volume_pct: u32, initial_muted: bool) -> Self {
        Self {
            current_q15: Self::target_q15(initial_volume_pct, initial_muted),
        }
    }

    pub fn target_q15(volume_pct: u32, muted: bool) -> i32 {
        if muted || volume_pct == 0 {
            0
        } else if volume_pct >= 100 {
            GAIN_UNITY_Q15
        } else {
            ((volume_pct as i32) * GAIN_UNITY_Q15 + 50) / 100
        }
    }

    /// Applies smooth per-sample volume ramp in place across interleaved stereo `[L, R, ...]`.
    pub fn apply_stereo_in_place(&mut self, samples: &mut [i16], volume_pct: u32, muted: bool) {
        let target = Self::target_q15(volume_pct, muted);
        if self.current_q15 == target {
            if target == 0 {
                samples.fill(0);
                return;
            }
            if target == GAIN_UNITY_Q15 {
                return;
            }
        }
        let step = GAIN_STEP_Q15.max(1);
        for frame in samples.chunks_exact_mut(2) {
            if self.current_q15 < target {
                self.current_q15 = (self.current_q15 + step).min(target);
            } else if self.current_q15 > target {
                self.current_q15 = (self.current_q15 - step).max(target);
            }
            let g = self.current_q15;
            if g == 0 {
                frame[0] = 0;
                frame[1] = 0;
            } else if g != GAIN_UNITY_Q15 {
                frame[0] = (((frame[0] as i32) * g) >> 15)
                    .clamp(i16::MIN as i32, i16::MAX as i32) as i16;
                frame[1] = (((frame[1] as i32) * g) >> 15)
                    .clamp(i16::MIN as i32, i16::MAX as i32) as i16;
            }
        }
    }
}

// ============================================================================
// Adaptive Lip Sync & Smooth Drift Trimming Jitter Queue (Platform-Independent)
// ============================================================================

#[inline]
pub fn ms_to_stereo_samples(ms: u32, sample_rate: u32) -> usize {
    let sr = if sample_rate == 0 {
        AIRPLAY_AUDIO_RATE
    } else {
        sample_rate
    } as usize;
    (sr * 2 * (ms as usize)) / 1000
}

#[inline]
pub fn stereo_samples_to_ms(samples: usize, sample_rate: u32) -> u32 {
    let sr = if sample_rate == 0 {
        AIRPLAY_AUDIO_RATE
    } else {
        sample_rate
    } as u64;
    ((samples as u64) * 1000 / (sr * 2)) as u32
}

#[derive(Clone, Debug)]
pub struct AirPlayAudioJitterQueue {
    sample_rate: u32,
    pending: VecDeque<Vec<i16>>,
    pending_samples: usize,
    prefilled: bool,
    prefill_started_at: Option<Instant>,
    gain_ramp: SmoothGainRamp,
    pub total_dropped_packets: u64,
}

impl AirPlayAudioJitterQueue {
    pub fn new(sample_rate: u32) -> Self {
        let rate = if sample_rate == 0 {
            AIRPLAY_AUDIO_RATE
        } else {
            sample_rate
        };
        Self {
            sample_rate: rate,
            pending: VecDeque::with_capacity(32),
            pending_samples: 0,
            prefilled: false,
            prefill_started_at: None,
            gain_ramp: SmoothGainRamp::new(get_airplay_audio_volume(), is_airplay_audio_muted()),
            total_dropped_packets: 0,
        }
    }

    pub fn clear(&mut self) {
        self.pending.clear();
        self.pending_samples = 0;
        self.prefilled = false;
        self.prefill_started_at = None;
    }

    pub fn pending_samples(&self) -> usize {
        self.pending_samples
    }

    /// Pushes one decoded stereo PCM packet (`i16` interleaved), applies Smooth Drift Trimming
    /// whenever `in_flight_samples + pending_samples > effective_cap_samples`, and returns
    /// `(dropped_now, queued_ms, target_ms)`.
    pub fn push_packet(
        &mut self,
        samples: Vec<i16>,
        in_flight_samples: usize,
        now: Instant,
    ) -> (u64, u32, u32) {
        let (target_ms, cap_ms) = current_airplay_audio_targets();
        if !samples.is_empty() {
            if !self.prefilled && self.prefill_started_at.is_none() {
                self.prefill_started_at = Some(now);
            }
            self.pending_samples = self.pending_samples.saturating_add(samples.len());
            self.pending.push_back(samples);
        }
        let dropped_now = self.trim_drift_if_needed(in_flight_samples, target_ms, cap_ms);
        let queued_ms =
            stereo_samples_to_ms(in_flight_samples.saturating_add(self.pending_samples), self.sample_rate);
        (dropped_now, queued_ms, target_ms)
    }

    fn trim_drift_if_needed(
        &mut self,
        in_flight_samples: usize,
        target_ms: u32,
        cap_ms: u32,
    ) -> u64 {
        let cap_samples = ms_to_stereo_samples(cap_ms, self.sample_rate).max(960);
        let trim_to_samples = ms_to_stereo_samples(target_ms.saturating_add(20), self.sample_rate)
            .min(cap_samples)
            .max(960);
        let mut dropped_now = 0u64;
        if in_flight_samples.saturating_add(self.pending_samples) > cap_samples {
            while in_flight_samples.saturating_add(self.pending_samples) > trim_to_samples
                && self.pending.len() > 1
            {
                if let Some(old) = self.pending.pop_front() {
                    self.pending_samples = self.pending_samples.saturating_sub(old.len());
                    dropped_now = dropped_now.saturating_add(1);
                } else {
                    break;
                }
            }
        }
        if dropped_now > 0 {
            self.total_dropped_packets = self.total_dropped_packets.saturating_add(dropped_now);
        }
        dropped_now
    }

    /// Pops ready PCM blocks for submission while keeping OS `in_flight` bounded so live
    /// Volume / Buffer / Lip Sync changes & Smooth Drift Trimming take effect without dropouts.
    pub fn drain_ready_blocks(
        &mut self,
        mut in_flight_samples: usize,
        max_slots_available: usize,
        now: Instant,
    ) -> (Vec<Vec<i16>>, u64, u32, u32) {
        let (target_ms, cap_ms) = current_airplay_audio_targets();
        let dropped_now = self.trim_drift_if_needed(in_flight_samples, target_ms, cap_ms);
        if self.pending.is_empty() || max_slots_available == 0 {
            let queued_ms = stereo_samples_to_ms(
                in_flight_samples.saturating_add(self.pending_samples),
                self.sample_rate,
            );
            return (Vec::new(), dropped_now, queued_ms, target_ms);
        }

        let target_samples = ms_to_stereo_samples(target_ms, self.sample_rate);
        if !self.prefilled {
            let started = *self.prefill_started_at.get_or_insert(now);
            let wait_limit = Duration::from_millis((target_ms as u64).saturating_add(20));
            if self.pending_samples < target_samples && now.duration_since(started) < wait_limit {
                let queued_ms = stereo_samples_to_ms(
                    in_flight_samples.saturating_add(self.pending_samples),
                    self.sample_rate,
                );
                return (Vec::new(), dropped_now, queued_ms, target_ms);
            }
            self.prefilled = true;
            self.prefill_started_at = None;
        }

        // Keep OS driver in-flight queue bounded to ~max(target_ms, 35 ms) <= cap_ms so:
        // (1) speaker never underruns (>= 35 ms cushion), AND
        // (2) sudden bursts stay in `pending` where Smooth Drift Trimming pops excess cleanly, AND
        // (3) live Volume / Buffer / Lip Sync edits take effect within ~35 ms with 0 dropout!
        let in_flight_budget_ms = target_ms.max(35).min(cap_ms);
        let in_flight_budget_samples =
            ms_to_stereo_samples(in_flight_budget_ms, self.sample_rate).max(1920);
        let vol = get_airplay_audio_volume();
        let muted = is_airplay_audio_muted();

        let mut out = Vec::new();
        while out.len() < max_slots_available {
            if in_flight_samples >= in_flight_budget_samples && in_flight_samples > 0 {
                break;
            }
            let Some(mut block) = self.pending.pop_front() else {
                break;
            };
            self.pending_samples = self.pending_samples.saturating_sub(block.len());
            if block.is_empty() {
                continue;
            }
            self.gain_ramp.apply_stereo_in_place(&mut block, vol, muted);
            in_flight_samples = in_flight_samples.saturating_add(block.len());
            out.push(block);
        }

        let queued_ms = stereo_samples_to_ms(
            in_flight_samples.saturating_add(self.pending_samples),
            self.sample_rate,
        );
        (out, dropped_now, queued_ms, target_ms)
    }
}

// ============================================================================
// RAII Win32 `timeBeginPeriod(1)` Timer Resolution Guard (`1 ms` OS wakeup)
// ============================================================================

#[cfg(windows)]
struct ScopedTimerResolution(u32);

#[cfg(windows)]
impl ScopedTimerResolution {
    fn new(period_ms: u32) -> Self {
        #[link(name = "winmm")]
        extern "system" {
            fn timeBeginPeriod(u_period: u32) -> u32;
        }
        unsafe {
            let _ = timeBeginPeriod(period_ms);
        }
        Self(period_ms)
    }
}

#[cfg(windows)]
impl Drop for ScopedTimerResolution {
    fn drop(&mut self) {
        #[link(name = "winmm")]
        extern "system" {
            fn timeEndPeriod(u_period: u32) -> u32;
        }
        unsafe {
            let _ = timeEndPeriod(self.0);
        }
    }
}

#[cfg(not(windows))]
struct ScopedTimerResolution;
#[cfg(not(windows))]
impl ScopedTimerResolution {
    fn new(_period_ms: u32) -> Self {
        Self
    }
}

// ============================================================================
// Native Win32 `winmm::waveOut` 12-Slot Pinned Ring-Buffer Speaker Output
// ============================================================================

#[cfg(windows)]
mod waveout_native {
    use super::*;

    const WAVE_MAPPER: u32 = 0xFFFF_FFFF;
    const WAVE_FORMAT_PCM: u16 = 1;
    const CALLBACK_NULL: u32 = 0;
    const WHDR_DONE: u32 = 0x0000_0001;
    const WHDR_PREPARED: u32 = 0x0000_0002;
    const WHDR_INQUEUE: u32 = 0x0000_0010;

    #[repr(C, packed)]
    struct WaveFormatEx {
        w_format_tag: u16,
        n_channels: u16,
        n_samples_per_sec: u32,
        n_avg_bytes_per_sec: u32,
        n_block_align: u16,
        w_bits_per_sample: u16,
        cb_size: u16,
    }

    #[repr(C)]
    struct WaveHdr {
        lp_data: *mut u8,
        dw_buffer_length: u32,
        dw_bytes_recorded: u32,
        dw_user: usize,
        dw_flags: u32,
        dw_loops: u32,
        lp_next: *mut WaveHdr,
        reserved: usize,
    }

    #[link(name = "winmm")]
    extern "system" {
        fn waveOutOpen(
            phwo: *mut *mut c_void,
            u_device_id: u32,
            pwfx: *const WaveFormatEx,
            dw_callback: usize,
            dw_instance: usize,
            fdw_open: u32,
        ) -> u32;
        fn waveOutPrepareHeader(hwo: *mut c_void, pwh: *mut WaveHdr, cbwh: u32) -> u32;
        fn waveOutUnprepareHeader(hwo: *mut c_void, pwh: *mut WaveHdr, cbwh: u32) -> u32;
        fn waveOutWrite(hwo: *mut c_void, pwh: *mut WaveHdr, cbwh: u32) -> u32;
        fn waveOutReset(hwo: *mut c_void) -> u32;
        fn waveOutClose(hwo: *mut c_void) -> u32;
        fn waveOutSetVolume(hwo: *mut c_void, dw_volume: u32) -> u32;
    }

    struct WaveSlot {
        hdr: Box<WaveHdr>,
        buf: Vec<u8>,
        prepared: bool,
        in_flight: bool,
        in_flight_samples: usize,
    }

    impl WaveSlot {
        fn new() -> Self {
            Self {
                hdr: Box::new(WaveHdr {
                    lp_data: ptr::null_mut(),
                    dw_buffer_length: 0,
                    dw_bytes_recorded: 0,
                    dw_user: 0,
                    dw_flags: 0,
                    dw_loops: 0,
                    lp_next: ptr::null_mut(),
                    reserved: 0,
                }),
                buf: Vec::with_capacity(4096),
                prepared: false,
                in_flight: false,
                in_flight_samples: 0,
            }
        }
    }

    pub struct WaveOutPlayer {
        hwo: *mut c_void,
        sample_rate: u32,
        slots: Vec<WaveSlot>,
        queue: AirPlayAudioJitterQueue,
        last_muted: bool,
        virtual_null_sink: bool,
        virtual_in_flight_samples: usize,
        virtual_last_drain_at: Instant,
    }

    impl WaveOutPlayer {
        pub fn open(sample_rate: u32) -> Self {
            let rate = if sample_rate == 0 {
                AIRPLAY_AUDIO_RATE
            } else {
                sample_rate
            };
            let wfx = WaveFormatEx {
                w_format_tag: WAVE_FORMAT_PCM,
                n_channels: 2,
                n_samples_per_sec: rate,
                n_avg_bytes_per_sec: rate * 4,
                n_block_align: 4,
                w_bits_per_sample: 16,
                cb_size: 0,
            };
            let mut hwo: *mut c_void = ptr::null_mut();
            let rc = unsafe { waveOutOpen(&mut hwo, WAVE_MAPPER, &wfx, 0, 0, CALLBACK_NULL) };
            let muted = is_airplay_audio_muted();
            let now = Instant::now();
            if rc != 0 || hwo.is_null() {
                return Self {
                    hwo: ptr::null_mut(),
                    sample_rate: rate,
                    slots: Vec::new(),
                    queue: AirPlayAudioJitterQueue::new(rate),
                    last_muted: muted,
                    virtual_null_sink: true,
                    virtual_in_flight_samples: 0,
                    virtual_last_drain_at: now,
                };
            }
            unsafe {
                let _ = waveOutSetVolume(hwo, if muted { 0x0000_0000 } else { 0xFFFF_FFFF });
            }
            let slots = (0..WAVEOUT_SLOTS).map(|_| WaveSlot::new()).collect();
            Self {
                hwo,
                sample_rate: rate,
                slots,
                queue: AirPlayAudioJitterQueue::new(rate),
                last_muted: muted,
                virtual_null_sink: false,
                virtual_in_flight_samples: 0,
                virtual_last_drain_at: now,
            }
        }

        fn reclaim_done_slots(&mut self, now: Instant) {
            if self.virtual_null_sink || self.hwo.is_null() {
                let elapsed_ms = now.duration_since(self.virtual_last_drain_at).as_millis() as u64;
                if elapsed_ms > 0 {
                    let drained =
                        ((elapsed_ms * (self.sample_rate as u64) * 2) / 1000) as usize;
                    self.virtual_in_flight_samples =
                        self.virtual_in_flight_samples.saturating_sub(drained);
                    self.virtual_last_drain_at = now;
                }
                return;
            }
            let hdr_sz = std::mem::size_of::<WaveHdr>() as u32;
            for slot in &mut self.slots {
                if slot.in_flight {
                    let flags = unsafe { ptr::read_volatile(&slot.hdr.dw_flags) };
                    if (flags & WHDR_DONE) != 0 || (flags & WHDR_INQUEUE) == 0 {
                        if slot.prepared || (flags & WHDR_PREPARED) != 0 {
                            unsafe {
                                let _ = waveOutUnprepareHeader(self.hwo, &mut *slot.hdr, hdr_sz);
                            }
                            slot.prepared = false;
                        }
                        slot.in_flight = false;
                        slot.in_flight_samples = 0;
                    }
                }
            }
        }

        fn in_flight_samples(&self) -> usize {
            if self.virtual_null_sink || self.hwo.is_null() {
                self.virtual_in_flight_samples
            } else {
                self.slots
                    .iter()
                    .filter(|s| s.in_flight)
                    .map(|s| s.in_flight_samples)
                    .sum()
            }
        }

        fn free_slot_count(&self) -> usize {
            if self.virtual_null_sink || self.hwo.is_null() {
                WAVEOUT_SLOTS
            } else {
                self.slots.iter().filter(|s| !s.in_flight).count()
            }
        }

        pub fn sync_mute(&mut self) {
            let muted = is_airplay_audio_muted();
            if muted != self.last_muted {
                self.last_muted = muted;
                if !self.hwo.is_null() {
                    unsafe {
                        let _ =
                            waveOutSetVolume(self.hwo, if muted { 0x0000_0000 } else { 0xFFFF_FFFF });
                    }
                }
            }
        }

        fn submit_ready(&mut self, now: Instant) -> (u64, u64, u32, u32) {
            self.reclaim_done_slots(now);
            let in_flight = self.in_flight_samples();
            let free_slots = self.free_slot_count();
            let (blocks, dropped, queued_ms, target_ms) =
                self.queue.drain_ready_blocks(in_flight, free_slots, now);
            let mut submitted: u64 = 0;

            if self.virtual_null_sink || self.hwo.is_null() {
                for blk in blocks {
                    let n = blk.len();
                    self.virtual_in_flight_samples =
                        self.virtual_in_flight_samples.saturating_add(n);
                    submitted = submitted.saturating_add(n as u64);
                }
                let final_ms = stereo_samples_to_ms(
                    self.in_flight_samples().saturating_add(self.queue.pending_samples()),
                    self.sample_rate,
                );
                return (submitted, dropped, final_ms, target_ms);
            }

            let hdr_sz = std::mem::size_of::<WaveHdr>() as u32;
            for blk in blocks {
                let Some(slot_idx) = self.slots.iter().position(|s| !s.in_flight) else {
                    break;
                };
                let take = blk.len();
                let slot = &mut self.slots[slot_idx];
                slot.buf.clear();
                slot.buf.reserve(take * 2);
                for s in blk {
                    slot.buf.extend_from_slice(&s.to_le_bytes());
                }
                slot.hdr.lp_data = slot.buf.as_mut_ptr();
                slot.hdr.dw_buffer_length = slot.buf.len() as u32;
                slot.hdr.dw_bytes_recorded = 0;
                slot.hdr.dw_user = 0;
                slot.hdr.dw_flags = 0;
                slot.hdr.dw_loops = 0;
                slot.hdr.lp_next = ptr::null_mut();
                slot.hdr.reserved = 0;

                let p_rc = unsafe { waveOutPrepareHeader(self.hwo, &mut *slot.hdr, hdr_sz) };
                if p_rc != 0 {
                    break;
                }
                slot.prepared = true;
                let w_rc = unsafe { waveOutWrite(self.hwo, &mut *slot.hdr, hdr_sz) };
                if w_rc != 0 {
                    unsafe {
                        let _ = waveOutUnprepareHeader(self.hwo, &mut *slot.hdr, hdr_sz);
                    }
                    slot.prepared = false;
                    break;
                }
                slot.in_flight = true;
                slot.in_flight_samples = take;
                submitted = submitted.saturating_add(take as u64);
            }
            let final_ms = stereo_samples_to_ms(
                self.in_flight_samples().saturating_add(self.queue.pending_samples()),
                self.sample_rate,
            );
            (submitted, dropped, final_ms.max(queued_ms.min(final_ms)), target_ms)
        }

        /// Enqueues one decoded stereo PCM packet, applies Smooth Drift Trimming & Smooth Gain Ramp,
        /// and submits ready blocks directly to `winmm::waveOut` (`(played_samples, dropped_pkts, buf_ms, target_ms)`).
        pub fn push_samples(&mut self, samples: Vec<i16>) -> (u64, u64, u32, u32) {
            if samples.is_empty() {
                return self.tick();
            }
            self.sync_mute();
            let now = Instant::now();
            self.reclaim_done_slots(now);
            let in_flight = self.in_flight_samples();
            let (drop_push, _buf_ms, _target_ms) = self.queue.push_packet(samples, in_flight, now);
            let (submitted, drop_drain, final_buf_ms, target_ms) = self.submit_ready(now);
            (submitted, drop_push.saturating_add(drop_drain), final_buf_ms, target_ms)
        }

        /// Periodic tick (`<= 8 ms` wake under `timeBeginPeriod(1)`) to reclaim finished `WAVEHDR`
        /// slots, release prefill-timeout packets, and refresh live `airplayAudioBufferMs`.
        pub fn tick(&mut self) -> (u64, u64, u32, u32) {
            self.sync_mute();
            let now = Instant::now();
            self.submit_ready(now)
        }

        pub fn flush(&mut self) {
            self.queue.clear();
            self.virtual_in_flight_samples = 0;
            self.virtual_last_drain_at = Instant::now();
            if self.hwo.is_null() {
                return;
            }
            unsafe {
                let _ = waveOutReset(self.hwo);
            }
            self.reclaim_done_slots(Instant::now());
        }
    }

    impl Drop for WaveOutPlayer {
        fn drop(&mut self) {
            self.queue.clear();
            if self.hwo.is_null() {
                return;
            }
            let hdr_sz = std::mem::size_of::<WaveHdr>() as u32;
            unsafe {
                let _ = waveOutReset(self.hwo);
            }
            for _ in 0..3 {
                self.reclaim_done_slots(Instant::now());
                if self.slots.iter().all(|s| !s.in_flight) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(4));
            }
            for slot in &mut self.slots {
                if slot.prepared {
                    unsafe {
                        let _ = waveOutUnprepareHeader(self.hwo, &mut *slot.hdr, hdr_sz);
                    }
                    slot.prepared = false;
                    slot.in_flight = false;
                    slot.in_flight_samples = 0;
                }
            }
            unsafe {
                let _ = waveOutClose(self.hwo);
            }
            self.hwo = ptr::null_mut();
        }
    }
}

#[cfg(not(windows))]
mod waveout_native {
    use super::*;
    pub struct WaveOutPlayer {
        queue: AirPlayAudioJitterQueue,
    }
    impl WaveOutPlayer {
        pub fn open(sample_rate: u32) -> Self {
            Self {
                queue: AirPlayAudioJitterQueue::new(sample_rate),
            }
        }
        pub fn sync_mute(&mut self) {}
        pub fn push_samples(&mut self, samples: Vec<i16>) -> (u64, u64, u32, u32) {
            let now = Instant::now();
            let (d1, _, _) = self.queue.push_packet(samples, 0, now);
            let (blks, d2, bms, tms) = self.queue.drain_ready_blocks(0, WAVEOUT_SLOTS, now);
            let played: u64 = blks.iter().map(|b| b.len() as u64).sum();
            (played, d1.saturating_add(d2), bms, tms)
        }
        pub fn tick(&mut self) -> (u64, u64, u32, u32) {
            let now = Instant::now();
            let (blks, d, bms, tms) = self.queue.drain_ready_blocks(0, WAVEOUT_SLOTS, now);
            let played: u64 = blks.iter().map(|b| b.len() as u64).sum();
            (played, d, bms, tms)
        }
        pub fn flush(&mut self) {
            self.queue.clear();
        }
    }
}

use waveout_native::WaveOutPlayer;

// ============================================================================
// Dual-Socket UDP AirPlay Audio Worker (`SETUP type=96` data + control)
// ============================================================================

fn publish_audio_playback_progress(played: u64, dropped: u64, buf_ms: u32, target_ms: u32) {
    AIRPLAY_AUDIO_BUFFER_MS.store(buf_ms, Ordering::Relaxed);
    if dropped > 0 {
        AIRPLAY_AUDIO_DROPPED_PACKETS.fetch_add(dropped, Ordering::Relaxed);
        add_trace_audio_dropped_packets(dropped);
    }
    if played > 0 {
        add_trace_audio_played_samples(played);
    }
    if played > 0 || dropped > 0 {
        if let Ok(mut st) = audio_state().lock() {
            st.active = true;
            st.played_samples = st.played_samples.saturating_add(played);
            st.buffer_ms = buf_ms;
            st.target_ms = target_ms;
            st.dropped_packets = AIRPLAY_AUDIO_DROPPED_PACKETS.load(Ordering::Relaxed);
        }
    }
}

/// Spawns the native AirPlay `type=96` UDP audio worker (`dataPort` RTP + `controlPort` RTCP Sync).
/// Uses `timeBeginPeriod(1)`, `REORDER_DEPTH = 3`, CNG `AES-128-CBC`, offline multi-codec decoder,
/// Smooth Drift Trimming, zipper-free per-sample Smooth Gain Ramp, and live zero-drop hot-reload.
pub fn spawn_airplay_audio_worker(
    aes_key: [u8; 16],
    eiv: [u8; 16],
    ct: u64,
    sample_rate: u32,
    spf: usize,
    flush_epoch: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
) -> io::Result<(u16, u16, JoinHandle<()>)> {
    let data_sock = UdpSocket::bind(crate::connection::bind_addr_v4(0))?;
    let ctrl_sock = UdpSocket::bind(crate::connection::bind_addr_v4(0))?;
    let data_port = data_sock.local_addr()?.port();
    let ctrl_port = ctrl_sock.local_addr()?.port();
    let _ = disable_udp_connreset(&data_sock);
    let _ = disable_udp_connreset(&ctrl_sock);
    let _ = data_sock.set_nonblocking(true);
    let _ = ctrl_sock.set_nonblocking(true);

    let effective_ct = if matches!(ct, 1 | 2 | 4 | 8) { ct } else { 8 };
    let effective_sr = if sample_rate == 0 {
        AIRPLAY_AUDIO_RATE
    } else {
        sample_rate
    };
    let codec_name = codec_label_for_ct(effective_ct);
    let (target_ms, _) = current_airplay_audio_targets();
    AIRPLAY_AUDIO_BUFFER_MS.store(0, Ordering::Relaxed);
    AIRPLAY_AUDIO_DROPPED_PACKETS.store(0, Ordering::Relaxed);

    if let Ok(mut st) = audio_state().lock() {
        st.active = true;
        st.muted = is_airplay_audio_muted();
        st.codec = Some(codec_name.to_string());
        st.sample_rate = effective_sr;
        st.decoded_packets = 0;
        st.played_samples = 0;
        st.decode_errors = 0;
        st.volume = get_airplay_audio_volume();
        st.buffer_preset = get_airplay_audio_buffer_preset().to_string();
        st.lip_sync = is_airplay_audio_lipsync_enabled();
        st.buffer_ms = 0;
        st.target_ms = target_ms;
        st.dropped_packets = 0;
    }
    set_trace_audio_codec(codec_name);

    let handle = std::thread::Builder::new()
        .name("airplay-audio-play".into())
        .spawn(move || {
            // Tahap 259 Pilar 2: Acquire 1 ms Windows multimedia timer granularity for the
            // lifetime of this AirPlay audio worker (RAII Drop calls timeEndPeriod(1)).
            let _timer_res = ScopedTimerResolution::new(1);

            let cbc = Aes128CbcKey::new(&aes_key).ok();
            let mut decoder: Option<AirPlayAudioDecoder> = None;
            let mut decoder_init_tried = false;
            let mut player: Option<WaveOutPlayer> = None;
            let mut reorder = RtpReorderQueue::default();
            let mut last_seq: Option<u16> = None;
            let mut seen_flush = flush_epoch.load(Ordering::SeqCst);

            let mut buf = [0u8; 4096];
            let mut data_rx_count: u64 = 0;
            let mut ctrl_rx_count: u64 = 0;
            let mut decoded_count: u64 = 0;
            let mut decode_err_count: u64 = 0;

            while !stop.load(Ordering::Relaxed) {
                // Honor RTSP FLUSH immediately without closing UDP sockets or restarting worker
                let cur_flush = flush_epoch.load(Ordering::SeqCst);
                if cur_flush != seen_flush {
                    seen_flush = cur_flush;
                    reorder.clear();
                    last_seq = None;
                    if let Some(ref mut p) = player {
                        p.flush();
                    }
                    AIRPLAY_AUDIO_BUFFER_MS.store(0, Ordering::Relaxed);
                }
                if let Some(ref mut p) = player {
                    let (played, dropped, buf_ms, t_ms) = p.tick();
                    publish_audio_playback_progress(played, dropped, buf_ms, t_ms);
                }

                let mut got_any = false;
                loop {
                    match data_sock.recv(&mut buf) {
                        Ok(n) => {
                            got_any = true;
                            data_rx_count = data_rx_count.saturating_add(1);
                            inc_trace_audio_data_rx();
                            if matches!(data_rx_count, 1 | 10 | 100) {
                                record_airplay_trace(
                                    "audio_rx",
                                    format!("pkt=#{data_rx_count} len={n} ch=data"),
                                );
                            }
                            if n <= 12 {
                                continue;
                            }
                            let seq = u16::from_be_bytes([buf[2], buf[3]]);
                            for pkt in reorder.push(seq, buf[..n].to_vec()) {
                                let pseq = u16::from_be_bytes([pkt[2], pkt[3]]);
                                if last_seq.replace(pseq) == Some(pseq) {
                                    continue;
                                }
                                let mut payload = pkt[12..].to_vec();
                                if let Some(ref c) = cbc {
                                    if c.decrypt_in_place(&eiv, &mut payload).is_err() {
                                        decode_err_count = decode_err_count.saturating_add(1);
                                        inc_trace_audio_decode_error();
                                        if let Ok(mut st) = audio_state().lock() {
                                            st.decode_errors = decode_err_count;
                                        }
                                        continue;
                                    }
                                }
                                if !decoder_init_tried {
                                    decoder_init_tried = true;
                                    match AirPlayAudioDecoder::open(effective_ct, effective_sr, spf) {
                                        Ok(d) => {
                                            record_airplay_trace(
                                                "audio_open",
                                                format!(
                                                    "codec={codec_name} ct={effective_ct} sr={effective_sr}"
                                                ),
                                            );
                                            decoder = Some(d);
                                        }
                                        Err(e) => {
                                            record_airplay_trace(
                                                "audio_open_err",
                                                format!("ct={effective_ct} err={e}"),
                                            );
                                        }
                                    }
                                }
                                let Some(ref mut dec) = decoder else {
                                    continue;
                                };
                                match dec.decode_payload(&payload) {
                                    Ok(pcm) if !pcm.is_empty() => {
                                        let pcm_len = pcm.len();
                                        decoded_count = decoded_count.saturating_add(1);
                                        inc_trace_audio_decoded();
                                        let p = player
                                            .get_or_insert_with(|| WaveOutPlayer::open(AIRPLAY_AUDIO_RATE));
                                        let (played, dropped, buf_ms, t_ms) = p.push_samples(pcm);
                                        publish_audio_playback_progress(played, dropped, buf_ms, t_ms);
                                        if let Ok(mut st) = audio_state().lock() {
                                            st.active = true;
                                            st.decoded_packets = decoded_count;
                                            st.buffer_ms = buf_ms;
                                            st.target_ms = t_ms;
                                            st.dropped_packets =
                                                AIRPLAY_AUDIO_DROPPED_PACKETS.load(Ordering::Relaxed);
                                        }
                                        if matches!(decoded_count, 1 | 50 | 500) {
                                            record_airplay_trace(
                                                "audio_decoded",
                                                format!(
                                                    "pkt=#{decoded_count} codec={codec_name} samples={pcm_len}"
                                                ),
                                            );
                                        }
                                    }
                                    Ok(_) => {}
                                    Err(_) => {
                                        decode_err_count = decode_err_count.saturating_add(1);
                                        inc_trace_audio_decode_error();
                                        if let Ok(mut st) = audio_state().lock() {
                                            st.decode_errors = decode_err_count;
                                        }
                                        if matches!(decode_err_count, 1 | 20 | 200) {
                                            record_airplay_trace(
                                                "audio_decode_err",
                                                format!("errs={decode_err_count} codec={codec_name}"),
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        Err(e)
                            if e.kind() == io::ErrorKind::WouldBlock
                                || e.kind() == io::ErrorKind::Interrupted
                                || e.kind() == io::ErrorKind::ConnectionReset =>
                        {
                            break;
                        }
                        Err(_) => break,
                    }
                }
                loop {
                    match ctrl_sock.recv(&mut buf) {
                        Ok(n) => {
                            got_any = true;
                            ctrl_rx_count = ctrl_rx_count.saturating_add(1);
                            inc_trace_audio_ctrl_rx();
                            if matches!(ctrl_rx_count, 1 | 10 | 100) {
                                let pt = if n >= 2 { buf[1] } else { 0 };
                                record_airplay_trace(
                                    "audio_ctrl_rx",
                                    format!("pkt=#{ctrl_rx_count} pt=0x{pt:02X} len={n}"),
                                );
                            }
                        }
                        Err(e)
                            if e.kind() == io::ErrorKind::WouldBlock
                                || e.kind() == io::ErrorKind::Interrupted
                                || e.kind() == io::ErrorKind::ConnectionReset =>
                        {
                            break;
                        }
                        Err(_) => break,
                    }
                }
                if !got_any {
                    let _ = wait_two_udps_readable(&data_sock, &ctrl_sock, 8);
                }
            }
            drop(player);
            mark_airplay_audio_inactive();
            record_airplay_trace("audio_stop", format!("codec={codec_name} decoded={decoded_count}"));
        })?;
    Ok((data_port, ctrl_port, handle))
}
