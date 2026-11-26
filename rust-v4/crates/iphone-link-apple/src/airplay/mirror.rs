//! AirPlay H.264 Mirror TCP stream reader:
//! - Accepts multiple sequential TCP connections on `dataPort` (iOS may close & reopen video TCP)
//! - 128-byte AirPlay mirror packet header (`size`, `hdr[4]` payload type, float display dimensions)
//! - `kind == 1`: unencrypted `avcC` (`AVCDecoderConfigurationRecord`) -> `avc1.XXYYZZ` +
//!   exact H.264 SPS RBSP width/height (with fallback to `hdr[56..64]` & `hdr[40..48]`) + Base64 `avcC`
//! - `kind == 0`: continuous Windows CNG `Aes128Ctr` decryption with `valid_avcc_au()` + mid-stream
//!   keystream restart recovery, `normalize_avcc_au_for_webcodecs()` (strips SEI=6 & AUD=9, rejects
//!   corrupt trailing bytes, detects IDR=5), and telemetry callback `sink.on_decrypt_failure()`
//! - `kind == 2 | 5`: heartbeat / streaming report (ignored cleanly without advancing keystream)

use std::io::{self, Read};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use crate::mirror::base64_encode;
use crate::wifi::configure_low_latency_std_tcp;
use crate::win_cng::Aes128Ctr;

use super::info::DISPLAY_SIZE;
use super::{record_airplay_trace, wait_tcp_readable, AirPlaySink, AirPlayState};

const MAX_MIRROR_PACKET_BYTES: usize = 32 * 1024 * 1024;

/// Continuous AES-128-CTR decryptor for AirPlay mirror H.264 packets (`kind == 0`).
///
/// Tahap 274 Hardening:
/// - Every `kind == 0` packet ALWAYS advances `self.ctr` by exact `buf.len()` bytes unless
///   a strict counter-0 resync is verified.
/// - Once synced (`self.synced == true`), counter-0 resync (`fresh_ctr`) is ONLY committed when
///   `valid_avcc_au_strict` passes AND the AU carries IDR (`5`), SPS (`7`), or PPS (`8`).
pub struct MirrorDecryptor {
    key: [u8; 16],
    iv: [u8; 16],
    ctr: Aes128Ctr,
    synced: bool,
    nal_len_size: usize,
    pub bad_count: u64,
    pub consecutive_bad_ctr: u64,
    pub ctr_resync_count: u64,
    pub ctr_decrypt_rejects: u64,
    pub ctr_keystream_bytes: u64,
}

impl MirrorDecryptor {
    pub fn new(key: [u8; 16], iv: [u8; 16]) -> Result<Self, String> {
        let ctr = Aes128Ctr::new(&key, &iv)?;
        Ok(Self {
            key,
            iv,
            ctr,
            synced: false,
            nal_len_size: 4,
            bad_count: 0,
            consecutive_bad_ctr: 0,
            ctr_resync_count: 0,
            ctr_decrypt_rejects: 0,
            ctr_keystream_bytes: 0,
        })
    }

    pub fn set_nal_len_size(&mut self, nal_len_size: usize) {
        self.nal_len_size = nal_len_size.clamp(1, 4);
    }

    pub fn nal_len_size(&self) -> usize {
        self.nal_len_size
    }

