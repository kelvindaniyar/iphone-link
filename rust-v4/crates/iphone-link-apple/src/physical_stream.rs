//! Pure-Rust physical iPhone USB CoreDevice DisplayService streamer (`00008150-...`).
//!
//! Opens `CoreDeviceProxy` -> `CDTunnel` -> in-process IPv6 TCP/UDP demuxer ->
//! HTTP/2 + binary RemoteXPC `DisplayService` (`startmediastream` / `stopmediastream`)
//! -> RFC 7798 HEVC RTP depacketizer + RTCP RCTL/RR/PLI feedback loop.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::ffi::c_void;
use std::net::{Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
use std::time::{Duration, Instant};

use idevice::core_device::{
    build_hvcc_record, build_keyframe_request, build_rtcp_rr_sdes_remb,
    hevc_codec_string_from_sps, is_rtcp, parse_hevc_sps_dimensions_and_trim,
    parse_rtcp_sender_report, split_annexb_nalus, FrameTrim, HevcDepacketizer, ReportBlock,
    RtpPacket,
};
use idevice::core_device_proxy::CoreDeviceProxy;
use idevice::lockdown::LockdownClient;
use idevice::provider::{PairingFile, UsbmuxdProvider};
use idevice::usbmuxd::{Connection, UsbmuxdAddr};
use idevice::Idevice;

use crate::link::{runtime, usbmux};
use crate::mirror::{extend_seq, Picker};

const RECEIVER_VIDEO_PORT: u16 = 50001;
const XPC_WRAPPER_MAGIC: u32 = 0x29B0_0B92;
const XPC_PAYLOAD_MAGIC: u32 = 0x4213_3742;
const XPC_PAYLOAD_PROTOCOL_VERSION: u32 = 5;

const XPC_FLAG_ALWAYS_SET: u32 = 0x0000_0001;
const XPC_FLAG_DATA_PRESENT: u32 = 0x0000_0100;
const XPC_FLAG_WANTING_REPLY: u32 = 0x0001_0000;
const XPC_FLAG_INIT_HANDSHAKE: u32 = 0x0040_0000;

const XPC_TYPE_NULL: u32 = 0x0000_1000;
const XPC_TYPE_BOOL: u32 = 0x0000_2000;
const XPC_TYPE_INT64: u32 = 0x0000_3000;
const XPC_TYPE_UINT64: u32 = 0x0000_4000;
const XPC_TYPE_DOUBLE: u32 = 0x0000_5000;
const XPC_TYPE_DATE: u32 = 0x0000_7000;
const XPC_TYPE_DATA: u32 = 0x0000_8000;
const XPC_TYPE_STRING: u32 = 0x0000_9000;
const XPC_TYPE_UUID: u32 = 0x0000_A000;
const XPC_TYPE_ARRAY: u32 = 0x0000_E000;
const XPC_TYPE_DICT: u32 = 0x0000_F000;

const TEMPLATE_CALL_UUID: &[u8] = b"11111111-2222-4333-8444-555555555555";

/// Decompressed protobuf template for `avcMediaStreamNegotiatorMediaBlob` (273 bytes).
/// Field 5 (offset 4: tag `0x2a`, len `0x7a` = 122) starts with Field 1 (`0x08` + 5-byte varint `session_id` at `pb[7..12]`).
const OFFER_MEDIA_BLOB_PROTOBUF_HEX: &str = concat!(
    "080110012a7a088182bae90810001a3f087b120a0801100118c387032000120a0801100218c387032000",
    "120a0801100118c387032000120a0801100218c3870320001a09464c533b53573a313b20011a27086412",
    "0a0801100118c387032000120a0801100218c3870320001a09464c533b53573a313b200e3800403f5001",
    "6001320d56696365726f7920312e372e3040004a0908ea1f1000188080014a0b080010c0d1e123188080",
    "204a0a08001080b489131880604a0508101084204a0b08001080dac409188080064a05080410e4324a0b",
    "080010809bee02188080084a0b08001080c2d72f188080404a0b080010808ece1c188080104a05080110",
    "ab026880c0dd87d2a0c0e9ed017002800100900101"
);

const OFFER_REMOTE_ENDPOINT_INFO_HEX: &str =
    "080010011a074d616331352c392208323230352e332e312a053235463830";

fn hex_decode(s: &str) -> Vec<u8> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks_exact(2) {
        let hi = (pair[0] as char).to_digit(16).unwrap_or(0) as u8;
        let lo = (pair[1] as char).to_digit(16).unwrap_or(0) as u8;
        out.push((hi << 4) | lo);
    }
    out
}

fn new_random_uuid_bytes_and_str() -> ([u8; 16], String) {
    static SEQ: AtomicU64 = AtomicU64::new(0x1357_2468_9ABC_DEF0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1);
    let s = SEQ.fetch_add(1, Ordering::Relaxed);
    let a = nanos.wrapping_mul(6364136223846793005).wrapping_add(s);
    let b = a.rotate_left(23) ^ s.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let mut raw = [0u8; 16];
    raw[..8].copy_from_slice(&a.to_be_bytes());
    raw[8..].copy_from_slice(&b.to_be_bytes());
    raw[6] = (raw[6] & 0x0F) | 0x40;
    raw[8] = (raw[8] & 0x3F) | 0x80;
    let uuid_str = format!(
        "{:02X}{:02X}{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
        raw[0], raw[1], raw[2], raw[3],
        raw[4], raw[5],
        raw[6], raw[7],
        raw[8], raw[9],
        raw[10], raw[11], raw[12], raw[13], raw[14], raw[15],
    );
    (raw, uuid_str)
}

/// Generates a cryptographically fresh non-zero per-session SSRC (`session_id`)
/// using Windows CNG (`BCryptGenRandom`) mixed with monotonic entropy.
pub fn generate_random_session_id() -> u32 {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let mut rng_bytes = [0u8; 4];
    #[cfg(windows)]
    let _ = crate::win_cng::fill_random(&mut rng_bytes);
    let cng_val = u32::from_ne_bytes(rng_bytes);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mixed = cng_val
        ^ (nanos as u32)
        ^ ((nanos >> 32) as u32).rotate_left(13)
        ^ (seq as u32).wrapping_mul(0x9E37_79B1);
    let candidate = if mixed == 0 || mixed == OFFER_TEMPLATE_LOCAL_SSRC {
        mixed.wrapping_add(0x1357_2469) | 0x1000_0001
    } else {
        mixed
    };
    if candidate == 0 { 0x2468_ACE1 } else { candidate }
}

fn encode_protobuf_varint_u32(mut v: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(5);
    loop {
        let mut b = (v & 0x7F) as u8;
        v >>= 7;
        if v != 0 {
            b |= 0x80;
            out.push(b);
        } else {
            out.push(b);
            break;
        }
    }
    out
}

/// Computes RFC 1950 Adler-32 checksum.
pub fn adler32(data: &[u8]) -> u32 {
    const MOD_ADLER: u32 = 65521;
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for &byte in data {
        a = (a + u32::from(byte)) % MOD_ADLER;
        b = (b + a) % MOD_ADLER;
    }
    (b << 16) | a
}

/// Wraps `raw` payload in a standard RFC 1950 zlib stream using DEFLATE stored blocks
/// (`CMF=0x78`, `FLG=0x01`, `BTYPE=00`, big-endian `Adler-32`) with zero external crates.
pub fn zlib_wrap_stored(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len() + 16);
    out.push(0x78); // CM=8 (deflate), CINFO=7 (32K window)
    out.push(0x01); // FCHECK valid: (0x78*256 + 0x01) % 31 == 0
    if raw.is_empty() {
        out.push(0x01); // BFINAL=1, BTYPE=00
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&(!0u16).to_le_bytes());
    } else {
        let chunks: Vec<&[u8]> = raw.chunks(65535).collect();
        for (idx, chunk) in chunks.iter().enumerate() {
            let is_final = idx + 1 == chunks.len();
            out.push(if is_final { 0x01 } else { 0x00 });
            let len = chunk.len() as u16;
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&(!len).to_le_bytes());
            out.extend_from_slice(chunk);
        }
    }
    out.extend_from_slice(&adler32(raw).to_be_bytes());
    out
}

/// Decompresses a stored-block RFC 1950 zlib stream and validates its Adler-32 checksum.
pub fn zlib_unwrap_stored(zlib_data: &[u8]) -> Result<Vec<u8>, String> {
    if zlib_data.len() < 11 {
        return Err("zlib stream too short".into());
    }
    let cmf = zlib_data[0];
    let flg = zlib_data[1];
    if (cmf & 0x0F) != 8 || (((u16::from(cmf) << 8) | u16::from(flg)) % 31) != 0 {
        return Err("invalid zlib header".into());
    }
    let mut pos = 2usize;
    let mut out = Vec::new();
    loop {
        if pos + 5 > zlib_data.len().saturating_sub(4) {
            return Err("truncated deflate stored block header".into());
        }
        let hdr = zlib_data[pos];
        let bfinal = (hdr & 0x01) != 0;
        let btype = (hdr >> 1) & 0x03;
        if btype != 0 {
            return Err(format!("unsupported deflate block type {btype}"));
        }
        let len = u16::from_le_bytes([zlib_data[pos + 1], zlib_data[pos + 2]]) as usize;
        let nlen = u16::from_le_bytes([zlib_data[pos + 3], zlib_data[pos + 4]]);
        if (len as u16) != !nlen {
            return Err("corrupt deflate stored block NLEN".into());
        }
        pos += 5;
        if pos + len > zlib_data.len().saturating_sub(4) {
            return Err("truncated deflate stored block payload".into());
        }
        out.extend_from_slice(&zlib_data[pos..pos + len]);
        pos += len;
        if bfinal {
            break;
        }
    }
    if pos + 4 != zlib_data.len() {
        return Err("trailing bytes after zlib Adler-32".into());
    }
    let expected = u32::from_be_bytes([
        zlib_data[pos],
        zlib_data[pos + 1],
        zlib_data[pos + 2],
        zlib_data[pos + 3],
    ]);
    let actual = adler32(&out);
    if actual != expected {
        return Err(format!(
            "Adler-32 mismatch: expected 0x{expected:08x}, got 0x{actual:08x}"
        ));
    }
    Ok(out)
}

fn patch_offer_media_blob_session_id(session_id: u32) -> Vec<u8> {
    let base = hex_decode(OFFER_MEDIA_BLOB_PROTOBUF_HEX);
    // Template structure:
    // base[0..4] = 08 01 10 01 (fields 1 & 2)
    // base[4]    = 0x2a (field 5, wire type 2)
    // base[5]    = 0x7a (length = 122 bytes, base[6..128])
    // base[6]    = 0x08 (inner field 1 = session_id)
    // base[7..12]= 81 82 ba e9 08 (5-byte varint for 0x8D2E8101)
    let new_varint = encode_protobuf_varint_u32(session_id);
    let orig_sub_len = base[5] as usize; // 122
    let new_sub_len = orig_sub_len - 5 + new_varint.len();
    let mut patched = Vec::with_capacity(base.len() - 5 + new_varint.len());
    patched.extend_from_slice(&base[0..5]);
    // new_sub_len is in 118..=122 (< 128), always a 1-byte varint
    patched.push(new_sub_len as u8);
    patched.push(base[6]); // 0x08
    patched.extend_from_slice(&new_varint);
    patched.extend_from_slice(&base[12..]);
    patched
}

fn write_bplist_length_marker(out: &mut Vec<u8>, high_nibble: u8, len: usize) {
    if len < 15 {
        out.push(high_nibble | (len as u8));
    } else if len <= 0xFF {
        out.push(high_nibble | 0x0F);
        out.push(0x10);
        out.push(len as u8);
    } else {
        out.push(high_nibble | 0x0F);
        out.push(0x11);
        out.extend_from_slice(&(len as u16).to_be_bytes());
    }
}

/// Builds a valid Apple `bplist00` `negotiatorOffer` carrying a specific `session_id`
/// inside a zlib-wrapped `avcMediaStreamNegotiatorMediaBlob` and `call_uuid_str`.
pub fn build_video_offer_bplist_with_session_id(call_uuid_str: &str, session_id: u32) -> Vec<u8> {
    let patched_pb = patch_offer_media_blob_session_id(session_id);
    let media_blob_zlib = zlib_wrap_stored(&patched_pb);
    let remote_ep_info = hex_decode(OFFER_REMOTE_ENDPOINT_INFO_HEX);
    let call_id_bytes = if call_uuid_str.is_empty() {
        TEMPLATE_CALL_UUID
    } else {
        call_uuid_str.as_bytes()
    };

    let keys: [&[u8]; 4] = [
        b"avcMediaStreamNegotiatorMediaBlob",
        b"avcMediaStreamNegotiatorMode",
        b"avcMediaStreamOptionCallID",
        b"avcMediaStreamOptionRemoteEndpointInfo",
    ];

    let mut out = Vec::with_capacity(512);
    out.extend_from_slice(b"bplist00");
    let mut offsets: Vec<u16> = Vec::with_capacity(9);

    // Obj 0: Root dictionary with 4 entries (keys 1..=4, values 5..=8)
    offsets.push(out.len() as u16);
    out.extend_from_slice(&[0xD4, 1, 2, 3, 4, 5, 6, 7, 8]);

    // Obj 1..=4: ASCII keys
    for k in keys {
        offsets.push(out.len() as u16);
        write_bplist_length_marker(&mut out, 0x50, k.len());
        out.extend_from_slice(k);
    }

    // Obj 5: Data (zlib-wrapped avcMediaStreamNegotiatorMediaBlob)
    offsets.push(out.len() as u16);
    write_bplist_length_marker(&mut out, 0x40, media_blob_zlib.len());
    out.extend_from_slice(&media_blob_zlib);

    // Obj 6: Int (avcMediaStreamNegotiatorMode = 5)
    offsets.push(out.len() as u16);
    out.extend_from_slice(&[0x10, 0x05]);

    // Obj 7: ASCII string (avcMediaStreamOptionCallID)
    offsets.push(out.len() as u16);
    write_bplist_length_marker(&mut out, 0x50, call_id_bytes.len());
    out.extend_from_slice(call_id_bytes);

    // Obj 8: Data (avcMediaStreamOptionRemoteEndpointInfo)
    offsets.push(out.len() as u16);
    write_bplist_length_marker(&mut out, 0x40, remote_ep_info.len());
    out.extend_from_slice(&remote_ep_info);

    // Offset table (2-byte big-endian offsets)
    let offset_table_offset = out.len() as u64;
    for off in &offsets {
        out.extend_from_slice(&off.to_be_bytes());
    }

    // 32-byte bplist00 trailer
    out.extend_from_slice(&[0u8; 6]);
    out.push(2); // offset_int_size = 2 bytes
    out.push(1); // object_ref_size = 1 byte
    out.extend_from_slice(&(offsets.len() as u64).to_be_bytes());
    out.extend_from_slice(&0u64.to_be_bytes()); // top_object = 0
    out.extend_from_slice(&offset_table_offset.to_be_bytes());
    out
}

/// Builds a `negotiatorOffer` `bplist00` with a freshly generated random `session_id` per session.
/// Returns `(bplist_bytes, session_id)`.
pub fn build_video_offer_bplist(call_uuid_str: &str) -> (Vec<u8>, u32) {
    let session_id = generate_random_session_id();
    let bplist = build_video_offer_bplist_with_session_id(call_uuid_str, session_id);
    (bplist, session_id)
}

/// Translates CoreDevice DisplayService errors (notably code `9022` when iPhone is busy
/// in a Phone/FaceTime/VoIP call, Camera, or Voice Memos) into a clear user-friendly message.
pub fn translate_coredevice_display_error_bilingual(
    code: Option<u32>,
    raw_msg: &str,
    lang: &str,
) -> String {
    let lower = raw_msg.to_ascii_lowercase();
    let is_9022 = code == Some(9022)
        || raw_msg.contains("9022")
        || (lower.contains("media stream") && (lower.contains("in use") || lower.contains("busy")));
    if is_9022 {
        if lang.eq_ignore_ascii_case("en") {
            return "iPhone is currently busy with a Phone/FaceTime call, Camera, or Voice Memos (code 9022). End the call or close that app, then try again.".to_string();
        }
        return "iPhone sedang digunakan oleh panggilan (Telepon/FaceTime), Kamera, atau Voice Memos (kode 9022). Akhiri panggilan/tutup aplikasi tersebut lalu coba lagi.".to_string();
    }
    let clean = raw_msg.trim();
    if clean.is_empty() {
        if lang.eq_ignore_ascii_case("en") {
            "DisplayService startmediastream was rejected by the iPhone.".to_string()
        } else {
            "DisplayService startmediastream ditolak perangkat.".to_string()
        }
    } else if let Some(c) = code {
        if clean.contains(&c.to_string()) {
            clean.to_string()
        } else {
            format!("{clean} (kode {c})")
        }
    } else {
        clean.to_string()
    }
}

pub fn translate_coredevice_display_error(code: Option<u32>, raw_msg: &str) -> String {
    translate_coredevice_display_error_bilingual(code, raw_msg, "id")
}

#[derive(Debug, Clone)]
enum XpcVal {
    Null,
    Bool(bool),
    Int64(i64),
    Uint64(u64),
    Double(f64),
    Date(i64),
    Data(Vec<u8>),
    Str(String),
    Uuid([u8; 16]),
    Array(Vec<XpcVal>),
    Dict(Vec<(String, XpcVal)>),
}

impl XpcVal {
    fn dict_get(&self, key: &str) -> Option<&XpcVal> {
        let Self::Dict(entries) = self else {
            return None;
        };
        entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }

    fn as_u64(&self) -> Option<u64> {
        match self {
            Self::Uint64(u) => Some(*u),
            Self::Int64(i) if *i >= 0 => Some(*i as u64),
            Self::Str(s) => s.parse::<u64>().ok(),
            _ => None,
        }
    }

    /// Lossless 32-bit integer extraction for XPC fields like `RemoteSSRC` / `LocalSSRC` / `SourcePort`
    /// where Apple's XPC encoder may serialize values with bit 31 set as negative `Int64`, `Uint64`,
    /// `Double`, or wrapped inside `{"int": ...}` / `{"uint": ...}`.
    fn as_u32_lossless(&self) -> Option<u32> {
        match self {
            Self::Uint64(u) => Some(*u as u32),
            Self::Int64(i) => Some((*i as u64) as u32),
            Self::Double(d) if d.is_finite() => Some((*d as i64) as u32),
            Self::Str(s) => s
                .parse::<u64>()
                .ok()
                .map(|v| v as u32)
                .or_else(|| s.parse::<i64>().ok().map(|v| v as u32)),
            Self::Dict(_) => self
                .dict_get("uint")
                .or_else(|| self.dict_get("int"))
                .or_else(|| self.dict_get("unsignedInteger"))
                .or_else(|| self.dict_get("integer"))
                .and_then(XpcVal::as_u32_lossless),
            _ => None,
        }
    }

    fn find_key_recursive<'a>(&'a self, target: &str) -> Option<&'a XpcVal> {
        match self {
            Self::Dict(entries) => {
                if let Some((_, v)) = entries.iter().find(|(k, _)| k == target) {
                    return Some(v);
                }
                for (_, v) in entries {
                    if let Some(found) = v.find_key_recursive(target) {
                        return Some(found);
                    }
                }
                None
            }
            Self::Array(items) => {
                for item in items {
                    if let Some(found) = item.find_key_recursive(target) {
                        return Some(found);
                    }
                }
                None
            }
            _ => None,
        }
    }
}

fn pad4(out: &mut Vec<u8>, len: usize) {
    let rem = len % 4;
    if rem != 0 {
        for _ in 0..(4 - rem) {
            out.push(0);
        }
    }
}

fn write_cstring_aligned(out: &mut Vec<u8>, s: &str) {
    let b = s.as_bytes();
    out.extend_from_slice(b);
    out.push(0);
    pad4(out, b.len() + 1);
}

fn encode_xpc_object(out: &mut Vec<u8>, val: &XpcVal) {
    match val {
        XpcVal::Null => out.extend_from_slice(&XPC_TYPE_NULL.to_le_bytes()),
        XpcVal::Bool(b) => {
            out.extend_from_slice(&XPC_TYPE_BOOL.to_le_bytes());
            out.extend_from_slice(&(if *b { 1u32 } else { 0u32 }).to_le_bytes());
        }
        XpcVal::Int64(v) => {
            out.extend_from_slice(&XPC_TYPE_INT64.to_le_bytes());
            out.extend_from_slice(&v.to_le_bytes());
        }
        XpcVal::Uint64(v) => {
            out.extend_from_slice(&XPC_TYPE_UINT64.to_le_bytes());
            out.extend_from_slice(&v.to_le_bytes());
        }
        XpcVal::Double(v) => {
            out.extend_from_slice(&XPC_TYPE_DOUBLE.to_le_bytes());
            out.extend_from_slice(&v.to_le_bytes());
        }
        XpcVal::Date(v) => {
            out.extend_from_slice(&XPC_TYPE_DATE.to_le_bytes());
            out.extend_from_slice(&v.to_le_bytes());
        }
        XpcVal::Data(bytes) => {
            out.extend_from_slice(&XPC_TYPE_DATA.to_le_bytes());
            out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            out.extend_from_slice(bytes);
            pad4(out, bytes.len());
        }
        XpcVal::Str(s) => {
            out.extend_from_slice(&XPC_TYPE_STRING.to_le_bytes());
            let b = s.as_bytes();
            let with_nul = b.len() + 1;
            out.extend_from_slice(&(with_nul as u32).to_le_bytes());
            out.extend_from_slice(b);
            out.push(0);
            pad4(out, with_nul);
        }
        XpcVal::Uuid(raw) => {
            out.extend_from_slice(&XPC_TYPE_UUID.to_le_bytes());
            out.extend_from_slice(raw);
        }
        XpcVal::Array(items) => {
            out.extend_from_slice(&XPC_TYPE_ARRAY.to_le_bytes());
            let mut body = Vec::new();
            body.extend_from_slice(&(items.len() as u32).to_le_bytes());
            for item in items {
                encode_xpc_object(&mut body, item);
            }
            out.extend_from_slice(&(body.len() as u32).to_le_bytes());
            out.extend_from_slice(&body);
        }
        XpcVal::Dict(entries) => {
            out.extend_from_slice(&XPC_TYPE_DICT.to_le_bytes());
            let mut body = Vec::new();
            body.extend_from_slice(&(entries.len() as u32).to_le_bytes());
            for (k, v) in entries {
                write_cstring_aligned(&mut body, k);
                encode_xpc_object(&mut body, v);
            }
            out.extend_from_slice(&(body.len() as u32).to_le_bytes());
            out.extend_from_slice(&body);
        }
    }
}

fn build_xpc_wrapper(flags: u32, message_id: u64, payload: Option<&XpcVal>) -> Vec<u8> {
    let mut body = Vec::new();
    if let Some(obj) = payload {
        body.extend_from_slice(&XPC_PAYLOAD_MAGIC.to_le_bytes());
        body.extend_from_slice(&XPC_PAYLOAD_PROTOCOL_VERSION.to_le_bytes());
        encode_xpc_object(&mut body, obj);
    }
    let mut out = Vec::with_capacity(24 + body.len());
    out.extend_from_slice(&XPC_WRAPPER_MAGIC.to_le_bytes());
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&(body.len() as u64).to_le_bytes());
    out.extend_from_slice(&message_id.to_le_bytes());
    out.extend_from_slice(&body);
    out
}

fn read_u32_le(data: &[u8], off: &mut usize) -> Option<u32> {
    let b = data.get(*off..*off + 4)?;
    *off += 4;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn read_u64_le(data: &[u8], off: &mut usize) -> Option<u64> {
    let b = data.get(*off..*off + 8)?;
    *off += 8;
    Some(u64::from_le_bytes([
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
    ]))
}

fn read_cstring_aligned(data: &[u8], off: &mut usize) -> Option<String> {
    let start = *off;
    let rel_nul = data.get(start..)?.iter().position(|&b| b == 0)?;
    let s = String::from_utf8_lossy(&data[start..start + rel_nul]).to_string();
    let raw_len = rel_nul + 1;
    let aligned = (raw_len + 3) & !3;
    *off = start + aligned.min(data.len().saturating_sub(start));
    Some(s)
}

fn decode_xpc_object(data: &[u8], off: &mut usize) -> Option<XpcVal> {
    let ty = read_u32_le(data, off)?;
    match ty {
        XPC_TYPE_NULL => Some(XpcVal::Null),
        XPC_TYPE_BOOL => {
            let v = read_u32_le(data, off)?;
            Some(XpcVal::Bool(v != 0))
        }
        XPC_TYPE_INT64 => {
            let v = read_u64_le(data, off)? as i64;
            Some(XpcVal::Int64(v))
        }
        XPC_TYPE_UINT64 => {
            let v = read_u64_le(data, off)?;
            Some(XpcVal::Uint64(v))
        }
        XPC_TYPE_DOUBLE => {
            let v = f64::from_bits(read_u64_le(data, off)?);
            Some(XpcVal::Double(v))
        }
        XPC_TYPE_DATE => {
            let v = read_u64_le(data, off)? as i64;
            Some(XpcVal::Date(v))
        }
        XPC_TYPE_DATA => {
            let len = read_u32_le(data, off)? as usize;
            let bytes = data.get(*off..*off + len)?.to_vec();
            let aligned = (len + 3) & !3;
            *off += aligned.min(data.len().saturating_sub(*off));
            Some(XpcVal::Data(bytes))
        }
        XPC_TYPE_STRING => {
            let len = read_u32_le(data, off)? as usize;
            let slice = data.get(*off..*off + len)?;
            let trimmed = slice.strip_suffix(&[0]).unwrap_or(slice);
            let s = String::from_utf8_lossy(trimmed).to_string();
            let aligned = (len + 3) & !3;
            *off += aligned.min(data.len().saturating_sub(*off));
            Some(XpcVal::Str(s))
        }
        XPC_TYPE_UUID => {
            let b = data.get(*off..*off + 16)?;
            let mut arr = [0u8; 16];
            arr.copy_from_slice(b);
            *off += 16;
            Some(XpcVal::Uuid(arr))
        }
        XPC_TYPE_ARRAY => {
            let byte_len = read_u32_le(data, off)? as usize;
            let end = (*off).checked_add(byte_len)?;
            let count = read_u32_le(data, off)? as usize;
            let mut items = Vec::with_capacity(count.min(4096));
            for _ in 0..count {
                if *off >= end {
                    break;
                }
                items.push(decode_xpc_object(data, off)?);
            }
            *off = end;
            Some(XpcVal::Array(items))
        }
        XPC_TYPE_DICT => {
            let byte_len = read_u32_le(data, off)? as usize;
            let end = (*off).checked_add(byte_len)?;
            let count = read_u32_le(data, off)? as usize;
            let mut entries = Vec::with_capacity(count.min(4096));
            for _ in 0..count {
                if *off >= end {
                    break;
                }
                let k = read_cstring_aligned(data, off)?;
                let v = decode_xpc_object(data, off)?;
                entries.push((k, v));
            }
            *off = end;
            Some(XpcVal::Dict(entries))
        }
        _ => Some(XpcVal::Null),
    }
}

fn ipv6_checksum(src: &[u8; 16], dst: &[u8; 16], next_hdr: u8, payload: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    for chunk in src.chunks_exact(2) {
        sum += u32::from(u16::from_be_bytes([chunk[0], chunk[1]]));
    }
    for chunk in dst.chunks_exact(2) {
        sum += u32::from(u16::from_be_bytes([chunk[0], chunk[1]]));
    }
    let plen = payload.len() as u32;
    sum += (plen >> 16) + (plen & 0xFFFF);
    sum += u32::from(next_hdr);

    let mut i = 0usize;
    while i + 1 < payload.len() {
        sum += u32::from(u16::from_be_bytes([payload[i], payload[i + 1]]));
        i += 2;
    }
    if i < payload.len() {
        sum += u32::from(payload[i]) << 8;
    }
    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    let csum = (!sum) as u16;
    if next_hdr == 17 && csum == 0 {
        0xFFFF
    } else {
        csum
    }
}

fn build_ipv6_tcp(
    src: [u8; 16],
    dst: [u8; 16],
    sport: u16,
    dport: u16,
    seq: u32,
    ack: u32,
    flags: u8,
    opts: &[u8],
    payload: &[u8],
) -> Vec<u8> {
    let hdr_len = 20 + opts.len();
    let doff = (((hdr_len / 4) as u8) & 0x0F) << 4;
    let tcp_len = hdr_len + payload.len();
    let mut tcp = Vec::with_capacity(tcp_len);
    tcp.extend_from_slice(&sport.to_be_bytes());
    tcp.extend_from_slice(&dport.to_be_bytes());
    tcp.extend_from_slice(&seq.to_be_bytes());
    tcp.extend_from_slice(&ack.to_be_bytes());
    tcp.push(doff);
    tcp.push(flags);
    tcp.extend_from_slice(&65535u16.to_be_bytes());
    tcp.extend_from_slice(&0u16.to_be_bytes());
    tcp.extend_from_slice(&0u16.to_be_bytes());
    tcp.extend_from_slice(opts);
    tcp.extend_from_slice(payload);

    let csum = ipv6_checksum(&src, &dst, 6, &tcp);
    tcp[16..18].copy_from_slice(&csum.to_be_bytes());

    let mut pkt = Vec::with_capacity(40 + tcp.len());
    pkt.extend_from_slice(&(6u32 << 28).to_be_bytes());
    pkt.extend_from_slice(&(tcp.len() as u16).to_be_bytes());
    pkt.push(6);
    pkt.push(64);
    pkt.extend_from_slice(&src);
    pkt.extend_from_slice(&dst);
    pkt.extend_from_slice(&tcp);
    pkt
}

fn build_ipv6_udp(
    src: [u8; 16],
    dst: [u8; 16],
    sport: u16,
    dport: u16,
    payload: &[u8],
) -> Vec<u8> {
    let udp_len = (8 + payload.len()) as u16;
    let mut udp = Vec::with_capacity(udp_len as usize);
    udp.extend_from_slice(&sport.to_be_bytes());
    udp.extend_from_slice(&dport.to_be_bytes());
    udp.extend_from_slice(&udp_len.to_be_bytes());
    udp.extend_from_slice(&0u16.to_be_bytes());
    udp.extend_from_slice(payload);

    let csum = ipv6_checksum(&src, &dst, 17, &udp);
    udp[6..8].copy_from_slice(&csum.to_be_bytes());

    let mut pkt = Vec::with_capacity(40 + udp.len());
    pkt.extend_from_slice(&(6u32 << 28).to_be_bytes());
    pkt.extend_from_slice(&udp_len.to_be_bytes());
    pkt.push(17);
    pkt.push(64);
    pkt.extend_from_slice(&src);
    pkt.extend_from_slice(&dst);
    pkt.extend_from_slice(&udp);
    pkt
}

#[derive(Debug)]
struct TcpSeg {
    seq: u32,
    ack_num: u32,
    window: u16,
    flags: u8,
    win_scale: Option<u8>,
    payload: Vec<u8>,
}

/// Parses RFC 7323 Window Scale option (Kind=3, Length=3) from TCP header options.
fn parse_tcp_win_scale(opts: &[u8]) -> Option<u8> {
    let mut i = 0usize;
    while i < opts.len() {
        let kind = opts[i];
        if kind == 0 {
            break;
        }
        if kind == 1 {
            i += 1;
            continue;
        }
        let len = *opts.get(i + 1)? as usize;
        if len < 2 || i + len > opts.len() {
            break;
        }
        if kind == 3 && len == 3 {
            return Some(opts[i + 2].min(14));
        }
        i += len;
    }
    None
}

struct TcpRouteEntry {
    tx: mpsc::Sender<TcpSeg>,
    read_waker: Arc<Mutex<Option<std::task::Waker>>>,
    write_waker: Arc<Mutex<Option<std::task::Waker>>>,
}

type TcpRoutes = Arc<Mutex<HashMap<(u16, u16), TcpRouteEntry>>>;

fn wake_route_reader(slot: &Arc<Mutex<Option<std::task::Waker>>>) {
    let w = slot.lock().ok().and_then(|mut g| g.take());
    if let Some(waker) = w {
        waker.wake();
    }
}

fn noop_waker() -> Waker {
    fn clone(_: *const ()) -> RawWaker {
        RawWaker::new(std::ptr::null(), &VT)
    }
    fn no_op(_: *const ()) {}
    static VT: RawWakerVTable = RawWakerVTable::new(clone, no_op, no_op, no_op);
    unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VT)) }
}

/// Power/Lock (`0x0C, 0x0030`) hold duration clamp (`480..=520 ms`): calibrated 0.5 s iOS
/// SpringBoard sleep hold (passes bounce threshold cleanly while staying well under 1.0 s Siri).
pub fn clamp_power_lock_hold_ms(hold_ms: u64) -> u64 {
    hold_ms.clamp(480, 520)
}

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryW(lp_lib_file_name: *const u16) -> *mut c_void;
    fn GetProcAddress(h_module: *mut c_void, lp_proc_name: *const u8) -> *const c_void;
}

#[derive(Clone, Copy)]
struct WinmmTimerApi {
    begin: Option<unsafe extern "system" fn(u32) -> u32>,
    end: Option<unsafe extern "system" fn(u32) -> u32>,
}

unsafe impl Send for WinmmTimerApi {}
unsafe impl Sync for WinmmTimerApi {}

fn winmm_timer_api() -> &'static WinmmTimerApi {
    static API: OnceLock<WinmmTimerApi> = OnceLock::new();
    API.get_or_init(|| {
        #[cfg(windows)]
        unsafe {
            let name: Vec<u16> = "winmm.dll\0".encode_utf16().collect();
            let h = LoadLibraryW(name.as_ptr());
            if h.is_null() {
                return WinmmTimerApi {
                    begin: None,
                    end: None,
                };
            }
            let pb = GetProcAddress(h, b"timeBeginPeriod\0".as_ptr());
            let pe = GetProcAddress(h, b"timeEndPeriod\0".as_ptr());
            WinmmTimerApi {
                begin: if pb.is_null() {
                    None
                } else {
                    Some(std::mem::transmute(pb))
                },
                end: if pe.is_null() {
                    None
                } else {
                    Some(std::mem::transmute(pe))
                },
            }
        }
        #[cfg(not(windows))]
        WinmmTimerApi {
            begin: None,
            end: None,
        }
    })
}

/// RAII Win32 `timeBeginPeriod(1)` guard active during physical tunnel I/O so scheduler wakes stay ~1 ms
/// even after touchpad lift during high-velocity iOS inertia scroll.
pub(crate) struct TunnelTimerPeriod1msGuard {
    active: bool,
}

impl TunnelTimerPeriod1msGuard {
    pub(crate) fn acquire_1ms() -> Self {
        let mut active = false;
        if let Some(begin_fn) = winmm_timer_api().begin {
            if unsafe { begin_fn(1) } == 0 {
                active = true;
            }
        }
        Self { active }
    }
}

