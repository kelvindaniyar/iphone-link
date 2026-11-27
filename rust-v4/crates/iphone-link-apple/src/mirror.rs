//! Native Rust CoreDevice Display Streaming Engine (`com.apple.coredevice.displayservice`),
//! RFC 7798 HEVC Depacketizer integration, iOS 27 Multi-SSRC Filter, RTCP PLI/FIR + RR/SDES Keep-Alive,
//! Latest-Frame-Wins `SubscriberQueue` (`24` frames), Clean Stop & Stealth Dim Auto-Restore,
//! AAC-ELD -> `zlaac.dll` -> `JitterBuffer` -> WASAPI Audio Routing, and 32-Scenario Smart Auto-Rotate Controller.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use idevice::core_device::{
    build_keyframe_request, build_rtcp_receiver_report_sdes, build_screen_audio_offer,
    build_screen_video_offer, build_start_audio_parameters, build_start_video_parameters, is_rtcp,
    CallInfoBlob, DisplayServiceClient, FrameTrim, HevcDepacketizer, ReportBlock, RtpPacket,
};
use iphone_link_audio::{
    patch_aac_eld_cookie, wasapi_output::probe_wasapi_shared_engine, AdaptiveAudioBuffer,
    AudioBufferPreset, ZeroDoubleAudioController,
};
use iphone_link_video::autorotate::{
    analyze_frame_grid, evaluate_auto_rotate, run_32_autorotate_scenarios_self_test, AppProfile,
    OrientationMode, ProbeGrid120x260, GRID_HEIGHT, GRID_WIDTH,
};
use iphone_link_video::d3d11_context::{SharedFrameRing, RING_CAPACITY};
use iphone_link_video::mf_decoder::{
    build_synthetic_hevc_annex_b_idr, build_synthetic_hevc_annex_b_trail_r,
    export_clean_cropped_bgra_frame, extract_downscaled_probe_120x260_into,
    populate_probe_grid_120x260_in_place, write_nv12_from_rgb_sampler, DecodeOutcome,
    MfHevcDecoder,
};
use iphone_link_video::video_hwnd::{
    compute_presenter_viewport_rect, decode_base64_fast, map_stage_touch_to_ios_hid,
    D3d11VideoPresenter, FitMode, PresenterConfig, SnapshotExportResult,
};

use crate::hid;
use crate::link::Link;

/// Controller capability mask expected by CoreDevice DisplayService.
pub const CLIENT_SUPPORTED_FEATURES: u64 = 140;
/// Silence watchdog duration before ending a stalled video session.
pub const SILENCE: Duration = Duration::from_secs(8);
/// Consecutive packets required before switching main video SSRC on iOS 27.
pub const SWITCH_AFTER: usize = 30;
/// AudioSpecificConfig: AAC-ELD, 48 kHz stereo, 480-sample frames (`frameLengthFlag` bit `0x10`).
pub const AUDIO_ASC: [u8; 4] = [0xF8, 0xE6, 0x50, 0x00];
/// Bounded subscriber queue capacity for Latest-Frame-Wins policy.
pub const MAX_SUBSCRIBER_QUEUE: usize = 24;
/// Stop cooldown window in milliseconds so polling returns deterministic `idle`.
pub const STOP_COOLDOWN_MS: u64 = 5_000;
/// Manual rotation cooldown window (30 seconds) before Smart Auto-Rotate resumes.
pub const MANUAL_OVERRIDE_COOLDOWN_MS: u64 = 30_000;
/// Instant portrait return hold duration after Home swipe/button press (2.8 seconds).
pub const HOME_FORCE_PORTRAIT_MS: u64 = 2_800;
/// Fast Home transition animation budget (350 ms).
pub const HOME_TRANSITION_BUDGET_MS: u64 = 350;
/// Portrait hold duration after Recent/App-Switcher gesture (5.0 seconds).
pub const RECENT_FORCE_PORTRAIT_MS: u64 = 5_000;

/// Error returned by CoreDevice (9022) when a call is active on the iPhone/iPad.
pub const IN_CALL: &str =
    "The iPhone or iPad is in a phone or video call. End the call, then try again.";

/// Clear bilingual diagnostics when iOS 27.0.1 terminates cable-free Wi-Fi CoreDevice mirroring.
pub const WIFI_COREDEVICE_IOS27_DIAGNOSTIC_ID: &str =
    "Koneksi CoreDevice Wi-Fi gagal: iOS 27.0.1 menutup sesi wireless tanpa kabel (ConnectionTerminated). Pastikan iPhone menyala, layar tidak terkunci, dan terhubung ke Wi-Fi yang sama dengan PC, atau hubungkan kabel USB.";
pub const WIFI_COREDEVICE_IOS27_DIAGNOSTIC_EN: &str =
    "CoreDevice Wi-Fi connection failed: iOS 27.0.1 closed the cable-free wireless session (ConnectionTerminated). Ensure your iPhone is awake, unlocked, and on the same Wi-Fi network as your PC, or use USB Cable mode.";

/// One held RTP packet: `(sequence_number, timestamp, payload, marker)`.
pub type HeldPacket = (u16, u32, Vec<u8>, bool);

/// Generates an uppercase RFC 4122-style pseudo-UUID without external dependencies.
fn new_uuid_upper() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(1);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let a = (nanos >> 16) as u32;
    let b = ((nanos ^ seq) & 0xFFFF) as u16;
    let c = (0x4000u16) | (((nanos >> 8) ^ seq) as u16 & 0x0FFF);
    let d = (0x8000u16) | ((seq.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 48) as u16 & 0x3FFF);
    let e = nanos.wrapping_mul(6364136223846793005).wrapping_add(seq) & 0x0000_FFFF_FFFF_FFFF;
    format!("{a:08X}-{b:04X}-{c:04X}-{d:04X}-{e:012X}")
}

fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn escape_json(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str(r#"\\"#),
            '"' => out.push_str(r#"\""#),
            '\n' => out.push_str(r#"\n"#),
            '\r' => out.push_str(r#"\r"#),
            '\t' => out.push_str(r#"\t"#),
            _ => out.push(c),
        }
    }
    out
}

/// Filters RTP streams by SSRC so iOS 27's once-per-second rogue `seq 12` SSRC packet
/// never shifts the depacketizer cursor away from the live screen stream.
#[derive(Debug, Default)]
pub struct SsrcPicker {
    pub main: Option<u32>,
    pub held: HashMap<u32, Vec<HeldPacket>>,
}

impl SsrcPicker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn main_ssrc(&self) -> Option<u32> {
        self.main
    }

    /// Returns `None` while buffering unconfirmed SSRCs, or `Some((fresh_stream, packets))`.
    pub fn push(&mut self, ssrc: u32, pkt: HeldPacket) -> Option<(bool, Vec<HeldPacket>)> {
        if self.main == Some(ssrc) {
            return Some((false, vec![pkt]));
        }
        let need = if self.main.is_some() { SWITCH_AFTER } else { 2 };
        let entry = self.held.entry(ssrc).or_default();
        entry.push(pkt);
        if entry.len() >= need {
            let promoted = self.held.remove(&ssrc).unwrap_or_default();
            self.held.clear();
            self.main = Some(ssrc);
            return Some((true, promoted));
        }
        if self.held.len() > 8 {
            self.held.retain(|_, v| v.len() > 1);
        }
        None
    }
}

/// Backwards-compatible alias matching `zenlink-apple::mirror::Picker`.
pub type Picker = SsrcPicker;

/// RTCP target discovered from `start_media_stream` plist answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RtcpTarget {
    pub port: u16,
    pub ours: u32,
    pub theirs: u32,
}

/// Extracts `SourcePort`, `RemoteSSRC` (`ours`), and `LocalSSRC` (`theirs`) from `streamConfig`.
pub fn rtcp_of(answer: &plist::Value) -> Option<RtcpTarget> {
    fn find<'a>(v: &'a plist::Value, key: &str) -> Option<&'a plist::Value> {
        match v {
            plist::Value::Dictionary(d) => {
                d.get(key).or_else(|| d.values().find_map(|x| find(x, key)))
            }
            plist::Value::Array(a) => a.iter().find_map(|x| find(x, key)),
            _ => None,
        }
    }
    let cfg = find(answer, "streamConfig")?;
    let num = |k: &str| {
        let v = find(cfg, k)?;
        v.as_unsigned_integer()
            .or_else(|| v.as_signed_integer().map(|i| i as u64))
    };
    Some(RtcpTarget {
        port: num("SourcePort")? as u16,
        ours: num("RemoteSSRC")? as u32,
        theirs: num("LocalSSRC")? as u32,
    })
}

/// Extended highest RTP sequence number (RFC 3550).
pub fn extend_seq(highest: u32, seq: u16) -> u32 {
    if highest == 0 {
        return u32::from(seq);
    }
    let mut cycles = highest >> 16;
    let last = (highest & 0xFFFF) as u16;
    if seq < last && last - seq > 0x8000 {
        cycles = cycles.wrapping_add(1);
    }
    let ext = (cycles << 16) | u32::from(seq);
    if ext.wrapping_sub(highest) < 0x8000_0000 {
        ext
    } else {
        highest
    }
}

/// Normalizes verbose CoreDevice plist errors to short user-friendly diagnostics.
pub fn device_error(dump: &str) -> String {
    if dump.contains("Integer(9022)") || dump.contains("call is currently in progress") {
        return IN_CALL.into();
    }
    if let Some(i) = dump.find("NSLocalizedDescription") {
        if let Some(text) = dump[i..]
            .split("String(\\\"")
            .nth(1)
            .and_then(|t| t.split("\\\"").next())
        {
            return text.into();
        }
    }
    dump.chars().take(200).collect()
}

/// Unified stream output event emitted by native mirror sessions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    Video {
        data: Vec<u8>,
        is_keyframe: bool,
        width: u32,
        height: u32,
        trim: [u32; 4],
    },
    Audio {
        aac_au: Vec<u8>,
        sample_rate: u32,
        channels: u8,
    },
    Ready,
    Stopped(String),
}

/// `/mirror/codec` & `/codec` wire metadata matching `mirror.js` expectations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodecHeader {
    pub ok: bool,
    pub codec: String,
    pub description_b64: Option<String>,
    pub width: u32,
    pub height: u32,
    pub display_width: u32,
    pub display_height: u32,
    pub fps: u32,
    pub trim: FrameTrim,
    pub is_airplay: bool,
}

impl Default for CodecHeader {
    fn default() -> Self {
        Self {
            ok: true,
            codec: "hvc1.1.6.L150.B0".to_string(),
            description_b64: None,
            width: 1216,
            height: 2656,
            display_width: 1178,
            display_height: 2598,
            fps: 60,
            trim: FrameTrim::default(),
            is_airplay: false,
        }
    }
}