    /// Decrypts `buf` in-place along the continuous AES-128-CTR keystream.
    ///
    /// Tahap 275 Invariant: EVERY `kind == 0` video packet advances `self.ctr` by exact
    /// `buf.len()` bytes (even on validation reject) and never rewinds `self.ctr`. Mid-stream
    /// counter-0 resync requires `>= 3` consecutive decrypt failures + strict IDR/SPS/PPS check.
    pub fn decrypt_in_place(&mut self, buf: &mut [u8]) -> Result<bool, String> {
        let mut trial = buf.to_vec();
        self.ctr.apply_keystream(&mut trial)?;
        let n_bytes = buf.len() as u64;
        self.ctr_keystream_bytes = self.ctr_keystream_bytes.saturating_add(n_bytes);
        super::add_trace_ctr_keystream_bytes(n_bytes);

        if valid_avcc_au_relaxed(&trial, self.nal_len_size) {
            buf.copy_from_slice(&trial);
            self.synced = true;
            self.consecutive_bad_ctr = 0;
            return Ok(true);
        }

        // Probe counter-0 keystream only before initial sync OR after >= 3 consecutive
        // running-CTR misses so a single padded/extension AU never false-resets `self.ctr`.
        let should_probe_fresh = if self.synced {
            self.consecutive_bad_ctr >= 3
        } else {
            true
        };
        if should_probe_fresh {
            let mut fresh = buf.to_vec();
            let mut fresh_ctr = Aes128Ctr::new(&self.key, &self.iv)?;
            fresh_ctr.apply_keystream(&mut fresh)?;
            let allow_resync = if self.synced {
                valid_avcc_au_strict(&fresh, self.nal_len_size)
                    && avcc_au_has_keyframe_or_params(&fresh, self.nal_len_size)
            } else {
                valid_avcc_au_strict(&fresh, self.nal_len_size)
            };
            if allow_resync {
                buf.copy_from_slice(&fresh);
                self.ctr = fresh_ctr;
                if self.synced {
                    self.ctr_resync_count = self.ctr_resync_count.saturating_add(1);
                    super::inc_trace_ctr_resync_count();
                    super::record_airplay_trace("ctr_resync", "ok");
                }
                self.synced = true;
                self.consecutive_bad_ctr = 0;
                return Ok(true);
            }
        }

        self.bad_count = self.bad_count.saturating_add(1);
        self.consecutive_bad_ctr = self.consecutive_bad_ctr.saturating_add(1);
        self.ctr_decrypt_rejects = self.ctr_decrypt_rejects.saturating_add(1);
        super::inc_trace_ctr_decrypt_rejects();
        Ok(false)
    }
}

fn summarize_avcc_nal_types(avcc: &[u8]) -> String {
    let mut types = Vec::new();
    let mut off = 0usize;
    while off + 4 <= avcc.len() && types.len() < 8 {
        let len =
            u32::from_be_bytes([avcc[off], avcc[off + 1], avcc[off + 2], avcc[off + 3]]) as usize;
        if len == 0 {
            break;
        }
        off += 4;
        if off + len > avcc.len() {
            break;
        }
        types.push((avcc[off] & 0x1F).to_string());
        off += len;
    }
    format!("[{}]", types.join(","))
}

fn read_be_nal_len(buf: &[u8], off: usize, len_sz: usize) -> usize {
    let mut n = 0usize;
    for b in &buf[off..off + len_sz] {
        n = (n << 8) | (*b as usize);
    }
    n
}

pub const MAX_RELAXED_TRAILING_ZERO_PAD: usize = 16;

fn validate_avcc_au_core(
    buf: &[u8],
    nal_len_size: usize,
    max_trailing_zero_pad: usize,
    allow_ext_nal: bool,
) -> bool {
    let len_sz = nal_len_size.clamp(1, 4);
    if buf.len() < len_sz + 1 {
        return false;
    }
    let mut off = 0usize;
    let mut count = 0usize;
    let mut has_meaningful = false;
    while off + len_sz <= buf.len() {
        let len = read_be_nal_len(buf, off, len_sz);
        if len == 0 {
            if count > 0
                && has_meaningful
                && (buf.len() - off) <= max_trailing_zero_pad
                && buf[off..].iter().all(|&b| b == 0)
            {
                return true;
            }
            return false;
        }
        off += len_sz;
        if off + len > buf.len() {
            return false;
        }
        let nal_hdr = buf[off];
        let forbidden = nal_hdr & 0x80;
        let nal_type = nal_hdr & 0x1F;
        let valid_type = if allow_ext_nal {
            (1..=12).contains(&nal_type) || matches!(nal_type, 19 | 20)
        } else {
            (1..=12).contains(&nal_type)
        };
        if forbidden != 0 || !valid_type {
            return false;
        }
        if matches!(nal_type, 1 | 5 | 6 | 7 | 8 | 9 | 19 | 20) {
            has_meaningful = true;
        }
        off += len;
        count += 1;
    }
    if count == 0 || !has_meaningful {
        return false;
    }
    if off == buf.len() {
        true
    } else if max_trailing_zero_pad == 0 {
        false
    } else {
        let rem = buf.len() - off;
        let pad_cap = max_trailing_zero_pad.max(len_sz.saturating_sub(1));
        rem <= pad_cap && buf[off..].iter().all(|&b| b == 0)
    }
}