impl Drop for TunnelTimerPeriod1msGuard {
    fn drop(&mut self) {
        if self.active {
            if let Some(end_fn) = winmm_timer_api().end {
                unsafe {
                    let _ = end_fn(1);
                }
            }
            self.active = false;
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct WsaPollFd {
    pub fd: usize,
    pub events: i16,
    pub revents: i16,
}

#[cfg(all(windows, target_pointer_width = "64"))]
const _: () = assert!(std::mem::size_of::<WsaPollFd>() == 16);

const WSAPOLL_RDNORM: i16 = 0x0100;
const WSAPOLL_WRNORM: i16 = 0x0010;
const WSAPOLL_ERR: i16 = 0x0001;
const WSAPOLL_HUP: i16 = 0x0002;
const WSAPOLL_NVAL: i16 = 0x0004;

type FnWsaPoll = unsafe extern "system" fn(*mut WsaPollFd, u32, i32) -> i32;

fn wsapoll_fn() -> Option<FnWsaPoll> {
    static API: OnceLock<Option<FnWsaPoll>> = OnceLock::new();
    *API.get_or_init(|| {
        #[cfg(windows)]
        unsafe {
            let name: Vec<u16> = "ws2_32.dll\0".encode_utf16().collect();
            let h = LoadLibraryW(name.as_ptr());
            if h.is_null() {
                return None;
            }
            let p = GetProcAddress(h, b"WSAPoll\0".as_ptr());
            if p.is_null() {
                None
            } else {
                Some(std::mem::transmute(p))
            }
        }
        #[cfg(not(windows))]
        None
    })
}

/// Event-driven WinSock2 `WSAPoll` wait (`< 0.1 ms` wakeup on Wi-Fi TCP arrival) with safe `sleep(1ms)`
/// fallback for USB adapters or poll errors.
pub(crate) fn wait_tunnel_socket_ready_or_timeout(
    raw_sock: Option<usize>,
    need_write: bool,
    timeout_ms: i32,
) -> (i32, i16) {
    if let (Some(fd), Some(poll_fn)) = (raw_sock, wsapoll_fn()) {
        let mut pfd = WsaPollFd {
            fd,
            events: if need_write {
                WSAPOLL_RDNORM | WSAPOLL_WRNORM
            } else {
                WSAPOLL_RDNORM
            },
            revents: 0,
        };
        let res = unsafe { poll_fn(&mut pfd, 1, timeout_ms.max(1)) };
        if res < 0 {
            std::thread::sleep(Duration::from_millis(timeout_ms.max(1) as u64));
            return (res, 0);
        }
        if res > 0
            && (pfd.revents & WSAPOLL_NVAL) != 0
            && (pfd.revents & (WSAPOLL_RDNORM | WSAPOLL_WRNORM)) == 0
        {
            std::thread::sleep(Duration::from_millis(1));
        }
        return (res, pfd.revents);
    }
    std::thread::sleep(Duration::from_millis(timeout_ms.max(1) as u64));
    (0, 0)
}

/// Opportunistically coalesces already-queued whole IPv6 packets from `tx_pending` into `tx_in_flight`
/// up to `16384 B` (one TLS 1.2 record payload, `0 ms` wait!).
pub(crate) fn coalesce_tx_batch(tx_pending: &mut VecDeque<Vec<u8>>, tx_in_flight: &mut Vec<u8>) {
    tx_in_flight.clear();
    while let Some(front) = tx_pending.front() {
        if !tx_in_flight.is_empty() && tx_in_flight.len() + front.len() > 16384 {
            break;
        }
        if let Some(pkt) = tx_pending.pop_front() {
            tx_in_flight.extend_from_slice(&pkt);
        }
        if tx_in_flight.len() >= 16384 {
            break;
        }
    }
}

#[inline]
fn compact_tunnel_rx_cursor(buf: &mut Vec<u8>, pos: &mut usize) {
    if *pos >= buf.len() {
        buf.clear();
        *pos = 0;
    } else if *pos >= 32768 {
        let rem = buf.len() - *pos;
        buf.copy_within(*pos.., 0);
        buf.truncate(rem);
        *pos = 0;
    }
}

struct CdTunnelRouter {
    src_ip: [u8; 16],
    dst_ip: [u8; 16],
    src_str: String,
    dst_str: String,
    rsd_port: u16,
    tx_pkt: mpsc::Sender<Vec<u8>>,
    tx_flush_req: Arc<AtomicU64>,
    tx_flush_ack: Arc<AtomicU64>,
    tcp_routes: TcpRoutes,
    next_ephemeral: AtomicU16,
    stop_pump: Arc<AtomicBool>,
    exit_reason: Arc<Mutex<Option<String>>>,
}

impl Drop for CdTunnelRouter {
    fn drop(&mut self) {
        self.stop_pump.store(true, Ordering::SeqCst);
        let drained: Vec<TcpRouteEntry> = self
            .tcp_routes
            .lock()
            .map(|mut g| g.drain().map(|(_, v)| v).collect())
            .unwrap_or_default();
        for entry in drained {
            wake_route_reader(&entry.read_waker);
            wake_route_reader(&entry.write_waker);
        }
    }
}

impl CdTunnelRouter {
    fn exit_reason(&self) -> Option<String> {
        self.exit_reason
            .lock()
            .ok()
            .and_then(|g| g.clone())
    }

    async fn flush_tx(&self) {
        let ticket = self.tx_flush_req.fetch_add(1, Ordering::SeqCst) + 1;
        let deadline = Instant::now() + Duration::from_millis(80);
        while self.tx_flush_ack.load(Ordering::Acquire) < ticket {
            if self.stop_pump.load(Ordering::Relaxed) || Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }

    #[cfg(test)]
    fn new(
        stream: Box<dyn idevice::ReadWrite>,
        src_ip: Ipv6Addr,
        dst_ip: Ipv6Addr,
        rsd_port: u16,
    ) -> (Arc<Self>, mpsc::Receiver<(u16, u16, Vec<u8>)>) {
        Self::new_with_raw_socket(stream, src_ip, dst_ip, rsd_port, None)
    }

    fn new_with_raw_socket(
        mut stream: Box<dyn idevice::ReadWrite>,
        src_ip: Ipv6Addr,
        dst_ip: Ipv6Addr,
        rsd_port: u16,
        raw_data_socket: Option<usize>,
    ) -> (Arc<Self>, mpsc::Receiver<(u16, u16, Vec<u8>)>) {
        let src_bytes = src_ip.octets();
        let dst_bytes = dst_ip.octets();
        let (tx_pkt, rx_pkt) = mpsc::channel::<Vec<u8>>();
        let tx_flush_req = Arc::new(AtomicU64::new(0));
        let tx_flush_ack = Arc::new(AtomicU64::new(0));
        let tx_flush_req_io = tx_flush_req.clone();
        let tx_flush_ack_io = tx_flush_ack.clone();
        let (udp_tx, udp_rx) = mpsc::channel::<(u16, u16, Vec<u8>)>();
        let tcp_routes: TcpRoutes = Arc::new(Mutex::new(HashMap::new()));
        let routes_rd = tcp_routes.clone();
        let stop_pump = Arc::new(AtomicBool::new(false));
        let stop_io = stop_pump.clone();
        let exit_reason: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let exit_reason_io = exit_reason.clone();

        let _ = std::thread::Builder::new()
            .name("iphone-cdtunnel-io".into())
            .spawn(move || {
                let _timer_guard = TunnelTimerPeriod1msGuard::acquire_1ms();
                let record_exit = |msg: String| {
                    if let Ok(mut g) = exit_reason_io.lock() {
                        if g.is_none() {
                            *g = Some(msg);
                        }
                    }
                };
                let _ = stream.set_stream_nonblocking(true);
                let waker = noop_waker();
                let mut cx = Context::from_waker(&waker);
                let mut rx_acc: Vec<u8> = Vec::with_capacity(131072);
                let mut rx_acc_pos: usize = 0;
                let mut tmp_buf = [0u8; 65536];
                let mut tx_pending: VecDeque<Vec<u8>> = VecDeque::new();
                let mut tx_in_flight: Vec<u8> = Vec::with_capacity(16384);
                let mut tx_in_flight_pos: usize = 0;
                let mut consecutive_poll_err_wake: u8 = 0;

                'pump: while !stop_io.load(Ordering::Relaxed) {
                    let flush_req_snap = tx_flush_req_io.load(Ordering::Acquire);
                    loop {
                        match rx_pkt.try_recv() {
                            Ok(pkt) => tx_pending.push_back(pkt),
                            Err(mpsc::TryRecvError::Empty) => break,
                            Err(mpsc::TryRecvError::Disconnected) => break 'pump,
                        }
                    }

                    let mut tx_blocked = false;
                    let mut tx_did_work = false;
                    let mut tx_batches = 0usize;
                    loop {
                        if tx_in_flight_pos >= tx_in_flight.len() {
                            if tx_batches >= 4 || tx_pending.is_empty() {
                                break;
                            }
                            coalesce_tx_batch(&mut tx_pending, &mut tx_in_flight);
                            tx_in_flight_pos = 0;
                            tx_batches += 1;
                        }
                        if tx_in_flight_pos >= tx_in_flight.len() {
                            break;
                        }
                        match Pin::new(&mut *stream)
                            .poll_write(&mut cx, &tx_in_flight[tx_in_flight_pos..])
                        {
                            Poll::Ready(Ok(0)) => {
                                record_exit("CDTunnel write returned 0 (peer closed)".into());
                                break 'pump;
                            }
                            Poll::Ready(Err(e))
                                if matches!(
                                    e.kind(),
                                    std::io::ErrorKind::WouldBlock
                                        | std::io::ErrorKind::Interrupted
                                ) =>
                            {
                                tx_blocked = true;
                                break;
                            }
                            Poll::Ready(Err(e)) => {
                                record_exit(format!("CDTunnel write error: {e}"));
                                break 'pump;
                            }
                            Poll::Ready(Ok(n)) => {
                                tx_in_flight_pos += n;
                                tx_did_work = true;
                                if tx_in_flight_pos >= tx_in_flight.len() {
                                    tx_in_flight.clear();
                                    tx_in_flight_pos = 0;
                                }
                            }
                            Poll::Pending => {
                                tx_blocked = true;
                                break;
                            }
                        }
                    }

                    // Always flush any buffered TLS ciphertext record inside TlsPskStream / SchannelTlsStream
                    // whenever tx_pending & tx_in_flight are drained, preserving atomic ciphertext across WouldBlock.
                    let mut flush_pending = false;
                    if tx_pending.is_empty()
                        && tx_in_flight_pos >= tx_in_flight.len()
                        && !tx_blocked
                    {
                        match Pin::new(&mut *stream).poll_flush(&mut cx) {
                            Poll::Ready(Ok(())) => {
                                tx_flush_ack_io.fetch_max(flush_req_snap, Ordering::Release);
                            }
                            Poll::Pending => {
                                flush_pending = true;
                            }
                            Poll::Ready(Err(e))
                                if matches!(
                                    e.kind(),
                                    std::io::ErrorKind::WouldBlock
                                        | std::io::ErrorKind::Interrupted
                                ) =>
                            {
                                flush_pending = true;
                            }
                            Poll::Ready(Err(e)) => {
                                record_exit(format!("CDTunnel flush error: {e}"));
                                break 'pump;
                            }
                        }
                    }

                    let mut read_would_block = false;
                    let mut read_did_work = false;
                    for _ in 0..64 {
                        let mut rbuf = tokio::io::ReadBuf::new(&mut tmp_buf);
                        match Pin::new(&mut *stream).poll_read(&mut cx, &mut rbuf) {
                            Poll::Ready(Ok(())) => {
                                let filled = rbuf.filled();
                                if filled.is_empty() {
                                    record_exit("CDTunnel read EOF (peer closed tunnel)".into());
                                    break 'pump;
                                }
                                read_did_work = true;
                                consecutive_poll_err_wake = 0;
                                rx_acc.extend_from_slice(filled);
                                while rx_acc.len().saturating_sub(rx_acc_pos) >= 40 {
                                    let ver = rx_acc[rx_acc_pos] >> 4;
                                    if ver != 6 {
                                        record_exit(format!(
                                            "CDTunnel IPv6 framing desync (version nibble={ver})"
                                        ));
                                        break 'pump;
                                    }
                                    let plen = u16::from_be_bytes([
                                        rx_acc[rx_acc_pos + 4],
                                        rx_acc[rx_acc_pos + 5],
                                    ]) as usize;
                                    let total_len = 40 + plen;
                                    if rx_acc.len().saturating_sub(rx_acc_pos) < total_len {
                                        break;
                                    }
                                    let next_hdr = rx_acc[rx_acc_pos + 6];
                                    let body = &rx_acc[rx_acc_pos + 40..rx_acc_pos + total_len];
                                    if next_hdr == 6 && body.len() >= 20 {
                                        let sport = u16::from_be_bytes([body[0], body[1]]);
                                        let dport = u16::from_be_bytes([body[2], body[3]]);
                                        let seq = u32::from_be_bytes([
                                            body[4], body[5], body[6], body[7],
                                        ]);
                                        let ack_num = u32::from_be_bytes([
                                            body[8], body[9], body[10], body[11],
                                        ]);
                                        let doff = ((body[12] >> 4) as usize) * 4;
                                        let flags = body[13];
                                        let window = u16::from_be_bytes([body[14], body[15]]);
                                        if doff >= 20 && doff <= body.len() {
                                            let win_scale = if (flags & 0x02) != 0 && doff > 20 {
                                                parse_tcp_win_scale(&body[20..doff])
                                            } else {
                                                None
                                            };
                                            let pay = body[doff..].to_vec();
                                            let wakers_opt = routes_rd.lock().ok().and_then(|guard| {
                                                guard.get(&(dport, sport)).and_then(|entry| {
                                                    if entry
                                                        .tx
                                                        .send(TcpSeg {
                                                            seq,
                                                            ack_num,
                                                            window,
                                                            flags,
                                                            win_scale,
                                                            payload: pay,
                                                        })
                                                        .is_ok()
                                                    {
                                                        Some((
                                                            Arc::clone(&entry.read_waker),
                                                            Arc::clone(&entry.write_waker),
                                                        ))
                                                    } else {
                                                        None
                                                    }
                                                })
                                            });
                                            if let Some((r_slot, w_slot)) = wakers_opt {
                                                wake_route_reader(&r_slot);
                                                wake_route_reader(&w_slot);
                                            }
                                        }
                                    } else if next_hdr == 17 && body.len() >= 8 {
                                        let sport = u16::from_be_bytes([body[0], body[1]]);
                                        let dport = u16::from_be_bytes([body[2], body[3]]);
                                        let _ = udp_tx.send((sport, dport, body[8..].to_vec()));
                                    }
                                    rx_acc_pos += total_len;
                                }
                                compact_tunnel_rx_cursor(&mut rx_acc, &mut rx_acc_pos);
                            }
                            Poll::Ready(Err(e))
                                if matches!(
                                    e.kind(),
                                    std::io::ErrorKind::WouldBlock
                                        | std::io::ErrorKind::Interrupted
                                        | std::io::ErrorKind::TimedOut
                                ) =>
                            {
                                read_would_block = true;
                                break;
                            }
                            Poll::Ready(Err(e)) => {
                                record_exit(format!("CDTunnel read error: {e}"));
                                break 'pump;
                            }
                            Poll::Pending => {
                                read_would_block = true;
                                break;
                            }
                        }
                    }

                    let tx_idle_or_blocked =
                        (tx_pending.is_empty() && tx_in_flight_pos >= tx_in_flight.len())
                            || tx_blocked;
                    if read_would_block
                        && !read_did_work
                        && !tx_did_work
                        && (tx_idle_or_blocked || flush_pending)
                    {
                        let (poll_res, revents) = wait_tunnel_socket_ready_or_timeout(
                            raw_data_socket,
                            tx_blocked || flush_pending,
                            1,
                        );
                        if poll_res > 0
                            && (revents & (WSAPOLL_ERR | WSAPOLL_HUP | WSAPOLL_NVAL)) != 0
                            && (revents & (WSAPOLL_RDNORM | WSAPOLL_WRNORM)) == 0
                        {
                            consecutive_poll_err_wake = consecutive_poll_err_wake.saturating_add(1);
                            if consecutive_poll_err_wake >= 2 {
                                std::thread::sleep(Duration::from_millis(1));
                            }
                        } else {
                            consecutive_poll_err_wake = 0;
                        }
                    } else if read_did_work || tx_did_work {
                        consecutive_poll_err_wake = 0;
                    }
                }
                stop_io.store(true, Ordering::Release);
                let drained_routes: Vec<TcpRouteEntry> = routes_rd
                    .lock()
                    .map(|mut g| g.drain().map(|(_, v)| v).collect())
                    .unwrap_or_default();
                for entry in drained_routes {
                    wake_route_reader(&entry.read_waker);
                    wake_route_reader(&entry.write_waker);
                }
            });

        (
            Arc::new(Self {
                src_ip: src_bytes,
                dst_ip: dst_bytes,
                src_str: src_ip.to_string(),
                dst_str: dst_ip.to_string(),
                rsd_port,
                tx_pkt,
                tx_flush_req,
                tx_flush_ack,
                tcp_routes,
                next_ephemeral: AtomicU16::new(49200),
                stop_pump,
                exit_reason,
            }),
            udp_rx,
        )
    }
}

/// TCP SYN options isolated by tunnel channel:
/// - `enable_wscale = true`: DDI bulk upload channels (`CdTcpRw`) use `MSS=1340 + NOP + WScale=6` (256 KB+ window).
/// - `enable_wscale = false`: `XpcH2Session` control/DisplayService channels use standard `MSS=1340` only.
pub fn build_syn_options_for_channel(enable_wscale: bool) -> &'static [u8] {
    if enable_wscale {
        &[0x02, 0x04, 0x05, 0x3C, 0x01, 0x03, 0x03, 0x06]
    } else {
        &[0x02, 0x04, 0x05, 0x3C]
    }
}

impl CdTunnelRouter {
    async fn connect_tcp(&self, dport: u16) -> Result<CdTcpConn, String> {
        self.connect_tcp_with_wscale(dport, true).await
    }

    async fn connect_tcp_with_wscale(
        &self,
        dport: u16,
        enable_wscale: bool,
    ) -> Result<CdTcpConn, String> {
        let sport = self.next_ephemeral.fetch_add(1, Ordering::Relaxed);
        let (seg_tx, seg_rx) = mpsc::channel();
        let read_waker = Arc::new(Mutex::new(None));
        let write_waker = Arc::new(Mutex::new(None));
        if let Ok(mut g) = self.tcp_routes.lock() {
            g.insert(
                (sport, dport),
                TcpRouteEntry {
                    tx: seg_tx,
                    read_waker: Arc::clone(&read_waker),
                    write_waker: Arc::clone(&write_waker),
                },
            );
        }

        let mut seq = 1000u32;
        let syn_opts = build_syn_options_for_channel(enable_wscale);
        let syn = build_ipv6_tcp(
            self.src_ip,
            self.dst_ip,
            sport,
            dport,
            seq,
            0,
            0x02,
            syn_opts,
            &[],
        );
        self.tx_pkt
            .send(syn)
            .map_err(|_| "tunnel writer closed".to_string())?;
        seq = seq.wrapping_add(1);

        let deadline = Instant::now() + Duration::from_secs(6);
        let (ack, syn_ack_win, peer_wscale) = loop {
            let rem = deadline.saturating_duration_since(Instant::now());
            if rem.is_zero() {
                return Err(format!("TCP SYN-ACK timeout on port {dport}"));
            }
            match seg_rx.recv_timeout(rem.min(Duration::from_millis(200))) {
                Ok(seg) => {
                    if (seg.flags & 0x12) == 0x12 {
                        let wscale = if enable_wscale {
                            seg.win_scale.unwrap_or(0).min(14)
                        } else {
                            0
                        };
                        break (seg.seq.wrapping_add(1), u32::from(seg.window), wscale);
                    }
                    if (seg.flags & 0x04) != 0 {
                        return Err(format!("TCP RST on port {dport}"));
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(format!("TCP channel closed before SYN-ACK on port {dport}"));
                }
            }
        };

        let ack_pkt = build_ipv6_tcp(self.src_ip, self.dst_ip, sport, dport, seq, ack, 0x10, &[], &[]);
        let _ = self.tx_pkt.send(ack_pkt);

        Ok(CdTcpConn {
            src_ip: self.src_ip,
            dst_ip: self.dst_ip,
            sport,
            dport,
            seq,
            snd_una: seq,
            ack,
            peer_window_bytes: syn_ack_win.max(1300),
            peer_wscale,
            unacked: VecDeque::new(),
            dup_ack_seq: seq,
            dup_ack_count: 0,
            rto: Duration::from_millis(250),
            zwp_last_at: None,
            zwp_probes: 0,
            tx_pkt: self.tx_pkt.clone(),
            tx_flush_barrier: Some((self.tx_flush_req.clone(), self.tx_flush_ack.clone())),
            seg_rx,
            rx_buf: Vec::new(),
            ooo: BTreeMap::new(),
            routes: self.tcp_routes.clone(),
            read_timeout: Duration::from_secs(10),
            read_waker,
            write_waker,
            peer_closed: false,
            router_stopped: self.stop_pump.clone(),
        })
    }

    fn send_udp(&self, sport: u16, dport: u16, payload: &[u8]) {
        let pkt = build_ipv6_udp(self.src_ip, self.dst_ip, sport, dport, payload);
        let _ = self.tx_pkt.send(pkt);
    }
}

#[derive(Debug, Clone)]
struct UnackedTcpSeg {
    seq: u32,
    payload: Vec<u8>,
    sent_at: Instant,
    retransmits: u8,
}

struct CdTcpConn {
    src_ip: [u8; 16],
    dst_ip: [u8; 16],
    sport: u16,
    dport: u16,
    seq: u32,
    snd_una: u32,
    ack: u32,
    peer_window_bytes: u32,
    peer_wscale: u8,
    unacked: VecDeque<UnackedTcpSeg>,
    dup_ack_seq: u32,
    dup_ack_count: u8,
    rto: Duration,
    zwp_last_at: Option<Instant>,
    zwp_probes: u8,
    tx_pkt: mpsc::Sender<Vec<u8>>,
    tx_flush_barrier: Option<(Arc<AtomicU64>, Arc<AtomicU64>)>,
    seg_rx: mpsc::Receiver<TcpSeg>,
    rx_buf: Vec<u8>,
    ooo: BTreeMap<u32, Vec<u8>>,
    routes: TcpRoutes,
    read_timeout: Duration,
    read_waker: Arc<Mutex<Option<std::task::Waker>>>,
    write_waker: Arc<Mutex<Option<std::task::Waker>>>,
    peer_closed: bool,
    router_stopped: Arc<AtomicBool>,
}

impl Drop for CdTcpConn {
    fn drop(&mut self) {
        let fin = build_ipv6_tcp(
            self.src_ip,
            self.dst_ip,
            self.sport,
            self.dport,
            self.seq,
            self.ack,
            0x11,
            &[],
            &[],
        );
        let _ = self.tx_pkt.send(fin);
        if let Ok(mut g) = self.routes.lock() {
            g.remove(&(self.sport, self.dport));
        }
    }
}

impl CdTcpConn {
    fn apply_peer_ack(&mut self, seg: &TcpSeg) {
        if (seg.flags & 0x10) == 0 {
            return;
        }
        let prev_win = self.peer_window_bytes;
        let new_win = if (seg.flags & 0x02) != 0 {
            u32::from(seg.window)
        } else {
            u32::from(seg.window) << self.peer_wscale
        };
        self.peer_window_bytes = new_win;
        if new_win > 0 {
            self.zwp_last_at = None;
            self.zwp_probes = 0;
        }

        let adv = seg.ack_num.wrapping_sub(self.snd_una) as i32;
        let not_future = seg.ack_num.wrapping_sub(self.seq) as i32;
        if adv > 0 && not_future <= 0 {
            self.snd_una = seg.ack_num;
            self.dup_ack_seq = seg.ack_num;
            self.dup_ack_count = 0;
            self.rto = Duration::from_millis(250);
            while let Some(front) = self.unacked.front_mut() {
                let end_seq = front.seq.wrapping_add(front.payload.len() as u32);
                if (end_seq.wrapping_sub(self.snd_una) as i32) <= 0 {
                    self.unacked.pop_front();
                } else {
                    let partial = self.snd_una.wrapping_sub(front.seq) as i32;
                    if partial > 0 {
                        let drop_n = (partial as usize).min(front.payload.len());
                        front.payload.drain(..drop_n);
                        front.seq = front.seq.wrapping_add(drop_n as u32);
                        front.retransmits = 0;
                        front.sent_at = Instant::now();
                    }
                    break;
                }
            }
        } else if adv == 0
            && !self.unacked.is_empty()
            && seg.payload.is_empty()
            && new_win == prev_win
            && new_win > 0
        {
            if self.dup_ack_seq == seg.ack_num {
                self.dup_ack_count = self.dup_ack_count.saturating_add(1);
            } else {
                self.dup_ack_seq = seg.ack_num;
                self.dup_ack_count = 1;
            }
            if self.dup_ack_count == 3 {
                if let Some(front) = self.unacked.front_mut() {
                    let pkt = build_ipv6_tcp(
                        self.src_ip,
                        self.dst_ip,
                        self.sport,
                        self.dport,
                        front.seq,
                        self.ack,
                        0x18,
                        &[],
                        &front.payload,
                    );
                    if self.tx_pkt.send(pkt).is_ok() {
                        front.sent_at = Instant::now();
                        front.retransmits = front.retransmits.saturating_add(1);
                    }
                }
            }
        }
    }

    fn send_windowed_slice(&mut self, data: &[u8]) -> Result<usize, String> {
        if self.peer_window_bytes == 0 {
            return Ok(0);
        }
        let in_flight = self.seq.wrapping_sub(self.snd_una);
        let effective_cap = self.peer_window_bytes.min(256 * 1024);
        let avail = effective_cap.saturating_sub(in_flight) as usize;
        if avail == 0 {
            return Ok(0);
        }
        let slice = &data[..data.len().min(avail)];
        let now = Instant::now();
        let mut sent = 0usize;
        for chunk in slice.chunks(1300) {
            let pkt = build_ipv6_tcp(
                self.src_ip,
                self.dst_ip,
                self.sport,
                self.dport,
                self.seq,
                self.ack,
                0x18,
                &[],
                chunk,
            );
            self.tx_pkt
                .send(pkt)
                .map_err(|_| "tunnel writer closed".to_string())?;
            self.unacked.push_back(UnackedTcpSeg {
                seq: self.seq,
                payload: chunk.to_vec(),
                sent_at: now,
                retransmits: 0,
            });
            self.seq = self.seq.wrapping_add(chunk.len() as u32);
            sent += chunk.len();
        }
        Ok(sent)
    }

    fn check_retransmit_or_zwp(
        &mut self,
        now: Instant,
        has_pending_write: bool,
    ) -> Result<Option<Duration>, std::io::Error> {
        if let Some(front) = self.unacked.front_mut() {
            let elapsed = now.saturating_duration_since(front.sent_at);
            if elapsed >= self.rto {
                if front.retransmits >= 8 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        format!(
                            "TCP retransmit limit exceeded on port {} at seq {}",
                            self.dport, front.seq
                        ),
                    ));
                }
                let pkt = build_ipv6_tcp(
                    self.src_ip,
                    self.dst_ip,
                    self.sport,
                    self.dport,
                    front.seq,
                    self.ack,
                    0x18,
                    &[],
                    &front.payload,
                );
                self.tx_pkt.send(pkt).map_err(|_| {
                    std::io::Error::new(std::io::ErrorKind::BrokenPipe, "tunnel writer closed")
                })?;
                front.sent_at = now;
                front.retransmits = front.retransmits.saturating_add(1);
                self.rto = (self.rto * 2).min(Duration::from_millis(2000));
                return Ok(Some(self.rto));
            }
            return Ok(Some(
                self.rto
                    .saturating_sub(elapsed)
                    .max(Duration::from_millis(10)),
            ));
        }

        if has_pending_write && self.peer_window_bytes == 0 {
            let zwp_interval = Duration::from_millis(250);
            match self.zwp_last_at {
                None => {
                    self.zwp_last_at = Some(now);
                    return Ok(Some(zwp_interval));
                }
                Some(last) => {
                    let elapsed = now.saturating_duration_since(last);
                    if elapsed >= zwp_interval {
                        if self.zwp_probes >= 40 {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::TimedOut,
                                format!("TCP zero-window stall timeout on port {}", self.dport),
                            ));
                        }
                        let probe = build_ipv6_tcp(
                            self.src_ip,
                            self.dst_ip,
                            self.sport,
                            self.dport,
                            self.seq.wrapping_sub(1),
                            self.ack,
                            0x10,
                            &[],
                            &[],
                        );
                        self.tx_pkt.send(probe).map_err(|_| {
                            std::io::Error::new(
                                std::io::ErrorKind::BrokenPipe,
                                "tunnel writer closed",
                            )
                        })?;
                        self.zwp_probes = self.zwp_probes.saturating_add(1);
                        self.zwp_last_at = Some(now);
                        return Ok(Some(zwp_interval));
                    }
                    return Ok(Some(zwp_interval.saturating_sub(elapsed).max(Duration::from_millis(10))));
                }
            }
        }

        Ok(None)
    }

    fn drain_nonblocking_for_async(&mut self) -> Result<bool, String> {
        let mut ack_needed = false;
        let mut peer_fin = false;
        loop {
            match self.seg_rx.try_recv() {
                Ok(seg) => {
                    if (seg.flags & 0x04) != 0 {
                        return Err(format!("TCP connection reset on port {}", self.dport));
                    }
                    self.apply_peer_ack(&seg);
                    if !seg.payload.is_empty() {
                        if seg.seq == self.ack {
                            self.ack = self.ack.wrapping_add(seg.payload.len() as u32);
                            self.rx_buf.extend_from_slice(&seg.payload);
                            self.drain_contiguous_ooo();
                        } else {
                            let delta = seg.seq.wrapping_sub(self.ack) as i32;
                            if delta > 0 {
                                self.ooo.insert(seg.seq, seg.payload);
                            } else if delta < 0 {
                                let skip = (-delta) as usize;
                                if skip < seg.payload.len() {
                                    let tail = &seg.payload[skip..];
                                    self.ack = self.ack.wrapping_add(tail.len() as u32);
                                    self.rx_buf.extend_from_slice(tail);
                                    self.drain_contiguous_ooo();
                                }
                            }
                        }
                        ack_needed = true;
                    } else if (seg.flags & 0x10) != 0 && seg.seq != self.ack {
                        ack_needed = true;
                    }
                    if (seg.flags & 0x01) != 0 {
                        self.ack = self.ack.wrapping_add(1);
                        ack_needed = true;
                        peer_fin = true;
                        break;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.peer_closed = true;
                    break;
                }
            }
        }
        if ack_needed {
            let ack_pkt = build_ipv6_tcp(
                self.src_ip,
                self.dst_ip,
                self.sport,
                self.dport,
                self.seq,
                self.ack,
                0x10,
                &[],
                &[],
            );
            let _ = self.tx_pkt.send(ack_pkt);
            TCP_ACKS_SENT.fetch_add(1, Ordering::SeqCst);
        }
        if peer_fin {
            self.peer_closed = true;
        }
        if self.router_stopped.load(Ordering::Acquire) {
            self.peer_closed = true;
        }
        Ok(ack_needed)
    }

    async fn flush(&mut self) {
        if let Some((ref req, ref ack)) = self.tx_flush_barrier {
            let ticket = req.fetch_add(1, Ordering::SeqCst) + 1;
            let deadline = Instant::now() + Duration::from_millis(80);
            while ack.load(Ordering::Acquire) < ticket && Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        }
    }

    fn send_all(&mut self, data: &[u8]) -> Result<(), String> {
        for chunk in data.chunks(1300) {
            let pkt = build_ipv6_tcp(
                self.src_ip,
                self.dst_ip,
                self.sport,
                self.dport,
                self.seq,
                self.ack,
                0x18,
                &[],
                chunk,
            );
            self.tx_pkt
                .send(pkt)
                .map_err(|_| "tunnel writer closed".to_string())?;
            self.seq = self.seq.wrapping_add(chunk.len() as u32);
        }
        Ok(())
    }

    fn drain_contiguous_ooo(&mut self) {
        while let Some(next_pay) = self.ooo.remove(&self.ack) {
            self.ack = self.ack.wrapping_add(next_pay.len() as u32);
            self.rx_buf.extend_from_slice(&next_pay);
        }
    }

    async fn read_exact(&mut self, n: usize) -> Result<Vec<u8>, String> {
        let deadline = Instant::now() + self.read_timeout;
        while self.rx_buf.len() < n {
            let rem = deadline.saturating_duration_since(Instant::now());
            if rem.is_zero() {
                return Err(format!("TCP read timeout on port {}", self.dport));
            }
            let seg = match self.seg_rx.recv_timeout(rem.min(Duration::from_millis(200))) {
                Ok(seg) => seg,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(format!("TCP stream closed on port {}", self.dport));
                }
            };
            if (seg.flags & 0x04) != 0 {
                return Err(format!("TCP connection reset on port {}", self.dport));
            }
            self.apply_peer_ack(&seg);
            if !seg.payload.is_empty() {
                if seg.seq == self.ack {
                    self.ack = self.ack.wrapping_add(seg.payload.len() as u32);
                    self.rx_buf.extend_from_slice(&seg.payload);
                    self.drain_contiguous_ooo();
                } else {
                    let delta = seg.seq.wrapping_sub(self.ack) as i32;
                    if delta > 0 {
                        self.ooo.insert(seg.seq, seg.payload);
                    } else if delta < 0 {
                        let skip = (-delta) as usize;
                        if skip < seg.payload.len() {
                            let tail = &seg.payload[skip..];
                            self.ack = self.ack.wrapping_add(tail.len() as u32);
                            self.rx_buf.extend_from_slice(tail);
                            self.drain_contiguous_ooo();
                        }
                    }
                }
                let ack_pkt = build_ipv6_tcp(
                    self.src_ip,
                    self.dst_ip,
                    self.sport,
                    self.dport,
                    self.seq,
                    self.ack,
                    0x10,
                    &[],
                    &[],
                );
                let _ = self.tx_pkt.send(ack_pkt);
                TCP_ACKS_SENT.fetch_add(1, Ordering::SeqCst);
            } else if (seg.flags & 0x01) != 0 {
                self.ack = self.ack.wrapping_add(1);
                let fin_ack = build_ipv6_tcp(
                    self.src_ip,
                    self.dst_ip,
                    self.sport,
                    self.dport,
                    self.seq,
                    self.ack,
                    0x10,
                    &[],
                    &[],
                );
                let _ = self.tx_pkt.send(fin_ack);
                TCP_ACKS_SENT.fetch_add(1, Ordering::SeqCst);
                return Err(format!("TCP FIN from peer on port {}", self.dport));
            }
        }
        let out = self.rx_buf[..n].to_vec();
        self.rx_buf.drain(..n);
        Ok(out)
    }

    fn drain_nonblocking(&mut self) -> Result<bool, String> {
        let mut ack_needed = false;
        let mut peer_fin = false;
        while let Ok(seg) = self.seg_rx.try_recv() {
            if (seg.flags & 0x04) != 0 {
                return Err(format!("TCP connection reset on port {}", self.dport));
            }
            self.apply_peer_ack(&seg);
            if !seg.payload.is_empty() {
                if seg.seq == self.ack {
                    self.ack = self.ack.wrapping_add(seg.payload.len() as u32);
                    self.rx_buf.extend_from_slice(&seg.payload);
                    self.drain_contiguous_ooo();
                } else {
                    let delta = seg.seq.wrapping_sub(self.ack) as i32;
                    if delta > 0 {
                        self.ooo.insert(seg.seq, seg.payload);
                    } else if delta < 0 {
                        let skip = (-delta) as usize;
                        if skip < seg.payload.len() {
                            let tail = &seg.payload[skip..];
                            self.ack = self.ack.wrapping_add(tail.len() as u32);
                            self.rx_buf.extend_from_slice(tail);
                            self.drain_contiguous_ooo();
                        }
                    }
                }
                ack_needed = true;
            } else if (seg.flags & 0x10) != 0 && seg.seq != self.ack {
                // Zero-payload keepalive probe or duplicate seq from iOS: answer with immediate ACK
                ack_needed = true;
            }
            if (seg.flags & 0x01) != 0 {
                // Peer closed TCP stream cleanly (FIN): advance ack and drop zombie session immediately
                self.ack = self.ack.wrapping_add(1);
                ack_needed = true;
                peer_fin = true;
                break;
            }
        }
        if ack_needed {
            let ack_pkt = build_ipv6_tcp(
                self.src_ip,
                self.dst_ip,
                self.sport,
                self.dport,
                self.seq,
                self.ack,
                0x10,
                &[],
                &[],
            );
            let _ = self.tx_pkt.send(ack_pkt);
            TCP_ACKS_SENT.fetch_add(1, Ordering::SeqCst);
        }
        if peer_fin {
            return Err(format!("TCP FIN from peer on port {}", self.dport));
        }
        Ok(ack_needed)
    }
}