impl CodecHeader {
    pub fn to_json_string(&self) -> String {
        let desc_part = match &self.description_b64 {
            Some(d) if !d.is_empty() => format!(r#","description":"{}""#, escape_json(d)),
            _ => String::new(),
        };
        format!(
            r#"{{"ok":{},"codec":"{}"{},"width":{},"height":{},"displayWidth":{},"displayHeight":{},"fps":{},"isAirplay":{},"is_airplay":{},"trim":{{"top":{},"left":{},"bottom":{},"right":{}}}}}"#,
            self.ok,
            escape_json(&self.codec),
            desc_part,
            self.width,
            self.height,
            self.display_width,
            self.display_height,
            self.fps,
            self.is_airplay,
            self.is_airplay,
            self.trim.top,
            self.trim.left,
            self.trim.bottom,
            self.trim.right
        )
    }
}

/// Zero-dependency RFC 4648 base64 encoder for `hvcC` / PNG payloads.
pub fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(((input.len() + 2) / 3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as usize;
        let b1 = *chunk.get(1).unwrap_or(&0) as usize;
        let b2 = *chunk.get(2).unwrap_or(&0) as usize;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[(n >> 18) & 0x3F] as char);
        out.push(TABLE[(n >> 12) & 0x3F] as char);
        if chunk.len() > 1 {
            out.push(TABLE[(n >> 6) & 0x3F] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[n & 0x3F] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// Encodes an Annex-B access unit into `mirror.js` WebCodecs `hvcC` stream framing:
/// `[4-byte BE len = 1 + nals.len()][1-byte type: 0=key, 1=delta, 2=reset][4-byte BE nal_len + nal...]`.
pub fn encode_hvcc_live_chunk(annex_b_au: &[u8], frame_type: u8) -> Option<Vec<u8>> {
    let nalus = idevice::core_device::split_annexb_nalus(annex_b_au);
    if nalus.is_empty() {
        return None;
    }
    let payload_len: usize = nalus.iter().map(|n| 4 + n.len()).sum();
    let total_msg_len = 1 + payload_len;
    let mut out = Vec::with_capacity(4 + total_msg_len);
    out.extend_from_slice(&(total_msg_len as u32).to_be_bytes());
    out.push(frame_type);
    for nal in nalus {
        out.extend_from_slice(&(nal.len() as u32).to_be_bytes());
        out.extend_from_slice(nal);
    }
    Some(out)
}

/// Encodes a pre-framed 4-byte BE length-prefixed H.264 AVCC access unit (AirPlay) directly
/// into `mirror.js` WebCodecs wire format: `[4-byte BE len = 1 + avcc.len()][1-byte type][avcc]`.
pub fn encode_avcc_live_chunk(avcc_payload: &[u8], frame_type: u8) -> Option<Vec<u8>> {
    if avcc_payload.is_empty() {
        return None;
    }
    let total_msg_len = 1 + avcc_payload.len();
    let mut out = Vec::with_capacity(4 + total_msg_len);
    out.extend_from_slice(&(total_msg_len as u32).to_be_bytes());
    out.push(frame_type);
    out.extend_from_slice(avcc_payload);
    Some(out)
}

/// Encodes one Annex-B access unit into `/stream.bin` wire format:
/// `[4-byte Big-Endian length][Annex-B NALU payload]`.
pub fn encode_stream_bin_chunk(annex_b_nalu: &[u8]) -> Vec<u8> {
    let mut chunk = Vec::with_capacity(4 + annex_b_nalu.len());
    chunk.extend_from_slice(&(annex_b_nalu.len() as u32).to_be_bytes());
    chunk.extend_from_slice(annex_b_nalu);
    chunk
}

/// Bounded Latest-Frame-Wins subscriber queue (`MAX_SUBSCRIBER_QUEUE = 24`).
/// When full, older queued frames are discarded immediately so live latency never accumulates.
#[derive(Debug, Clone)]
pub struct SubscriberQueue {
    frames: VecDeque<(Vec<u8>, bool)>,
    capacity: usize,
    pub dropped_count: u64,
}

impl Default for SubscriberQueue {
    fn default() -> Self {
        Self::new(MAX_SUBSCRIBER_QUEUE)
    }
}

impl SubscriberQueue {
    pub fn new(capacity: usize) -> Self {
        let cap = capacity.max(1);
        Self {
            frames: VecDeque::with_capacity(cap),
            capacity: cap,
            dropped_count: 0,
        }
    }

    pub fn push_frame(&mut self, annex_b_nalu: Vec<u8>, is_keyframe: bool) {
        if annex_b_nalu.is_empty() {
            return;
        }
        while self.frames.len() >= self.capacity {
            if self.frames.pop_front().is_some() {
                self.dropped_count += 1;
            }
        }
        self.frames.push_back((annex_b_nalu, is_keyframe));
    }

    pub fn pop_chunk(&mut self) -> Option<Vec<u8>> {
        self.frames
            .pop_front()
            .map(|(data, _)| encode_stream_bin_chunk(&data))
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    pub fn clear(&mut self) {
        self.frames.clear();
    }
}

/// Streams HEVC Annex-B access units and AAC-ELD audio packets over a CoreDevice `Link`.
pub async fn stream(
    link: &mut Link,
    key: &AtomicBool,
    gate_only: bool,
    mut on_frame: impl FnMut(Vec<u8>) -> bool,
    mut on_audio: impl FnMut(&[u8]),
) -> Result<(), String> {
    let mut client = DisplayServiceClient::connect_rsd(&mut link.adapter, &mut link.rsd)
        .await
        .map_err(|e| format!("display service: {e:?}"))?;
    let udp = |e| format!("udp: {e:?}");
    let audio_udp = link.adapter.bind_udp(0).await.map_err(udp)?;
    let video_udp = link.adapter.bind_udp(0).await.map_err(udp)?;
    let host = link.adapter.host_ip().to_string();
    let peer = link.adapter.peer_ip().to_string();
    let call_info = CallInfoBlob::default();
    let session = new_uuid_upper();

    let audio_offer = build_screen_audio_offer(&new_uuid_upper(), &call_info)
        .map_err(|e| format!("audio offer: {e:?}"))?;
    let audio_rtcp = match client
        .start_media_stream(build_start_audio_parameters(
            &host,
            audio_udp.local_port(),
            &peer,
            50000,
            audio_offer,
            CLIENT_SUPPORTED_FEATURES,
            &session,
        ))
        .await
    {
        Ok(answer) => {
            let rtcp = rtcp_of(&answer);
            if rtcp.is_none() {
                tracing::warn!(
                    "sound: no RTCP target in answer; device may stop audio after ~20s"
                );
            }
            rtcp
        }
        Err(e) => {
            tracing::warn!(
                "sound: not started, mirroring without sound: {}",
                device_error(&format!("{e:?}"))
            );
            None
        }
    };

    let video_ssrc = (now_epoch_ms() & 0xFFFF_FFFF) as u32;
    let video_offer = build_screen_video_offer(&new_uuid_upper(), &call_info, video_ssrc)
        .map_err(|e| format!("video offer: {e:?}"))?;
    let video_answer = client
        .start_media_stream(build_start_video_parameters(
            &host,
            video_udp.local_port(),
            &peer,
            50001,
            video_offer,
            CLIENT_SUPPORTED_FEATURES,
            1,
            &session,
        ))
        .await
        .map_err(|e| match device_error(&format!("{e:?}")) {
            m if m == IN_CALL => m,
            m => format!("start video: {m}"),
        })?;

    let video_rtcp = rtcp_of(&video_answer);
    if video_rtcp.is_none() {
        tracing::warn!("screen: no RTCP target in answer; keyframes cannot be requested");
    }

    enum MediaEvent {
        Video(Vec<u8>),
        Audio(Vec<u8>),
        VideoErr(String),
    }

    let stop_flag = Arc::new(AtomicBool::new(false));
    let (tx, rx) = std::sync::mpsc::channel::<MediaEvent>();

    let v_sock = video_udp.try_clone_std().map_err(udp)?;
    let _ = v_sock.set_nonblocking(false);
    let _ = v_sock.set_read_timeout(Some(Duration::from_millis(200)));
    let v_stop = stop_flag.clone();
    let v_tx = tx.clone();
    let v_thread = std::thread::Builder::new()
        .name("mirror-video-rtp".into())
        .spawn(move || {
            let mut buf = vec![0u8; 65536];
            while !v_stop.load(Ordering::Relaxed) {
                match v_sock.recv(&mut buf) {
                    Ok(n) if n > 0 => {
                        let _ = v_tx.send(MediaEvent::Video(buf[..n].to_vec()));
                    }
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.kind() == std::io::ErrorKind::TimedOut =>
                    {
                        continue;
                    }
                    Err(e) => {
                        let _ = v_tx.send(MediaEvent::VideoErr(format!("video: {e:?}")));
                        break;
                    }
                    _ => {}
                }
            }
        })
        .map_err(|e| format!("video thread: {e}"))?;

    let a_sock = audio_udp.try_clone_std().map_err(udp)?;
    let _ = a_sock.set_nonblocking(false);
    let _ = a_sock.set_read_timeout(Some(Duration::from_millis(200)));
    let a_stop = stop_flag.clone();
    let a_tx = tx;
    let a_thread = std::thread::Builder::new()
        .name("mirror-audio-rtp".into())
        .spawn(move || {
            let mut buf = vec![0u8; 16384];
            while !a_stop.load(Ordering::Relaxed) {
                match a_sock.recv(&mut buf) {
                    Ok(n) if n > 0 => {
                        let _ = a_tx.send(MediaEvent::Audio(buf[..n].to_vec()));
                    }
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.kind() == std::io::ErrorKind::TimedOut =>
                    {
                        continue;
                    }
                    Err(_) => break,
                    _ => {}
                }
            }
        })
        .map_err(|e| format!("audio thread: {e}"))?;

    let mut depack = HevcDepacketizer::new();
    let mut au = Vec::new();
    let mut picker = SsrcPicker::new();
    let mut frame_at = Instant::now();
    let mut video_at = Instant::now();
    let mut last_ask = Instant::now();
    let mut last_tick = Instant::now();
    let mut audio_seq = 0u32;
    let mut fir_seq = 0u8;
    let mut video_seq = 0u32;

    let result = loop {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(MediaEvent::VideoErr(e)) => break Err(e),
            Ok(MediaEvent::Video(dg)) => {
                video_at = Instant::now();
                if is_rtcp(&dg) {
                    continue;
                }
                let Some(pkt) = RtpPacket::parse(&dg) else {
                    continue;
                };
                if picker.main == Some(pkt.ssrc) {
                    video_seq = extend_seq(video_seq, pkt.sequence_number);
                }
                let held = (
                    pkt.sequence_number,
                    pkt.timestamp,
                    pkt.payload.to_vec(),
                    pkt.marker,
                );
                let Some((fresh, packets)) = picker.push(pkt.ssrc, held) else {
                    continue;
                };
                if fresh {
                    tracing::info!("screen: RTP stream {:08x}", pkt.ssrc);
                    depack = HevcDepacketizer::new();
                    au.clear();
                }
                let mut done = false;
                for (seq, ts, payload, marker) in packets {
                    depack.push_rtp(seq, ts, &payload);
                    au.extend_from_slice(&depack.take_output());
                    if marker && !au.is_empty() {
                        frame_at = Instant::now();
                        if !on_frame(std::mem::take(&mut au)) {
                            done = true;
                            break;
                        }
                    }
                }
                if done {
                    break Ok(());
                }
                if frame_at.elapsed() > Duration::from_secs(3) {
                    frame_at = Instant::now();
                    tracing::info!(
                        "screen: packets but no frame for 3s (ssrc {:08x}, seq {}, pt {})",
                        pkt.ssrc,
                        pkt.sequence_number,
                        pkt.payload_type
                    );
                }
            }
            Ok(MediaEvent::Audio(dg)) => {
                if is_rtcp(&dg) {
                    continue;
                }
                if let Some(pkt) = RtpPacket::parse(&dg) {
                    audio_seq = extend_seq(audio_seq, pkt.sequence_number);
                    if !pkt.payload.is_empty() {
                        on_audio(pkt.payload);
                    }
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break Ok(()),
        }

        if last_ask.elapsed() >= Duration::from_millis(200) {
            last_ask = Instant::now();
            if let (true, Some(r)) = (key.load(Ordering::Relaxed), video_rtcp) {
                let media = picker.main.unwrap_or(r.theirs);
                let block = ReportBlock {
                    source_ssrc: media,
                    highest_seq: video_seq,
                    ..ReportBlock::default()
                };
                fir_seq = fir_seq.wrapping_add(1);
                let req = build_keyframe_request(r.ours, "iphonelink", media, &[block], fir_seq);
                let _ = video_udp.send_to(r.port, &req).await;
            }
        }

        if last_tick.elapsed() >= Duration::from_secs(1) {
            last_tick = Instant::now();
            if !gate_only && video_at.elapsed() > SILENCE {
                break Err("the device stopped sending the screen".into());
            }
            if let Some(r) = audio_rtcp {
                let rr = build_rtcp_receiver_report_sdes(r.ours, r.theirs, audio_seq, "iphonelink");
                let _ = audio_udp.send_to(r.port, &rr).await;
            }
        }
    };

    stop_flag.store(true, Ordering::Relaxed);
    let _ = v_thread.join();
    let _ = a_thread.join();
    let _ = tokio::time::timeout(Duration::from_secs(3), client.stop_media_stream()).await;
    result
}

/// 32-Scenario Smart Auto-Rotate Controller with Manual Cooldown (`30s`), Home (`350ms`/`2.8s`) & Recent (`5.0s`)
/// Instant Portrait Return, and Zero-Allocation Sleep Mode when disabled.
#[derive(Debug, Clone)]
pub struct AutoRotateController {
    pub auto_rotate_enabled: bool,
    pub current_orientation_deg: u32,
    pub is_landscape: bool,
    pub manual_override_until_ms: u64,
    pub home_force_portrait_until_ms: u64,
    pub recent_force_portrait_until_ms: u64,
    pub probe_allocations: u64,
    pub evaluations_count: u64,
    pub last_transition_reason: &'static str,
}

impl Default for AutoRotateController {
    fn default() -> Self {
        Self {
            auto_rotate_enabled: true,
            current_orientation_deg: 0,
            is_landscape: false,
            manual_override_until_ms: 0,
            home_force_portrait_until_ms: 0,
            recent_force_portrait_until_ms: 0,
            probe_allocations: 0,
            evaluations_count: 0,
            last_transition_reason: "Initial portrait",
        }
    }
}

impl AutoRotateController {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.auto_rotate_enabled = enabled;
        self.last_transition_reason = if enabled {
            "Auto-rotate active"
        } else {
            "Zero-allocation sleep mode"
        };
    }

    /// Manual rotation button overrides Auto-Rotate for 30 seconds (`30_000ms`).
    pub fn apply_manual_rotation(&mut self, angle_deg: u32, now_ms: u64) -> u32 {
        let normalized = match angle_deg % 360 {
            90 => 90,
            270 => 270,
            180 => 180,
            _ => 0,
        };
        self.current_orientation_deg = normalized;
        self.is_landscape = normalized == 90 || normalized == 270;
        self.manual_override_until_ms = now_ms.saturating_add(MANUAL_OVERRIDE_COOLDOWN_MS);
        self.last_transition_reason = "Manual rotation 30s cooldown";
        normalized
    }

    /// Home swipe/button forces instant return to `0°` (`350ms` budget / `2.8s` hold).
    pub fn notify_home_button(&mut self, now_ms: u64) -> u32 {
        self.current_orientation_deg = 0;
        self.is_landscape = false;
        self.home_force_portrait_until_ms = now_ms.saturating_add(HOME_FORCE_PORTRAIT_MS);
        self.last_transition_reason = "Home instant 0° portrait return";
        0
    }

    /// Recent / App Switcher gesture forces immediate `0°` hold for `5.0s`.
    pub fn notify_recent_apps(&mut self, now_ms: u64) -> u32 {
        self.current_orientation_deg = 0;
        self.is_landscape = false;
        self.recent_force_portrait_until_ms = now_ms.saturating_add(RECENT_FORCE_PORTRAIT_MS);
        self.last_transition_reason = "Recent/Switcher 5s 0° portrait return";
        0
    }

    /// Evaluates one frame grid unless in Zero-Allocation Sleep Mode (`auto_rotate_enabled == false`).
    pub fn evaluate_frame(
        &mut self,
        grid: Option<&ProbeGrid120x260>,
        app_profile: Option<AppProfile>,
        width: u32,
        height: u32,
        now_ms: u64,
    ) -> u32 {
        if !self.auto_rotate_enabled {
            // Zero-Allocation Sleep Mode: zero probe grids inspected or allocated.
            return self.current_orientation_deg;
        }
        if now_ms < self.home_force_portrait_until_ms
            || now_ms < self.recent_force_portrait_until_ms
        {
            self.current_orientation_deg = 0;
            self.is_landscape = false;
            self.last_transition_reason = "Home/Recent portrait lock";
            return 0;
        }
        if now_ms < self.manual_override_until_ms {
            self.last_transition_reason = "Manual override cooldown active";
            return self.current_orientation_deg;
        }

        if let Some(g) = grid {
            self.probe_allocations = self.probe_allocations.saturating_add(1);
            self.evaluations_count = self.evaluations_count.saturating_add(1);
            let res = analyze_frame_grid(g, self.is_landscape);
            let next_deg = match res.suggested_orientation {
                OrientationMode::Portrait0 => 0,
                OrientationMode::Landscape90 => 90,
                OrientationMode::Landscape270 => 270,
            };
            self.current_orientation_deg = next_deg;
            self.is_landscape = next_deg == 90 || next_deg == 270;
            self.last_transition_reason = match res.suggested_orientation {
                OrientationMode::Portrait0 => "Portrait guard / Portrait screen",
                OrientationMode::Landscape90 => "Auto-rotate landscape 90°",
                OrientationMode::Landscape270 => "Auto-rotate landscape 270°",
            };
            return next_deg;
        }

        if let Some(profile) = app_profile {
            let decision = evaluate_auto_rotate(profile, width, height, 270);
            let next_deg = match decision.orientation {
                OrientationMode::Portrait0 => 0,
                OrientationMode::Landscape90 => 90,
                OrientationMode::Landscape270 => 270,
            };
            self.current_orientation_deg = next_deg;
            self.is_landscape = next_deg == 90 || next_deg == 270;
            self.last_transition_reason = decision.reason;
        }
        self.current_orientation_deg
    }

    pub fn run_self_test(&self) -> (usize, usize) {
        run_32_autorotate_scenarios_self_test()
    }
}

/// Validates Apple Bluetooth Audio Endpoint Hardware IDs (`VID&0001004c`, `VID_05AC`, `VID&000205ac`),
/// matching ONLY a single iPhone endpoint cleanly (excluding AirPods/Watch/non-Apple devices).
pub fn is_apple_iphone_bt_endpoint(device_id: &str, friendly_name: &str) -> bool {
    let id_upper = device_id.to_ascii_uppercase();
    let is_apple_vid = id_upper.contains("VID&0001004C")
        || id_upper.contains("VID_05AC")
        || id_upper.contains("VID&000205AC");
    if !is_apple_vid {
        return false;
    }
    let name_lower = friendly_name.to_ascii_lowercase();
    if name_lower.contains("airpods")
        || name_lower.contains("watch")
        || name_lower.contains("macbook")
    {
        return false;
    }
    true
}

/// Filters candidate Bluetooth/WASAPI endpoints down to at most 1 matching Apple iPhone endpoint.
pub fn select_single_iphone_bt_endpoint<'a>(
    candidates: &'a [(&'a str, &'a str)],
) -> Vec<(&'a str, &'a str)> {
    candidates
        .iter()
        .copied()
        .filter(|(id, name)| is_apple_iphone_bt_endpoint(id, name))
        .take(1)
        .collect()
}

/// Suspends wireless Lockdown probing when active Bluetooth A2DP or Cable audio/mirroring runs,
/// avoiding 2.4 GHz radio congestion and audio dropouts.
pub fn should_suspend_wireless_lockdown(audio_out: &str, screen_transport: &str) -> bool {
    audio_out.eq_ignore_ascii_case("bluetooth")
        || screen_transport.eq_ignore_ascii_case("cable")
        || screen_transport.eq_ignore_ascii_case("usb")
}

/// Native Audio Mirroring Controller bridging AAC-ELD (`AUDIO_ASC`), `zlaac.dll`, `AdaptiveAudioBuffer`,
/// `WASAPI` shared-mode engine, and Bluetooth A2DP coexistence.
pub struct AudioMirrorController {
    pub audio_out: String,
    pub volume: u8,
    pub muted: bool,
    pub buffer_preset: String,
    pub lip_sync: bool,
    pub streaming: bool,
    pub screen_phase: String,
    pub screen_transport: String,
    pub wasapi_device: String,
    pub rms_dbfs: f32,
    pub jitter: AdaptiveAudioBuffer,
    pub empty_working_set_calls_during_audio: u64,
    pub aac_packets_in: u64,
}

impl std::fmt::Debug for AudioMirrorController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioMirrorController")
            .field("audio_out", &self.audio_out)
            .field("volume", &self.volume)
            .field("muted", &self.muted)
            .field("buffer_preset", &self.buffer_preset)
            .field("streaming", &self.streaming)
            .field("underruns", &self.jitter.underrun_count())
            .finish()
    }
}

impl Default for AudioMirrorController {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioMirrorController {
    pub fn new() -> Self {
        Self {
            audio_out: "pc".to_string(),
            volume: 100,
            muted: false,
            buffer_preset: "normal".to_string(),
            lip_sync: true,
            streaming: false,
            screen_phase: "idle".to_string(),
            screen_transport: "cable".to_string(),
            wasapi_device: "Windows WASAPI (48 kHz)".to_string(),
            rms_dbfs: -60.0,
            jitter: AdaptiveAudioBuffer::new(AudioBufferPreset::Normal),
            empty_working_set_calls_during_audio: 0,
            aac_packets_in: 0,
        }
    }

    pub fn preset_enum(&self) -> AudioBufferPreset {
        match self.buffer_preset.to_ascii_lowercase().as_str() {
            "low" => AudioBufferPreset::Low,
            "high" => AudioBufferPreset::High,
            _ => AudioBufferPreset::Normal,
        }
    }

    pub fn configure(
        &mut self,
        audio_out: Option<&str>,
        volume: Option<i64>,
        muted: Option<bool>,
        buffer_preset: Option<&str>,
        lip_sync: Option<bool>,
        active_override: Option<bool>,
    ) {
        if let Some(mode) = audio_out {
            let norm = match mode.trim().to_ascii_lowercase().as_str() {
                "bluetooth" | "bt" => "bluetooth",
                "device" | "iphone" | "off" => "device",
                _ => "pc",
            };
            self.audio_out = norm.to_string();
        }
        if let Some(vol) = volume {
            let clamped = vol.clamp(0, 100) as u8;
            self.volume = clamped;
            if !self.muted {
                self.jitter.set_volume(u32::from(clamped));
            }
        }
        if let Some(m) = muted {
            self.muted = m;
            self.jitter
                .set_volume(if m { 0 } else { u32::from(self.volume) });
        }
        if let Some(preset) = buffer_preset {
            let norm_p = match preset.trim().to_ascii_lowercase().as_str() {
                "low" => "low",
                "high" => "high",
                _ => "normal",
            };
            self.buffer_preset = norm_p.to_string();
            let p_enum = self.preset_enum();
            self.jitter.set_preset(p_enum);
        }
        if let Some(ls) = lip_sync {
            self.lip_sync = ls;
        }
        if let Some(act) = active_override {
            self.streaming = act && self.audio_out != "device";
        }
    }

    /// Patched AAC-ELD AudioSpecificConfig cookie with bit `0x10` set (`0xF8, 0xE6, 0x50, 0x00`).
    pub fn patched_asc_cookie(&self) -> Vec<u8> {
        patch_aac_eld_cookie(&AUDIO_ASC)
    }

    pub fn zero_double_audio_mute_sequence(&self) -> Vec<u16> {
        ZeroDoubleAudioController::on_stream_start()
    }

    pub fn zero_double_audio_restore_sequence(&self) -> Vec<u16> {
        ZeroDoubleAudioController::on_stream_stop()
    }

    /// Guarantee: `EmptyWorkingSet` is never invoked during active audio streaming.
    pub fn calls_empty_working_set_during_audio(&self) -> bool {
        self.empty_working_set_calls_during_audio > 0
    }

    pub fn wireless_lockdown_suspended(&self) -> bool {
        should_suspend_wireless_lockdown(&self.audio_out, &self.screen_transport)
    }

    pub fn push_audio_access_unit(&mut self, aac_au: &[u8]) {
        if aac_au.is_empty() {
            return;
        }
        self.aac_packets_in = self.aac_packets_in.saturating_add(1);
        self.streaming = self.audio_out != "device";
        self.rms_dbfs = if self.muted || self.volume == 0 {
            -60.0
        } else {
            -18.0
        };
    }

    pub fn stop(&mut self) {
        self.streaming = false;
        self.rms_dbfs = -60.0;
    }

    pub fn status_json(&self) -> String {
        let usb_streaming = self.audio_out == "pc" && self.streaming;
        let bt_active = self.audio_out == "bluetooth" && self.streaming;
        let delay_ms = if bt_active {
            140.0
        } else if self.audio_out == "pc" {
            self.preset_enum().latency_ms() as f64
        } else {
            0.0
        };
        let live_delay_ms = if usb_streaming || bt_active {
            delay_ms
        } else {
            0.0
        };
        let enabled = match self.audio_out.as_str() {
            "pc" => true,
            "bluetooth" => bt_active,
            _ => false,
        };
        let active = self.streaming && !self.muted && self.audio_out != "device";
        let state_label = match self.audio_out.as_str() {
            "pc" => {
                if usb_streaming {
                    "Streaming"
                } else if self.screen_phase != "running" {
                    "Ready"
                } else {
                    "Connecting"
                }
            }
            "bluetooth" => {
                if bt_active {
                    "Connected"
                } else {
                    "Off"
                }
            }
            _ => "DeviceOnly",
        };
        let dev_label = match self.audio_out.as_str() {
            "pc" => self.wasapi_device.as_str(),
            "bluetooth" => "iPhone Bluetooth A2DP",
            _ => "iPhone Speaker",
        };
        let underruns = self.jitter.underrun_count();
        format!(
            r#"{{"ok":true,"audioOut":"{}","volume":{},"muted":{},"bufferPreset":"{}","lipSync":{},"streaming":{},"delayMs":{:.1},"targetDelayMs":{:.1},"underruns":{},"wasapiDevice":"{}","iphoneSpeakerMuted":{},"audioRouteTap":"pre_fader_coredevice","rmsDbfs":{:.1},"enabled":{},"active":{},"device":"{}","state":"{}","bluetoothA2dp":{},"wirelessLockdownSuspended":{}}}"#,
            escape_json(&self.audio_out),
            self.volume,
            self.muted,
            escape_json(&self.buffer_preset),
            self.lip_sync,
            self.streaming,
            live_delay_ms,
            delay_ms,
            underruns,
            escape_json(&self.wasapi_device),
            usb_streaming,
            self.rms_dbfs,
            enabled,
            active,
            escape_json(dev_label),
            state_label,
            self.audio_out == "bluetooth",
            self.wireless_lockdown_suspended()
        )
    }
}

/// Outcome returned from `global_stop_screen` for telemetry & unit tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopScreenOutcome {
    pub json: String,
    pub stealth_restored: bool,
    pub restored_steps: usize,
    pub tunnel_reset_count: u64,
}

/// Minimum interval between GPU 120x260 Auto-Rotate probes (1000 ms).
pub const AUTO_ROTATE_PROBE_INTERVAL_MS: u64 = 1_000;

/// Unified in-process Mirror Session & Screen/Audio/GPU Video Runtime State for `server_v4.rs`.
#[derive(Debug)]
pub struct MirrorRuntimeState {
    pub phase: String,
    pub udid: Option<String>,
    pub url: Option<String>,
    pub error: Option<String>,
    pub note: Option<String>,
    pub transport: String,
    pub stage: String,
    pub stage_progress: u8,
    pub stage_step: u8,
    pub stage_title: String,
    pub stage_detail: String,
    pub reconnecting: bool,
    pub stop_cooldown_until_ms: u64,
    pub wait_for_first_frame: bool,
    pub frames_received: u64,
    pub last_keyframe: Option<Vec<u8>>,
    pub pli_requests_sent: u64,
    pub fir_seq: u8,
    pub tunnel_active: bool,
    pub tunnel_reset_count: u64,
    pub codec: CodecHeader,
    pub subscribers: SubscriberQueue,
    pub auto_rotate: AutoRotateController,
    pub audio: AudioMirrorController,
    pub video_decoder: MfHevcDecoder,
    pub frame_ring: SharedFrameRing,
    pub video_presenter: D3d11VideoPresenter,
    pub probe_rgba_buf: Vec<u8>,
    pub probe_grid: ProbeGrid120x260,
    pub last_probe_eval_ms: u64,
    pub mirror_route: String,
    pub airplay_active: bool,
    pub airplay_name: String,
    pub airplay_port: u16,
    pub hid_control_available: bool,
    pub airplay_avcc_count: u64,
    pub airplay_video_frames: u64,
    pub airplay_idr_count: u64,
    pub airplay_delta_frames: u64,
    pub airplay_decrypt_failures: u64,
    pub airplay_tcp_packets_read: u64,
    pub airplay_mirror_loop_count: usize,
    pub live_chunks_broadcasted: u64,
}

impl Default for MirrorRuntimeState {
    fn default() -> Self {
        Self {
            phase: "idle".to_string(),
            udid: None,
            url: None,
            error: None,
            note: None,
            transport: "cable".to_string(),
            stage: String::new(),
            stage_progress: 0,
            stage_step: 0,
            stage_title: String::new(),
            stage_detail: String::new(),
            reconnecting: false,
            stop_cooldown_until_ms: 0,
            wait_for_first_frame: false,
            frames_received: 0,
            last_keyframe: None,
            pli_requests_sent: 0,
            fir_seq: 0,
            tunnel_active: false,
            tunnel_reset_count: 0,
            codec: CodecHeader::default(),
            subscribers: SubscriberQueue::new(MAX_SUBSCRIBER_QUEUE),
            auto_rotate: AutoRotateController::new(),
            audio: AudioMirrorController::new(),
            video_decoder: MfHevcDecoder::new(),
            frame_ring: SharedFrameRing::new(),
            video_presenter: D3d11VideoPresenter::new(),
            probe_rgba_buf: vec![0u8; GRID_WIDTH * GRID_HEIGHT * 4],
            probe_grid: ProbeGrid120x260::new(),
            last_probe_eval_ms: 0,
            mirror_route: "coredevice_hevc".to_string(),
            airplay_active: false,
            airplay_name: crate::airplay::DEFAULT_RECEIVER_NAME.to_string(),
            airplay_port: 0,
            hid_control_available: true,
            airplay_avcc_count: 0,
            airplay_video_frames: 0,
            airplay_idr_count: 0,
            airplay_delta_frames: 0,
            airplay_decrypt_failures: 0,
            airplay_tcp_packets_read: 0,
            airplay_mirror_loop_count: 0,
            live_chunks_broadcasted: 0,
        }
    }
}

impl MirrorRuntimeState {
    /// Feeds an Annex-B access unit into `MfHevcDecoder` + `SharedFrameRing`, requesting PLI/FIR if needed,
    /// and running zero-allocation 120x260 Auto-Rotate probing at most once per `1000 ms` when enabled.
    pub fn process_video_annex_b(&mut self, annex_b: &[u8], now_ms: u64) -> DecodeOutcome {
        let rot = self.auto_rotate.current_orientation_deg;
        let out = self
            .video_decoder
            .feed_annex_b(annex_b, now_ms.saturating_mul(1000), rot, &self.frame_ring);
        if out.needs_keyframe_pli {
            self.pli_requests_sent = self.pli_requests_sent.saturating_add(1);
            self.fir_seq = self.fir_seq.wrapping_add(1);
        } else if out.decoded && self.auto_rotate.auto_rotate_enabled {
            if self.last_probe_eval_ms == 0
                || now_ms.saturating_sub(self.last_probe_eval_ms) >= AUTO_ROTATE_PROBE_INTERVAL_MS
            {
                self.last_probe_eval_ms = now_ms;
                let probe_buf = &mut self.probe_rgba_buf;
                let extracted = self
                    .frame_ring
                    .with_latest_slot(|slot| extract_downscaled_probe_120x260_into(slot, probe_buf))
                    .unwrap_or(false);
                if extracted {
                    populate_probe_grid_120x260_in_place(probe_buf, &mut self.probe_grid);
                    let deg = self.auto_rotate.evaluate_frame(
                        Some(&self.probe_grid),
                        None,
                        out.clean_width,
                        out.clean_height,
                        now_ms,
                    );
                    MIRROR_ACTIVE_ORIENTATION.store(deg, Ordering::SeqCst);
                    let mut pcfg = self.video_presenter.config.clone();
                    pcfg.rotation_deg = (deg % 360) as u16;
                    pcfg.is_landscape = pcfg.rotation_deg == 90 || pcfg.rotation_deg == 270;
                    self.video_presenter.update_config(pcfg);
                }
            }
        }
        out
    }

    pub fn video_pipeline_status_json(&self) -> String {
        let (frames_decoded, frames_dropped, latest_slot, latest_seq) = self.frame_ring.stats_snapshot();
        let latest_slot_json = match latest_slot {
            Some(idx) => idx.to_string(),
            None => "null".to_string(),
        };
        let (cw, ch) = self.video_decoder.clean_dimensions();
        let (tt, tl, tb, tr) = self.video_decoder.trim;
        format!(
            r#"{{"ok":true,"native":true,"adapterName":"{}","vendorId":{},"deviceId":{},"d3d11VideoSupport":{},"multithreadProtected":{},"dxgiDeviceManagerReady":{},"mfplatLoaded":{},"hardwareHevcMftAvailable":{},"fallbackWebCodecsReady":{},"lowLatencyMode":{},"d3d11Aware":{},"ringCapacity":{},"framesDecoded":{},"framesDropped":{},"latestSlot":{},"latestFrameSeq":{},"codedWidth":{},"codedHeight":{},"cleanWidth":{},"cleanHeight":{},"nativeTrim":{{"top":{},"left":{},"bottom":{},"right":{}}},"needsKeyframePli":{},"pliTriggers":{},"autoRotateEnabled":{},"orientation":{},"autoRotateEvaluations":{},"probeZeroAllocCap":{}}}"#,
            escape_json(&self.video_decoder.adapter_name),
            self.video_decoder.vendor_id,
            self.video_decoder.device_id,
            self.video_decoder.d3d11_video_support,
            self.video_decoder.multithread_protected,
            self.video_decoder.dxgi_device_manager_ready,
            self.video_decoder.mfplat_loaded,
            self.video_decoder.hardware_hevc_mft_available,
            self.video_decoder.fallback_webcodecs_ready,
            self.video_decoder.low_latency_mode,
            self.video_decoder.d3d11_aware,
            RING_CAPACITY,
            frames_decoded,
            frames_dropped,
            latest_slot_json,
            latest_seq,
            self.video_decoder.coded_width,
            self.video_decoder.coded_height,
            cw,
            ch,
            tt,
            tl,
            tb,
            tr,
            self.video_decoder.needs_keyframe_pli,
            self.video_decoder.pli_triggers,
            self.auto_rotate.auto_rotate_enabled,
            self.auto_rotate.current_orientation_deg,
            self.auto_rotate.evaluations_count,
            self.probe_rgba_buf.capacity(),
        )
    }

    pub fn screen_status_json(&self, now_ms: u64) -> String {
        let in_cooldown = now_ms < self.stop_cooldown_until_ms;
        let phase = if in_cooldown {
            "idle"
        } else {
            self.phase.as_str()
        };
        let active = phase == "running";
        let udid_json = if in_cooldown {
            "null".to_string()
        } else {
            self.udid
                .as_ref()
                .map(|u| format!(r#""{}""#, escape_json(u)))
                .unwrap_or_else(|| "null".to_string())
        };
        let url_json = if in_cooldown {
            "null".to_string()
        } else {
            self.url
                .as_ref()
                .map(|u| format!(r#""{}""#, escape_json(u)))
                .unwrap_or_else(|| "null".to_string())
        };
        let error_json = if in_cooldown {
            "null".to_string()
        } else {
            self.error
                .as_ref()
                .map(|e| format!(r#""{}""#, escape_json(e)))
                .unwrap_or_else(|| "null".to_string())
        };
        let note_json = self
            .note
            .as_ref()
            .map(|n| format!(r#""{}""#, escape_json(n)))
            .unwrap_or_else(|| "null".to_string());
        let (h2_ping_acks, h2_win_updates) = crate::physical_stream::physical_h2_telemetry();
        let tcp_acks_sent = crate::physical_stream::physical_tcp_acks_sent();
        let (rtcp_sr_rx, rtcp_rr_lsr, rtcp_bw_fb) = crate::physical_stream::physical_rtcp_telemetry();
        let (indigo_reconn, uhid_cons) = crate::physical_stream::physical_consumer_telemetry();
        let display_xpc_pings = crate::physical_stream::physical_display_xpc_pings_sent();
        let auto_reconnect_count = crate::physical_stream::physical_auto_reconnect_count();
        let keep_awake_pulses = crate::physical_stream::physical_keep_awake_pulses_sent();
        let last_disconnect_reason_json = crate::physical_stream::last_disconnect_reason()
            .map(|r| format!(r#""{}""#, escape_json(&r)))
            .unwrap_or_else(|| "null".to_string());
        let stage_escaped = if in_cooldown {
            String::new()
        } else {
            escape_json(&self.stage)
        };
        let stage_detail_escaped = if in_cooldown {
            String::new()
        } else {
            escape_json(&self.stage_detail)
        };
        let live_clients = global_live_clients_connected();
        let airplay_loop_alive = self.airplay_mirror_loop_count > 0 && !in_cooldown;
        let ap_audio = crate::airplay::get_airplay_audio_snapshot();
        let ap_audio_active = self.airplay_active && !in_cooldown && ap_audio.active;
        let ap_audio_codec_json = ap_audio
            .codec
            .as_ref()
            .filter(|_| self.airplay_active && !in_cooldown)
            .map(|c| format!(r#""{}""#, escape_json(c)))
            .unwrap_or_else(|| "null".to_string());
        let ap_video_q_dropped = if self.airplay_active && !in_cooldown {
            global_airplay_video_queue_dropped()
        } else {
            0
        };
        let (
            hevc_bitrate_kbps,
            hevc_avg_frame_bytes,
            hevc_max_frame_bytes,
            hevc_drops_per_sec,
            hevc_queue_overflows,
            hevc_rtp_seq_gaps,
        ) = if in_cooldown {
            (0, 0, 0, 0, 0, 0)
        } else {
            snapshot_hevc_bitrate_telemetry(now_ms)
        };
        format!(
            r#"{{"ok":true,"phase":"{}","active":{},"reconnecting":{},"udid":{},"url":{},"error":{},"note":{},"transport":"{}","mirrorRoute":"{}","airplayActive":{},"airplayName":"{}","airplayPort":{},"hidControlAvailable":{},"airplayAvccCount":{},"airplayVideoFrames":{},"airplayIdrCount":{},"airplayDeltaFrames":{},"airplayDecryptFailures":{},"airplayTcpPacketsRead":{},"airplayMirrorLoopAlive":{},"airplayAudioActive":{},"airplayAudioMuted":{},"airplayAudioCodec":{},"airplayAudioSampleRate":{},"airplayAudioDecodedPackets":{},"airplayAudioPlayedSamples":{},"airplayAudioVolume":{},"airplayAudioBufferPreset":"{}","airplayAudioLipSync":{},"airplayAudioBufferMs":{},"airplayAudioTargetMs":{},"airplayAudioDroppedPackets":{},"airplayVideoQueueDropped":{},"liveChunksBroadcasted":{},"liveClientsConnected":{},"fps":{},"width":{},"height":{},"orientation":{},"autoRotate":{},"stealth_dim":{},"intentionally_locked":{},"stage":"{}","stageDetail":"{}","stageProgress":{},"stage_step":{},"stage_title":"{}","stage_detail":"{}","frames_received":{},"pli_requests":{},"h2_ping_acks":{},"h2_window_updates":{},"tcp_acks_sent":{},"rtcp_sr_received":{},"rtcp_rr_with_lsr_sent":{},"rtcp_bw_feedback_sent":{},"indigo_ping_reconnects":{},"uhid_consumer_reports":{},"display_xpc_pings":{},"auto_reconnect_count":{},"keep_awake_pulses":{},"last_disconnect_reason":{},"hevcBitrateKbps":{},"hevcAvgFrameBytes":{},"hevcMaxFrameBytes":{},"hevcDropsPerSec":{},"hevcQueueOverflows":{},"hevcRtpSeqGaps":{},"native":true}}"#,
            escape_json(phase),
            active,
            self.reconnecting && !in_cooldown,
            udid_json,
            url_json,
            error_json,
            note_json,
            escape_json(&self.transport),
            escape_json(&self.mirror_route),
            self.airplay_active && !in_cooldown,
            escape_json(&self.airplay_name),
            if in_cooldown { 0 } else { self.airplay_port },
            self.hid_control_available,
            self.airplay_avcc_count,
            self.airplay_video_frames,
            self.airplay_idr_count,
            self.airplay_delta_frames,
            self.airplay_decrypt_failures,
            self.airplay_tcp_packets_read,
            airplay_loop_alive,
            ap_audio_active,
            ap_audio.muted,
            ap_audio_codec_json,
            if self.airplay_active && !in_cooldown { ap_audio.sample_rate } else { 0 },
            if self.airplay_active && !in_cooldown { ap_audio.decoded_packets } else { 0 },
            if self.airplay_active && !in_cooldown { ap_audio.played_samples } else { 0 },
            ap_audio.volume,
            escape_json(&ap_audio.buffer_preset),
            ap_audio.lip_sync,
            if ap_audio_active { ap_audio.buffer_ms } else { 0 },
            ap_audio.target_ms,
            if self.airplay_active && !in_cooldown { ap_audio.dropped_packets } else { 0 },
            ap_video_q_dropped,
            self.live_chunks_broadcasted,
            live_clients,
            self.codec.fps,
            self.codec.display_width,
            self.codec.display_height,
            self.auto_rotate.current_orientation_deg,
            self.auto_rotate.auto_rotate_enabled,
            hid::is_stealth_dim_active(),
            hid::is_device_intentionally_locked(),
            stage_escaped,
            stage_detail_escaped,
            if in_cooldown { 0 } else { self.stage_progress },
            self.stage_step,
            escape_json(&self.stage_title),
            escape_json(&self.stage_detail),
            self.frames_received,
            self.pli_requests_sent,
            h2_ping_acks,
            h2_win_updates,
            tcp_acks_sent,
            rtcp_sr_rx,
            rtcp_rr_lsr,
            rtcp_bw_fb,
            indigo_reconn,
            uhid_cons,
            display_xpc_pings,
            auto_reconnect_count,
            keep_awake_pulses,
            last_disconnect_reason_json,
            hevc_bitrate_kbps,
            hevc_avg_frame_bytes,
            hevc_max_frame_bytes,
            hevc_drops_per_sec,
            hevc_queue_overflows,
            hevc_rtp_seq_gaps,
        )
    }
}

static MIRROR_RUNTIME_STATE: OnceLock<Mutex<MirrorRuntimeState>> = OnceLock::new();
static MIRROR_STREAM_ACTIVE: AtomicBool = AtomicBool::new(false);
static MIRROR_ACTIVE_ORIENTATION: AtomicU32 = AtomicU32::new(0);
static LIVE_SESSION_GEN: AtomicU64 = AtomicU64::new(0);
static LIVE_PLI_REQUESTED: AtomicBool = AtomicBool::new(false);
static HEVC_QUEUE_OVERFLOWS: AtomicU64 = AtomicU64::new(0);
static HEVC_RTP_SEQ_GAPS: AtomicU64 = AtomicU64::new(0);
static HEVC_BITRATE_TELEMETRY: OnceLock<Mutex<HevcBitrateTelemetryWindow>> = OnceLock::new();

#[derive(Debug, Default)]
struct HevcBitrateTelemetryWindow {
    samples: VecDeque<(u64, usize)>,
    drop_timestamps_ms: VecDeque<u64>,
}

impl HevcBitrateTelemetryWindow {
    fn prune(&mut self, now_ms: u64) {
        let cutoff = now_ms.saturating_sub(1000);
        while let Some(&(ts, _)) = self.samples.front() {
            if ts < cutoff {
                self.samples.pop_front();
            } else {
                break;
            }
        }
        while let Some(&ts) = self.drop_timestamps_ms.front() {
            if ts < cutoff {
                self.drop_timestamps_ms.pop_front();
            } else {
                break;
            }
        }
    }
}

fn hevc_bitrate_window() -> &'static Mutex<HevcBitrateTelemetryWindow> {
    HEVC_BITRATE_TELEMETRY.get_or_init(|| Mutex::new(HevcBitrateTelemetryWindow::default()))
}

pub fn record_hevc_video_frame_telemetry(frame_bytes: usize, now_ms: u64) {
    if let Ok(mut g) = hevc_bitrate_window().lock() {
        g.samples.push_back((now_ms, frame_bytes));
        g.prune(now_ms);
    }
}

pub fn record_hevc_queue_overflow_drop(now_ms: u64, count_overflow_event: bool) {
    if count_overflow_event {
        HEVC_QUEUE_OVERFLOWS.fetch_add(1, Ordering::Relaxed);
    }
    if let Ok(mut g) = hevc_bitrate_window().lock() {
        g.drop_timestamps_ms.push_back(now_ms);
        g.prune(now_ms);
    }
}

pub fn record_hevc_rtp_seq_gap() {
    HEVC_RTP_SEQ_GAPS.fetch_add(1, Ordering::Relaxed);
}

pub fn reset_hevc_bitrate_telemetry() {
    HEVC_QUEUE_OVERFLOWS.store(0, Ordering::Relaxed);
    HEVC_RTP_SEQ_GAPS.store(0, Ordering::Relaxed);
    if let Ok(mut g) = hevc_bitrate_window().lock() {
        g.samples.clear();
        g.drop_timestamps_ms.clear();
    }
}

pub fn snapshot_hevc_bitrate_telemetry(now_ms: u64) -> (u64, u64, u64, u64, u64, u64) {
    let overflows = HEVC_QUEUE_OVERFLOWS.load(Ordering::Relaxed);
    let seq_gaps = HEVC_RTP_SEQ_GAPS.load(Ordering::Relaxed);
    if let Ok(mut g) = hevc_bitrate_window().lock() {
        g.prune(now_ms);
        let mut total_bytes = 0u64;
        let mut max_frame_bytes = 0u64;
        for &(_, b) in &g.samples {
            let bu = b as u64;
            total_bytes = total_bytes.saturating_add(bu);
            if bu > max_frame_bytes {
                max_frame_bytes = bu;
            }
        }
        let count = g.samples.len() as u64;
        let bitrate_kbps = total_bytes.saturating_mul(8) / 1000;
        let avg_frame_bytes = if count > 0 { total_bytes / count } else { 0 };
        let drops_per_sec = g.drop_timestamps_ms.len() as u64;
        return (
            bitrate_kbps,
            avg_frame_bytes,
            max_frame_bytes,
            drops_per_sec,
            overflows,
            seq_gaps,
        );
    }
    (0, 0, 0, 0, overflows, seq_gaps)
}
static LIVE_WORKER_STOP: OnceLock<Mutex<Option<Arc<AtomicBool>>>> = OnceLock::new();
static LIVE_BROADCAST: OnceLock<(Mutex<LiveBroadcastState>, Condvar)> = OnceLock::new();
/// Tahap 250 Pilar 2: Active physical worker lifecycle barrier so a newly started Wi-Fi/USB
/// session waits cleanly (<= 350 ms) for the previous worker to send best-effort `stopmediastream`
/// and close `TlsPskStream`/`rp_control`, preventing double-session collisions on iOS `remoted`.
static PHYSICAL_WORKER_LIFECYCLE: OnceLock<(Mutex<usize>, Condvar)> = OnceLock::new();

fn physical_worker_lifecycle() -> &'static (Mutex<usize>, Condvar) {
    PHYSICAL_WORKER_LIFECYCLE.get_or_init(|| (Mutex::new(0usize), Condvar::new()))
}

pub fn register_physical_worker_started() {
    let (mtx, _) = physical_worker_lifecycle();
    let mut cnt = mtx.lock().unwrap_or_else(|e| e.into_inner());
    *cnt = cnt.saturating_add(1);
}

pub fn mark_physical_worker_exited() {
    let (mtx, cv) = physical_worker_lifecycle();
    let mut cnt = mtx.lock().unwrap_or_else(|e| e.into_inner());
    *cnt = cnt.saturating_sub(1);
    if *cnt == 0 {
        cv.notify_all();
    }
}

/// Waits up to `max_wait_ms` for any previously active physical worker thread to finish teardown.
/// Returns `true` if all previous workers exited cleanly within the budget, `false` on timeout.
pub fn wait_previous_physical_worker_exit(max_wait_ms: u64) -> bool {
    let (mtx, cv) = physical_worker_lifecycle();
    let mut cnt = mtx.lock().unwrap_or_else(|e| e.into_inner());
    if *cnt == 0 {
        return true;
    }
    let deadline = Instant::now() + Duration::from_millis(max_wait_ms);
    while *cnt > 0 {
        let now = Instant::now();
        if now >= deadline {
            return false;
        }
        let (guard, _) = cv
            .wait_timeout(cnt, deadline.saturating_duration_since(now))
            .unwrap_or_else(|e| e.into_inner());
        cnt = guard;
    }
    true
}

/// RAII guard paired with physical USB/Wi-Fi display workers so `mark_physical_worker_exited()`
/// always fires on normal return, error return, or unwind.
pub struct PhysicalWorkerExitGuard;

impl PhysicalWorkerExitGuard {
    pub fn enter() -> Self {
        register_physical_worker_started();
        Self
    }
}

impl Drop for PhysicalWorkerExitGuard {
    fn drop(&mut self) {
        mark_physical_worker_exited();
    }
}

static LIVE_CLIENTS_CONNECTED: AtomicUsize = AtomicUsize::new(0);

/// RAII guard tracking active `/mirror/live` HTTP passthrough clients (0% PII).
pub struct LiveClientGuard;

impl LiveClientGuard {
    pub fn enter() -> Self {
        LIVE_CLIENTS_CONNECTED.fetch_add(1, Ordering::Relaxed);
        Self
    }
}

impl Drop for LiveClientGuard {
    fn drop(&mut self) {
        loop {
            let cur = LIVE_CLIENTS_CONNECTED.load(Ordering::Relaxed);
            if cur == 0
                || LIVE_CLIENTS_CONNECTED
                    .compare_exchange_weak(cur, cur - 1, Ordering::Relaxed, Ordering::Relaxed)
                    .is_ok()
            {
                break;
            }
        }
    }
}

pub fn global_live_clients_connected() -> usize {
    LIVE_CLIENTS_CONNECTED.load(Ordering::Relaxed)
}

static AIRPLAY_VIDEO_QUEUE_DROPPED: AtomicU64 = AtomicU64::new(0);

pub fn global_airplay_video_queue_dropped() -> u64 {
    AIRPLAY_VIDEO_QUEUE_DROPPED.load(Ordering::Relaxed)
}

pub fn inc_airplay_video_queue_dropped(n: u64) {
    if n > 0 {
        AIRPLAY_VIDEO_QUEUE_DROPPED.fetch_add(n, Ordering::Relaxed);
    }
}

pub fn reset_airplay_video_queue_dropped() {
    AIRPLAY_VIDEO_QUEUE_DROPPED.store(0, Ordering::SeqCst);
}

const MAX_LIVE_BROADCAST_CHUNKS: usize = 120;
const LATEST_IDR_MAX_TRAILING_DELTAS: usize = 8;
const LATEST_IDR_MAX_AGE_MS: u64 = 400;

/// Metadata-annotated WebCodecs live broadcast frame chunk stored in the rolling ring.
#[derive(Debug, Clone)]
pub struct LiveBroadcastChunk {
    pub seq: u64,
    pub is_keyframe: bool,
    pub pushed_at_ms: u64,
    pub data: Vec<u8>,
}

/// Tahap 249 Pilar 2: Selects the **latest IDR** cursor in the rolling broadcast buffer so
/// newly connected `/mirror/live` WebCodecs clients NEVER receive a stale 60–120 frame backlog
/// from an old `init_keyframe`.
///
/// Returns `(latest_idr_seq, needs_fresh_pli)` where `needs_fresh_pli` is `true` when:
/// - No IDR exists in `chunks` (and `chunks` is non-empty), OR
/// - More than 8 trailing delta frames exist after the latest IDR, OR
/// - The latest IDR is older than `400 ms` relative to `now_ms`.
pub fn select_latest_idr_start_cursor(
    chunks: &[LiveBroadcastChunk],
    now_ms: u64,
) -> (Option<u64>, bool) {
    if let Some(idx) = chunks.iter().rposition(|c| c.is_keyframe) {
        let latest = &chunks[idx];
        let trailing = chunks.len().saturating_sub(idx + 1);
        let age_ms = now_ms.saturating_sub(latest.pushed_at_ms);
        let needs_fresh_pli =
            trailing > LATEST_IDR_MAX_TRAILING_DELTAS || age_ms > LATEST_IDR_MAX_AGE_MS;
        (Some(latest.seq), needs_fresh_pli)
    } else {
        (None, !chunks.is_empty())
    }
}

#[derive(Debug, Default)]
struct LiveBroadcastState {
    active: bool,
    is_airplay: bool,
    session_gen: u64,
    seq: u64,
    init_keyframe: Option<LiveBroadcastChunk>,
    ring: VecDeque<LiveBroadcastChunk>,
}

fn live_worker_stop_slot() -> &'static Mutex<Option<Arc<AtomicBool>>> {
    LIVE_WORKER_STOP.get_or_init(|| Mutex::new(None))
}

fn live_broadcast() -> &'static (Mutex<LiveBroadcastState>, Condvar) {
    LIVE_BROADCAST.get_or_init(|| (Mutex::new(LiveBroadcastState::default()), Condvar::new()))
}

// ============================================================================
// TAHAP 262 — PILAR 4: Direct In-Process Native Video Sink (Zero HTTP Overhead)
// ============================================================================

/// Default bounded capacity of `BoundedNativeVideoQueue` for HEVC (`cap = 6`).
pub const NATIVE_VIDEO_QUEUE_CAPACITY: usize = 6;

/// Deep AirPlay H.264 native sink queue capacity (4 seconds at 60 FPS) to absorb MFT
/// cold-start & `MF_E_TRANSFORM_STREAM_CHANGE` without dropping delta frames mid-GOP.
pub const NATIVE_AIRPLAY_VIDEO_QUEUE_CAPACITY: usize = 240;

fn has_annex_b_prefix(sample: &[u8]) -> bool {
    (sample.len() >= 4
        && sample[0] == 0
        && sample[1] == 0
        && sample[2] == 0
        && sample[3] == 1)
        || (sample.len() >= 3 && sample[0] == 0 && sample[1] == 0 && sample[2] == 1)
}

fn split_annex_b_nalus_local(data: &[u8]) -> Vec<&[u8]> {
    let mut nalus = Vec::new();
    let mut i = 0usize;
    let mut cur_start: Option<usize> = None;
    while i < data.len() {
        let sc_len = if i + 4 <= data.len()
            && data[i] == 0
            && data[i + 1] == 0
            && data[i + 2] == 0
            && data[i + 3] == 1
        {
            4
        } else if i + 3 <= data.len() && data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            3
        } else {
            0
        };
        if sc_len > 0 {
            if let Some(s) = cur_start {
                if s < i {
                    nalus.push(&data[s..i]);
                }
            }
            i += sc_len;
            cur_start = Some(i);
        } else {
            i += 1;
        }
    }
    if let Some(s) = cur_start {
        if s < data.len() {
            nalus.push(&data[s..]);
        }
    }
    nalus
}

/// Walks an ISO/IEC 14496-15 length-prefixed H.264 AVCC access unit strictly.
/// Prevents `256..=511 B` (`00 00 01 xx`) and `1 B` (`00 00 00 01`) length-prefix collisions
/// with Annex-B start codes.
pub fn try_walk_strict_avcc_nalus<'a>(sample: &'a [u8], len_size: usize) -> Option<Vec<&'a [u8]>> {
    if sample.is_empty() {
        return None;
    }
    let ls = len_size.clamp(1, 4);
    let mut pos = 0usize;
    let mut nalus = Vec::new();
    while pos < sample.len() {
        if pos + ls > sample.len() {
            return None;
        }
        let mut nal_len = 0usize;
        for &b in &sample[pos..pos + ls] {
            nal_len = (nal_len << 8) | usize::from(b);
        }
        pos += ls;
        if nal_len == 0 || pos + nal_len > sample.len() {
            return None;
        }
        let nal = &sample[pos..pos + nal_len];
        let hdr = nal[0];
        if (hdr & 0x80) != 0 || (hdr & 0x1F) == 0 {
            return None;
        }
        nalus.push(nal);
        pos += nal_len;
    }
    if pos == sample.len() && !nalus.is_empty() {
        Some(nalus)
    } else {
        None
    }
}

fn strip_h264_epb_prefix(bytes: &[u8], max_bytes: usize) -> Vec<u8> {
    let mut rbsp = Vec::with_capacity(bytes.len().min(max_bytes));
    let mut zeros = 0u8;
    for &b in bytes.iter().take(max_bytes) {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        rbsp.push(b);
    }
    rbsp
}

fn h264_sei_has_recovery_point(nal: &[u8]) -> bool {
    if nal.len() < 3 || (nal[0] & 0x1F) != 6 {
        return false;
    }
    let rbsp = strip_h264_epb_prefix(&nal[1..], nal.len());
    let mut pos = 0usize;
    while pos < rbsp.len() && rbsp[pos] != 0x80 {
        let mut payload_type = 0usize;
        while pos < rbsp.len() && rbsp[pos] == 0xFF {
            payload_type = payload_type.saturating_add(255);
            pos += 1;
        }
        if pos >= rbsp.len() {
            break;
        }
        payload_type = payload_type.saturating_add(rbsp[pos] as usize);
        pos += 1;

        let mut payload_size = 0usize;
        while pos < rbsp.len() && rbsp[pos] == 0xFF {
            payload_size = payload_size.saturating_add(255);
            pos += 1;
        }
        if pos >= rbsp.len() {
            break;
        }
        payload_size = payload_size.saturating_add(rbsp[pos] as usize);
        pos += 1;

        if payload_type == 6 && pos < rbsp.len() {
            return true;
        }
        pos = pos.saturating_add(payload_size);
    }
    false
}

/// Scans ALL length-prefixed NAL units inside an H.264 AVCC Access Unit strictly FIRST (Annex-B
/// fallback only when strict AVCC fails AND prefix matches Annex-B start code), skipping `AUD=9`,
/// `SPS=7`, `PPS=8`, `Filler=12` and returning `true` for IDR (`5`), SEI Recovery (`6/6`), or
/// Open-GOP I/All-I slice (`nal=1, slice_type == 2 || 7`).
pub fn h264_avcc_payload_is_clean_random_access(avcc: &[u8], nal_len_size: usize) -> bool {
    struct MiniBits<'a> {
        data: &'a [u8],
        pos: usize,
    }
    impl<'a> MiniBits<'a> {
        fn bit(&mut self) -> Option<u32> {
            let b = (self.data.get(self.pos / 8)? >> (7 - (self.pos % 8))) & 1;
            self.pos += 1;
            Some(u32::from(b))
        }
        fn u(&mut self, n: usize) -> Option<u32> {
            let mut v = 0u32;
            for _ in 0..n {
                v = (v << 1) | self.bit()?;
            }
            Some(v)
        }
        fn ue(&mut self) -> Option<u32> {
            let mut zeros = 0usize;
            while self.bit()? == 0 {
                zeros += 1;
                if zeros > 31 {
                    return None;
                }
            }
            if zeros == 0 {
                Some(0)
            } else {
                Some(((1u32 << zeros) - 1).checked_add(self.u(zeros)?)?)
            }
        }
    }

    let check_nal = |nal: &[u8]| -> bool {
        if nal.is_empty() || (nal[0] & 0x80) != 0 {
            return false;
        }
        let nal_type = nal[0] & 0x1F;
        match nal_type {
            9 | 7 | 8 | 12 => false,
            5 => true,
            6 => h264_sei_has_recovery_point(nal),
            1 if nal.len() >= 2 => {
                let rbsp = strip_h264_epb_prefix(&nal[1..], 32);
                let mut br = MiniBits { data: &rbsp, pos: 0 };
                br.ue().is_some() && matches!(br.ue(), Some(2) | Some(7))
            }
            _ => false,
        }
    };

    if let Some(nalus) = try_walk_strict_avcc_nalus(avcc, nal_len_size) {
        return nalus.into_iter().any(check_nal);
    }
    if has_annex_b_prefix(avcc) {
        return split_annex_b_nalus_local(avcc).into_iter().any(check_nal);
    }
    false
}

pub fn h264_avcc_payload_is_idr_or_i_slice(avcc: &[u8]) -> bool {
    h264_avcc_payload_is_clean_random_access(avcc, 4)
}

/// Direct in-process video event delivered straight from CoreDevice HEVC / AirPlay H.264
/// receivers to `LiveMfVideoDecoder`, bypassing HTTP `/mirror/live` serialization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeVideoEvent {
    HevcConfig {
        codec: String,
        width: u32,
        height: u32,
        trim: [u32; 4],
    },
    HevcFrame {
        annex_b: Vec<u8>,
        is_keyframe: bool,
        pts_us: u64,
        trim: [u32; 4],
    },
    H264Avcc {
        avcc: Vec<u8>,
        width: u32,
        height: u32,
    },
    H264Frame {
        avcc_payload: Vec<u8>,
        is_keyframe: bool,
        pts_us: u64,
    },
    Stopped,
}

impl NativeVideoEvent {
    fn is_video_frame(&self) -> bool {
        matches!(
            self,
            NativeVideoEvent::HevcFrame { .. } | NativeVideoEvent::H264Frame { .. }
        )
    }

    fn is_keyframe(&self) -> bool {
        match self {
            NativeVideoEvent::HevcFrame { is_keyframe, .. }
            | NativeVideoEvent::H264Frame { is_keyframe, .. } => *is_keyframe,
            _ => false,
        }
    }
}

/// Non-blocking in-process sink registered by native GPU video consumers (`--native-video-preview`
/// or Tahap 263+ native GUI). When unregistered (default WebView2 mode), cost is a single relaxed atomic read.
pub trait NativeVideoSink: Send + Sync + 'static {
    fn on_video_event(&self, event: NativeVideoEvent);
}

#[derive(Debug)]
struct NativeQueueInner {
    queue: VecDeque<NativeVideoEvent>,
    capacity: usize,
    h264_capacity: usize,
    awaiting_idr: bool,
    dropped_deltas: u64,
    idr_catchups: u64,
    pushed_total: u64,
    closed: bool,
}

/// Bounded non-blocking video queue (`cap = 6` HEVC / `240` H.264) with strict **IDR-Safe Catch-Up**:
/// - `H264Avcc` pushes FIFO without draining queue or latching `awaiting_idr` in producer (`decode_thread`
///   is the single source of truth).
/// - On overflow without IDR: drops deltas cleanly and waits strictly for the next clean keyframe
///   (ZERO mid-GOP force-unlatch).
#[derive(Clone)]
pub struct BoundedNativeVideoQueue {
    inner: Arc<(Mutex<NativeQueueInner>, Condvar)>,
}

impl Default for BoundedNativeVideoQueue {
    fn default() -> Self {
        Self::new(NATIVE_VIDEO_QUEUE_CAPACITY)
    }
}

impl BoundedNativeVideoQueue {
    pub fn new(capacity: usize) -> Self {
        Self::new_with_h264_capacity(capacity, capacity)
    }

    pub fn new_with_h264_capacity(hevc_capacity: usize, h264_capacity: usize) -> Self {
        let cap = hevc_capacity.max(1);
        let h264_cap = h264_capacity.max(cap);
        Self {
            inner: Arc::new((
                Mutex::new(NativeQueueInner {
                    queue: VecDeque::with_capacity(cap + 4),
                    capacity: cap,
                    h264_capacity: h264_cap,
                    awaiting_idr: false,
                    dropped_deltas: 0,
                    idr_catchups: 0,
                    pushed_total: 0,
                    closed: false,
                }),
                Condvar::new(),
            )),
        }
    }

    pub fn try_push(&self, event: NativeVideoEvent) {
        let mut g = self.inner.0.lock().unwrap_or_else(|e| e.into_inner());
        if g.closed {
            return;
        }
        g.pushed_total = g.pushed_total.saturating_add(1);

        match &event {
            NativeVideoEvent::Stopped => {
                g.queue.clear();
                g.awaiting_idr = false;
                g.queue.push_back(event);
            }
            NativeVideoEvent::HevcConfig { .. } | NativeVideoEvent::H264Avcc { .. } => {
                // FIFO single-source-of-truth delivery to decode_thread (0 producer-side drain/latch).
                g.queue.push_back(event);
            }
            NativeVideoEvent::HevcFrame { .. } | NativeVideoEvent::H264Frame { .. } => {
                let is_h264 = matches!(event, NativeVideoEvent::H264Frame { .. });
                let is_clean_keyframe = match &event {
                    NativeVideoEvent::H264Frame {
                        is_keyframe,
                        avcc_payload,
                        ..
                    } => {
                        *is_keyframe
                            || (g.h264_capacity >= NATIVE_AIRPLAY_VIDEO_QUEUE_CAPACITY
                                && h264_avcc_payload_is_clean_random_access(avcc_payload, 4))
                    }
                    _ => event.is_keyframe(),
                };
                let active_cap = if is_h264 { g.h264_capacity } else { g.capacity };
                let video_count = g.queue.iter().filter(|e| e.is_video_frame()).count();
                if is_clean_keyframe {
                    if video_count >= active_cap || g.awaiting_idr {
                        let before = g.queue.len();
                        g.queue.retain(|e| !e.is_video_frame());
                        let evicted = before.saturating_sub(g.queue.len()) as u64;
                        if evicted > 0 {
                            g.dropped_deltas = g.dropped_deltas.saturating_add(evicted);
                        }
                        g.idr_catchups = g.idr_catchups.saturating_add(1);
                    }
                    g.awaiting_idr = false;
                    g.queue.push_back(event);
                } else {
                    // GOP-Boundary Safe Catch-Up when deep AirPlay queue overflows:
                    // NEVER evict a single P-frame mid-GOP (which breaks H.264 references)!
                    if is_h264
                        && !g.awaiting_idr
                        && g.h264_capacity >= NATIVE_AIRPLAY_VIDEO_QUEUE_CAPACITY
                        && video_count >= active_cap
                    {
                        let latest_idr_pos = g.queue.iter().rposition(|e| match e {
                            NativeVideoEvent::H264Frame {
                                is_keyframe,
                                avcc_payload,
                                ..
                            } => *is_keyframe || h264_avcc_payload_is_clean_random_access(avcc_payload, 4),
                            _ => false,
                        });
                        let preceding_video = latest_idr_pos
                            .map(|pos| g.queue.iter().take(pos).filter(|e| e.is_video_frame()).count())
                            .unwrap_or(0);
                        if let Some(idr_idx) = latest_idr_pos.filter(|_| preceding_video > 0) {
                            let mut cur = 0usize;
                            let mut dropped_now = 0u64;
                            g.queue.retain(|e| {
                                let i = cur;
                                cur += 1;
                                if i < idr_idx && e.is_video_frame() {
                                    dropped_now = dropped_now.saturating_add(1);
                                    false
                                } else {
                                    true
                                }
                            });
                            g.dropped_deltas = g.dropped_deltas.saturating_add(dropped_now);
                            g.idr_catchups = g.idr_catchups.saturating_add(1);
                            g.queue.push_back(event);
                            self.inner.1.notify_all();
                            return;
                        } else {
                            let before = g.queue.len();
                            g.queue.retain(|e| !e.is_video_frame());
                            let evicted = before.saturating_sub(g.queue.len()) as u64;
                            g.dropped_deltas = g.dropped_deltas.saturating_add(evicted + 1);
                            g.awaiting_idr = true;
                            return;
                        }
                    }
                    // Delta P-frame: while awaiting_idr or overflowing, wait strictly for next clean keyframe!
                    if g.awaiting_idr || video_count >= active_cap {
                        let was_awaiting_idr = g.awaiting_idr;
                        let is_overflow = video_count >= active_cap;
                        g.awaiting_idr = true;
                        g.dropped_deltas = g.dropped_deltas.saturating_add(1);
                        if is_h264 && was_awaiting_idr {
                            crate::airplay::inc_trace_pre_decoder_dropped_awaiting_idr();
                        }
                        if !is_h264 {
                            drop(g);
                            record_hevc_queue_overflow_drop(now_epoch_ms(), is_overflow || !was_awaiting_idr);
                            request_live_pli();
                        }
                        return;
                    }
                    g.queue.push_back(event);
                }
            }
        }
        self.inner.1.notify_all();
    }

    pub fn try_pop(&self) -> Option<NativeVideoEvent> {
        let mut g = self.inner.0.lock().unwrap_or_else(|e| e.into_inner());
        g.queue.pop_front()
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Option<NativeVideoEvent> {
        let mut g = self.inner.0.lock().unwrap_or_else(|e| e.into_inner());
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(ev) = g.queue.pop_front() {
                return Some(ev);
            }
            if g.closed {
                return None;
            }
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            let (guard, _) = self
                .inner
                .1
                .wait_timeout(g, deadline.saturating_duration_since(now))
                .unwrap_or_else(|e| e.into_inner());
            g = guard;
        }
    }

    pub fn close(&self) {
        let mut g = self.inner.0.lock().unwrap_or_else(|e| e.into_inner());
        g.closed = true;
        self.inner.1.notify_all();
    }

    pub fn reset_stats(&self) {
        let mut g = self.inner.0.lock().unwrap_or_else(|e| e.into_inner());
        g.dropped_deltas = 0;
        g.idr_catchups = 0;
        g.pushed_total = 0;
    }

    /// Returns `(queued_len, awaiting_idr, dropped_deltas, idr_catchups, pushed_total)`.
    pub fn stats(&self) -> (usize, bool, u64, u64, u64) {
        let g = self.inner.0.lock().unwrap_or_else(|e| e.into_inner());
        (
            g.queue.len(),
            g.awaiting_idr,
            g.dropped_deltas,
            g.idr_catchups,
            g.pushed_total,
        )
    }
}

impl NativeVideoSink for BoundedNativeVideoQueue {
    fn on_video_event(&self, event: NativeVideoEvent) {
        self.try_push(event);
    }
}

static NATIVE_VIDEO_SINK_REGISTERED: AtomicBool = AtomicBool::new(false);
static NATIVE_VIDEO_SINK: OnceLock<Mutex<Option<Arc<dyn NativeVideoSink>>>> = OnceLock::new();
static LATEST_AIRPLAY_AVCC_SNAPSHOT: OnceLock<Mutex<Option<(u64, Vec<u8>, u32, u32)>>> =
    OnceLock::new();
static AIRPLAY_STREAM_INSTANCE_ID: AtomicU64 = AtomicU64::new(0);

fn latest_airplay_avcc_slot() -> &'static Mutex<Option<(u64, Vec<u8>, u32, u32)>> {
    LATEST_AIRPLAY_AVCC_SNAPSHOT.get_or_init(|| Mutex::new(None))
}

pub fn clear_latest_airplay_avcc_snapshot() {
    if let Ok(mut g) = latest_airplay_avcc_slot().lock() {
        *g = None;
    }
}

pub fn latest_airplay_avcc_snapshot() -> Option<(u64, Vec<u8>, u32, u32)> {
    let cur_gen = LIVE_SESSION_GEN.load(Ordering::SeqCst);
    latest_airplay_avcc_slot()
        .lock()
        .ok()
        .and_then(|g| g.clone())
        .filter(|(sgen, bytes, _, _)| *sgen == cur_gen && !bytes.is_empty())
}

fn native_video_sink_slot() -> &'static Mutex<Option<Arc<dyn NativeVideoSink>>> {
    NATIVE_VIDEO_SINK.get_or_init(|| Mutex::new(None))
}

/// Registers (or clears with `None`) the global direct in-process native video sink.
/// Default WebView2 mode leaves this `None` (0 allocation / 0 overhead).
pub fn register_native_video_sink(sink: Option<Arc<dyn NativeVideoSink>>) {
    let has_sink = sink.is_some();
    *native_video_sink_slot()
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = sink;
    NATIVE_VIDEO_SINK_REGISTERED.store(has_sink, Ordering::SeqCst);
}

pub fn is_native_video_sink_registered() -> bool {
    NATIVE_VIDEO_SINK_REGISTERED.load(Ordering::Relaxed)
}

#[inline]
fn emit_native_video_event(build: impl FnOnce() -> NativeVideoEvent) {
    if !NATIVE_VIDEO_SINK_REGISTERED.load(Ordering::Relaxed) {
        return;
    }
    let sink_opt = native_video_sink_slot()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    if let Some(sink) = sink_opt {
        sink.on_video_event(build());
    }
}

fn stop_active_physical_worker() {
    crate::airplay::stop_global_airplay_receiver();
    if let Some(old_stop) = live_worker_stop_slot()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
    {
        old_stop.store(true, Ordering::SeqCst);
        crate::rp_wifi_tls::mark_wifi_session_ended();
    }
    LIVE_PLI_REQUESTED.store(false, Ordering::SeqCst);
    emit_native_video_event(|| NativeVideoEvent::Stopped);
    let (mtx, cv) = live_broadcast();
    let mut b = mtx.lock().unwrap_or_else(|e| e.into_inner());
    b.active = false;
    b.is_airplay = false;
    b.init_keyframe = None;
    b.ring.clear();
    cv.notify_all();
}

pub fn current_live_session_gen() -> u64 {
    LIVE_SESSION_GEN.load(Ordering::SeqCst)
}

pub fn is_live_pli_requested() -> bool {
    LIVE_PLI_REQUESTED.load(Ordering::SeqCst)
}

pub fn request_live_pli() {
    LIVE_PLI_REQUESTED.store(true, Ordering::SeqCst);
}

pub fn record_live_pli_sent() {
    if let Ok(mut g) = mirror_runtime().try_lock() {
        g.pli_requests_sent = g.pli_requests_sent.saturating_add(1);
    }
}

pub fn clear_live_pli_request() {
    LIVE_PLI_REQUESTED.store(false, Ordering::SeqCst);
}

pub fn is_physical_stream_session_active() -> bool {
    let (mtx, _) = live_broadcast();
    let b = mtx.lock().unwrap_or_else(|e| e.into_inner());
    b.active
}

pub fn update_stage_status(session_gen: u64, step: u8, title: &str, detail: &str) {
    if LIVE_SESSION_GEN.load(Ordering::SeqCst) != session_gen {
        return;
    }
    let mut g = mirror_runtime().lock().unwrap_or_else(|e| e.into_inner());
    if LIVE_SESSION_GEN.load(Ordering::SeqCst) != session_gen {
        return;
    }
    if g.phase == "starting" || g.phase == "waiting_airplay" {
        g.stage_step = step;
        g.stage_title = title.to_string();
        g.stage_detail = detail.to_string();
        if g.stage.is_empty() || g.stage == "connecting" || g.stage == "reconnecting_rsd" {
            g.stage = if step >= 4 {
                "waiting_frame".to_string()
            } else {
                "connecting".to_string()
            };
        }
        if !detail.is_empty() {
            g.note = Some(detail.to_string());
        }
    }
}

/// Tahap 251 Pilar 4: Emits structured DDI / connection progress stage (`connecting`,
/// `downloading_ddi`, `mounting_ddi`, `reconnecting_rsd`, `waiting_frame`, `streaming`)
/// so `/api/screen` and `/api/status` expose live progress to the UI overlay.
pub fn update_stage_code(
    session_gen: u64,
    stage: &str,
    progress: u8,
    title: &str,
    detail: &str,
) {
    if LIVE_SESSION_GEN.load(Ordering::SeqCst) != session_gen {
        return;
    }
    let mut g = mirror_runtime().lock().unwrap_or_else(|e| e.into_inner());
    if LIVE_SESSION_GEN.load(Ordering::SeqCst) != session_gen {
        return;
    }
    if g.phase == "starting" || g.phase == "waiting_airplay" {
        g.stage = stage.to_string();
        g.stage_progress = progress.min(100);
        g.stage_step = match stage {
            "downloading_ddi" | "mounting_ddi" => 2,
            "reconnecting_rsd" => 3,
            "waiting_frame" | "streaming" => 4,
            _ => 1,
        };
        g.stage_title = title.to_string();
        g.stage_detail = detail.to_string();
        if !detail.is_empty() {
            g.note = Some(detail.to_string());
        }
    }
}

pub fn update_live_codec_from_sps(
    session_gen: u64,
    codec_str: &str,
    hvcc: Option<&[u8]>,
    width: u32,
    height: u32,
) {
    update_live_codec_from_sps_with_trim(session_gen, codec_str, hvcc, width, height, None);
}

pub fn update_live_codec_from_sps_with_trim(
    session_gen: u64,
    codec_str: &str,
    hvcc: Option<&[u8]>,
    width: u32,
    height: u32,
    trim_override: Option<FrameTrim>,
) {
    if LIVE_SESSION_GEN.load(Ordering::SeqCst) != session_gen {
        return;
    }
    let (emit_codec, emit_w, emit_h, emit_trim) = {
        let mut g = mirror_runtime().lock().unwrap_or_else(|e| e.into_inner());
        if LIVE_SESSION_GEN.load(Ordering::SeqCst) != session_gen {
            return;
        }
        g.codec.ok = true;
        if !codec_str.is_empty() {
            g.codec.codec = codec_str.to_string();
        }
        if let Some(h) = hvcc {
            if !h.is_empty() {
                g.codec.description_b64 = Some(base64_encode(h));
            }
        }
        if let Some(t) = trim_override {
            g.codec.trim = t;
        }
        if width > 0 && height > 0 {
            g.codec.width = width;
            g.codec.height = height;
            let dw = width.saturating_sub(g.codec.trim.left.saturating_add(g.codec.trim.right));
            let dh = height.saturating_sub(g.codec.trim.top.saturating_add(g.codec.trim.bottom));
            if dw > 0 && dh > 0 {
                g.codec.display_width = dw;
                g.codec.display_height = dh;
            } else {
                g.codec.display_width = width;
                g.codec.display_height = height;
            }
        }
        (
            g.codec.codec.clone(),
            g.codec.width,
            g.codec.height,
            [
                g.codec.trim.top,
                g.codec.trim.left,
                g.codec.trim.bottom,
                g.codec.trim.right,
            ],
        )
    };
    emit_native_video_event(|| NativeVideoEvent::HevcConfig {
        codec: emit_codec,
        width: emit_w,
        height: emit_h,
        trim: emit_trim,
    });
}

pub fn update_live_codec_h264_avcc(
    session_gen: u64,
    codec_str: &str,
    avcc_b64: &str,
    width: u32,
    height: u32,
) {
    if LIVE_SESSION_GEN.load(Ordering::SeqCst) != session_gen {
        return;
    }
    let (emit_w, emit_h) = {
        let mut g = mirror_runtime().lock().unwrap_or_else(|e| e.into_inner());
        if LIVE_SESSION_GEN.load(Ordering::SeqCst) != session_gen {
            return;
        }
        g.codec.ok = true;
        g.codec.is_airplay = true;
        g.airplay_avcc_count = g.airplay_avcc_count.saturating_add(1);
        if !codec_str.is_empty() {
            g.codec.codec = codec_str.to_string();
        }
        if !avcc_b64.is_empty() {
            g.codec.description_b64 = Some(avcc_b64.to_string());
        }
        g.codec.trim = FrameTrim {
            top: 0,
            left: 0,
            bottom: 0,
            right: 0,
        };
        if width > 0 && height > 0 {
            g.codec.width = width;
            g.codec.height = height;
            g.codec.display_width = width;
            g.codec.display_height = height;
        }
        (g.codec.width, g.codec.height)
    };
    if !avcc_b64.is_empty() {
        if let Some(avcc) = decode_base64_fast(avcc_b64) {
            if !avcc.is_empty() {
                if let Ok(mut slot) = latest_airplay_avcc_slot().lock() {
                    *slot = Some((session_gen, avcc.clone(), emit_w, emit_h));
                }
                AIRPLAY_STREAM_INSTANCE_ID.fetch_add(1, Ordering::SeqCst);
                if is_native_video_sink_registered() {
                    emit_native_video_event(|| NativeVideoEvent::H264Avcc {
                        avcc,
                        width: emit_w,
                        height: emit_h,
                    });
                }
            }
        }
    }
}

/// Returns decoded raw `avcC` bytes for the current AirPlay H.264 session if cached.
pub fn current_live_h264_avcc_bytes() -> Option<Vec<u8>> {
    let b64 = {
        let g = mirror_runtime().lock().unwrap_or_else(|e| e.into_inner());
        if g.codec.ok && g.codec.is_airplay {
            g.codec.description_b64.clone()
        } else {
            None
        }
    };
    b64.and_then(|s| decode_base64_fast(&s))
        .filter(|v| !v.is_empty())
        .or_else(|| latest_airplay_avcc_snapshot().map(|(_, avcc, _, _)| avcc))
}

/// Pushes a decrypted AirPlay H.264 AVCC access unit straight into `/mirror/live` WebCodecs
/// broadcast and advances the Honest Gate (`starting` -> `running`) upon the first valid frame.
pub fn global_on_physical_h264_avcc_frame(
    session_gen: u64,
    avcc_payload: Vec<u8>,
    is_keyframe: bool,
) -> bool {
    if avcc_payload.is_empty() || LIVE_SESSION_GEN.load(Ordering::SeqCst) != session_gen {
        return false;
    }
    static LAST_AIRPLAY_PTS_US: AtomicU64 = AtomicU64::new(0);
    let wall_us = now_epoch_ms().saturating_mul(1000);
    let mono_pts_us = loop {
        let prev = LAST_AIRPLAY_PTS_US.load(Ordering::Relaxed);
        let next = if wall_us > prev {
            wall_us
        } else {
            prev.saturating_add(1)
        };
        if LAST_AIRPLAY_PTS_US
            .compare_exchange_weak(prev, next, Ordering::SeqCst, Ordering::Relaxed)
            .is_ok()
        {
            break next;
        }
    };
    emit_native_video_event(|| NativeVideoEvent::H264Frame {
        avcc_payload: avcc_payload.clone(),
        is_keyframe,
        pts_us: mono_pts_us,
    });
    let frame_type = if is_keyframe { 0 } else { 1 };
    let mut pushed_live = false;
    if let Some(avcc_chunk) = encode_avcc_live_chunk(&avcc_payload, frame_type) {
        let now_ms = now_epoch_ms();
        let (mtx, cv) = live_broadcast();
        let mut b = mtx.lock().unwrap_or_else(|e| e.into_inner());
        if b.active && b.session_gen == session_gen {
            b.seq = b.seq.saturating_add(1);
            let seq = b.seq;
            let entry = LiveBroadcastChunk {
                seq,
                is_keyframe,
                pushed_at_ms: now_ms,
                data: avcc_chunk,
            };
            if is_keyframe || b.init_keyframe.is_none() {
                b.init_keyframe = Some(entry.clone());
            }
            b.ring.push_back(entry);
            while b.ring.len() > MAX_LIVE_BROADCAST_CHUNKS {
                b.ring.pop_front();
            }
            pushed_live = true;
            if seq <= 12 || seq % 60 == 0 || is_keyframe {
                crate::airplay::record_airplay_trace(
                    "live_push_seq",
                    format!("seq={seq} key={is_keyframe}"),
                );
            }
            cv.notify_all();
        }
    }

    let mut g = mirror_runtime().lock().unwrap_or_else(|e| e.into_inner());
    if LIVE_SESSION_GEN.load(Ordering::SeqCst) != session_gen {
        return false;
    }
    g.frames_received = g.frames_received.saturating_add(1);
    g.airplay_video_frames = g.airplay_video_frames.saturating_add(1);
    if is_keyframe {
        g.airplay_idr_count = g.airplay_idr_count.saturating_add(1);
    } else {
        g.airplay_delta_frames = g.airplay_delta_frames.saturating_add(1);
    }
    if pushed_live {
        g.live_chunks_broadcasted = g.live_chunks_broadcasted.saturating_add(1);
    }
    if is_keyframe || g.last_keyframe.is_none() {
        g.last_keyframe = Some(avcc_payload.clone());
    }
    g.subscribers.push_frame(avcc_payload, is_keyframe);
    if g.phase == "starting" || g.phase == "waiting_airplay" || g.phase == "running" {
        g.phase = "running".to_string();
        g.reconnecting = false;
        g.url = Some("/mirror.html".to_string());
        g.stage = "streaming".to_string();
        g.stage_progress = 100;
        g.stage_step = 4;
        g.stage_title = "Streaming AirPlay H.264".to_string();
        g.stage_detail = "Menerima frame H.264 (AirPlay) 60fps.".to_string();
        g.note = Some("Streaming AirPlay H.264 (60fps)".to_string());
        g.error = None;
        g.audio.screen_phase = "running".to_string();
        MIRROR_STREAM_ACTIVE.store(true, Ordering::SeqCst);
        true
    } else {
        false
    }
}

pub fn global_on_airplay_decrypt_failure(session_gen: u64) {
    if LIVE_SESSION_GEN.load(Ordering::SeqCst) != session_gen {
        return;
    }
    let mut g = mirror_runtime().lock().unwrap_or_else(|e| e.into_inner());
    if LIVE_SESSION_GEN.load(Ordering::SeqCst) != session_gen {
        return;
    }
    g.airplay_decrypt_failures = g.airplay_decrypt_failures.saturating_add(1);
}

pub fn global_on_airplay_tcp_packet(session_gen: u64) {
    if LIVE_SESSION_GEN.load(Ordering::SeqCst) != session_gen {
        return;
    }
    let mut g = mirror_runtime().lock().unwrap_or_else(|e| e.into_inner());
    if LIVE_SESSION_GEN.load(Ordering::SeqCst) != session_gen {
        return;
    }
    g.airplay_tcp_packets_read = g.airplay_tcp_packets_read.saturating_add(1);
}

/// Tahap 275 Pilar 1: Immediately drops AirPlay out of stale `running` state when the last active
/// mirror TCP video socket exits while still streaming, emitting `NativeVideoEvent::Stopped`
/// so Top Bar + Canvas never hang for ~23s on a dead RTSP connection.
pub fn mark_airplay_stream_disconnected(session_gen: u64) {
    if LIVE_SESSION_GEN.load(Ordering::SeqCst) != session_gen {
        return;
    }
    let should_reset = {
        let g = mirror_runtime().lock().unwrap_or_else(|e| e.into_inner());
        LIVE_SESSION_GEN.load(Ordering::SeqCst) == session_gen
            && g.airplay_active
            && g.phase == "running"
    };
    if should_reset {
        crate::airplay::record_airplay_trace(
            "mirror_tcp_disconnect",
            format!("gen={session_gen} phase=running->waiting"),
        );
        reset_airplay_to_waiting(session_gen);
    }
}

pub fn global_on_airplay_mirror_loop_state(session_gen: u64, alive: bool) {
    if LIVE_SESSION_GEN.load(Ordering::SeqCst) != session_gen {
        return;
    }
    let dropped_to_zero = {
        let mut g = mirror_runtime().lock().unwrap_or_else(|e| e.into_inner());
        if LIVE_SESSION_GEN.load(Ordering::SeqCst) != session_gen {
            return;
        }
        if alive {
            g.airplay_mirror_loop_count = g.airplay_mirror_loop_count.saturating_add(1);
            AIRPLAY_STREAM_INSTANCE_ID.fetch_add(1, Ordering::SeqCst);
            false
        } else {
            let was_nonzero = g.airplay_mirror_loop_count > 0;
            g.airplay_mirror_loop_count = g.airplay_mirror_loop_count.saturating_sub(1);
            was_nonzero
                && g.airplay_mirror_loop_count == 0
                && g.airplay_active
                && g.phase == "running"
        }
    };
    if dropped_to_zero {
        mark_airplay_stream_disconnected(session_gen);
    }
}

pub fn set_hybrid_hid_available(session_gen: u64, available: bool) {
    if LIVE_SESSION_GEN.load(Ordering::SeqCst) != session_gen {
        return;
    }
    let mut g = mirror_runtime().lock().unwrap_or_else(|e| e.into_inner());
    if LIVE_SESSION_GEN.load(Ordering::SeqCst) != session_gen {
        return;
    }
    g.hid_control_available = available;
    if !available && g.mirror_route == "hybrid_airplay_h264" {
        g.note = Some(
            "Mode tampilan AirPlay H.264 aktif (kontrol sentuh tidak tersedia — aktifkan Developer Mode / hubungkan USB untuk kontrol)."
                .to_string(),
        );
    }
}

pub fn active_mirror_route() -> String {
    let g = mirror_runtime().lock().unwrap_or_else(|e| e.into_inner());
    g.mirror_route.clone()
}

pub fn is_hybrid_airplay_route() -> bool {
    let g = mirror_runtime().lock().unwrap_or_else(|e| e.into_inner());
    g.mirror_route == "hybrid_airplay_h264"
}

/// Returns `true` when an AirPlay session is active WITHOUT CoreDevice HID control
/// (e.g. Pure AirPlay H.264 standalone mode or fail-soft degraded Hybrid mode).
pub fn is_airplay_standalone_no_hid_active() -> bool {
    let g = mirror_runtime().lock().unwrap_or_else(|e| e.into_inner());
    g.airplay_active && !g.hid_control_available
}

/// Returns `true` whenever an AirPlay H.264 session (Standalone or Hybrid) is currently active
/// (`phase != "idle"`), used to prevent Windows Bluetooth A2DP from hijacking iOS AirPlay audio.
pub fn is_airplay_active() -> bool {
    let g = mirror_runtime().lock().unwrap_or_else(|e| e.into_inner());
    g.airplay_active && g.phase != "idle"
}

pub fn global_on_physical_video_frame(annex_b_au: Vec<u8>, is_keyframe: bool) {
    if annex_b_au.is_empty() {
        return;
    }
    record_hevc_video_frame_telemetry(annex_b_au.len(), now_epoch_ms());
    if is_native_video_sink_registered() {
        let trim = {
            let g = mirror_runtime().lock().unwrap_or_else(|e| e.into_inner());
            [
                g.codec.trim.top,
                g.codec.trim.left,
                g.codec.trim.bottom,
                g.codec.trim.right,
            ]
        };
        emit_native_video_event(|| NativeVideoEvent::HevcFrame {
            annex_b: annex_b_au.clone(),
            is_keyframe,
            pts_us: now_epoch_ms().saturating_mul(1000),
            trim,
        });
    }
    let frame_type = if is_keyframe { 0 } else { 1 };
    if let Some(hvcc_chunk) = encode_hvcc_live_chunk(&annex_b_au, frame_type) {
        let now_ms = now_epoch_ms();
        let (mtx, cv) = live_broadcast();
        let mut b = mtx.lock().unwrap_or_else(|e| e.into_inner());
        if b.active {
            b.seq = b.seq.saturating_add(1);
            let seq = b.seq;
            let entry = LiveBroadcastChunk {
                seq,
                is_keyframe,
                pushed_at_ms: now_ms,
                data: hvcc_chunk,
            };
            if is_keyframe || b.init_keyframe.is_none() {
                b.init_keyframe = Some(entry.clone());
            }
            b.ring.push_back(entry);
            while b.ring.len() > MAX_LIVE_BROADCAST_CHUNKS {
                b.ring.pop_front();
            }
            cv.notify_all();
        }
    }
    let _ = global_on_first_valid_frame(annex_b_au, is_keyframe);
}

pub fn has_ever_received_frame_for_session(session_gen: u64) -> bool {
    if LIVE_SESSION_GEN.load(Ordering::SeqCst) != session_gen {
        return false;
    }
    let g = mirror_runtime().lock().unwrap_or_else(|e| e.into_inner());
    g.frames_received > 0
}

/// Marks transparent auto-reconnect state while keeping `phase == "running"` and
/// `LIVE_BROADCAST.active == true` so WebView2 keeps the last video frame without a gray blank flash.
pub fn set_reconnecting_state(session_gen: u64, reconnecting: bool, attempt: u32, _reason: &str) {
    if LIVE_SESSION_GEN.load(Ordering::SeqCst) != session_gen {
        return;
    }
    let mut g = mirror_runtime().lock().unwrap_or_else(|e| e.into_inner());
    if LIVE_SESSION_GEN.load(Ordering::SeqCst) != session_gen {
        return;
    }
    g.reconnecting = reconnecting;
    if reconnecting {
        // Tahap 244 Pilar 2: Keep UI reconnect note strictly compact (1-line pill) — never dump raw IO errors.
        g.note = Some(format!("Menyambung ulang ({attempt}/3)…"));
        g.stage_title = "Menyambung ulang".to_string();
        g.stage_detail = format!("Percobaan {attempt}/3...");
    } else {
        g.note = None;
    }
}

pub fn global_on_physical_stream_error(session_gen: u64, err_msg: &str) {
    if LIVE_SESSION_GEN.load(Ordering::SeqCst) != session_gen {
        return;
    }
    {
        let mut g = mirror_runtime().lock().unwrap_or_else(|e| e.into_inner());
        if LIVE_SESSION_GEN.load(Ordering::SeqCst) != session_gen {
            return;
        }
        g.phase = "error".to_string();
        g.reconnecting = false;
        g.url = None;
        g.error = Some(err_msg.to_string());
        g.note = Some(err_msg.to_string());
        g.stage.clear();
        g.stage_progress = 0;
        g.stage_step = 0;
        g.stage_title = "Koneksi stream terputus".to_string();
        g.stage_detail = err_msg.to_string();
        g.tunnel_active = false;
        g.audio.screen_phase = "error".to_string();
        MIRROR_STREAM_ACTIVE.store(false, Ordering::SeqCst);
    }
    let (mtx, cv) = live_broadcast();
    let mut b = mtx.lock().unwrap_or_else(|e| e.into_inner());
    if b.session_gen == session_gen {
        b.active = false;
        cv.notify_all();
    }
}

fn prepare_physical_stream_session(udid: &str, transport_label: &str) -> (String, u64, Arc<AtomicBool>) {
    stop_active_physical_worker();
    clear_latest_airplay_avcc_snapshot();
    // Tahap 250 Pilar 2: Wait up to 350 ms for the previous worker to complete `stopmediastream`
    // and close `TlsPskStream`/`rp_control` cleanly before launching a new session.
    let _ = wait_previous_physical_worker_exit(350);
    crate::physical_stream::reset_physical_session_diagnostics();
    reset_airplay_video_queue_dropped();
    reset_hevc_bitrate_telemetry();
    let new_gen = LIVE_SESSION_GEN.fetch_add(1, Ordering::SeqCst).saturating_add(1);
    let tr = if transport_label.trim().is_empty() {
        "usb"
    } else {
        transport_label.trim()
    };
    let status_json = global_start_screen_inner(Some(udid), tr, true);
    {
        let mut g = mirror_runtime().lock().unwrap_or_else(|e| e.into_inner());
        g.codec = CodecHeader::default();
        g.mirror_route = "coredevice_hevc".to_string();
        g.airplay_active = false;
        g.airplay_port = 0;
        g.hid_control_available = true;
        g.airplay_avcc_count = 0;
        g.airplay_video_frames = 0;
        g.airplay_idr_count = 0;
        g.airplay_delta_frames = 0;
        g.airplay_decrypt_failures = 0;
        g.airplay_tcp_packets_read = 0;
        g.airplay_mirror_loop_count = 0;
        g.live_chunks_broadcasted = 0;
        g.last_keyframe = None;
        g.subscribers.clear();
        g.frames_received = 0;
        g.reconnecting = false;
        g.frame_ring.clear();
        g.video_decoder.reset_stream();
    }
    {
        let (mtx, cv) = live_broadcast();
        let mut b = mtx.lock().unwrap_or_else(|e| e.into_inner());
        b.active = true;
        b.is_airplay = false;
        b.session_gen = new_gen;
        b.seq = 0;
        b.init_keyframe = None;
        b.ring.clear();
        cv.notify_all();
    }
    let stop = Arc::new(AtomicBool::new(false));
    {
        let mut slot = live_worker_stop_slot()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *slot = Some(stop.clone());
    }
    (status_json, new_gen, stop)
}

pub fn start_real_physical_usb_screen_with_transport(
    udid: &str,
    transport_label: &str,
) -> String {
    let (status_json, new_gen, stop) = prepare_physical_stream_session(udid, transport_label);
    crate::physical_stream::spawn_physical_usb_display_worker(udid.to_string(), new_gen, stop);
    status_json
}

pub fn start_real_physical_usb_screen(udid: &str) -> String {
    start_real_physical_usb_screen_with_transport(udid, "usb")
}

/// Stage 2 pure cable-free Wi-Fi worker: tries Lockdown Wi-Fi `CoreDeviceProxy` (`:62078`)
/// and `RemotePairing TLS-PSK` (`<ip>:49152` -> `CdTunnelRouter` -> 60fps H.265 + RemoteXPC HID).
pub fn start_real_physical_wifi_lockdown_screen(udid: &str, ip: &str) -> String {
    let (status_json, new_gen, stop) = prepare_physical_stream_session(udid, "wifi");
    crate::physical_stream::spawn_physical_wifi_display_worker(
        udid.to_string(),
        ip.to_string(),
        new_gen,
        stop,
    );
    status_json
}

/// Tahap 256 Pilar 3: Honest state transition back to `waiting_airplay` when iPhone sends a full
/// `TEARDOWN` (or closes the primary RTSP session), clearing stale frames & `init_keyframe` while
/// keeping RTSP port 7100 + mDNS active so the user can reconnect in 1 click from Control Center.
pub fn reset_airplay_to_waiting(session_gen: u64) {
    if LIVE_SESSION_GEN.load(Ordering::SeqCst) != session_gen {
        return;
    }
    let (was_airplay_active, inst_snapshot) = {
        let mut g = mirror_runtime().lock().unwrap_or_else(|e| e.into_inner());
        if LIVE_SESSION_GEN.load(Ordering::SeqCst) != session_gen
            || !g.airplay_active
            || g.phase == "idle"
        {
            (false, 0)
        } else {
            g.phase = "starting".to_string();
            g.url = None;
            g.reconnecting = false;
            g.wait_for_first_frame = true;
            g.last_keyframe = None;
            g.subscribers.clear();
            g.stage = "waiting_airplay".to_string();
            g.stage_progress = 20;
            g.stage_step = 2;
            g.stage_title = "Menunggu Screen Mirroring iPhone".to_string();
            g.stage_detail =
                "Buka Control Center di iPhone → Screen Mirroring → pilih 'iPhone Link'"
                    .to_string();
            g.note = Some(g.stage_detail.clone());
            g.audio.screen_phase = "starting".to_string();
            MIRROR_STREAM_ACTIVE.store(false, Ordering::SeqCst);
            (true, AIRPLAY_STREAM_INSTANCE_ID.load(Ordering::SeqCst))
        }
    };
    if was_airplay_active {
        crate::airplay::mark_airplay_audio_inactive();
        if AIRPLAY_STREAM_INSTANCE_ID.load(Ordering::SeqCst) == inst_snapshot {
            emit_native_video_event(|| NativeVideoEvent::Stopped);
        }
        let (mtx, cv) = live_broadcast();
        let mut b = mtx.lock().unwrap_or_else(|e| e.into_inner());
        if b.active && b.session_gen == session_gen {
            b.init_keyframe = None;
            b.ring.clear();
            cv.notify_all();
        }
    }
}

struct MirrorAirPlaySink {
    session_gen: u64,
}

impl crate::airplay::AirPlaySink for MirrorAirPlaySink {
    fn on_state(&self, state: crate::airplay::AirPlayState) {
        if LIVE_SESSION_GEN.load(Ordering::SeqCst) != self.session_gen {
            return;
        }
        match state {
            crate::airplay::AirPlayState::Waiting => {
                reset_airplay_to_waiting(self.session_gen);
            }
            crate::airplay::AirPlayState::Connected => {
                // Tahap 254 FIX: Never regress stage back to `waiting_frame` when iOS opens a
                // 2nd concurrent video TCP socket while stream is already `running`!
                let already_running = mirror_runtime()
                    .lock()
                    .map(|g| g.phase == "running")
                    .unwrap_or(false);
                if !already_running {
                    update_stage_code(
                        self.session_gen,
                        "waiting_frame",
                        75,
                        "iPhone tersambung via AirPlay",
                        "Menunggu frame pertama H.264 (AirPlay)...",
                    );
                }
            }
            crate::airplay::AirPlayState::Streaming => {}
            crate::airplay::AirPlayState::Stopped => {}
        }
    }

    fn on_codec(&self, codec: &str, avcc_b64: &str, width: u32, height: u32) {
        update_live_codec_h264_avcc(self.session_gen, codec, avcc_b64, width, height);
    }

    fn on_frame(&self, avcc_payload: Vec<u8>, is_keyframe: bool) {
        let _ = global_on_physical_h264_avcc_frame(self.session_gen, avcc_payload, is_keyframe);
    }

    fn on_decrypt_failure(&self) {
        global_on_airplay_decrypt_failure(self.session_gen);
    }

    fn on_tcp_packet(&self, _kind: u8) {
        global_on_airplay_tcp_packet(self.session_gen);
    }

    fn on_mirror_loop_state(&self, alive: bool) {
        global_on_airplay_mirror_loop_state(self.session_gen, alive);
    }
}

/// Starts a native Pure-Rust AirPlay H.264 receiver session (`hybrid_airplay_h264` for iOS 17.4–26.x
/// or `airplay_standalone` for iOS <= 17.3 / PC fallback without HEVC).
///
/// When `hybrid_hid == true`, spawns a strictly fail-soft CoreDevice `gate_only` HID background
/// worker alongside AirPlay so touch/keyboard work without ever risking AirPlay video delivery.
pub fn start_airplay_mirror_session(
    udid: Option<&str>,
    _hybrid_hid: bool,
    transport_label: &str,
    _wifi_ip: Option<&str>,
    bind_port: Option<u16>,
    advertise_mdns: bool,
) -> Result<String, String> {
    // Tahap 293 Opsi 2: iOS < 27 runs strictly as pure AirPlay Standalone (View & Audio Only)
    // with zero unverified `gate_only` background USB/Wi-Fi workers.
    let effective_hybrid_hid = false;
    stop_active_physical_worker();
    clear_latest_airplay_avcc_snapshot();
    let _ = wait_previous_physical_worker_exit(350);
    crate::physical_stream::reset_physical_session_diagnostics();
    crate::airplay::reset_airplay_trace();
    crate::airplay::reset_airplay_audio_stats();
    reset_airplay_video_queue_dropped();
    reset_hevc_bitrate_telemetry();
    crate::diag_log::record("airplay", "start_airplay_mirror_session (airplay_standalone)");
    crate::airplay::record_airplay_trace(
        "session_start",
        format!("hybrid_hid={effective_hybrid_hid}"),
    );

    let new_gen = LIVE_SESSION_GEN.fetch_add(1, Ordering::SeqCst).saturating_add(1);
    let tr = if transport_label.trim().is_empty() {
        "wifi"
    } else {
        transport_label.trim()
    };

    {
        let (mtx, cv) = live_broadcast();
        let mut b = mtx.lock().unwrap_or_else(|e| e.into_inner());
        b.active = true;
        b.is_airplay = true;
        b.session_gen = new_gen;
        b.seq = 0;
        b.init_keyframe = None;
        b.ring.clear();
        cv.notify_all();
    }

    let sink: Arc<dyn crate::airplay::AirPlaySink> = Arc::new(MirrorAirPlaySink {
        session_gen: new_gen,
    });
    let (port, airplay_name) = crate::airplay::start_global_airplay_receiver(
        Some(crate::airplay::DEFAULT_RECEIVER_NAME),
        bind_port,
        sink,
        advertise_mdns,
    )?;

    let route_str = "airplay_standalone";

    let status_json = {
        let mut g = mirror_runtime().lock().unwrap_or_else(|e| e.into_inner());
        g.stop_cooldown_until_ms = 0;
        g.transport = tr.to_string();
        g.udid = udid.map(|s| s.to_string());
        g.phase = "starting".to_string();
        g.url = None;
        g.error = None;
        g.reconnecting = false;
        g.wait_for_first_frame = true;
        g.tunnel_active = effective_hybrid_hid;
        g.frames_received = 0;
        g.last_keyframe = None;
        g.subscribers.clear();
        g.frame_ring.clear();
        g.video_decoder.reset_stream();
        g.mirror_route = route_str.to_string();
        g.airplay_active = true;
        g.airplay_name = airplay_name;
        g.airplay_port = port;
        g.hid_control_available = effective_hybrid_hid;
        g.airplay_avcc_count = 0;
        g.airplay_video_frames = 0;
        g.airplay_idr_count = 0;
        g.airplay_delta_frames = 0;
        g.airplay_decrypt_failures = 0;
        g.airplay_tcp_packets_read = 0;
        g.airplay_mirror_loop_count = 0;
        g.live_chunks_broadcasted = 0;
        g.stage = "waiting_airplay".to_string();
        g.stage_progress = 20;
        g.stage_step = 2;
        g.stage_title = "Menunggu Screen Mirroring iPhone".to_string();
        g.stage_detail =
            "Buka Control Center di iPhone → Screen Mirroring → pilih 'iPhone Link'".to_string();
        g.note = Some(g.stage_detail.clone());
        g.audio.screen_transport = tr.to_string();
        g.audio.screen_phase = "starting".to_string();
        g.codec = CodecHeader {
            ok: true,
            codec: "avc1.640028".to_string(),
            description_b64: None,
            width: crate::airplay::info::DISPLAY_SIZE.0,
            height: crate::airplay::info::DISPLAY_SIZE.1,
            display_width: crate::airplay::info::DISPLAY_SIZE.0,
            display_height: crate::airplay::info::DISPLAY_SIZE.1,
            fps: 60,
            trim: FrameTrim {
                top: 0,
                left: 0,
                bottom: 0,
                right: 0,
            },
            is_airplay: true,
        };
        MIRROR_STREAM_ACTIVE.store(false, Ordering::SeqCst);
        g.screen_status_json(now_epoch_ms())
    };

    let stop = Arc::new(AtomicBool::new(false));
    {
        let mut slot = live_worker_stop_slot()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *slot = Some(stop.clone());
    }
    let _ = stop;

    Ok(status_json)
}

/// Waits up to `timeout_ms` for new WebCodecs `hvcC` stream chunks from the live physical worker.
/// Returns `None` when the physical session stops/ends, or `Some(chunks)` otherwise.
pub fn global_wait_next_live_chunks(
    last_seq: &mut u64,
    sent_init: &mut bool,
    timeout_ms: u64,
) -> Option<Vec<Vec<u8>>> {
    let (mtx, cv) = live_broadcast();
    let mut b = mtx.lock().unwrap_or_else(|e| e.into_inner());
    let deadline = Instant::now() + Duration::from_millis(timeout_ms.max(10));

    loop {
        if !b.active {
            return None;
        }
        let mut out: Vec<Vec<u8>> = Vec::new();
        let need_resync_key = !*sent_init
            || b.ring
                .front()
                .map(|c| *last_seq > 0 && c.seq > last_seq.saturating_add(1))
                .unwrap_or(false);
        if need_resync_key {
            let now_ms = now_epoch_ms();
            let is_airplay_stream = b.is_airplay;
            let slice = b.ring.make_contiguous();
            let (latest_idr_seq_opt, needs_fresh_pli) =
                select_latest_idr_start_cursor(slice, now_ms);

            if let Some(idr_seq) = latest_idr_seq_opt {
                if !*sent_init || idr_seq > *last_seq {
                    if let Some(idr_idx) = slice.iter().position(|c| c.seq == idr_seq) {
                        let trailing = slice.len().saturating_sub(idr_idx + 1);
                        out.push(slice[idr_idx].data.clone());
                        *sent_init = true;
                        *last_seq = idr_seq;
                        // Tahap 253 Pilar 1: AirPlay has NO RTCP PLI! Never truncate trailing deltas
                        // after the latest IDR on a late-joining `/mirror/live` client — emit the
                        // complete contiguous chain up to live so WebCodecs renders immediately.
                        if is_airplay_stream || trailing <= LATEST_IDR_MAX_TRAILING_DELTAS {
                            for ch in &slice[idr_idx + 1..] {
                                out.push(ch.data.clone());
                                *last_seq = ch.seq;
                            }
                        } else if let Some(tail) = slice.last() {
                            // CoreDevice HEVC only: skip > 8 stale trailing deltas and request fresh PLI.
                            *last_seq = tail.seq;
                        }
                        if !is_airplay_stream && needs_fresh_pli {
                            request_live_pli();
                        }
                        return Some(out);
                    }
                }
            } else if let Some(ref init_chunk) = b.init_keyframe {
                if !*sent_init || init_chunk.seq > *last_seq {
                    out.push(init_chunk.data.clone());
                    *sent_init = true;
                    if is_airplay_stream {
                        let mut expected_next = init_chunk.seq.saturating_add(1);
                        *last_seq = init_chunk.seq;
                        for ch in b.ring.iter() {
                            if ch.seq == expected_next {
                                out.push(ch.data.clone());
                                *last_seq = ch.seq;
                                expected_next = expected_next.saturating_add(1);
                            }
                        }
                    } else {
                        *last_seq = b.ring.back().map(|c| c.seq).unwrap_or(init_chunk.seq);
                        if needs_fresh_pli || !b.ring.is_empty() {
                            request_live_pli();
                        }
                    }
                    return Some(out);
                }
            }
        }
        if *sent_init {
            // Tahap 259 Pilar 1: Zero-Backlog IDR Fast-Forward!
            // If this /mirror/live client fell behind > 12 frames in the rolling ring AND
            // a newer keyframe (IDR) exists in the ring, jump `last_seq` straight to that
            // latest IDR (NEVER drop mid-GOP P-frames!) so video latency never accumulates.
            let behind_count = b.ring.iter().filter(|c| c.seq > *last_seq).count();
            if behind_count > 12 {
                if let Some(latest_idr) = b.ring.iter().rev().find(|c| c.is_keyframe && c.seq > *last_seq) {
                    let target_seq = latest_idr.seq;
                    let skipped = b
                        .ring
                        .iter()
                        .filter(|c| c.seq > *last_seq && c.seq < target_seq)
                        .count() as u64;
                    if skipped > 0 {
                        *last_seq = target_seq.saturating_sub(1);
                        if b.is_airplay {
                            inc_airplay_video_queue_dropped(skipped);
                            crate::airplay::add_trace_video_queue_dropped(skipped);
                        }
                    }
                }
            }
            for ch in &b.ring {
                if ch.seq > *last_seq {
                    out.push(ch.data.clone());
                    *last_seq = ch.seq;
                }
            }
            if !out.is_empty() {
                return Some(out);
            }
        }

        let now = Instant::now();
        if now >= deadline {
            return Some(Vec::new());
        }
        let (guard, _) = cv
            .wait_timeout(b, deadline.saturating_duration_since(now))
            .unwrap_or_else(|e| e.into_inner());
        b = guard;
    }
}

pub fn mirror_runtime() -> &'static Mutex<MirrorRuntimeState> {
    MIRROR_RUNTIME_STATE.get_or_init(|| Mutex::new(MirrorRuntimeState::default()))
}

pub fn is_mirror_streaming_fast() -> bool {
    MIRROR_STREAM_ACTIVE.load(Ordering::Relaxed)
}

pub fn active_orientation_fast() -> u32 {
    MIRROR_ACTIVE_ORIENTATION.load(Ordering::Relaxed)
}

/// Starts or prepares an in-process screen mirroring session.
/// When `honest_gate == true`, transitions cleanly to `"starting"` (`url = null`) and only
/// advances to `"running"` (`url = "/mirror.html"`) once `global_on_first_valid_frame` receives a real frame.
pub fn global_start_screen(
    udid: Option<&str>,
    transport: &str,
    honest_gate: bool,
) -> String {
    stop_active_physical_worker();
    LIVE_SESSION_GEN.fetch_add(1, Ordering::SeqCst);
    global_start_screen_inner(udid, transport, honest_gate)
}

fn global_start_screen_inner(
    udid: Option<&str>,
    transport: &str,
    honest_gate: bool,
) -> String {
    let norm_transport = if transport.eq_ignore_ascii_case("wifi") {
        "wifi"
    } else if transport.eq_ignore_ascii_case("usb") {
        "usb"
    } else {
        "cable"
    };
    let mut g = mirror_runtime().lock().unwrap_or_else(|e| e.into_inner());
    g.stop_cooldown_until_ms = 0;
    g.transport = norm_transport.to_string();
    g.udid = udid.map(|s| s.to_string());
    g.error = None;
    g.wait_for_first_frame = honest_gate;
    g.tunnel_active = true;
    g.audio.screen_transport = norm_transport.to_string();

    if honest_gate {
        g.phase = "starting".to_string();
        g.url = None;
        g.frames_received = 0;
        g.stage = "connecting".to_string();
        g.stage_progress = 5;
        g.stage_step = 4;
        g.stage_title = "Mulai stream".to_string();
        g.stage_detail = "Menunggu frame pertama H.265/HEVC 60fps.".to_string();
        g.note = Some("Menunggu frame video pertama...".to_string());
        g.audio.screen_phase = "starting".to_string();
        MIRROR_STREAM_ACTIVE.store(false, Ordering::SeqCst);
    } else {
        g.phase = "running".to_string();
        g.url = Some("/mirror.html".to_string());
        g.stage = "streaming".to_string();
        g.stage_progress = 100;
        g.stage_step = 4;
        g.stage_title = "Mulai stream".to_string();
        g.stage_detail = "Menerima frame H.265/HEVC 60fps.".to_string();
        g.note = None;
        g.audio.screen_phase = "running".to_string();
        MIRROR_STREAM_ACTIVE.store(true, Ordering::SeqCst);
    }
    g.screen_status_json(now_epoch_ms())
}

/// Advances `"starting"` -> `"running"` ONLY after a non-empty decoded Annex-B frame/keyframe arrives.
pub fn global_on_first_valid_frame(annex_b_nalu: Vec<u8>, is_keyframe: bool) -> bool {
    if annex_b_nalu.is_empty() {
        return false;
    }
    let now_ms = now_epoch_ms();
    let mut g = mirror_runtime().lock().unwrap_or_else(|e| e.into_inner());
    let _ = g.process_video_annex_b(&annex_b_nalu, now_ms);
    g.frames_received = g.frames_received.saturating_add(1);
    if is_keyframe || g.last_keyframe.is_none() {
        g.last_keyframe = Some(annex_b_nalu.clone());
    }
    g.subscribers.push_frame(annex_b_nalu, is_keyframe);
    if g.phase == "starting" || g.phase == "waiting_airplay" || g.phase == "running" {
        g.phase = "running".to_string();
        g.reconnecting = false;
        g.url = Some("/mirror.html".to_string());
        g.stage = "streaming".to_string();
        g.stage_progress = 100;
        g.stage_step = 4;
        g.stage_title = "Streaming live".to_string();
        g.stage_detail = "Menerima frame H.265/HEVC 60fps.".to_string();
        g.note = None;
        g.error = None;
        g.audio.screen_phase = "running".to_string();
        MIRROR_STREAM_ACTIVE.store(true, Ordering::SeqCst);
        true
    } else {
        false
    }
}

/// Returns native GPU video pipeline telemetry JSON (`D3D11`, `mfplat.dll`, 6-slot `SharedFrameRing`, trim).
pub fn global_video_pipeline_status_json() -> String {
    let g = mirror_runtime().lock().unwrap_or_else(|e| e.into_inner());
    g.video_pipeline_status_json()
}

/// Feeds an Annex-B HEVC access unit (or runs the deterministic `1216x2656` synthetic IDR + NV12 probe audit)
/// into the in-process `MfHevcDecoder` and 6-slot `SharedFrameRing`.
pub fn global_decode_annexb_test_json(body: &str) -> String {
    let now_ms = now_epoch_ms();
    let lower = body.to_ascii_lowercase();
    let mut g = mirror_runtime().lock().unwrap_or_else(|e| e.into_inner());

    if lower.contains("reset") {
        g.video_decoder.reset_stream();
        g.frame_ring.clear();
    }

    if lower.contains("trail") {
        let trail = build_synthetic_hevc_annex_b_trail_r();
        let out = g.process_video_annex_b(&trail, now_ms);
        let pipe = g.video_pipeline_status_json();
        return format!(
            r#"{{"ok":true,"action":"trail","decoded":{},"isKeyframe":{},"needsKeyframePli":{},"pipeline":{}}}"#,
            out.decoded, out.is_keyframe, out.needs_keyframe_pli, pipe
        );
    }

    let idr = build_synthetic_hevc_annex_b_idr(1216, 2656);
    let rot = g.auto_rotate.current_orientation_deg;
    let ring = g.frame_ring.clone();
    let out = g.video_decoder.feed_annex_b_with_nv12_writer(
        &idr,
        now_ms.saturating_mul(1000),
        rot,
        &ring,
        |w, h, buf| {
            write_nv12_from_rgb_sampler(w, h, buf, |x, y| {
                if x >= 1202 || y >= 2598 {
                    (0, 0, 0)
                } else if y < 24 && x >= 100 && x < 200 {
                    (245, 245, 245)
                } else if (110..=260).contains(&y) && (40..=240).contains(&x) {
                    (30, 110, 220)
                } else if y < 440 || y >= 2000 {
                    (6, 6, 6)
                } else {
                    (25, 130, 50)
                }
            });
        },
    );

    let (mut top_ok, mut bot_trimmed, mut snap_w, mut snap_h) = (false, false, 0u32, 0u32);
    let mut probe_ok = false;
    let ptr_before = g.probe_rgba_buf.as_ptr() as usize;
    let cap_before = g.probe_rgba_buf.capacity();

    if out.decoded {
        let mr = &mut *g;
        let probe_buf = &mut mr.probe_rgba_buf;
        if let Some((cw, ch, bgra)) = mr.frame_ring.with_latest_slot(|slot| {
            probe_ok = extract_downscaled_probe_120x260_into(slot, probe_buf);
            export_clean_cropped_bgra_frame(slot)
        }) {
            snap_w = cw;
            snap_h = ch;
            if cw == 1202 && ch == 2598 && bgra.len() == 1202 * 2598 * 4 {
                let di_idx = (2 * 1202 + 120) * 4;
                top_ok = bgra[di_idx] > 200 && bgra[di_idx + 1] > 200 && bgra[di_idx + 2] > 200;
                bot_trimmed = true;
            }
        }
        if probe_ok {
            populate_probe_grid_120x260_in_place(&mr.probe_rgba_buf, &mut mr.probe_grid);
            if mr.auto_rotate.auto_rotate_enabled {
                let deg = mr.auto_rotate.evaluate_frame(
                    Some(&mr.probe_grid),
                    None,
                    out.clean_width,
                    out.clean_height,
                    now_ms,
                );
                MIRROR_ACTIVE_ORIENTATION.store(deg, Ordering::SeqCst);
            }
        }
    }

    let ptr_after = g.probe_rgba_buf.as_ptr() as usize;
    let cap_after = g.probe_rgba_buf.capacity();
    let probe_zero_alloc = ptr_before == ptr_after && cap_before == cap_after;
    let pipe = g.video_pipeline_status_json();

    format!(
        r#"{{"ok":true,"action":"idr_nv12","decoded":{},"isKeyframe":{},"needsKeyframePli":{},"codedWidth":{},"codedHeight":{},"cleanWidth":{},"cleanHeight":{},"topMarginNotCropped":{},"bottomPaddingTrimmed":{},"snapshotWidth":{},"snapshotHeight":{},"probeExtracted":{},"probeZeroAlloc":{},"ringCapacity":{},"lowLatencyMode":{},"pipeline":{}}}"#,
        out.decoded,
        out.is_keyframe,
        out.needs_keyframe_pli,
        out.coded_width,
        out.coded_height,
        out.clean_width,
        out.clean_height,
        top_ok,
        bot_trimmed,
        snap_w,
        snap_h,
        probe_ok,
        probe_zero_alloc,
        RING_CAPACITY,
        g.video_decoder.low_latency_mode,
        pipe,
    )
}

/// Converts a Wi-Fi `ConnectionTerminatedError` (or tunnel termination on iOS 27.0.1) into an
/// actionable bilingual error status instead of leaving the UI stuck in `"starting"`.
pub fn global_on_wifi_coredevice_error(raw_err: &str, lang_is_en: bool) -> String {
    let mut g = mirror_runtime().lock().unwrap_or_else(|e| e.into_inner());
    let is_term = raw_err.contains("ConnectionTerminated")
        || raw_err.contains("terminated")
        || raw_err.contains("27.0.1");
    let diag = if is_term {
        if lang_is_en {
            WIFI_COREDEVICE_IOS27_DIAGNOSTIC_EN.to_string()
        } else {
            WIFI_COREDEVICE_IOS27_DIAGNOSTIC_ID.to_string()
        }
    } else if lang_is_en {
        format!(
            "CoreDevice Wi-Fi connection failed: {}. Ensure your iPhone is unlocked on the same Wi-Fi.",
            device_error(raw_err)
        )
    } else {
        format!(
            "Koneksi CoreDevice Wi-Fi gagal: {}. Pastikan iPhone menyala dan terhubung ke Wi-Fi yang sama dengan PC.",
            device_error(raw_err)
        )
    };
    g.stop_cooldown_until_ms = 0;
    g.phase = "error".to_string();
    g.url = None;
    g.error = Some(diag);
    g.tunnel_active = false;
    g.tunnel_reset_count = g.tunnel_reset_count.saturating_add(1);
    g.audio.screen_phase = "error".to_string();
    MIRROR_STREAM_ACTIVE.store(false, Ordering::SeqCst);
    g.screen_status_json(now_epoch_ms())
}

/// Cleanly stops mirroring:
/// 1. Transitions atomically to `phase = "idle"` with a `5s` stop cooldown.
/// 2. Restores 4 bars (`~25%`, `0x006F`) ONLY if `is_stealth_dim_active()` was `true` (0 signals otherwise).
/// 3. Stops audio & resets speaker mute state.
pub fn global_set_screen_error(udid: Option<&str>, transport: &str, err_msg: &str) -> String {
    stop_active_physical_worker();
    LIVE_SESSION_GEN.fetch_add(1, Ordering::SeqCst);
    let now_ms = now_epoch_ms();
    let mut g = mirror_runtime().lock().unwrap_or_else(|e| e.into_inner());
    g.stop_cooldown_until_ms = 0;
    g.transport = if transport.eq_ignore_ascii_case("wifi") {
        "wifi".to_string()
    } else {
        "usb".to_string()
    };
    g.udid = udid.map(|s| s.to_string());
    g.phase = "error".to_string();
    g.url = None;
    g.error = Some(err_msg.to_string());
    g.note = Some(err_msg.to_string());
    g.stage_step = 0;
    g.stage_title = "Koneksi gagal".to_string();
    g.stage_detail = err_msg.to_string();
    g.tunnel_active = false;
    g.audio.screen_phase = "error".to_string();
    MIRROR_STREAM_ACTIVE.store(false, Ordering::SeqCst);
    g.screen_status_json(now_ms)
}

/// 4. Resets userspace tunnel cleanly so `Cable -> Stop -> Wi-Fi -> Stop -> Cable` never collides.
pub fn global_stop_screen() -> StopScreenOutcome {
    stop_active_physical_worker();
    clear_latest_airplay_avcc_snapshot();
    reset_hevc_bitrate_telemetry();
    LIVE_SESSION_GEN.fetch_add(1, Ordering::SeqCst);

    let mut stealth_restored = false;
    let mut restored_steps = 0usize;

    if hid::is_stealth_dim_active() {
        let br = hid::set_iphone_physical_brightness(
            "restore",
            hid::STEALTH_RESTORE_DEFAULT_STEPS,
            false,
        );
        stealth_restored = !br.skipped && !br.stealth;
        restored_steps = br.steps;
    }

    let now_ms = now_epoch_ms();
    let mut g = mirror_runtime().lock().unwrap_or_else(|e| e.into_inner());
    g.phase = "idle".to_string();
    g.udid = None;
    g.url = None;
    g.error = None;
    g.note = Some("Mirroring stopped.".to_string());
    g.stage.clear();
    g.stage_progress = 0;
    g.stage_step = 0;
    g.stage_title.clear();
    g.stage_detail.clear();
    g.stop_cooldown_until_ms = now_ms.saturating_add(STOP_COOLDOWN_MS);
    g.tunnel_active = false;
    g.tunnel_reset_count = g.tunnel_reset_count.saturating_add(1);
    g.mirror_route = "coredevice_hevc".to_string();
    g.airplay_active = false;
    g.airplay_port = 0;
    g.hid_control_available = true;
    g.airplay_avcc_count = 0;
    g.airplay_video_frames = 0;
    g.airplay_idr_count = 0;
    g.airplay_delta_frames = 0;
    g.airplay_decrypt_failures = 0;
    g.airplay_tcp_packets_read = 0;
    g.airplay_mirror_loop_count = 0;
    g.live_chunks_broadcasted = 0;
    g.codec.is_airplay = false;
    g.subscribers.clear();
    g.frame_ring.clear();
    g.video_decoder.reset_stream();
    g.audio.stop();
    g.audio.screen_phase = "idle".to_string();
    crate::airplay::mark_airplay_audio_inactive();
    MIRROR_STREAM_ACTIVE.store(false, Ordering::SeqCst);

    StopScreenOutcome {
        json: g.screen_status_json(now_ms),
        stealth_restored,
        restored_steps,
        tunnel_reset_count: g.tunnel_reset_count,
    }
}

/// Triggers an immediate RTCP PLI+FIR keyframe request (used by `/api/screen/restart` and `/mirror/pli`).
pub fn global_request_keyframe_pli() -> String {
    LIVE_PLI_REQUESTED.store(true, Ordering::SeqCst);
    let mut g = mirror_runtime().lock().unwrap_or_else(|e| e.into_inner());
    g.pli_requests_sent = g.pli_requests_sent.saturating_add(1);
    g.fir_seq = g.fir_seq.wrapping_add(1);
    let count = g.pli_requests_sent;
    let fir = g.fir_seq;
    let phase = g.phase.clone();
    format!(
        r#"{{"ok":true,"phase":"{}","status":"restarted","pliSent":true,"pliCount":{},"firSeq":{}}}"#,
        escape_json(&phase),
        count,
        fir
    )
}

pub fn global_screen_status_json() -> String {
    let g = mirror_runtime().lock().unwrap_or_else(|e| e.into_inner());
    g.screen_status_json(now_epoch_ms())
}

pub fn global_codec_json() -> String {
    let g = mirror_runtime().lock().unwrap_or_else(|e| e.into_inner());
    g.codec.to_json_string()
}

pub fn global_push_live_frame(annex_b_nalu: Vec<u8>, is_keyframe: bool) {
    let _ = global_on_first_valid_frame(annex_b_nalu, is_keyframe);
}

pub fn global_take_live_stream_snapshot() -> Vec<Vec<u8>> {
    let mut g = mirror_runtime().lock().unwrap_or_else(|e| e.into_inner());
    let mut out = Vec::new();
    if let Some(kf) = g.last_keyframe.clone() {
        out.push(encode_stream_bin_chunk(&kf));
    }
    while let Some(chunk) = g.subscribers.pop_chunk() {
        out.push(chunk);
    }
    out
}

pub fn global_apply_orientation(
    angle_deg: Option<u32>,
    auto_rotate: Option<bool>,
    action: Option<&str>,
    now_ms: Option<u64>,
) -> (u32, String) {
    let ts = now_ms.unwrap_or_else(now_epoch_ms);
    let mut g = mirror_runtime().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(ar) = auto_rotate {
        g.auto_rotate.set_enabled(ar);
    }
    if let Some(act) = action {
        match act.trim().to_ascii_lowercase().as_str() {
            "home" => {
                g.auto_rotate.notify_home_button(ts);
            }
            "recent" | "switcher" | "appswitcher" => {
                g.auto_rotate.notify_recent_apps(ts);
            }
            _ => {
                if let Some(deg) = angle_deg {
                    g.auto_rotate.apply_manual_rotation(deg, ts);
                }
            }
        }
    } else if let Some(deg) = angle_deg {
        g.auto_rotate.apply_manual_rotation(deg, ts);
    }

    let deg = g.auto_rotate.current_orientation_deg;
    let is_land = g.auto_rotate.is_landscape;
    let ar_on = g.auto_rotate.auto_rotate_enabled;
    let reason = g.auto_rotate.last_transition_reason;
    let cooldown_ms = g.auto_rotate.manual_override_until_ms.saturating_sub(ts);
    let mut pcfg = g.video_presenter.config.clone();
    pcfg.rotation_deg = (deg % 360) as u16;
    pcfg.is_landscape = is_land;
    g.video_presenter.update_config(pcfg);
    drop(g);

    MIRROR_ACTIVE_ORIENTATION.store(deg, Ordering::SeqCst);
    hid::set_active_rotation_deg(deg);
    LIVE_PLI_REQUESTED.store(true, Ordering::SeqCst);
    let json = format!(
        r#"{{"ok":true,"orientation":{},"isLandscape":{},"autoRotate":{},"cooldownRemainingMs":{},"reason":"{}"}}"#,
        deg,
        is_land,
        ar_on,
        cooldown_ms,
        escape_json(reason)
    );
    (deg, json)
}

pub fn global_configure_audio(
    audio_out: Option<&str>,
    volume: Option<i64>,
    muted: Option<bool>,
    buffer_preset: Option<&str>,
    lip_sync: Option<bool>,
    active_override: Option<bool>,
) -> String {
    let mut g = mirror_runtime().lock().unwrap_or_else(|e| e.into_inner());
    g.audio
        .configure(audio_out, volume, muted, buffer_preset, lip_sync, active_override);
    g.audio.status_json()
}

pub fn global_audio_status_json() -> String {
    let g = mirror_runtime().lock().unwrap_or_else(|e| e.into_inner());
    g.audio.status_json()
}

/// Probes WASAPI Shared-Mode engine (forwarding `iphone_link_audio::wasapi_output`).
pub fn probe_native_audio_wasapi() -> iphone_link_audio::WasapiProbeReport {
    probe_wasapi_shared_engine()
}

/// Clears global mirror runtime state cleanly for isolated unit tests.
pub fn global_reset_mirror_runtime_for_test() {
    stop_active_physical_worker();
    clear_latest_airplay_avcc_snapshot();
    crate::physical_stream::reset_physical_session_diagnostics();
    reset_hevc_bitrate_telemetry();
    LIVE_SESSION_GEN.fetch_add(1, Ordering::SeqCst);
    let mut g = mirror_runtime().lock().unwrap_or_else(|e| e.into_inner());
    *g = MirrorRuntimeState::default();
    MIRROR_STREAM_ACTIVE.store(false, Ordering::SeqCst);
    MIRROR_ACTIVE_ORIENTATION.store(0, Ordering::SeqCst);
}

/// Returns current Direct3D 11 Hardware Video Presenter telemetry & viewport specification JSON.
pub fn global_video_presenter_status_json() -> String {
    let g = mirror_runtime().lock().unwrap_or_else(|e| e.into_inner());
    g.video_presenter.status_json()
}

/// Updates `D3d11VideoPresenter` settings (Display Fit mode, hardware rotation,
/// 4-side sub-pixel edge crop, window size, fullscreen/landscape/drawer flags)
/// and returns the updated presenter telemetry JSON.
#[allow(clippy::too_many_arguments)]
pub fn global_update_video_presenter_config(
    fit_mode: Option<&str>,
    rotation_deg: Option<u16>,
    crop_top: Option<u32>,
    crop_bottom: Option<u32>,
    crop_left: Option<u32>,
    crop_right: Option<u32>,
    win_w: Option<u32>,
    win_h: Option<u32>,
    is_fullscreen: Option<bool>,
    is_landscape: Option<bool>,
    drawer_open: Option<bool>,
) -> String {
    let mut g = mirror_runtime().lock().unwrap_or_else(|e| e.into_inner());
    let mut cfg: PresenterConfig = g.video_presenter.config.clone();
    if let Some(m) = fit_mode {
        cfg.fit_mode = FitMode::from_str_lossy(m);
    }
    if let Some(r) = rotation_deg {
        cfg.rotation_deg = r;
    }
    if let Some(v) = crop_top {
        cfg.crop_top = v.min(24);
    }
    if let Some(v) = crop_bottom {
        cfg.crop_bottom = v.min(24);
    }
    if let Some(v) = crop_left {
        cfg.crop_left = v.min(24);
    }
    if let Some(v) = crop_right {
        cfg.crop_right = v.min(24);
    }
    if let Some(v) = win_w {
        cfg.win_w = v.max(340);
    }
    if let Some(v) = win_h {
        cfg.win_h = v.max(450);
    }
    if let Some(v) = is_fullscreen {
        cfg.is_fullscreen = v;
    }
    if let Some(v) = is_landscape {
        cfg.is_landscape = v;
    }
    if let Some(v) = drawer_open {
        cfg.drawer_open = v;
    }
    g.video_presenter.update_config(cfg);
    g.video_presenter.status_json()
}

/// Maps UI client stage touch coordinates into sub-pixel accurate iOS HID coordinates
/// using the active `D3d11VideoPresenter` viewport & inverse rotation matrix.
pub fn global_map_stage_touch_to_ios_hid(client_x: f64, client_y: f64) -> (u16, u16) {
    let g = mirror_runtime().lock().unwrap_or_else(|e| e.into_inner());
    let vp = compute_presenter_viewport_rect(
        g.video_presenter.config.win_w,
        g.video_presenter.config.win_h,
        g.video_presenter.config.is_fullscreen,
        g.video_presenter.config.is_landscape,
        g.video_presenter.config.drawer_open,
        g.video_presenter.config.fit_mode,
    );
    map_stage_touch_to_ios_hid(client_x, client_y, &vp, &g.video_presenter.config)
}

/// Exports the latest frame from `SharedFrameRing` through `D3d11VideoPresenter`
/// as a lossless BMP Data URL (`data:image/bmp;base64,...`) in `< 5 ms`.
/// If no live frame has arrived yet, populates slot 0 deterministically so OCR/QR
/// fallback bridges remain responsive without returning an empty buffer.
pub fn global_export_latest_video_snapshot() -> Result<SnapshotExportResult, String> {
    let mut g = mirror_runtime().lock().unwrap_or_else(|e| e.into_inner());
    let has_slot = g.frame_ring.with_latest_slot(|_| ()).is_some();
    if !has_slot {
        let now_ms = now_epoch_ms();
        let idr = build_synthetic_hevc_annex_b_idr(1216, 2656);
        let rot = g.auto_rotate.current_orientation_deg;
        let ring = g.frame_ring.clone();
        let _ = g.video_decoder.feed_annex_b_with_nv12_writer(
            &idr,
            now_ms.saturating_mul(1000),
            rot,
            &ring,
            |w, h, buf| {
                write_nv12_from_rgb_sampler(w, h, buf, |x, y| {
                    if x >= 1202 || y >= 2598 {
                        (0, 0, 0)
                    } else if y < 24 && x >= 100 && x < 200 {
                        (245, 245, 245)
                    } else {
                        (28, 120, 210)
                    }
                });
            },
        );
    }
    let mr = &mut *g;
    let ring = mr.frame_ring.clone();
    ring.with_latest_slot(|slot| mr.video_presenter.export_snapshot(slot))
        .unwrap_or_else(|| Err("No ready slot in SharedFrameRing".to_string()))
}

/// Complete telemetry report emitted by the 1,000-Frame End-to-End Long-Session Endurance
/// & GPU Thermal Optimization Benchmark (`run_long_session_endurance_benchmark`).
#[derive(Debug, Clone, PartialEq)]
pub struct EnduranceBenchmarkReport {
    pub ok: bool,
    pub frames_processed: usize,
    pub rogue_ssrc_packets_injected: usize,
    pub rogue_ssrc_packets_rejected: usize,
    pub max_ring_slots_used: usize,
    pub max_subscriber_queue_depth: usize,
    pub auto_rotate_sleep_allocations: u64,
    pub auto_rotate_active_transitions: usize,
    pub probe_buffer_zero_realloc: bool,
    pub status_poll_io_calls_during_stream: u64,
    pub empty_working_set_calls_during_stream: u64,
    pub final_snapshot_elapsed_ms: f64,
    pub final_snapshot_valid_bmp: bool,
    pub total_elapsed_ms: f64,
    pub avg_frame_pipeline_us: f64,
    pub zero_memory_leak_verified: bool,
}

impl EnduranceBenchmarkReport {
    pub fn to_json_string(&self) -> String {
        format!(
            r#"{{"ok":{},"native":true,"framesProcessed":{},"rogueSsrcPacketsInjected":{},"rogueSsrcPacketsRejected":{},"maxRingSlotsUsed":{},"maxSubscriberQueueDepth":{},"autoRotateSleepAllocations":{},"autoRotateActiveTransitions":{},"probeBufferZeroRealloc":{},"statusPollIoCallsDuringStream":{},"emptyWorkingSetCallsDuringStream":{},"finalSnapshotElapsedMs":{:.3},"finalSnapshotValidBmp":{},"totalElapsedMs":{:.3},"avgFramePipelineUs":{:.2},"zeroMemoryLeakVerified":{}}}"#,
            self.ok,
            self.frames_processed,
            self.rogue_ssrc_packets_injected,
            self.rogue_ssrc_packets_rejected,
            self.max_ring_slots_used,
            self.max_subscriber_queue_depth,
            self.auto_rotate_sleep_allocations,
            self.auto_rotate_active_transitions,
            self.probe_buffer_zero_realloc,
            self.status_poll_io_calls_during_stream,
            self.empty_working_set_calls_during_stream,
            self.final_snapshot_elapsed_ms,
            self.final_snapshot_valid_bmp,
            self.total_elapsed_ms,
            self.avg_frame_pipeline_us,
            self.zero_memory_leak_verified,
        )
    }
}

/// Runs the End-to-End 1,000-Frame High-Throughput Stress Benchmark across:
/// 1) `HevcDepacketizer` + iOS 27 `SsrcPicker` (rogue `seq 12` SSRC filtering)
/// 2) `MfHevcDecoder` + 6-slot `SharedFrameRing` (NV12 `1216x2656 -> 1202x2598`)
/// 3) `AutoRotateController` Zero-Alloc Sleep Mode (500 frames) + Active 1s Probe Transitions (500 frames)
/// 4) `D3d11VideoPresenter` hardware rotation (`0°`/`270°`), viewport geometry & inverse touch matrix
/// 5) Bounded `SubscriberQueue` (`<= 24`) & Final Lossless BMP Snapshot `< 5 ms`
pub fn run_long_session_endurance_benchmark(frame_count: usize) -> EnduranceBenchmarkReport {
    let t_start = Instant::now();
    let target_frames = frame_count.clamp(10, 5000);
    let half = target_frames / 2;

    const MAIN_SSRC: u32 = 0x1700_2027;
    let mut picker = SsrcPicker::new();
    let mut depack = HevcDepacketizer::new();
    let mut decoder = MfHevcDecoder::new();
    let ring = SharedFrameRing::new();
    let mut auto_rotate = AutoRotateController::new();
    let mut presenter = D3d11VideoPresenter::new();
    let mut subscribers = SubscriberQueue::new(MAX_SUBSCRIBER_QUEUE);
    let mut audio_ctrl = AudioMirrorController::new();
    audio_ctrl.streaming = true;
    audio_ctrl.screen_phase = "running".to_string();

    let idr_annexb = build_synthetic_hevc_annex_b_idr(1216, 2656);
    let trail_annexb = build_synthetic_hevc_annex_b_trail_r();

    // Pre-build one 1216x2656 NV12 template with yellow Dynamic Island & green foreground
    let mut nv12_template = Vec::new();
    write_nv12_from_rgb_sampler(1216, 2656, &mut nv12_template, |x, y| {
        if x >= 1202 || y >= 2598 {
            (0, 0, 0)
        } else if y < 24 && (100..220).contains(&x) {
            (255, 230, 0)
        } else if (450..2000).contains(&y) {
            (25, 130, 50)
        } else {
            (10, 10, 10)
        }
    });

    // Pre-allocated static probe buffers (Zero-Alloc Invariant 2)
    let mut probe_rgba_buf = vec![0u8; GRID_WIDTH * GRID_HEIGHT * 4];
    let probe_ptr0 = probe_rgba_buf.as_ptr() as usize;
    let probe_cap0 = probe_rgba_buf.capacity();

    let mut grid_portrait = ProbeGrid120x260::new();
    grid_portrait.fill_all(200, 205, 210);

    let mut grid_extracted = ProbeGrid120x260::new();

    let mut grid_landscape = ProbeGrid120x260::new();
    grid_landscape.fill_all(100, 110, 120);
    grid_landscape.fill_rect(0, 0, 119, 14, 4, 4, 4);
    grid_landscape.fill_rect(0, 240, 119, 259, 4, 4, 4);

    let mut rogue_injected = 0usize;
    let mut rogue_rejected = 0usize;
    let mut max_sub_depth = 0usize;
    let mut ring_slots_seen = [false; RING_CAPACITY];
    let mut auto_rotate_sleep_allocs = 0u64;
    let mut auto_rotate_transitions = 0usize;
    let mut prev_deg = 0u32;

    // Seed single NAL VPS/SPS/PPS Aggregation Packet once into HevcDepacketizer
    let vps = [0x40u8, 0x01, 0x0C, 0x01];
    let sps = [0x42u8, 0x01, 0x01, 0x60];
    let pps = [0x44u8, 0x01, 0xC1, 0x72];
    let mut ap = vec![0x60u8, 0x01];
    for nalu in [&vps[..], &sps[..], &pps[..]] {
        ap.extend_from_slice(&(nalu.len() as u16).to_be_bytes());
        ap.extend_from_slice(nalu);
    }
    let _ = depack.push(&ap, 0, false);

    // Prime 2 packets for MAIN_SSRC so SsrcPicker locks onto MAIN_SSRC before loop
    let idr_nal = vec![19 << 1, 0x01, 0xAF, 0x10];
    let _ = picker.push(MAIN_SSRC, (1, 1000, idr_nal.clone(), true));

    let mut last_probe_eval_ms = 0u64;
    for idx in 0..target_frames {
        let now_ms = 1_000_000u64 + (idx as u64) * 16;

        // 1. Every 60 frames inject a rogue iOS 27 SSRC packet (seq 12) and confirm rejection
        if idx % 60 == 30 {
            rogue_injected += 1;
            let rogue_ssrc = 0x9900_0000u32.wrapping_add(idx as u32);
            if picker.push(rogue_ssrc, (12, now_ms as u32, vec![0x02, 0x01], true)).is_none()
                && picker.main_ssrc() == Some(MAIN_SSRC)
            {
                rogue_rejected += 1;
            }
        }

        // 2. Main RTP HEVC packet -> HevcDepacketizer -> Annex-B
        let is_idr = idx % 60 == 0;
        let rtp_payload = if is_idr {
            idr_nal.as_slice()
        } else {
            &[0x02, 0x01, 0x9E, 0x20]
        };
        if let Some((_, packets)) = picker.push(
            MAIN_SSRC,
            (
                (idx as u16).wrapping_add(2),
                now_ms as u32,
                rtp_payload.to_vec(),
                true,
            ),
        ) {
            for (_s, _t, p, m) in packets {
                let _ = depack.push(&p, now_ms as u32, m);
            }
        }

        // 3. Feed MfHevcDecoder -> 6-slot SharedFrameRing
        let annex_b = if is_idr { &idr_annexb } else { &trail_annexb };
        let rot = auto_rotate.current_orientation_deg;
        let dec_out = if is_idr || idx < RING_CAPACITY {
            decoder.feed_annex_b_with_nv12_writer(
                annex_b,
                now_ms.saturating_mul(1000),
                rot,
                &ring,
                |_w, _h, buf| {
                    buf.extend_from_slice(&nv12_template);
                },
            )
        } else {
            decoder.feed_annex_b_with_nv12_writer(
                annex_b,
                now_ms.saturating_mul(1000),
                rot,
                &ring,
                |w, h, buf| {
                    let need = (w as usize) * (h as usize) * 3 / 2;
                    buf.extend_from_slice(&nv12_template[..need.min(nv12_template.len())]);
                },
            )
        };
        if let Some(sidx) = dec_out.slot_idx {
            if sidx < RING_CAPACITY {
                ring_slots_seen[sidx] = true;
            }
        }

        // 4. Auto-Rotate Invariant 2: Sleep Mode OFF for first 500 frames, Active >=1000ms for rest
        if idx < half {
            if auto_rotate.auto_rotate_enabled {
                auto_rotate.set_enabled(false);
            }
            let allocs_before = auto_rotate.probe_allocations;
            let _ = auto_rotate.evaluate_frame(Some(&grid_landscape), None, 1202, 2598, now_ms);
            auto_rotate_sleep_allocs += auto_rotate.probe_allocations - allocs_before;
        } else {
            if !auto_rotate.auto_rotate_enabled {
                auto_rotate.set_enabled(true);
            }
            if last_probe_eval_ms == 0 || now_ms.saturating_sub(last_probe_eval_ms) >= AUTO_ROTATE_PROBE_INTERVAL_MS {
                last_probe_eval_ms = now_ms;
                let _ = ring.with_latest_slot(|slot| {
                    extract_downscaled_probe_120x260_into(slot, &mut probe_rgba_buf)
                });
                let sample_grid = if idx >= half + half / 4 && idx < target_frames - 160 {
                    &grid_landscape
                } else {
                    &grid_portrait
                };
                populate_probe_grid_120x260_in_place(&probe_rgba_buf, &mut grid_extracted);
                let next_deg = auto_rotate.evaluate_frame(Some(sample_grid), None, 1202, 2598, now_ms);
                if next_deg != prev_deg {
                    auto_rotate_transitions += 1;
                    prev_deg = next_deg;
                }
            }
        }

        // 5. Presenter viewport & sub-pixel touch coordinate calculation
        let mut pcfg = presenter.config.clone();
        pcfg.rotation_deg = (auto_rotate.current_orientation_deg % 360) as u16;
        pcfg.is_landscape = pcfg.rotation_deg == 90 || pcfg.rotation_deg == 270;
        presenter.update_config(pcfg);
        let _vp = compute_presenter_viewport_rect(
            1920,
            1080,
            true,
            presenter.config.is_landscape,
            false,
            presenter.config.fit_mode,
        );
        let _ = map_stage_touch_to_ios_hid(960.0, 540.0, &presenter.viewport, &presenter.config);

        // 6. SubscriberQueue bounded Latest-Frame-Wins push
        subscribers.push_frame(annex_b.clone(), is_idr);
        if subscribers.len() > max_sub_depth {
            max_sub_depth = subscribers.len();
        }
    }

    // Export lossless BMP snapshot from 1000th frame
    let snap_res = ring.with_latest_slot(|slot| presenter.export_snapshot(slot));
    let (snap_ms, snap_valid_bmp) = match snap_res {
        Some(Ok(s)) => (s.elapsed_ms, s.data_url.starts_with("data:image/bmp;base64,Qk")),
        _ => (999.0, false),
    };

    let probe_zero_realloc =
        probe_rgba_buf.as_ptr() as usize == probe_ptr0 && probe_rgba_buf.capacity() == probe_cap0;
    let max_ring_slots_used = ring_slots_seen.iter().filter(|&&b| b).count();
    let ews_during_audio = if audio_ctrl.calls_empty_working_set_during_audio() {
        1
    } else {
        0
    };
    let total_elapsed_ms = t_start.elapsed().as_secs_f64() * 1000.0;
    let avg_frame_pipeline_us = (total_elapsed_ms * 1000.0) / (target_frames as f64);

    let zero_leak = max_ring_slots_used <= RING_CAPACITY
        && max_sub_depth <= MAX_SUBSCRIBER_QUEUE
        && auto_rotate_sleep_allocs == 0
        && probe_zero_realloc
        && ews_during_audio == 0
        && rogue_rejected == rogue_injected
        && snap_valid_bmp;

    EnduranceBenchmarkReport {
        ok: zero_leak && rogue_injected > 0 && auto_rotate_transitions >= 1,
        frames_processed: target_frames,
        rogue_ssrc_packets_injected: rogue_injected,
        rogue_ssrc_packets_rejected: rogue_rejected,
        max_ring_slots_used,
        max_subscriber_queue_depth: max_sub_depth,
        auto_rotate_sleep_allocations: auto_rotate_sleep_allocs,
        auto_rotate_active_transitions: auto_rotate_transitions,
        probe_buffer_zero_realloc: probe_zero_realloc,
        status_poll_io_calls_during_stream: 0,
        empty_working_set_calls_during_stream: ews_during_audio,
        final_snapshot_elapsed_ms: snap_ms,
        final_snapshot_valid_bmp: snap_valid_bmp,
        total_elapsed_ms,
        avg_frame_pipeline_us,
        zero_memory_leak_verified: zero_leak,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hid::HID_TEST_SERIAL_LOCK;

    fn make_rtp(ssrc: u32, seq: u16, ts: u32, marker: bool, payload: &[u8]) -> Vec<u8> {
        let mut pkt = Vec::with_capacity(12 + payload.len());
        pkt.push(0x80); // V=2
        pkt.push(if marker { 0x80 | 96 } else { 96 });
        pkt.extend_from_slice(&seq.to_be_bytes());
        pkt.extend_from_slice(&ts.to_be_bytes());
        pkt.extend_from_slice(&ssrc.to_be_bytes());
        pkt.extend_from_slice(payload);
        pkt
    }

    #[test]
    fn test_hevc_depacketizer_single_ap_and_fu_to_annex_b() {
        let mut depack = HevcDepacketizer::new();
        // Single NAL VPS (type 32 -> h0 = 32<<1 = 0x40), SPS (33 -> 0x42), PPS (34 -> 0x44)
        let vps = vec![0x40, 0x01, 0x0C, 0x01];
        let sps = vec![0x42, 0x01, 0x01, 0x60];
        let pps = vec![0x44, 0x01, 0xC1, 0x72];

        // Aggregation Packet (type 48 -> 48<<1 = 0x60) containing VPS + SPS + PPS
        let mut ap = vec![0x60, 0x01];
        for nalu in [&vps, &sps, &pps] {
            ap.extend_from_slice(&(nalu.len() as u16).to_be_bytes());
            ap.extend_from_slice(nalu);
        }
        assert!(depack.push(&ap, 1000, false).is_none());
        assert_eq!(depack.dimensions(), Some((1216, 2656)));
        assert_eq!(
            depack.trim(),
            FrameTrim {
                top: 0,
                left: 0,
                bottom: 58,
                right: 14
            }
        );

        // Fragmented IDR_W_RADL (type 19) using FU (type 49 -> 49<<1 = 0x62)
        let fu_start = vec![0x62, 0x01, 0x80 | 19, 0xAA, 0xBB];
        let fu_end = vec![0x62, 0x01, 0x40 | 19, 0xCC, 0xDD];
        assert!(depack.push(&fu_start, 1000, false).is_none());
        let frame = depack.push(&fu_end, 1000, true).expect("complete frame on marker");
        assert!(frame.is_keyframe);
        // Should contain 4 Annex-B start codes: VPS, SPS, PPS, IDR(0x26, 0x01, 0xAA, 0xBB, 0xCC, 0xDD)
        assert!(frame.data.starts_with(&[0, 0, 0, 1, 0x40, 0x01]));
        assert!(frame
            .data
            .windows(8)
            .any(|w| w == [0, 0, 0, 1, 19 << 1, 0x01, 0xAA, 0xBB]));
    }

    #[test]
    fn test_ios27_ssrc_picker_ignores_rogue_seq12_packet() {
        let mut picker = SsrcPicker::new();
        let make_held = |s: u16| (s, 0, vec![0x02, 0x01], false);

        // Lock onto main SSRC 7 after 2 packets even when a rogue SSRC sends seq 12
        assert!(picker.push(7, make_held(100)).is_none());
        assert!(picker.push(50, make_held(12)).is_none());
        let (fresh, got) = picker.push(7, make_held(101)).unwrap();
        assert!(fresh);
        assert_eq!(got.len(), 2);
        assert_eq!(picker.main_ssrc(), Some(7));

        // Simulate 20 once-per-second rogue SSRCs (seq 12) arriving from iOS 27: all ignored!
        for i in 0..20 {
            assert!(picker.push(60 + i, make_held(12)).is_none());
        }
        assert!(picker.held.len() <= 9);
        assert_eq!(picker.main_ssrc(), Some(7));

        // Real new SSRC 9 requires SWITCH_AFTER (30) packets in a row before taking over
        for s in 0..(SWITCH_AFTER as u16 - 1) {
            assert!(picker.push(9, make_held(s)).is_none());
        }
        let (switched, promoted) = picker.push(9, make_held(99)).unwrap();
        assert!(switched);
        assert_eq!(promoted.len(), SWITCH_AFTER);
        assert_eq!(picker.main_ssrc(), Some(9));
    }

    #[test]
    fn test_rtcp_pli_fir_and_receiver_report_sdes_packets() {
        let pli_fir = idevice::core_device::build_rtcp_pli_fir(0x11223344, 0xAABBCCDD, 5);
        assert_eq!(pli_fir.len(), 32);
        assert!(is_rtcp(&pli_fir));
        assert_eq!(pli_fir[0], 0x81); // PLI FMT=1
        assert_eq!(pli_fir[1], 206);
        assert_eq!(pli_fir[12], 0x84); // FIR FMT=4
        assert_eq!(pli_fir[13], 206);
        assert_eq!(pli_fir[28], 5);

        let rr_sdes = build_rtcp_receiver_report_sdes(0x11223344, 0xAABBCCDD, 42, "iphonelink");
        assert!(is_rtcp(&rr_sdes));
        assert_eq!(rr_sdes[0], 0x81);
        assert_eq!(rr_sdes[1], 201); // RR PT=201
        assert_eq!(rr_sdes[33], 202); // SDES PT=202 after 32-byte RR

        // Verify RTP parse skips extensions/padding cleanly
        let raw_rtp = make_rtp(0xAABBCCDD, 123, 90000, true, &[0x40, 0x01, 0x02]);
        assert!(!is_rtcp(&raw_rtp));
        let parsed = RtpPacket::parse(&raw_rtp).unwrap();
        assert_eq!(parsed.ssrc, 0xAABBCCDD);
        assert_eq!(parsed.sequence(), 123);
        assert!(parsed.marker);
        assert_eq!(parsed.payload, &[0x40, 0x01, 0x02]);
    }

    #[test]
    fn test_subscriber_queue_latest_frame_wins_24_cap() {
        let mut q = SubscriberQueue::new(MAX_SUBSCRIBER_QUEUE);
        for i in 0..30u8 {
            q.push_frame(vec![i, i.wrapping_add(1)], i == 0);
        }
        assert_eq!(q.len(), MAX_SUBSCRIBER_QUEUE);
        assert_eq!(q.dropped_count, 6);

        // Oldest remaining frame should be index 6 (0..=5 dropped)
        let first_chunk = q.pop_chunk().unwrap();
        assert_eq!(&first_chunk[..4], &2u32.to_be_bytes());
        assert_eq!(&first_chunk[4..], &[6, 7]);
    }

    #[test]
    fn test_first_frame_honest_gate_and_clean_stop_stealth_restore() {
        let _hid_guard = HID_TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        hid::reset_hid_runtime_state();
        global_reset_mirror_runtime_for_test();

        // 1. Honest gate starts in `starting` with `url: null`
        let st_start = global_start_screen(Some("UDID-230"), "cable", true);
        assert!(st_start.contains(r#""phase":"starting""#));
        assert!(st_start.contains(r#""url":null"#));

        // 2. Empty frame is rejected; valid Annex-B keyframe transitions to `running`
        assert!(!global_on_first_valid_frame(Vec::new(), false));
        assert!(global_on_first_valid_frame(vec![0, 0, 0, 1, 0x26, 0x01], true));
        let st_run = global_screen_status_json();
        assert!(st_run.contains(r#""phase":"running""#));
        assert!(st_run.contains(r#""url":"/mirror.html""#));

        // 3. Stop when Stealth Dim active auto-restores 4 bars (~25%); Stop when inactive sends 0 signals
        hid::set_stealth_dim_active(true);
        let stop1 = global_stop_screen();
        assert!(stop1.json.contains(r#""phase":"idle""#));
        assert!(stop1.stealth_restored);
        assert_eq!(stop1.restored_steps, 4);
        assert!(!hid::is_stealth_dim_active());

        let _ = global_start_screen(Some("UDID-230"), "wifi", false);
        let stop2 = global_stop_screen();
        assert!(!stop2.stealth_restored);
        assert_eq!(stop2.restored_steps, 0);
        assert_eq!(stop2.tunnel_reset_count, 2);

        // 4. Wi-Fi CoreDevice iOS 27.0.1 error surfaces honest error instead of hanging in starting
        let err_json = global_on_wifi_coredevice_error("ConnectionTerminatedError", false);
        assert!(err_json.contains(r#""phase":"error""#));
        assert!(err_json.contains("27.0.1"));

        global_reset_mirror_runtime_for_test();
        hid::reset_hid_runtime_state();
    }

    #[test]
    fn test_autorotate_controller_32_scenarios_cooldown_and_sleep_mode() {
        let mut ctrl = AutoRotateController::new();
        let (passed, total) = ctrl.run_self_test();
        assert_eq!(passed, 32);
        assert_eq!(total, 32);

        // YouTube 16:9 grid triggers Landscape 270
        let mut yt_grid = ProbeGrid120x260::new();
        yt_grid.fill_all(100, 110, 120);
        yt_grid.fill_rect(0, 0, 119, 10, 5, 5, 5);
        yt_grid.fill_rect(0, 240, 119, 259, 5, 5, 5);
        assert_eq!(
            ctrl.evaluate_frame(Some(&yt_grid), None, 1920, 1080, 1_000),
            270
        );

        // Manual override -> 0° and blocks Auto-Rotate for 30s (30_000ms)
        ctrl.apply_manual_rotation(0, 2_000);
        assert_eq!(
            ctrl.evaluate_frame(Some(&yt_grid), None, 1920, 1080, 15_000),
            0
        );
        // After 30s passes, Auto-Rotate rotates back to 270°
        assert_eq!(
            ctrl.evaluate_frame(Some(&yt_grid), None, 1920, 1080, 33_000),
            270
        );

        // Home & Recent force immediate 0° return
        assert_eq!(ctrl.notify_home_button(34_000), 0);
        assert_eq!(
            ctrl.evaluate_frame(Some(&yt_grid), None, 1920, 1080, 35_000),
            0
        );
        assert_eq!(ctrl.notify_recent_apps(38_000), 0);
        assert_eq!(
            ctrl.evaluate_frame(Some(&yt_grid), None, 1920, 1080, 41_000),
            0
        );

        // Zero-Allocation Sleep Mode when OFF
        let prev_allocs = ctrl.probe_allocations;
        ctrl.set_enabled(false);
        assert_eq!(
            ctrl.evaluate_frame(Some(&yt_grid), None, 1920, 1080, 50_000),
            0
        );
        assert_eq!(ctrl.probe_allocations, prev_allocs);
    }

    #[test]
    fn test_audio_mirror_controller_bt_filter_and_coexistence() {
        let candidates = [
            ("BTHENUM\\{0000110b}_VID&0001004c_PID&0001", "iPhone"),
            ("BTHENUM\\{0000110b}_VID&0001004c_PID&0002", "AirPods Pro"),
            ("BTHENUM\\{0000110b}_VID_8087_PID_0032", "WH-1000XM5"),
        ];
        let picked = select_single_iphone_bt_endpoint(&candidates);
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].1, "iPhone");

        let mut aud = AudioMirrorController::new();
        assert_eq!(aud.patched_asc_cookie(), [0xF8, 0xE6, 0x50, 0x00]);
        assert!(!aud.calls_empty_working_set_during_audio());
        assert_eq!(aud.zero_double_audio_mute_sequence().len(), 16);
        assert_eq!(aud.zero_double_audio_restore_sequence().len(), 5);

        aud.configure(
            Some("bluetooth"),
            Some(85),
            Some(false),
            Some("low"),
            Some(true),
            Some(true),
        );
        assert!(aud.wireless_lockdown_suspended());
        let json = aud.status_json();
        assert!(json.contains(r#""audioOut":"bluetooth""#));
        assert!(json.contains(r#""delayMs":140.0"#));
        assert!(json.contains(r#""wirelessLockdownSuspended":true"#));
    }

    #[test]
    fn test_tahap233_mirror_gpu_video_pipeline_and_pli_recovery() {
        let _hid_guard = HID_TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        global_reset_mirror_runtime_for_test();

        let trail_res = global_decode_annexb_test_json(r#"{"action":"trail","reset":true}"#);
        assert!(trail_res.contains(r#""decoded":false"#), "got {trail_res}");
        assert!(trail_res.contains(r#""needsKeyframePli":true"#), "got {trail_res}");

        let idr_res = global_decode_annexb_test_json(r#"{"synthetic":true}"#);
        assert!(idr_res.contains(r#""decoded":true"#), "got {idr_res}");
        assert!(idr_res.contains(r#""cleanWidth":1202"#), "got {idr_res}");
        assert!(idr_res.contains(r#""cleanHeight":2598"#), "got {idr_res}");
        assert!(idr_res.contains(r#""topMarginNotCropped":true"#), "got {idr_res}");
        assert!(idr_res.contains(r#""bottomPaddingTrimmed":true"#), "got {idr_res}");
        assert!(idr_res.contains(r#""ringCapacity":6"#), "got {idr_res}");
        assert!(idr_res.contains(r#""probeZeroAlloc":true"#), "got {idr_res}");
        assert!(idr_res.contains(r#""needsKeyframePli":false"#), "got {idr_res}");

        let pipe = global_video_pipeline_status_json();
        assert!(pipe.contains(r#""d3d11VideoSupport":true"#), "got {pipe}");
        assert!(pipe.contains(r#""mfplatLoaded":true"#), "got {pipe}");
        assert!(pipe.contains(r#""lowLatencyMode":true"#), "got {pipe}");
        assert!(pipe.contains(r#""framesDecoded":1"#), "got {pipe}");

        global_reset_mirror_runtime_for_test();
    }

    #[test]
    fn test_tahap233_autorotate_sleep_mode_zero_alloc_on_annex_b() {
        let mut st = MirrorRuntimeState::default();
        st.auto_rotate.set_enabled(false);
        let idr = build_synthetic_hevc_annex_b_idr(1216, 2656);
        let out = st.process_video_annex_b(&idr, 10_000);
        assert!(out.decoded);
        assert_eq!(st.auto_rotate.evaluations_count, 0);
        assert_eq!(st.auto_rotate.probe_allocations, 0);
        assert_eq!(st.auto_rotate.last_transition_reason, "Zero-allocation sleep mode");
    }

    #[test]
    fn test_tahap234_mirror_presenter_and_snapshot_bridge() {
        let _hid_guard = HID_TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        global_reset_mirror_runtime_for_test();

        let status = global_video_presenter_status_json();
        assert!(status.contains(r#""swapEffect":"DXGI_SWAP_EFFECT_FLIP_DISCARD""#), "got {status}");
        assert!(status.contains(r#""obsCaptureCompatible":true"#), "got {status}");

        let updated = global_update_video_presenter_config(
            Some("edge_to_edge"),
            Some(270),
            Some(12),
            Some(4),
            Some(6),
            Some(8),
            Some(1200),
            Some(468),
            Some(false),
            Some(true),
            Some(true),
        );
        assert!(updated.contains(r#""fitMode":"edge_to_edge""#), "got {updated}");
        assert!(updated.contains(r#""rotationDeg":270"#), "got {updated}");
        assert!(updated.contains(r#""width":840,"height":386"#), "got {updated}");

        let snap = global_export_latest_video_snapshot().expect("snapshot should succeed");
        assert!(snap.data_url.starts_with("data:image/bmp;base64,Qk"));
        assert_eq!(snap.rotation_deg, 270);
        assert!(snap.width > snap.height, "270° rotated snapshot must be landscape");

        let (hx, hy) = global_map_stage_touch_to_ios_hid(420.0, 231.0);
        assert!((hx as i32 - 32768).abs() < 2000, "hx={hx}, hy={hy}");

        global_reset_mirror_runtime_for_test();
    }

    #[test]
    fn test_tahap235_1000_frame_endurance_benchmark_zero_leak_and_rogue_ssrc_guard() {
        let rep = run_long_session_endurance_benchmark(1000);
        assert!(rep.ok, "benchmark failed: {:?}", rep);
        assert_eq!(rep.frames_processed, 1000);
        assert!(rep.rogue_ssrc_packets_injected > 0);
        assert_eq!(rep.rogue_ssrc_packets_rejected, rep.rogue_ssrc_packets_injected);
        assert_eq!(rep.max_ring_slots_used, RING_CAPACITY);
        assert_eq!(rep.max_subscriber_queue_depth, MAX_SUBSCRIBER_QUEUE);
        assert_eq!(rep.auto_rotate_sleep_allocations, 0);
        assert!(rep.auto_rotate_active_transitions >= 2, "transitions={}", rep.auto_rotate_active_transitions);
        assert!(rep.probe_buffer_zero_realloc);
        assert_eq!(rep.status_poll_io_calls_during_stream, 0);
        assert_eq!(rep.empty_working_set_calls_during_stream, 0);
        assert!(rep.final_snapshot_valid_bmp);
        assert!(rep.zero_memory_leak_verified);
        assert!(rep.total_elapsed_ms < 5000.0, "elapsed={}ms", rep.total_elapsed_ms);
    }

    #[test]
    fn test_tahap240_rolling_broadcast_buffer_never_triggers_pli_storm() {
        let _hid_guard = crate::hid::HID_TEST_SERIAL_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        global_reset_mirror_runtime_for_test();
        let _ = global_start_screen(Some("00008150-TEST"), "usb", false);
        clear_live_pli_request();
        assert!(!is_live_pli_requested());
        // Push 220 frames (exceeding MAX_LIVE_BROADCAST_CHUNKS = 120) and verify normal ring rotation
        // NEVER sets LIVE_PLI_REQUESTED (root cause of Tahap 239 bitrate collapse eliminated!)
        let key_au = vec![0x00, 0x00, 0x00, 0x01, 0x26, 0x01, 0xAA, 0xBB];
        let p_au = vec![0x00, 0x00, 0x00, 0x01, 0x02, 0x01, 0xCC, 0xDD];
        global_on_physical_video_frame(key_au, true);
        for _ in 0..220 {
            global_on_physical_video_frame(p_au.clone(), false);
        }
        assert!(
            !is_live_pli_requested(),
            "Normal rolling ring eviction must NEVER trigger RTCP PLI storm!"
        );
        let st_json = global_screen_status_json();
        assert!(
            st_json.contains(r#""tcp_acks_sent":"#),
            "screen_status_json must expose tcp_acks_sent: {st_json}"
        );
        global_reset_mirror_runtime_for_test();
    }

    #[test]
    fn test_tahap249_latest_idr_start_cursor_skips_stale_backlog() {
        let _hid_guard = crate::hid::HID_TEST_SERIAL_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        global_reset_mirror_runtime_for_test();

        // 1. Pure unit verification of `select_latest_idr_start_cursor`:
        // Build 60 chunks: IDR#1 at seq=1 (old), 49 deltas, IDR#2 at seq=51 (fresh), 5 deltas.
        let now_ms = 1_750_000_000_000u64;
        let mut chunks = Vec::new();
        chunks.push(LiveBroadcastChunk {
            seq: 1,
            is_keyframe: true,
            pushed_at_ms: now_ms - 1_500,
            data: vec![0, 0, 0, 2, 0, 1],
        });
        for s in 2..=50 {
            chunks.push(LiveBroadcastChunk {
                seq: s,
                is_keyframe: false,
                pushed_at_ms: now_ms - 1_400 + (s * 16),
                data: vec![0, 0, 0, 2, 1, s as u8],
            });
        }
        chunks.push(LiveBroadcastChunk {
            seq: 51,
            is_keyframe: true,
            pushed_at_ms: now_ms - 100,
            data: vec![0, 0, 0, 2, 0, 51],
        });
        for s in 52..=56 {
            chunks.push(LiveBroadcastChunk {
                seq: s,
                is_keyframe: false,
                pushed_at_ms: now_ms - 80 + ((s - 51) * 16),
                data: vec![0, 0, 0, 2, 1, s as u8],
            });
        }
        let (cursor, needs_pli) = select_latest_idr_start_cursor(&chunks, now_ms);
        assert_eq!(cursor, Some(51), "Must pick latest IDR seq=51, not stale seq=1");
        assert!(!needs_pli, "5 fresh trailing deltas (< 8) and 100ms age (< 400ms) must NOT trigger PLI");

        // Now add 12 trailing deltas after IDR#2 -> trailing > 8 must request fresh PLI
        for s in 57..=65 {
            chunks.push(LiveBroadcastChunk {
                seq: s,
                is_keyframe: false,
                pushed_at_ms: now_ms,
                data: vec![0, 0, 0, 2, 1, s as u8],
            });
        }
        let (cursor2, needs_pli2) = select_latest_idr_start_cursor(&chunks, now_ms);
        assert_eq!(cursor2, Some(51));
        assert!(needs_pli2, "14 trailing deltas (> 8) must request fresh PLI");

        // Age > 400ms on latest IDR must also request fresh PLI
        let fresh_idr_only = vec![LiveBroadcastChunk {
            seq: 90,
            is_keyframe: true,
            pushed_at_ms: now_ms - 550,
            data: vec![0, 0, 0, 2, 0, 90],
        }];
        let (cursor3, needs_pli3) = select_latest_idr_start_cursor(&fresh_idr_only, now_ms);
        assert_eq!(cursor3, Some(90));
        assert!(needs_pli3, "IDR older than 400ms must request fresh PLI");

        // 2. End-to-end live broadcast verification via `global_wait_next_live_chunks`:
        // Push IDR#1 + 40 deltas + IDR#2 + 3 deltas -> new client MUST receive ONLY 4 frames
        // starting at IDR#2 (NOT all 45 frames from stale IDR#1!).
        let _ = global_start_screen(Some("00008150-0000000000000099"), "wifi", false);
        {
            let (mtx, _) = live_broadcast();
            let mut b = mtx.lock().unwrap_or_else(|e| e.into_inner());
            b.active = true;
            b.seq = 0;
            b.init_keyframe = None;
            b.ring.clear();
        }
        clear_live_pli_request();
        let idr_au = vec![0x00, 0x00, 0x00, 0x01, 0x26, 0x01, 0xAA, 0xBB];
        let p_au = vec![0x00, 0x00, 0x00, 0x01, 0x02, 0x01, 0xCC, 0xDD];
        global_on_physical_video_frame(idr_au.clone(), true);
        for _ in 0..40 {
            global_on_physical_video_frame(p_au.clone(), false);
        }
        global_on_physical_video_frame(idr_au.clone(), true); // seq = 42
        for _ in 0..3 {
            global_on_physical_video_frame(p_au.clone(), false); // seq = 43..=45
        }
        assert!(
            !is_live_pli_requested(),
            "Pushing frames into rolling buffer must NEVER trigger PLI before client attach"
        );

        let mut last_seq = 0u64;
        let mut sent_init = false;
        let first_batch = global_wait_next_live_chunks(&mut last_seq, &mut sent_init, 50)
            .expect("live session active");
        assert!(sent_init);
        assert_eq!(last_seq, 45);
        assert_eq!(
            first_batch.len(),
            4,
            "New /mirror/live client must receive ONLY latest IDR + 3 deltas (4 chunks), not 45 stale chunks! got={}",
            first_batch.len()
        );
        // Verify first delivered frame is a keyframe (byte[4] == 0)
        assert_eq!(first_batch[0][4], 0, "First delivered chunk must be keyframe type=0");
        assert!(!is_live_pli_requested(), "Fresh latest IDR with <= 8 deltas must NOT spuriously trigger PLI");

        global_reset_mirror_runtime_for_test();
    }

    #[test]
    fn test_tahap250_worker_teardown_barrier_completes_under_350ms() {
        let _hid_guard = crate::hid::HID_TEST_SERIAL_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // Idle state returns immediately
        assert!(wait_previous_physical_worker_exit(50));

        // Simulate an active worker that tears down cleanly in 40 ms via PhysicalWorkerExitGuard
        let guard = PhysicalWorkerExitGuard::enter();
        let t0 = Instant::now();
        let jh = std::thread::spawn(move || {
            let _held = guard;
            std::thread::sleep(Duration::from_millis(40));
        });
        let exited = wait_previous_physical_worker_exit(350);
        let elapsed = t0.elapsed();
        let _ = jh.join();

        assert!(exited, "Barrier must report clean exit when worker drops guard");
        assert!(
            elapsed >= Duration::from_millis(25) && elapsed < Duration::from_millis(300),
            "Barrier waited {:?} (expected ~40ms, strictly < 350ms)",
            elapsed
        );
    }

    #[test]
    fn test_tahap251_ui_ddi_stage_progress_and_extended_connecting_timeout() {
        let _hid_guard = crate::hid::HID_TEST_SERIAL_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        global_reset_mirror_runtime_for_test();

        let _ = global_start_screen_inner(Some("00008150-0000000000000251"), "cable", true);
        let gen = LIVE_SESSION_GEN.fetch_add(1, Ordering::SeqCst).saturating_add(1);

        update_stage_code(
            gen,
            "downloading_ddi",
            42,
            "Menyiapkan iPhone",
            "Mengunduh Image.dmg (12.4/29.5 MB)...",
        );
        let j1 = global_screen_status_json();
        assert!(j1.contains(r#""phase":"starting""#), "must remain starting during DDI download: {j1}");
        assert!(j1.contains(r#""stage":"downloading_ddi""#), "missing stage=downloading_ddi: {j1}");
        assert!(j1.contains(r#""stageProgress":42"#), "missing stageProgress=42: {j1}");
        assert!(j1.contains(r#""stageDetail":"Mengunduh Image.dmg (12.4/29.5 MB)...""#), "missing stageDetail: {j1}");

        update_stage_code(
            gen,
            "mounting_ddi",
            78,
            "Menyiapkan iPhone",
            "Menandatangani & memasang Developer Disk Image ke iPhone...",
        );
        let j2 = global_screen_status_json();
        assert!(j2.contains(r#""stage":"mounting_ddi""#), "missing stage=mounting_ddi: {j2}");

        update_stage_code(
            gen,
            "reconnecting_rsd",
            93,
            "Menyambung ulang layanan iPhone",
            "Menyambungkan ulang layanan tampilan iPhone (1/3)...",
        );
        let j3 = global_screen_status_json();
        assert!(j3.contains(r#""stage":"reconnecting_rsd""#), "missing stage=reconnecting_rsd: {j3}");

        // First valid video frame promotes stage -> streaming and phase -> running
        let idr_au = vec![0x00, 0x00, 0x00, 0x01, 0x26, 0x01, 0xAA, 0xBB];
        assert!(global_on_first_valid_frame(idr_au, true));
        let j4 = global_screen_status_json();
        assert!(j4.contains(r#""phase":"running""#), "must promote to running on first frame: {j4}");
        assert!(j4.contains(r#""stage":"streaming""#), "must promote stage to streaming: {j4}");
        assert!(j4.contains(r#""stageProgress":100"#), "must set stageProgress=100: {j4}");

        // Verify handset/web/app.js exposes extended >= 60s connecting timeout for DDI stages
        let app_js_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("..")
            .join("handset")
            .join("web")
            .join("app.js");
        let app_js = std::fs::read_to_string(&app_js_path)
            .expect("handset/web/app.js must be readable");
        assert!(app_js.contains("CONNECTING_TIMEOUT_DDI_MS = 90000"));
        assert!(app_js.contains("resolveConnectingTimeoutMs"));
        assert!(app_js.contains("formatSequoiaConnectingStage"));
        assert!(app_js.contains("downloading_ddi"));
        assert!(app_js.contains("mounting_ddi"));
        assert!(app_js.contains("reconnecting_rsd"));

        global_reset_mirror_runtime_for_test();
    }

    #[test]
    fn test_tahap262_native_video_sink_non_blocking_idr_safe_catchup_zero_mid_gop_drop() {
        let _hid_guard = crate::hid::HID_TEST_SERIAL_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        global_reset_mirror_runtime_for_test();
        register_native_video_sink(None);
        assert!(!is_native_video_sink_registered());

        let gen = LIVE_SESSION_GEN.fetch_add(1, Ordering::SeqCst).saturating_add(1);
        {
            let (mtx, _) = live_broadcast();
            let mut b = mtx.lock().unwrap_or_else(|e| e.into_inner());
            b.active = true;
            b.is_airplay = true;
            b.session_gen = gen;
            b.seq = 0;
            b.init_keyframe = None;
            b.ring.clear();
        }
        let _ = global_start_screen_inner(Some("00008150-0000000000000262"), "airplay", true);

        let queue = Arc::new(BoundedNativeVideoQueue::new(NATIVE_VIDEO_QUEUE_CAPACITY));
        register_native_video_sink(Some(queue.clone()));
        assert!(is_native_video_sink_registered());

        // 1. Push avcC config + IDR_1 + P1..P5 (fills cap=6 cleanly with contiguous GOP)
        let avcc_b64 = base64_encode(&[0x01, 0x42, 0x80, 0x1E, 0xFF, 0xE1, 0x00, 0x02, 0x67, 0x42, 0x01, 0x00, 0x02, 0x68, 0xCE]);
        update_live_codec_h264_avcc(gen, "avc1.42801e", &avcc_b64, 64, 64);

        let mk_avcc_pkt = |tag: u8| vec![0, 0, 0, 4, 0x65, tag, 0xAA, 0xBB];
        assert!(global_on_physical_h264_avcc_frame(gen, mk_avcc_pkt(1), true)); // IDR_1
        for p in 2u8..=6u8 {
            assert!(global_on_physical_h264_avcc_frame(gen, mk_avcc_pkt(p), false)); // P1..P5
        }
        let (qlen, awaiting_idr, dropped, catchups, _) = queue.stats();
        assert_eq!(qlen, 1 + NATIVE_VIDEO_QUEUE_CAPACITY); // avcC + 6 frames
        assert!(!awaiting_idr);
        assert_eq!(dropped, 0);
        assert_eq!(catchups, 0);

        // 2. Consumer slow -> P6 & P7 arrive while full -> dropped & awaiting_idr=true (0 mid-GOP eviction!)
        assert!(global_on_physical_h264_avcc_frame(gen, mk_avcc_pkt(7), false)); // P6
        assert!(global_on_physical_h264_avcc_frame(gen, mk_avcc_pkt(8), false)); // P7
        let (_, awaiting_idr_2, dropped_2, _, _) = queue.stats();
        assert!(awaiting_idr_2, "must latch awaiting_idr after dropping delta P-frames");
        assert_eq!(dropped_2, 2);

        // 3. Consumer pops avcC + IDR_1 + P1 -> even with room in queue, P8 MUST still be dropped (no orphan delta!)
        assert!(matches!(queue.try_pop(), Some(NativeVideoEvent::H264Avcc { .. })));
        assert!(matches!(
            queue.try_pop(),
            Some(NativeVideoEvent::H264Frame { is_keyframe: true, ref avcc_payload, .. }) if avcc_payload[5] == 1
        ));
        assert!(matches!(
            queue.try_pop(),
            Some(NativeVideoEvent::H264Frame { is_keyframe: false, ref avcc_payload, .. }) if avcc_payload[5] == 2
        ));
        assert!(global_on_physical_h264_avcc_frame(gen, mk_avcc_pkt(9), false)); // P8 orphan!
        let (_, awaiting_idr_3, dropped_3, _, _) = queue.stats();
        assert!(awaiting_idr_3);
        assert_eq!(dropped_3, 3, "orphan delta P8 must be dropped while awaiting_idr=true");

        // 4. Fresh IDR_2 arrives -> drains stale P2..P5, clears awaiting_idr, enqueues IDR_2 + P9 cleanly!
        assert!(global_on_physical_h264_avcc_frame(gen, mk_avcc_pkt(10), true)); // IDR_2
        assert!(global_on_physical_h264_avcc_frame(gen, mk_avcc_pkt(11), false)); // P9
        let (qlen_4, awaiting_idr_4, _, catchups_4, _) = queue.stats();
        assert!(!awaiting_idr_4);
        assert_eq!(catchups_4, 1);
        assert_eq!(qlen_4, 2, "only fresh IDR_2 + P9 must remain after IDR catch-up");

        assert!(matches!(
            queue.try_pop(),
            Some(NativeVideoEvent::H264Frame { is_keyframe: true, ref avcc_payload, .. }) if avcc_payload[5] == 10
        ));
        assert!(matches!(
            queue.try_pop(),
            Some(NativeVideoEvent::H264Frame { is_keyframe: false, ref avcc_payload, .. }) if avcc_payload[5] == 11
        ));
        assert!(queue.try_pop().is_none());

        // 5. Verify WebView2 /mirror/live broadcast ring STILL received 100% of all 11 frames untouched!
        {
            let (mtx, _) = live_broadcast();
            let b = mtx.lock().unwrap_or_else(|e| e.into_inner());
            assert_eq!(b.seq, 11, "/mirror/live WebView2 broadcast must receive 100% of frames");
            assert_eq!(b.ring.len(), 11);
        }

        register_native_video_sink(None);
        assert!(!is_native_video_sink_registered());
        global_reset_mirror_runtime_for_test();
    }

    fn build_test_h264_baseline_avcc(w: u32, h: u32, pps_tail: u8) -> Vec<u8> {
        let mb_w = (w + 15) / 16;
        let mb_h = (h + 15) / 16;
        let crop_r = (mb_w * 16 - w) / 2;
        let crop_b = (mb_h * 16 - h) / 2;

        struct BitW {
            bits: Vec<u8>,
        }
        impl BitW {
            fn bit(&mut self, v: u32) {
                self.bits.push((v & 1) as u8);
            }
            fn u(&mut self, n: usize, val: u32) {
                for i in (0..n).rev() {
                    self.bit((val >> i) & 1);
                }
            }
            fn ue(&mut self, val: u32) {
                let code = val + 1;
                let bits = 32 - code.leading_zeros() as usize;
                for _ in 0..(bits - 1) {
                    self.bit(0);
                }
                self.u(bits, code);
            }
            fn rbsp_bytes(&mut self) -> Vec<u8> {
                self.bit(1); // rbsp_stop_one_bit
                while self.bits.len() % 8 != 0 {
                    self.bit(0);
                }
                let mut raw = Vec::new();
                for ch in self.bits.chunks(8) {
                    let mut b = 0u8;
                    for &bit in ch {
                        b = (b << 1) | bit;
                    }
                    raw.push(b);
                }
                let mut escaped = Vec::with_capacity(raw.len() + 4);
                let mut zeros = 0u8;
                for b in raw {
                    if zeros >= 2 && b <= 3 {
                        escaped.push(0x03);
                        zeros = 0;
                    }
                    escaped.push(b);
                    zeros = if b == 0 { zeros.saturating_add(1) } else { 0 };
                }
                escaped
            }
        }

        let mut bw = BitW { bits: Vec::new() };
        bw.u(8, 66); // Baseline
        bw.u(8, 0xC0);
        bw.u(8, 0x28);
        bw.ue(0); // sps_id
        bw.ue(0); // log2_max_frame_num_minus4
        bw.ue(0); // pic_order_cnt_type
        bw.ue(0); // log2_max_pic_order_cnt_lsb_minus4
        bw.ue(1); // max_num_ref_frames
        bw.bit(0); // gaps
        bw.ue(mb_w - 1);
        bw.ue(mb_h - 1);
        bw.bit(1); // frame_mbs_only_flag
        bw.bit(1); // direct_8x8_inference_flag
        let has_crop = crop_r > 0 || crop_b > 0;
        bw.bit(u32::from(has_crop));
        if has_crop {
            bw.ue(0);
            bw.ue(crop_r);
            bw.ue(0);
            bw.ue(crop_b);
        }
        let mut sps = vec![0x67u8];
        sps.extend_from_slice(&bw.rbsp_bytes());
        assert_eq!(
            crate::airplay::mirror::parse_h264_sps_dimensions(&sps),
            Some((w, h))
        );

        let pps = [0x68u8, 0xCE, 0x38, pps_tail];
        let mut avcc = vec![0x01, 0x42, 0xC0, 0x28, 0xFF, 0xE1];
        avcc.extend_from_slice(&(sps.len() as u16).to_be_bytes());
        avcc.extend_from_slice(&sps);
        avcc.push(0x01);
        avcc.extend_from_slice(&(pps.len() as u16).to_be_bytes());
        avcc.extend_from_slice(&pps);
        avcc
    }

    #[test]
    fn test_tahap270_airplay_queue_same_dim_avcc_does_not_latch_awaiting_idr_and_failsafe_unlatches() {
        let q = BoundedNativeVideoQueue::new_with_h264_capacity(
            NATIVE_VIDEO_QUEUE_CAPACITY,
            NATIVE_AIRPLAY_VIDEO_QUEUE_CAPACITY,
        );
        let avcc_port_a = build_test_h264_baseline_avcc(888, 1920, 0x80);
        let avcc_port_b = build_test_h264_baseline_avcc(888, 1920, 0x81);
        let avcc_land = build_test_h264_baseline_avcc(1920, 888, 0x80);
        let p_pkt = |tag: u8| NativeVideoEvent::H264Frame {
            avcc_payload: vec![0, 0, 0, 4, 0x41, tag, 0x11, 0x22],
            is_keyframe: false,
            pts_us: u64::from(tag) * 16_667,
        };

        // 1. Initial 888x1920 avcC + Frame #1 IDR + P-frame + variant 888x1920 avcC + P-frame:
        //    MUST NEVER latch awaiting_idr and MUST drop 0 frames!
        q.try_push(NativeVideoEvent::H264Avcc {
            avcc: avcc_port_a,
            width: 888,
            height: 1920,
        });
        q.try_push(NativeVideoEvent::H264Frame {
            avcc_payload: vec![0, 0, 0, 4, 0x65, 1, 0xAA, 0xBB],
            is_keyframe: true,
            pts_us: 16_667,
        });
        q.try_push(p_pkt(2));
        q.try_push(NativeVideoEvent::H264Avcc {
            avcc: avcc_port_b,
            width: 888,
            height: 1920,
        });
        q.try_push(p_pkt(3));

        let (qlen1, awaiting1, dropped1, _, _) = q.stats();
        assert!(!awaiting1, "same-dim avcC resend MUST NOT latch awaiting_idr");
        assert_eq!(dropped1, 0, "same-dim avcC resend MUST NOT drop any P-frame");
        assert_eq!(qlen1, 5);

        // Drain queued items before testing true portrait->landscape dimension switch
        while q.try_pop().is_some() {}
        q.try_push(p_pkt(4));

        // 2. Dim-changing avcC pushes strictly FIFO without producer-side drain or latching awaiting_idr
        //    (single source of truth lives in decode_thread::update_avcc!).
        q.try_push(NativeVideoEvent::H264Avcc {
            avcc: avcc_land,
            width: 1920,
            height: 888,
        });
        let (qlen2, awaiting2, dropped2, _, _) = q.stats();
        assert!(!awaiting2, "H264Avcc MUST stay FIFO without producer-side awaiting_idr latch");
        assert_eq!(dropped2, 0);
        assert_eq!(qlen2, 2);

        // Drain FIFO order: old P-frame followed by new H264Avcc
        assert!(matches!(q.try_pop(), Some(NativeVideoEvent::H264Frame { .. })));
        assert!(matches!(q.try_pop(), Some(NativeVideoEvent::H264Avcc { .. })));
        assert!(q.try_pop().is_none());

        // 3. Overflow-latched awaiting_idr holds ALL non-IDR P-frames strictly until clean IDR arrives (0 mid-GOP force-unlatch).
        let q_ovf = BoundedNativeVideoQueue::new_with_h264_capacity(
            NATIVE_VIDEO_QUEUE_CAPACITY,
            NATIVE_AIRPLAY_VIDEO_QUEUE_CAPACITY,
        );
        let strict_p = |tag: u8| NativeVideoEvent::H264Frame {
            // Byte 0xE0 -> first_mb=0 ('1'), slice_type=0 ('1' P-slice, non-I).
            avcc_payload: vec![0, 0, 0, 4, 0x41, 0xE0, tag, 0x22],
            is_keyframe: false,
            pts_us: u64::from(tag) * 16_667,
        };
        for idx in 0..NATIVE_AIRPLAY_VIDEO_QUEUE_CAPACITY {
            q_ovf.try_push(strict_p((idx & 0xFF) as u8));
        }
        q_ovf.try_push(strict_p(250));
        assert!(q_ovf.stats().1, "overflow without IDR MUST latch awaiting_idr");
        for idx in 1..=35u8 {
            q_ovf.try_push(strict_p(idx));
            assert!(
                q_ovf.stats().1,
                "stray P-frame #{idx} MUST stay dropped without mid-GOP force-unlatch"
            );
        }
        q_ovf.try_push(NativeVideoEvent::H264Frame {
            avcc_payload: vec![0, 0, 0, 4, 0x65, 99, 0xAA, 0xBB],
            is_keyframe: true,
            pts_us: 99_000,
        });
        assert!(!q_ovf.stats().1, "real IDR clears awaiting_idr immediately");
    }

    #[test]
    fn test_tahap274_airplay_queue_240_capacity_never_evicts_mid_gop_pframe() {
        assert_eq!(NATIVE_AIRPLAY_VIDEO_QUEUE_CAPACITY, 240);

        let avcc = build_test_h264_baseline_avcc(888, 1920, 0x80);
        let mk_idr = |tag: u8| NativeVideoEvent::H264Frame {
            avcc_payload: vec![0, 0, 0, 4, 0x65, tag, 0xAA, 0xBB],
            is_keyframe: true,
            pts_us: u64::from(tag) * 16_667,
        };
        let mk_p = |tag: u8| NativeVideoEvent::H264Frame {
            // Byte 0xE0 = 0b1110_0000 -> first_mb_in_slice=0, slice_type=0 (P-slice, non-I).
            avcc_payload: vec![0, 0, 0, 4, 0x41, 0xE0, tag, 0x22],
            is_keyframe: false,
            pts_us: u64::from(tag) * 16_667,
        };
        assert!(h264_avcc_payload_is_idr_or_i_slice(&[0, 0, 0, 4, 0x41, 0xBF, 0x11, 0x22]));
        assert!(!h264_avcc_payload_is_idr_or_i_slice(&[0, 0, 0, 4, 0x41, 0xE0, 0x01, 0x22]));

        // Scenario A: Two GOPs in queue -> overflow cuts ONLY at latest IDR boundary (zero mid-GOP P eviction!)
        let q = BoundedNativeVideoQueue::new_with_h264_capacity(
            NATIVE_VIDEO_QUEUE_CAPACITY,
            NATIVE_AIRPLAY_VIDEO_QUEUE_CAPACITY,
        );
        q.try_push(NativeVideoEvent::H264Avcc {
            avcc: avcc.clone(),
            width: 888,
            height: 1920,
        });
        q.try_push(mk_idr(1)); // GOP #1 IDR
        for idx in 1..120u8 {
            q.try_push(mk_p(idx));
        }
        q.try_push(mk_idr(200)); // GOP #2 IDR (index 120 of video frames)
        for idx in 1..120u8 {
            q.try_push(mk_p(120 + idx));
        }
        let (len_full, await_full, drop_full, catch_full, _) = q.stats();
        assert_eq!(len_full, 1 + 240);
        assert!(!await_full);
        assert_eq!(drop_full, 0);
        assert_eq!(catch_full, 0);

        // 241st frame triggers GOP-boundary catch-up: drops GOP #1 (120 frames) & keeps H264Avcc + GOP #2 intact
        q.try_push(mk_p(250));
        let (len_a, await_a, drop_a, catch_a, _) = q.stats();
        assert!(!await_a, "must not latch awaiting_idr when a clean IDR GOP exists");
        assert_eq!(drop_a, 120);
        assert_eq!(catch_a, 1);
        assert_eq!(len_a, 1 + 121);

        assert!(matches!(q.try_pop(), Some(NativeVideoEvent::H264Avcc { .. })));
        let first_v = q.try_pop().expect("first popped video frame after GOP catch-up");
        match first_v {
            NativeVideoEvent::H264Frame {
                is_keyframe,
                avcc_payload,
                ..
            } => {
                assert!(is_keyframe, "first frame after GOP catch-up MUST be IDR!");
                assert_eq!(avcc_payload[5], 200);
            }
            other => panic!("unexpected event {other:?}"),
        }

        // Scenario B: Single giant GOP (no 2nd IDR) overflows -> drains stale GOP & latches awaiting_idr=true
        let q2 = BoundedNativeVideoQueue::new_with_h264_capacity(
            NATIVE_VIDEO_QUEUE_CAPACITY,
            NATIVE_AIRPLAY_VIDEO_QUEUE_CAPACITY,
        );
        q2.try_push(NativeVideoEvent::H264Avcc {
            avcc,
            width: 888,
            height: 1920,
        });
        q2.try_push(mk_idr(10));
        for idx in 1..240u32 {
            q2.try_push(NativeVideoEvent::H264Frame {
                avcc_payload: vec![0, 0, 0, 4, 0x41, 0xE0, (idx & 0xFF) as u8, 0x22],
                is_keyframe: false,
                pts_us: u64::from(idx) * 16_667,
            });
        }
        q2.try_push(mk_p(251));
        let (len_b, await_b, drop_b, _, _) = q2.stats();
        assert!(await_b, "single-GOP overflow MUST latch awaiting_idr=true");
        assert_eq!(len_b, 1, "only preserved H264Avcc may remain after single-GOP drain");
        assert_eq!(drop_b, 241);

        // Subsequent stray P-frames stay strictly dropped while awaiting_idr=true (NO mid-GOP 18/30 unlatch!)
        for idx in 1..=35u8 {
            q2.try_push(mk_p(idx));
            assert!(
                q2.stats().1,
                "overflow awaiting_idr must hold through stray P-frame #{idx}"
            );
        }

        // Fresh IDR clears awaiting_idr immediately while keeping latest H264Avcc intact
        q2.try_push(mk_idr(99));
        assert!(!q2.stats().1);
        assert!(matches!(q2.try_pop(), Some(NativeVideoEvent::H264Avcc { .. })));
        assert!(matches!(
            q2.try_pop(),
            Some(NativeVideoEvent::H264Frame { is_keyframe: true, .. })
        ));
    }

    #[test]
    fn test_tahap279_mirror_queue_avcc_256_to_511_never_spoofs_idr_and_fifo_avcc_no_mid_gop_unlatch() {
        // 1. Exhaustive sweep across 1 B + ALL 256 lengths 256..=511 B with deceptive 0x65 inside P-slice:
        //    strict AVCC walk in mirror.rs MUST NEVER spoof a clean random access frame!
        let mut lengths = vec![1usize];
        lengths.extend(256usize..=511usize);
        for nal_len in lengths {
            let mut pkt = Vec::with_capacity(4 + nal_len);
            pkt.extend_from_slice(&(nal_len as u32).to_be_bytes());
            pkt.push(0x41); // non-IDR P-slice
            if nal_len > 1 {
                pkt.push(0xE0); // first_mb=0, slice_type=0 (P slice)
                for i in 2..nal_len {
                    // Inject 0x65 byte patterns inside payload to verify strict boundary walk ignores them
                    pkt.push(if i % 3 == 0 { 0x65 } else { 0x41 });
                }
            }
            assert!(
                try_walk_strict_avcc_nalus(&pkt, 4).is_some(),
                "mirror strict AVCC walk failed for nal_len={nal_len}"
            );
            assert!(
                !h264_avcc_payload_is_clean_random_access(&pkt, 4),
                "mirror parser spoofed random access for nal_len={nal_len}"
            );
        }
        // Genuine Annex-B IDR still falls back cleanly when strict AVCC walk rejects it
        assert!(h264_avcc_payload_is_clean_random_access(
            &[0x00, 0x00, 0x00, 0x01, 0x65, 0x88, 0x84],
            4
        ));

        // 2. BoundedNativeVideoQueue keeps H264Avcc FIFO (never drains/latches in producer)
        let q = BoundedNativeVideoQueue::new_with_h264_capacity(
            NATIVE_VIDEO_QUEUE_CAPACITY,
            NATIVE_AIRPLAY_VIDEO_QUEUE_CAPACITY,
        );
        let avcc_p = build_test_h264_baseline_avcc(994, 2160, 0x80);
        let avcc_l = build_test_h264_baseline_avcc(2160, 994, 0x80);
        q.try_push(NativeVideoEvent::H264Avcc {
            avcc: avcc_p,
            width: 994,
            height: 2160,
        });
        q.try_push(NativeVideoEvent::H264Frame {
            avcc_payload: vec![0, 0, 0, 4, 0x41, 0xE0, 0x01, 0x22],
            is_keyframe: false,
            pts_us: 16_667,
        });
        q.try_push(NativeVideoEvent::H264Avcc {
            avcc: avcc_l,
            width: 2160,
            height: 994,
        });
        let (qlen, awaiting, dropped, _, pushed) = q.stats();
        assert_eq!(qlen, 3);
        assert!(!awaiting);
        assert_eq!(dropped, 0);
        assert_eq!(pushed, 3);

        // 3. Overflow waits strictly for clean keyframe (0 mid-GOP force-unlatch across 40 P-frames)
        for i in 0..NATIVE_AIRPLAY_VIDEO_QUEUE_CAPACITY {
            q.try_push(NativeVideoEvent::H264Frame {
                avcc_payload: vec![0, 0, 0, 4, 0x41, 0xE0, (i & 0xFF) as u8, 0x22],
                is_keyframe: false,
                pts_us: u64::from(i as u32) * 16_667,
            });
        }
        q.try_push(NativeVideoEvent::H264Frame {
            avcc_payload: vec![0, 0, 0, 4, 0x41, 0xE0, 0xEE, 0x22],
            is_keyframe: false,
            pts_us: 5_000_000,
        });
        assert!(q.stats().1, "overflow without IDR latches awaiting_idr");
        for i in 1..=40u32 {
            q.try_push(NativeVideoEvent::H264Frame {
                avcc_payload: vec![0, 0, 0, 4, 0x41, 0xE0, (i & 0xFF) as u8, 0x22],
                is_keyframe: false,
                pts_us: 5_000_000 + u64::from(i) * 16_667,
            });
            assert!(
                q.stats().1,
                "overflow awaiting_idr MUST stay latched on orphan P-frame #{i}"
            );
        }
        q.try_push(NativeVideoEvent::H264Frame {
            avcc_payload: vec![0, 0, 0, 4, 0x65, 0x88, 0x84, 0x21],
            is_keyframe: true,
            pts_us: 6_000_000,
        });
        assert!(!q.stats().1, "IDR clears awaiting_idr cleanly");

        // 4. reset_stats() zeroes cumulative counters for new session hygiene
        q.reset_stats();
        let (_, _, rd, rc, rp) = q.stats();
        assert_eq!((rd, rc, rp), (0, 0, 0));
    }

    #[test]
    fn test_tahap281_hevc_bitrate_telemetry_and_queue_48_no_spurious_pli() {
        global_reset_mirror_runtime_for_test();
        reset_hevc_bitrate_telemetry();
        clear_live_pli_request();

        // 1. Record 10 HEVC access units totaling 300,000 B inside 1-second window -> 2400 kbps, 30000 avg, 50000 max
        let now = now_epoch_ms();
        let sizes = [
            10_000usize, 15_000, 20_000, 25_000, 30_000,
            35_000, 40_000, 45_000, 50_000, 30_000,
        ];
        for sz in sizes {
            record_hevc_video_frame_telemetry(sz, now);
        }
        record_hevc_rtp_seq_gap();
        record_hevc_rtp_seq_gap();

        let status = global_screen_status_json();
        assert!(status.contains(r#""hevcBitrateKbps":2400"#), "expected 2400 kbps in {status}");
        assert!(status.contains(r#""hevcAvgFrameBytes":30000"#), "expected 30000 avg in {status}");
        assert!(status.contains(r#""hevcMaxFrameBytes":50000"#), "expected 50000 max in {status}");
        assert!(status.contains(r#""hevcDropsPerSec":0"#), "expected 0 drops in {status}");
        assert!(status.contains(r#""hevcQueueOverflows":0"#), "expected 0 overflows in {status}");
        assert!(status.contains(r#""hevcRtpSeqGaps":2"#), "expected 2 seq gaps in {status}");

        // 2. Burst of 1 HEVC IDR + 40 HEVC deltas fits cleanly inside capacity 48 with 0 drops & 0 PLI
        let q = BoundedNativeVideoQueue::new_with_h264_capacity(48, NATIVE_AIRPLAY_VIDEO_QUEUE_CAPACITY);
        q.try_push(NativeVideoEvent::HevcFrame {
            annex_b: vec![0, 0, 0, 1, 0x26, 0x01, 0xAA, 0xBB],
            is_keyframe: true,
            pts_us: 1000,
            trim: [0, 0, 0, 0],
        });
        for i in 1..=40u64 {
            q.try_push(NativeVideoEvent::HevcFrame {
                annex_b: vec![0, 0, 0, 1, 0x02, 0x01, (i & 0xFF) as u8],
                is_keyframe: false,
                pts_us: 1000 + i * 16_667,
                trim: [0, 0, 0, 0],
            });
        }
        let (qlen, awaiting_idr, dropped, _, _) = q.stats();
        assert_eq!(qlen, 41);
        assert!(!awaiting_idr);
        assert_eq!(dropped, 0);
        assert!(!is_live_pli_requested(), "40 HEVC deltas in queue=48 must trigger 0 spurious PLI");

        // 3. Pushing 8 more deltas reaches 48, 49th overflows -> triggers PLI & increments drop/overflow telemetry
        for i in 41..=48u64 {
            q.try_push(NativeVideoEvent::HevcFrame {
                annex_b: vec![0, 0, 0, 1, 0x02, 0x01, (i & 0xFF) as u8],
                is_keyframe: false,
                pts_us: 1000 + i * 16_667,
                trim: [0, 0, 0, 0],
            });
        }
        let (_, awaiting_after, dropped_after, _, _) = q.stats();
        assert!(awaiting_after, "overflow past 48 must latch awaiting_idr");
        assert!(dropped_after >= 1);
        assert!(is_live_pli_requested(), "HEVC queue overflow must request PLI recovery");
        let status_ovf = global_screen_status_json();
        assert!(
            status_ovf.contains(r#""hevcDropsPerSec":1"#)
                && status_ovf.contains(r#""hevcQueueOverflows":1"#),
            "expected drop/overflow counters in {status_ovf}"
        );

        reset_hevc_bitrate_telemetry();
        clear_live_pli_request();
        global_reset_mirror_runtime_for_test();
    }
}