/// Strictly validates an H.264 AVCC Access Unit with `nal_len_size`-byte BE length prefixes,
/// `forbidden_zero_bit == 0`, valid `nal_type` (`1..=12`), at least one meaningful NAL
/// (`1 | 5 | 6 | 7 | 8 | 9`), and exact end-of-buffer NALU alignment (`off == buf.len()`).
pub fn valid_avcc_au_strict(buf: &[u8], nal_len_size: usize) -> bool {
    validate_avcc_au_core(buf, nal_len_size, 0, false)
}

/// Returns `true` when `buf` passes `valid_avcc_au_strict` and contains at least one
/// IDR (`5`), SPS (`7`), or PPS (`8`) NALU.
pub fn avcc_au_has_keyframe_or_params(buf: &[u8], nal_len_size: usize) -> bool {
    if !valid_avcc_au_strict(buf, nal_len_size) {
        return false;
    }
    let len_sz = nal_len_size.clamp(1, 4);
    let mut off = 0usize;
    while off + len_sz <= buf.len() {
        let len = read_be_nal_len(buf, off, len_sz);
        off += len_sz;
        if len == 0 || off + len > buf.len() {
            return false;
        }
        if matches!(buf[off] & 0x1F, 5 | 7 | 8) {
            return true;
        }
        off += len;
    }
    false
}

/// Relaxed validation for continuous CTR-decrypted iOS 16–27 AirPlay AVCC AUs: accepts `nal_type`
/// `1..=12 | 19 | 20` and tolerates up to `16` bytes of trailing `0x00` alignment padding so
/// valid video packets NEVER get misclassified as keystream desyncs.
pub fn valid_avcc_au_relaxed(buf: &[u8], nal_len_size: usize) -> bool {
    validate_avcc_au_core(buf, nal_len_size, MAX_RELAXED_TRAILING_ZERO_PAD, true)
}

/// Validates a `nal_len_size`-byte BE length-prefixed H.264 AVCC Access Unit using relaxed
/// continuous-CTR rules (up to 16 B trailing `0x00` padding and extension NALUs `19 | 20`).
pub fn valid_avcc_au_with_len_size(buf: &[u8], nal_len_size: usize) -> bool {
    valid_avcc_au_relaxed(buf, nal_len_size)
}

/// Strips trailing all-`0x00` alignment padding after the last intact NALU so downstream MFT /
/// Annex-B converters never see bogus zero-length NAL headers.
pub fn strip_trailing_zero_padding_avcc(buf: &[u8], nal_len_size: usize) -> &[u8] {
    let len_sz = nal_len_size.clamp(1, 4);
    let mut off = 0usize;
    while off + len_sz <= buf.len() {
        let len = read_be_nal_len(buf, off, len_sz);
        if len == 0 {
            if buf[off..].iter().all(|&b| b == 0) {
                return &buf[..off];
            }
            return buf;
        }
        let next = off + len_sz + len;
        if next > buf.len() {
            return buf;
        }
        off = next;
    }
    if off < buf.len() && buf[off..].iter().all(|&b| b == 0) {
        &buf[..off]
    } else {
        buf
    }
}

/// Validates that `buf` is a well-formed 4-byte big-endian length-prefixed H.264 AVCC Access Unit.
pub fn valid_avcc_au(buf: &[u8]) -> bool {
    valid_avcc_au_with_len_size(buf, 4)
}

#[derive(Debug, PartialEq, Eq)]
pub enum NormalizedAvccOutcome {
    Video { payload: Vec<u8>, is_keyframe: bool },
    NonVclOnly,
    Invalid,
}