/// AsyncRead + AsyncWrite adapter over `CdTcpConn` so `idevice::cryptexd` and other
/// RemoteXPC services can run natively inside the active CDTunnel router.
struct CdTcpRw {
    conn: Mutex<CdTcpConn>,
}

impl std::fmt::Debug for CdTcpRw {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CdTcpRw").finish()
    }
}

impl CdTcpRw {
    fn new(conn: CdTcpConn) -> Self {
        Self {
            conn: Mutex::new(conn),
        }
    }
}

impl tokio::io::AsyncRead for CdTcpRw {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let mut guard = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        if let Err(e) = guard.drain_nonblocking_for_async() {
            if guard.rx_buf.is_empty() {
                return Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::ConnectionReset,
                    e,
                )));
            }
        }
        if !guard.unacked.is_empty() {
            let _ = guard.check_retransmit_or_zwp(Instant::now(), false)?;
        }
        if !guard.rx_buf.is_empty() {
            let n = guard.rx_buf.len().min(buf.unfilled_mut().len());
            buf.put_slice(&guard.rx_buf[..n]);
            guard.rx_buf.drain(..n);
            return Poll::Ready(Ok(()));
        }
        if guard.peer_closed || guard.router_stopped.load(Ordering::Acquire) {
            return Poll::Ready(Ok(()));
        }

        // Register Tokio waker, then re-drain to guarantee zero lost-wakeup race.
        if let Ok(mut wslot) = guard.read_waker.lock() {
            *wslot = Some(cx.waker().clone());
        }
        if let Err(e) = guard.drain_nonblocking_for_async() {
            if guard.rx_buf.is_empty() {
                return Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::ConnectionReset,
                    e,
                )));
            }
        }
        if !guard.rx_buf.is_empty() {
            if let Ok(mut wslot) = guard.read_waker.lock() {
                *wslot = None;
            }
            let n = guard.rx_buf.len().min(buf.unfilled_mut().len());
            buf.put_slice(&guard.rx_buf[..n]);
            guard.rx_buf.drain(..n);
            return Poll::Ready(Ok(()));
        }
        if guard.peer_closed || guard.router_stopped.load(Ordering::Acquire) {
            if let Ok(mut wslot) = guard.read_waker.lock() {
                *wslot = None;
            }
            return Poll::Ready(Ok(()));
        }
        Poll::Pending
    }
}

impl tokio::io::AsyncWrite for CdTcpRw {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let mut guard = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .drain_nonblocking_for_async()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::ConnectionReset, e))?;
        if guard.peer_closed || guard.router_stopped.load(Ordering::Acquire) {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "TCP peer closed",
            )));
        }
        let _ = guard.check_retransmit_or_zwp(Instant::now(), true)?;
        let sent = guard
            .send_windowed_slice(buf)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::BrokenPipe, e))?;
        if sent > 0 {
            return Poll::Ready(Ok(sent));
        }

        // Window saturated or Zero-Window: register waker, re-drain, retry.
        if let Ok(mut wslot) = guard.write_waker.lock() {
            *wslot = Some(cx.waker().clone());
        }
        guard
            .drain_nonblocking_for_async()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::ConnectionReset, e))?;
        if guard.peer_closed || guard.router_stopped.load(Ordering::Acquire) {
            if let Ok(mut wslot) = guard.write_waker.lock() {
                *wslot = None;
            }
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "TCP peer closed",
            )));
        }
        let sent_retry = guard
            .send_windowed_slice(buf)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::BrokenPipe, e))?;
        if sent_retry > 0 {
            if let Ok(mut wslot) = guard.write_waker.lock() {
                *wslot = None;
            }
            return Poll::Ready(Ok(sent_retry));
        }
        let _ = guard.check_retransmit_or_zwp(Instant::now(), true)?;
        Poll::Pending
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let mut guard = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .drain_nonblocking_for_async()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::ConnectionReset, e))?;
        let _ = guard.check_retransmit_or_zwp(Instant::now(), false)?;
        if guard.unacked.is_empty() {
            return Poll::Ready(Ok(()));
        }
        if guard.peer_closed || guard.router_stopped.load(Ordering::Acquire) {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "TCP peer closed before ACK flush completed",
            )));
        }
        if let Ok(mut wslot) = guard.write_waker.lock() {
            *wslot = Some(cx.waker().clone());
        }
        guard
            .drain_nonblocking_for_async()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::ConnectionReset, e))?;
        if guard.unacked.is_empty() {
            if let Ok(mut wslot) = guard.write_waker.lock() {
                *wslot = None;
            }
            return Poll::Ready(Ok(()));
        }
        if guard.peer_closed || guard.router_stopped.load(Ordering::Acquire) {
            if let Ok(mut wslot) = guard.write_waker.lock() {
                *wslot = None;
            }
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "TCP peer closed before ACK flush completed",
            )));
        }
        let _ = guard.check_retransmit_or_zwp(Instant::now(), false)?;
        Poll::Pending
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.poll_flush(cx)
    }
}

struct CdTunnelRsdProvider {
    router: Arc<CdTunnelRouter>,
}

impl idevice::provider::RsdProvider for CdTunnelRsdProvider {
    async fn connect_to_service_port(
        &mut self,
        port: u16,
    ) -> Result<Box<dyn idevice::ReadWrite>, idevice::IdeviceError> {
        let conn = self
            .router
            .connect_tcp(port)
            .await
            .map_err(|e| idevice::IdeviceError::Io(std::io::Error::other(e)))?;
        Ok(Box::new(CdTcpRw::new(conn)))
    }
}

fn rsd_handshake_from_peer_info(peer_info: &XpcVal) -> idevice::rsd::RsdHandshake {
    let mut services = HashMap::new();
    if let Some(XpcVal::Dict(entries)) = peer_info.dict_get("Services") {
        for (name, svc_val) in entries {
            let port = svc_val
                .dict_get("Port")
                .and_then(|v| {
                    v.as_u64()
                        .or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok()))
                })
                .unwrap_or(0) as u16;
            if port == 0 {
                continue;
            }
            let entitlement = svc_val
                .dict_get("Entitlement")
                .and_then(XpcVal::as_str)
                .unwrap_or("")
                .to_string();
            let uses_remote_xpc = svc_val
                .dict_get("Properties")
                .and_then(|p| p.dict_get("UsesRemoteXPC"))
                .and_then(|v| match v {
                    XpcVal::Bool(b) => Some(*b),
                    _ => None,
                })
                .unwrap_or(false);
            services.insert(
                name.clone(),
                idevice::rsd::RsdService {
                    entitlement,
                    port,
                    uses_remote_xpc,
                    features: None,
                    service_version: None,
                },
            );
        }
    }
    let uuid = peer_info
        .dict_get("UUID")
        .and_then(XpcVal::as_str)
        .unwrap_or("00000000-0000-0000-0000-000000000000")
        .to_string();
    idevice::rsd::RsdHandshake {
        services,
        protocol_version: 7,
        uuid,
    }
}

fn h2_frame(ftype: u8, flags: u8, stream_id: u32, payload: &[u8]) -> Vec<u8> {
    let l = payload.len();
    let mut out = Vec::with_capacity(9 + l);
    out.push(((l >> 16) & 0xFF) as u8);
    out.push(((l >> 8) & 0xFF) as u8);
    out.push((l & 0xFF) as u8);
    out.push(ftype);
    out.push(flags);
    out.extend_from_slice(&stream_id.to_be_bytes());
    out.extend_from_slice(payload);
    out
}

struct XpcH2Session {
    conn: CdTcpConn,
    next_msg_id: u64,
    xpc_acc: Vec<u8>,
    bytes_sent_since_win_update: usize,
    pkts_sent_since_win_update: u64,
    pending_ping_sent_at: Option<Instant>,
    last_activity_at: Instant,
}