/// Validates every 4-byte BE length-prefixed NALU in `avcc` (stripping trailing `<= 16` B `0x00`
/// alignment padding), strips SEI (`6`), AUD (`9`), and Filler (`12`), preserves SPS (`7`),
/// PPS (`8`), IDR (`5`), and slice (`1 | 19 | 20`) NALUs intact, and classifies non-VCL-only AUs.
pub fn normalize_avcc_au_classified(avcc: &[u8]) -> NormalizedAvccOutcome {
    let clean_slice = strip_trailing_zero_padding_avcc(avcc, 4);
    if !valid_avcc_au(clean_slice) {
        return NormalizedAvccOutcome::Invalid;
    }
    let mut out = Vec::with_capacity(clean_slice.len());
    let mut is_keyframe = false;
    let mut off = 0usize;

    while off + 4 <= clean_slice.len() {
        let len = u32::from_be_bytes([
            clean_slice[off],
            clean_slice[off + 1],
            clean_slice[off + 2],
            clean_slice[off + 3],
        ]) as usize;
        if len == 0 {
            if clean_slice[off..].iter().all(|&b| b == 0) {
                break;
            }
            return NormalizedAvccOutcome::Invalid;
        }
        off += 4;
        if off + len > clean_slice.len() {
            return NormalizedAvccOutcome::Invalid;
        }
        let nal = &clean_slice[off..off + len];
        off += len;
        let nal_type = nal[0] & 0x1F;
        if nal_type == 6 || nal_type == 9 || nal_type == 12 {
            // Strip SEI (6), AUD (9), & Filler (12) NALUs so WebCodecs/MFT H.264 decoder never stalls.
            continue;
        }
        if nal_type == 5 {
            is_keyframe = true;
        }
        out.extend_from_slice(&(len as u32).to_be_bytes());
        out.extend_from_slice(nal);
    }

    if off < clean_slice.len() && !clean_slice[off..].iter().all(|&b| b == 0) {
        return NormalizedAvccOutcome::Invalid;
    }
    if out.is_empty() {
        return NormalizedAvccOutcome::NonVclOnly;
    }
    NormalizedAvccOutcome::Video {
        payload: out,
        is_keyframe,
    }
}

/// Validates every 4-byte BE length-prefixed NALU in `avcc`, strips SEI (`6`), AUD (`9`), and
/// Filler (`12`), rejects truncated or trailing corrupt bytes (`None`), and returns
/// `Some((clean_avcc_payload, is_keyframe))` where `is_keyframe == true` iff any NALU is IDR (`5`).
pub fn normalize_avcc_au_for_webcodecs(avcc: &[u8]) -> Option<(Vec<u8>, bool)> {
    match normalize_avcc_au_classified(avcc) {
        NormalizedAvccOutcome::Video {
            payload,
            is_keyframe,
        } => Some((payload, is_keyframe)),
        NormalizedAvccOutcome::NonVclOnly | NormalizedAvccOutcome::Invalid => None,
    }
}

/// Backwards-compatible wrapper over `normalize_avcc_au_for_webcodecs`.
pub fn filter_avcc_and_detect_idr(avcc: &[u8]) -> (Vec<u8>, bool) {
    normalize_avcc_au_for_webcodecs(avcc).unwrap_or_else(|| (Vec::new(), false))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedAvcC {
    pub codec: String,
    pub avcc_b64: String,
    pub width: u32,
    pub height: u32,
    pub nal_len_size: u8,
}

/// Parses an ISO/IEC 14496-15 `AVCDecoderConfigurationRecord` (`avcC`) received in AirPlay
/// `kind == 1` packets, verifying `lengthSizeMinusOne` and extracting exact SPS dimensions.
pub fn parse_avcc_config(avcc: &[u8], fallback_w: u32, fallback_h: u32) -> Option<ParsedAvcC> {
    if avcc.len() < 7 || avcc[0] != 1 {
        return None;
    }
    let profile = avcc[1];
    let compat = avcc[2];
    let level = avcc[3];
    let nal_len_size = (avcc[4] & 0x03) + 1;
    let codec = format!("avc1.{profile:02X}{compat:02X}{level:02X}");

    let mut dims = None;
    let num_sps = (avcc[5] & 0x1F) as usize;
    if num_sps > 0 && avcc.len() >= 8 {
        let sps_len = u16::from_be_bytes([avcc[6], avcc[7]]) as usize;
        if sps_len > 1 && 8 + sps_len <= avcc.len() {
            dims = parse_h264_sps_dimensions(&avcc[8..8 + sps_len]);
        }
    }

    let (w, h) = dims.unwrap_or_else(|| {
        let fw = if fallback_w >= 64 {
            fallback_w
        } else {
            DISPLAY_SIZE.0
        };
        let fh = if fallback_h >= 64 {
            fallback_h
        } else {
            DISPLAY_SIZE.1
        };
        (fw, fh)
    });

    Some(ParsedAvcC {
        codec,
        avcc_b64: base64_encode(avcc),
        width: w,
        height: h,
        nal_len_size,
    })
}

/// Minimal ITU-T H.264 SPS RBSP dimension parser (supports Baseline/Main/High profiles & cropping).
pub fn parse_h264_sps_dimensions(sps_nal: &[u8]) -> Option<(u32, u32)> {
    if sps_nal.len() < 4 || (sps_nal[0] & 0x1F) != 7 {
        return None;
    }
    let mut rbsp = Vec::with_capacity(sps_nal.len());
    let payload = &sps_nal[1..];
    let mut i = 0usize;
    while i < payload.len() {
        if i + 2 < payload.len() && payload[i] == 0 && payload[i + 1] == 0 && payload[i + 2] == 3 {
            rbsp.push(0);
            rbsp.push(0);
            i += 3;
        } else {
            rbsp.push(payload[i]);
            i += 1;
        }
    }

    struct BitReader<'a> {
        data: &'a [u8],
        bit_pos: usize,
    }
    impl<'a> BitReader<'a> {
        fn read_bits(&mut self, n: usize) -> Option<u32> {
            if n == 0 {
                return Some(0);
            }
            if n > 32 || self.bit_pos + n > self.data.len() * 8 {
                return None;
            }
            let mut val = 0u32;
            for _ in 0..n {
                let byte = self.data[self.bit_pos / 8];
                let bit = (byte >> (7 - (self.bit_pos % 8))) & 1;
                val = (val << 1) | u32::from(bit);
                self.bit_pos += 1;
            }
            Some(val)
        }
        fn read_ue(&mut self) -> Option<u32> {
            let mut zeros = 0usize;
            while self.read_bits(1)? == 0 {
                zeros += 1;
                if zeros > 31 {
                    return None;
                }
            }
            let suffix = self.read_bits(zeros)?;
            Some(((1u32 << zeros) - 1).checked_add(suffix)?)
        }
        fn read_se(&mut self) -> Option<i32> {
            let u = self.read_ue()?;
            let signed = if (u & 1) != 0 {
                ((u + 1) / 2) as i32
            } else {
                -((u / 2) as i32)
            };
            Some(signed)
        }
    }

    let mut br = BitReader {
        data: &rbsp,
        bit_pos: 0,
    };
    let profile_idc = br.read_bits(8)?;
    let _constraints = br.read_bits(8)?;
    let _level_idc = br.read_bits(8)?;
    let _sps_id = br.read_ue()?;

    let mut chroma_format_idc = 1u32;
    let mut separate_colour_plane_flag = 0u32;
    if matches!(
        profile_idc,
        100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135
    ) {
        chroma_format_idc = br.read_ue()?;
        if chroma_format_idc == 3 {
            separate_colour_plane_flag = br.read_bits(1)?;
        }
        let _bit_depth_luma = br.read_ue()?;
        let _bit_depth_chroma = br.read_ue()?;
        let _qpprime = br.read_bits(1)?;
        let seq_scaling_matrix_present = br.read_bits(1)?;
        if seq_scaling_matrix_present == 1 {
            let count = if chroma_format_idc != 3 { 8 } else { 12 };
            for idx in 0..count {
                if br.read_bits(1)? == 1 {
                    let size = if idx < 6 { 16 } else { 64 };
                    let mut last_scale = 8i32;
                    let mut next_scale = 8i32;
                    for _ in 0..size {
                        if next_scale != 0 {
                            let delta = br.read_se()?;
                            next_scale = (last_scale + delta + 256) % 256;
                        }
                        last_scale = if next_scale == 0 {
                            last_scale
                        } else {
                            next_scale
                        };
                    }
                }
            }
        }
    }

    let _log2_max_frame_num_minus4 = br.read_ue()?;
    let pic_order_cnt_type = br.read_ue()?;
    if pic_order_cnt_type == 0 {
        let _log2_max_poc = br.read_ue()?;
    } else if pic_order_cnt_type == 1 {
        let _delta = br.read_bits(1)?;
        let _off_nr = br.read_se()?;
        let _off_tb = br.read_se()?;
        let num_ref = br.read_ue()?;
        for _ in 0..num_ref.min(256) {
            let _ = br.read_se()?;
        }
    }
    let _max_num_ref_frames = br.read_ue()?;
    let _gaps = br.read_bits(1)?;
    let pic_width_in_mbs_minus1 = br.read_ue()?;
    let pic_height_in_map_units_minus1 = br.read_ue()?;
    let frame_mbs_only_flag = br.read_bits(1)?;
    if frame_mbs_only_flag == 0 {
        let _mb_adaptive = br.read_bits(1)?;
    }
    let _direct_8x8 = br.read_bits(1)?;
    let frame_cropping_flag = br.read_bits(1)?;
    let (crop_l, crop_r, crop_t, crop_b) = if frame_cropping_flag == 1 {
        (br.read_ue()?, br.read_ue()?, br.read_ue()?, br.read_ue()?)
    } else {
        (0, 0, 0, 0)
    };

    let chroma_array_type = if separate_colour_plane_flag == 0 {
        chroma_format_idc
    } else {
        0
    };
    let (crop_unit_x, crop_unit_y) = match chroma_array_type {
        0 => (1, 2 - frame_mbs_only_flag),
        1 => (2, 2 * (2 - frame_mbs_only_flag)),
        2 => (2, 2 - frame_mbs_only_flag),
        3 => (1, 2 - frame_mbs_only_flag),
        _ => (1, 1),
    };

    let raw_w = (pic_width_in_mbs_minus1 + 1).checked_mul(16)?;
    let raw_h = (2 - frame_mbs_only_flag)
        .checked_mul(pic_height_in_map_units_minus1 + 1)?
        .checked_mul(16)?;
    let width = raw_w.checked_sub((crop_l + crop_r).checked_mul(crop_unit_x)?)?;
    let height = raw_h.checked_sub((crop_t + crop_b).checked_mul(crop_unit_y)?)?;
    if width >= 64 && height >= 64 && width <= 8192 && height <= 8192 {
        Some((width, height))
    } else {
        None
    }
}