impl XpcH2Session {
    async fn open(router: &CdTunnelRouter, port: u16) -> Result<Self, String> {
        tokio::time::timeout(Duration::from_secs(8), async move {
            let mut conn = router.connect_tcp_with_wscale(port, false).await?;

            let mut init = Vec::new();
            init.extend_from_slice(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n");
            let mut settings = Vec::with_capacity(12);
            settings.extend_from_slice(&3u16.to_be_bytes());
            settings.extend_from_slice(&100u32.to_be_bytes());
            settings.extend_from_slice(&4u16.to_be_bytes());
            settings.extend_from_slice(&(16u32 * 1024 * 1024).to_be_bytes());
            init.extend_from_slice(&h2_frame(4, 0, 0, &settings));
            init.extend_from_slice(&h2_frame(
                8,
                0,
                0,
                &(16u32 * 1024 * 1024 - 65535).to_be_bytes(),
            ));
            init.extend_from_slice(&h2_frame(1, 4, 1, &[]));
            let empty_dict = XpcVal::Dict(Vec::new());
            init.extend_from_slice(&h2_frame(
                0,
                0,
                1,
                &build_xpc_wrapper(XPC_FLAG_ALWAYS_SET, 0, Some(&empty_dict)),
            ));
            init.extend_from_slice(&h2_frame(1, 4, 3, &[]));
            init.extend_from_slice(&h2_frame(0, 0, 1, &build_xpc_wrapper(0x0201, 0, None)));
            init.extend_from_slice(&h2_frame(
                0,
                0,
                3,
                &build_xpc_wrapper(XPC_FLAG_ALWAYS_SET | XPC_FLAG_INIT_HANDSHAKE, 0, None),
            ));
            conn.send_all(&init)?;

            loop {
                let fh = conn.read_exact(9).await?;
                let flen = ((fh[0] as usize) << 16) | ((fh[1] as usize) << 8) | (fh[2] as usize);
                let ftype = fh[3];
                let fflags = fh[4];
                if flen > 0 {
                    let _ = conn.read_exact(flen).await?;
                }
                if ftype == 4 && (fflags & 1) == 0 {
                    conn.send_all(&h2_frame(4, 1, 0, &[]))?;
                    break;
                }
            }

            Ok(Self {
                conn,
                next_msg_id: 1,
                xpc_acc: Vec::new(),
                bytes_sent_since_win_update: 0,
                pkts_sent_since_win_update: 0,
                pending_ping_sent_at: None,
                last_activity_at: Instant::now(),
            })
        })
        .await
        .map_err(|_| format!("Timeout XpcH2Session::open ke port {port} (> 8 detik)"))?
    }

    async fn rpc_with_timeout(
        &mut self,
        req: &XpcVal,
        wanting_reply: bool,
        timeout: Duration,
    ) -> Result<XpcVal, String> {
        tokio::time::timeout(timeout, self.rpc(req, wanting_reply))
            .await
            .map_err(|_| {
                format!(
                    "Timeout RemoteXPC RPC di port {} (> {} detik)",
                    self.conn.dport,
                    timeout.as_secs()
                )
            })?
    }

    async fn rpc(&mut self, req: &XpcVal, wanting_reply: bool) -> Result<XpcVal, String> {
        let mut flags = XPC_FLAG_ALWAYS_SET | XPC_FLAG_DATA_PRESENT;
        if wanting_reply {
            flags |= XPC_FLAG_WANTING_REPLY;
        }
        let w = build_xpc_wrapper(flags, self.next_msg_id, Some(req));
        self.next_msg_id += 2;
        for chunk in w.chunks(16384) {
            self.conn.send_all(&h2_frame(0, 0, 1, chunk))?;
        }

        loop {
            let fh = self.conn.read_exact(9).await?;
            let flen = ((fh[0] as usize) << 16) | ((fh[1] as usize) << 8) | (fh[2] as usize);
            let ftype = fh[3];
            let fflags = fh[4];
            let fbody = if flen > 0 {
                self.conn.read_exact(flen).await?
            } else {
                Vec::new()
            };
            if ftype == 6 && (fflags & 1) == 0 && fbody.len() == 8 {
                let _ = self.conn.send_all(&h2_frame(6, 1, 0, &fbody));
            } else if ftype == 0 && !fbody.is_empty() {
                self.xpc_acc.extend_from_slice(&fbody);
                while self.xpc_acc.len() >= 24 {
                    let magic = u32::from_le_bytes([
                        self.xpc_acc[0],
                        self.xpc_acc[1],
                        self.xpc_acc[2],
                        self.xpc_acc[3],
                    ]);
                    if magic != XPC_WRAPPER_MAGIC {
                        return Err(format!("invalid XpcWrapper magic: 0x{magic:08x}"));
                    }
                    let size = u64::from_le_bytes([
                        self.xpc_acc[8],
                        self.xpc_acc[9],
                        self.xpc_acc[10],
                        self.xpc_acc[11],
                        self.xpc_acc[12],
                        self.xpc_acc[13],
                        self.xpc_acc[14],
                        self.xpc_acc[15],
                    ]) as usize;
                    if self.xpc_acc.len() < 24 + size {
                        break;
                    }
                    let frame = self.xpc_acc[..24 + size].to_vec();
                    self.xpc_acc.drain(..24 + size);
                    if size >= 12 {
                        let mut off = 32usize; // 24 wrapper + 8 payload header
                        if let Some(val) = decode_xpc_object(&frame, &mut off) {
                            if let XpcVal::Dict(ref d) = val {
                                if !d.is_empty() {
                                    return Ok(val);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    fn drain_incoming_h2_nonblocking(&mut self) -> Result<(), String> {
        let _ = self.conn.drain_nonblocking()?;
        while self.conn.rx_buf.len() >= 9 {
            let flen = ((self.conn.rx_buf[0] as usize) << 16)
                | ((self.conn.rx_buf[1] as usize) << 8)
                | (self.conn.rx_buf[2] as usize);
            if self.conn.rx_buf.len() < 9 + flen {
                break;
            }
            let ftype = self.conn.rx_buf[3];
            let fflags = self.conn.rx_buf[4];
            let sid = u32::from_be_bytes([
                self.conn.rx_buf[5] & 0x7F,
                self.conn.rx_buf[6],
                self.conn.rx_buf[7],
                self.conn.rx_buf[8],
            ]);
            let fbody = self.conn.rx_buf[9..9 + flen].to_vec();
            self.conn.rx_buf.drain(..9 + flen);
            if ftype == 6 && (fflags & 1) == 0 && fbody.len() == 8 {
                let _ = self.conn.send_all(&h2_frame(6, 1, 0, &fbody));
                H2_PING_ACKS_SENT.fetch_add(1, Ordering::SeqCst);
                self.last_activity_at = Instant::now();
            } else if ftype == 6 && (fflags & 1) == 1 {
                // Peer ACKed our proactive keepalive PING: connection is genuinely alive!
                self.pending_ping_sent_at = None;
                self.last_activity_at = Instant::now();
            } else if ftype == 4 && (fflags & 1) == 0 {
                let _ = self.conn.send_all(&h2_frame(4, 1, 0, &[]));
                self.last_activity_at = Instant::now();
            } else if ftype == 7 || ftype == 3 {
                return Err(format!("HTTP/2 peer closed stream (ftype={ftype}, sid={sid})"));
            } else if ftype == 0 && flen > 0 {
                let _ = self
                    .conn
                    .send_all(&h2_frame(8, 0, 0, &0x007F_FFFFu32.to_be_bytes()));
                let target_sid = if sid != 0 { sid } else { 1 };
                let _ = self
                    .conn
                    .send_all(&h2_frame(8, 0, target_sid, &0x007F_FFFFu32.to_be_bytes()));
                H2_WINDOW_UPDATES_SENT.fetch_add(2, Ordering::SeqCst);
                self.last_activity_at = Instant::now();
            }
        }
        Ok(())
    }

    #[allow(dead_code)]
    fn send_ping(&mut self, opaque: [u8; 8]) -> Result<(), String> {
        if self.pending_ping_sent_at.is_none() {
            self.pending_ping_sent_at = Some(Instant::now());
        }
        self.conn.send_all(&h2_frame(6, 0, 0, &opaque))
    }

    #[allow(dead_code)]
    fn is_ping_timed_out(&self) -> bool {
        self.pending_ping_sent_at
            .is_some_and(|sent| sent.elapsed() >= Duration::from_secs(5))
    }

    async fn flush(&mut self) {
        self.conn.flush().await;
    }

    fn send_oneway(&mut self, req: &XpcVal) -> Result<(), String> {
        self.drain_incoming_h2_nonblocking()?;
        let flags = XPC_FLAG_ALWAYS_SET | XPC_FLAG_DATA_PRESENT;
        let w = build_xpc_wrapper(flags, self.next_msg_id, Some(req));
        self.next_msg_id += 2;
        for chunk in w.chunks(16384) {
            self.conn.send_all(&h2_frame(0, 0, 1, chunk))?;
        }
        self.bytes_sent_since_win_update = self.bytes_sent_since_win_update.saturating_add(w.len());
        self.pkts_sent_since_win_update = self.pkts_sent_since_win_update.saturating_add(1);
        if self.bytes_sent_since_win_update >= 16_384 || self.pkts_sent_since_win_update >= 100 {
            let _ = self
                .conn
                .send_all(&h2_frame(8, 0, 0, &0x007F_FFFFu32.to_be_bytes()));
            let _ = self
                .conn
                .send_all(&h2_frame(8, 0, 1, &0x007F_FFFFu32.to_be_bytes()));
            H2_WINDOW_UPDATES_SENT.fetch_add(2, Ordering::SeqCst);
            self.bytes_sent_since_win_update = 0;
            self.pkts_sent_since_win_update = 0;
        }
        self.last_activity_at = Instant::now();
        Ok(())
    }
}

/// Drains incoming HTTP/2 server handshake frames on a newly opened `indigo_xpc` session
/// for up to `INDIGO_XPC_FRESH_OPEN_SETTLE_MS` (180 ms) so iOS completes stream-3
/// `XPC_FLAG_INIT_HANDSHAKE` before the first `IndigoButtonEvent` hits the wire.
async fn settle_fresh_indigo_xpc(sess: &mut XpcH2Session) {
    let deadline = Instant::now() + Duration::from_millis(INDIGO_XPC_FRESH_OPEN_SETTLE_MS);
    loop {
        if sess.drain_incoming_h2_nonblocking().is_err() {
            break;
        }
        if Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
}

pub const UHID_TOUCHSCREEN_SERVICE_ID: u64 = 257;
pub const UHID_KEYBOARD_DEFAULT_SERVICE_ID: u64 = 0x1_0000_2001;
pub const UHID_CONSUMER_DEFAULT_SERVICE_ID: u64 = 0x1_0000_2002;
pub const INDIGO_XPC_FRESH_OPEN_SETTLE_MS: u64 = 180;
const KEYBOARD_ATTACH_SETTLE_MS: u64 = 300;
const CONSUMER_ATTACH_SETTLE_MS: u64 = 120;
const TOUCH_AUTH_SETTLE_MS: u64 = 250;

/// Standard USB HID Consumer Control Report Descriptor (`Usage Page 0x0C`: Power `0x0030`,
/// Home `0x0040`, Brightness `0x006F`/`0x0070`, Volume `0x00E9`/`0x00EA`, Eject `0x00B8`).
pub const CONSUMER_CONTROL_REPORT_DESCRIPTOR_25B: [u8; 25] = [
    0x05, 0x0C, // Usage Page (Consumer Devices)
    0x09, 0x01, // Usage (Consumer Control)
    0xA1, 0x01, // Collection (Application)
    0x85, 0x02, //   Report ID (2)
    0x19, 0x00, //   Usage Minimum (0x00)
    0x2A, 0xFF, 0x02, // Usage Maximum (0x02FF)
    0x15, 0x00, //   Logical Minimum (0x00)
    0x26, 0xFF, 0x02, // Logical Maximum (0x02FF)
    0x75, 0x10, //   Report Size (16)
    0x95, 0x01, //   Report Count (1)
    0x81, 0x00, //   Input (Data, Array, Abs)
    0xC0, // End Collection
];

pub fn build_consumer_report_3b(usage: u16) -> Vec<u8> {
    vec![0x02, (usage & 0xFF) as u8, (usage >> 8) as u8]
}

pub const KEYBOARD_REPORT_DESCRIPTOR_67B: [u8; 67] = [
    0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0x05, 0x07, 0x19, 0xE0, 0x29, 0xE7, 0x15, 0x00, 0x25, 0x01,
    0x95, 0x08, 0x75, 0x01, 0x81, 0x02, 0x95, 0x01, 0x75, 0x08, 0x81, 0x01, 0x05, 0x07, 0x19, 0x00,
    0x29, 0xFF, 0x15, 0x00, 0x26, 0xFF, 0x00, 0x95, 0x06, 0x75, 0x08, 0x81, 0x00, 0x05, 0x08, 0x19,
    0x01, 0x29, 0x05, 0x15, 0x00, 0x25, 0x01, 0x95, 0x05, 0x75, 0x01, 0x91, 0x02, 0x95, 0x01, 0x75,
    0x03, 0x91, 0x01,
];

#[derive(Debug, Clone)]
pub enum PhysicalHidCommand {
    Touch {
        contact: bool,
        x: u16,
        y: u16,
    },
    TouchSequence {
        /// `(contact, x, y, delay_after_ms)`
        steps: Vec<(bool, u16, u16, u64)>,
    },
    Button {
        page: u16,
        usage: u16,
        is_down: bool,
    },
    ButtonPress {
        page: u16,
        usage: u16,
        hold_ms: u64,
    },
    HomeCombo,
    ButtonSequence {
        /// `(page, usage, is_down, delay_after_ms)`
        events: Vec<(u16, u16, bool, u64)>,
    },
    KeyboardUsages {
        usages: Vec<u8>,
    },
    KeyboardSequence {
        /// `(usages, delay_after_ms)`
        steps: Vec<(Vec<u8>, u64)>,
    },
}

struct PhysicalHidEnvelope {
    cmd: PhysicalHidCommand,
    ack: Option<mpsc::Sender<Result<u64, String>>>,
}

static PHYSICAL_HID_TX: OnceLock<Mutex<Option<mpsc::Sender<PhysicalHidEnvelope>>>> = OnceLock::new();
static PHYSICAL_HID_ACTIVE: AtomicBool = AtomicBool::new(false);
static PHYSICAL_HID_PACKETS_SENT: AtomicU64 = AtomicU64::new(0);
static PHYSICAL_HID_ERRORS: AtomicU64 = AtomicU64::new(0);
static H2_PING_ACKS_SENT: AtomicU64 = AtomicU64::new(0);
static H2_WINDOW_UPDATES_SENT: AtomicU64 = AtomicU64::new(0);
static TCP_ACKS_SENT: AtomicU64 = AtomicU64::new(0);
static RTCP_SR_RECEIVED: AtomicU64 = AtomicU64::new(0);
static RTCP_RR_WITH_LSR_SENT: AtomicU64 = AtomicU64::new(0);
static RTCP_BW_FEEDBACK_SENT: AtomicU64 = AtomicU64::new(0);
static INDIGO_PING_TIMEOUT_RECONNECTS: AtomicU64 = AtomicU64::new(0);
static UHID_CONSUMER_REPORTS_SENT: AtomicU64 = AtomicU64::new(0);
static DISPLAY_XPC_PINGS_SENT: AtomicU64 = AtomicU64::new(0);
static AUTO_RECONNECT_COUNT: AtomicU64 = AtomicU64::new(0);
static KEEP_AWAKE_PULSES_SENT: AtomicU64 = AtomicU64::new(0);
static LAST_HID_TX_EPOCH_MS: AtomicU64 = AtomicU64::new(0);
static TOUCH_CONTACT_DOWN: AtomicBool = AtomicBool::new(false);
static LAST_DISCONNECT_REASON: OnceLock<Mutex<Option<String>>> = OnceLock::new();
static ACTIVE_SESSION_DEVICE_IDENTITY: OnceLock<Mutex<(String, String, String, String)>> =
    OnceLock::new();
static MONO_EPOCH: OnceLock<Instant> = OnceLock::new();

fn last_disconnect_reason_slot() -> &'static Mutex<Option<String>> {
    LAST_DISCONNECT_REASON.get_or_init(|| Mutex::new(None))
}

fn active_identity_slot() -> &'static Mutex<(String, String, String, String)> {
    ACTIVE_SESSION_DEVICE_IDENTITY
        .get_or_init(|| Mutex::new((String::new(), String::new(), String::new(), String::new())))
}

fn now_epoch_ms_u64() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Updates the active mirrored session's live device identity `(udid, device_name, marketing_model, bt_mac)`
/// discovered via Lockdown, mDNS Bonjour SRV host, or CoreDevice RSD Handshake properties.
pub fn set_active_session_device_identity(
    udid: &str,
    device_name: Option<&str>,
    marketing_model: Option<&str>,
    bt_mac: Option<&str>,
) {
    if let Ok(mut g) = active_identity_slot().lock() {
        let same_udid = !udid.is_empty() && g.0.eq_ignore_ascii_case(udid);
        if !udid.is_empty() && !same_udid {
            g.0 = udid.to_string();
            g.1.clear();
            g.2.clear();
            g.3.clear();
        }
        if let Some(n) = device_name.map(str::trim).filter(|s| !s.is_empty()) {
            g.1 = n.to_string();
        }
        if let Some(m) = marketing_model.map(str::trim).filter(|s| !s.is_empty()) {
            g.2 = m.to_string();
        }
        if let Some(b) = bt_mac.map(str::trim).filter(|s| !s.is_empty()) {
            g.3 = b.to_ascii_uppercase();
        }
    }
}

/// Returns `(udid, device_name, marketing_model, bluetooth_address)` for the active mirrored session.
pub fn active_session_device_identity() -> (String, String, String, String) {
    active_identity_slot()
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default()
}

/// Tahap 244 Pilar 2: Pure stream liveness decision separating RTP video pauses (VFR static screens
/// like Apple Notes where `RTCP SR` keeps arriving every ~1 s) from true dead tunnels (`>= 12 s`
/// total silence across ALL RTP + RTCP packets).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamLivenessAction {
    Healthy,
    SendSoftPli,
    HardReconnect,
}

pub fn evaluate_stream_liveness(
    rtp_silence: Duration,
    tunnel_rx_silence: Duration,
    since_last_hid: Option<Duration>,
    intentionally_locked: bool,
    sent_first_key: bool,
    soft_pli_already_sent: bool,
) -> StreamLivenessAction {
    if tunnel_rx_silence >= Duration::from_secs(12) {
        return StreamLivenessAction::HardReconnect;
    }
    if sent_first_key
        && !intentionally_locked
        && !soft_pli_already_sent
        && rtp_silence >= Duration::from_millis(1_500)
        && since_last_hid
            .is_some_and(|d| d >= Duration::from_millis(1_500) && d <= Duration::from_secs(10))
    {
        return StreamLivenessAction::SendSoftPli;
    }
    StreamLivenessAction::Healthy
}

/// Tahap 255 Pilar 2: Pure helper proving `gate_only` RTP packets are strictly silent-drained
/// and NEVER touch `CodecHeader`, `LIVE_BROADCAST`, or `global_on_physical_video_frame()`.
pub fn process_rtp_video_packet_guarded(
    gate_only: bool,
    session_gen: u64,
    depack: &mut HevcDepacketizer,
    au: &mut Vec<u8>,
    codec_ready: &mut bool,
    sent_first_key: &mut bool,
    seq: u16,
    ts: u32,
    payload: &[u8],
    marker: bool,
) -> bool {
    if gate_only {
        // Silent drain-only: NEVER depacketize, NEVER update CodecHeader, NEVER push video frames!
        return false;
    }
    depack.push_rtp(seq, ts, payload);
    let chunk = depack.take_output();
    if !chunk.is_empty() {
        au.extend_from_slice(&chunk);
    }
    if marker && !au.is_empty() {
        let complete_au = std::mem::take(au);
        if !*codec_ready {
            if let (Some(vps), Some(sps), Some(pps)) = (
                depack.vps.as_deref(),
                depack.sps.as_deref(),
                depack.pps.as_deref(),
            ) {
                let codec_str = hevc_codec_string_from_sps(sps)
                    .unwrap_or_else(|| "hev1.1.6.L150.B0".to_string());
                let hvcc = build_hvcc_record(vps, sps, pps);
                let (cw, ch, trim) = parse_hevc_sps_dimensions_and_trim(sps)
                    .unwrap_or((1216, 2656, FrameTrim::zero()));
                crate::mirror::update_live_codec_from_sps_with_trim(
                    session_gen,
                    &codec_str,
                    hvcc.as_deref(),
                    cw,
                    ch,
                    Some(trim),
                );
                *codec_ready = true;
            }
        }
        let has_key = split_annexb_nalus(&complete_au)
            .into_iter()
            .any(|n| !n.is_empty() && matches!((n[0] >> 1) & 0x3F, 16..=21));
        if *codec_ready && (*sent_first_key || has_key) {
            *sent_first_key = true;
            crate::mirror::global_on_physical_video_frame(complete_au, has_key);
            if has_key {
                crate::mirror::clear_live_pli_request();
            }
            return true;
        }
    }
    false
}

/// Mode-aware wrapper over `evaluate_stream_liveness`.
/// When `gate_only == true` (Hybrid AirPlay H.264 on `iOS 17.4–26.x`), Apple `DisplayService`
/// operates strictly as a HID permission gate and intentionally emits **0 RTP video frames**;
/// video silence MUST NEVER trigger `HardReconnect` or unsolicited PLI spam.
pub fn evaluate_stream_liveness_for_mode(
    rtp_silence: Duration,
    tunnel_rx_silence: Duration,
    since_last_hid: Option<Duration>,
    intentionally_locked: bool,
    sent_first_key: bool,
    soft_pli_already_sent: bool,
    gate_only: bool,
) -> StreamLivenessAction {
    if gate_only {
        return StreamLivenessAction::Healthy;
    }
    evaluate_stream_liveness(
        rtp_silence,
        tunnel_rx_silence,
        since_last_hid,
        intentionally_locked,
        sent_first_key,
        soft_pli_already_sent,
    )
}

fn monotonic_ns_48() -> u64 {
    let start = MONO_EPOCH.get_or_init(Instant::now);
    (start.elapsed().as_nanos() as u64) & 0x0000_FFFF_FFFF_FFFF
}

pub fn build_touchscreen_report_58b(contact: bool, x: u16, y: u16, mono_ns: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(58);
    out.push(0x09); // TOUCHSCREEN_REPORT_ID
    out.push(0x01);
    out.push(0x05);
    out.push(if contact { 0xC2 } else { 0x02 });
    out.extend_from_slice(&x.to_le_bytes());
    out.extend_from_slice(&y.to_le_bytes());
    out.extend_from_slice(&[0u8; 32]);
    out.extend_from_slice(&[0x02, 0x00, 0x00, 0x00]);
    let ts_le = mono_ns.to_le_bytes();
    out.extend_from_slice(&ts_le[..6]);
    out.extend_from_slice(&[0u8; 8]);
    out
}

pub fn build_keyboard_report_39b(usages: &[u8], mono_ns: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(39);
    out.push(0x01); // KEYBOARD_REPORT_ID
    let mut bitmap = [0u8; 30];
    for &u in usages {
        if (u as usize) < 240 {
            bitmap[(u as usize) / 8] |= 1u8 << ((u as usize) % 8);
        }
    }
    out.extend_from_slice(&bitmap);
    let ts_le = mono_ns.to_le_bytes();
    out.extend_from_slice(&ts_le[..6]);
    out.extend_from_slice(&[0x00, 0x00]);
    out
}

fn build_indigo_button_xpc(usage_page: u16, usage_code: u16, is_down: bool) -> XpcVal {
    XpcVal::Dict(vec![
        (
            "featureIdentifier".into(),
            XpcVal::Str("com.apple.coredevice.feature.remote.hid.button".into()),
        ),
        ("messageType".into(), XpcVal::Str("IndigoButtonEvent".into())),
        (
            "payload".into(),
            XpcVal::Dict(vec![
                ("state".into(), XpcVal::Uint64(if is_down { 1 } else { 2 })),
                ("usageCode".into(), XpcVal::Uint64(u64::from(usage_code))),
                ("usagePage".into(), XpcVal::Uint64(u64::from(usage_page))),
            ]),
        ),
    ])
}

fn build_uhid_send_xpc(report: Vec<u8>, service_id: u64) -> XpcVal {
    XpcVal::Dict(vec![
        (
            "featureIdentifier".into(),
            XpcVal::Str("com.apple.coredevice.feature.remote.universalhidservice".into()),
        ),
        ("messageType".into(), XpcVal::Str("Request".into())),
        (
            "payload".into(),
            XpcVal::Dict(vec![(
                "send".into(),
                XpcVal::Dict(vec![
                    ("_0".into(), XpcVal::Data(report)),
                    ("_1".into(), XpcVal::Uint64(service_id)),
                ]),
            )]),
        ),
    ])
}

fn keyboard_descriptor_with_end_collection() -> Vec<u8> {
    let mut desc = Vec::with_capacity(KEYBOARD_REPORT_DESCRIPTOR_67B.len() + 1);
    desc.extend_from_slice(&KEYBOARD_REPORT_DESCRIPTOR_67B);
    desc.push(0xC0); // End Collection
    desc
}

fn build_uhid_create_keyboard_xpc() -> XpcVal {
    let usage_page = XpcVal::Int64(1);
    let usage = XpcVal::Int64(6);
    let vendor = XpcVal::Int64(0x05AC);
    let prod = XpcVal::Int64(0x0250);
    let svc_id = XpcVal::Uint64(UHID_KEYBOARD_DEFAULT_SERVICE_ID);

    let storage = XpcVal::Dict(vec![
        (
            "Manufacturer".into(),
            XpcVal::Dict(vec![("string".into(), XpcVal::Str("Apple Inc.".into()))]),
        ),
        (
            "Product".into(),
            XpcVal::Dict(vec![("string".into(), XpcVal::Str("Magic Keyboard".into()))]),
        ),
        ("ProductID".into(), XpcVal::Dict(vec![("int".into(), prod.clone())])),
        ("VendorID".into(), XpcVal::Dict(vec![("int".into(), vendor.clone())])),
        ("PrimaryUsage".into(), XpcVal::Dict(vec![("int".into(), usage.clone())])),
        (
            "PrimaryUsagePage".into(),
            XpcVal::Dict(vec![("int".into(), usage_page.clone())]),
        ),
        (
            "DeviceUsagePairs".into(),
            XpcVal::Dict(vec![(
                "array".into(),
                XpcVal::Array(vec![XpcVal::Dict(vec![(
                    "dictionary".into(),
                    XpcVal::Dict(vec![
                        ("DeviceUsage".into(), XpcVal::Dict(vec![("int".into(), usage.clone())])),
                        (
                            "DeviceUsagePage".into(),
                            XpcVal::Dict(vec![("int".into(), usage_page.clone())]),
                        ),
                    ]),
                )])]),
            )]),
        ),
        (
            "Transport".into(),
            XpcVal::Dict(vec![("string".into(), XpcVal::Str("USB".into()))]),
        ),
        (
            "ReportDescriptor".into(),
            XpcVal::Dict(vec![(
                "data".into(),
                XpcVal::Data(keyboard_descriptor_with_end_collection()),
            )]),
        ),
        (
            "UniversalControlVirtualService".into(),
            XpcVal::Dict(vec![("bool".into(), XpcVal::Bool(true))]),
        ),
        ("_ServiceID".into(), XpcVal::Dict(vec![("uint".into(), svc_id.clone())])),
    ]);

    let top_pair = XpcVal::Dict(vec![
        ("DeviceUsage".into(), usage),
        ("DeviceUsagePage".into(), usage_page),
    ]);

    XpcVal::Dict(vec![
        (
            "featureIdentifier".into(),
            XpcVal::Str("com.apple.coredevice.feature.remote.universalhidservice".into()),
        ),
        ("messageType".into(), XpcVal::Str("Request".into())),
        (
            "payload".into(),
            XpcVal::Dict(vec![(
                "createService".into(),
                XpcVal::Dict(vec![(
                    "_0".into(),
                    XpcVal::Dict(vec![
                        ("DeviceUsagePairs".into(), XpcVal::Array(vec![top_pair])),
                        ("PrimaryUsage".into(), XpcVal::Uint64(6)),
                        ("PrimaryUsagePage".into(), XpcVal::Uint64(1)),
                        ("Product".into(), XpcVal::Str("Magic Keyboard".into())),
                        ("ProductID".into(), prod),
                        ("VendorID".into(), vendor),
                        ("_CoreDevice_codablePropertyStorage".into(), storage),
                        ("_ServiceID".into(), svc_id),
                    ]),
                )]),
            )]),
        ),
    ])
}

fn build_uhid_create_consumer_xpc() -> XpcVal {
    let usage_page = XpcVal::Int64(0x0C);
    let usage = XpcVal::Int64(0x01);
    let vendor = XpcVal::Int64(0x05AC);
    let prod = XpcVal::Int64(0x0251);
    let svc_id = XpcVal::Uint64(UHID_CONSUMER_DEFAULT_SERVICE_ID);

    let storage = XpcVal::Dict(vec![
        (
            "Manufacturer".into(),
            XpcVal::Dict(vec![("string".into(), XpcVal::Str("Apple Inc.".into()))]),
        ),
        (
            "Product".into(),
            XpcVal::Dict(vec![("string".into(), XpcVal::Str("Apple Consumer Control".into()))]),
        ),
        ("ProductID".into(), XpcVal::Dict(vec![("int".into(), prod.clone())])),
        ("VendorID".into(), XpcVal::Dict(vec![("int".into(), vendor.clone())])),
        ("PrimaryUsage".into(), XpcVal::Dict(vec![("int".into(), usage.clone())])),
        (
            "PrimaryUsagePage".into(),
            XpcVal::Dict(vec![("int".into(), usage_page.clone())]),
        ),
        (
            "DeviceUsagePairs".into(),
            XpcVal::Dict(vec![(
                "array".into(),
                XpcVal::Array(vec![XpcVal::Dict(vec![(
                    "dictionary".into(),
                    XpcVal::Dict(vec![
                        ("DeviceUsage".into(), XpcVal::Dict(vec![("int".into(), usage.clone())])),
                        (
                            "DeviceUsagePage".into(),
                            XpcVal::Dict(vec![("int".into(), usage_page.clone())]),
                        ),
                    ]),
                )])]),
            )]),
        ),
        (
            "Transport".into(),
            XpcVal::Dict(vec![("string".into(), XpcVal::Str("USB".into()))]),
        ),
        (
            "ReportDescriptor".into(),
            XpcVal::Dict(vec![(
                "data".into(),
                XpcVal::Data(CONSUMER_CONTROL_REPORT_DESCRIPTOR_25B.to_vec()),
            )]),
        ),
        (
            "UniversalControlVirtualService".into(),
            XpcVal::Dict(vec![("bool".into(), XpcVal::Bool(true))]),
        ),
        ("_ServiceID".into(), XpcVal::Dict(vec![("uint".into(), svc_id.clone())])),
    ]);

    let top_pair = XpcVal::Dict(vec![
        ("DeviceUsage".into(), usage),
        ("DeviceUsagePage".into(), usage_page),
    ]);

    XpcVal::Dict(vec![
        (
            "featureIdentifier".into(),
            XpcVal::Str("com.apple.coredevice.feature.remote.universalhidservice".into()),
        ),
        ("messageType".into(), XpcVal::Str("Request".into())),
        (
            "payload".into(),
            XpcVal::Dict(vec![(
                "createService".into(),
                XpcVal::Dict(vec![(
                    "_0".into(),
                    XpcVal::Dict(vec![
                        ("DeviceUsagePairs".into(), XpcVal::Array(vec![top_pair])),
                        ("PrimaryUsage".into(), XpcVal::Uint64(0x01)),
                        ("PrimaryUsagePage".into(), XpcVal::Uint64(0x0C)),
                        ("Product".into(), XpcVal::Str("Apple Consumer Control".into())),
                        ("ProductID".into(), prod),
                        ("VendorID".into(), vendor),
                        ("_CoreDevice_codablePropertyStorage".into(), storage),
                        ("_ServiceID".into(), svc_id),
                    ]),
                )]),
            )]),
        ),
    ])
}

fn hid_tx_slot() -> &'static Mutex<Option<mpsc::Sender<PhysicalHidEnvelope>>> {
    PHYSICAL_HID_TX.get_or_init(|| Mutex::new(None))
}

pub fn is_physical_hid_active() -> bool {
    PHYSICAL_HID_ACTIVE.load(Ordering::SeqCst)
}

pub fn physical_hid_telemetry() -> (u64, u64, bool) {
    (
        PHYSICAL_HID_PACKETS_SENT.load(Ordering::SeqCst),
        PHYSICAL_HID_ERRORS.load(Ordering::SeqCst),
        PHYSICAL_HID_ACTIVE.load(Ordering::SeqCst),
    )
}

pub fn physical_h2_telemetry() -> (u64, u64) {
    (
        H2_PING_ACKS_SENT.load(Ordering::SeqCst),
        H2_WINDOW_UPDATES_SENT.load(Ordering::SeqCst),
    )
}

pub fn physical_tcp_acks_sent() -> u64 {
    TCP_ACKS_SENT.load(Ordering::SeqCst)
}

pub fn physical_rtcp_telemetry() -> (u64, u64, u64) {
    (
        RTCP_SR_RECEIVED.load(Ordering::SeqCst),
        RTCP_RR_WITH_LSR_SENT.load(Ordering::SeqCst),
        RTCP_BW_FEEDBACK_SENT.load(Ordering::SeqCst),
    )
}

pub fn physical_consumer_telemetry() -> (u64, u64) {
    (
        INDIGO_PING_TIMEOUT_RECONNECTS.load(Ordering::SeqCst),
        UHID_CONSUMER_REPORTS_SENT.load(Ordering::SeqCst),
    )
}

pub fn physical_display_xpc_pings_sent() -> u64 {
    DISPLAY_XPC_PINGS_SENT.load(Ordering::SeqCst)
}

pub fn physical_auto_reconnect_count() -> u64 {
    AUTO_RECONNECT_COUNT.load(Ordering::SeqCst)
}

pub fn physical_keep_awake_pulses_sent() -> u64 {
    KEEP_AWAKE_PULSES_SENT.load(Ordering::SeqCst)
}

pub fn set_last_disconnect_reason(reason: Option<String>) {
    if let Ok(mut g) = last_disconnect_reason_slot().lock() {
        *g = reason;
    }
}

pub fn last_disconnect_reason() -> Option<String> {
    last_disconnect_reason_slot()
        .lock()
        .ok()
        .and_then(|g| g.clone())
}

pub fn reset_physical_session_diagnostics() {
    DISPLAY_XPC_PINGS_SENT.store(0, Ordering::SeqCst);
    AUTO_RECONNECT_COUNT.store(0, Ordering::SeqCst);
    KEEP_AWAKE_PULSES_SENT.store(0, Ordering::SeqCst);
    LAST_HID_TX_EPOCH_MS.store(0, Ordering::SeqCst);
    TOUCH_CONTACT_DOWN.store(false, Ordering::SeqCst);
    set_last_disconnect_reason(None);
}

pub fn reset_physical_hid_counters_for_test() {
    PHYSICAL_HID_PACKETS_SENT.store(0, Ordering::SeqCst);
    PHYSICAL_HID_ERRORS.store(0, Ordering::SeqCst);
    H2_PING_ACKS_SENT.store(0, Ordering::SeqCst);
    H2_WINDOW_UPDATES_SENT.store(0, Ordering::SeqCst);
    TCP_ACKS_SENT.store(0, Ordering::SeqCst);
    RTCP_SR_RECEIVED.store(0, Ordering::SeqCst);
    RTCP_RR_WITH_LSR_SENT.store(0, Ordering::SeqCst);
    RTCP_BW_FEEDBACK_SENT.store(0, Ordering::SeqCst);
    INDIGO_PING_TIMEOUT_RECONNECTS.store(0, Ordering::SeqCst);
    UHID_CONSUMER_REPORTS_SENT.store(0, Ordering::SeqCst);
    reset_physical_session_diagnostics();
}

#[cfg(test)]
pub(crate) fn set_physical_hid_active_for_test(active: bool) {
    PHYSICAL_HID_ACTIVE.store(active, Ordering::SeqCst);
}

pub fn enqueue_physical_hid_command(cmd: PhysicalHidCommand) -> bool {
    let tx_opt = {
        let guard = hid_tx_slot().lock().unwrap_or_else(|e| e.into_inner());
        guard.clone()
    };
    let tx = match tx_opt {
        Some(t) => t,
        None => {
            // Brief retry if display stream is right at stage 4 transition
            let mut found = None;
            for _ in 0..8 {
                std::thread::sleep(Duration::from_millis(50));
                if let Some(t) = hid_tx_slot().lock().unwrap_or_else(|e| e.into_inner()).clone() {
                    found = Some(t);
                    break;
                }
            }
            match found {
                Some(t) => t,
                None => return false,
            }
        }
    };
    let (ack_tx, ack_rx) = mpsc::channel();
    if tx.send(PhysicalHidEnvelope { cmd, ack: Some(ack_tx) }).is_err() {
        return false;
    }
    match ack_rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(_)) => true,
        Ok(Err(_)) => false,
        Err(_) => false,
    }
}

struct HidSlotGuard;
impl Drop for HidSlotGuard {
    fn drop(&mut self) {
        PHYSICAL_HID_ACTIVE.store(false, Ordering::SeqCst);
        if let Ok(mut g) = hid_tx_slot().lock() {
            *g = None;
        }
    }
}

async fn ensure_keyboard_surface_async(
    router: &CdTunnelRouter,
    uhid_port: u16,
    uhid_opt: &mut Option<XpcH2Session>,
    kb_id_opt: &mut Option<u64>,
    auth_ready_at: Instant,
) -> Result<u64, String> {
    if let Some(id) = *kb_id_opt {
        return Ok(id);
    }
    let now = Instant::now();
    if now < auth_ready_at {
        std::thread::sleep(auth_ready_at - now);
    }
    let uhid = match uhid_opt.as_mut() {
        Some(u) => u,
        None => {
            if uhid_port == 0 {
                return Err("universalhidservice port unavailable".into());
            }
            let opened = XpcH2Session::open(router, uhid_port).await?;
            *uhid_opt = Some(opened);
            uhid_opt.as_mut().unwrap()
        }
    };
    uhid.conn.read_timeout = Duration::from_millis(2500);
    let req = build_uhid_create_keyboard_xpc();
    let resp = uhid.rpc(&req, true).await?;
    let assigned = resp
        .dict_get("serviceID")
        .and_then(XpcVal::as_u64)
        .unwrap_or(UHID_KEYBOARD_DEFAULT_SERVICE_ID);
    std::thread::sleep(Duration::from_millis(KEYBOARD_ATTACH_SETTLE_MS));
    *kb_id_opt = Some(assigned);
    Ok(assigned)
}

async fn ensure_consumer_surface_async(
    router: &CdTunnelRouter,
    uhid_port: u16,
    uhid_opt: &mut Option<XpcH2Session>,
    consumer_id_opt: &mut Option<u64>,
    auth_ready_at: Instant,
) -> Result<u64, String> {
    if let Some(id) = *consumer_id_opt {
        return Ok(id);
    }
    let now = Instant::now();
    if now < auth_ready_at {
        std::thread::sleep(auth_ready_at - now);
    }
    let uhid = match uhid_opt.as_mut() {
        Some(u) => u,
        None => {
            if uhid_port == 0 {
                return Err("universalhidservice port unavailable".into());
            }
            let opened = tokio::time::timeout(
                Duration::from_millis(1200),
                XpcH2Session::open(router, uhid_port),
            )
            .await
            .map_err(|_| "universalhidservice consumer connect timeout".to_string())??;
            *uhid_opt = Some(opened);
            uhid_opt.as_mut().unwrap()
        }
    };
    uhid.conn.read_timeout = Duration::from_millis(1500);
    let req = build_uhid_create_consumer_xpc();
    let resp = uhid.rpc(&req, true).await?;
    let assigned = resp
        .dict_get("serviceID")
        .and_then(XpcVal::as_u64)
        .unwrap_or(UHID_CONSUMER_DEFAULT_SERVICE_ID);
    std::thread::sleep(Duration::from_millis(CONSUMER_ATTACH_SETTLE_MS));
    *consumer_id_opt = Some(assigned);
    Ok(assigned)
}

async fn send_consumer_uhid_event(
    router: &CdTunnelRouter,
    uhid_port: u16,
    uhid_opt: &mut Option<XpcH2Session>,
    consumer_id_opt: &mut Option<u64>,
    kb_id_opt: &mut Option<u64>,
    auth_ready_at: Instant,
    usage: u16,
    is_down: bool,
) -> Result<(), String> {
    let svc_id = ensure_consumer_surface_async(
        router,
        uhid_port,
        uhid_opt,
        consumer_id_opt,
        auth_ready_at,
    )
    .await?;
    let rep = build_consumer_report_3b(if is_down { usage } else { 0 });
    let req = build_uhid_send_xpc(rep, svc_id);
    send_xpc_oneway_auto_heal(
        router,
        uhid_port,
        uhid_opt,
        &req,
        Some(kb_id_opt),
        "universalhidservice-consumer",
    )
    .await?;
    UHID_CONSUMER_REPORTS_SENT.fetch_add(1, Ordering::SeqCst);
    Ok(())
}

async fn send_xpc_oneway_auto_heal(
    router: &CdTunnelRouter,
    port: u16,
    session_opt: &mut Option<XpcH2Session>,
    req: &XpcVal,
    kb_id_opt: Option<&mut Option<u64>>,
    label: &str,
) -> Result<(), String> {
    if session_opt.is_none() && port > 0 {
        *session_opt = tokio::time::timeout(
            Duration::from_millis(1200),
            XpcH2Session::open(router, port),
        )
        .await
        .ok()
        .and_then(|r| r.ok());
    }
    if let Some(sess) = session_opt.as_mut() {
        if sess.send_oneway(req).is_ok() {
            return Ok(());
        }
    }
    // Auto-heal HTTP/2 XPC sub-stream once after TCP reset/FIN/stall (bounded to 1200 ms)
    *session_opt = None;
    if let Some(kb) = kb_id_opt {
        *kb = None;
    }
    if port == 0 {
        return Err(format!("{label} port unavailable"));
    }
    let mut reopened = tokio::time::timeout(
        Duration::from_millis(1200),
        XpcH2Session::open(router, port),
    )
    .await
    .map_err(|_| format!("{label} reconnect timeout"))??;
    if label.contains("indigo") {
        settle_fresh_indigo_xpc(&mut reopened).await;
    }
    reopened.send_oneway(req)?;
    *session_opt = Some(reopened);
    Ok(())
}

fn spawn_physical_hid_worker(
    router: Arc<CdTunnelRouter>,
    indigo_port: u16,
    uhid_port: u16,
    session_gen: u64,
    stop: Arc<AtomicBool>,
    rx: mpsc::Receiver<PhysicalHidEnvelope>,
) {
    let _ = std::thread::Builder::new()
        .name("iphone-physical-hid".into())
        .spawn(move || {
            runtime().block_on(async move {
                let auth_ready_at = Instant::now() + Duration::from_millis(TOUCH_AUTH_SETTLE_MS);
                let mut indigo_xpc: Option<XpcH2Session> = if indigo_port > 0 {
                    let mut opened = tokio::time::timeout(
                        Duration::from_millis(1500),
                        XpcH2Session::open(&router, indigo_port),
                    )
                    .await
                    .ok()
                    .and_then(|r| r.ok());
                    if let Some(ref mut s) = opened {
                        settle_fresh_indigo_xpc(s).await;
                    }
                    opened
                } else {
                    None
                };
                let mut uhid_xpc: Option<XpcH2Session> = if uhid_port > 0 {
                    tokio::time::timeout(
                        Duration::from_millis(1500),
                        XpcH2Session::open(&router, uhid_port),
                    )
                    .await
                    .ok()
                    .and_then(|r| r.ok())
                } else {
                    None
                };
                let mut kb_service_id: Option<u64> = None;
                let mut consumer_service_id: Option<u64> = None;
                let mut pending_next: Option<PhysicalHidEnvelope> = None;
                let mut last_real_hid_at = Instant::now();

                let ensure_auth_settle = || {
                    let now = Instant::now();
                    if now < auth_ready_at {
                        std::thread::sleep(auth_ready_at - now);
                    }
                };

                while !stop.load(Ordering::Relaxed)
                    && crate::mirror::current_live_session_gen() == session_gen
                {
                    // Tahap 243 Pilar 1 (1:1 ZenLink Parity): Keep indigo_xpc & uhid_xpc 100% passive-calm
                    // (drain-only) during live streaming — answer every iOS TCP ACK, HTTP/2 PING ACK, and
                    // WINDOW_UPDATE, and NEVER send unsolicited HTTP/2 PING or force-reopen idle indigo_xpc
                    // at t=30..35s (which previously destabilized iOS remoted/RTP for 20 seconds!).
                    let drain_both = |ind_opt: &mut Option<XpcH2Session>,
                                      uh_opt: &mut Option<XpcH2Session>,
                                      kb_opt: &mut Option<u64>,
                                      cons_opt: &mut Option<u64>| {
                        if let Some(ref mut ind) = ind_opt {
                            if ind.drain_incoming_h2_nonblocking().is_err() {
                                INDIGO_PING_TIMEOUT_RECONNECTS.fetch_add(1, Ordering::SeqCst);
                                *ind_opt = None;
                            }
                        }
                        if let Some(ref mut uhid) = uh_opt {
                            if uhid.drain_incoming_h2_nonblocking().is_err() {
                                *uh_opt = None;
                                *kb_opt = None;
                                *cons_opt = None;
                            }
                        }
                    };

                    // Tahap 242 Safe Idle Keep-Awake (45 s): prevents iOS auto-lock screen blanking during
                    // long passive viewing without ever touching brightness, volume, or active touch drags.
                    if last_real_hid_at.elapsed() >= Duration::from_secs(45)
                        && !crate::hid::is_device_intentionally_locked()
                        && !TOUCH_CONTACT_DOWN.load(Ordering::Relaxed)
                        && uhid_port > 0
                    {
                        last_real_hid_at = Instant::now();
                        if let Ok(svc_id) = ensure_keyboard_surface_async(
                            &router,
                            uhid_port,
                            &mut uhid_xpc,
                            &mut kb_service_id,
                            auth_ready_at,
                        )
                        .await
                        {
                            let down_rep = build_keyboard_report_39b(&[0xE0], monotonic_ns_48());
                            let down_req = build_uhid_send_xpc(down_rep, svc_id);
                            if send_xpc_oneway_auto_heal(
                                &router,
                                uhid_port,
                                &mut uhid_xpc,
                                &down_req,
                                Some(&mut kb_service_id),
                                "universalhidservice-keepawake",
                            )
                            .await
                            .is_ok()
                            {
                                std::thread::sleep(Duration::from_millis(12));
                                let up_rep = build_keyboard_report_39b(&[], monotonic_ns_48());
                                let up_req = build_uhid_send_xpc(up_rep, svc_id);
                                let _ = send_xpc_oneway_auto_heal(
                                    &router,
                                    uhid_port,
                                    &mut uhid_xpc,
                                    &up_req,
                                    Some(&mut kb_service_id),
                                    "universalhidservice-keepawake",
                                )
                                .await;
                                KEEP_AWAKE_PULSES_SENT.fetch_add(1, Ordering::SeqCst);
                            }
                        }
                    }

                    let mut env = if let Some(next_env) = pending_next.take() {
                        next_env
                    } else {
                        match rx.recv_timeout(Duration::from_millis(20)) {
                            Ok(env) => env,
                            Err(mpsc::RecvTimeoutError::Timeout) => {
                                drain_both(
                                    &mut indigo_xpc,
                                    &mut uhid_xpc,
                                    &mut kb_service_id,
                                    &mut consumer_service_id,
                                );
                                continue;
                            }
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        }
                    };
                    last_real_hid_at = Instant::now();
                    LAST_HID_TX_EPOCH_MS.store(now_epoch_ms_u64(), Ordering::Relaxed);
                    drain_both(
                        &mut indigo_xpc,
                        &mut uhid_xpc,
                        &mut kb_service_id,
                        &mut consumer_service_id,
                    );

                    // Touch Move Coalescing: drain back-to-back intermediate Touch(contact=true) moves
                    // while preserving releases, gestures, buttons, and keyboard ordering.
                    if matches!(env.cmd, PhysicalHidCommand::Touch { contact: true, .. }) {
                        while let Ok(peek_env) = rx.try_recv() {
                            if matches!(peek_env.cmd, PhysicalHidCommand::Touch { contact: true, .. })
                            {
                                PHYSICAL_HID_PACKETS_SENT.fetch_add(1, Ordering::SeqCst);
                                if let Some(old_ack) = env.ack.take() {
                                    let _ = old_ack.send(Ok(1));
                                }
                                env = peek_env;
                            } else {
                                pending_next = Some(peek_env);
                                break;
                            }
                        }
                    }

                    match &env.cmd {
                        PhysicalHidCommand::Touch { contact, .. } => {
                            TOUCH_CONTACT_DOWN.store(*contact, Ordering::SeqCst);
                        }
                        PhysicalHidCommand::TouchSequence { .. } => {
                            TOUCH_CONTACT_DOWN.store(false, Ordering::SeqCst);
                        }
                        _ => {}
                    }

                    let mut sent_this_cmd = 0u64;
                    let res: Result<u64, String> = async {
                        match env.cmd {
                            PhysicalHidCommand::Touch { contact, x, y } => {
                                ensure_auth_settle();
                                let report = build_touchscreen_report_58b(
                                    contact,
                                    x,
                                    y,
                                    monotonic_ns_48(),
                                );
                                let req = build_uhid_send_xpc(report, UHID_TOUCHSCREEN_SERVICE_ID);
                                send_xpc_oneway_auto_heal(
                                    &router,
                                    uhid_port,
                                    &mut uhid_xpc,
                                    &req,
                                    Some(&mut kb_service_id),
                                    "universalhidservice",
                                )
                                .await?;
                                sent_this_cmd += 1;
                                Ok(sent_this_cmd)
                            }
                            PhysicalHidCommand::TouchSequence { steps } => {
                                ensure_auth_settle();
                                for (contact, x, y, delay_ms) in steps {
                                    let report = build_touchscreen_report_58b(
                                        contact,
                                        x,
                                        y,
                                        monotonic_ns_48(),
                                    );
                                    let req =
                                        build_uhid_send_xpc(report, UHID_TOUCHSCREEN_SERVICE_ID);
                                    send_xpc_oneway_auto_heal(
                                        &router,
                                        uhid_port,
                                        &mut uhid_xpc,
                                        &req,
                                        Some(&mut kb_service_id),
                                        "universalhidservice",
                                    )
                                    .await?;
                                    sent_this_cmd += 1;
                                    if delay_ms > 0 {
                                        std::thread::sleep(Duration::from_millis(delay_ms));
                                        if let Some(ref mut ind) = indigo_xpc {
                                            if ind.drain_incoming_h2_nonblocking().is_err() {
                                                indigo_xpc = None;
                                            }
                                        }
                                    }
                                }
                                Ok(sent_this_cmd)
                            }
                            PhysicalHidCommand::Button { page, usage, is_down } => {
                                // Keep healthy passive-calm indigo_xpc warm (Tahap 271: 0 idle timer kill;
                                // drain_incoming_h2_nonblocking detects any true peer close/GOAWAY).
                                if let Some(ref mut ind) = indigo_xpc {
                                    if ind.drain_incoming_h2_nonblocking().is_err() {
                                        indigo_xpc = None;
                                    }
                                }
                                let req = build_indigo_button_xpc(page, usage, is_down);
                                let ind_res = send_xpc_oneway_auto_heal(
                                    &router,
                                    indigo_port,
                                    &mut indigo_xpc,
                                    &req,
                                    None,
                                    "indigo HID service",
                                )
                                .await;
                                if ind_res.is_ok() {
                                    sent_this_cmd += 1;
                                    Ok(sent_this_cmd)
                                } else if page == 0x0C && uhid_port > 0 {
                                    send_consumer_uhid_event(
                                        &router,
                                        uhid_port,
                                        &mut uhid_xpc,
                                        &mut consumer_service_id,
                                        &mut kb_service_id,
                                        auth_ready_at,
                                        usage,
                                        is_down,
                                    )
                                    .await?;
                                    sent_this_cmd += 1;
                                    Ok(sent_this_cmd)
                                } else {
                                    Err(ind_res.unwrap_err())
                                }
                            }
                            PhysicalHidCommand::ButtonPress { page, usage, hold_ms } => {
                                // Release any lingering touch contact before Power/Lock or Wake button dispatch
                                if TOUCH_CONTACT_DOWN.swap(false, Ordering::SeqCst) && uhid_port > 0 {
                                    let rel = build_touchscreen_report_58b(
                                        false,
                                        32768,
                                        32768,
                                        monotonic_ns_48(),
                                    );
                                    let rel_req = build_uhid_send_xpc(rel, UHID_TOUCHSCREEN_SERVICE_ID);
                                    let _ = send_xpc_oneway_auto_heal(
                                        &router,
                                        uhid_port,
                                        &mut uhid_xpc,
                                        &rel_req,
                                        Some(&mut kb_service_id),
                                        "universalhidservice",
                                    )
                                    .await;
                                }
                                let effective_hold = if usage == 0x0030 {
                                    clamp_power_lock_hold_ms(hold_ms)
                                } else {
                                    hold_ms
                                };
                                // Exact Tahap 259 Warm-Session Parity: NEVER kill healthy indigo_xpc on an idle
                                // timer; only reopen if drain_incoming_h2_nonblocking() detects peer FIN/GOAWAY.
                                if let Some(ref mut ind) = indigo_xpc {
                                    if ind.drain_incoming_h2_nonblocking().is_err() {
                                        indigo_xpc = None;
                                    }
                                }
                                if indigo_xpc.is_none() && indigo_port > 0 {
                                    let mut opened = tokio::time::timeout(
                                        Duration::from_millis(1200),
                                        XpcH2Session::open(&router, indigo_port),
                                    )
                                    .await
                                    .ok()
                                    .and_then(|r| r.ok());
                                    if let Some(ref mut s) = opened {
                                        settle_fresh_indigo_xpc(s).await;
                                    }
                                    indigo_xpc = opened;
                                }

                                // Atomic down + flush + router flush barrier + hold + up + flush on the SAME indigo_xpc session
                                // (with Anti-Siri Up-Retry Guard: never resend Down if Up fails after Down was transmitted).
                                let mut wire_channel = "indigo_xpc";
                                let mut indigo_pair_ok = false;
                                let down = build_indigo_button_xpc(page, usage, true);
                                let mut down_ok = false;
                                if let Some(ref mut sess) = indigo_xpc {
                                    if sess.send_oneway(&down).is_ok() {
                                        down_ok = true;
                                    } else {
                                        indigo_xpc = None;
                                    }
                                }
                                if !down_ok && indigo_xpc.is_none() && indigo_port > 0 {
                                    let mut reopened = tokio::time::timeout(
                                        Duration::from_millis(1200),
                                        XpcH2Session::open(&router, indigo_port),
                                    )
                                    .await
                                    .ok()
                                    .and_then(|r| r.ok());
                                    if let Some(ref mut s) = reopened {
                                        settle_fresh_indigo_xpc(s).await;
                                        if s.send_oneway(&down).is_ok() {
                                            down_ok = true;
                                            indigo_xpc = reopened;
                                        }
                                    }
                                }
                                if down_ok {
                                    if let Some(ref mut sess) = indigo_xpc {
                                        sess.flush().await;
                                        router.flush_tx().await;
                                        sent_this_cmd += 1;
                                        if effective_hold > 0 {
                                            tokio::time::sleep(Duration::from_millis(effective_hold)).await;
                                        }
                                        let up = build_indigo_button_xpc(page, usage, false);
                                        if sess.send_oneway(&up).is_ok() {
                                            sess.flush().await;
                                            router.flush_tx().await;
                                            sent_this_cmd += 1;
                                            indigo_pair_ok = true;
                                        } else {
                                            // Down ALREADY landed on iOS! Drop dead stream and retry Up ONLY once.
                                            indigo_xpc = None;
                                            if indigo_port > 0 {
                                                let mut rel_sess = tokio::time::timeout(
                                                    Duration::from_millis(600),
                                                    XpcH2Session::open(&router, indigo_port),
                                                )
                                                .await
                                                .ok()
                                                .and_then(|r| r.ok());
                                                if let Some(ref mut rs) = rel_sess {
                                                    settle_fresh_indigo_xpc(rs).await;
                                                    if rs.send_oneway(&up).is_ok() {
                                                        rs.flush().await;
                                                        router.flush_tx().await;
                                                        sent_this_cmd += 1;
                                                        indigo_pair_ok = true;
                                                        wire_channel = "indigo_xpc_up_retry";
                                                        indigo_xpc = rel_sess;
                                                    }
                                                }
                                            }
                                            if !indigo_pair_ok && page == 0x0C && uhid_port > 0 {
                                                if send_consumer_uhid_event(
                                                    &router,
                                                    uhid_port,
                                                    &mut uhid_xpc,
                                                    &mut consumer_service_id,
                                                    &mut kb_service_id,
                                                    auth_ready_at,
                                                    usage,
                                                    false,
                                                )
                                                .await
                                                .is_ok()
                                                {
                                                    router.flush_tx().await;
                                                    sent_this_cmd += 1;
                                                    wire_channel = "uhid_up_recovery";
                                                    indigo_pair_ok = true;
                                                }
                                            }
                                            if !indigo_pair_ok {
                                                if page == 0x0C && usage == 0x0030 {
                                                    crate::hid::record_lock_wire_outcome(
                                                        wire_channel,
                                                        false,
                                                        Some("indigo Up retry failed after Down"),
                                                    );
                                                }
                                                return Err("indigo HID Up retry failed after Down".into());
                                            }
                                        }
                                    }
                                }

                                if indigo_pair_ok {
                                    if page == 0x0C && usage == 0x0030 {
                                        crate::hid::record_lock_wire_outcome(wire_channel, true, None);
                                    }
                                    Ok(sent_this_cmd)
                                } else if page == 0x0C && uhid_port > 0 {
                                    // Only run Consumer Control Down+hold+Up fallback when Indigo Down never went out!
                                    send_consumer_uhid_event(
                                        &router,
                                        uhid_port,
                                        &mut uhid_xpc,
                                        &mut consumer_service_id,
                                        &mut kb_service_id,
                                        auth_ready_at,
                                        usage,
                                        true,
                                    )
                                    .await?;
                                    router.flush_tx().await;
                                    sent_this_cmd += 1;
                                    if effective_hold > 0 {
                                        tokio::time::sleep(Duration::from_millis(effective_hold)).await;
                                    }
                                    send_consumer_uhid_event(
                                        &router,
                                        uhid_port,
                                        &mut uhid_xpc,
                                        &mut consumer_service_id,
                                        &mut kb_service_id,
                                        auth_ready_at,
                                        usage,
                                        false,
                                    )
                                    .await?;
                                    router.flush_tx().await;
                                    sent_this_cmd += 1;
                                    if usage == 0x0030 {
                                        crate::hid::record_lock_wire_outcome("uhid_consumer", true, None);
                                    }
                                    Ok(sent_this_cmd)
                                } else {
                                    if page == 0x0C && usage == 0x0030 {
                                        crate::hid::record_lock_wire_outcome(
                                            "indigo_xpc",
                                            false,
                                            Some("indigo HID service atomic button pair failed"),
                                        );
                                    }
                                    Err("indigo HID service atomic button pair failed".into())
                                }
                            }
                            PhysicalHidCommand::HomeCombo => {
                                // 1) dtuhidd-FIRST: Force Touch Release + 6-step bottom Home-Bar Swipe-Up (65500 -> 26000)
                                // Never blocked by stale Indigo session after long keyboard/paste bursts!
                                ensure_auth_settle();
                                let swipe_steps: [(bool, u16, u16, u64); 9] = [
                                    (false, 32768, 32768, 8),
                                    (true, 32768, 65500, 10),
                                    (true, 32768, 59000, 10),
                                    (true, 32768, 52500, 10),
                                    (true, 32768, 46000, 10),
                                    (true, 32768, 39500, 10),
                                    (true, 32768, 32500, 10),
                                    (true, 32768, 26000, 10),
                                    (false, 32768, 26000, 0),
                                ];
                                for (contact, x, y, delay_ms) in swipe_steps {
                                    let report = build_touchscreen_report_58b(
                                        contact,
                                        x,
                                        y,
                                        monotonic_ns_48(),
                                    );
                                    let req = build_uhid_send_xpc(
                                        report,
                                        UHID_TOUCHSCREEN_SERVICE_ID,
                                    );
                                    send_xpc_oneway_auto_heal(
                                        &router,
                                        uhid_port,
                                        &mut uhid_xpc,
                                        &req,
                                        Some(&mut kb_service_id),
                                        "universalhidservice",
                                    )
                                    .await?;
                                    sent_this_cmd += 1;
                                    if delay_ms > 0 {
                                        std::thread::sleep(Duration::from_millis(delay_ms));
                                    }
                                }
                                // 2) Best-effort non-blocking Indigo Consumer Home (0x0C, 0x0040) capped at 150 ms
                                if indigo_port > 0 {
                                    let down = build_indigo_button_xpc(0x0C, 0x0040, true);
                                    let ind_ok = tokio::time::timeout(
                                        Duration::from_millis(150),
                                        send_xpc_oneway_auto_heal(
                                            &router,
                                            indigo_port,
                                            &mut indigo_xpc,
                                            &down,
                                            None,
                                            "indigo HID service",
                                        ),
                                    )
                                    .await
                                    .ok()
                                    .and_then(|r| r.ok())
                                    .is_some();
                                    if ind_ok {
                                        sent_this_cmd += 1;
                                        std::thread::sleep(Duration::from_millis(30));
                                        let up = build_indigo_button_xpc(0x0C, 0x0040, false);
                                        if tokio::time::timeout(
                                            Duration::from_millis(150),
                                            send_xpc_oneway_auto_heal(
                                                &router,
                                                indigo_port,
                                                &mut indigo_xpc,
                                                &up,
                                                None,
                                                "indigo HID service",
                                            ),
                                        )
                                        .await
                                        .ok()
                                        .and_then(|r| r.ok())
                                        .is_some()
                                        {
                                            sent_this_cmd += 1;
                                        }
                                    }
                                }
                                Ok(sent_this_cmd)
                            }
                            PhysicalHidCommand::ButtonSequence { events } => {
                                // Keep healthy warm indigo_xpc alive (0 idle timer force-kill;
                                // only reopen when drain_incoming_h2_nonblocking detects peer FIN/GOAWAY).
                                if let Some(ref mut ind) = indigo_xpc {
                                    if ind.drain_incoming_h2_nonblocking().is_err() {
                                        indigo_xpc = None;
                                    }
                                }
                                let mut use_uhid_consumer = indigo_port == 0;
                                for (page, usage, is_down, delay_ms) in events {
                                    if !use_uhid_consumer {
                                        let req = build_indigo_button_xpc(page, usage, is_down);
                                        if send_xpc_oneway_auto_heal(
                                            &router,
                                            indigo_port,
                                            &mut indigo_xpc,
                                            &req,
                                            None,
                                            "indigo HID service",
                                        )
                                        .await
                                        .is_err()
                                        {
                                            if page == 0x0C && uhid_port > 0 {
                                                use_uhid_consumer = true;
                                            } else {
                                                return Err("indigo HID service failed".into());
                                            }
                                        }
                                    }
                                    if use_uhid_consumer && page == 0x0C && uhid_port > 0 {
                                        send_consumer_uhid_event(
                                            &router,
                                            uhid_port,
                                            &mut uhid_xpc,
                                            &mut consumer_service_id,
                                            &mut kb_service_id,
                                            auth_ready_at,
                                            usage,
                                            is_down,
                                        )
                                        .await?;
                                    }
                                    sent_this_cmd += 1;
                                    if delay_ms > 0 {
                                        std::thread::sleep(Duration::from_millis(delay_ms));
                                    }
                                }
                                Ok(sent_this_cmd)
                            }
                            PhysicalHidCommand::KeyboardUsages { usages } => {
                                let svc_id = ensure_keyboard_surface_async(
                                    &router,
                                    uhid_port,
                                    &mut uhid_xpc,
                                    &mut kb_service_id,
                                    auth_ready_at,
                                )
                                .await?;
                                let report =
                                    build_keyboard_report_39b(&usages, monotonic_ns_48());
                                let req = build_uhid_send_xpc(report, svc_id);
                                if send_xpc_oneway_auto_heal(
                                    &router,
                                    uhid_port,
                                    &mut uhid_xpc,
                                    &req,
                                    Some(&mut kb_service_id),
                                    "universalhidservice",
                                )
                                .await
                                .is_err()
                                {
                                    let new_svc = ensure_keyboard_surface_async(
                                        &router,
                                        uhid_port,
                                        &mut uhid_xpc,
                                        &mut kb_service_id,
                                        auth_ready_at,
                                    )
                                    .await?;
                                    let report2 =
                                        build_keyboard_report_39b(&usages, monotonic_ns_48());
                                    let req2 = build_uhid_send_xpc(report2, new_svc);
                                    uhid_xpc
                                        .as_mut()
                                        .ok_or_else(|| "universalhidservice unavailable".to_string())?
                                        .send_oneway(&req2)?;
                                }
                                sent_this_cmd += 1;
                                Ok(sent_this_cmd)
                            }
                            PhysicalHidCommand::KeyboardSequence { steps } => {
                                let mut svc_id = ensure_keyboard_surface_async(
                                    &router,
                                    uhid_port,
                                    &mut uhid_xpc,
                                    &mut kb_service_id,
                                    auth_ready_at,
                                )
                                .await?;
                                let mut last_was_empty = false;
                                for (step_idx, (usages, delay_ms)) in steps.into_iter().enumerate() {
                                    last_was_empty = usages.is_empty();
                                    let report = build_keyboard_report_39b(
                                        &usages,
                                        monotonic_ns_48(),
                                    );
                                    let req = build_uhid_send_xpc(report, svc_id);
                                    if send_xpc_oneway_auto_heal(
                                        &router,
                                        uhid_port,
                                        &mut uhid_xpc,
                                        &req,
                                        Some(&mut kb_service_id),
                                        "universalhidservice",
                                    )
                                    .await
                                    .is_err()
                                    {
                                        svc_id = ensure_keyboard_surface_async(
                                            &router,
                                            uhid_port,
                                            &mut uhid_xpc,
                                            &mut kb_service_id,
                                            auth_ready_at,
                                        )
                                        .await?;
                                        let report2 = build_keyboard_report_39b(
                                            &usages,
                                            monotonic_ns_48(),
                                        );
                                        let req2 = build_uhid_send_xpc(report2, svc_id);
                                        uhid_xpc
                                            .as_mut()
                                            .ok_or_else(|| "universalhidservice unavailable".to_string())?
                                            .send_oneway(&req2)?;
                                    }
                                    sent_this_cmd += 1;
                                    // Keep Indigo + UHID TCP/H2 ACK windows fresh during long paste bursts
                                    if step_idx % 8 == 7 {
                                        if let Some(ref mut ind) = indigo_xpc {
                                            if ind.drain_incoming_h2_nonblocking().is_err() {
                                                indigo_xpc = None;
                                            }
                                        }
                                        if let Some(ref mut uh) = uhid_xpc {
                                            if uh.drain_incoming_h2_nonblocking().is_err() {
                                                uhid_xpc = None;
                                                kb_service_id = None;
                                            }
                                        }
                                    }
                                    if delay_ms > 0 {
                                        std::thread::sleep(Duration::from_millis(delay_ms));
                                    }
                                }
                                // Always guarantee an all-keys-released trailing report so iOS never holds a stuck modifier
                                if !last_was_empty {
                                    let release_rep = build_keyboard_report_39b(&[], monotonic_ns_48());
                                    let release_req = build_uhid_send_xpc(release_rep, svc_id);
                                    let _ = send_xpc_oneway_auto_heal(
                                        &router,
                                        uhid_port,
                                        &mut uhid_xpc,
                                        &release_req,
                                        Some(&mut kb_service_id),
                                        "universalhidservice",
                                    )
                                    .await;
                                    sent_this_cmd += 1;
                                }
                                Ok(sent_this_cmd)
                            }
                        }
                    }
                    .await;

                    match &res {
                        Ok(n) => {
                            PHYSICAL_HID_PACKETS_SENT.fetch_add(*n, Ordering::SeqCst);
                        }
                        Err(_) => {
                            PHYSICAL_HID_ERRORS.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                    if let Some(ack) = env.ack {
                        let _ = ack.send(res);
                    }
                }
            });
        });
}

/// Legacy default `session_id` (our SSRC) embedded inside the original Xcode offer template (`0x8D2E8101`).
const OFFER_TEMPLATE_LOCAL_SSRC: u32 = 0x8D2E_8101;

fn extract_rtcp_from_xpc_answer_with_default(
    ans: &XpcVal,
    default_local_ssrc: u32,
) -> (u16, u32, u32) {
    let sc = ans
        .dict_get("CoreDevice.output")
        .and_then(|o| o.dict_get("connection"))
        .and_then(|c| c.dict_get("streamConfig"))
        .or_else(|| ans.find_key_recursive("streamConfig"));
    let port = sc
        .and_then(|s| s.find_key_recursive("SourcePort"))
        .and_then(XpcVal::as_u32_lossless)
        .unwrap_or(0) as u16;
    let fallback_ours = if default_local_ssrc != 0 {
        default_local_ssrc
    } else {
        OFFER_TEMPLATE_LOCAL_SSRC
    };
    let ours = sc
        .and_then(|s| s.find_key_recursive("RemoteSSRC"))
        .and_then(XpcVal::as_u32_lossless)
        .filter(|&v| v != 0)
        .unwrap_or(fallback_ours);
    let theirs = sc
        .and_then(|s| s.find_key_recursive("LocalSSRC"))
        .and_then(XpcVal::as_u32_lossless)
        .unwrap_or(0);
    (port, ours, theirs)
}

#[allow(dead_code)]
fn extract_rtcp_from_xpc_answer(ans: &XpcVal) -> (u16, u32, u32) {
    extract_rtcp_from_xpc_answer_with_default(ans, OFFER_TEMPLATE_LOCAL_SSRC)
}

pub fn sanitize_udid_label(raw: &str) -> String {
    let mut s = raw.trim().to_string();
    for suffix in [
        " (Wi-Fi)",
        " (WiFi)",
        " (USB)",
        " (Cable)",
        " (Offline)",
        " (Online)",
    ] {
        if let Some(pos) = s.to_ascii_lowercase().find(&suffix.to_ascii_lowercase()) {
            s.truncate(pos);
        }
    }
    s.trim().to_string()
}

async fn run_physical_usb_display_async(
    udid: String,
    session_gen: u64,
    stop: Arc<AtomicBool>,
) -> Result<(), String> {
    run_physical_usb_display_async_ex(udid, session_gen, stop, false).await
}

async fn run_physical_usb_display_async_ex(
    udid: String,
    session_gen: u64,
    stop: Arc<AtomicBool>,
    gate_only: bool,
) -> Result<(), String> {
    let clean_udid = sanitize_udid_label(&udid);
    let mut matched_dev = None;
    let mut last_err = String::new();
    for attempt in 0..5u8 {
        match usbmux().await {
            Ok(mut mux) => match mux.get_devices().await {
                Ok(devs) => {
                    let exact_usb = devs.iter().find(|d| {
                        d.connection_type == Connection::Usb
                            && (clean_udid.is_empty()
                                || d.udid.eq_ignore_ascii_case(&clean_udid))
                    });
                    if let Some(found) = exact_usb {
                        matched_dev = Some(found.clone());
                        break;
                    }
                    if let Some(any_usb) =
                        devs.iter().find(|d| d.connection_type == Connection::Usb)
                    {
                        matched_dev = Some(any_usb.clone());
                        break;
                    }
                    // Also accept Network usbmuxd entries (Wi-Fi synced devices) as fallback
                    let exact_net = devs.iter().find(|d| {
                        matches!(d.connection_type, Connection::Network(_))
                            && (clean_udid.is_empty()
                                || d.udid.eq_ignore_ascii_case(&clean_udid))
                    });
                    if let Some(found_net) = exact_net {
                        matched_dev = Some(found_net.clone());
                        break;
                    }
                    if let Some(any_net) = devs
                        .iter()
                        .find(|d| matches!(d.connection_type, Connection::Network(_)))
                    {
                        matched_dev = Some(any_net.clone());
                        break;
                    }
                }
                Err(e) => {
                    last_err = format!("usbmuxd list: {e}");
                }
            },
            Err(e) => {
                last_err = e;
            }
        }
        if attempt < 4 {
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    }
    let dev = matched_dev.ok_or_else(|| {
        if last_err.is_empty() {
            "iPhone USB tidak ditemukan di usbmuxd. Pastikan kabel terhubung dan layar iPhone terbuka.".to_string()
        } else {
            crate::diag_log::sanitize_ui_no_raw_udid(&last_err)
        }
    })?;
    let _udid = dev.udid.clone();
    let addr = UsbmuxdAddr::from_env_var().unwrap_or_default();
    let provider = dev.to_provider(addr, "iphone-link");

    if !gate_only {
        crate::diag_log::record("usb_tunnel", "CoreDeviceProxy connecting...");
        crate::mirror::update_stage_status(
            session_gen,
            2,
            "Koneksi aman",
            "Membuka terowongan CoreDeviceProxy TLS ke iPhone...",
        );
    }

    let proxy = tokio::time::timeout(
        Duration::from_secs(10),
        CoreDeviceProxy::connect(&provider),
    )
    .await
    .map_err(|_| {
        crate::diag_log::record("usb_tunnel", "CoreDeviceProxy connect timeout (>10s)");
        "Timeout membuka terowongan USB ke iPhone (>10 detik). Pastikan layar iPhone terbuka kuncinya, sudah mengetuk 'Percayai Komputer Ini' (Trust), dan layanan Apple Mobile Device Service (27015) aktif. / USB tunnel connection timed out (>10s). Ensure your iPhone is unlocked, 'Trust This Computer' is accepted, and Apple Mobile Device Service (27015) is running.".to_string()
    })?
    .map_err(|e| {
        let es = format!("{e}");
        crate::diag_log::record("usb_tunnel", &format!("CoreDeviceProxy error: {es}"));
        let low = es.to_ascii_lowercase();
        if low.contains("pairing") || low.contains("trust") || low.contains("password") || low.contains("locked") {
            format!("iPhone belum dipercaya (Trust) atau layar masih terkunci: {es} / iPhone is locked or not trusted yet: {es}")
        } else if low.contains("27015") || low.contains("connection refused") || low.contains("timed out") {
            format!("Layanan Apple USB (port 27015) tidak merespons ({es}). Klik 'Perbaiki Driver' atau tutup 3uTools/iTunes bila sedang berjalan. / Apple USB Service (27015) did not respond ({es}).")
        } else {
            format!("CoreDeviceProxy gagal: {es}")
        }
    })?;
    let info = proxy.tunnel_info().clone();
    let src_ip: Ipv6Addr = info
        .client_address
        .parse()
        .map_err(|_| format!("invalid client IPv6: {}", info.client_address))?;
    let dst_ip: Ipv6Addr = info
        .server_address
        .parse()
        .map_err(|_| format!("invalid server IPv6: {}", info.server_address))?;
    let mut adapter = proxy
        .create_software_tunnel()
        .map_err(|e| format!("CDTunnel adapter failed: {e}"))?;
    let raw_tunnel = adapter
        .take_tunnel_stream()
        .ok_or_else(|| "CDTunnel stream missing".to_string())?;

    run_physical_tunnel_display_async(
        raw_tunnel,
        src_ip,
        dst_ip,
        info.server_rsd_port,
        None,
        session_gen,
        stop,
        None,
        false,
        Some(provider),
        gate_only,
    )
    .await
}

async fn run_physical_wifi_lockdown_proxy(
    udid: &str,
    ip: &str,
) -> Result<(Box<dyn idevice::ReadWrite>, Ipv6Addr, Ipv6Addr, u16, Option<usize>), String> {
    let addr: SocketAddr = format!("{ip}:62078")
        .parse()
        .map_err(|e| format!("Alamat IP Wi-Fi tidak valid ({ip}): {e}"))?;
    let stream = tokio::time::timeout(
        Duration::from_millis(1500),
        tokio::net::TcpStream::connect(addr),
    )
    .await
    .map_err(|_| format!("Timeout lockdown Wi-Fi ke {ip}:62078"))?
    .map_err(|e| format!("Gagal konek lockdown Wi-Fi {ip}:62078: {e}"))?;
    let _ = stream.set_nodelay(true);
    crate::wifi::configure_low_latency_tokio_tcp(&stream);

    let mut lockdown = LockdownClient::new(Idevice::new(Box::new(stream), "iphone-link"));
    let pairing_opt = PairingFile::load_from_lockdown_dir(udid);
    if let Some(ref pf) = pairing_opt {
        lockdown
            .start_session(pf)
            .await
            .map_err(|e| format!("Lockdown TLS session Wi-Fi gagal: {e}"))?;
    }
    let (svc_port, ssl) = lockdown
        .start_service("com.apple.internal.devicecompute.CoreDeviceProxy")
        .await
        .map_err(|e| format!("StartService CoreDeviceProxy Wi-Fi ditolak: {e}"))?;

    let svc_addr: SocketAddr = format!("{ip}:{svc_port}")
        .parse()
        .map_err(|e| format!("Invalid service addr: {e}"))?;
    let svc_tcp = tokio::time::timeout(
        Duration::from_secs(3),
        tokio::net::TcpStream::connect(svc_addr),
    )
    .await
    .map_err(|_| format!("Timeout konek CoreDeviceProxy Wi-Fi {svc_addr}"))?
    .map_err(|e| format!("Gagal konek CoreDeviceProxy Wi-Fi {svc_addr}: {e}"))?;
    let _ = svc_tcp.set_nodelay(true);
    crate::wifi::configure_low_latency_tokio_tcp(&svc_tcp);
    #[cfg(windows)]
    let raw_sock = {
        use std::os::windows::io::AsRawSocket;
        Some(svc_tcp.as_raw_socket() as usize)
    };
    #[cfg(not(windows))]
    let raw_sock: Option<usize> = None;

    let svc_rw: Box<dyn idevice::ReadWrite> = if ssl {
        let pf = pairing_opt.ok_or_else(|| {
            "PairingFile Lockdown untuk iPhone tidak ditemukan. Hubungkan USB sekali lalu ketuk Trust.".to_string()
        })?;
        idevice::tls::upgrade_client_tls(Box::new(svc_tcp), &pf)
            .await
            .map_err(|e| format!("TLS CoreDeviceProxy Wi-Fi gagal: {e}"))?
    } else {
        Box::new(svc_tcp)
    };

    let proxy = CoreDeviceProxy::new(Idevice::new(svc_rw, "iphone-link"))
        .await
        .map_err(|e| format!("CoreDeviceProxy handshake Wi-Fi gagal: {e}"))?;
    let info = proxy.tunnel_info().clone();
    let src_ip: Ipv6Addr = info
        .client_address
        .parse()
        .map_err(|_| format!("invalid client IPv6: {}", info.client_address))?;
    let dst_ip: Ipv6Addr = info
        .server_address
        .parse()
        .map_err(|_| format!("invalid server IPv6: {}", info.server_address))?;
    let mut adapter = proxy
        .create_software_tunnel()
        .map_err(|e| format!("CDTunnel adapter Wi-Fi failed: {e}"))?;
    let raw_tunnel = adapter
        .take_tunnel_stream()
        .ok_or_else(|| "CDTunnel stream Wi-Fi missing".to_string())?;
    Ok((raw_tunnel, src_ip, dst_ip, info.server_rsd_port, raw_sock))
}

async fn run_physical_wifi_display_async(
    udid: String,
    ip: String,
    session_gen: u64,
    stop: Arc<AtomicBool>,
) -> Result<(), String> {
    run_physical_wifi_display_async_ex(udid, ip, session_gen, stop, false).await
}

/// Formats a localized RemotePairing Wi-Fi error message without empty parentheses (Tahap 293).
pub fn format_wifi_remote_pairing_error(ip: &str, detail: Option<&str>, lang_en: bool) -> String {
    let trimmed_ip = ip.trim();
    if trimmed_ip.is_empty() {
        return if lang_en {
            "Wi-Fi is not set up for this iPhone yet (Wi-Fi IP is empty). Connect via USB cable once and click 'Set up Wi-Fi'.".to_string()
        } else {
            "Wi-Fi belum diatur untuk iPhone ini (IP Wi-Fi kosong). Hubungkan kabel USB sekali lalu klik 'Atur Wi-Fi'.".to_string()
        };
    }
    let d_part = detail
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| format!(" ({s})"))
        .unwrap_or_default();
    if lang_en {
        format!(
            "Wi-Fi RemotePairing connection to {trimmed_ip} failed{d_part}. Make sure your iPhone is on the same Wi-Fi network and 'Set up Wi-Fi' was completed once via USB."
        )
    } else {
        format!(
            "Koneksi RemotePairing Wi-Fi ke {trimmed_ip} gagal{d_part}. Pastikan iPhone berada di Wi-Fi yang sama dan sudah pernah klik 'Atur Wi-Fi' sekali via USB."
        )
    }
}

async fn run_physical_wifi_display_async_ex(
    udid: String,
    ip: String,
    session_gen: u64,
    stop: Arc<AtomicBool>,
    gate_only: bool,
) -> Result<(), String> {
    if !crate::wifi::has_remote_record(&udid) && crate::wifi::all_records().is_empty() {
        crate::diag_log::record("wifi_err", "missing_remote_plist");
        return Err(format_wifi_remote_pairing_error(
            &ip,
            Some("record RemotePairing (remote_<udid>.plist) belum tersedia"),
            false,
        ));
    }
    let stage_target = if ip.trim().is_empty() {
        "mDNS LAN".to_string()
    } else {
        ip.clone()
    };
    if !gate_only {
        crate::mirror::update_stage_status(
            session_gen,
            2,
            "Koneksi aman",
            &format!("Membuka terowongan RemotePairing TLS-PSK Wi-Fi ke {stage_target}..."),
        );
    }

    // Path A (Primary - Tahap 240): Real Apple RemotePairing over Wi-Fi (_remotepairing._tcp.local.
    // + Pair-Verify X25519/Ed25519/ChaCha20-Poly1305 + createListener + TLS 1.2 PSK 0x00A8 + CDTunnel)
    let path_a_err = match crate::rp_wifi_tls::connect_real_apple_rp_wifi_tunnel(&udid, &ip).await {
        Ok(raw_rp) => {
            if !gate_only {
                crate::mirror::update_stage_status(
                    session_gen,
                    3,
                    "Siapkan perangkat",
                    &format!(
                        "Terowongan Wi-Fi RemotePairing TLS-PSK aktif ({}:{}) - memulai DisplayService...",
                        raw_rp.resolved_ip, raw_rp.resolved_port
                    ),
                );
            }
            match run_physical_tunnel_display_async(
                raw_rp.stream,
                raw_rp.client_ip,
                raw_rp.server_ip,
                raw_rp.rsd_port,
                raw_rp.raw_data_socket,
                session_gen,
                stop.clone(),
                Some(raw_rp.control),
                true,
                None,
                gate_only,
            )
            .await
            {
                Ok(()) => return Ok(()),
                Err(e) => e,
            }
        }
        Err(e) => e,
    };

    if stop.load(Ordering::Relaxed) || crate::mirror::current_live_session_gen() != session_gen {
        return Ok(());
    }

    // Path B (Fallback): Direct Wireless Lockdown CoreDeviceProxy (<ip>:62078)
    let path_b_err = if !ip.trim().is_empty() {
        crate::diag_log::record("wifi_lockdown_fallback", "trying port 62078 CoreDeviceProxy");
        match run_physical_wifi_lockdown_proxy(&udid, &ip).await {
            Ok((raw_tunnel, src_ip, dst_ip, rsd_port, raw_sock)) => {
                crate::diag_log::record("wifi_lockdown_fallback", &format!("connected rsd_port={rsd_port}"));
                match run_physical_tunnel_display_async(
                    raw_tunnel,
                    src_ip,
                    dst_ip,
                    rsd_port,
                    raw_sock,
                    session_gen,
                    stop.clone(),
                    None,
                    true,
                    None,
                    gate_only,
                )
                .await
                {
                    Ok(()) => return Ok(()),
                    Err(e) => e,
                }
            }
            Err(e) => {
                crate::diag_log::record("wifi_lockdown_fallback", &format!("failed: {e}"));
                e
            }
        }
    } else {
        crate::diag_log::record("wifi_lockdown_fallback", "skipped empty last_ip");
        "IP Wi-Fi belum terresolusi via mDNS".to_string()
    };

    let merged_err = format!("{path_a_err} | {path_b_err}");
    crate::diag_log::record("wifi_err", &format!("RemotePairing failed: {merged_err}"));
    Err(format_wifi_remote_pairing_error(&ip, Some(&merged_err), false))
}

async fn run_physical_tunnel_display_async(
    raw_tunnel: Box<dyn idevice::ReadWrite>,
    src_ip: Ipv6Addr,
    dst_ip: Ipv6Addr,
    rsd_port: u16,
    raw_data_socket: Option<usize>,
    session_gen: u64,
    stop: Arc<AtomicBool>,
    mut rp_control: Option<tokio::net::TcpStream>,
    is_wifi_transport: bool,
    usb_provider: Option<UsbmuxdProvider>,
    gate_only: bool,
) -> Result<(), String> {
    let (router, udp_rx) =
        CdTunnelRouter::new_with_raw_socket(raw_tunnel, src_ip, dst_ip, rsd_port, raw_data_socket);

    // Keep RSD RemoteXPC alive for the entire mirroring session.
    let mut rsd_xpc = XpcH2Session::open(&router, router.rsd_port).await?;
    let rsd_uuid_bytes: [u8; 16] = [
        0x44, 0x96, 0xDB, 0x8D, 0x47, 0x80, 0x3F, 0xC8, 0xAE, 0xB9, 0xF2, 0xA9, 0x31, 0x9E, 0xB8,
        0x6A,
    ];
    let rsd_handshake = XpcVal::Dict(vec![
        ("MessageType".into(), XpcVal::Str("Handshake".into())),
        ("MessagingProtocolVersion".into(), XpcVal::Uint64(7)),
        ("UUID".into(), XpcVal::Uuid(rsd_uuid_bytes)),
        (
            "Properties".into(),
            XpcVal::Dict(vec![
                (
                    "RemoteXPCVersionFlags".into(),
                    XpcVal::Uint64(0x0100_0000_0000_0006),
                ),
                ("SensitivePropertiesVisible".into(), XpcVal::Bool(true)),
            ]),
        ),
        ("Services".into(), XpcVal::Dict(Vec::new())),
    ]);
    let mut peer_info = rsd_xpc
        .rpc_with_timeout(&rsd_handshake, false, Duration::from_secs(8))
        .await?;
    let stream_diag_cat = if is_wifi_transport { "wifi_rsd" } else { "usb_tunnel" };
    crate::diag_log::record(stream_diag_cat, "RSD handshake completed");
    let mut rsd_product_version = String::new();
    if let Some(props) = peer_info.dict_get("Properties") {
        let rsd_udid = props
            .dict_get("UniqueDeviceID")
            .and_then(XpcVal::as_str)
            .unwrap_or("");
        let rsd_name = props
            .dict_get("DeviceName")
            .and_then(XpcVal::as_str)
            .or_else(|| props.dict_get("BonjourHostName").and_then(XpcVal::as_str));
        let rsd_ptype = props
            .dict_get("ProductType")
            .and_then(XpcVal::as_str)
            .unwrap_or("");
        if let Some(pv) = props.dict_get("ProductVersion").and_then(XpcVal::as_str) {
            rsd_product_version = pv.to_string();
        }
        let rsd_model = crate::models::resolve_apple_model(rsd_ptype);
        let rsd_bt = props
            .dict_get("BluetoothAddress")
            .and_then(XpcVal::as_str);
        if !rsd_udid.is_empty() || rsd_name.is_some() || rsd_bt.is_some() {
            set_active_session_device_identity(rsd_udid, rsd_name, rsd_model, rsd_bt);
            if !rsd_udid.is_empty() {
                crate::wifi::update_device_identity_cache(rsd_udid, rsd_name, rsd_model, rsd_bt);
            }
        }
    }
    let extract_ports = |info: &XpcVal| -> (u16, u16, u16) {
        let svc_port = |svc: &str| -> u16 {
            info.dict_get("Services")
                .and_then(|s| s.dict_get(svc))
                .and_then(|d| d.dict_get("Port"))
                .and_then(XpcVal::as_u64)
                .unwrap_or(0) as u16
        };
        (
            svc_port("com.apple.coredevice.displayservice"),
            svc_port("com.apple.coredevice.hid.indigo"),
            svc_port("com.apple.coredevice.hid.universalhidservice"),
        )
    };
    let (mut disp_port, mut indigo_port, mut uhid_port) = extract_ports(&peer_info);

    // Tahap 251 Pilar 4: Auto-mount real DDI (Cryptex1 iOS 27+ / Personalized iOS 17–26) +
    // RSD reconnect (up to 3x @ 400 ms) whenever fresh-reboot iPhone omits DisplayService.
    if disp_port == 0 {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        let mut rsd_hs = rsd_handshake_from_peer_info(&peer_info);
        let mut tunnel_provider = CdTunnelRsdProvider {
            router: router.clone(),
        };
        crate::link::ensure_ddi_with_rsd(
            &mut tunnel_provider,
            &mut rsd_hs,
            usb_provider.as_ref(),
            &rsd_product_version,
            |stage, pct, detail| {
                if !gate_only {
                    crate::mirror::update_stage_code(
                        session_gen,
                        stage.as_str(),
                        pct,
                        "Menyiapkan iPhone",
                        detail,
                    );
                }
            },
        )
        .await?;

        // Drop previous RSD connection and reconnect up to 3x @ 400 ms until DisplayService appears.
        drop(rsd_xpc);
        let mut refreshed_xpc: Option<XpcH2Session> = None;
        for attempt in 0..3u8 {
            if stop.load(Ordering::Relaxed) {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(400)).await;
            if !gate_only {
                crate::mirror::update_stage_code(
                    session_gen,
                    "reconnecting_rsd",
                    90u8.saturating_add(attempt * 3),
                    "Menyambung ulang layanan iPhone",
                    &format!(
                        "Menyambungkan ulang layanan tampilan iPhone ({}/3)...",
                        attempt + 1
                    ),
                );
            }
            if let Ok(Ok(mut next_xpc)) = tokio::time::timeout(
                Duration::from_secs(5),
                XpcH2Session::open(&router, router.rsd_port),
            )
            .await
            {
                if let Ok(next_info) = next_xpc
                    .rpc_with_timeout(&rsd_handshake, false, Duration::from_secs(5))
                    .await
                {
                    let (d_p, i_p, u_p) = extract_ports(&next_info);
                    if d_p != 0 {
                        disp_port = d_p;
                        indigo_port = i_p;
                        uhid_port = u_p;
                        peer_info = next_info;
                        refreshed_xpc = Some(next_xpc);
                        break;
                    }
                    refreshed_xpc = Some(next_xpc);
                }
            }
        }
        if let Some(xpc) = refreshed_xpc {
            rsd_xpc = xpc;
        } else {
            rsd_xpc = XpcH2Session::open(&router, router.rsd_port).await?;
        }
        let _ = &peer_info;
        if disp_port == 0 {
            if is_wifi_transport {
                return Err(crate::link::WIFI_DDI_POST_REBOOT_BILINGUAL.into());
            }
            return Err(
                "Developer Disk Image sudah terpasang, tetapi layanan DisplayService belum aktif. Buka kunci layar iPhone lalu coba lagi. / Developer Disk Image mounted, but DisplayService is not active yet. Unlock your iPhone and retry."
                    .into(),
            );
        }
    }

    if stop.load(Ordering::Relaxed) {
        return Ok(());
    }

    if !gate_only {
        crate::diag_log::record(stream_diag_cat, &format!("DisplayService starting on port {disp_port}"));
        crate::mirror::update_stage_status(
            session_gen,
            3,
            "Siapkan perangkat",
            "Memulai layanan layar DisplayService iPhone...",
        );
    }

    let mut disp_xpc = XpcH2Session::open(&router, disp_port).await?;
    let (_, dev_id_str) = new_random_uuid_bytes_and_str();
    let (_, inv_id_str) = new_random_uuid_bytes_and_str();
    let (_, call_uuid_str) = new_random_uuid_bytes_and_str();
    let (sess_uuid_bytes, _) = new_random_uuid_bytes_and_str();
    let (offer_bplist, offer_session_id) = build_video_offer_bplist(&call_uuid_str);

    let start_req = XpcVal::Dict(vec![
        (
            "CoreDevice.CoreDeviceDDIProtocolVersion".into(),
            XpcVal::Int64(2),
        ),
        (
            "CoreDevice.coreDeviceVersion".into(),
            XpcVal::Dict(vec![
                (
                    "components".into(),
                    XpcVal::Array(vec![XpcVal::Uint64(629), XpcVal::Uint64(3)]),
                ),
                ("originalComponentsCount".into(), XpcVal::Int64(2)),
                ("stringValue".into(), XpcVal::Str("629.3".into())),
            ]),
        ),
        ("CoreDevice.deviceIdentifier".into(), XpcVal::Str(dev_id_str.clone())),
        (
            "CoreDevice.featureIdentifier".into(),
            XpcVal::Str("com.apple.coredevice.feature.startmediastream".into()),
        ),
        ("CoreDevice.action".into(), XpcVal::Dict(Vec::new())),
        (
            "CoreDevice.actionIdentifier".into(),
            XpcVal::Str("com.apple.coredevice.action.mediastreamstart".into()),
        ),
        (
            "CoreDevice.input".into(),
            XpcVal::Dict(vec![
                ("clientSupportedFeatures".into(), XpcVal::Uint64(140)),
                ("direction".into(), XpcVal::Str("output".into())),
                ("negotiatorOffer".into(), XpcVal::Data(offer_bplist)),
                (
                    "options".into(),
                    XpcVal::Dict(vec![
                        (
                            "AVCMediaStreamNegotiatorAccessNetworkType".into(),
                            XpcVal::Dict(vec![("int".into(), XpcVal::Int64(1))]),
                        ),
                        (
                            "AVCMediaStreamNegotiatorTransportProtocolType".into(),
                            XpcVal::Dict(vec![("int".into(), XpcVal::Int64(2))]),
                        ),
                        (
                            "CoreDeviceVideoDisplayMode".into(),
                            XpcVal::Dict(vec![("string".into(), XpcVal::Str("DisplayByID".into()))]),
                        ),
                        (
                            "VideoStreamForDisplayID".into(),
                            XpcVal::Dict(vec![("int".into(), XpcVal::Int64(1))]),
                        ),
                        (
                            "avcMediaStreamOptionClientSessionID".into(),
                            XpcVal::Dict(vec![("uuid".into(), XpcVal::Uuid(sess_uuid_bytes))]),
                        ),
                    ]),
                ),
                ("receiverIP".into(), XpcVal::Str(router.src_str.clone())),
                ("receiverPort".into(), XpcVal::Uint64(u64::from(RECEIVER_VIDEO_PORT))),
                ("senderIP".into(), XpcVal::Str(router.dst_str.clone())),
                ("timeout".into(), XpcVal::Uint64(20)),
                ("type".into(), XpcVal::Str("video".into())),
            ]),
        ),
        ("CoreDevice.invocationIdentifier".into(), XpcVal::Str(inv_id_str)),
    ]);

    let ans = disp_xpc
        .rpc_with_timeout(&start_req, true, Duration::from_secs(12))
        .await?;
    crate::diag_log::record("usb_tunnel", "DisplayService startmediastream reply OK");
    if let Some(err_obj) = ans.dict_get("CoreDevice.error") {
        let code = err_obj.dict_get("code").and_then(XpcVal::as_u32_lossless);
        let msg = err_obj
            .dict_get("userInfo")
            .and_then(|u| u.dict_get("NSLocalizedDescription"))
            .and_then(XpcVal::as_str)
            .unwrap_or("DisplayService startmediastream ditolak perangkat");
        return Err(translate_coredevice_display_error(code, msg));
    }

    let (mut rtcp_port, ours_ssrc, mut theirs_ssrc) =
        extract_rtcp_from_xpc_answer_with_default(&ans, offer_session_id);

    let (hid_tx, hid_rx) = mpsc::channel::<PhysicalHidEnvelope>();
    if let Ok(mut g) = hid_tx_slot().lock() {
        *g = Some(hid_tx);
    }
    let hid_ready = indigo_port > 0 || uhid_port > 0;
    PHYSICAL_HID_ACTIVE.store(hid_ready, Ordering::SeqCst);
    if gate_only {
        crate::mirror::set_hybrid_hid_available(session_gen, hid_ready);
    }
    let _hid_guard = HidSlotGuard;
    if hid_ready {
        spawn_physical_hid_worker(
            router.clone(),
            indigo_port,
            uhid_port,
            session_gen,
            stop.clone(),
            hid_rx,
        );
    }

    if !gate_only {
        crate::mirror::update_stage_status(
            session_gen,
            4,
            "Mulai stream",
            &format!(
                "Menerima stream video H.265/HEVC (RTCP port={rtcp_port}, ours=0x{ours_ssrc:08x}, theirs=0x{theirs_ssrc:08x})..."
            ),
        );
    }

    let mut picker = Picker::new();
    let mut depack = HevcDepacketizer::new();
    let mut au = Vec::new();
    let mut codec_ready = false;
    let mut last_applied_sps: Option<Vec<u8>> = None;
    let mut sent_first_key = false;
    let mut fir_seq = 1u8;
    let mut highest_seq = 0u32;
    let mut last_sr_ntp_mid = 0u32;
    let mut last_sr_recv_at: Option<Instant> = None;
    let mut last_pkt_at = Instant::now();
    let mut last_any_tunnel_rx_at = Instant::now();
    let mut last_rtcp_at = Instant::now();
    let mut last_pli_at = Instant::now() - Duration::from_secs(1);
    let mut last_ctrl_drain_at = Instant::now();
    let mut soft_pli_sent_for_silence = false;
    let mut disp_xpc_opt = Some(disp_xpc);
    let mut rsd_xpc_opt = Some(rsd_xpc);
    let mut prev_pkt_seq: Option<u16> = None;
    let mut fu_in_progress = false;
    let mut au_in_progress = false;
    let mut last_gap_pli_at = Instant::now() - Duration::from_secs(5);
    let stream_started_at = Instant::now();
    let mut video_frames_count: u64 = 0;
    let mut rtcp_sr_count: u64 = 0;

    // Tahap 243 ROOT-CAUSE FIX (Xcode / Golden v4.0.0 Byte-Exact RTCP RR + Empty-CNAME SDES):
    // Apple AVConference enables `RTCPTimeoutEnabled=True, RTCPTimeoutInterval=20s` and negotiates
    // `allowRTCPFB=0`. Previously, appending unnegotiated Google REMB (`PT=206 FMT=15`) and/or a
    // mismatched fallback `ours_ssrc` (`0x41424344` when negative Int64 failed `as_u64()`) caused
    // iOS 27.2 AVConference to reject every RR and kill the video stream at EXACTLY t = 20.0 s!
    // Sending exact 44-byte `RR (PT=201, LSR/DLSR) + SDES (PT=202, empty CNAME "")` with lossless
    // `RemoteSSRC` satisfies AVConference's RTCP keepalive watchdog indefinitely.
    let send_rtcp_rr_xcode = |router_ref: &CdTunnelRouter,
                              rtcp_p: u16,
                              ours: u32,
                              theirs: u32,
                              ext_seq: u32,
                              sr_mid: u32,
                              sr_at: Option<Instant>| {
        if rtcp_p == 0 || theirs == 0 || ours == 0 {
            return;
        }
        let dlsr = sr_at
            .map(|t| (t.elapsed().as_secs_f64() * 65536.0).min(u32::MAX as f64) as u32)
            .unwrap_or(0);
        let block = ReportBlock {
            source_ssrc: theirs,
            fraction_lost: 0,
            cumulative_lost: 0,
            highest_seq: ext_seq,
            jitter: 0,
            lsr: sr_mid,
            dlsr,
        };
        let pkt = build_rtcp_rr_sdes_remb(ours, block, "", 0);
        router_ref.send_udp(RECEIVER_VIDEO_PORT, rtcp_p, &pkt);
        if sr_mid != 0 {
            RTCP_RR_WITH_LSR_SENT.fetch_add(1, Ordering::SeqCst);
        }
        RTCP_BW_FEEDBACK_SENT.fetch_add(1, Ordering::SeqCst);
    };

    while !stop.load(Ordering::Relaxed) && crate::mirror::current_live_session_gen() == session_gen {
        match udp_rx.recv_timeout(Duration::from_millis(80)) {
            Ok((sport, dport, data)) => {
                if dport != RECEIVER_VIDEO_PORT {
                    continue;
                }
                if rtcp_port == 0 && sport != 0 {
                    rtcp_port = sport;
                }
                if is_rtcp(&data) {
                    // Tahap 244 Pilar 2: Any incoming RTCP packet (especially RFC 3550 SR PT=200
                    // sent every ~1 s on static screens like Apple Notes) proves the tunnel is 100% ALIVE!
                    last_any_tunnel_rx_at = Instant::now();
                    // Parse incoming RFC 3550 Sender Report (PT=200) -> extract LSR and echo
                    // exact Xcode 44-byte RR (LSR + DLSR ≈ 0) + empty-CNAME SDES immediately!
                    if let Some((sr_ssrc, ntp_mid)) = parse_rtcp_sender_report(&data) {
                        last_sr_ntp_mid = ntp_mid;
                        last_sr_recv_at = Some(Instant::now());
                        if sport != 0 && rtcp_port == 0 {
                            rtcp_port = sport;
                        }
                        if theirs_ssrc == 0 && sr_ssrc != 0 {
                            theirs_ssrc = sr_ssrc;
                        }
                        rtcp_sr_count = rtcp_sr_count.saturating_add(1);
                        RTCP_SR_RECEIVED.fetch_add(1, Ordering::SeqCst);
                        let media = picker.main.unwrap_or(theirs_ssrc);
                        send_rtcp_rr_xcode(
                            &router,
                            rtcp_port,
                            ours_ssrc,
                            media,
                            highest_seq,
                            last_sr_ntp_mid,
                            last_sr_recv_at,
                        );
                        last_rtcp_at = Instant::now();
                    }
                    continue;
                }
                let Some(pkt) = RtpPacket::parse(&data) else {
                    continue;
                };
                last_pkt_at = Instant::now();
                last_any_tunnel_rx_at = Instant::now();
                // Tahap 255 Pilar 2: Absolute `gate_only` isolation! If any RTP video packet arrives
                // while running as a Hybrid HID gate, perform silent drain-only — NEVER request PLI,
                // NEVER update CodecHeader (would overwrite AirPlay `avc1` -> `hev1`!), and NEVER
                // push frames into `LIVE_BROADCAST`.
                if gate_only {
                    continue;
                }
                soft_pli_sent_for_silence = false;
                if theirs_ssrc == 0 {
                    theirs_ssrc = pkt.ssrc;
                }
                if picker.main == Some(pkt.ssrc) {
                    highest_seq = extend_seq(highest_seq, pkt.sequence_number);
                    if let Some(prev_s) = prev_pkt_seq {
                        let forward_gap = pkt.sequence_number.wrapping_sub(prev_s);
                        // Record non-PII RTP sequence gap; request PLI ONLY if an in-flight FU/AU
                        // assembly was actually interrupted or >=3 packets were lost (1.5s cooldown)
                        // so benign 2-step gaps never spike IDR keyframes & crush Wi-Fi ABR QP!
                        if forward_gap > 1 && forward_gap < 500 {
                            crate::mirror::record_hevc_rtp_seq_gap();
                            if (fu_in_progress || au_in_progress || forward_gap >= 3)
                                && last_gap_pli_at.elapsed() >= Duration::from_millis(1500)
                            {
                                fu_in_progress = false;
                                last_gap_pli_at = Instant::now();
                                crate::mirror::request_live_pli();
                            }
                        }
                    }
                    prev_pkt_seq = Some(pkt.sequence_number);
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
                    depack = HevcDepacketizer::new();
                    au.clear();
                    fu_in_progress = false;
                    au_in_progress = false;
                    theirs_ssrc = pkt.ssrc;
                }

                for (seq, ts, payload, marker) in packets {
                    if payload.len() >= 3 && ((payload[0] >> 1) & 0x3F) == 49 {
                        let fu_hdr = payload[2];
                        if (fu_hdr & 0x80) != 0 {
                            fu_in_progress = true;
                        } else if (fu_hdr & 0x40) != 0 {
                            fu_in_progress = false;
                        }
                    }
                    depack.push_rtp(seq, ts, &payload);
                    let chunk = depack.take_output();
                    if !chunk.is_empty() {
                        au.extend_from_slice(&chunk);
                    }
                    if marker {
                        fu_in_progress = false;
                        au_in_progress = false;
                    } else {
                        au_in_progress = true;
                    }
                    if marker && !au.is_empty() {
                        let complete_au = std::mem::take(&mut au);
                        let sps_changed = depack.sps.as_deref() != last_applied_sps.as_deref();
                        if !codec_ready || sps_changed {
                            if let (Some(vps), Some(sps), Some(pps)) =
                                (depack.vps.as_deref(), depack.sps.as_deref(), depack.pps.as_deref())
                            {
                                let codec_str = hevc_codec_string_from_sps(sps)
                                    .unwrap_or_else(|| "hev1.1.6.L150.B0".to_string());
                                let hvcc = build_hvcc_record(vps, sps, pps);
                                let (cw, ch, trim) = parse_hevc_sps_dimensions_and_trim(sps)
                                    .unwrap_or((1216, 2656, FrameTrim::zero()));
                                crate::mirror::update_live_codec_from_sps_with_trim(
                                    session_gen,
                                    &codec_str,
                                    hvcc.as_deref(),
                                    cw,
                                    ch,
                                    Some(trim),
                                );
                                last_applied_sps = Some(sps.to_vec());
                                codec_ready = true;
                            }
                        }

                        let has_key = split_annexb_nalus(&complete_au).into_iter().any(|n| {
                            !n.is_empty() && matches!((n[0] >> 1) & 0x3F, 16..=21)
                        });
                        if codec_ready && (sent_first_key || has_key) {
                            if !sent_first_key {
                                set_last_disconnect_reason(None);
                                // Tahap 249 Pilar 2: On Wi-Fi cold-start first IDR, immediately send 1x exact
                                // 44-byte Xcode RTCP RR+SDES (ZERO REMB/RCTL) so AVConference ramps cleanly.
                                if is_wifi_transport && rtcp_port != 0 {
                                    let media = picker.main.unwrap_or(theirs_ssrc);
                                    send_rtcp_rr_xcode(
                                        &router,
                                        rtcp_port,
                                        ours_ssrc,
                                        media,
                                        highest_seq,
                                        last_sr_ntp_mid,
                                        last_sr_recv_at,
                                    );
                                    last_rtcp_at = Instant::now();
                                }
                            }
                            sent_first_key = true;
                            video_frames_count = video_frames_count.saturating_add(1);
                            if !gate_only {
                                crate::mirror::global_on_physical_video_frame(complete_au, has_key);
                            }
                            if has_key {
                                crate::mirror::clear_live_pli_request();
                            }
                        }
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                crate::rp_wifi_tls::mark_wifi_session_ended();
                if stop.load(Ordering::Relaxed)
                    || crate::mirror::current_live_session_gen() != session_gen
                {
                    return Ok(());
                }
                let cause = router
                    .exit_reason()
                    .unwrap_or_else(|| "CDTunnel router disconnected unexpectedly".to_string());
                let disc_cat = if is_wifi_transport { "wifi_stream" } else { "usb_stream" };
                crate::diag_log::record(
                    disc_cat,
                    &format!(
                        "tunnel_disconnected cause={cause} frames={video_frames_count} rtcp={rtcp_sr_count} elapsed_ms={}",
                        stream_started_at.elapsed().as_millis()
                    ),
                );
                set_last_disconnect_reason(Some(cause.clone()));
                return Err(cause);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }

        // Tahap 243 Pilar 1 (1:1 ZenLink Parity): Passive drain-only every <= 80 ms on display_xpc,
        // rsd_xpc, and RPPairing control TCP to answer every iOS TCP ACK / HTTP/2 PING ACK /
        // WINDOW_UPDATE without sending unsolicited PINGs that upset iOS 27.2 DisplayService!
        if last_ctrl_drain_at.elapsed() >= Duration::from_millis(80) {
            last_ctrl_drain_at = Instant::now();
            if let Some(ref mut dx) = disp_xpc_opt {
                if dx.drain_incoming_h2_nonblocking().is_err() {
                    disp_xpc_opt = None;
                }
            }
            if let Some(ref mut rx_s) = rsd_xpc_opt {
                if rx_s.drain_incoming_h2_nonblocking().is_err() {
                    rsd_xpc_opt = None;
                }
            }
            if let Some(ref mut ctrl) = rp_control {
                let _ = crate::rp_wifi_tls::drain_rp_control_nonblocking(ctrl);
            }
        }

        // Tahap 243: Exact 44-byte Xcode RTCP RR (LSR/DLSR, 0% loss) + empty-CNAME SDES every 1000 ms
        // (matching `screen_stream.py` 1:1 — zero unnegotiated REMB/RCTL packets!).
        if rtcp_port > 0 && last_rtcp_at.elapsed() >= Duration::from_millis(1000) {
            last_rtcp_at = Instant::now();
            let media = picker.main.unwrap_or(theirs_ssrc);
            send_rtcp_rr_xcode(
                &router,
                rtcp_port,
                ours_ssrc,
                media,
                highest_seq,
                last_sr_ntp_mid,
                last_sr_recv_at,
            );
        }

        // Tahap 244 Pilar 2 — RTCP SR Liveness Guard for Static Screens (e.g. Apple Notes):
        // Separate RTP video silence (normal when iOS H.265 VFR pauses on a static screen while
        // RTCP SR keeps arriving every ~1 s) from true total tunnel silence (RTP + RTCP SR dead >= 12 s).
        let last_hid_ms = LAST_HID_TX_EPOCH_MS.load(Ordering::Relaxed);
        let since_last_hid = if last_hid_ms > 0 {
            Some(Duration::from_millis(
                now_epoch_ms_u64().saturating_sub(last_hid_ms),
            ))
        } else {
            None
        };
        let liveness = evaluate_stream_liveness_for_mode(
            last_pkt_at.elapsed(),
            last_any_tunnel_rx_at.elapsed(),
            since_last_hid,
            crate::hid::is_device_intentionally_locked(),
            sent_first_key,
            soft_pli_sent_for_silence,
            gate_only,
        );
        let need_soft_recovery_pli = liveness == StreamLivenessAction::SendSoftPli;

        // Tahap 241/244: Match ZenLink clean PLI policy — send PLI ONLY at startup (!sent_first_key),
        // genuine loss/rotation/wake request, or post-HID silent refresh (never unsolicited periodic PLI!).
        // In Hybrid `gate_only` mode (iOS 17.4–26.x), DisplayService carries 0 video frames so skip PLI.
        let urgent_cooldown = if sent_first_key {
            Duration::from_millis(1000)
        } else {
            Duration::from_millis(200)
        };
        let need_urgent = !gate_only
            && (((!sent_first_key || crate::mirror::is_live_pli_requested())
                && last_pli_at.elapsed() >= urgent_cooldown)
                || need_soft_recovery_pli);
        if rtcp_port > 0 && theirs_ssrc != 0 && need_urgent {
            if need_soft_recovery_pli {
                soft_pli_sent_for_silence = true;
            }
            last_pli_at = Instant::now();
            crate::mirror::clear_live_pli_request();
            crate::mirror::record_live_pli_sent();
            let media = picker.main.unwrap_or(theirs_ssrc);
            let dlsr = last_sr_recv_at
                .map(|t| (t.elapsed().as_secs_f64() * 65536.0).min(u32::MAX as f64) as u32)
                .unwrap_or(0);
            let block = ReportBlock {
                source_ssrc: media,
                fraction_lost: 0,
                cumulative_lost: 0,
                highest_seq,
                jitter: 0,
                lsr: last_sr_ntp_mid,
                dlsr,
            };
            let pli = build_keyframe_request(ours_ssrc, "", media, &[block], fir_seq);
            fir_seq = fir_seq.wrapping_add(1);
            router.send_udp(RECEIVER_VIDEO_PORT, rtcp_port, &pli);
        }

        if liveness == StreamLivenessAction::HardReconnect {
            crate::rp_wifi_tls::mark_wifi_session_ended();
            let reason = if let Some(cause) = router.exit_reason() {
                format!("Tunnel RX silence 12s ({cause})")
            } else {
                "Koneksi tunnel iPhone terhenti selama 12 detik (tidak ada RTP/RTCP).".to_string()
            };
            let disc_cat = if is_wifi_transport { "wifi_stream" } else { "usb_stream" };
            crate::diag_log::record(
                disc_cat,
                &format!(
                    "rx_silence_12s cause={reason} frames={video_frames_count} rtcp={rtcp_sr_count} elapsed_ms={}",
                    stream_started_at.elapsed().as_millis()
                ),
            );
            set_last_disconnect_reason(Some(reason.clone()));
            return Err(reason);
        }
    }

    // Tahap 250 Pilar 2: Gracefully stop CoreDevice DisplayService with bounded <= 120 ms best-effort
    // timeout (non-blocking) and explicitly drop XPC, RPPairing control TCP, and CDTunnel router so
    // `remoted`/`dtremotedisplayd` release the session immediately for fast back-to-back reconnects.
    if let Some(mut disp_xpc) = disp_xpc_opt.take() {
        let (_, stop_inv) = new_random_uuid_bytes_and_str();
        let stop_req = XpcVal::Dict(vec![
            (
                "CoreDevice.CoreDeviceDDIProtocolVersion".into(),
                XpcVal::Int64(2),
            ),
            (
                "CoreDevice.coreDeviceVersion".into(),
                XpcVal::Dict(vec![
                    (
                        "components".into(),
                        XpcVal::Array(vec![XpcVal::Uint64(629), XpcVal::Uint64(3)]),
                    ),
                    ("originalComponentsCount".into(), XpcVal::Int64(2)),
                    ("stringValue".into(), XpcVal::Str("629.3".into())),
                ]),
            ),
            ("CoreDevice.deviceIdentifier".into(), XpcVal::Str(dev_id_str)),
            (
                "CoreDevice.featureIdentifier".into(),
                XpcVal::Str("com.apple.coredevice.feature.stopmediastream".into()),
            ),
            ("CoreDevice.action".into(), XpcVal::Dict(Vec::new())),
            (
                "CoreDevice.actionIdentifier".into(),
                XpcVal::Str("com.apple.coredevice.action.mediastreamstop".into()),
            ),
            (
                "CoreDevice.input".into(),
                XpcVal::Dict(vec![("stopAll".into(), XpcVal::Bool(true))]),
            ),
            ("CoreDevice.invocationIdentifier".into(), XpcVal::Str(stop_inv)),
        ]);
        disp_xpc.conn.read_timeout = Duration::from_millis(120);
        let _ = tokio::time::timeout(Duration::from_millis(120), disp_xpc.rpc(&stop_req, true)).await;
        drop(disp_xpc);
    }
    drop(rsd_xpc_opt);
    drop(rp_control);
    drop(router);
    crate::rp_wifi_tls::mark_wifi_session_ended();
    Ok(())
}

/// Sleeps up to `total_ms` in 40 ms slices, returning `false` immediately (< 100 ms) if user stops mirroring.
fn sleep_interruptible(stop: &AtomicBool, session_gen: u64, total_ms: u64) -> bool {
    let start = Instant::now();
    let target = Duration::from_millis(total_ms);
    while start.elapsed() < target {
        if stop.load(Ordering::Relaxed)
            || crate::mirror::current_live_session_gen() != session_gen
        {
            return false;
        }
        let rem = target.saturating_sub(start.elapsed());
        std::thread::sleep(rem.min(Duration::from_millis(40)));
    }
    !stop.load(Ordering::Relaxed) && crate::mirror::current_live_session_gen() == session_gen
}

/// Preserves the true `attempt=0` disconnect cause when auto-retries fail after active frames,
/// ensuring later retry failures (e.g. `usbmuxd` device gone) never overwrite the root cause.
pub fn compose_preserved_stream_disconnect_error(
    initial_err: Option<&str>,
    last_err: &str,
    had_frames: bool,
) -> String {
    let clean_last = crate::diag_log::sanitize_ui_no_raw_udid(last_err.trim());
    if had_frames {
        if let Some(init) = initial_err.map(str::trim).filter(|s| !s.is_empty()) {
            let clean_init = crate::diag_log::sanitize_ui_no_raw_udid(init);
            if !clean_init.is_empty() && clean_init != clean_last {
                return format!("{clean_init} (Sambung ulang gagal: {clean_last})");
            }
            if !clean_init.is_empty() {
                return clean_init;
            }
        }
    }
    clean_last
}

pub fn spawn_physical_usb_display_worker(
    udid: String,
    session_gen: u64,
    stop: Arc<AtomicBool>,
) {
    let worker_guard = crate::mirror::PhysicalWorkerExitGuard::enter();
    let _ = std::thread::Builder::new()
        .name("iphone-physical-stream".into())
        .spawn(move || {
            let _worker_guard = worker_guard;
            let mut initial_err: Option<String> = None;
            let mut last_err = String::new();
            let mut had_frames_ever = false;
            for attempt in 0..=3u32 {
                if stop.load(Ordering::Relaxed)
                    || crate::mirror::current_live_session_gen() != session_gen
                {
                    return;
                }
                let res = runtime().block_on(run_physical_usb_display_async(
                    udid.clone(),
                    session_gen,
                    stop.clone(),
                ));
                if stop.load(Ordering::Relaxed)
                    || crate::mirror::current_live_session_gen() != session_gen
                {
                    return;
                }
                match res {
                    Ok(()) => return,
                    Err(err_msg) => {
                        let had_frames = crate::mirror::has_ever_received_frame_for_session(session_gen);
                        had_frames_ever |= had_frames;
                        if attempt == 0 && initial_err.is_none() {
                            initial_err = Some(err_msg.clone());
                            crate::diag_log::record(
                                "usb_stream",
                                &format!("attempt=0 error={err_msg} had_frames={had_frames}"),
                            );
                        } else {
                            crate::diag_log::record(
                                "usb_retry",
                                &format!("attempt={attempt} error={err_msg} had_frames={had_frames}"),
                            );
                        }
                        last_err = err_msg.clone();
                        let preserved_now = compose_preserved_stream_disconnect_error(
                            initial_err.as_deref(),
                            &last_err,
                            had_frames_ever,
                        );
                        set_last_disconnect_reason(Some(preserved_now));
                        let max_retries = if had_frames { 3 } else { 1 };
                        if attempt < max_retries {
                            if had_frames {
                                AUTO_RECONNECT_COUNT.fetch_add(1, Ordering::SeqCst);
                                crate::mirror::set_reconnecting_state(
                                    session_gen,
                                    true,
                                    attempt + 1,
                                    &err_msg,
                                );
                            }
                            if !sleep_interruptible(&stop, session_gen, 250) {
                                return;
                            }
                            crate::mirror::request_live_pli();
                            continue;
                        }
                        break;
                    }
                }
            }
            if !stop.load(Ordering::Relaxed)
                && crate::mirror::current_live_session_gen() == session_gen
            {
                let final_err = compose_preserved_stream_disconnect_error(
                    initial_err.as_deref(),
                    &last_err,
                    had_frames_ever,
                );
                set_last_disconnect_reason(Some(final_err.clone()));
                crate::mirror::global_on_physical_stream_error(session_gen, &final_err);
            }
        });
}

pub fn spawn_physical_wifi_display_worker(
    udid: String,
    ip: String,
    session_gen: u64,
    stop: Arc<AtomicBool>,
) {
    let worker_guard = crate::mirror::PhysicalWorkerExitGuard::enter();
    let _ = std::thread::Builder::new()
        .name("iphone-wifi-stream".into())
        .spawn(move || {
            let _worker_guard = worker_guard;
            let mut initial_err: Option<String> = None;
            let mut last_err = String::new();
            let mut had_frames_ever = false;
            for attempt in 0..=3u32 {
                if stop.load(Ordering::Relaxed)
                    || crate::mirror::current_live_session_gen() != session_gen
                {
                    return;
                }
                let res = runtime().block_on(run_physical_wifi_display_async(
                    udid.clone(),
                    ip.clone(),
                    session_gen,
                    stop.clone(),
                ));
                if stop.load(Ordering::Relaxed)
                    || crate::mirror::current_live_session_gen() != session_gen
                {
                    return;
                }
                match res {
                    Ok(()) => return,
                    Err(err_msg) => {
                        let had_frames = crate::mirror::has_ever_received_frame_for_session(session_gen);
                        had_frames_ever |= had_frames;
                        if attempt == 0 && initial_err.is_none() {
                            initial_err = Some(err_msg.clone());
                            crate::diag_log::record(
                                "wifi_stream",
                                &format!("attempt=0 error={err_msg} had_frames={had_frames}"),
                            );
                        } else {
                            crate::diag_log::record(
                                "wifi_retry",
                                &format!("attempt={attempt} error={err_msg} had_frames={had_frames}"),
                            );
                        }
                        last_err = err_msg.clone();
                        let preserved_now = compose_preserved_stream_disconnect_error(
                            initial_err.as_deref(),
                            &last_err,
                            had_frames_ever,
                        );
                        set_last_disconnect_reason(Some(preserved_now));
                        // Tahap 244 Pilar 4: Allow 1 automatic cold-start retry (matching USB!) when
                        // iOS `remoted` drops `rsd_xpc`/`DisplayService` with `early eof` on first
                        // wake before frame #1, and up to 3 transparent reconnects once streaming.
                        let max_retries = if had_frames { 3 } else { 1 };
                        if attempt < max_retries {
                            if had_frames {
                                AUTO_RECONNECT_COUNT.fetch_add(1, Ordering::SeqCst);
                                crate::mirror::set_reconnecting_state(
                                    session_gen,
                                    true,
                                    attempt + 1,
                                    &err_msg,
                                );
                            } else {
                                crate::mirror::update_stage_status(
                                    session_gen,
                                    2,
                                    "Menyambungkan Wi-Fi",
                                    "Mencoba ulang koneksi DisplayService Wi-Fi...",
                                );
                            }
                            if !sleep_interruptible(&stop, session_gen, 300) {
                                return;
                            }
                            crate::mirror::request_live_pli();
                            continue;
                        }
                        break;
                    }
                }
            }
            if !stop.load(Ordering::Relaxed)
                && crate::mirror::current_live_session_gen() == session_gen
            {
                let final_err = compose_preserved_stream_disconnect_error(
                    initial_err.as_deref(),
                    &last_err,
                    had_frames_ever,
                );
                set_last_disconnect_reason(Some(final_err.clone()));
                crate::mirror::global_on_physical_stream_error(session_gen, &final_err);
            }
        });
}

/// Tahap 293 (Opsi 2): Hybrid AirPlay+HID background `gate_only` worker is disabled for iOS < 27
/// so iOS 17-26 runs cleanly as pure View & Audio Only AirPlay without unverified USB HID background tasks.
pub fn spawn_physical_hybrid_hid_gate_worker(
    _udid: String,
    _wifi_ip: Option<String>,
    _transport: String,
    session_gen: u64,
    _stop: Arc<AtomicBool>,
) {
    crate::diag_log::record("airplay", "gate_only disabled for iOS < 27 (View & Audio Only)");
    crate::mirror::set_hybrid_hid_available(session_gen, false);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tahap238_remotexpc_hid_reports_and_xpc_packets() {
        // 1. Exact 58-byte touchscreen digitizer report (report ID 0x09, 0xC2 contact vs 0x02 release)
        let contact = build_touchscreen_report_58b(true, 16384, 49152, 0x1122_3344_5566);
        assert_eq!(contact.len(), 58);
        assert_eq!(&contact[0..4], &[0x09, 0x01, 0x05, 0xC2]);
        assert_eq!(&contact[4..6], &16384u16.to_le_bytes());
        assert_eq!(&contact[6..8], &49152u16.to_le_bytes());
        assert_eq!(&contact[40..44], &[0x02, 0x00, 0x00, 0x00]);
        assert_eq!(&contact[44..50], &[0x66, 0x55, 0x44, 0x33, 0x22, 0x11]);

        let release = build_touchscreen_report_58b(false, 16384, 49152, 0x01);
        assert_eq!(release.len(), 58);
        assert_eq!(release[3], 0x02);

        // 2. Exact 39-byte keyboard bitmap report + 68-byte HID descriptor
        assert_eq!(keyboard_descriptor_with_end_collection().len(), 68);
        let kb = build_keyboard_report_39b(&[0x04, 0xE1], 0x123456);
        assert_eq!(kb.len(), 39);
        assert_eq!(kb[0], 0x01);
        assert_eq!(kb[1], 1u8 << 4); // usage 0x04 -> byte 0, bit 4
        assert_eq!(kb[1 + (0xE1usize / 8)], 1u8 << (0xE1usize % 8));
        assert_eq!(&kb[37..39], &[0x00, 0x00]);

        // 3. Round-trip XPC dictionary encoding/decoding for Indigo button & UHID createService
        let btn = build_indigo_button_xpc(12, 0x0040, true);
        let wrapped = build_xpc_wrapper(XPC_FLAG_ALWAYS_SET | XPC_FLAG_DATA_PRESENT, 1, Some(&btn));
        let mut off = 32usize;
        let decoded = decode_xpc_object(&wrapped, &mut off).expect("decode indigo xpc");
        assert_eq!(decoded.dict_get("messageType").and_then(XpcVal::as_str), Some("IndigoButtonEvent"));

        let kbd_req = build_uhid_create_keyboard_xpc();
        let kbd_wrap = build_xpc_wrapper(XPC_FLAG_ALWAYS_SET | XPC_FLAG_DATA_PRESENT | XPC_FLAG_WANTING_REPLY, 3, Some(&kbd_req));
        let mut koff = 32usize;
        let kdec = decode_xpc_object(&kbd_wrap, &mut koff).expect("decode createService xpc");
        assert!(kdec
            .dict_get("payload")
            .and_then(|p| p.dict_get("createService"))
            .and_then(|c| c.dict_get("_0"))
            .and_then(|z| z.dict_get("_CoreDevice_codablePropertyStorage"))
            .is_some());
    }

    #[test]
    fn test_tahap238_full_cdtunnel_remotexpc_hid_worker_touch_button_keyboard_and_stealth() {
        let _hid_guard = crate::hid::HID_TEST_SERIAL_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        crate::hid::reset_hid_runtime_state();
        reset_physical_hid_counters_for_test();
        let _slot_guard = HidSlotGuard;

        #[derive(Debug)]
        struct MockTunnelDuplex {
            rx_buf: Arc<Mutex<VecDeque<u8>>>,
            tx_pkt_to_peer: mpsc::Sender<Vec<u8>>,
            closed: Arc<AtomicBool>,
        }

        impl tokio::io::AsyncRead for MockTunnelDuplex {
            fn poll_read(
                self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
                buf: &mut tokio::io::ReadBuf<'_>,
            ) -> Poll<std::io::Result<()>> {
                if self.closed.load(Ordering::Relaxed) {
                    return Poll::Ready(Ok(()));
                }
                let Ok(mut q) = self.rx_buf.lock() else {
                    return Poll::Pending;
                };
                if q.is_empty() {
                    return Poll::Pending;
                }
                let max_take = buf.unfilled_mut().len().min(q.len());
                let mut tmp = Vec::with_capacity(max_take);
                for _ in 0..max_take {
                    if let Some(b) = q.pop_front() {
                        tmp.push(b);
                    }
                }
                buf.put_slice(&tmp);
                Poll::Ready(Ok(()))
            }
        }

        impl tokio::io::AsyncWrite for MockTunnelDuplex {
            fn poll_write(
                self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
                buf: &[u8],
            ) -> Poll<std::io::Result<usize>> {
                let _ = self.tx_pkt_to_peer.send(buf.to_vec());
                Poll::Ready(Ok(buf.len()))
            }

            fn poll_flush(
                self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
            ) -> Poll<std::io::Result<()>> {
                Poll::Ready(Ok(()))
            }

            fn poll_shutdown(
                self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
            ) -> Poll<std::io::Result<()>> {
                self.closed.store(true, Ordering::SeqCst);
                Poll::Ready(Ok(()))
            }
        }

        let src_ip: Ipv6Addr = "fd80:1::1".parse().unwrap();
        let dst_ip: Ipv6Addr = "fd80:1::2".parse().unwrap();
        let src_bytes = src_ip.octets();
        let dst_bytes = dst_ip.octets();

        let peer_rx_queue = Arc::new(Mutex::new(VecDeque::<u8>::new()));
        let (tx_to_peer, rx_from_client) = mpsc::channel::<Vec<u8>>();
        let peer_closed = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));

        let indigo_events_rx = Arc::new(AtomicU64::new(0));
        let touch_reports_rx = Arc::new(AtomicU64::new(0));
        let kb_reports_rx = Arc::new(AtomicU64::new(0));
        let kb_service_created = Arc::new(AtomicBool::new(false));

        let peer_rx_q_th = peer_rx_queue.clone();
        let peer_stop_th = stop.clone();
        let ind_cnt_th = indigo_events_rx.clone();
        let touch_cnt_th = touch_reports_rx.clone();
        let kb_cnt_th = kb_reports_rx.clone();
        let kb_created_th = kb_service_created.clone();

        struct PeerConn {
            server_seq: u32,
            client_ack: u32,
            h2_preface_done: bool,
            h2_acc: Vec<u8>,
            xpc_acc: Vec<u8>,
        }

        let peer_thread = std::thread::spawn(move || {
            let mut conns: HashMap<(u16, u16), PeerConn> = HashMap::new();
            let mut peer_tx_acc: Vec<u8> = Vec::new();
            let push_to_client = |bytes: &[u8]| {
                if let Ok(mut q) = peer_rx_q_th.lock() {
                    q.extend(bytes);
                }
            };
            while !peer_stop_th.load(Ordering::Relaxed) {
                let chunk = match rx_from_client.recv_timeout(Duration::from_millis(20)) {
                    Ok(p) => p,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                };
                peer_tx_acc.extend_from_slice(&chunk);
                while peer_tx_acc.len() >= 40 {
                    if (peer_tx_acc[0] >> 4) != 6 {
                        peer_tx_acc.clear();
                        break;
                    }
                    let plen = u16::from_be_bytes([peer_tx_acc[4], peer_tx_acc[5]]) as usize;
                    let total_len = 40 + plen;
                    if peer_tx_acc.len() < total_len {
                        break;
                    }
                    let pkt = peer_tx_acc[..total_len].to_vec();
                    peer_tx_acc.drain(..total_len);
                    if pkt.len() < 60 || pkt[6] != 6 {
                        continue;
                    }
                    let tcp = &pkt[40..];
                let sport = u16::from_be_bytes([tcp[0], tcp[1]]);
                let dport = u16::from_be_bytes([tcp[2], tcp[3]]);
                let seq = u32::from_be_bytes([tcp[4], tcp[5], tcp[6], tcp[7]]);
                let doff = ((tcp[12] >> 4) as usize) * 4;
                let flags = tcp[13];
                if doff < 20 || doff > tcp.len() {
                    continue;
                }
                let payload = &tcp[doff..];
                if (flags & 0x02) != 0 {
                    let s_seq = 10_000u32;
                    let c_ack = seq.wrapping_add(1);
                    conns.insert(
                        (sport, dport),
                        PeerConn {
                            server_seq: s_seq.wrapping_add(1),
                            client_ack: c_ack,
                            h2_preface_done: false,
                            h2_acc: Vec::new(),
                            xpc_acc: Vec::new(),
                        },
                    );
                    let syn_ack = build_ipv6_tcp(
                        dst_bytes,
                        src_bytes,
                        dport,
                        sport,
                        s_seq,
                        c_ack,
                        0x12,
                        &[],
                        &[],
                    );
                    push_to_client(&syn_ack);
                    continue;
                }
                if !payload.is_empty() {
                    if let Some(c) = conns.get_mut(&(sport, dport)) {
                        c.client_ack = seq.wrapping_add(payload.len() as u32);
                        c.h2_acc.extend_from_slice(payload);
                        let ack_pkt = build_ipv6_tcp(
                            dst_bytes,
                            src_bytes,
                            dport,
                            sport,
                            c.server_seq,
                            c.client_ack,
                            0x10,
                            &[],
                            &[],
                        );
                        push_to_client(&ack_pkt);

                        if !c.h2_preface_done && c.h2_acc.starts_with(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n") {
                            c.h2_acc.drain(..24);
                            c.h2_preface_done = true;
                            let settings = h2_frame(4, 0, 0, &[]);
                            let s_pkt = build_ipv6_tcp(
                                dst_bytes,
                                src_bytes,
                                dport,
                                sport,
                                c.server_seq,
                                c.client_ack,
                                0x18,
                                &[],
                                &settings,
                            );
                            c.server_seq = c.server_seq.wrapping_add(settings.len() as u32);
                            push_to_client(&s_pkt);
                        }

                        while c.h2_preface_done && c.h2_acc.len() >= 9 {
                            let flen = ((c.h2_acc[0] as usize) << 16)
                                | ((c.h2_acc[1] as usize) << 8)
                                | (c.h2_acc[2] as usize);
                            if c.h2_acc.len() < 9 + flen {
                                break;
                            }
                            let ftype = c.h2_acc[3];
                            let fbody = c.h2_acc[9..9 + flen].to_vec();
                            c.h2_acc.drain(..9 + flen);
                            if ftype == 0 && !fbody.is_empty() {
                                c.xpc_acc.extend_from_slice(&fbody);
                                while c.xpc_acc.len() >= 24 {
                                    let size = u64::from_le_bytes([
                                        c.xpc_acc[8],
                                        c.xpc_acc[9],
                                        c.xpc_acc[10],
                                        c.xpc_acc[11],
                                        c.xpc_acc[12],
                                        c.xpc_acc[13],
                                        c.xpc_acc[14],
                                        c.xpc_acc[15],
                                    ]) as usize;
                                    if c.xpc_acc.len() < 24 + size {
                                        break;
                                    }
                                    let frame = c.xpc_acc[..24 + size].to_vec();
                                    c.xpc_acc.drain(..24 + size);
                                    let xflags = u32::from_le_bytes([frame[4], frame[5], frame[6], frame[7]]);
                                    let msg_id = u64::from_le_bytes([
                                        frame[16], frame[17], frame[18], frame[19],
                                        frame[20], frame[21], frame[22], frame[23],
                                    ]);
                                    if size >= 12 {
                                        let mut off = 32usize;
                                        if let Some(val) = decode_xpc_object(&frame, &mut off) {
                                            if val.dict_get("messageType").and_then(XpcVal::as_str) == Some("IndigoButtonEvent") {
                                                ind_cnt_th.fetch_add(1, Ordering::SeqCst);
                                            }
                                            if let Some(pay) = val.dict_get("payload") {
                                                if let Some(send_d) = pay.dict_get("send") {
                                                    if let Some(XpcVal::Data(rep)) = send_d.dict_get("_0") {
                                                        if rep.len() == 58 && rep[0] == 0x09 {
                                                            touch_cnt_th.fetch_add(1, Ordering::SeqCst);
                                                        } else if rep.len() == 39 && rep[0] == 0x01 {
                                                            kb_cnt_th.fetch_add(1, Ordering::SeqCst);
                                                        }
                                                    }
                                                }
                                                if pay.dict_get("createService").is_some() {
                                                    kb_created_th.store(true, Ordering::SeqCst);
                                                }
                                            }
                                        }
                                    }
                                    if (xflags & XPC_FLAG_WANTING_REPLY) != 0 {
                                        let reply_obj = XpcVal::Dict(vec![(
                                            "serviceID".into(),
                                            XpcVal::Uint64(UHID_KEYBOARD_DEFAULT_SERVICE_ID),
                                        )]);
                                        let rwrap = build_xpc_wrapper(
                                            XPC_FLAG_ALWAYS_SET | XPC_FLAG_DATA_PRESENT,
                                            msg_id,
                                            Some(&reply_obj),
                                        );
                                        let rframe = h2_frame(0, 0, 1, &rwrap);
                                        let rpkt = build_ipv6_tcp(
                                            dst_bytes,
                                            src_bytes,
                                            dport,
                                            sport,
                                            c.server_seq,
                                            c.client_ack,
                                            0x18,
                                            &[],
                                            &rframe,
                                        );
                                        c.server_seq = c.server_seq.wrapping_add(rframe.len() as u32);
                                        push_to_client(&rpkt);
                                    }
                                }
                            }
                        }
                    }
                }
                }
            }
        });

        let mock_stream = Box::new(MockTunnelDuplex {
            rx_buf: peer_rx_queue,
            tx_pkt_to_peer: tx_to_peer,
            closed: peer_closed,
        });
        let (router, _udp_rx) = CdTunnelRouter::new(mock_stream, src_ip, dst_ip, 51000);
        let session_gen = crate::mirror::current_live_session_gen();
        let (hid_tx, hid_rx) = mpsc::channel();
        if let Ok(mut g) = hid_tx_slot().lock() {
            *g = Some(hid_tx);
        }
        PHYSICAL_HID_ACTIVE.store(true, Ordering::SeqCst);
        spawn_physical_hid_worker(router, 51001, 51002, session_gen, stop.clone(), hid_rx);

        assert!(crate::hid::dispatch_physical_button_name("home", "press").unwrap());
        let (ok_c, _, _, sent_c, _) = crate::hid::dispatch_touch_payload_json(
            r#"{"type":"contact","x":32768,"y":42000}"#,
            0,
        );
        assert!(ok_c && sent_c);
        let (ok_r, _, _, sent_r, _) = crate::hid::dispatch_touch_payload_json(
            r#"{"type":"release","x":32768,"y":42000}"#,
            0,
        );
        assert!(ok_r && sent_r);
        assert!(crate::hid::dispatch_physical_swipe_direction("left").unwrap());
        assert!(crate::hid::dispatch_physical_swipe_direction("right").unwrap());
        assert!(crate::hid::dispatch_physical_key_usages(&[11]));
        assert!(crate::hid::dispatch_physical_key_usages(&[]));
        let sd_min = crate::hid::set_iphone_physical_brightness("min", 16, true);
        assert!(sd_min.ok && sd_min.stealth && sd_min.steps == 16);
        let sd_res = crate::hid::set_iphone_physical_brightness("restore", 4, true);
        assert!(sd_res.ok && !sd_res.stealth && sd_res.steps == 4);
        let lock_res = crate::hid::lock_iphone_screen_with_hold(true);
        assert!(lock_res.locked);
        let wake_res = crate::hid::wake_and_unlock_iphone("123456");
        assert!(wake_res.woken && wake_res.unlocked);

        let (pkts_tx, hid_err, hid_act) = physical_hid_telemetry();
        assert!(hid_act, "HID worker must be active");
        assert_eq!(hid_err, 0, "Expected 0 HID errors, got {hid_err}");
        assert!(
            pkts_tx >= 60,
            "Expected >= 60 RemoteXPC HID packets transmitted across buttons/touch/swipes/keys/stealth/lock/wake, got {pkts_tx}"
        );
        assert!(kb_service_created.load(Ordering::SeqCst), "createService keyboard must have been registered");
        assert!(indigo_events_rx.load(Ordering::SeqCst) >= 42, "indigo count={}", indigo_events_rx.load(Ordering::SeqCst));
        assert!(touch_reports_rx.load(Ordering::SeqCst) >= 20, "touch count={}", touch_reports_rx.load(Ordering::SeqCst));
        assert!(kb_reports_rx.load(Ordering::SeqCst) >= 14, "kb count={}", kb_reports_rx.load(Ordering::SeqCst));

        stop.store(true, Ordering::SeqCst);
        let _ = peer_thread.join();
    }

    #[test]
    fn test_tahap239_rtp_seq_wrap_and_h2_endurance_and_recent_hold() {
        // 1. Verify 32-bit extended RTP sequence wrap-around (65535 -> 0 -> 1000 => 66536)
        let mut h = extend_seq(0, 65000);
        assert_eq!(h, 65000);
        h = extend_seq(h, 65535);
        assert_eq!(h, 65535);
        h = extend_seq(h, 0);
        assert_eq!(h, 65536);
        h = extend_seq(h, 1000);
        assert_eq!(h, 66536);

        // 2. Verify sanitize_udid_label strips transport suffixes
        assert_eq!(
            sanitize_udid_label("00008101-0000000000000001 (Wi-Fi)"),
            "00008101-0000000000000001"
        );
        assert_eq!(
            sanitize_udid_label("00008101-0000000000000001 (USB)"),
            "00008101-0000000000000001"
        );

        // 3. Verify 1-click HomeCombo + Recent (240ms hold gesture) + 120 Touch Moves trigger HTTP/2 WINDOW_UPDATE
        let _hid_guard = crate::hid::HID_TEST_SERIAL_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        crate::hid::reset_hid_runtime_state();
        reset_physical_hid_counters_for_test();
        let _slot_guard = HidSlotGuard;

        #[derive(Debug)]
        struct EnduranceMockDuplex {
            rx_buf: Arc<Mutex<VecDeque<u8>>>,
            tx_to_peer: mpsc::Sender<Vec<u8>>,
            closed: Arc<AtomicBool>,
        }
        impl tokio::io::AsyncRead for EnduranceMockDuplex {
            fn poll_read(
                self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
                buf: &mut tokio::io::ReadBuf<'_>,
            ) -> Poll<std::io::Result<()>> {
                if self.closed.load(Ordering::Relaxed) {
                    return Poll::Ready(Ok(()));
                }
                let Ok(mut q) = self.rx_buf.lock() else {
                    return Poll::Pending;
                };
                if q.is_empty() {
                    return Poll::Pending;
                }
                let max_take = buf.unfilled_mut().len().min(q.len());
                let mut tmp = Vec::with_capacity(max_take);
                for _ in 0..max_take {
                    if let Some(b) = q.pop_front() {
                        tmp.push(b);
                    }
                }
                buf.put_slice(&tmp);
                Poll::Ready(Ok(()))
            }
        }
        impl tokio::io::AsyncWrite for EnduranceMockDuplex {
            fn poll_write(
                self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
                buf: &[u8],
            ) -> Poll<std::io::Result<usize>> {
                let _ = self.tx_to_peer.send(buf.to_vec());
                Poll::Ready(Ok(buf.len()))
            }
            fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
                Poll::Ready(Ok(()))
            }
            fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
                self.closed.store(true, Ordering::SeqCst);
                Poll::Ready(Ok(()))
            }
        }
        impl Unpin for EnduranceMockDuplex {}

        let (tx_to_peer, rx_from_app) = mpsc::channel::<Vec<u8>>();
        let peer_rx_queue = Arc::new(Mutex::new(VecDeque::<u8>::new()));
        let closed = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let touch_cnt = Arc::new(AtomicU64::new(0));
        let ping_acks_seen = Arc::new(AtomicU64::new(0));
        let win_updates_seen = Arc::new(AtomicU64::new(0));

        let peer_rx_c = peer_rx_queue.clone();
        let closed_c = closed.clone();
        let stop_c = stop.clone();
        let touch_cnt_c = touch_cnt.clone();
        let ping_acks_c = ping_acks_seen.clone();
        let win_updates_c = win_updates_seen.clone();

        let (local_ip, dst_ip) = (
            Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 1),
            Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 2),
        );
        let src_bytes = local_ip.octets();
        let dst_bytes = dst_ip.octets();

        let peer_thread = std::thread::spawn(move || {
            let mut tcp_rx_streams: std::collections::HashMap<(u16, u16), (u32, u32, bool, Vec<u8>)> =
                std::collections::HashMap::new();
            let mut peer_tx_acc: Vec<u8> = Vec::new();
            let mut sent_ping = false;
            while !stop_c.load(Ordering::Relaxed) && !closed_c.load(Ordering::Relaxed) {
                let Ok(chunk) = rx_from_app.recv_timeout(Duration::from_millis(20)) else {
                    continue;
                };
                peer_tx_acc.extend_from_slice(&chunk);
                while peer_tx_acc.len() >= 40 {
                    if (peer_tx_acc[0] >> 4) != 6 {
                        peer_tx_acc.clear();
                        break;
                    }
                    let plen = u16::from_be_bytes([peer_tx_acc[4], peer_tx_acc[5]]) as usize;
                    let total_len = 40 + plen;
                    if peer_tx_acc.len() < total_len {
                        break;
                    }
                    let pkt = peer_tx_acc[..total_len].to_vec();
                    peer_tx_acc.drain(..total_len);
                    if pkt.len() < 60 || (pkt[0] >> 4) != 6 || pkt[6] != 6 {
                        continue;
                    }
                    let tcp = &pkt[40..];
                let src_p = u16::from_be_bytes([tcp[0], tcp[1]]);
                let dst_p = u16::from_be_bytes([tcp[2], tcp[3]]);
                let seq = u32::from_be_bytes([tcp[4], tcp[5], tcp[6], tcp[7]]);
                let doff = ((tcp[12] >> 4) as usize) * 4;
                let flags = tcp[13];
                if doff < 20 || doff > tcp.len() {
                    continue;
                }
                let payload = &tcp[doff..];
                if (flags & 0x02) != 0 {
                    let s_seq = 5000u32;
                    let ack_v = seq.wrapping_add(1);
                    tcp_rx_streams.insert(
                        (src_p, dst_p),
                        (s_seq.wrapping_add(1), ack_v, false, Vec::new()),
                    );
                    let sa = build_ipv6_tcp(dst_bytes, src_bytes, dst_p, src_p, s_seq, ack_v, 0x12, &[], &[]);
                    peer_rx_c.lock().unwrap().extend(sa);
                    continue;
                }
                if !payload.is_empty() {
                    if let Some(ent) = tcp_rx_streams.get_mut(&(src_p, dst_p)) {
                        ent.1 = seq.wrapping_add(payload.len() as u32);
                        let ack = build_ipv6_tcp(dst_bytes, src_bytes, dst_p, src_p, ent.0, ent.1, 0x10, &[], &[]);
                        peer_rx_c.lock().unwrap().extend(ack);
                        ent.3.extend_from_slice(payload);
                        if !ent.2 && ent.3.starts_with(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n") {
                            ent.2 = true;
                            ent.3.drain(..24);
                            let sf = h2_frame(4, 0, 0, &[]);
                            let pf = h2_frame(6, 0, 0, b"PING239!");
                            let mut comb = Vec::new();
                            comb.extend_from_slice(&sf);
                            if !sent_ping && dst_p == 51002 {
                                sent_ping = true;
                                comb.extend_from_slice(&pf);
                            }
                            let resp = build_ipv6_tcp(dst_bytes, src_bytes, dst_p, src_p, ent.0, ent.1, 0x18, &[], &comb);
                            ent.0 = ent.0.wrapping_add(comb.len() as u32);
                            peer_rx_c.lock().unwrap().extend(resp);
                        }
                        if ent.2 {
                            while ent.3.len() >= 9 {
                                let flen = ((ent.3[0] as usize) << 16)
                                    | ((ent.3[1] as usize) << 8)
                                    | (ent.3[2] as usize);
                                if ent.3.len() < 9 + flen {
                                    break;
                                }
                                let ft = ent.3[3];
                                let ff = ent.3[4];
                                let body = ent.3[9..9 + flen].to_vec();
                                ent.3.drain(..9 + flen);
                                if ft == 6 && (ff & 1) == 1 {
                                    ping_acks_c.fetch_add(1, Ordering::SeqCst);
                                } else if ft == 8 {
                                    win_updates_c.fetch_add(1, Ordering::SeqCst);
                                } else if ft == 0 && body.len() >= 32 {
                                    let mut off = 32usize;
                                    if let Some(val) = decode_xpc_object(&body, &mut off) {
                                        if val.dict_get("payload").and_then(|p| p.dict_get("send")).is_some() {
                                            touch_cnt_c.fetch_add(1, Ordering::SeqCst);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                }
            }
        });

        let (router, _udp_rx) = CdTunnelRouter::new(
            Box::new(EnduranceMockDuplex {
                rx_buf: peer_rx_queue,
                tx_to_peer,
                closed,
            }),
            local_ip,
            dst_ip,
            51000,
        );
        let session_gen = crate::mirror::current_live_session_gen();
        let (hid_tx, hid_rx) = mpsc::channel();
        if let Ok(mut g) = hid_tx_slot().lock() {
            *g = Some(hid_tx);
        }
        PHYSICAL_HID_ACTIVE.store(true, Ordering::SeqCst);
        spawn_physical_hid_worker(router, 51001, 51002, session_gen, stop.clone(), hid_rx);

        assert!(crate::hid::dispatch_physical_button_name("home", "press").unwrap());
        assert!(crate::hid::dispatch_physical_button_name("recent", "press").unwrap());
        let before_moves = touch_cnt.load(Ordering::SeqCst);
        assert!(before_moves >= 27, "HomeCombo (8) + Recent (20) >= 27, got {before_moves}");

        for i in 0..105u16 {
            let _ = crate::hid::dispatch_physical_touch(true, 10000 + i * 100, 30000);
        }
        let _ = crate::hid::dispatch_physical_touch(false, 20400, 30000);

        let (h2_pings, h2_winups) = physical_h2_telemetry();
        let tcp_acks = physical_tcp_acks_sent();
        assert!(h2_pings >= 1, "Expected >=1 H2 PING ACK sent, got {h2_pings}");
        assert!(h2_winups >= 2, "Expected >=2 proactive H2 WINDOW_UPDATE sent, got {h2_winups}");
        assert!(tcp_acks >= 1, "Expected >=1 TCP ACK sent, got {tcp_acks}");
        assert!(win_updates_seen.load(Ordering::SeqCst) >= 2);
        assert!(ping_acks_seen.load(Ordering::SeqCst) >= 1);

        stop.store(true, Ordering::SeqCst);
        let _ = peer_thread.join();
    }

    #[test]
    fn test_tahap240_tcp_fin_detection_and_ack_counter() {
        let (tx_pkt, rx_pkt) = mpsc::channel::<Vec<u8>>();
        let (seg_tx, seg_rx) = mpsc::channel::<TcpSeg>();
        let routes: TcpRoutes = Arc::new(Mutex::new(HashMap::new()));
        let mut conn = CdTcpConn {
            src_ip: [0xFD, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2],
            dst_ip: [0xFD, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1],
            sport: 50000,
            dport: 51001,
            seq: 1001,
            snd_una: 1001,
            ack: 2001,
            peer_window_bytes: 65535,
            peer_wscale: 0,
            unacked: VecDeque::new(),
            dup_ack_seq: 1001,
            dup_ack_count: 0,
            rto: Duration::from_millis(250),
            zwp_last_at: None,
            zwp_probes: 0,
            tx_pkt,
            tx_flush_barrier: None,
            seg_rx,
            rx_buf: Vec::new(),
            ooo: BTreeMap::new(),
            routes,
            read_timeout: Duration::from_secs(1),
            read_waker: Arc::new(Mutex::new(None)),
            write_waker: Arc::new(Mutex::new(None)),
            peer_closed: false,
            router_stopped: Arc::new(AtomicBool::new(false)),
        };
        let before = physical_tcp_acks_sent();
        // 1. Peer sends data segment -> ACK sent and counter increments
        seg_tx
            .send(TcpSeg {
                seq: 2001,
                ack_num: 1001,
                window: 65535,
                flags: 0x18,
                win_scale: None,
                payload: vec![1, 2, 3, 4],
            })
            .unwrap();
        assert_eq!(conn.drain_nonblocking().unwrap(), true);
        assert!(physical_tcp_acks_sent() > before);
        assert!(rx_pkt.try_recv().is_ok());

        // 2. Peer sends TCP FIN (0x11) -> FIN-ACK sent and drain returns Err so zombie drops immediately!
        let before_fin = physical_tcp_acks_sent();
        seg_tx
            .send(TcpSeg {
                seq: 2005,
                ack_num: 1001,
                window: 65535,
                flags: 0x11,
                win_scale: None,
                payload: Vec::new(),
            })
            .unwrap();
        let fin_res = conn.drain_nonblocking();
        assert!(fin_res.is_err(), "TCP FIN must return Err to drop zombie XPC session");
        assert!(physical_tcp_acks_sent() > before_fin);
        assert!(rx_pkt.try_recv().is_ok());
    }

    #[test]
    fn test_tahap241_rtcp_sr_to_rr_lsr_dlsr_remb_and_uhid_consumer_descriptor() {
        // 1. Parse RFC 3550 Sender Report (PT=200) -> exact 32-bit mid-NTP LSR extraction
        let mut sr = Vec::new();
        sr.push(0x80); // V=2, RC=0
        sr.push(200); // PT=SR
        sr.extend_from_slice(&6u16.to_be_bytes()); // length = 6 words (28 bytes)
        sr.extend_from_slice(&0x1234_5678u32.to_be_bytes()); // sender SSRC
        sr.extend_from_slice(&0xAABB_CCDDu32.to_be_bytes()); // NTP MSW
        sr.extend_from_slice(&0xEEFF_1122u32.to_be_bytes()); // NTP LSW
        sr.extend_from_slice(&[0u8; 12]); // RTP ts, pkt count, octet count
        let (sr_ssrc, lsr) = parse_rtcp_sender_report(&sr).expect("parse SR");
        assert_eq!(sr_ssrc, 0x1234_5678);
        assert_eq!(lsr, 0xCCDD_EEFF);

        // 2. Build compound RR (LSR/DLSR) + SDES + REMB (50 Mbps USB & 30 Mbps Wi-Fi)
        let block = ReportBlock {
            source_ssrc: sr_ssrc,
            fraction_lost: 0,
            cumulative_lost: 0,
            highest_seq: 9999,
            jitter: 0,
            lsr,
            dlsr: 655, // ~10 ms in 1/65536s units
        };
        let pkt_usb = build_rtcp_rr_sdes_remb(0x4142_4344, block, "iphone-link", 50_000_000);
        assert_eq!(pkt_usb[1], 201); // RR first
        assert_eq!(&pkt_usb[24..28], &lsr.to_be_bytes());
        assert_eq!(&pkt_usb[28..32], &655u32.to_be_bytes());
        assert!(
            pkt_usb.windows(4).any(|w| w == b"REMB"),
            "Compound RTCP feedback must carry REMB high-bitrate block"
        );
        let pkt_wifi = build_rtcp_rr_sdes_remb(0x4142_4344, block, "iphone-link", 30_000_000);
        assert!(pkt_wifi.windows(4).any(|w| w == b"REMB"));

        // 3. UHID Consumer Control descriptor & 3-byte Lock/Brightness report roundtrip
        assert_eq!(CONSUMER_CONTROL_REPORT_DESCRIPTOR_25B.len(), 25);
        assert_eq!(build_consumer_report_3b(0x0030), vec![0x02, 0x30, 0x00]); // Power/Lock
        assert_eq!(build_consumer_report_3b(0x0070), vec![0x02, 0x70, 0x00]); // Brightness Down
        assert_eq!(build_consumer_report_3b(0x0000), vec![0x02, 0x00, 0x00]); // Release
        let xpc_cons = build_uhid_create_consumer_xpc();
        let wrapped = build_xpc_wrapper(
            XPC_FLAG_ALWAYS_SET | XPC_FLAG_DATA_PRESENT | XPC_FLAG_WANTING_REPLY,
            5,
            Some(&xpc_cons),
        );
        let mut off = 32usize;
        let dec = decode_xpc_object(&wrapped, &mut off).expect("decode consumer createService");
        assert!(dec
            .dict_get("payload")
            .and_then(|p| p.dict_get("createService"))
            .and_then(|c| c.dict_get("_0"))
            .and_then(|z| z.dict_get("PrimaryUsagePage"))
            .and_then(XpcVal::as_u64)
            == Some(0x0C));
    }

    #[test]
    fn test_tahap242_cdtunnel_atomic_fragmented_wouldblock_and_xpc_ping_drain() {
        let _hid_guard = crate::hid::HID_TEST_SERIAL_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        reset_physical_hid_counters_for_test();

        // Mock tunnel that fragments reads into 7-byte chunks and alternates WouldBlock on both read & write.
        #[derive(Debug)]
        struct FragmentedTunnel {
            rx_buf: Arc<Mutex<VecDeque<u8>>>,
            tx_raw: Arc<Mutex<Vec<u8>>>,
            closed: Arc<AtomicBool>,
            r_step: Arc<AtomicU64>,
            w_step: Arc<AtomicU64>,
        }
        impl tokio::io::AsyncRead for FragmentedTunnel {
            fn poll_read(
                self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
                buf: &mut tokio::io::ReadBuf<'_>,
            ) -> Poll<std::io::Result<()>> {
                if self.closed.load(Ordering::Relaxed) {
                    return Poll::Ready(Ok(()));
                }
                let s = self.r_step.fetch_add(1, Ordering::Relaxed);
                if s % 2 == 0 {
                    return Poll::Pending;
                }
                let Ok(mut q) = self.rx_buf.lock() else {
                    return Poll::Pending;
                };
                if q.is_empty() {
                    return Poll::Pending;
                }
                let take = 7usize.min(q.len()).min(buf.unfilled_mut().len());
                let mut tmp = Vec::with_capacity(take);
                for _ in 0..take {
                    if let Some(b) = q.pop_front() {
                        tmp.push(b);
                    }
                }
                buf.put_slice(&tmp);
                Poll::Ready(Ok(()))
            }
        }
        impl tokio::io::AsyncWrite for FragmentedTunnel {
            fn poll_write(
                self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
                buf: &[u8],
            ) -> Poll<std::io::Result<usize>> {
                let s = self.w_step.fetch_add(1, Ordering::Relaxed);
                if s % 2 == 0 {
                    return Poll::Pending;
                }
                let take = 9usize.min(buf.len());
                if let Ok(mut tx) = self.tx_raw.lock() {
                    tx.extend_from_slice(&buf[..take]);
                }
                Poll::Ready(Ok(take))
            }
            fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
                Poll::Ready(Ok(()))
            }
            fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
                self.closed.store(true, Ordering::SeqCst);
                Poll::Ready(Ok(()))
            }
        }

        let src_ip: Ipv6Addr = "fd80:242::1".parse().unwrap();
        let dst_ip: Ipv6Addr = "fd80:242::2".parse().unwrap();
        let rx_q = Arc::new(Mutex::new(VecDeque::<u8>::new()));
        let tx_raw = Arc::new(Mutex::new(Vec::<u8>::new()));
        let closed = Arc::new(AtomicBool::new(false));
        let mock = Box::new(FragmentedTunnel {
            rx_buf: rx_q.clone(),
            tx_raw: tx_raw.clone(),
            closed: closed.clone(),
            r_step: Arc::new(AtomicU64::new(0)),
            w_step: Arc::new(AtomicU64::new(0)),
        });

        let (router, udp_rx) = CdTunnelRouter::new(mock, src_ip, dst_ip, 52000);
        // Inject 3 back-to-back IPv6 UDP datagrams fragmented across 7-byte WouldBlock chunks
        for idx in 0..3u8 {
            let pay = vec![0xAA, 0xBB, idx, 0xCC, 0xDD];
            let pkt = build_ipv6_udp(dst_ip.octets(), src_ip.octets(), 50001, RECEIVER_VIDEO_PORT, &pay);
            rx_q.lock().unwrap().extend(pkt);
        }
        for idx in 0..3u8 {
            let (_sp, dp, pay) = udp_rx
                .recv_timeout(Duration::from_secs(2))
                .expect("fragmented IPv6 UDP packet must reassemble atomically");
            assert_eq!(dp, RECEIVER_VIDEO_PORT);
            assert_eq!(pay, vec![0xAA, 0xBB, idx, 0xCC, 0xDD]);
        }

        // Send outbound UDP packet across 9-byte fragmented WouldBlock writes and confirm full IPv6 integrity
        router.send_udp(RECEIVER_VIDEO_PORT, 50001, b"RTCP-TAHAP-242-ATOMIC-TX");
        let deadline = Instant::now() + Duration::from_secs(2);
        let expected_len = 40 + 8 + b"RTCP-TAHAP-242-ATOMIC-TX".len();
        while tx_raw.lock().unwrap().len() < expected_len && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        let sent = tx_raw.lock().unwrap().clone();
        assert_eq!(sent.len(), expected_len);
        assert_eq!(sent[0] >> 4, 6);
        assert!(sent.ends_with(b"RTCP-TAHAP-242-ATOMIC-TX"));

        // Verify interruptible sleep aborts in < 100 ms when stop flag trips
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_trigger = stop_flag.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            stop_trigger.store(true, Ordering::SeqCst);
        });
        let t0 = Instant::now();
        let kept_going = sleep_interruptible(&stop_flag, crate::mirror::current_live_session_gen(), 2000);
        assert!(!kept_going, "sleep_interruptible must abort on stop");
        assert!(t0.elapsed() < Duration::from_millis(120), "abort took {:?}", t0.elapsed());

        closed.store(true, Ordering::SeqCst);
    }

    #[test]
    fn test_tahap243_rtcp_negative_int64_ssrc_and_xcode_44b_rr_sdes() {
        // 1. Verify lossless u32 extraction even when Apple XPC encodes bit-31-set SSRCs as negative Int64
        let neg_remote_ssrc = (0x8D2E_8101u32 as i32) as i64; // -1926332159
        let neg_local_ssrc = (0xCAFE_BABEu32 as i32) as i64;
        let mock_ans = XpcVal::Dict(vec![(
            "CoreDevice.output".into(),
            XpcVal::Dict(vec![(
                "connection".into(),
                XpcVal::Dict(vec![(
                    "streamConfig".into(),
                    XpcVal::Dict(vec![
                        ("SourcePort".into(), XpcVal::Int64(54321)),
                        ("RemoteSSRC".into(), XpcVal::Int64(neg_remote_ssrc)),
                        ("LocalSSRC".into(), XpcVal::Int64(neg_local_ssrc)),
                    ]),
                )]),
            )]),
        )]);
        let (port, ours, theirs) = extract_rtcp_from_xpc_answer(&mock_ans);
        assert_eq!(port, 54321);
        assert_eq!(ours, 0x8D2E_8101);
        assert_eq!(theirs, 0xCAFE_BABE);

        // 2. Verify exact 44-byte Xcode compound RR + empty-CNAME SDES (zero REMB!)
        let block = ReportBlock {
            source_ssrc: theirs,
            fraction_lost: 0,
            cumulative_lost: 0,
            highest_seq: 1234,
            jitter: 0,
            lsr: 0x1122_3344,
            dlsr: 0x0000_0040,
        };
        let pkt = build_rtcp_rr_sdes_remb(ours, block, "", 0);
        assert_eq!(pkt.len(), 44, "Xcode RR+SDES compound must be exactly 44 bytes");
        assert_eq!(&pkt[0..4], &[0x81, 0xC9, 0x00, 0x07]);
        assert_eq!(&pkt[4..8], &ours.to_be_bytes());
        assert_eq!(&pkt[8..12], &theirs.to_be_bytes());
        // Trailing SDES must match Xcode literal `81 CA 00 02 <ours> 01 00 00 00`
        assert_eq!(&pkt[32..36], &[0x81, 0xCA, 0x00, 0x02]);
        assert_eq!(&pkt[36..40], &ours.to_be_bytes());
        assert_eq!(&pkt[40..44], &[0x01, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn test_tahap244_static_screen_rtcp_sr_liveness_guard() {
        // 1. Static screen (Apple Notes): 25s without RTP video frames, while RTCP SR keeps
        // arriving every ~0.8s -> MUST stay Healthy (ZERO false reconnect, ZERO unsolicited PLI!).
        assert_eq!(
            evaluate_stream_liveness(
                Duration::from_secs(25),
                Duration::from_millis(800),
                None,
                false,
                true,
                false,
            ),
            StreamLivenessAction::Healthy
        );

        // 2. Even 15s after last HID interaction with RTCP SR still fresh -> still Healthy!
        assert_eq!(
            evaluate_stream_liveness(
                Duration::from_secs(15),
                Duration::from_millis(950),
                Some(Duration::from_secs(15)),
                false,
                true,
                false,
            ),
            StreamLivenessAction::Healthy
        );

        // 3. Recent HID interaction (2s ago) with 1.8s RTP silence while tunnel is alive -> SendSoftPli once.
        assert_eq!(
            evaluate_stream_liveness(
                Duration::from_millis(1800),
                Duration::from_millis(600),
                Some(Duration::from_secs(2)),
                false,
                true,
                false,
            ),
            StreamLivenessAction::SendSoftPli
        );
        // Once soft PLI already sent for this pause, do not spam PLI:
        assert_eq!(
            evaluate_stream_liveness(
                Duration::from_millis(2500),
                Duration::from_millis(600),
                Some(Duration::from_secs(3)),
                false,
                true,
                true,
            ),
            StreamLivenessAction::Healthy
        );

        // 4. Genuine total tunnel blackout (BOTH RTP and RTCP SR silent >= 12s) -> HardReconnect!
        assert_eq!(
            evaluate_stream_liveness(
                Duration::from_secs(13),
                Duration::from_secs(12),
                None,
                false,
                true,
                false,
            ),
            StreamLivenessAction::HardReconnect
        );
    }

    fn build_test_hevc_sps_nal(
        coded_w: u32,
        coded_h: u32,
        conf_flag: bool,
        conf_offsets_lrtb: (u32, u32, u32, u32),
    ) -> Vec<u8> {
        let mut bits: Vec<u8> = Vec::with_capacity(256);
        let push_ue = |bits: &mut Vec<u8>, v: u32| {
            let val = v + 1;
            let total_bits = 32 - val.leading_zeros() as usize;
            for _ in 0..(total_bits - 1) {
                bits.push(0);
            }
            for shift in (0..total_bits).rev() {
                bits.push(((val >> shift) & 1) as u8);
            }
        };
        bits.extend_from_slice(&[0, 0, 0, 0]); // vps_id
        bits.extend_from_slice(&[0, 0, 0]); // max_sub_layers_minus1 = 0
        bits.push(1); // temporal_id_nesting_flag
        bits.extend(std::iter::repeat_n(0u8, 96)); // general_profile_tier_level (12 bytes)
        push_ue(&mut bits, 0); // sps_id
        push_ue(&mut bits, 1); // chroma_format_idc = 1 (4:2:0 -> sub_w=2, sub_h=2)
        push_ue(&mut bits, coded_w);
        push_ue(&mut bits, coded_h);
        if conf_flag {
            bits.push(1);
            push_ue(&mut bits, conf_offsets_lrtb.0); // left
            push_ue(&mut bits, conf_offsets_lrtb.1); // right
            push_ue(&mut bits, conf_offsets_lrtb.2); // top
            push_ue(&mut bits, conf_offsets_lrtb.3); // bottom
        } else {
            bits.push(0);
        }
        bits.push(1); // RBSP stop bit
        while bits.len() % 8 != 0 {
            bits.push(0);
        }
        let mut raw_rbsp = Vec::with_capacity(bits.len() / 8);
        for chunk in bits.chunks(8) {
            let mut b = 0u8;
            for &bit in chunk {
                b = (b << 1) | (bit & 1);
            }
            raw_rbsp.push(b);
        }
        // Escape 00 00 {00..03} -> 00 00 03 {00..03}
        let mut nal = vec![33u8 << 1, 0x01]; // HEVC SPS NAL header
        let mut zeros = 0u32;
        for byte in raw_rbsp {
            if zeros >= 2 && byte <= 3 {
                nal.push(3);
                zeros = 0;
            }
            nal.push(byte);
            zeros = if byte == 0 { zeros + 1 } else { 0 };
        }
        nal
    }

    #[test]
    fn test_tahap248_m6_multi_model_sps_trail_n_h264_random_session_9022_and_ios_route() {
        use idevice::core_device::{
            build_avcc_record, h264_codec_string_from_sps, H264Depacketizer,
        };
        use crate::link::{resolve_ios_mirror_route, IosMirrorRoute};

        // 1. Dynamic HEVC SPS conformance_window_flag parsing across 4 Apple models (0 hardcoded bias!)
        let sps_se = build_test_hevc_sps_nal(752, 1344, true, (0, 1, 0, 5)); // right=2, bottom=10 -> 750x1334
        let (w_se, h_se, t_se) = parse_hevc_sps_dimensions_and_trim(&sps_se).expect("SE SPS");
        assert_eq!((w_se, h_se), (752, 1344));
        assert_eq!(t_se, FrameTrim { top: 0, left: 0, bottom: 10, right: 2 });
        assert_eq!((w_se - t_se.right, h_se - t_se.bottom), (750, 1334));

        let sps_13 = build_test_hevc_sps_nal(1184, 2560, true, (0, 7, 0, 14)); // right=14, bottom=28 -> 1170x2532
        let (w_13, h_13, t_13) = parse_hevc_sps_dimensions_and_trim(&sps_13).expect("iPhone 13 SPS");
        assert_eq!((w_13, h_13), (1184, 2560));
        assert_eq!(t_13, FrameTrim { top: 0, left: 0, bottom: 28, right: 14 });
        assert_eq!((w_13 - t_13.right, h_13 - t_13.bottom), (1170, 2532));

        let sps_17p = build_test_hevc_sps_nal(1216, 2656, true, (0, 7, 0, 29)); // right=14, bottom=58 -> 1202x2598
        let (w_17, h_17, t_17) = parse_hevc_sps_dimensions_and_trim(&sps_17p).expect("iPhone 17 Pro SPS");
        assert_eq!((w_17, h_17), (1216, 2656));
        assert_eq!(t_17, FrameTrim { top: 0, left: 0, bottom: 58, right: 14 });
        assert_eq!((w_17 - t_17.right, h_17 - t_17.bottom), (1202, 2598));

        let sps_ipad = build_test_hevc_sps_nal(2048, 2732, false, (0, 0, 0, 0)); // conformance_window_flag=0 -> zero trim
        let (w_ip, h_ip, t_ip) = parse_hevc_sps_dimensions_and_trim(&sps_ipad).expect("iPad SPS");
        assert_eq!((w_ip, h_ip), (2048, 2732));
        assert_eq!(t_ip, FrameTrim::zero());

        // 2. HevcDepacketizer accepts TRAIL_N (NAL type 0) + updates trim dynamically per model
        let mut h_dep = HevcDepacketizer::new();
        assert!(h_dep.push(&sps_ipad, 100, false).is_none());
        assert_eq!(h_dep.dimensions(), Some((2048, 2732)));
        assert_eq!(h_dep.trim(), FrameTrim::zero());
        let trail_n_nal = vec![0x00, 0x01, 0xDE, 0xAD, 0xBE, 0xEF]; // type = (0x00 >> 1) & 0x3F = 0 (TRAIL_N)
        let frame_tn = h_dep.push(&trail_n_nal, 200, true).expect("TRAIL_N emitted");
        assert!(!frame_tn.is_keyframe);
        assert!(frame_tn.data.windows(trail_n_nal.len()).any(|w| w == trail_n_nal.as_slice()));

        // Verify Picker 1-SSRC vs rogue seq=12 still intact
        let mut picker = Picker::new();
        // Feed 2 contiguous packets from main SSRC so Picker locks onto it, while rogue seq=12 is rejected
        let _ = picker.push(0x1122_3344, (1, 90000, trail_n_nal.clone(), false));
        let locked = picker.push(0x1122_3344, (2, 90000, trail_n_nal.clone(), true));
        assert!(locked.is_some());
        assert!(picker.push(0x9988_7766, (12, 90000, vec![0x00, 0x01, 0x11], false)).is_none());

        // 3. H264Depacketizer RFC 6184: Single NAL + STAP-A (24) + FU-A (28) -> Annex-B + avcC + avc1.640028
        let mut avc = H264Depacketizer::new();
        let sps_264 = vec![0x67, 0x64, 0x00, 0x28, 0xAC, 0xD9]; // High profile 0x64, compat 0x00, level 4.0 (0x28)
        let pps_264 = vec![0x68, 0xEB, 0xEF, 0x20];
        let mut stap_a = vec![0x78]; // NRI=3, type=24 (STAP-A)
        for n in [&sps_264, &pps_264] {
            stap_a.extend_from_slice(&(n.len() as u16).to_be_bytes());
            stap_a.extend_from_slice(n);
        }
        assert!(avc.push(&stap_a, 1000, false).is_none());
        assert_eq!(avc.codec_string().as_deref(), Some("avc1.640028"));
        assert_eq!(h264_codec_string_from_sps(&sps_264).as_deref(), Some("avc1.640028"));
        let avcc = avc.build_avcc().expect("valid avcC record");
        assert_eq!(Some(avcc.clone()), build_avcc_record(&sps_264, &pps_264));
        assert_eq!(&avcc[0..6], &[0x01, 0x64, 0x00, 0x28, 0xFF, 0xE1]);

        // FU-A fragmented IDR (type 5)
        let fua_start = vec![0x7C, 0x80 | 5, 0x88, 0x84];
        let fua_end = vec![0x7C, 0x40 | 5, 0x21, 0x43];
        assert!(avc.push(&fua_start, 1000, false).is_none());
        let idr_frame = avc.push(&fua_end, 1000, true).expect("FU-A IDR frame");
        assert!(idr_frame.is_keyframe);
        assert!(idr_frame.data.starts_with(&[0, 0, 0, 1, 0x67, 0x64]));
        assert!(idr_frame
            .data
            .windows(9)
            .any(|w| w == [0, 0, 0, 1, 0x65, 0x88, 0x84, 0x21, 0x43]));

        // Single non-IDR slice (type 1)
        let non_idr = vec![0x41, 0x9A, 0x22, 0x33];
        let delta_frame = avc.push(&non_idr, 2000, true).expect("Single NAL non-IDR");
        assert!(!delta_frame.is_keyframe);
        assert_eq!(delta_frame.data, vec![0, 0, 0, 1, 0x41, 0x9A, 0x22, 0x33]);

        // 4. Random session_id per call + valid zlib blob roundtrip inside bplist00
        let (bp1, sid1) = build_video_offer_bplist("AAAAAAAA-BBBB-4CCC-8DDD-EEEEEEEEEEEE");
        let (bp2, sid2) = build_video_offer_bplist("11111111-2222-4333-8444-555555555555");
        assert_ne!(sid1, 0);
        assert_ne!(sid2, 0);
        assert_ne!(sid1, sid2, "consecutive calls must generate distinct random session_ids");
        assert!(bp1.starts_with(b"bplist00"));
        assert!(bp2.starts_with(b"bplist00"));
        // Verify stored-block zlib roundtrip & patched session_id varint
        let patched_pb = patch_offer_media_blob_session_id(sid1);
        let z = zlib_wrap_stored(&patched_pb);
        let roundtrip_pb = zlib_unwrap_stored(&z).expect("valid RFC 1950 zlib stored stream");
        assert_eq!(roundtrip_pb, patched_pb);
        let sid1_varint = encode_protobuf_varint_u32(sid1);
        assert_eq!(&roundtrip_pb[7..7 + sid1_varint.len()], sid1_varint.as_slice());

        // 5. CoreDevice 9022 friendly bilingual translation
        let err_id = translate_coredevice_display_error(Some(9022), "Failed with code 9022");
        assert!(err_id.contains("9022") && err_id.contains("FaceTime") && err_id.contains("Voice Memos"));
        let err_en = translate_coredevice_display_error_bilingual(Some(9022), "Error 9022", "en");
        assert!(err_en.contains("9022") && err_en.contains("FaceTime"));

        // 6. ProductVersion -> IosMirrorRoute mapping (Tahap 293 Opsi 2: iOS < 27 -> AirPlayStandalone)
        assert_eq!(resolve_ios_mirror_route("16.7.10"), IosMirrorRoute::AirPlayStandalone);
        assert_eq!(resolve_ios_mirror_route("17.2.1"), IosMirrorRoute::AirPlayStandalone);
        assert_eq!(resolve_ios_mirror_route("17.4.1"), IosMirrorRoute::AirPlayStandalone);
        assert_eq!(resolve_ios_mirror_route("18.5"), IosMirrorRoute::AirPlayStandalone);
        assert_eq!(resolve_ios_mirror_route("26.2"), IosMirrorRoute::AirPlayStandalone);
        assert_eq!(resolve_ios_mirror_route("27.2"), IosMirrorRoute::CoreDeviceNativeHevc);
        assert_eq!(resolve_ios_mirror_route(""), IosMirrorRoute::CoreDeviceNativeHevc);
    }

    #[test]
    fn test_tahap281_cdtunnel_wsapoll_wakeup_and_tx_coalescing() {
        use std::io::Write;
        use std::net::TcpListener;
        use std::os::windows::io::AsRawSocket;

        // 1. Verify WsaPollFd ABI layout & RAII 1 ms timer resolution guard
        assert_eq!(std::mem::size_of::<WsaPollFd>(), 16);
        let _timer_guard = TunnelTimerPeriod1msGuard::acquire_1ms();

        // 2. Real TCP loopback WSAPoll idle timeout & < 2 ms packet arrival wakeup
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
        let addr = listener.local_addr().unwrap();
        let cli = std::net::TcpStream::connect(addr).expect("connect");
        let (srv, _) = listener.accept().expect("accept");
        let _ = cli.set_nodelay(true);
        let _ = srv.set_nodelay(true);
        let _ = cli.set_nonblocking(true);
        let raw_fd = cli.as_raw_socket() as usize;

        // Idle timeout should return 0 without error
        let (idle_res, _) = wait_tunnel_socket_ready_or_timeout(Some(raw_fd), false, 2);
        assert_eq!(idle_res, 0, "idle WSAPoll must time out cleanly returning 0");

        // Trigger delayed loopback write and measure WSAPoll wakeup latency (3 trials -> best < 2.0 ms)
        let mut min_wakeup_us = f64::MAX;
        let mut drain_buf = [0u8; 256];
        for trial in 0..3u8 {
            let sent_at: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
            let sent_at_w = sent_at.clone();
            let mut srv_dup = srv.try_clone().expect("try_clone");
            let jh = std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(6));
                if let Ok(mut g) = sent_at_w.lock() {
                    *g = Some(Instant::now());
                }
                let _ = srv_dup.write_all(&[0x5Au8; 64]);
                let _ = srv_dup.flush();
            });
            let (res, revents) = wait_tunnel_socket_ready_or_timeout(Some(raw_fd), false, 500);
            let woken_at = Instant::now();
            let _ = jh.join();
            assert!(res > 0 && (revents & WSAPOLL_RDNORM) != 0, "trial {trial}: expected POLLRDNORM");
            let sent_ts = sent_at.lock().unwrap().expect("sent timestamp");
            let us = woken_at.saturating_duration_since(sent_ts).as_secs_f64() * 1_000_000.0;
            if us < min_wakeup_us {
                min_wakeup_us = us;
            }
            use std::io::Read;
            let _ = (&cli).read(&mut drain_buf);
        }
        eprintln!("TAHAP281_WSAPOLL_MIN_WAKEUP_US={:.1}", min_wakeup_us);
        assert!(
            min_wakeup_us < 2000.0,
            "Expected WSAPoll sub-2ms wakeup (<2000 us), got {:.1} us",
            min_wakeup_us
        );

        // 3. Opportunistic TX batch coalescing preserves exact IPv6 framing & >16KB packets
        let src = Ipv6Addr::LOCALHOST.octets();
        let dst = Ipv6Addr::LOCALHOST.octets();
        let mut pending: VecDeque<Vec<u8>> = VecDeque::new();
        for i in 0..4u16 {
            pending.push_back(build_ipv6_udp(src, dst, 50001, 50002 + i, &[i as u8; 128]));
        }
        let mut batch = Vec::with_capacity(16384);
        coalesce_tx_batch(&mut pending, &mut batch);
        assert!(pending.is_empty(), "4 small IPv6 packets must coalesce into 1 TX batch");
        let mut parsed_count = 0usize;
        let mut off = 0usize;
        while off < batch.len() {
            assert_eq!(batch[off] >> 4, 6);
            let plen = u16::from_be_bytes([batch[off + 4], batch[off + 5]]) as usize;
            off += 40 + plen;
            parsed_count += 1;
        }
        assert_eq!(off, batch.len());
        assert_eq!(parsed_count, 4);

        // 20 x 1200B packets cap cleanly <= 16384B and leave tail queued
        for _ in 0..20 {
            pending.push_back(build_ipv6_udp(src, dst, 50001, 50002, &[0xCC; 1152]));
        }
        coalesce_tx_batch(&mut pending, &mut batch);
        assert!(batch.len() <= 16384 && !pending.is_empty());

        // Single giant >16384B packet still drains when batch is empty (no livelock)
        pending.clear();
        pending.push_back(vec![0x60u8; 20000]);
        coalesce_tx_batch(&mut pending, &mut batch);
        assert_eq!(batch.len(), 20000);
        assert!(pending.is_empty());
    }

    #[test]
    fn test_tahap293_cdtcprw_async_read_wakes_tokio_task_on_delayed_segment() {
        use tokio::io::AsyncReadExt;

        let src = Ipv6Addr::LOCALHOST.octets();
        let dst = Ipv6Addr::LOCALHOST.octets();
        let (tx_pkt, _rx_pkt) = mpsc::channel();
        let (seg_tx, seg_rx) = mpsc::channel();
        let read_waker = Arc::new(Mutex::new(None));
        let waker_for_sender = Arc::clone(&read_waker);
        let tcp_routes: TcpRoutes = Arc::new(Mutex::new(HashMap::new()));
        let stopped = Arc::new(AtomicBool::new(false));

        let conn = CdTcpConn {
            src_ip: src,
            dst_ip: dst,
            sport: 51000,
            dport: 58783,
            seq: 1001,
            snd_una: 1001,
            ack: 2000,
            peer_window_bytes: 65535,
            peer_wscale: 0,
            unacked: VecDeque::new(),
            dup_ack_seq: 1001,
            dup_ack_count: 0,
            rto: Duration::from_millis(250),
            zwp_last_at: None,
            zwp_probes: 0,
            tx_pkt,
            tx_flush_barrier: None,
            seg_rx,
            rx_buf: Vec::new(),
            ooo: BTreeMap::new(),
            routes: tcp_routes,
            read_timeout: Duration::from_secs(5),
            read_waker,
            write_waker: Arc::new(Mutex::new(None)),
            peer_closed: false,
            router_stopped: stopped,
        };
        let mut rw = CdTcpRw::new(conn);

        // Inject delayed payload after 150ms so poll_read MUST hit Poll::Pending first, then wake!
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            let _ = seg_tx.send(TcpSeg {
                seq: 2000,
                ack_num: 1001,
                window: 65535,
                flags: 0x18,
                win_scale: None,
                payload: b"CRYPTEXD_OK".to_vec(),
            });
            wake_route_reader(&waker_for_sender);
        });

        let mut buf = [0u8; 11];
        let res = runtime().block_on(async move {
            tokio::time::timeout(Duration::from_secs(2), rw.read_exact(&mut buf))
                .await
                .expect("CdTcpRw::poll_read MUST wake within 2s (no Tokio deadlock!)")
                .map(|_| buf)
        });
        assert_eq!(res.unwrap(), *b"CRYPTEXD_OK");
    }

    #[test]
    fn test_tahap293_cdtcprw_async_read_wakes_on_eof_or_router_shutdown() {
        use tokio::io::AsyncReadExt;

        let src = Ipv6Addr::LOCALHOST.octets();
        let dst = Ipv6Addr::LOCALHOST.octets();
        let (tx_pkt, _rx_pkt) = mpsc::channel();
        let (seg_tx, seg_rx) = mpsc::channel();
        let read_waker = Arc::new(Mutex::new(None));
        let waker_for_shutdown = Arc::clone(&read_waker);
        let tcp_routes: TcpRoutes = Arc::new(Mutex::new(HashMap::new()));
        let stopped = Arc::new(AtomicBool::new(false));
        let stopped_bg = Arc::clone(&stopped);

        let conn = CdTcpConn {
            src_ip: src,
            dst_ip: dst,
            sport: 51001,
            dport: 58784,
            seq: 1001,
            snd_una: 1001,
            ack: 3000,
            peer_window_bytes: 65535,
            peer_wscale: 0,
            unacked: VecDeque::new(),
            dup_ack_seq: 1001,
            dup_ack_count: 0,
            rto: Duration::from_millis(250),
            zwp_last_at: None,
            zwp_probes: 0,
            tx_pkt,
            tx_flush_barrier: None,
            seg_rx,
            rx_buf: Vec::new(),
            ooo: BTreeMap::new(),
            routes: tcp_routes,
            read_timeout: Duration::from_secs(5),
            read_waker,
            write_waker: Arc::new(Mutex::new(None)),
            peer_closed: false,
            router_stopped: stopped,
        };
        let mut rw = CdTcpRw::new(conn);

        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(80));
            drop(seg_tx);
            stopped_bg.store(true, Ordering::Release);
            wake_route_reader(&waker_for_shutdown);
        });

        let mut buf = [0u8; 16];
        let n = runtime().block_on(async move {
            tokio::time::timeout(Duration::from_secs(2), rw.read(&mut buf))
                .await
                .expect("CdTcpRw::poll_read MUST wake cleanly on router shutdown")
                .expect("clean EOF Ok(0)")
        });
        assert_eq!(n, 0);
    }

    #[test]
    fn test_tahap293_wifi_error_never_prints_empty_parens_and_matches_locale() {
        let err_empty_id = format_wifi_remote_pairing_error("", Some("IP Wi-Fi kosong"), false);
        assert!(
            !err_empty_id.contains("()"),
            "Empty IP error must NEVER contain empty parens: {err_empty_id}"
        );
        assert!(
            err_empty_id.contains("'Atur Wi-Fi'") && !err_empty_id.contains("Set up Wi-Fi"),
            "Indonesian error must match UI button 'Atur Wi-Fi': {err_empty_id}"
        );

        let err_empty_en = format_wifi_remote_pairing_error("", Some("empty"), true);
        assert!(
            !err_empty_en.contains("()") && err_empty_en.contains("'Set up Wi-Fi'"),
            "English error mismatch: {err_empty_en}"
        );

        let err_ip_id = format_wifi_remote_pairing_error("198.51.100.42", Some("timeout"), false);
        assert!(err_ip_id.contains("198.51.100.42") && err_ip_id.contains("'Atur Wi-Fi'"));
    }

    fn build_test_ipv6_tcp_win(
        src: [u8; 16],
        dst: [u8; 16],
        sport: u16,
        dport: u16,
        seq: u32,
        ack: u32,
        flags: u8,
        window: u16,
        opts: &[u8],
        payload: &[u8],
    ) -> Vec<u8> {
        let mut pkt = build_ipv6_tcp(src, dst, sport, dport, seq, ack, flags, opts, payload);
        pkt[40 + 14..40 + 16].copy_from_slice(&window.to_be_bytes());
        pkt[40 + 16..40 + 18].copy_from_slice(&[0u8, 0u8]);
        let csum = ipv6_checksum(&src, &dst, 6, &pkt[40..]);
        pkt[40 + 16..40 + 18].copy_from_slice(&csum.to_be_bytes());
        pkt
    }

    #[test]
    fn test_tahap295_tcp_options_wscale_and_rto_probe() {
        use tokio::io::AsyncWriteExt;

        // 1. parse_tcp_win_scale edge cases
        assert_eq!(parse_tcp_win_scale(&[]), None);
        assert_eq!(parse_tcp_win_scale(&[0x02, 0x04, 0x05, 0x3C]), None);
        assert_eq!(
            parse_tcp_win_scale(&[0x02, 0x04, 0x05, 0x3C, 0x01, 0x03, 0x03, 0x06]),
            Some(6)
        );
        assert_eq!(parse_tcp_win_scale(&[0x03, 0x03, 20]), Some(14));
        assert_eq!(parse_tcp_win_scale(&[0x01, 0x00, 0x03, 0x03, 0x05]), None);

        // 2. Wrap-around ACK near u32::MAX, Zero-Window=0 backpressure, and ZWP/RTO retransmit
        let src = Ipv6Addr::LOCALHOST.octets();
        let dst = Ipv6Addr::LOCALHOST.octets();
        let (tx_pkt, rx_pkt) = mpsc::channel();
        let (seg_tx, seg_rx) = mpsc::channel();
        let read_waker = Arc::new(Mutex::new(None));
        let write_waker = Arc::new(Mutex::new(None));
        let waker_for_tx = Arc::clone(&write_waker);
        let tcp_routes: TcpRoutes = Arc::new(Mutex::new(HashMap::new()));
        let stopped = Arc::new(AtomicBool::new(false));

        let init_seq = u32::MAX - 500;
        let conn = CdTcpConn {
            src_ip: src,
            dst_ip: dst,
            sport: 52000,
            dport: 59000,
            seq: init_seq,
            snd_una: init_seq,
            ack: 2000,
            peer_window_bytes: 0, // Zero-Window starts closed!
            peer_wscale: 4,
            unacked: VecDeque::new(),
            dup_ack_seq: init_seq,
            dup_ack_count: 0,
            rto: Duration::from_millis(120),
            zwp_last_at: None,
            zwp_probes: 0,
            tx_pkt,
            tx_flush_barrier: None,
            seg_rx,
            rx_buf: Vec::new(),
            ooo: BTreeMap::new(),
            routes: tcp_routes,
            read_timeout: Duration::from_secs(5),
            read_waker,
            write_waker,
            peer_closed: false,
            router_stopped: stopped,
        };
        let mut rw = CdTcpRw::new(conn);

        // Reopen peer window after 120ms, then drop first data segment & ACK wrapped sequence
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(120));
            let _ = seg_tx.send(TcpSeg {
                seq: 2000,
                ack_num: init_seq,
                window: 256, // 256 << 4 = 4096 bytes
                flags: 0x10,
                win_scale: None,
                payload: Vec::new(),
            });
            wake_route_reader(&waker_for_tx);

            // Wait for data segment, ignore first send, accept RTO retransmit
            let mut got = 0usize;
            let expected_end = init_seq.wrapping_add(1400);
            while let Ok(pkt) = rx_pkt.recv_timeout(Duration::from_secs(3)) {
                let tcp = &pkt[40..];
                let doff = ((tcp[12] >> 4) as usize) * 4;
                let p_len = tcp.len().saturating_sub(doff);
                if p_len > 0 {
                    got += 1;
                    if got == 2 {
                        let _ = seg_tx.send(TcpSeg {
                            seq: 2000,
                            ack_num: expected_end,
                            window: 256,
                            flags: 0x10,
                            win_scale: None,
                            payload: Vec::new(),
                        });
                        wake_route_reader(&waker_for_tx);
                        break;
                    }
                }
            }
        });

        let data = vec![0xA5u8; 1400];
        runtime().block_on(async move {
            tokio::time::timeout(Duration::from_secs(4), async {
                rw.write_all(&data).await.expect("write across zero-window & wraparound");
                rw.flush().await.expect("flush waits until wrapped ACK arrives after RTO retransmit");
            })
            .await
            .expect("wraparound ACK + zero-window probe MUST complete in <4s");
        });
    }

    #[test]
    fn test_tahap295_cd_tcp_sliding_window_multi_mb_upload_and_packet_loss() {
        use tokio::io::AsyncWriteExt;

        #[derive(Debug)]
        struct TestTunnelDuplex {
            rx_buf: Arc<Mutex<VecDeque<u8>>>,
            tx_pkt_to_peer: mpsc::Sender<Vec<u8>>,
            closed: Arc<AtomicBool>,
        }
        impl tokio::io::AsyncRead for TestTunnelDuplex {
            fn poll_read(
                self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
                buf: &mut tokio::io::ReadBuf<'_>,
            ) -> Poll<std::io::Result<()>> {
                if self.closed.load(Ordering::Relaxed) {
                    return Poll::Ready(Ok(()));
                }
                let Ok(mut q) = self.rx_buf.lock() else {
                    return Poll::Pending;
                };
                if q.is_empty() {
                    return Poll::Pending;
                }
                let max_take = buf.unfilled_mut().len().min(q.len());
                let mut tmp = Vec::with_capacity(max_take);
                for _ in 0..max_take {
                    if let Some(b) = q.pop_front() {
                        tmp.push(b);
                    }
                }
                buf.put_slice(&tmp);
                Poll::Ready(Ok(()))
            }
        }
        impl tokio::io::AsyncWrite for TestTunnelDuplex {
            fn poll_write(
                self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
                buf: &[u8],
            ) -> Poll<std::io::Result<usize>> {
                let _ = self.tx_pkt_to_peer.send(buf.to_vec());
                Poll::Ready(Ok(buf.len()))
            }
            fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
                Poll::Ready(Ok(()))
            }
            fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
                self.closed.store(true, Ordering::SeqCst);
                Poll::Ready(Ok(()))
            }
        }

        fn run_upload_scenario(
            payload_len: usize,
            syn_ack_wscale: Option<u8>,
            ack_raw_window: u16,
            drop_every: u64,
            trigger_zero_window: bool,
        ) {
            let src_ip: Ipv6Addr = "fd80:295::1".parse().unwrap();
            let dst_ip: Ipv6Addr = "fd80:295::2".parse().unwrap();
            let src_bytes = src_ip.octets();
            let dst_bytes = dst_ip.octets();

            let peer_rx_queue = Arc::new(Mutex::new(VecDeque::<u8>::new()));
            let (tx_to_peer, rx_from_client) = mpsc::channel::<Vec<u8>>();
            let closed = Arc::new(AtomicBool::new(false));
            let stop = Arc::new(AtomicBool::new(false));
            let stop_srv = Arc::clone(&stop);
            let rx_q_srv = Arc::clone(&peer_rx_queue);
            let got_data = Arc::new(Mutex::new(Vec::<u8>::with_capacity(payload_len)));
            let got_data_srv = Arc::clone(&got_data);

            let peer_th = std::thread::spawn(move || {
                let mut acc = Vec::new();
                let mut rcv_nxt: u32 = 0;
                let server_seq: u32 = 77_000;
                let mut ooo: BTreeMap<u32, Vec<u8>> = BTreeMap::new();
                let mut pkt_cnt: u64 = 0;
                let mut paused_once = false;
                let mut zero_win_until: Option<Instant> = None;
                let mut sport_connected = 0u16;
                let mut dport_connected = 0u16;

                let push = |pkt: &[u8]| {
                    if let Ok(mut q) = rx_q_srv.lock() {
                        q.extend(pkt);
                    }
                };

                while !stop_srv.load(Ordering::Relaxed) {
                    if let Some(until) = zero_win_until {
                        if Instant::now() >= until && dport_connected != 0 {
                            zero_win_until = None;
                            push(&build_test_ipv6_tcp_win(
                                dst_bytes,
                                src_bytes,
                                dport_connected,
                                sport_connected,
                                server_seq + 1,
                                rcv_nxt,
                                0x10,
                                ack_raw_window,
                                &[],
                                &[],
                            ));
                        }
                    }

                    let chunk = match rx_from_client.recv_timeout(Duration::from_millis(10)) {
                        Ok(c) => c,
                        Err(mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    };
                    acc.extend_from_slice(&chunk);
                    while acc.len() >= 40 {
                        let plen = u16::from_be_bytes([acc[4], acc[5]]) as usize;
                        if acc.len() < 40 + plen {
                            break;
                        }
                        let pkt = acc[..40 + plen].to_vec();
                        acc.drain(..40 + plen);
                        if pkt[6] != 6 || pkt.len() < 60 {
                            continue;
                        }
                        let tcp = &pkt[40..];
                        let sport = u16::from_be_bytes([tcp[0], tcp[1]]);
                        let dport = u16::from_be_bytes([tcp[2], tcp[3]]);
                        let seq = u32::from_be_bytes([tcp[4], tcp[5], tcp[6], tcp[7]]);
                        let doff = ((tcp[12] >> 4) as usize) * 4;
                        let flags = tcp[13];
                        let payload = &tcp[doff..];

                        if (flags & 0x02) != 0 {
                            sport_connected = sport;
                            dport_connected = dport;
                            // Client SYN must propose MSS + WScale=6
                            assert_eq!(parse_tcp_win_scale(&tcp[20..doff]), Some(6));
                            rcv_nxt = seq.wrapping_add(1);
                            let syn_opts: Vec<u8> = match syn_ack_wscale {
                                Some(ws) => vec![0x02, 0x04, 0x05, 0x3C, 0x01, 0x03, 0x03, ws],
                                None => vec![0x02, 0x04, 0x05, 0x3C],
                            };
                            push(&build_test_ipv6_tcp_win(
                                dst_bytes,
                                src_bytes,
                                dport,
                                sport,
                                server_seq,
                                rcv_nxt,
                                0x12,
                                65535,
                                &syn_opts,
                                &[],
                            ));
                            continue;
                        }

                        if payload.is_empty() {
                            if zero_win_until.is_some() {
                                push(&build_test_ipv6_tcp_win(
                                    dst_bytes,
                                    src_bytes,
                                    dport,
                                    sport,
                                    server_seq + 1,
                                    rcv_nxt,
                                    0x10,
                                    0,
                                    &[],
                                    &[],
                                ));
                            }
                            continue;
                        }

                        if zero_win_until.is_some() {
                            push(&build_test_ipv6_tcp_win(
                                dst_bytes,
                                src_bytes,
                                dport,
                                sport,
                                server_seq + 1,
                                rcv_nxt,
                                0x10,
                                0,
                                &[],
                                &[],
                            ));
                            continue;
                        }

                        pkt_cnt += 1;
                        if drop_every > 0 && (pkt_cnt % drop_every == 7) {
                            // Deliberate packet drop: send NO ACK so sender recovers via DupACK/RTO
                            continue;
                        }

                        let mut lock = got_data_srv.lock().unwrap();
                        if seq == rcv_nxt {
                            lock.extend_from_slice(payload);
                            rcv_nxt = rcv_nxt.wrapping_add(payload.len() as u32);
                            while let Some(next_p) = ooo.remove(&rcv_nxt) {
                                rcv_nxt = rcv_nxt.wrapping_add(next_p.len() as u32);
                                lock.extend_from_slice(&next_p);
                            }
                        } else if (seq.wrapping_sub(rcv_nxt) as i32) > 0 {
                            ooo.entry(seq).or_insert_with(|| payload.to_vec());
                        }
                        let cur_len = lock.len();
                        drop(lock);

                        let win_out = if trigger_zero_window && !paused_once && cur_len >= payload_len / 3 {
                            paused_once = true;
                            zero_win_until = Some(Instant::now() + Duration::from_millis(80));
                            0
                        } else {
                            ack_raw_window
                        };
                        push(&build_test_ipv6_tcp_win(
                            dst_bytes,
                            src_bytes,
                            dport,
                            sport,
                            server_seq + 1,
                            rcv_nxt,
                            0x10,
                            win_out,
                            &[],
                            &[],
                        ));
                    }
                }
            });

            let mock = Box::new(TestTunnelDuplex {
                rx_buf: peer_rx_queue,
                tx_pkt_to_peer: tx_to_peer,
                closed,
            });
            let (router, _udp_rx) = CdTunnelRouter::new(mock, src_ip, dst_ip, 58783);
            let payload: Vec<u8> = (0..payload_len)
                .map(|i| ((i.wrapping_mul(31).wrapping_add(7)) & 0xFF) as u8)
                .collect();

            let t0 = Instant::now();
            runtime().block_on(async {
                let conn = router.connect_tcp(59010).await.expect("SYN-ACK");
                let mut rw = CdTcpRw::new(conn);
                tokio::time::timeout(Duration::from_secs(10), async {
                    rw.write_all(&payload).await.expect("multi-MB write_all");
                    rw.flush().await.expect("flush awaits full peer ACK");
                })
                .await
                .expect("multi-MB CdTcpRw upload MUST complete cleanly without deadlock");
            });
            assert!(
                t0.elapsed() < Duration::from_secs(8),
                "Upload took too long: {:?}",
                t0.elapsed()
            );

            stop.store(true, Ordering::Release);
            drop(router);
            let _ = peer_th.join();

            let received = got_data.lock().unwrap();
            assert_eq!(received.len(), payload.len(), "byte count mismatch");
            assert_eq!(*received, payload, "payload corruption detected");
        }

        // Variant A: WScale=0 (65,535 B window), 5% deterministic drops + Zero-Window pause, 4.2 MB upload
        run_upload_scenario(4_200_000, None, 65535, 20, true);
        // Variant B: WScale=6 (4096 << 6 = 262,144 B window), lossless 1 MB upload
        run_upload_scenario(1_048_576, Some(6), 4096, 0, false);
    }

    #[test]
    fn test_tahap296_usb_disconnect_preserves_initial_root_cause_across_retry() {
        // 1. WScale=6 SYN option MUST be isolated to DDI bulk channel only (XpcH2Session stays standard MSS=1340)
        assert_eq!(
            build_syn_options_for_channel(false),
            &[0x02, 0x04, 0x05, 0x3C],
            "XpcH2Session SYN must omit Window Scale"
        );
        assert_eq!(
            build_syn_options_for_channel(true),
            &[0x02, 0x04, 0x05, 0x3C, 0x01, 0x03, 0x03, 0x06],
            "DDI CdTcpRw SYN must include WScale=6"
        );

        // 2. Active-stream attempt=0 root cause MUST survive retry failure (attempt=1..3) without raw UDIDs
        let preserved = compose_preserved_stream_disconnect_error(
            Some("CDTunnel read EOF (peer closed tunnel)"),
            "iPhone USB 00008101-0011223344556677 tidak ditemukan di usbmuxd.",
            true,
        );
        assert!(
            preserved.contains("CDTunnel read EOF (peer closed tunnel)"),
            "missing initial root cause in: {preserved}"
        );
        assert!(
            preserved.contains("Sambung ulang gagal:"),
            "missing retry context in: {preserved}"
        );
        assert!(
            !preserved.contains("00008101-0011223344556677"),
            "raw UDID must be stripped from preserved error: {preserved}"
        );

        // 3. Before first frame (`had_frames = false`), return sanitized latest error
        let pre_frame = compose_preserved_stream_disconnect_error(
            Some("early eof"),
            "iPhone USB 00008101-0011223344556677 tidak ditemukan di usbmuxd.",
            false,
        );
        assert_eq!(pre_frame, "iPhone USB tidak ditemukan di usbmuxd.");
    }
}