fn read_exact_stoppable(
    stream: &mut TcpStream,
    buf: &mut [u8],
    stop: &AtomicBool,
) -> io::Result<bool> {
    let mut read_so_far = 0usize;
    while read_so_far < buf.len() {
        if stop.load(Ordering::Relaxed) {
            return Ok(false);
        }
        match stream.read(&mut buf[read_so_far..]) {
            Ok(0) => return Ok(false),
            Ok(n) => read_so_far += n,
            Err(e)
                if e.kind() == io::ErrorKind::WouldBlock
                    || e.kind() == io::ErrorKind::TimedOut
                    || e.kind() == io::ErrorKind::Interrupted =>
            {
                let _ = wait_tcp_readable(stream, 50);
                continue;
            }
            Err(e) => return Err(e),
        }
    }
    Ok(true)
}

struct MirrorLoopAliveGuard {
    sink: Arc<dyn AirPlaySink>,
}

impl MirrorLoopAliveGuard {
    fn new(sink: Arc<dyn AirPlaySink>) -> Self {
        sink.on_mirror_loop_state(true);
        Self { sink }
    }
}

impl Drop for MirrorLoopAliveGuard {
    fn drop(&mut self) {
        self.sink.on_mirror_loop_state(false);
    }
}

fn extract_header_fallback_dims(hdr: &[u8; 128]) -> (u32, u32) {
    let parse_pair = |off: usize| -> Option<(u32, u32)> {
        let w_f = f32::from_le_bytes([hdr[off], hdr[off + 1], hdr[off + 2], hdr[off + 3]]);
        let h_f = f32::from_le_bytes([hdr[off + 4], hdr[off + 5], hdr[off + 6], hdr[off + 7]]);
        if w_f.is_finite() && h_f.is_finite() && w_f >= 64.0 && h_f >= 64.0 && w_f <= 8192.0 && h_f <= 8192.0 {
            Some((w_f.round() as u32, h_f.round() as u32))
        } else {
            None
        }
    };
    parse_pair(56)
        .or_else(|| parse_pair(40))
        .unwrap_or(DISPLAY_SIZE)
}

fn process_single_mirror_stream(
    mut stream: TcpStream,
    stream_key: [u8; 16],
    stream_iv: [u8; 16],
    sink: &Arc<dyn AirPlaySink>,
    stop: &AtomicBool,
    conn_seq: u64,
) {
    // Tahap 255 FIX: Disable delayed ACK (TCP_NODELAY + SIO_TCP_SET_ACK_FREQUENCY=1)
    // and use non-blocking TCP + Win32 WSAPoll (0 SO_RCVTIMEO IRP loss, 0 FD_SETSIZE cap!).
    configure_low_latency_std_tcp(&stream);
    let _ = stream.set_nonblocking(true);
    let _alive_guard = MirrorLoopAliveGuard::new(sink.clone());
    record_airplay_trace("mirror_accept", format!("conn=#{conn_seq}"));
    sink.on_state(AirPlayState::Connected);

    let Ok(mut decryptor) = MirrorDecryptor::new(stream_key, stream_iv) else {
        record_airplay_trace("mirror_eof", format!("conn=#{conn_seq} ctr_init_err"));
        return;
    };

    let mut hdr = [0u8; 128];
    let mut payload = Vec::new();
    let mut first_frame_sent = false;
    let mut k0_count: u64 = 0;

    while !stop.load(Ordering::Relaxed) {
        match read_exact_stoppable(&mut stream, &mut hdr, stop) {
            Ok(true) => {}
            Ok(false) => {
                record_airplay_trace("mirror_eof", format!("conn=#{conn_seq} eof_or_stop k0={k0_count}"));
                break;
            }
            Err(e) => {
                record_airplay_trace("mirror_eof", format!("conn=#{conn_seq} hdr_err={:?} k0={k0_count}", e.kind()));
                break;
            }
        }

        let size = u32::from_le_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]) as usize;
        // Match both zenlink-airplay (`header[4]`) & handset/airplay.py (`u16_le(header[4..6]) & 0xFF`).
        let kind = (u16::from_le_bytes([hdr[4], hdr[5]]) & 0xFF) as u8;
        sink.on_tcp_packet(kind);

        if size == 0 {
            if kind == 2 || kind == 5 {
                record_airplay_trace(if kind == 2 { "pkt_k2" } else { "pkt_k5" }, "len=0");
            }
            continue;
        }
        if size > MAX_MIRROR_PACKET_BYTES {
            record_airplay_trace("mirror_eof", format!("conn=#{conn_seq} oversize={size}"));
            break;
        }

        payload.resize(size, 0);
        match read_exact_stoppable(&mut stream, &mut payload, stop) {
            Ok(true) => {}
            Ok(false) => {
                record_airplay_trace("mirror_eof", format!("conn=#{conn_seq} body_eof len={size} k0={k0_count}"));
                break;
            }
            Err(e) => {
                record_airplay_trace("mirror_eof", format!("conn=#{conn_seq} body_err={:?} len={size}", e.kind()));
                break;
            }
        }

        match kind {
            1 => {
                // Unencrypted avcC codec header (SPS + PPS) — NEVER advances CTR keystream.
                let (fallback_w, fallback_h) = extract_header_fallback_dims(&hdr);
                if let Some(cfg) = parse_avcc_config(&payload, fallback_w, fallback_h) {
                    decryptor.set_nal_len_size(cfg.nal_len_size as usize);
                    super::inc_trace_avcc_updates();
                    record_airplay_trace(
                        "pkt_k1",
                        format!("len={size} codec={} {}x{}", cfg.codec, cfg.width, cfg.height),
                    );
                    sink.on_codec(&cfg.codec, &cfg.avcc_b64, cfg.width, cfg.height);
                } else {
                    record_airplay_trace("pkt_k1", format!("len={size} parse_none"));
                }
            }
            0 => {
                // Encrypted H.264 AVCC Access Unit — ALWAYS advances continuous AES-128-CTR by exact `size`.
                k0_count = k0_count.saturating_add(1);
                super::inc_trace_tcp_video_packets();
                match decryptor.decrypt_in_place(&mut payload) {
                    Ok(true) => {}
                    Ok(false) => {
                        record_airplay_trace(
                            "decrypt_fail",
                            format!("k0=#{k0_count} len={size} bad={}", decryptor.bad_count),
                        );
                        sink.on_decrypt_failure();
                        continue;
                    }
                    Err(_) => {
                        record_airplay_trace("mirror_eof", format!("conn=#{conn_seq} ctr_err"));
                        break;
                    }
                }
                match normalize_avcc_au_classified(&payload) {
                    NormalizedAvccOutcome::Video {
                        payload: clean_avcc,
                        is_keyframe,
                    } => {
                        if k0_count <= 12 || k0_count % 60 == 0 || is_keyframe {
                            let nals = summarize_avcc_nal_types(&clean_avcc);
                            record_airplay_trace(
                                "pkt_k0",
                                format!("k0=#{k0_count} len={size} key={is_keyframe} nals={nals}"),
                            );
                        }
                        if !first_frame_sent {
                            first_frame_sent = true;
                            sink.on_state(AirPlayState::Streaming);
                        }
                        sink.on_frame(clean_avcc, is_keyframe);
                    }
                    NormalizedAvccOutcome::NonVclOnly => {
                        if k0_count <= 12 {
                            record_airplay_trace("pkt_k0", format!("k0=#{k0_count} len={size} non_vcl_skip"));
                        }
                    }
                    NormalizedAvccOutcome::Invalid => {
                        record_airplay_trace("decrypt_fail", format!("k0=#{k0_count} len={size} norm_invalid"));
                        sink.on_decrypt_failure();
                    }
                }
            }
            2 => {
                record_airplay_trace("pkt_k2", format!("len={size}"));
            }
            5 => {
                record_airplay_trace("pkt_k5", format!("len={size}"));
            }
            other => {
                record_airplay_trace("pkt_other", format!("kind={other} len={size}"));
            }
        }
    }
    if !stop.load(Ordering::Relaxed) {
        super::inc_trace_tcp_disconnects();
    }
}

/// Runs the AirPlay Mirror TCP listener on `listener`, spawning a concurrent worker thread for
/// EVERY accepted TCP video connection (1:1 `zenlink-airplay/src/server.rs:451-471`) so iOS
/// opening a 2nd video socket while socket #1 lingers idle NEVER stalls the stream!
pub fn run_mirror_listener(
    listener: TcpListener,
    stream_key: [u8; 16],
    stream_iv: [u8; 16],
    sink: Arc<dyn AirPlaySink>,
    stop: Arc<AtomicBool>,
) {
    let _ = listener.set_nonblocking(true);
    let mut workers: Vec<JoinHandle<()>> = Vec::new();
    let mut conn_seq: u64 = 0;
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                conn_seq = conn_seq.saturating_add(1);
                let seq = conn_seq;
                let sink_c = sink.clone();
                let stop_c = stop.clone();
                if let Ok(h) = std::thread::Builder::new()
                    .name("airplay-video-conn".into())
                    .spawn(move || {
                        process_single_mirror_stream(stream, stream_key, stream_iv, &sink_c, &stop_c, seq);
                    })
                {
                    workers.retain(|w| !w.is_finished());
                    workers.push(h);
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(15));
            }
            Err(_) => break,
        }
    }
    for h in workers {
        let _ = h.join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_avcc_parse_and_sei_strip_and_idr_detect() {
        let sps = vec![0x67, 0x42, 0xC0, 0x1F, 0xF4, 0x02, 0x80, 0x2D, 0xD0];
        let pps = vec![0x68, 0xCE, 0x38, 0x80];
        let mut avcc = vec![
            0x01, 0x42, 0xC0, 0x1F, 0xFF, 0xE1,
            0x00, sps.len() as u8,
        ];
        avcc.extend_from_slice(&sps);
        avcc.push(0x01);
        avcc.extend_from_slice(&[0x00, pps.len() as u8]);
        avcc.extend_from_slice(&pps);

        let parsed = parse_avcc_config(&avcc, 1920, 1080).expect("avcC must parse");
        assert_eq!(parsed.codec, "avc1.42C01F");
        assert_eq!((parsed.width, parsed.height), (1280, 720));
        assert_eq!(parsed.nal_len_size, 4);

        // Build AVCC AU containing AUD (nal_type=9) + SEI (nal_type=6) + IDR (nal_type=5)
        let mut au = Vec::new();
        let aud = [0x09u8, 0x10];
        let sei = [0x06u8, 0x05, 0x01, 0xAA];
        let idr = [0x65u8, 0x88, 0x84, 0x21, 0xA0];
        au.extend_from_slice(&(aud.len() as u32).to_be_bytes());
        au.extend_from_slice(&aud);
        au.extend_from_slice(&(sei.len() as u32).to_be_bytes());
        au.extend_from_slice(&sei);
        au.extend_from_slice(&(idr.len() as u32).to_be_bytes());
        au.extend_from_slice(&idr);

        assert!(valid_avcc_au(&au));
        let (clean, is_key) = normalize_avcc_au_for_webcodecs(&au).expect("valid AU");
        assert!(is_key);
        assert_eq!(clean.len(), 4 + idr.len());
        assert_eq!(&clean[4..], &idr);

        // Trailing corrupt byte must be rejected cleanly
        let mut corrupt = au.clone();
        corrupt.push(0xFF);
        assert!(normalize_avcc_au_for_webcodecs(&corrupt).is_none());
    }
}
