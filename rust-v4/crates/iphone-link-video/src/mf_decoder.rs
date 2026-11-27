//! Windows Media Foundation H.265 hardware decoder (`MF_LOW_LATENCY = 1` + `D3D11_AWARE = 1`),
//! Annex-B access unit parser & RBSP SPS extractor, zero-allocation `120x260` NV12/BGRA probe
//! for 32-Scenario Smart Auto-Rotate, lossless trimmed snapshot readback, and inverse touch matrix.

use std::ffi::c_void;
use std::sync::Arc;

use crate::autorotate::{ProbeGrid120x260, GRID_HEIGHT, GRID_WIDTH};
use crate::d3d11_context::{
    ensure_com_mta_initialized, get_or_probe_d3d11_video_context, probe_mfplat_delay_load,
    ComPtr, D3d11Box, D3d11Nv12VramRing, D3d11Texture2dDesc, Guid, LiveD3d11Gpu,
    MftRegisterTypeInfo, Rect, SharedFrameRing, SharedFrameSlot, VideoColorInfo, VideoColorMatrix,
    D3D11_MAP_WRITE_DISCARD, IID_ID3D10_MULTITHREAD, IID_ID3D11_TEXTURE2D, MFSTARTUP_NOSOCKET,
    MFT_CATEGORY_VIDEO_DECODER, MFT_ENUM_FLAG_LOCALMFT, MFT_ENUM_FLAG_SORTANDFILTER,
    MFT_ENUM_FLAG_SYNCMFT, MF_MEDIA_TYPE_VIDEO, MF_VIDEO_FORMAT_H264, MF_VIDEO_FORMAT_HEVC,
};

pub const MF_VERSION: u32 = 0x0002_0070;

pub const HEVC_NAL_TRAIL_N: u8 = 0;
pub const HEVC_NAL_TRAIL_R: u8 = 1;
pub const HEVC_NAL_IDR_W_RADL: u8 = 19;
pub const HEVC_NAL_IDR_N_LP: u8 = 20;
pub const HEVC_NAL_CRA_NUT: u8 = 21;
pub const HEVC_NAL_VPS: u8 = 32;
pub const HEVC_NAL_SPS: u8 = 33;
pub const HEVC_NAL_PPS: u8 = 34;

#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryW(lpLibFileName: *const u16) -> *mut c_void;
    fn GetProcAddress(hModule: *mut c_void, lpProcName: *const u8) -> *const c_void;
    fn FreeLibrary(hLibModule: *mut c_void) -> i32;
}

type FnMFStartup = unsafe extern "system" fn(u32, u32) -> i32;
type FnMFShutdown = unsafe extern "system" fn() -> i32;

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Probe real Windows Media Foundation subsystem (mfplat.dll) startup and shutdown.
pub fn probe_media_foundation() -> Result<bool, String> {
    let status = probe_mfplat_delay_load(false);
    if !status.mfplat_loaded {
        return Err(status.diagnostic);
    }
    let mfplat_wide = to_wide("mfplat.dll");
    let h_mfplat = unsafe { LoadLibraryW(mfplat_wide.as_ptr()) };
    if h_mfplat.is_null() {
        return Err("Could not load mfplat.dll (Media Feature Pack missing)".to_string());
    }

    unsafe {
        let p_startup = GetProcAddress(h_mfplat, b"MFStartup\0".as_ptr());
        let p_shutdown = GetProcAddress(h_mfplat, b"MFShutdown\0".as_ptr());

        if p_startup.is_null() || p_shutdown.is_null() {
            FreeLibrary(h_mfplat);
            return Err("Failed to resolve MFStartup or MFShutdown".to_string());
        }

        let startup: FnMFStartup = std::mem::transmute(p_startup);
        let shutdown: FnMFShutdown = std::mem::transmute(p_shutdown);

        let hr = startup(MF_VERSION, 0);
        let ok = hr == 0;
        let _ = shutdown();
        FreeLibrary(h_mfplat);

        if ok {
            Ok(true)
        } else {
            Err(format!("MFStartup failed with HRESULT 0x{:08X}", hr))
        }
    }
}

/// Splits an Annex-B byte stream (`00 00 01` / `00 00 00 01`) into NAL unit slices without start codes.
pub fn split_annex_b_nalus(data: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0usize;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut out = Vec::with_capacity(starts.len());
    for (n, &s) in starts.iter().enumerate() {
        let mut e = starts.get(n + 1).map_or(data.len(), |&next| next - 3);
        while e > s && n + 1 < starts.len() && data[e - 1] == 0 {
            e -= 1;
        }
        if e > s {
            out.push(&data[s..e]);
        }
    }
    out
}

/// Returns the 6-bit H.265/HEVC NALU type (`(nal[0] >> 1) & 0x3F`).
#[inline]
pub fn hevc_nal_unit_type(nal: &[u8]) -> Option<u8> {
    nal.first().map(|b| (b >> 1) & 0x3F)
}

/// Exp-Golomb RBSP bit reader stripping emulation prevention `0x03` bytes after `0x00 0x00`.
struct RbspBits {
    data: Vec<u8>,
    pos: usize,
}

impl RbspBits {
    fn new(nal: &[u8]) -> Self {
        let mut data = Vec::with_capacity(nal.len());
        let mut zeros = 0u32;
        for &b in nal {
            if zeros >= 2 && b == 3 {
                zeros = 0;
                continue;
            }
            zeros = if b == 0 { zeros + 1 } else { 0 };
            data.push(b);
        }
        Self { data, pos: 0 }
    }

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

    fn skip(&mut self, n: usize) {
        self.pos += n;
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
            Some(((1u32 << zeros) - 1) + self.u(zeros)?)
        }
    }
}

/// Extracts `(coded_width, coded_height, trim_top_left_bottom_right)` from an Annex-B HEVC SPS NALU (`33`).
/// Maps Apple iPhone 17 Pro (`1216 x 2656`) cleanly to `(top: 0, left: 0, bottom: 58, right: 14)`.
pub fn parse_hevc_sps_dimensions(annex_b: &[u8]) -> Option<(u32, u32, (u32, u32, u32, u32))> {
    let sps = split_annex_b_nalus(annex_b)
        .into_iter()
        .find(|n| n.len() > 2 && hevc_nal_unit_type(n) == Some(HEVC_NAL_SPS))?;

    let mut b = RbspBits::new(&sps[2..]);
    b.skip(4); // sps_video_parameter_set_id
    let sub_layers = b.u(3)? as usize;
    b.skip(1); // sps_temporal_id_nesting_flag
    b.skip(96); // general_profile_tier_level (12 bytes)
    let mut sub_layer_flags = Vec::with_capacity(sub_layers);
    for _ in 0..sub_layers {
        sub_layer_flags.push((b.bit()?, b.bit()?));
    }
    if sub_layers > 0 {
        b.skip(2 * (8 - sub_layers));
    }
    for (prof_present, level_present) in sub_layer_flags {
        b.skip(88 * prof_present as usize + 8 * level_present as usize);
    }
    let _sps_id = b.ue()?;
    let chroma = b.ue()?;
    if chroma == 3 {
        b.skip(1);
    }
    let w = b.ue()?;
    let h = b.ue()?;
    if !(64..=8192).contains(&w) || !(64..=8192).contains(&h) {
        return None;
    }

    let mut trim = (0u32, 0u32, 0u32, 0u32);
    if b.bit().unwrap_or(0) == 1 {
        let (sx, sy) = match chroma {
            1 => (2, 2),
            2 => (2, 1),
            _ => (1, 1),
        };
        let l = b.ue().unwrap_or(0) * sx;
        let r = b.ue().unwrap_or(0) * sx;
        let t = b.ue().unwrap_or(0) * sy;
        let bot = b.ue().unwrap_or(0) * sy;
        if l + r < w && t + bot < h {
            trim = (t, l, bot, r);
        }
    }
    Some((w, h, trim))
}

fn read_h26x_vui_color_info(b: &mut RbspBits) -> Option<VideoColorInfo> {
    if b.bit()? != 1 {
        return None; // vui_parameters_present_flag == 0
    }
    if b.bit()? == 1 {
        let aspect_idc = b.u(8)?;
        if aspect_idc == 255 {
            b.skip(32); // sar_width(16) + sar_height(16)
        }
    }
    if b.bit()? == 1 {
        let _overscan_appropriate = b.bit()?;
    }
    if b.bit()? != 1 {
        return None; // video_signal_type_present_flag == 0
    }
    let _video_format = b.u(3)?;
    let full_range = b.bit()? == 1;
    let mut matrix = VideoColorMatrix::Bt709;
    if b.bit()? == 1 {
        let _colour_primaries = b.u(8)?;
        let _transfer_characteristics = b.u(8)?;
        let matrix_coeffs = b.u(8)?;
        matrix = match matrix_coeffs {
            1 => VideoColorMatrix::Bt709,
            5 | 6 => VideoColorMatrix::Bt601,
            9 | 10 => VideoColorMatrix::Bt2020,
            _ => VideoColorMatrix::Bt709,
        };
    }
    Some(VideoColorInfo { matrix, full_range })
}

fn skip_hevc_st_ref_pic_sets(b: &mut RbspBits, num_sets: u32) -> Option<()> {
    let count = usize::try_from(num_sets).ok()?.min(64);
    let mut num_delta_pocs = [0u32; 65];
    for idx in 0..count {
        let inter_rps = if idx != 0 { b.bit()? == 1 } else { false };
        if inter_rps {
            let delta_idx_minus1 = if idx == count { b.ue()? } else { 0 };
            let ref_idx = idx.checked_sub(delta_idx_minus1 as usize + 1)?;
            let _delta_rps_sign = b.bit()?;
            let _abs_delta_rps_minus1 = b.ue()?;
            let ref_ndp = num_delta_pocs.get(ref_idx).copied().unwrap_or(0) as usize;
            let mut next_ndp = 0u32;
            for _ in 0..=ref_ndp {
                let used = b.bit()?;
                let use_delta = if used == 0 { b.bit()? } else { 1 };
                if used == 1 || use_delta == 1 {
                    next_ndp += 1;
                }
            }
            num_delta_pocs[idx] = next_ndp;
        } else {
            let num_neg = b.ue()?;
            let num_pos = b.ue()?;
            if num_neg > 64 || num_pos > 64 {
                return None;
            }
            for _ in 0..num_neg {
                let _delta = b.ue()?;
                let _used = b.bit()?;
            }
            for _ in 0..num_pos {
                let _delta = b.ue()?;
                let _used = b.bit()?;
            }
            num_delta_pocs[idx] = num_neg + num_pos;
        }
    }
    Some(())
}

/// Extracts `VideoColorInfo` (`BT.709`/`BT.601`/`BT.2020`, Full vs Limited range) from HEVC SPS VUI.
pub fn parse_hevc_sps_vui_color(annex_b: &[u8]) -> Option<VideoColorInfo> {
    let sps = if annex_b.len() >= 2 && hevc_nal_unit_type(annex_b) == Some(HEVC_NAL_SPS) {
        annex_b
    } else {
        split_annex_b_nalus(annex_b)
            .into_iter()
            .find(|n| n.len() > 2 && hevc_nal_unit_type(n) == Some(HEVC_NAL_SPS))?
    };

    let mut b = RbspBits::new(&sps[2..]);
    b.skip(4); // sps_video_parameter_set_id
    let sub_layers = b.u(3)? as usize;
    b.skip(1); // sps_temporal_id_nesting_flag
    b.skip(96); // general_profile_tier_level
    let mut sub_layer_flags = Vec::with_capacity(sub_layers);
    for _ in 0..sub_layers {
        sub_layer_flags.push((b.bit()?, b.bit()?));
    }
    if sub_layers > 0 {
        b.skip(2 * (8 - sub_layers));
    }
    for (prof_present, level_present) in sub_layer_flags {
        b.skip(88 * prof_present as usize + 8 * level_present as usize);
    }
    let _sps_id = b.ue()?;
    let chroma = b.ue()?;
    if chroma == 3 {
        b.skip(1);
    }
    let _w = b.ue()?;
    let _h = b.ue()?;
    if b.bit()? == 1 {
        let _l = b.ue()?;
        let _r = b.ue()?;
        let _t = b.ue()?;
        let _bot = b.ue()?;
    }
    let _bd_luma = b.ue()?;
    let _bd_chroma = b.ue()?;
    let log2_max_poc_lsb_m4 = b.ue()?;
    let ord_present = b.bit()? == 1;
    let start_layer = if ord_present { 0 } else { sub_layers };
    for _ in start_layer..=sub_layers {
        let _max_dec = b.ue()?;
        let _max_num_reorder = b.ue()?;
        let _max_latency = b.ue()?;
    }
    for _ in 0..6 {
        let _ = b.ue()?;
    }
    if b.bit()? == 1 && b.bit()? == 1 {
        return None; // custom sps_scaling_list_data
    }
    let _amp = b.bit()?;
    let _sao = b.bit()?;
    if b.bit()? == 1 {
        b.skip(8); // pcm sample bit depths
        let _ = b.ue()?;
        let _ = b.ue()?;
        b.skip(1);
    }
    let num_st_rps = b.ue()?;
    skip_hevc_st_ref_pic_sets(&mut b, num_st_rps)?;
    if b.bit()? == 1 {
        let num_lt = b.ue()? as usize;
        let poc_bits = (log2_max_poc_lsb_m4 as usize + 4).min(16);
        for _ in 0..num_lt {
            b.skip(poc_bits + 1);
        }
    }
    let _tmvp = b.bit()?;
    let _strong_intra = b.bit()?;
    read_h26x_vui_color_info(&mut b)
}

fn push_ue_bits(bits: &mut Vec<u8>, v: u32) {
    let val = v + 1;
    let total_bits = 32 - val.leading_zeros() as usize;
    let leading_zeros = total_bits - 1;
    for _ in 0..leading_zeros {
        bits.push(0);
    }
    for shift in (0..total_bits).rev() {
        bits.push(((val >> shift) & 1) as u8);
    }
}

fn escape_rbsp_to_nal_payload(raw_rbsp: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw_rbsp.len() + 4);
    let mut zeros = 0u32;
    for &byte in raw_rbsp {
        if zeros >= 2 && byte <= 3 {
            out.push(3);
            zeros = 0;
        }
        out.push(byte);
        zeros = if byte == 0 { zeros + 1 } else { 0 };
    }
    out
}

/// Builds a deterministic synthetic Annex-B HEVC `VPS (32) + SPS (33) + PPS (34) + IDR_W_RADL (19)` packet
/// carrying the real Exp-Golomb SPS geometry `(width, height)`.
pub fn build_synthetic_hevc_annex_b_idr(width: u32, height: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(128);
    // 1. VPS (type 32 -> header 0x40, 0x01)
    out.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, HEVC_NAL_VPS << 1, 0x01, 0x0C, 0x01]);

    // 2. SPS (type 33 -> header 0x42, 0x01)
    let mut bits: Vec<u8> = Vec::with_capacity(192);
    bits.extend_from_slice(&[0, 0, 0, 0]); // sps_video_parameter_set_id
    bits.extend_from_slice(&[0, 0, 0]); // sps_max_sub_layers_minus1 = 0
    bits.push(1); // sps_temporal_id_nesting_flag = 1
    bits.extend(std::iter::repeat_n(0u8, 96)); // general_profile_tier_level
    push_ue_bits(&mut bits, 0); // sps_seq_parameter_set_id
    push_ue_bits(&mut bits, 1); // chroma_format_idc = 1 (4:2:0)
    push_ue_bits(&mut bits, width); // pic_width_in_luma_samples
    push_ue_bits(&mut bits, height); // pic_height_in_luma_samples
    if width == 1216 && height == 2656 {
        bits.push(1); // conformance_window_flag
        push_ue_bits(&mut bits, 0); // left
        push_ue_bits(&mut bits, 7); // right (7 * 2 = 14)
        push_ue_bits(&mut bits, 0); // top
        push_ue_bits(&mut bits, 29); // bottom (29 * 2 = 58)
    } else {
        bits.push(0);
    }
    bits.push(1); // RBSP stop bit
    while bits.len() % 8 != 0 {
        bits.push(0);
    }
    let mut raw_sps = Vec::with_capacity(bits.len() / 8);
    for chunk in bits.chunks(8) {
        let mut b = 0u8;
        for &bit in chunk {
            b = (b << 1) | (bit & 1);
        }
        raw_sps.push(b);
    }
    out.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, HEVC_NAL_SPS << 1, 0x01]);
    out.extend_from_slice(&escape_rbsp_to_nal_payload(&raw_sps));

    // 3. PPS (type 34 -> header 0x44, 0x01)
    out.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, HEVC_NAL_PPS << 1, 0x01, 0xC1, 0x72]);

    // 4. IDR_W_RADL (type 19 -> header 0x26, 0x01)
    out.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, HEVC_NAL_IDR_W_RADL << 1, 0x01, 0xAF, 0x08, 0xFF]);
    out
}

/// Builds a synthetic Annex-B non-IDR HEVC `TRAIL_R (1)` access unit.
pub fn build_synthetic_hevc_annex_b_trail_r() -> Vec<u8> {
    vec![0x00, 0x00, 0x00, 0x01, HEVC_NAL_TRAIL_R << 1, 0x01, 0x9E, 0x10, 0x00]
}

/// Outcome from feeding an Annex-B HEVC access unit into `MfHevcDecoder`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeOutcome {
    pub decoded: bool,
    pub is_keyframe: bool,
    pub needs_keyframe_pli: bool,
    pub slot_idx: Option<usize>,
    pub frame_seq: u64,
    pub coded_width: u32,
    pub coded_height: u32,
    pub clean_width: u32,
    pub clean_height: u32,
}

/// Converts a single RGB triplet to BT.709 limited-range YUV `(Y, U, V)`.
#[inline(always)]
pub fn rgb_to_yuv_bt709(r: u8, g: u8, b: u8) -> (u8, u8, u8) {
    let ri = r as i32;
    let gi = g as i32;
    let bi = b as i32;
    let y = (((47 * ri + 157 * gi + 16 * bi + 128) >> 8) + 16).clamp(16, 235) as u8;
    let u = (((-26 * ri - 86 * gi + 112 * bi + 128) >> 8) + 128).clamp(16, 240) as u8;
    let v = (((112 * ri - 102 * gi - 10 * bi + 128) >> 8) + 128).clamp(16, 240) as u8;
    (y, u, v)
}

/// Converts BT.709 limited-range YUV `(Y, U, V)` to RGB using integer math (`0` floats).
#[inline(always)]
pub fn yuv_bt709_to_rgb(y: u8, u: u8, v: u8) -> (u8, u8, u8) {
    yuv_to_rgb_with_color_info(
        y,
        u,
        v,
        VideoColorInfo {
            matrix: VideoColorMatrix::Bt709,
            full_range: false,
        },
    )
}

/// Converts NV12 `(Y, U, V)` to sRGB `(R, G, B)` honoring `VideoColorInfo`
/// (`BT.709` / `BT.601` / `BT.2020`, Full `0..255` vs Limited `16..235`) matching D3D11 VP (`ΔRGB <= 3`).
#[inline(always)]
pub fn yuv_to_rgb_with_color_info(y: u8, u: u8, v: u8, color: VideoColorInfo) -> (u8, u8, u8) {
    let d = u as i32 - 128;
    let e = v as i32 - 128;
    let (cy, crv, cgu, cgv, cbu, c) = match (color.matrix, color.full_range) {
        (VideoColorMatrix::Bt709, true) => (256, 403, 48, 120, 475, y as i32),
        (VideoColorMatrix::Bt709, false) => (298, 459, 55, 136, 541, (y as i32 - 16).max(0)),
        (VideoColorMatrix::Bt601, true) => (256, 359, 88, 183, 454, y as i32),
        (VideoColorMatrix::Bt601, false) => (298, 409, 100, 208, 516, (y as i32 - 16).max(0)),
        (VideoColorMatrix::Bt2020, true) => (256, 378, 42, 146, 482, y as i32),
        (VideoColorMatrix::Bt2020, false) => (298, 430, 48, 167, 548, (y as i32 - 16).max(0)),
    };
    let y_term = cy * c + 128;
    let r = (y_term + crv * e) >> 8;
    let g = (y_term - cgu * d - cgv * e) >> 8;
    let b = (y_term + cbu * d) >> 8;
    (
        r.clamp(0, 255) as u8,
        g.clamp(0, 255) as u8,
        b.clamp(0, 255) as u8,
    )
}

/// Writes a synthetic or rendered NV12 planar+interleaved frame into `out_nv12` (in-place capacity reuse).
pub fn write_nv12_from_rgb_sampler(
    coded_w: u32,
    coded_h: u32,
    out_nv12: &mut Vec<u8>,
    sampler: impl Fn(u32, u32) -> (u8, u8, u8),
) {
    let w = coded_w as usize;
    let h = coded_h as usize;
    let y_len = w * h;
    let total_len = y_len + y_len / 2;
    out_nv12.resize(total_len, 0);
    let (y_plane, uv_plane) = out_nv12.split_at_mut(y_len);
    for row in 0..h {
        let row_off = row * w;
        let uv_off = (row >> 1) * w;
        for col in 0..w {
            let (r, g, b) = sampler(col as u32, row as u32);
            let (y, u, v) = rgb_to_yuv_bt709(r, g, b);
            y_plane[row_off + col] = y;
            if (row & 1) == 0 && (col & 1) == 0 && uv_off + col + 1 < uv_plane.len() {
                uv_plane[uv_off + col] = u;
                uv_plane[uv_off + col + 1] = v;
            }
        }
    }
}

/// Extracts a downscaled `120 x 260` RGBA probe from `SharedFrameSlot` (`nv12_data` or `bgra_data`)
/// over the clean cropped region (excluding bottom/right macroblock padding) with **0 heap allocations**.
pub fn extract_downscaled_probe_120x260_into(
    slot: &SharedFrameSlot,
    out_rgba_120x260: &mut [u8],
) -> bool {
    let needed = GRID_WIDTH * GRID_HEIGHT * 4;
    if out_rgba_120x260.len() < needed {
        return false;
    }
    let coded_w = slot.coded_width as usize;
    let coded_h = slot.coded_height as usize;
    let clean_w = slot.clean_width() as usize;
    let clean_h = slot.clean_height() as usize;
    if coded_w == 0 || coded_h == 0 || clean_w == 0 || clean_h == 0 {
        return false;
    }
    let off_x = slot.trim_left as usize;
    let off_y = slot.trim_top as usize;

    let has_nv12 = slot.nv12_data.len() >= coded_w * coded_h + (coded_w * coded_h) / 2;
    let has_bgra = slot.bgra_data.len() >= coded_w * coded_h * 4;
    if !has_nv12 && !has_bgra {
        return false;
    }

    let y_len = coded_w * coded_h;
    for gy in 0..GRID_HEIGHT {
        let sy = (off_y + (gy * clean_h) / GRID_HEIGHT).min(coded_h - 1);
        let row_dst = gy * GRID_WIDTH * 4;
        for gx in 0..GRID_WIDTH {
            let sx = (off_x + (gx * clean_w) / GRID_WIDTH).min(coded_w - 1);
            let dst = row_dst + gx * 4;
            let (r, g, b) = if has_nv12 {
                let y_val = slot.nv12_data[sy * coded_w + sx];
                let uv_idx = y_len + (sy >> 1) * coded_w + (sx & !1);
                let u_val = slot.nv12_data[uv_idx];
                let v_val = slot.nv12_data[uv_idx + 1];
                yuv_bt709_to_rgb(y_val, u_val, v_val)
            } else {
                let idx = (sy * coded_w + sx) * 4;
                (slot.bgra_data[idx + 2], slot.bgra_data[idx + 1], slot.bgra_data[idx])
            };
            out_rgba_120x260[dst] = r;
            out_rgba_120x260[dst + 1] = g;
            out_rgba_120x260[dst + 2] = b;
            out_rgba_120x260[dst + 3] = 255;
        }
    }
    true
}

/// Copies a `120 x 260 x 4` RGBA buffer into an existing `ProbeGrid120x260` in-place (`0` heap allocations).
pub fn populate_probe_grid_120x260_in_place(rgba_120x260: &[u8], grid: &mut ProbeGrid120x260) {
    if rgba_120x260.len() < GRID_WIDTH * GRID_HEIGHT * 4 {
        return;
    }
    for gy in 0..GRID_HEIGHT {
        let row = gy * GRID_WIDTH * 4;
        for gx in 0..GRID_WIDTH {
            let idx = row + gx * 4;
            grid.set_pixel(
                gx,
                gy,
                rgba_120x260[idx],
                rgba_120x260[idx + 1],
                rgba_120x260[idx + 2],
            );
        }
    }
}

/// Lossless BGRA8 snapshot readback cropped strictly by `(trim_top, trim_left, trim_bottom, trim_right)`.
/// For Apple H.265 `1216 x 2656`: trims `bottom: 58` and `right: 14` (`1202 x 2598`) while keeping
/// `top: 0, left: 0` untouched so Dynamic Island (`y = 0..12`) stays 100% preserved.
pub fn export_clean_cropped_bgra_frame(slot: &SharedFrameSlot) -> (u32, u32, Vec<u8>) {
    let coded_w = slot.coded_width as usize;
    let coded_h = slot.coded_height as usize;
    let clean_w = slot.clean_width() as usize;
    let clean_h = slot.clean_height() as usize;
    if coded_w == 0 || coded_h == 0 || clean_w == 0 || clean_h == 0 {
        return (0, 0, Vec::new());
    }

    let off_x = slot.trim_left as usize;
    let off_y = slot.trim_top as usize;
    let mut out = vec![0u8; clean_w * clean_h * 4];

    let has_nv12 = slot.nv12_data.len() >= coded_w * coded_h + (coded_w * coded_h) / 2;
    let has_bgra = slot.bgra_data.len() >= coded_w * coded_h * 4;

    if has_nv12 {
        let y_len = coded_w * coded_h;
        for y in 0..clean_h {
            let sy = (off_y + y).min(coded_h - 1);
            let y_row = sy * coded_w;
            let uv_row = y_len + (sy >> 1) * coded_w;
            let dst_row = y * clean_w * 4;
            for x in 0..clean_w {
                let sx = (off_x + x).min(coded_w - 1);
                let y_val = slot.nv12_data[y_row + sx];
                let uv_idx = uv_row + (sx & !1);
                let (r, g, b) = yuv_bt709_to_rgb(
                    y_val,
                    slot.nv12_data[uv_idx],
                    slot.nv12_data[uv_idx + 1],
                );
                let d = dst_row + x * 4;
                out[d] = b;
                out[d + 1] = g;
                out[d + 2] = r;
                out[d + 3] = 255;
            }
        }
    } else if has_bgra {
        let copy_bytes = clean_w * 4;
        for y in 0..clean_h {
            let sy = (off_y + y).min(coded_h - 1);
            let src_start = (sy * coded_w + off_x) * 4;
            let dst_start = y * clean_w * 4;
            out[dst_start..dst_start + copy_bytes]
                .copy_from_slice(&slot.bgra_data[src_start..src_start + copy_bytes]);
        }
    }

    (clean_w as u32, clean_h as u32, out)
}

/// Low-Latency Media Foundation HEVC (`IMFTransform`, `MF_LOW_LATENCY = 1`, `D3D11_AWARE = 1`)
/// decoder state machine managing parameter sets (`VPS/SPS/PPS`), PLI keyframe recovery, and
/// publishing decoded frames into the 6-slot `SharedFrameRing`.
#[derive(Debug, Clone)]
pub struct MfHevcDecoder {
    pub adapter_name: String,
    pub vendor_id: u32,
    pub device_id: u32,
    pub d3d11_video_support: bool,
    pub multithread_protected: bool,
    pub dxgi_device_manager_ready: bool,
    pub mfplat_loaded: bool,
    pub hardware_hevc_mft_available: bool,
    pub fallback_webcodecs_ready: bool,
    pub low_latency_mode: bool,
    pub d3d11_aware: bool,
    pub has_vps: bool,
    pub has_sps: bool,
    pub has_pps: bool,
    pub has_seen_idr: bool,
    pub needs_keyframe_pli: bool,
    pub coded_width: u32,
    pub coded_height: u32,
    pub trim: (u32, u32, u32, u32), // (top, left, bottom, right)
    pub frames_decoded: u64,
    pub pli_triggers: u64,
}

impl Default for MfHevcDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl MfHevcDecoder {
    pub fn new() -> Self {
        let ctx = get_or_probe_d3d11_video_context();
        Self {
            adapter_name: ctx.adapter_name.clone(),
            vendor_id: ctx.vendor_id,
            device_id: ctx.device_id,
            d3d11_video_support: ctx.d3d11_video_support,
            multithread_protected: ctx.multithread_protected,
            dxgi_device_manager_ready: ctx.dxgi_device_manager_ready,
            mfplat_loaded: ctx.mfplat_loaded,
            hardware_hevc_mft_available: ctx.hardware_hevc_mft_available,
            fallback_webcodecs_ready: ctx.fallback_webcodecs_ready,
            low_latency_mode: true, // MF_LOW_LATENCY = 1
            d3d11_aware: ctx.d3d11_video_support && ctx.dxgi_device_manager_ready,
            has_vps: false,
            has_sps: false,
            has_pps: false,
            has_seen_idr: false,
            needs_keyframe_pli: false,
            coded_width: 1216,
            coded_height: 2656,
            trim: (0, 0, 58, 14),
            frames_decoded: 0,
            pli_triggers: 0,
        }
    }

    pub fn reset_stream(&mut self) {
        self.has_vps = false;
        self.has_sps = false;
        self.has_pps = false;
        self.has_seen_idr = false;
        self.needs_keyframe_pli = false;
        self.coded_width = 1216;
        self.coded_height = 2656;
        self.trim = (0, 0, 58, 14);
    }

    pub fn notify_packet_loss(&mut self) {
        self.needs_keyframe_pli = true;
        self.pli_triggers = self.pli_triggers.saturating_add(1);
    }

    pub fn clean_dimensions(&self) -> (u32, u32) {
        (
            self.coded_width.saturating_sub(self.trim.1 + self.trim.3),
            self.coded_height.saturating_sub(self.trim.0 + self.trim.2),
        )
    }

    /// Feeds one Annex-B HEVC access unit (`VPS/SPS/PPS/IDR` or `TRAIL_R`) with custom NV12 fill callback.
    /// Automatically flags `needs_keyframe_pli = true` when a non-IDR frame arrives without an anchor keyframe.
    pub fn feed_annex_b_with_nv12_writer(
        &mut self,
        annex_b: &[u8],
        pts_us: u64,
        rotation_deg: u32,
        ring: &SharedFrameRing,
        fill_nv12: impl FnOnce(u32, u32, &mut Vec<u8>),
    ) -> DecodeOutcome {
        let nalus = split_annex_b_nalus(annex_b);
        if nalus.is_empty() {
            self.needs_keyframe_pli = true;
            self.pli_triggers = self.pli_triggers.saturating_add(1);
            let (cw, ch) = self.clean_dimensions();
            return DecodeOutcome {
                decoded: false,
                is_keyframe: false,
                needs_keyframe_pli: true,
                slot_idx: None,
                frame_seq: 0,
                coded_width: self.coded_width,
                coded_height: self.coded_height,
                clean_width: cw,
                clean_height: ch,
            };
        }

        let mut is_keyframe = false;
        for n in &nalus {
            match hevc_nal_unit_type(n) {
                Some(HEVC_NAL_VPS) => self.has_vps = true,
                Some(HEVC_NAL_SPS) => {
                    self.has_sps = true;
                    if let Some((w, h, tr)) = parse_hevc_sps_dimensions(annex_b) {
                        self.coded_width = w;
                        self.coded_height = h;
                        self.trim = tr;
                    }
                }
                Some(HEVC_NAL_PPS) => self.has_pps = true,
                Some(HEVC_NAL_IDR_W_RADL | HEVC_NAL_IDR_N_LP | HEVC_NAL_CRA_NUT) => {
                    is_keyframe = true;
                }
                _ => {}
            }
        }

        let params_ready = self.has_vps && self.has_sps && self.has_pps;
        if !is_keyframe && (!params_ready || !self.has_seen_idr || self.needs_keyframe_pli) {
            self.needs_keyframe_pli = true;
            self.pli_triggers = self.pli_triggers.saturating_add(1);
            let (cw, ch) = self.clean_dimensions();
            return DecodeOutcome {
                decoded: false,
                is_keyframe: false,
                needs_keyframe_pli: true,
                slot_idx: None,
                frame_seq: 0,
                coded_width: self.coded_width,
                coded_height: self.coded_height,
                clean_width: cw,
                clean_height: ch,
            };
        }

        if is_keyframe {
            self.has_seen_idr = true;
            self.needs_keyframe_pli = false;
        }

        let w = self.coded_width;
        let h = self.coded_height;
        let trim = self.trim;
        let (slot_idx, frame_seq, _dropped) =
            ring.push_nv12_slot(w, h, trim, pts_us, rotation_deg, |buf| {
                fill_nv12(w, h, buf);
            });
        self.frames_decoded = self.frames_decoded.saturating_add(1);
        let (cw, ch) = self.clean_dimensions();

        DecodeOutcome {
            decoded: true,
            is_keyframe,
            needs_keyframe_pli: false,
            slot_idx: Some(slot_idx),
            frame_seq,
            coded_width: w,
            coded_height: h,
            clean_width: cw,
            clean_height: ch,
        }
    }

    /// Feeds an Annex-B HEVC access unit without external surface readback (allocates/reuses compact NV12 slot if empty).
    pub fn feed_annex_b(
        &mut self,
        annex_b: &[u8],
        pts_us: u64,
        rotation_deg: u32,
        ring: &SharedFrameRing,
    ) -> DecodeOutcome {
        self.feed_annex_b_with_nv12_writer(annex_b, pts_us, rotation_deg, ring, |w, h, buf| {
            let need = (w as usize) * (h as usize) * 3 / 2;
            if buf.len() != need {
                buf.resize(need, 128);
            }
        })
    }
}

/// 4-Side Edge Crop parameters (0..24 px per side).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EdgeCrop {
    pub top: u32,
    pub bottom: u32,
    pub left: u32,
    pub right: u32,
}

/// Downscale factor setting for performance optimization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownscaleFactor {
    Native1x,     // 1:1 original resolution (1216x2656)
    Scale75,      // 0.75x high-performance downscale (912x1992)
}

/// Screen rotation angle for touch coordinate inversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RotationAngle {
    Deg0,   // Portrait
    Deg90,  // Landscape Left
    Deg180, // Inverted Portrait
    Deg270, // Landscape Right (eFootball 2027 Asymmetric Lock)
}

/// Inverse Touch Matrix: converts PC viewport normalized coordinates (0..65535)
/// into physical iOS device touch coordinates based on active rotation.
pub fn inverse_touch_transform(pc_x: u16, pc_y: u16, rotation: RotationAngle) -> (u16, u16) {
    match rotation {
        RotationAngle::Deg0 => (pc_x, pc_y),
        RotationAngle::Deg90 => (pc_y, 65535 - pc_x),
        RotationAngle::Deg180 => (65535 - pc_x, 65535 - pc_y),
        RotationAngle::Deg270 => (65535 - pc_y, pc_x),
    }
}

/// HEVC Hardware Video Processing Pipeline configuration.
#[derive(Debug, Clone)]
pub struct HevcPipelineConfig {
    pub raw_width: u32,
    pub raw_height: u32,
    pub macroblock_trim: (u32, u32, u32, u32), // (top, bottom, left, right)
    pub edge_crop: EdgeCrop,
    pub downscale: DownscaleFactor,
    pub rotation: RotationAngle,
}

impl Default for HevcPipelineConfig {
    fn default() -> Self {
        Self {
            raw_width: 1216,
            raw_height: 2656,
            macroblock_trim: (0, 58, 0, 14), // Default Apple H.265 trim
            edge_crop: EdgeCrop::default(),
            downscale: DownscaleFactor::Native1x,
            rotation: RotationAngle::Deg0,
        }
    }
}

impl HevcPipelineConfig {
    /// Calculate final presentation dimensions after macroblock trim, edge crop, and downscaling.
    pub fn calculate_output_dimensions(&self) -> (u32, u32) {
        let (top, bottom, left, right) = self.macroblock_trim;
        let mut w = self.raw_width.saturating_sub(left + right);
        let mut h = self.raw_height.saturating_sub(top + bottom);

        // Apply edge crop
        w = w.saturating_sub(self.edge_crop.left + self.edge_crop.right);
        h = h.saturating_sub(self.edge_crop.top + self.edge_crop.bottom);

        // Apply downscaling
        match self.downscale {
            DownscaleFactor::Native1x => (w, h),
            DownscaleFactor::Scale75 => ((w * 3) / 4, (h * 3) / 4),
        }
    }
}

// ============================================================================
// TAHAP 262 — PILAR 2: Unified Real COM LiveMfVideoDecoder (HEVC & H.264)
// ============================================================================

#[link(name = "ole32")]
extern "system" {
    fn CoTaskMemFree(pv: *mut c_void);
}

// Media Foundation COM IIDs & Attribute GUIDs
const IID_IMF_TRANSFORM: Guid = Guid {
    data1: 0xbf94c121,
    data2: 0x5b05,
    data3: 0x4e6f,
    data4: [0x80, 0x00, 0xba, 0x59, 0x89, 0x61, 0x41, 0x4d],
};

const IID_IMF_DXGI_BUFFER: Guid = Guid {
    data1: 0xe7174cfa,
    data2: 0x1c9e,
    data3: 0x48b1,
    data4: [0x88, 0x66, 0x62, 0x62, 0x26, 0xbf, 0xc2, 0x58],
};

const IID_IMF_2D_BUFFER: Guid = Guid {
    data1: 0x7dc9d5f9,
    data2: 0x9ed9,
    data3: 0x44ec,
    data4: [0x9b, 0xbf, 0x06, 0x00, 0xbb, 0x58, 0x9f, 0xbb],
};

const MF_MT_MAJOR_TYPE: Guid = Guid {
    data1: 0x48eba18e,
    data2: 0xf8c9,
    data3: 0x4687,
    data4: [0xbf, 0x11, 0x0a, 0x74, 0xc9, 0xf9, 0x6a, 0x8f],
};

const MF_MT_SUBTYPE: Guid = Guid {
    data1: 0xf7e34c9a,
    data2: 0x42e8,
    data3: 0x4714,
    data4: [0xb7, 0x4b, 0xcb, 0x29, 0xd7, 0x2c, 0x35, 0xe5],
};

const MF_MT_FRAME_SIZE: Guid = Guid {
    data1: 0x1652c33d,
    data2: 0xd6b2,
    data3: 0x4012,
    data4: [0xb8, 0x34, 0x72, 0x03, 0x08, 0x49, 0xa3, 0x7d],
};

const MF_MT_INTERLACE_MODE: Guid = Guid {
    data1: 0xe2724bb8,
    data2: 0xe676,
    data3: 0x4806,
    data4: [0xb4, 0xb2, 0xa8, 0xd6, 0xef, 0xb4, 0x4c, 0xcd],
};

const MF_MT_MINIMUM_DISPLAY_APERTURE: Guid = Guid {
    data1: 0xd7388766,
    data2: 0x18fe,
    data3: 0x48c6,
    data4: [0xa1, 0x77, 0xee, 0x89, 0x48, 0x67, 0xc8, 0xc4],
};

// DEFINE_GUID (MF_MT_GEOMETRIC_APERTURE, 0x66758743, 0x7e5f, 0x400d, 0x98, 0x0a, 0xaa, 0x85, 0x96, 0xc8, 0x56, 0x96)
const MF_MT_GEOMETRIC_APERTURE: Guid = Guid {
    data1: 0x66758743,
    data2: 0x7e5f,
    data3: 0x400d,
    data4: [0x98, 0x0a, 0xaa, 0x85, 0x96, 0xc8, 0x56, 0x96],
};
// DEFINE_GUID (MF_MT_PAN_SCAN_APERTURE, 0x79614dde, 0x9187, 0x48fb, 0xb8, 0xc7, 0x4d, 0x52, 0x68, 0x9d, 0xe6, 0x49)
const MF_MT_PAN_SCAN_APERTURE: Guid = Guid {
    data1: 0x79614dde,
    data2: 0x9187,
    data3: 0x48fb,
    data4: [0xb8, 0xc7, 0x4d, 0x52, 0x68, 0x9d, 0xe6, 0x49],
};

/// Reconciles MFT output aperture metadata (`MF_MT_MINIMUM_DISPLAY_APERTURE`, `MF_MT_GEOMETRIC_APERTURE`,
/// `MF_MT_PAN_SCAN_APERTURE`) against SPS conformance trim (`expected_trim` / `expected_coded_dims`) so that
/// cropping happens AT MOST ONCE (zero double-trimming).
/// Returns `(visible_rect, used_mft_aperture)`.
pub fn resolve_reconciled_aperture_rect(
    coded_w: u32,
    coded_h: u32,
    mft_aperture: Option<Rect>,
    codec: VideoCodecKind,
    expected_trim: (u32, u32, u32, u32),
    expected_coded_dims: Option<(u32, u32)>,
    is_airplay: bool,
) -> (Rect, bool) {
    let full = Rect {
        left: 0,
        top: 0,
        right: coded_w as i32,
        bottom: coded_h as i32,
    };
    if coded_w == 0 || coded_h == 0 {
        return (full, false);
    }

    if let Some(ap) = mft_aperture {
        let is_in_bounds = ap.left >= 0
            && ap.top >= 0
            && ap.right > ap.left
            && ap.bottom > ap.top
            && (ap.right as u32) <= coded_w
            && (ap.bottom as u32) <= coded_h;
        let is_strict_crop = ap.left > 0
            || ap.top > 0
            || (ap.right as u32) < coded_w
            || (ap.bottom as u32) < coded_h;
        if is_in_bounds && is_strict_crop {
            return (ap, true);
        }
    }

    let max_trim = match codec {
        VideoCodecKind::Hevc => 96,
        VideoCodecKind::H264 => 32,
    };
    let (tt, tl, tb, tr) = if is_airplay { (0, 0, 0, 0) } else { expected_trim };
    if (tt, tl, tb, tr) != (0, 0, 0, 0)
        && tl + tr <= max_trim
        && tt + tb <= max_trim
        && tl + tr < coded_w
        && tt + tb < coded_h
    {
        return (
            Rect {
                left: tl as i32,
                top: tt as i32,
                right: (coded_w - tr) as i32,
                bottom: (coded_h - tb) as i32,
            },
            false,
        );
    }

    if !is_airplay && codec == VideoCodecKind::H264 {
        if let Some((ew, eh)) = expected_coded_dims {
            if ew > 0
                && eh > 0
                && ew <= coded_w
                && eh <= coded_h
                && (ew < coded_w || eh < coded_h)
                && coded_w.saturating_sub(ew) <= 32
                && coded_h.saturating_sub(eh) <= 32
            {
                let (et, el, _eb, _er) = expected_trim;
                let ox = el.min(coded_w.saturating_sub(ew)) as i32;
                let oy = et.min(coded_h.saturating_sub(eh)) as i32;
                return (
                    Rect {
                        left: ox,
                        top: oy,
                        right: ox + ew as i32,
                        bottom: oy + eh as i32,
                    },
                    false,
                );
            }
        }
    }

    (full, false)
}

// DEFINE_GUID (MF_MT_DEFAULT_STRIDE, 0x644b4e48, 0x1e02, 0x4516, 0xb0, 0xeb, 0xc0, 0x1c, 0xa9, 0xd4, 0x9a, 0xc6)
const MF_MT_DEFAULT_STRIDE: Guid = Guid {
    data1: 0x644b4e48,
    data2: 0x1e02,
    data3: 0x4516,
    data4: [0xb0, 0xeb, 0xc0, 0x1c, 0xa9, 0xd4, 0x9a, 0xc6],
};

/// Ensures `ID3D10Multithread::SetMultithreadProtected(TRUE)` is active and confirmed via
/// `GetMultithreadProtected()` on the shared `ID3D11Device` / `ID3D11DeviceContext`.
pub fn ensure_d3d11_multithread_protected(gpu: &LiveD3d11Gpu) -> bool {
    let Ok(mt) = gpu
        .context()
        .query_interface(&IID_ID3D10_MULTITHREAD)
        .or_else(|_| gpu.device().query_interface(&IID_ID3D10_MULTITHREAD))
    else {
        return false;
    };
    unsafe {
        let vt = mt.vtable();
        let set_mt: unsafe extern "system" fn(*mut c_void, i32) -> i32 =
            std::mem::transmute(*vt.add(5));
        let get_mt: unsafe extern "system" fn(*mut c_void) -> i32 =
            std::mem::transmute(*vt.add(6));
        let _ = set_mt(mt.as_ptr(), 1);
        get_mt(mt.as_ptr()) != 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DxvaSubresourceCopyPlan {
    pub src_subresource: u32,
    pub copy_w: u32,
    pub copy_h: u32,
}

/// Computes an exact DXVA `CopySubresourceRegion` plan preserving `GetSubresourceIndex()` when MFT
/// outputs into a multi-slice `Texture2DArray` DPB pool (`ArraySize > 1`).
pub fn resolve_dxva_subresource_copy_plan(
    raw_subresource_index: u32,
    src_desc: &D3d11Texture2dDesc,
    dst_desc: &D3d11Texture2dDesc,
) -> DxvaSubresourceCopyPlan {
    let src_subresource = if src_desc.array_size > 1 && raw_subresource_index < src_desc.array_size
    {
        raw_subresource_index
    } else {
        0
    };
    let copy_w = dst_desc.width.min(src_desc.width).max(2) & !1;
    let copy_h = dst_desc.height.min(src_desc.height).max(2) & !1;
    DxvaSubresourceCopyPlan {
        src_subresource,
        copy_w,
        copy_h,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Nv12CpuPlaneLayout {
    pub src_stride: usize,
    pub y_plane_rows: usize,
    pub uv_plane_offset_bytes: usize,
    pub uv_plane_rows: usize,
    pub row_copy_bytes: usize,
}

/// Computes source stride (`MF_MT_DEFAULT_STRIDE` / `Lock2D` pitch) and aligned NV12 Y/UV plane
/// offsets so CPU software fallback uploads never shift chroma rows green/purple on padded MFT buffers.
pub fn compute_nv12_cpu_plane_layout(
    visible_w: u32,
    coded_h: u32,
    upload_w: u32,
    upload_h: u32,
    lock_pitch: Option<i32>,
    default_stride: i32,
    total_buffer_bytes: Option<usize>,
) -> Nv12CpuPlaneLayout {
    let min_w = visible_w.max(upload_w).max(2) as usize;
    let src_stride = lock_pitch
        .and_then(|p| {
            let abs = p.unsigned_abs() as usize;
            (abs > 0).then_some(abs)
        })
        .or_else(|| {
            let abs = default_stride.unsigned_abs() as usize;
            (abs > 0).then_some(abs)
        })
        .unwrap_or(min_w);
    let y_plane_rows = upload_h.max(2) as usize;
    let uv_plane_rows = (y_plane_rows / 2).max(1);
    let nominal_plane_h = coded_h.max(upload_h).max(2) as usize;
    let uv_start_row = total_buffer_bytes
        .filter(|&bytes| src_stride > 0 && bytes >= src_stride * (y_plane_rows + uv_plane_rows))
        .map(|bytes| {
            let total_rows = bytes / src_stride;
            let inferred = ((total_rows * 2) / 3) & !1usize;
            if inferred >= y_plane_rows && inferred <= nominal_plane_h.saturating_add(64) {
                inferred
            } else {
                nominal_plane_h
            }
        })
        .unwrap_or(nominal_plane_h);
    let row_copy_bytes = visible_w.min(upload_w).max(2) as usize;
    Nv12CpuPlaneLayout {
        src_stride,
        y_plane_rows,
        uv_plane_offset_bytes: uv_start_row.saturating_mul(src_stride),
        uv_plane_rows,
        row_copy_bytes,
    }
}

// DEFINE_GUID (IID_ICodecAPI, 0x901db4c7, 0x31ce, 0x41a2, 0x85, 0xdc, 0x8f, 0xa0, 0xbf, 0x41, 0xb8, 0xda)
const IID_ICODEC_API: Guid = Guid {
    data1: 0x901db4c7,
    data2: 0x31ce,
    data3: 0x41a2,
    data4: [0x85, 0xdc, 0x8f, 0xa0, 0xbf, 0x41, 0xb8, 0xda],
};

// CODECAPI_AVLowLatencyMode == {9c27891a-ed7a-40e1-88e8-b22727a024ee}
const CODECAPI_AV_LOW_LATENCY_MODE: Guid = Guid {
    data1: 0x9c27891a,
    data2: 0xed7a,
    data3: 0x40e1,
    data4: [0x88, 0xe8, 0xb2, 0x27, 0x27, 0xa0, 0x24, 0xee],
};

const VT_BOOL: u16 = 11;
const VT_UI4: u16 = 19;

#[repr(C)]
#[derive(Clone, Copy)]
struct VariantBool {
    vt: u16,
    w_reserved1: u16,
    w_reserved2: u16,
    w_reserved3: u16,
    bool_val: i16,
    _pad: [u16; 3],
    _tail: u64,
}

impl VariantBool {
    const fn variant_true() -> Self {
        Self {
            vt: VT_BOOL,
            w_reserved1: 0,
            w_reserved2: 0,
            w_reserved3: 0,
            bool_val: -1, // VARIANT_TRUE (0xFFFF)
            _pad: [0; 3],
            _tail: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VariantUl4 {
    vt: u16,
    w_reserved1: u16,
    w_reserved2: u16,
    w_reserved3: u16,
    ul_val: u32,
    _pad: u32,
    _tail: u64,
}

// DEFINE_GUID (MF_MT_YUV_MATRIX, 0x3e23d450, 0x2c75, 0x4d25, 0xa0, 0x0e, 0xb9, 0x16, 0x70, 0xd1, 0x23, 0x27)
const MF_MT_YUV_MATRIX: Guid = Guid {
    data1: 0x3e23d450,
    data2: 0x2c75,
    data3: 0x4d25,
    data4: [0xa0, 0x0e, 0xb9, 0x16, 0x70, 0xd1, 0x23, 0x27],
};

// DEFINE_GUID (MF_MT_VIDEO_NOMINAL_RANGE, 0xc21b8ee5, 0xb956, 0x4071, 0x8d, 0xaf, 0x32, 0x5e, 0xdf, 0x5c, 0xab, 0x11)
const MF_MT_VIDEO_NOMINAL_RANGE: Guid = Guid {
    data1: 0xc21b8ee5,
    data2: 0xb956,
    data3: 0x4071,
    data4: [0x8d, 0xaf, 0x32, 0x5e, 0xdf, 0x5c, 0xab, 0x11],
};

const MF_VIDEO_FORMAT_NV12: Guid = Guid {
    data1: 0x3231564e,
    data2: 0x0000,
    data3: 0x0010,
    data4: [0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71],
};

const MF_LOW_LATENCY: Guid = Guid {
    data1: 0x9c27891a,
    data2: 0xed7a,
    data3: 0x40e1,
    data4: [0x88, 0xe8, 0xb2, 0x27, 0x27, 0xa0, 0x24, 0xee],
};

const MF_SA_D3D11_AWARE: Guid = Guid {
    data1: 0x206b4fc8,
    data2: 0xfcf9,
    data3: 0x4c51,
    data4: [0xaf, 0xe3, 0x97, 0x64, 0x36, 0x9e, 0x33, 0xa0],
};

const MF_SAMPLE_EXTENSION_CLEAN_POINT: Guid = Guid {
    data1: 0x9cdf01d8,
    data2: 0xa0f0,
    data3: 0x43ba,
    data4: [0xb0, 0x77, 0xea, 0xa0, 0x6c, 0xbd, 0x72, 0x8a],
};

const MFT_MESSAGE_COMMAND_FLUSH: u32 = 0x0000_0000;
const MFT_MESSAGE_COMMAND_DRAIN: u32 = 0x0000_0001;
const MFT_MESSAGE_SET_D3D_MANAGER: u32 = 0x0000_0002;
const MFT_MESSAGE_NOTIFY_BEGIN_STREAMING: u32 = 0x1000_0000;
const MFT_MESSAGE_NOTIFY_END_OF_STREAM: u32 = 0x1000_0001;
const MFT_MESSAGE_NOTIFY_END_STREAMING: u32 = 0x1000_0002;
const MFT_MESSAGE_NOTIFY_START_OF_STREAM: u32 = 0x1000_0003;
const MFT_OUTPUT_STREAM_PROVIDES_SAMPLES: u32 = 0x0000_0100;
const MF_VIDEO_INTERLACE_PROGRESSIVE: u32 = 2;

const MF_E_NOTACCEPTING: i32 = 0xC00D_36B5u32 as i32;
const MF_E_TRANSFORM_STREAM_CHANGE: i32 = 0xC00D_6D61u32 as i32;
const MF_E_TRANSFORM_NEED_MORE_INPUT: i32 = 0xC00D_6D72u32 as i32;
const MF_E_NO_MORE_TYPES: i32 = 0xC00D_36B9u32 as i32;

const FRAME_DURATION_100NS: i64 = 166_667;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct MftOutputStreamInfo {
    dw_flags: u32,
    cb_size: u32,
    cb_alignment: u32,
}

#[repr(C)]
struct MftOutputDataBuffer {
    dw_stream_id: u32,
    p_sample: *mut c_void,
    dw_status: u32,
    p_events: *mut c_void,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct MfOffset {
    fract: u16,
    value: i16,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct MfVideoArea {
    offset_x: MfOffset,
    offset_y: MfOffset,
    cx: i32,
    cy: i32,
}

type FnMFTEnumExLocal = unsafe extern "system" fn(
    Guid,
    u32,
    *const MftRegisterTypeInfo,
    *const MftRegisterTypeInfo,
    *mut *mut *mut c_void,
    *mut u32,
) -> i32;
type FnMFCreateMediaType = unsafe extern "system" fn(*mut *mut c_void) -> i32;
type FnMFCreateSample = unsafe extern "system" fn(*mut *mut c_void) -> i32;
type FnMFCreateMemoryBuffer = unsafe extern "system" fn(u32, *mut *mut c_void) -> i32;

/// Video codec supported by `LiveMfVideoDecoder`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodecKind {
    /// CoreDevice DisplayService H.265/HEVC Annex-B (`MFVideoFormat_HEVC`).
    Hevc,
    /// AirPlay Screen Mirroring H.264/AVC (`MFVideoFormat_H264` / `CMSH264DecoderMFT`).
    H264,
}

impl VideoCodecKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            VideoCodecKind::Hevc => "HEVC",
            VideoCodecKind::H264 => "H.264",
        }
    }

    fn mf_subtype(&self) -> Guid {
        match self {
            VideoCodecKind::Hevc => MF_VIDEO_FORMAT_HEVC,
            VideoCodecKind::H264 => MF_VIDEO_FORMAT_H264,
        }
    }
}

/// Structured error returned by `LiveMfVideoDecoder` (zero panics on missing HEVC or Windows N/KN).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecoderError {
    MediaFoundationUnavailable(String),
    HevcExtensionMissing,
    CodecNotFound(VideoCodecKind),
    InvalidConfig(String),
    ComError { op: &'static str, hr: i32 },
}

impl std::fmt::Display for DecoderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecoderError::MediaFoundationUnavailable(s) => write!(f, "Media Foundation unavailable: {s}"),
            DecoderError::HevcExtensionMissing => {
                write!(f, "Microsoft HEVC Video Extensions MFT is not installed")
            }
            DecoderError::CodecNotFound(c) => write!(f, "No Media Foundation decoder MFT for {c:?}"),
            DecoderError::InvalidConfig(s) => write!(f, "Invalid codec configuration: {s}"),
            DecoderError::ComError { op, hr } => {
                write!(f, "Media Foundation COM error in {op}: HRESULT=0x{:08X}", *hr as u32)
            }
        }
    }
}

impl std::error::Error for DecoderError {}

/// Parsed ISO/IEC 14496-15 `AVCDecoderConfigurationRecord` (`avcC`) for AirPlay H.264.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvccConfig {
    pub nal_length_size: usize,
    pub sps_list: Vec<Vec<u8>>,
    pub pps_list: Vec<Vec<u8>>,
    pub raw_avcc: Vec<u8>,
    pub coded_dimensions: Option<(u32, u32, (u32, u32, u32, u32))>,
}

/// Parses an AirPlay H.264 `avcC` configuration record into SPS/PPS NALUs and `nal_length_size`.
pub fn parse_avcc_sps_pps(avcc: &[u8]) -> Result<AvccConfig, DecoderError> {
    if avcc.len() < 7 {
        return Err(DecoderError::InvalidConfig(format!(
            "avcC too short ({} bytes)",
            avcc.len()
        )));
    }
    if avcc[0] != 1 {
        return Err(DecoderError::InvalidConfig(format!(
            "unsupported avcC configurationVersion {}",
            avcc[0]
        )));
    }
    let nal_length_size = ((avcc[4] & 0x03) as usize) + 1;
    let num_sps = (avcc[5] & 0x1F) as usize;
    let mut pos = 6usize;
    let mut sps_list = Vec::with_capacity(num_sps);
    for _ in 0..num_sps {
        if pos + 2 > avcc.len() {
            return Err(DecoderError::InvalidConfig("truncated avcC SPS length".to_string()));
        }
        let len = u16::from_be_bytes([avcc[pos], avcc[pos + 1]]) as usize;
        pos += 2;
        if pos + len > avcc.len() {
            return Err(DecoderError::InvalidConfig("truncated avcC SPS payload".to_string()));
        }
        if len > 0 {
            sps_list.push(avcc[pos..pos + len].to_vec());
        }
        pos += len;
    }
    if pos >= avcc.len() {
        return Err(DecoderError::InvalidConfig("missing avcC PPS count".to_string()));
    }
    let num_pps = avcc[pos] as usize;
    pos += 1;
    let mut pps_list = Vec::with_capacity(num_pps);
    for _ in 0..num_pps {
        if pos + 2 > avcc.len() {
            return Err(DecoderError::InvalidConfig("truncated avcC PPS length".to_string()));
        }
        let len = u16::from_be_bytes([avcc[pos], avcc[pos + 1]]) as usize;
        pos += 2;
        if pos + len > avcc.len() {
            return Err(DecoderError::InvalidConfig("truncated avcC PPS payload".to_string()));
        }
        if len > 0 {
            pps_list.push(avcc[pos..pos + len].to_vec());
        }
        pos += len;
    }
    let coded_dimensions = sps_list.first().and_then(|s| parse_h264_sps_dimensions(s));
    Ok(AvccConfig {
        nal_length_size,
        sps_list,
        pps_list,
        raw_avcc: avcc.to_vec(),
        coded_dimensions,
    })
}

#[inline]
fn has_annex_b_prefix(sample: &[u8]) -> bool {
    (sample.len() >= 4
        && sample[0] == 0
        && sample[1] == 0
        && sample[2] == 0
        && sample[3] == 1)
        || (sample.len() >= 3 && sample[0] == 0 && sample[1] == 0 && sample[2] == 1)
}

/// Walks an ISO/IEC 14496-15 length-prefixed H.264 AVCC access unit strictly.
/// Eliminates `256..=511 B` (`00 00 01 xx`) and `1 B` (`00 00 00 01`) length-prefix collisions
/// with Annex-B start codes by validating `nal_len > 0`, `pos + nal_len <= sample.len()`,
/// `(nal[0] & 0x80) == 0` (`forbidden_zero_bit == 0`), `(nal[0] & 0x1F) != 0`, and exact end-of-buffer consumption.
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

fn avcc_strict_sample_to_annex_b(
    cfg: &AvccConfig,
    sample: &[u8],
    is_keyframe: bool,
) -> (Vec<u8>, bool) {
    if sample.is_empty() {
        return (Vec::new(), false);
    }
    let mut nalus: Vec<&[u8]> = if let Some(strict) =
        try_walk_strict_avcc_nalus(sample, cfg.nal_length_size)
    {
        strict
    } else {
        let ls = cfg.nal_length_size.clamp(1, 4);
        let mut pos = 0usize;
        let mut fallback = Vec::new();
        while pos + ls <= sample.len() {
            let mut nal_len = 0usize;
            for &b in &sample[pos..pos + ls] {
                nal_len = (nal_len << 8) | usize::from(b);
            }
            pos += ls;
            if nal_len == 0 || pos + nal_len > sample.len() {
                break;
            }
            let nal = &sample[pos..pos + nal_len];
            pos += nal_len;
            if (nal[0] & 0x80) != 0 || (nal[0] & 0x1F) == 0 {
                break;
            }
            fallback.push(nal);
        }
        fallback
    };

    let has_256_to_511_nal = nalus.iter().any(|n| (256..=511).contains(&n.len()));
    let mut has_sps = false;
    let mut has_pps = false;
    for nal in &nalus {
        match nal[0] & 0x1F {
            7 => has_sps = true,
            8 => has_pps = true,
            _ => {}
        }
    }

    let mut out = Vec::with_capacity(sample.len() + 128);
    if is_keyframe {
        if !has_sps {
            for sps in &cfg.sps_list {
                out.extend_from_slice(&[0, 0, 0, 1]);
                out.extend_from_slice(sps);
            }
        }
        if !has_pps {
            for pps in &cfg.pps_list {
                out.extend_from_slice(&[0, 0, 0, 1]);
                out.extend_from_slice(pps);
            }
        }
    }
    for nal in nalus.drain(..) {
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(nal);
    }
    (out, has_256_to_511_nal)
}

/// Converts a length-prefixed AVCC H.264 access unit into Annex-B (`00 00 00 01`), prepending
/// cached SPS+PPS NALUs at the start of every IDR keyframe (skipping duplicates if already present).
/// Always tries strict AVCC parsing FIRST before any Annex-B fallback so `256..=511 B` or `1 B`
/// AVCC samples are never misread as Annex-B start codes.
pub fn avcc_sample_to_annex_b(cfg: &AvccConfig, sample: &[u8], is_keyframe: bool) -> Vec<u8> {
    if sample.is_empty() {
        return Vec::new();
    }
    if try_walk_strict_avcc_nalus(sample, cfg.nal_length_size).is_some() {
        return avcc_strict_sample_to_annex_b(cfg, sample, is_keyframe).0;
    }

    let mut has_sps = false;
    let mut has_pps = false;
    let nalus: Vec<&[u8]> = if has_annex_b_prefix(sample) {
        split_annex_b_nalus(sample)
    } else {
        Vec::new()
    };
    if nalus.is_empty() {
        return avcc_strict_sample_to_annex_b(cfg, sample, is_keyframe).0;
    }
    for n in &nalus {
        if let Some(first) = n.first() {
            match first & 0x1F {
                7 => has_sps = true,
                8 => has_pps = true,
                _ => {}
            }
        }
    }
    let mut out = Vec::with_capacity(sample.len() + 128);
    if is_keyframe {
        if !has_sps {
            for sps in &cfg.sps_list {
                out.extend_from_slice(&[0, 0, 0, 1]);
                out.extend_from_slice(sps);
            }
        }
        if !has_pps {
            for pps in &cfg.pps_list {
                out.extend_from_slice(&[0, 0, 0, 1]);
                out.extend_from_slice(pps);
            }
        }
    }
    for nal in nalus {
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(nal);
    }
    out
}

/// Scans a strictly valid AVCC access unit for inline SPS (`7`) / PPS (`8`) / IDR (`5`) NALUs
/// and refreshes `cfg` only when valid parameter sets (`forbidden_zero_bit == 0` and SPS dims `>= 64x64`) change.
fn refresh_inline_avcc_params_and_detect_idr(cfg: &mut AvccConfig, sample: &[u8]) -> (bool, bool) {
    let Some(nalus) = try_walk_strict_avcc_nalus(sample, cfg.nal_length_size) else {
        return (false, false);
    };

    let mut inline_sps: Vec<Vec<u8>> = Vec::new();
    let mut inline_pps: Vec<Vec<u8>> = Vec::new();
    let mut has_idr = false;

    for nal in nalus {
        let hdr = nal[0];
        if (hdr & 0x80) != 0 {
            continue;
        }
        match hdr & 0x1F {
            5 => has_idr = true,
            7 => {
                if parse_h264_sps_dimensions(nal).is_some() {
                    inline_sps.push(nal.to_vec());
                }
            }
            8 => inline_pps.push(nal.to_vec()),
            _ => {}
        }
    }

    let mut params_changed = false;
    if !inline_sps.is_empty() && inline_sps != cfg.sps_list {
        cfg.sps_list = inline_sps;
        cfg.coded_dimensions = cfg.sps_list.first().and_then(|s| parse_h264_sps_dimensions(s));
        params_changed = true;
    }
    if !inline_pps.is_empty() && inline_pps != cfg.pps_list {
        cfg.pps_list = inline_pps;
        params_changed = true;
    }
    (has_idr, params_changed)
}

/// Only flush/reset MFT DPB after `>= 5` consecutive non-`MF_E_NOTACCEPTING` input errors.
pub const H264_MAX_CONSECUTIVE_INPUT_ERRORS_BEFORE_FLUSH: u32 = 5;

/// Returns `true` when `nal` is an H.264 SEI (`nal_type == 6`) containing a Recovery Point (`payload_type == 6`).
pub fn is_h264_sei_recovery_point(nal: &[u8]) -> bool {
    if nal.len() < 3 || (nal[0] & 0x1F) != 6 {
        return false;
    }
    let mut rbsp = Vec::with_capacity(nal.len());
    let payload = &nal[1..];
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

/// Returns `true` when `nal` is a non-IDR slice (`nal_type == 1`) whose Exp-Golomb `slice_type` is I (`2` or `7`).
pub fn is_h264_i_slice(nal: &[u8]) -> bool {
    if nal.len() < 2 || (nal[0] & 0x1F) != 1 {
        return false;
    }
    let mut b = RbspBits::new(&nal[1..]);
    if b.ue().is_none() {
        return false;
    }
    matches!(b.ue(), Some(2) | Some(7))
}

fn h264_nal_is_clean_random_access(nal: &[u8]) -> bool {
    let Some(&first) = nal.first() else {
        return false;
    };
    if (first & 0x80) != 0 {
        return false;
    }
    match first & 0x1F {
        9 | 7 | 8 | 12 => false,
        5 => true,
        6 => is_h264_sei_recovery_point(nal),
        1 => is_h264_i_slice(nal),
        _ => false,
    }
}

/// Scans ALL NALUs inside an H.264 Access Unit (strict length-prefixed AVCC first, Annex-B fallback
/// only if strict AVCC fails AND prefix matches Annex-B start code), returning `true` on IDR (`5`),
/// SEI Recovery Point (`6/6`), or Open-GOP I/All-I slice (`nal=1, slice_type=2|7`).
pub fn h264_avcc_payload_is_clean_random_access(sample: &[u8], nal_len_size: usize) -> bool {
    if sample.is_empty() {
        return false;
    }
    if let Some(nalus) = try_walk_strict_avcc_nalus(sample, nal_len_size) {
        return nalus.into_iter().any(h264_nal_is_clean_random_access);
    }
    if has_annex_b_prefix(sample) {
        return split_annex_b_nalus(sample)
            .into_iter()
            .any(h264_nal_is_clean_random_access);
    }
    false
}

fn detect_h264_open_gop_resume_strict(cfg: &AvccConfig, sample: &[u8]) -> bool {
    let Some(nalus) = try_walk_strict_avcc_nalus(sample, cfg.nal_length_size) else {
        return false;
    };
    nalus.into_iter().any(h264_nal_is_clean_random_access)
}

impl RbspBits {
    fn se(&mut self) -> Option<i32> {
        let code = self.ue()?;
        let mag = ((code + 1) >> 1) as i32;
        if (code & 1) == 1 {
            Some(mag)
        } else {
            Some(-mag)
        }
    }
}

/// Parses an H.264/AVC SPS NALU (either raw NALU starting with `forbidden_zero_bit == 0` & type `7`
/// or inside an Annex-B stream) and returns `(coded_width, coded_height, (trim_top, trim_left, trim_bottom, trim_right))`
/// enforcing valid display bounds `(64..=8192)`.
pub fn parse_h264_sps_dimensions(data: &[u8]) -> Option<(u32, u32, (u32, u32, u32, u32))> {
    let sps = if has_annex_b_prefix(data) {
        split_annex_b_nalus(data)
            .into_iter()
            .find(|n| n.len() > 4 && (n[0] & 0x80) == 0 && (n[0] & 0x1F) == 7)?
    } else if data.len() > 4 && (data[0] & 0x80) == 0 && (data[0] & 0x1F) == 7 {
        data
    } else {
        return None;
    };

    let mut b = RbspBits::new(&sps[1..]);
    let profile_idc = b.u(8)?;
    let _constraints = b.u(8)?;
    let _level_idc = b.u(8)?;
    let _sps_id = b.ue()?;

    let mut chroma_format_idc = 1u32;
    let mut separate_colour_plane_flag = 0u32;
    if matches!(
        profile_idc,
        100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135
    ) {
        chroma_format_idc = b.ue()?;
        if chroma_format_idc == 3 {
            separate_colour_plane_flag = b.bit()?;
        }
        let _bit_depth_luma = b.ue()?;
        let _bit_depth_chroma = b.ue()?;
        let _qpprime_bypass = b.bit()?;
        if b.bit()? == 1 {
            let count = if chroma_format_idc != 3 { 8 } else { 12 };
            for i in 0..count {
                if b.bit()? == 1 {
                    let size = if i < 6 { 16 } else { 64 };
                    let mut last_scale = 8i32;
                    let mut next_scale = 8i32;
                    for _ in 0..size {
                        if next_scale != 0 {
                            let delta = b.se()?;
                            next_scale = (last_scale + delta + 256) % 256;
                        }
                        last_scale = if next_scale == 0 { last_scale } else { next_scale };
                    }
                }
            }
        }
    }

    let _log2_max_frame_num_m4 = b.ue()?;
    let poc_type = b.ue()?;
    if poc_type == 0 {
        let _log2_max_poc_lsb_m4 = b.ue()?;
    } else if poc_type == 1 {
        let _delta_pic_order_always_zero = b.bit()?;
        let _offset_non_ref = b.se()?;
        let _offset_top_to_bottom = b.se()?;
        let num_ref_in_cycle = b.ue()?;
        for _ in 0..num_ref_in_cycle {
            let _ = b.se()?;
        }
    }
    let _max_num_ref_frames = b.ue()?;
    let _gaps_allowed = b.bit()?;
    let pic_width_in_mbs_minus1 = b.ue()?;
    let pic_height_in_map_units_minus1 = b.ue()?;
    let frame_mbs_only_flag = b.bit()?;
    if frame_mbs_only_flag == 0 {
        let _mb_adaptive_frame_field = b.bit()?;
    }
    let _direct_8x8 = b.bit()?;

    let coded_w = (pic_width_in_mbs_minus1 + 1) * 16;
    let coded_h = (2 - frame_mbs_only_flag) * (pic_height_in_map_units_minus1 + 1) * 16;
    if !(64..=8192).contains(&coded_w) || !(64..=8192).contains(&coded_h) {
        return None;
    }

    let mut trim = (0u32, 0u32, 0u32, 0u32);
    if b.bit().unwrap_or(0) == 1 {
        let chroma_array_type = if separate_colour_plane_flag == 0 {
            chroma_format_idc
        } else {
            0
        };
        let (sub_w_c, sub_h_c) = match chroma_array_type {
            1 => (2u32, 2u32),
            2 => (2u32, 1u32),
            _ => (1u32, 1u32),
        };
        let (crop_unit_x, crop_unit_y) = if chroma_array_type == 0 {
            (1u32, 2 - frame_mbs_only_flag)
        } else {
            (sub_w_c, sub_h_c * (2 - frame_mbs_only_flag))
        };
        let l = b.ue().unwrap_or(0) * crop_unit_x;
        let r = b.ue().unwrap_or(0) * crop_unit_x;
        let t = b.ue().unwrap_or(0) * crop_unit_y;
        let bot = b.ue().unwrap_or(0) * crop_unit_y;
        if l + r < coded_w && t + bot < coded_h {
            trim = (t, l, bot, r);
        }
    }
    Some((coded_w, coded_h, trim))
}

/// Extracts `VideoColorInfo` (`BT.709`/`BT.601`/`BT.2020`, Full vs Limited range) from H.264 SPS VUI.
pub fn parse_h264_sps_vui_color(data: &[u8]) -> Option<VideoColorInfo> {
    let sps = if has_annex_b_prefix(data) {
        split_annex_b_nalus(data)
            .into_iter()
            .find(|n| n.len() > 4 && (n[0] & 0x80) == 0 && (n[0] & 0x1F) == 7)?
    } else if data.len() > 4 && (data[0] & 0x80) == 0 && (data[0] & 0x1F) == 7 {
        data
    } else {
        return None;
    };

    let mut b = RbspBits::new(&sps[1..]);
    let profile_idc = b.u(8)?;
    let _constraints = b.u(8)?;
    let _level_idc = b.u(8)?;
    let _sps_id = b.ue()?;
    if matches!(
        profile_idc,
        100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135
    ) {
        let chroma_format_idc = b.ue()?;
        if chroma_format_idc == 3 {
            let _ = b.bit()?;
        }
        let _ = b.ue()?;
        let _ = b.ue()?;
        let _ = b.bit()?;
        if b.bit()? == 1 {
            let count = if chroma_format_idc != 3 { 8 } else { 12 };
            for i in 0..count {
                if b.bit()? == 1 {
                    let size = if i < 6 { 16 } else { 64 };
                    let mut last_scale = 8i32;
                    let mut next_scale = 8i32;
                    for _ in 0..size {
                        if next_scale != 0 {
                            let delta = b.se()?;
                            next_scale = (last_scale + delta + 256) % 256;
                        }
                        last_scale = if next_scale == 0 { last_scale } else { next_scale };
                    }
                }
            }
        }
    }
    let _ = b.ue()?;
    let poc_type = b.ue()?;
    if poc_type == 0 {
        let _ = b.ue()?;
    } else if poc_type == 1 {
        let _ = b.bit()?;
        let _ = b.se()?;
        let _ = b.se()?;
        let n = b.ue()?;
        for _ in 0..n {
            let _ = b.se()?;
        }
    }
    let _ = b.ue()?;
    let _ = b.bit()?;
    let _ = b.ue()?;
    let _ = b.ue()?;
    let frame_mbs_only_flag = b.bit()?;
    if frame_mbs_only_flag == 0 {
        let _ = b.bit()?;
    }
    let _ = b.bit()?;
    if b.bit()? == 1 {
        let _ = b.ue()?;
        let _ = b.ue()?;
        let _ = b.ue()?;
        let _ = b.ue()?;
    }
    read_h26x_vui_color_info(&mut b)
}

/// Resolves deterministic BT.709 color metadata for AirPlay H.264 streams:
/// defaults to BT.709 Studio (`16..235`) unless SPS VUI explicitly signals `video_full_range_flag == 1`,
/// immune to inconsistent Media Foundation output type attribute mutations.
pub fn resolve_deterministic_bt709_color(vui: Option<VideoColorInfo>) -> VideoColorInfo {
    VideoColorInfo {
        matrix: VideoColorMatrix::Bt709,
        full_range: vui.map(|v| v.full_range).unwrap_or(false),
    }
}

/// Returns `true` if the H.264 access unit (strict AVCC first, Annex-B fallback only if strict AVCC
/// fails and prefix matches Annex-B) contains at least one VCL slice NALU (`1..=5`), or uses a safe
/// fallback for opaque non-Annex-B `>= 6` byte payloads.
pub fn h264_avcc_has_video_slice_nal(avcc_payload: &[u8], nal_length_size: usize) -> bool {
    if avcc_payload.len() < 2 {
        return false;
    }
    if let Some(nalus) = try_walk_strict_avcc_nalus(avcc_payload, nal_length_size) {
        return nalus
            .into_iter()
            .any(|nal| !nal.is_empty() && (1..=5).contains(&(nal[0] & 0x1F)));
    }
    if has_annex_b_prefix(avcc_payload) {
        return split_annex_b_nalus(avcc_payload)
            .into_iter()
            .any(|nal| !nal.is_empty() && (1..=5).contains(&(nal[0] & 0x1F)));
    }
    avcc_payload.len() >= 6
}

fn coded_tuple_to_visible(dims: Option<(u32, u32, (u32, u32, u32, u32))>) -> Option<(u32, u32)> {
    dims.and_then(|(cw, ch, tr)| {
        let vw = cw.saturating_sub(tr.1 + tr.3);
        let vh = ch.saturating_sub(tr.0 + tr.2);
        if vw > 0 && vh > 0 { Some((vw, vh)) } else { None }
    })
}

/// Deterministic, 100% ITU-T H.264 Baseline `I_PCM` (`mb_type = 25`) IDR frame bundle
/// suitable for real COM decoding on Windows `CMSH264DecoderMFT` (Annex-B and `avcC`+AVCC).
#[derive(Debug, Clone)]
pub struct SyntheticH264Frame {
    pub width: u32,
    pub height: u32,
    pub sps: Vec<u8>,
    pub pps: Vec<u8>,
    pub idr_slice: Vec<u8>,
    pub avcc_config: Vec<u8>,
    pub avcc_sample: Vec<u8>,
    pub annex_b: Vec<u8>,
}

fn bits_to_bytes(bits: &mut Vec<u8>) -> Vec<u8> {
    while bits.len() % 8 != 0 {
        bits.push(0);
    }
    let mut out = Vec::with_capacity(bits.len() / 8);
    for chunk in bits.chunks_exact(8) {
        let mut byte = 0u8;
        for &b in chunk {
            byte = (byte << 1) | (b & 1);
        }
        out.push(byte);
    }
    out
}

fn push_fixed_bits(bits: &mut Vec<u8>, val: u32, n: usize) {
    for shift in (0..n).rev() {
        bits.push(((val >> shift) & 1) as u8);
    }
}

/// Synthesizes a 100% spec-valid H.264 Baseline `I_PCM` (`mb_type = 25`) IDR frame of size
/// `(width, height)` (rounded up to multiples of 16) with controlled constant luma `luma_y`
/// and neutral chroma (`Cb = 0x80, Cr = 0x80`).
pub fn build_synthetic_h264_ipcm_idr(
    width: u32,
    height: u32,
    luma_y: u8,
    idr_pic_id: u32,
) -> SyntheticH264Frame {
    let mb_w = (width.max(16) + 15) / 16;
    let mb_h = (height.max(16) + 15) / 16;
    let w = mb_w * 16;
    let h = mb_h * 16;

    // 1. SPS NALU (header 0x67, Baseline profile_idc=66, constraint=0x80, level=30)
    let mut sps_bits = Vec::with_capacity(128);
    push_fixed_bits(&mut sps_bits, 66, 8); // profile_idc = Baseline
    push_fixed_bits(&mut sps_bits, 0x80, 8); // constraint_set0_flag = 1
    push_fixed_bits(&mut sps_bits, 30, 8); // level_idc = 3.0
    push_ue_bits(&mut sps_bits, 0); // seq_parameter_set_id = 0
    push_ue_bits(&mut sps_bits, 0); // log2_max_frame_num_minus4 = 0 (frame_num is 4 bits)
    push_ue_bits(&mut sps_bits, 2); // pic_order_cnt_type = 2 (no POC fields in slice header)
    push_ue_bits(&mut sps_bits, 1); // max_num_ref_frames = 1
    sps_bits.push(0); // gaps_in_frame_num_value_allowed_flag = 0
    push_ue_bits(&mut sps_bits, mb_w - 1); // pic_width_in_mbs_minus1
    push_ue_bits(&mut sps_bits, mb_h - 1); // pic_height_in_map_units_minus1
    sps_bits.push(1); // frame_mbs_only_flag = 1
    sps_bits.push(1); // direct_8x8_inference_flag = 1
    sps_bits.push(0); // frame_cropping_flag = 0
    sps_bits.push(0); // vui_parameters_present_flag = 0
    sps_bits.push(1); // rbsp_stop_one_bit
    let sps_rbsp = bits_to_bytes(&mut sps_bits);
    let mut sps = Vec::with_capacity(1 + sps_rbsp.len() + 2);
    sps.push(0x67);
    sps.extend_from_slice(&escape_rbsp_to_nal_payload(&sps_rbsp));

    // 2. PPS NALU (header 0x68, CAVLC, deblocking_filter_control_present_flag = 1)
    let mut pps_bits = Vec::with_capacity(64);
    push_ue_bits(&mut pps_bits, 0); // pic_parameter_set_id = 0
    push_ue_bits(&mut pps_bits, 0); // seq_parameter_set_id = 0
    pps_bits.push(0); // entropy_coding_mode_flag = 0 (CAVLC)
    pps_bits.push(0); // bottom_field_pic_order_in_frame_present_flag = 0
    push_ue_bits(&mut pps_bits, 0); // num_slice_groups_minus1 = 0
    push_ue_bits(&mut pps_bits, 0); // num_ref_idx_l0_default_active_minus1 = 0
    push_ue_bits(&mut pps_bits, 0); // num_ref_idx_l1_default_active_minus1 = 0
    pps_bits.push(0); // weighted_pred_flag = 0
    push_fixed_bits(&mut pps_bits, 0, 2); // weighted_bipred_idc = 0
    push_ue_bits(&mut pps_bits, 0); // pic_init_qp_minus26 = se(0)
    push_ue_bits(&mut pps_bits, 0); // pic_init_qs_minus26 = se(0)
    push_ue_bits(&mut pps_bits, 0); // chroma_qp_index_offset = se(0)
    pps_bits.push(1); // deblocking_filter_control_present_flag = 1
    pps_bits.push(0); // constrained_intra_pred_flag = 0
    pps_bits.push(0); // redundant_pic_cnt_present_flag = 0
    pps_bits.push(1); // rbsp_stop_one_bit
    let pps_rbsp = bits_to_bytes(&mut pps_bits);
    let mut pps = Vec::with_capacity(1 + pps_rbsp.len() + 2);
    pps.push(0x68);
    pps.extend_from_slice(&escape_rbsp_to_nal_payload(&pps_rbsp));

    // 3. IDR Slice NALU (header 0x65) with mb_w * mb_h I_PCM (mb_type = 25) macroblocks
    let mut sl_bits = Vec::with_capacity((mb_w * mb_h) as usize * 392 * 8 + 128);
    push_ue_bits(&mut sl_bits, 0); // first_mb_in_slice = 0
    push_ue_bits(&mut sl_bits, 7); // slice_type = 7 (I slice, all I)
    push_ue_bits(&mut sl_bits, 0); // pic_parameter_set_id = 0
    push_fixed_bits(&mut sl_bits, 0, 4); // frame_num = 0 (4 bits)
    push_ue_bits(&mut sl_bits, idr_pic_id); // idr_pic_id
    sl_bits.push(0); // no_output_of_prior_pics_flag = 0
    sl_bits.push(0); // long_term_reference_flag = 0
    push_ue_bits(&mut sl_bits, 0); // slice_qp_delta = se(0)
    push_ue_bits(&mut sl_bits, 1); // disable_deblocking_filter_idc = 1

    let total_mbs = (mb_w * mb_h) as usize;
    for _ in 0..total_mbs {
        push_ue_bits(&mut sl_bits, 25); // mb_type = I_PCM (25)
        while sl_bits.len() % 8 != 0 {
            sl_bits.push(0); // pcm_alignment_zero_bit
        }
        // 256 luma samples (16x16)
        for _ in 0..256 {
            push_fixed_bits(&mut sl_bits, u32::from(luma_y), 8);
        }
        // 64 Cb + 64 Cr chroma samples (8x8 each)
        for _ in 0..128 {
            push_fixed_bits(&mut sl_bits, 0x80, 8);
        }
    }
    sl_bits.push(1); // rbsp_stop_one_bit
    let sl_rbsp = bits_to_bytes(&mut sl_bits);
    let mut idr_slice = Vec::with_capacity(1 + sl_rbsp.len() + 16);
    idr_slice.push(0x65);
    idr_slice.extend_from_slice(&escape_rbsp_to_nal_payload(&sl_rbsp));

    // 4. ISO/IEC 14496-15 avcC record
    let mut avcc_config = Vec::with_capacity(11 + sps.len() + pps.len());
    avcc_config.push(1); // configurationVersion
    avcc_config.push(66); // AVCProfileIndication
    avcc_config.push(0x80); // profile_compatibility
    avcc_config.push(30); // AVCLevelIndication
    avcc_config.push(0xFF); // 6 reserved 1-bits + lengthSizeMinusOne=3 (4-byte lengths)
    avcc_config.push(0xE1); // 3 reserved 1-bits + numOfSequenceParameterSets=1
    avcc_config.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    avcc_config.extend_from_slice(&sps);
    avcc_config.push(1); // numOfPictureParameterSets = 1
    avcc_config.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    avcc_config.extend_from_slice(&pps);

    // 5. Length-prefixed AVCC sample (4-byte BE length + IDR slice)
    let mut avcc_sample = Vec::with_capacity(4 + idr_slice.len());
    avcc_sample.extend_from_slice(&(idr_slice.len() as u32).to_be_bytes());
    avcc_sample.extend_from_slice(&idr_slice);

    // 6. Annex-B access unit (SPS + PPS + IDR)
    let mut annex_b = Vec::with_capacity(12 + sps.len() + pps.len() + idr_slice.len());
    annex_b.extend_from_slice(&[0, 0, 0, 1]);
    annex_b.extend_from_slice(&sps);
    annex_b.extend_from_slice(&[0, 0, 0, 1]);
    annex_b.extend_from_slice(&pps);
    annex_b.extend_from_slice(&[0, 0, 0, 1]);
    annex_b.extend_from_slice(&idr_slice);

    SyntheticH264Frame {
        width: w,
        height: h,
        sps,
        pps,
        idr_slice,
        avcc_config,
        avcc_sample,
        annex_b,
    }
}

/// Synthesizes a spec-valid ITU-T H.264 Baseline `P_SKIP` (`mb_skip_run = total_mbs`) AVCC access
/// unit for `frame_num` (`0..=15`) compatible with `build_synthetic_h264_ipcm_idr` (`avcC`).
pub fn build_synthetic_h264_p_skip_avcc_sample(width: u32, height: u32, frame_num: u32) -> Vec<u8> {
    let mb_w = (width.max(16) + 15) / 16;
    let mb_h = (height.max(16) + 15) / 16;
    let total_mbs = mb_w * mb_h;

    let mut sl_bits = Vec::with_capacity(64);
    push_ue_bits(&mut sl_bits, 0); // first_mb_in_slice = 0
    push_ue_bits(&mut sl_bits, 0); // slice_type = 0 (P slice)
    push_ue_bits(&mut sl_bits, 0); // pic_parameter_set_id = 0
    push_fixed_bits(&mut sl_bits, frame_num & 0x0F, 4); // frame_num (4 bits, matches log2_max_frame_num_minus4=0)
    sl_bits.push(0); // num_ref_idx_active_override_flag = 0
    sl_bits.push(0); // ref_pic_list_modification_flag_l0 = 0
    sl_bits.push(0); // adaptive_ref_pic_marking_mode_flag = 0 (nal_ref_idc != 0)
    push_ue_bits(&mut sl_bits, 0); // slice_qp_delta = se(0)
    push_ue_bits(&mut sl_bits, 1); // disable_deblocking_filter_idc = 1
    push_ue_bits(&mut sl_bits, total_mbs); // mb_skip_run = all MBs skipped
    sl_bits.push(1); // rbsp_stop_one_bit

    let rbsp = bits_to_bytes(&mut sl_bits);
    let mut nal = Vec::with_capacity(1 + rbsp.len() + 4);
    nal.push(0x41); // forbidden_zero_bit=0, nal_ref_idc=2, nal_unit_type=1 (non-IDR P slice)
    nal.extend_from_slice(&escape_rbsp_to_nal_payload(&rbsp));

    let mut avcc_sample = Vec::with_capacity(4 + nal.len());
    avcc_sample.extend_from_slice(&(nal.len() as u32).to_be_bytes());
    avcc_sample.extend_from_slice(&nal);
    avcc_sample
}

/// Wraps a length-prefixed AVCC access unit with leading AirPlay `AUD` (type 9) + `SEI` (type 6) NALs.
pub fn wrap_avcc_with_aud_sei(avcc_sample: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(15 + avcc_sample.len());
    out.extend_from_slice(&2u32.to_be_bytes());
    out.extend_from_slice(&[0x09, 0xF0]);
    out.extend_from_slice(&5u32.to_be_bytes());
    out.extend_from_slice(&[0x06, 0x05, 0x01, 0xAA, 0x80]);
    out.extend_from_slice(avcc_sample);
    out
}

/// Synthesizes a 100% spec-valid ITU-T H.264 Baseline CAVLC P-slice AVCC sample whose NALU payload
/// has EXACT byte length `target_nal_len` (e.g., `263, 295, 327, 359, 487, 504`), forcing the 4-byte
/// AVCC length header to match `00 00 01 xx` for real COM MFT `256..=511 B` collision testing.
pub fn build_synthetic_h264_p_exact_len_avcc_sample(
    width: u32,
    height: u32,
    frame_num: u32,
    target_nal_len: usize,
) -> Vec<u8> {
    let mb_w = (width.max(16) + 15) / 16;
    let mb_h = (height.max(16) + 15) / 16;
    let total_mbs = (mb_w * mb_h) as usize;
    let mut header_bits = Vec::with_capacity(32);
    push_ue_bits(&mut header_bits, 0); // first_mb_in_slice = 0
    push_ue_bits(&mut header_bits, 0); // slice_type = 0 (P slice)
    push_ue_bits(&mut header_bits, 0); // pic_parameter_set_id = 0
    push_fixed_bits(&mut header_bits, frame_num & 0x0F, 4); // frame_num (4 bits)
    header_bits.push(0); // num_ref_idx_active_override_flag = 0
    header_bits.push(0); // ref_pic_list_modification_flag_l0 = 0
    header_bits.push(0); // adaptive_ref_pic_marking_mode_flag = 0
    push_ue_bits(&mut header_bits, 0); // slice_qp_delta = 0
    push_ue_bits(&mut header_bits, 1); // disable_deblocking_filter_idc = 1

    let build_for_k = |p_l0_mbs: usize| -> Vec<u8> {
        let mut sl_bits = header_bits.clone();
        sl_bits.reserve(p_l0_mbs * 5 + 32);
        for _ in 0..p_l0_mbs {
            // mb_skip_run=0 ('1'), mb_type=0 P_L0_16x16 ('1'), mvd_x=0 ('1'), mvd_y=0 ('1'), cbp=0 ('1')
            sl_bits.extend_from_slice(&[1, 1, 1, 1, 1]);
        }
        let remain = total_mbs.saturating_sub(p_l0_mbs);
        if remain > 0 {
            push_ue_bits(&mut sl_bits, remain as u32);
        }
        sl_bits.push(1); // rbsp_stop_one_bit
        let rbsp = bits_to_bytes(&mut sl_bits);
        let mut nal = Vec::with_capacity(1 + rbsp.len() + 4);
        nal.push(0x41);
        nal.extend_from_slice(&escape_rbsp_to_nal_payload(&rbsp));
        nal
    };

    let base_k = ((target_nal_len.saturating_sub(6) * 8) / 5).min(total_mbs.saturating_sub(1));
    let mut chosen_nal = build_for_k(base_k);
    if chosen_nal.len() != target_nal_len {
        let lo = base_k.saturating_sub(64);
        let hi = (base_k + 64).min(total_mbs.saturating_sub(1));
        for k in lo..=hi {
            let cand = build_for_k(k);
            if cand.len() == target_nal_len {
                chosen_nal = cand;
                break;
            }
        }
    }
    assert_eq!(
        chosen_nal.len(),
        target_nal_len,
        "exact-length H.264 P-slice NALU target mismatch for {width}x{height}"
    );
    let mut avcc_sample = Vec::with_capacity(4 + chosen_nal.len());
    avcc_sample.extend_from_slice(&(chosen_nal.len() as u32).to_be_bytes());
    avcc_sample.extend_from_slice(&chosen_nal);
    avcc_sample
}

/// Synthesizes a spec-valid non-mod-16 cropped H.264 Baseline `I_PCM` IDR frame (`SPS` sets
/// `frame_cropping_flag=1` so visible dimensions equal even `(vis_w, vis_h)` such as `888x1920`
/// or `990x2160` inside macroblock-aligned coded storage). Fast byte-aligned emission for HD tests.
pub fn build_synthetic_h264_ipcm_idr_cropped(
    vis_w: u32,
    vis_h: u32,
    luma_y: u8,
    idr_pic_id: u32,
) -> SyntheticH264Frame {
    let vw = vis_w.max(2) & !1;
    let vh = vis_h.max(2) & !1;
    let mb_w = (vw.max(16) + 15) / 16;
    let mb_h = (vh.max(16) + 15) / 16;
    let coded_w = mb_w * 16;
    let coded_h = mb_h * 16;
    let crop_right_units = coded_w.saturating_sub(vw) / 2;
    let crop_bottom_units = coded_h.saturating_sub(vh) / 2;
    let has_crop = crop_right_units > 0 || crop_bottom_units > 0;

    // 1. SPS NALU (header 0x67, Baseline profile_idc=66, level=40)
    let mut sps_bits = Vec::with_capacity(160);
    push_fixed_bits(&mut sps_bits, 66, 8);
    push_fixed_bits(&mut sps_bits, 0x80, 8);
    push_fixed_bits(&mut sps_bits, 40, 8);
    push_ue_bits(&mut sps_bits, 0);
    push_ue_bits(&mut sps_bits, 0);
    push_ue_bits(&mut sps_bits, 2);
    push_ue_bits(&mut sps_bits, 1);
    sps_bits.push(0);
    push_ue_bits(&mut sps_bits, mb_w - 1);
    push_ue_bits(&mut sps_bits, mb_h - 1);
    sps_bits.push(1);
    sps_bits.push(1);
    if has_crop {
        sps_bits.push(1); // frame_cropping_flag = 1
        push_ue_bits(&mut sps_bits, 0);
        push_ue_bits(&mut sps_bits, crop_right_units);
        push_ue_bits(&mut sps_bits, 0);
        push_ue_bits(&mut sps_bits, crop_bottom_units);
    } else {
        sps_bits.push(0);
    }
    sps_bits.push(0); // vui_parameters_present_flag = 0
    sps_bits.push(1); // rbsp_stop_one_bit
    let sps_rbsp = bits_to_bytes(&mut sps_bits);
    let mut sps = Vec::with_capacity(1 + sps_rbsp.len() + 2);
    sps.push(0x67);
    sps.extend_from_slice(&escape_rbsp_to_nal_payload(&sps_rbsp));

    // 2. PPS NALU (header 0x68, CAVLC, deblocking_filter_control_present_flag = 1)
    let mut pps_bits = Vec::with_capacity(64);
    push_ue_bits(&mut pps_bits, 0);
    push_ue_bits(&mut pps_bits, 0);
    pps_bits.push(0);
    pps_bits.push(0);
    push_ue_bits(&mut pps_bits, 0);
    push_ue_bits(&mut pps_bits, 0);
    push_ue_bits(&mut pps_bits, 0);
    pps_bits.push(0);
    push_fixed_bits(&mut pps_bits, 0, 2);
    push_ue_bits(&mut pps_bits, 0);
    push_ue_bits(&mut pps_bits, 0);
    push_ue_bits(&mut pps_bits, 0);
    pps_bits.push(1);
    pps_bits.push(0);
    pps_bits.push(0);
    pps_bits.push(1);
    let pps_rbsp = bits_to_bytes(&mut pps_bits);
    let mut pps = Vec::with_capacity(1 + pps_rbsp.len() + 2);
    pps.push(0x68);
    pps.extend_from_slice(&escape_rbsp_to_nal_payload(&pps_rbsp));

    // 3. Fast byte-aligned I_PCM IDR slice emission
    let mut sl_hdr_bits = Vec::with_capacity(64);
    push_ue_bits(&mut sl_hdr_bits, 0); // first_mb_in_slice = 0
    push_ue_bits(&mut sl_hdr_bits, 7); // slice_type = 7 (I slice)
    push_ue_bits(&mut sl_hdr_bits, 0); // pic_parameter_set_id = 0
    push_fixed_bits(&mut sl_hdr_bits, 0, 4); // frame_num = 0
    push_ue_bits(&mut sl_hdr_bits, idr_pic_id); // idr_pic_id
    sl_hdr_bits.push(0); // no_output_of_prior_pics_flag = 0
    sl_hdr_bits.push(0); // long_term_reference_flag = 0
    push_ue_bits(&mut sl_hdr_bits, 0); // slice_qp_delta = 0
    push_ue_bits(&mut sl_hdr_bits, 1); // disable_deblocking_filter_idc = 1
    // First MB header (mb_type = 25) + alignment zeros right after slice header
    push_ue_bits(&mut sl_hdr_bits, 25);
    while sl_hdr_bits.len() % 8 != 0 {
        sl_hdr_bits.push(0);
    }
    let sl_prefix = bits_to_bytes(&mut sl_hdr_bits);

    let total_mbs = (mb_w * mb_h) as usize;
    let mut sl_rbsp = Vec::with_capacity(sl_prefix.len() + total_mbs * 386 + 2);
    sl_rbsp.extend_from_slice(&sl_prefix);
    sl_rbsp.resize(sl_rbsp.len() + 256, luma_y);
    sl_rbsp.resize(sl_rbsp.len() + 128, 0x80);

    // Subsequent byte-aligned I_PCM MBs: ue(25) [9 bits] + 7 zero pad bits = [0x0D, 0x00]
    let mut subsequent_mb = Vec::with_capacity(386);
    subsequent_mb.extend_from_slice(&[0x0D, 0x00]);
    subsequent_mb.resize(subsequent_mb.len() + 256, luma_y);
    subsequent_mb.resize(subsequent_mb.len() + 128, 0x80);
    for _ in 1..total_mbs {
        sl_rbsp.extend_from_slice(&subsequent_mb);
    }
    sl_rbsp.push(0x80); // rbsp_stop_one_bit (byte-aligned)

    let mut idr_slice = Vec::with_capacity(1 + sl_rbsp.len() + 32);
    idr_slice.push(0x65);
    idr_slice.extend_from_slice(&escape_rbsp_to_nal_payload(&sl_rbsp));

    // 4. ISO/IEC 14496-15 avcC record
    let mut avcc_config = Vec::with_capacity(11 + sps.len() + pps.len());
    avcc_config.push(1);
    avcc_config.push(66);
    avcc_config.push(0x80);
    avcc_config.push(40);
    avcc_config.push(0xFF);
    avcc_config.push(0xE1);
    avcc_config.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    avcc_config.extend_from_slice(&sps);
    avcc_config.push(1);
    avcc_config.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    avcc_config.extend_from_slice(&pps);

    let mut avcc_sample = Vec::with_capacity(4 + idr_slice.len());
    avcc_sample.extend_from_slice(&(idr_slice.len() as u32).to_be_bytes());
    avcc_sample.extend_from_slice(&idr_slice);

    let mut annex_b = Vec::with_capacity(12 + sps.len() + pps.len() + idr_slice.len());
    annex_b.extend_from_slice(&[0, 0, 0, 1]);
    annex_b.extend_from_slice(&sps);
    annex_b.extend_from_slice(&[0, 0, 0, 1]);
    annex_b.extend_from_slice(&pps);
    annex_b.extend_from_slice(&[0, 0, 0, 1]);
    annex_b.extend_from_slice(&idr_slice);

    SyntheticH264Frame {
        width: vw,
        height: vh,
        sps,
        pps,
        idr_slice,
        avcc_config,
        avcc_sample,
        annex_b,
    }
}

/// Synthesizes a cropped non-mod-16 `P_SKIP` AVCC access unit preceded by `AUD` (type 9) + `SEI`
/// (type 6) NALUs matching AirPlay framing.
pub fn build_synthetic_h264_p_skip_cropped_with_aud_sei(
    vis_w: u32,
    vis_h: u32,
    frame_num: u32,
) -> Vec<u8> {
    let p_au = build_synthetic_h264_p_skip_avcc_sample(vis_w, vis_h, frame_num);
    wrap_avcc_with_aud_sei(&p_au)
}

/// Statistics returned by `LiveMfVideoDecoder::feed_access_unit` / `feed_avcc_sample`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiveDecodeStats {
    pub frames_emitted: u32,
    pub total_decoded_frames: u64,
    pub stream_changes: u32,
    pub coded_width: u32,
    pub coded_height: u32,
    pub visible_width: u32,
    pub visible_height: u32,
    pub hardware_d3d11: bool,
    pub zero_copy_gpu_frames: u64,
    pub software_upload_frames: u64,
}

// Low-level COM vtable helpers for Media Foundation interfaces (verified against Windows SDK x64)
unsafe fn mf_attr_get_u32(obj: &ComPtr, key: &Guid) -> Result<u32, i32> {
    let vt = obj.vtable();
    let f: unsafe extern "system" fn(*mut c_void, *const Guid, *mut u32) -> i32 =
        std::mem::transmute(*vt.add(7));
    let mut v = 0u32;
    let hr = f(obj.as_ptr(), key, &mut v);
    if hr >= 0 { Ok(v) } else { Err(hr) }
}

unsafe fn mf_attr_get_u64(obj: &ComPtr, key: &Guid) -> Result<u64, i32> {
    let vt = obj.vtable();
    let f: unsafe extern "system" fn(*mut c_void, *const Guid, *mut u64) -> i32 =
        std::mem::transmute(*vt.add(8));
    let mut v = 0u64;
    let hr = f(obj.as_ptr(), key, &mut v);
    if hr >= 0 { Ok(v) } else { Err(hr) }
}

unsafe fn mf_attr_get_guid(obj: &ComPtr, key: &Guid) -> Result<Guid, i32> {
    let vt = obj.vtable();
    let f: unsafe extern "system" fn(*mut c_void, *const Guid, *mut Guid) -> i32 =
        std::mem::transmute(*vt.add(10));
    let mut g = Guid {
        data1: 0,
        data2: 0,
        data3: 0,
        data4: [0; 8],
    };
    let hr = f(obj.as_ptr(), key, &mut g);
    if hr >= 0 { Ok(g) } else { Err(hr) }
}

unsafe fn mf_attr_get_blob(obj: &ComPtr, key: &Guid, buf: &mut [u8]) -> Result<u32, i32> {
    let vt = obj.vtable();
    let f: unsafe extern "system" fn(*mut c_void, *const Guid, *mut u8, u32, *mut u32) -> i32 =
        std::mem::transmute(*vt.add(15));
    let mut got = 0u32;
    let hr = f(obj.as_ptr(), key, buf.as_mut_ptr(), buf.len() as u32, &mut got);
    if hr >= 0 { Ok(got) } else { Err(hr) }
}

unsafe fn mf_attr_set_u32(obj: &ComPtr, key: &Guid, val: u32) -> Result<(), i32> {
    let vt = obj.vtable();
    let f: unsafe extern "system" fn(*mut c_void, *const Guid, u32) -> i32 =
        std::mem::transmute(*vt.add(21));
    let hr = f(obj.as_ptr(), key, val);
    if hr >= 0 { Ok(()) } else { Err(hr) }
}

unsafe fn mf_attr_set_u64(obj: &ComPtr, key: &Guid, val: u64) -> Result<(), i32> {
    let vt = obj.vtable();
    let f: unsafe extern "system" fn(*mut c_void, *const Guid, u64) -> i32 =
        std::mem::transmute(*vt.add(22));
    let hr = f(obj.as_ptr(), key, val);
    if hr >= 0 { Ok(()) } else { Err(hr) }
}

unsafe fn mf_attr_set_guid(obj: &ComPtr, key: &Guid, val: &Guid) -> Result<(), i32> {
    let vt = obj.vtable();
    let f: unsafe extern "system" fn(*mut c_void, *const Guid, *const Guid) -> i32 =
        std::mem::transmute(*vt.add(24));
    let hr = f(obj.as_ptr(), key, val);
    if hr >= 0 { Ok(()) } else { Err(hr) }
}

pub const MIN_PTS_DELTA_100NS: i64 = 10_000;
pub const MAX_PTS_DELTA_100NS: i64 = 1_000_000;
pub const DEFAULT_PTS_DELTA_100NS: i64 = 166_667;

/// Clamps per-frame PTS deltas into `[10_000..=1_000_000]` 100ns (`1..100 ms`), falling back to
/// `166_667` (`~16.67 ms`) when non-monotonic or duplicate so static screen pauses/bursts never
/// stall `CMSH264DecoderMFT`.
pub fn clamp_monotonic_pts_delta_100ns(last_pts_100ns: i64, candidate_100ns: i64) -> i64 {
    if last_pts_100ns < 0 {
        return candidate_100ns.max(0);
    }
    let raw_delta = candidate_100ns.saturating_sub(last_pts_100ns);
    if raw_delta <= 0 {
        last_pts_100ns.saturating_add(DEFAULT_PTS_DELTA_100NS)
    } else {
        last_pts_100ns.saturating_add(raw_delta.clamp(MIN_PTS_DELTA_100NS, MAX_PTS_DELTA_100NS))
    }
}

/// Computes `0`-based monotonic sample timestamps (`0, 166_667, 333_334, ...` 100ns units)
/// exclusively for AirPlay `VideoCodecKind::H264` so MFT never sees epoch jumps or stale wall-clock PTS.
pub fn next_h264_zero_based_pts_100ns(last_pts_100ns: i64) -> i64 {
    if last_pts_100ns < 0 {
        0
    } else {
        last_pts_100ns.saturating_add(FRAME_DURATION_100NS)
    }
}

/// Real COM Windows Media Foundation video decoder (`IMFTransform`) supporting BOTH
/// CoreDevice **HEVC (`MFVideoFormat_HEVC`)** and AirPlay **H.264 (`MFVideoFormat_H264`)**
/// with zero-copy `IMFDXGIBuffer` → VRAM `CopySubresourceRegion` and automatic `IMF2DBuffer::Lock2D` software fallback.
pub struct LiveMfVideoDecoder {
    gpu: Arc<LiveD3d11Gpu>,
    codec: VideoCodecKind,
    mft: Option<ComPtr>,
    upload_tex: Option<(ComPtr, u32, u32)>,
    avcc_cfg: Option<AvccConfig>,
    last_raw_avcc: Option<Vec<u8>>,
    hardware_d3d11: bool,
    d3d11_multithread_protected: bool,
    mf_low_latency_enabled: bool,
    provides_samples: bool,
    out_buf_size: u32,
    coded_width: u32,
    coded_height: u32,
    default_stride: i32,
    visible_rect: Rect,
    last_mft_aperture: Option<Rect>,
    expected_trim: (u32, u32, u32, u32),
    color_info: VideoColorInfo,
    vui_color_info: Option<VideoColorInfo>,
    expected_coded_dims: Option<(u32, u32)>,
    total_decoded_frames: u64,
    stream_changes: u32,
    zero_copy_gpu_frames: u64,
    software_upload_frames: u64,
    not_accepting_drains: u64,
    stale_dim_dropped: u64,
    last_hresult: i32,
    last_input_pts_100ns: i64,
    consecutive_input_errors: u32,
    dpb_preserved_flushes_skipped: u64,
    idr_keyframes_detected: u64,
    avcc_small_256_511_frames: u64,
    mft_reconfigs: u64,
    awaiting_idr: bool,
    awaiting_idr_empty_dpb: bool,
    dropped_pre_idr_frames: u64,
    force_prepend_next_sps_pps: bool,
    keep_cpu_nv12_backup: bool,
    hevc_sw_fallback_attempted: bool,
    mf_started: bool,
    h_mfplat: *mut c_void,
    fn_create_media_type: FnMFCreateMediaType,
    fn_create_sample: FnMFCreateSample,
    fn_create_memory_buffer: FnMFCreateMemoryBuffer,
    fn_mft_enum_ex: FnMFTEnumExLocal,
    fn_mf_shutdown: FnMFShutdown,
}

unsafe impl Send for LiveMfVideoDecoder {}

/// Evaluates whether a `VideoCodecKind::Hevc` D3D11 DXVA failure before frame #1
/// should retry once using CPU software MFT (`allow_d3d11 = false`). Never alters `H264`.
pub fn should_retry_hevc_software_mft_fallback(
    codec: VideoCodecKind,
    hardware_d3d11: bool,
    total_decoded_frames: u64,
    already_retried: bool,
) -> bool {
    codec == VideoCodecKind::Hevc && hardware_d3d11 && total_decoded_frames == 0 && !already_retried
}

impl LiveMfVideoDecoder {
    /// Activates a real Windows Media Foundation decoder MFT (`HEVC` or `H.264`) bound to `gpu`.
    /// Delay-loads `mfplat.dll` safely via `LoadLibraryW` (zero crash on Windows N/KN editions).
    pub fn new(gpu: Arc<LiveD3d11Gpu>, codec: VideoCodecKind) -> Result<Self, DecoderError> {
        Self::new_with_d3d11_option(gpu, codec, true)
    }

    /// Activates a real Windows Media Foundation decoder MFT with configurable `allow_d3d11`
    /// so `VideoCodecKind::Hevc` can safely fall back to CPU software MFT on older GPUs.
    pub fn new_with_d3d11_option(
        gpu: Arc<LiveD3d11Gpu>,
        codec: VideoCodecKind,
        allow_d3d11: bool,
    ) -> Result<Self, DecoderError> {
        let _ = ensure_com_mta_initialized();
        let mf_w = to_wide("mfplat.dll");
        let h_mfplat = unsafe { LoadLibraryW(mf_w.as_ptr()) };
        if h_mfplat.is_null() {
            return Err(DecoderError::MediaFoundationUnavailable(
                "mfplat.dll not found (Windows N/KN edition without Media Feature Pack)".to_string(),
            ));
        }

        let resolve = |sym: &[u8]| -> Result<*const c_void, DecoderError> {
            let p = unsafe { GetProcAddress(h_mfplat, sym.as_ptr()) };
            if p.is_null() {
                Err(DecoderError::MediaFoundationUnavailable(format!(
                    "missing symbol {}",
                    String::from_utf8_lossy(&sym[..sym.len().saturating_sub(1)])
                )))
            } else {
                Ok(p)
            }
        };

        let syms = (|| -> Result<_, DecoderError> {
            let p_startup = resolve(b"MFStartup\0")?;
            let p_shutdown = resolve(b"MFShutdown\0")?;
            let p_enum = resolve(b"MFTEnumEx\0")?;
            let p_mt = resolve(b"MFCreateMediaType\0")?;
            let p_sample = resolve(b"MFCreateSample\0")?;
            let p_buf = resolve(b"MFCreateMemoryBuffer\0")?;
            Ok((p_startup, p_shutdown, p_enum, p_mt, p_sample, p_buf))
        })();

        let (p_startup, p_shutdown, p_enum, p_mt, p_sample, p_buf) = match syms {
            Ok(v) => v,
            Err(e) => {
                unsafe { FreeLibrary(h_mfplat) };
                return Err(e);
            }
        };

        let fn_startup: FnMFStartup = unsafe { std::mem::transmute(p_startup) };
        let fn_mf_shutdown: FnMFShutdown = unsafe { std::mem::transmute(p_shutdown) };
        let fn_mft_enum_ex: FnMFTEnumExLocal = unsafe { std::mem::transmute(p_enum) };
        let fn_create_media_type: FnMFCreateMediaType = unsafe { std::mem::transmute(p_mt) };
        let fn_create_sample: FnMFCreateSample = unsafe { std::mem::transmute(p_sample) };
        let fn_create_memory_buffer: FnMFCreateMemoryBuffer = unsafe { std::mem::transmute(p_buf) };

        let hr_start = unsafe { fn_startup(MF_VERSION, MFSTARTUP_NOSOCKET) };
        if hr_start < 0 {
            unsafe { FreeLibrary(h_mfplat) };
            return Err(DecoderError::ComError {
                op: "MFStartup",
                hr: hr_start,
            });
        }

        let keep_cpu_nv12_backup = !gpu.has_video_processor();
        let d3d11_multithread_protected = ensure_d3d11_multithread_protected(&gpu);
        let mut dec = Self {
            gpu,
            codec,
            mft: None,
            upload_tex: None,
            avcc_cfg: None,
            last_raw_avcc: None,
            hardware_d3d11: false,
            d3d11_multithread_protected,
            mf_low_latency_enabled: false,
            provides_samples: false,
            out_buf_size: 0,
            coded_width: 0,
            coded_height: 0,
            default_stride: 0,
            visible_rect: Rect::default(),
            last_mft_aperture: None,
            expected_trim: (0, 0, 0, 0),
            color_info: VideoColorInfo::default(),
            vui_color_info: None,
            expected_coded_dims: None,
            total_decoded_frames: 0,
            stream_changes: 0,
            zero_copy_gpu_frames: 0,
            software_upload_frames: 0,
            not_accepting_drains: 0,
            stale_dim_dropped: 0,
            last_hresult: 0,
            last_input_pts_100ns: -1,
            consecutive_input_errors: 0,
            dpb_preserved_flushes_skipped: 0,
            idr_keyframes_detected: 0,
            avcc_small_256_511_frames: 0,
            mft_reconfigs: 0,
            awaiting_idr: false,
            awaiting_idr_empty_dpb: false,
            dropped_pre_idr_frames: 0,
            force_prepend_next_sps_pps: false,
            keep_cpu_nv12_backup,
            hevc_sw_fallback_attempted: !allow_d3d11,
            mf_started: true,
            h_mfplat,
            fn_create_media_type,
            fn_create_sample,
            fn_create_memory_buffer,
            fn_mft_enum_ex,
            fn_mf_shutdown,
        };

        if let Err(err) = dec.activate_mft_with_options(1920, 1080, allow_d3d11) {
            if codec == VideoCodecKind::Hevc && allow_d3d11 && !dec.hevc_sw_fallback_attempted {
                dec.hevc_sw_fallback_attempted = true;
                dec.activate_mft_with_options(1920, 1080, false)?;
                return Ok(dec);
            }
            return Err(err);
        }
        Ok(dec)
    }

    /// Deterministically flushes and releases the active COM `IMFTransform` & staging upload texture
    /// so repeated MFT re-creations across long sessions never accumulate COM or GPU handles.
    pub fn clean_teardown(&mut self) {
        if self.mft.is_some() {
            let _ = self.send_mft_message(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
            let _ = self.send_mft_message(MFT_MESSAGE_COMMAND_FLUSH, 0);
            let _ = self.send_mft_message(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
        }
        self.upload_tex = None;
        self.mft = None;
        self.hardware_d3d11 = false;
        self.mf_low_latency_enabled = false;
        self.provides_samples = false;
        self.out_buf_size = 0;
        self.default_stride = 0;
        self.last_mft_aperture = None;
        self.last_input_pts_100ns = -1;
        self.consecutive_input_errors = 0;
    }

    fn activate_mft(&mut self, init_w: u32, init_h: u32) -> Result<(), DecoderError> {
        self.activate_mft_with_options(init_w, init_h, true)
    }

    fn activate_mft_with_options(
        &mut self,
        init_w: u32,
        init_h: u32,
        allow_d3d11: bool,
    ) -> Result<(), DecoderError> {
        self.clean_teardown();

        let input_info = MftRegisterTypeInfo {
            guid_major_type: MF_MEDIA_TYPE_VIDEO,
            guid_subtype: self.codec.mf_subtype(),
        };
        let flags = MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_LOCALMFT | MFT_ENUM_FLAG_SORTANDFILTER;
        let mut pp_activates: *mut *mut c_void = std::ptr::null_mut();
        let mut count = 0u32;

        let hr = unsafe {
            (self.fn_mft_enum_ex)(
                MFT_CATEGORY_VIDEO_DECODER,
                flags,
                &input_info,
                std::ptr::null(),
                &mut pp_activates,
                &mut count,
            )
        };
        if hr < 0 {
            if !pp_activates.is_null() {
                unsafe { CoTaskMemFree(pp_activates as *mut c_void) };
            }
            return Err(DecoderError::ComError { op: "MFTEnumEx", hr });
        }
        if pp_activates.is_null() || count == 0 {
            if !pp_activates.is_null() {
                unsafe { CoTaskMemFree(pp_activates as *mut c_void) };
            }
            return Err(match self.codec {
                VideoCodecKind::Hevc => DecoderError::HevcExtensionMissing,
                VideoCodecKind::H264 => DecoderError::CodecNotFound(VideoCodecKind::H264),
            });
        }

        let mut activated: Option<ComPtr> = None;
        unsafe {
            let slice = std::slice::from_raw_parts_mut(pp_activates, count as usize);
            for raw_act in slice.iter_mut() {
                if let Some(act) = ComPtr::from_raw(*raw_act) {
                    *raw_act = std::ptr::null_mut();
                    if activated.is_none() {
                        let vt = act.vtable();
                        let activate_obj: unsafe extern "system" fn(
                            *mut c_void,
                            *const Guid,
                            *mut *mut c_void,
                        ) -> i32 = std::mem::transmute(*vt.add(33));
                        let mut p_mft: *mut c_void = std::ptr::null_mut();
                        if activate_obj(act.as_ptr(), &IID_IMF_TRANSFORM, &mut p_mft) >= 0 {
                            activated = ComPtr::from_raw(p_mft);
                        }
                    }
                }
            }
            CoTaskMemFree(pp_activates as *mut c_void);
        }

        let mft = activated.ok_or(match self.codec {
            VideoCodecKind::Hevc => DecoderError::HevcExtensionMissing,
            VideoCodecKind::H264 => DecoderError::CodecNotFound(VideoCodecKind::H264),
        })?;

        // Configure MF_LOW_LATENCY = 1 + ICodecAPI::CODECAPI_AVLowLatencyMode (VT_BOOL=VARIANT_TRUE
        // with VT_UI4 fallback) and bind IMFDXGIDeviceManager if MFT is D3D11-aware BEFORE SetInputType/BEGIN_STREAMING.
        self.last_input_pts_100ns = -1;
        let mut low_latency_attr_ok = false;
        let mut low_latency_codec_api_ok = false;
        unsafe {
            let vt = mft.vtable();
            let get_attrs: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32 =
                std::mem::transmute(*vt.add(8));
            let mut p_attrs: *mut c_void = std::ptr::null_mut();
            if get_attrs(mft.as_ptr(), &mut p_attrs) >= 0 {
                if let Some(attrs) = ComPtr::from_raw(p_attrs) {
                    if mf_attr_set_u32(&attrs, &MF_LOW_LATENCY, 1).is_ok() {
                        low_latency_attr_ok = true;
                    }
                    let d3d11_aware = mf_attr_get_u32(&attrs, &MF_SA_D3D11_AWARE).unwrap_or(0);
                    if allow_d3d11 && d3d11_aware != 0 {
                        if let Some(mgr) = self.gpu.dxgi_manager() {
                            let proc_msg: unsafe extern "system" fn(*mut c_void, u32, usize) -> i32 =
                                std::mem::transmute(*vt.add(23));
                            if proc_msg(
                                mft.as_ptr(),
                                MFT_MESSAGE_SET_D3D_MANAGER,
                                mgr.as_ptr() as usize,
                            ) >= 0
                            {
                                self.hardware_d3d11 = true;
                            }
                        }
                    }
                }
            }
            if let Ok(codec_api) = mft.query_interface(&IID_ICODEC_API) {
                let cvt = codec_api.vtable();
                // IUnknown (0..2), IsSupported (3), IsModifiable (4), GetParameterRange (5),
                // GetParameterValues (6), GetDefaultValue (7), GetValue (8), SetValue (9).
                let set_val: unsafe extern "system" fn(
                    *mut c_void,
                    *const Guid,
                    *const c_void,
                ) -> i32 = std::mem::transmute(*cvt.add(9));
                let var_bool = VariantBool::variant_true();
                let hr_bool = set_val(
                    codec_api.as_ptr(),
                    &CODECAPI_AV_LOW_LATENCY_MODE,
                    (&var_bool as *const VariantBool).cast::<c_void>(),
                );
                if hr_bool >= 0 {
                    low_latency_codec_api_ok = true;
                } else {
                    let var_ul4 = VariantUl4 {
                        vt: VT_UI4,
                        w_reserved1: 0,
                        w_reserved2: 0,
                        w_reserved3: 0,
                        ul_val: 1,
                        _pad: 0,
                        _tail: 0,
                    };
                    let hr_ul4 = set_val(
                        codec_api.as_ptr(),
                        &CODECAPI_AV_LOW_LATENCY_MODE,
                        (&var_ul4 as *const VariantUl4).cast::<c_void>(),
                    );
                    if hr_ul4 >= 0 {
                        low_latency_codec_api_ok = true;
                    }
                }
            }
        }
        self.mf_low_latency_enabled = low_latency_attr_ok || low_latency_codec_api_ok;

        self.mft = Some(mft);
        self.set_input_media_type(init_w.max(64), init_h.max(64))?;
        // Microsoft HEVC MFT may not expose output types until the first IDR triggers STREAM_CHANGE.
        let _ = self.pick_nv12_output_type();
        self.send_mft_message(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
        self.send_mft_message(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
        Ok(())
    }

    fn set_input_media_type(&mut self, w: u32, h: u32) -> Result<(), DecoderError> {
        let mft = self
            .mft
            .as_ref()
            .ok_or_else(|| DecoderError::InvalidConfig("MFT not initialized".to_string()))?;
        unsafe {
            let mut p_mt: *mut c_void = std::ptr::null_mut();
            let hr_mt = (self.fn_create_media_type)(&mut p_mt);
            if hr_mt < 0 || p_mt.is_null() {
                return Err(DecoderError::ComError {
                    op: "MFCreateMediaType",
                    hr: hr_mt,
                });
            }
            let mt = ComPtr::from_raw(p_mt).ok_or(DecoderError::ComError {
                op: "MFCreateMediaType",
                hr: -1,
            })?;
            mf_attr_set_guid(&mt, &MF_MT_MAJOR_TYPE, &MF_MEDIA_TYPE_VIDEO).map_err(|hr| {
                DecoderError::ComError {
                    op: "SetGUID(MAJOR_TYPE)",
                    hr,
                }
            })?;
            mf_attr_set_guid(&mt, &MF_MT_SUBTYPE, &self.codec.mf_subtype()).map_err(|hr| {
                DecoderError::ComError {
                    op: "SetGUID(SUBTYPE)",
                    hr,
                }
            })?;
            let packed_size = ((w as u64) << 32) | (h as u64);
            let _ = mf_attr_set_u64(&mt, &MF_MT_FRAME_SIZE, packed_size);
            let _ = mf_attr_set_u32(&mt, &MF_MT_INTERLACE_MODE, MF_VIDEO_INTERLACE_PROGRESSIVE);

            let vt = mft.vtable();
            let set_in: unsafe extern "system" fn(*mut c_void, u32, *mut c_void, u32) -> i32 =
                std::mem::transmute(*vt.add(15));
            let hr = set_in(mft.as_ptr(), 0, mt.as_ptr(), 0);
            if hr < 0 {
                return Err(DecoderError::ComError {
                    op: "IMFTransform::SetInputType",
                    hr,
                });
            }
        }
        Ok(())
    }

    fn send_mft_message(&self, msg: u32, param: usize) -> Result<(), DecoderError> {
        let mft = self
            .mft
            .as_ref()
            .ok_or_else(|| DecoderError::InvalidConfig("MFT not initialized".to_string()))?;
        let hr = unsafe {
            let vt = mft.vtable();
            let proc_msg: unsafe extern "system" fn(*mut c_void, u32, usize) -> i32 =
                std::mem::transmute(*vt.add(23));
            proc_msg(mft.as_ptr(), msg, param)
        };
        if hr < 0 {
            Err(DecoderError::ComError {
                op: "IMFTransform::ProcessMessage",
                hr,
            })
        } else {
            Ok(())
        }
    }

    fn pick_nv12_output_type(&mut self) -> Result<(), DecoderError> {
        let mft = self
            .mft
            .as_ref()
            .ok_or_else(|| DecoderError::InvalidConfig("MFT not initialized".to_string()))?;
        unsafe {
            let vt = mft.vtable();
            let get_out_avail: unsafe extern "system" fn(
                *mut c_void,
                u32,
                u32,
                *mut *mut c_void,
            ) -> i32 = std::mem::transmute(*vt.add(14));
            let set_out: unsafe extern "system" fn(*mut c_void, u32, *mut c_void, u32) -> i32 =
                std::mem::transmute(*vt.add(16));
            let get_out_info: unsafe extern "system" fn(
                *mut c_void,
                u32,
                *mut MftOutputStreamInfo,
            ) -> i32 = std::mem::transmute(*vt.add(7));

            let mut idx = 0u32;
            let mut last_set_err: Option<i32> = None;
            loop {
                let mut p_mt: *mut c_void = std::ptr::null_mut();
                let hr = get_out_avail(mft.as_ptr(), 0, idx, &mut p_mt);
                idx = idx.saturating_add(1);
                if hr == MF_E_NO_MORE_TYPES || hr < 0 || p_mt.is_null() {
                    if let Some(set_hr) = last_set_err {
                        return Err(DecoderError::ComError {
                            op: "SetOutputType(NV12)",
                            hr: set_hr,
                        });
                    }
                    return Err(DecoderError::ComError {
                        op: "GetOutputAvailableType(NV12)",
                        hr,
                    });
                }
                let Some(mt) = ComPtr::from_raw(p_mt) else {
                    continue;
                };
                if mf_attr_get_guid(&mt, &MF_MT_SUBTYPE).ok() != Some(MF_VIDEO_FORMAT_NV12) {
                    continue;
                }
                // Keep MFT's enumerated NV12 IMFMediaType completely unmodified before SetOutputType.
                let hr_set = set_out(mft.as_ptr(), 0, mt.as_ptr(), 0);
                if hr_set < 0 {
                    last_set_err = Some(hr_set);
                    continue;
                }
                let packed = mf_attr_get_u64(&mt, &MF_MT_FRAME_SIZE).unwrap_or(0);
                let w = (packed >> 32) as u32;
                let h = (packed & 0xFFFF_FFFF) as u32;
                if w > 0 && h > 0 {
                    self.coded_width = w;
                    self.coded_height = h;
                    self.visible_rect = Rect {
                        left: 0,
                        top: 0,
                        right: w as i32,
                        bottom: h as i32,
                    };
                }
                let raw_stride = mf_attr_get_u32(&mt, &MF_MT_DEFAULT_STRIDE).unwrap_or(w.max(2));
                let stride_i32 = raw_stride as i32;
                self.default_stride = if stride_i32 == 0 {
                    w.max(2) as i32
                } else {
                    stride_i32
                };

                // Read MF_MT_MINIMUM_DISPLAY_APERTURE, then MF_MT_GEOMETRIC_APERTURE, then MF_MT_PAN_SCAN_APERTURE.
                let mut parsed_aperture: Option<Rect> = None;
                for ap_guid in [
                    &MF_MT_MINIMUM_DISPLAY_APERTURE,
                    &MF_MT_GEOMETRIC_APERTURE,
                    &MF_MT_PAN_SCAN_APERTURE,
                ] {
                    let mut area_bytes = [0u8; std::mem::size_of::<MfVideoArea>()];
                    if let Ok(got) = mf_attr_get_blob(&mt, ap_guid, &mut area_bytes) {
                        if got as usize >= std::mem::size_of::<MfVideoArea>() {
                            let area: MfVideoArea =
                                std::ptr::read_unaligned(area_bytes.as_ptr() as *const MfVideoArea);
                            if area.cx > 0 && area.cy > 0 {
                                let ox = i32::from(area.offset_x.value);
                                let oy = i32::from(area.offset_y.value);
                                let cand = Rect {
                                    left: ox,
                                    top: oy,
                                    right: ox + area.cx,
                                    bottom: oy + area.cy,
                                };
                                if cand.left >= 0
                                    && cand.top >= 0
                                    && (cand.right as u32) <= w
                                    && (cand.bottom as u32) <= h
                                    && (cand.left > 0
                                        || cand.top > 0
                                        || (cand.right as u32) < w
                                        || (cand.bottom as u32) < h)
                                {
                                    parsed_aperture = Some(cand);
                                    break;
                                }
                            }
                        }
                    }
                }
                self.last_mft_aperture = parsed_aperture;
                let (reconciled_vis, _) = resolve_reconciled_aperture_rect(
                    w,
                    h,
                    self.last_mft_aperture,
                    self.codec,
                    self.expected_trim,
                    self.expected_coded_dims,
                    false,
                );
                self.visible_rect = reconciled_vis;

                let mut info = MftOutputStreamInfo::default();
                let hr_info = get_out_info(mft.as_ptr(), 0, &mut info);
                if hr_info >= 0 {
                    self.provides_samples =
                        (info.dw_flags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES) != 0;
                    self.out_buf_size = info.cb_size;
                }
                if self.codec == VideoCodecKind::H264 {
                    // AirPlay H.264: lock deterministic BT.709 color so fluctuating MFT stream-change
                    // attributes never crush dark scenes or shift tint across mid-stream events.
                    self.color_info = resolve_deterministic_bt709_color(self.vui_color_info);
                } else {
                    let mf_matrix = mf_attr_get_u32(&mt, &MF_MT_YUV_MATRIX).ok();
                    let mf_range = mf_attr_get_u32(&mt, &MF_MT_VIDEO_NOMINAL_RANGE).ok();
                    self.color_info = VideoColorInfo::from_mf_attributes(
                        mf_matrix,
                        mf_range,
                        self.vui_color_info.unwrap_or_default(),
                    );
                }
                return Ok(());
            }
        }
    }

    /// Flushes buffered reference pictures inside the MFT (`MFT_MESSAGE_COMMAND_FLUSH`).
    pub fn flush(&self) {
        let _ = self.send_mft_message(MFT_MESSAGE_COMMAND_FLUSH, 0);
    }

    fn discard_buffered_outputs_before_switch(&mut self) {
        let Some(ref mft) = self.mft else {
            return;
        };
        for _ in 0..32 {
            let _owned_out = if self.provides_samples {
                None
            } else {
                self.create_output_sample().ok()
            };
            if !self.provides_samples && _owned_out.is_none() {
                break;
            }
            let mut out_buf = MftOutputDataBuffer {
                dw_stream_id: 0,
                p_sample: _owned_out
                    .as_ref()
                    .map_or(std::ptr::null_mut(), |s| s.as_ptr()),
                dw_status: 0,
                p_events: std::ptr::null_mut(),
            };
            let mut status = 0u32;
            let hr = unsafe {
                let vt = mft.vtable();
                let proc_out: unsafe extern "system" fn(
                    *mut c_void,
                    u32,
                    u32,
                    *mut MftOutputDataBuffer,
                    *mut u32,
                ) -> i32 = std::mem::transmute(*vt.add(25));
                proc_out(mft.as_ptr(), 0, 1, &mut out_buf, &mut status)
            };
            if !out_buf.p_events.is_null() {
                let _ev_guard = unsafe { ComPtr::from_raw(out_buf.p_events) };
            }
            if self.provides_samples && !out_buf.p_sample.is_null() {
                let _samp_guard = unsafe { ComPtr::from_raw(out_buf.p_sample) };
            }
            if hr == MF_E_TRANSFORM_NEED_MORE_INPUT || hr == MF_E_TRANSFORM_STREAM_CHANGE || hr < 0 {
                break;
            }
        }
    }

    /// Forces SPS/PPS Annex-B prepending on the very next AirPlay frame while clearing `awaiting_idr`.
    pub fn force_next_sps_pps_prepend(&mut self) {
        self.force_prepend_next_sps_pps = true;
        self.awaiting_idr = false;
        self.awaiting_idr_empty_dpb = false;
    }

    /// Arms strict empty-DPB IDR/SEI-Recovery/I-slice waiting on a freshly created MFT so
    /// orphan mid-GOP P-frames are never decoded against an empty reference picture buffer.
    pub fn arm_empty_dpb_idr_wait(&mut self) {
        self.awaiting_idr = true;
        self.awaiting_idr_empty_dpb = true;
        self.force_prepend_next_sps_pps = true;
    }

    /// Records a non-`MF_E_NOTACCEPTING` `ProcessInput` failure while preserving healthy MFT DPB
    /// for sporadic corrupt/short delta packets (`< 5` consecutive errors). Returns `true` only
    /// once `>= H264_MAX_CONSECUTIVE_INPUT_ERRORS_BEFORE_FLUSH` (`5`) consecutive failures warrant
    /// an MFT `COMMAND_FLUSH` + empty-DPB IDR reset.
    pub fn record_nonfatal_input_error_preserving_dpb(&mut self, hr: i32) -> bool {
        self.last_hresult = hr;
        self.consecutive_input_errors = self.consecutive_input_errors.saturating_add(1);
        if self.consecutive_input_errors >= H264_MAX_CONSECUTIVE_INPUT_ERRORS_BEFORE_FLUSH {
            true
        } else {
            self.dpb_preserved_flushes_skipped = self.dpb_preserved_flushes_skipped.saturating_add(1);
            false
        }
    }

    fn reconfigure_for_sps_change(
        &mut self,
        dims_changed: bool,
        new_w: u32,
        new_h: u32,
    ) -> Result<(), DecoderError> {
        let _ = self.send_mft_message(MFT_MESSAGE_COMMAND_DRAIN, 0);
        self.discard_buffered_outputs_before_switch();
        self.flush();
        if dims_changed {
            // Full COM MFT re-instantiate on genuine pkt_k1 dimension changes prevents silent MFT stalls.
            self.clean_teardown();
            self.activate_mft(new_w.max(64), new_h.max(64))?;
            self.stream_changes = self.stream_changes.saturating_add(1);
            self.mft_reconfigs = self.mft_reconfigs.saturating_add(1);
            self.force_prepend_next_sps_pps = true;
            self.awaiting_idr_empty_dpb = true;
        } else {
            self.awaiting_idr_empty_dpb = false;
        }
        // Strictly wait for genuine IDR / SEI Recovery / Open-GOP I-slice (zero empty-DPB force-unlatch).
        self.awaiting_idr = true;
        self.consecutive_input_errors = 0;
        Ok(())
    }

    /// Updates AirPlay H.264 `avcC` parameter sets. When coded SPS dimensions change mid-stream
    /// (`old_dims != new_dims`, e.g. Portrait <-> Landscape), drains & flushes MFT and latches
    /// `awaiting_idr = true`; when coded dimensions stay identical (`!had_prior` or same-dim resend),
    /// refreshes cached `SPS/PPS` silently without flushing or latching `awaiting_idr`.
    pub fn update_avcc(&mut self, avcc: &[u8]) -> Result<bool, DecoderError> {
        if self.avcc_cfg.is_some() && self.last_raw_avcc.as_deref() == Some(avcc) {
            return Ok(false);
        }
        let parsed = parse_avcc_sps_pps(avcc)?;
        let had_prior = self.avcc_cfg.is_some();
        let old_dims = self
            .avcc_cfg
            .as_ref()
            .and_then(|old| old.coded_dimensions.map(|(w, h, _)| (w, h)));
        let new_dims = parsed.coded_dimensions.map(|(w, h, _)| (w, h));
        let changed = self
            .avcc_cfg
            .as_ref()
            .map(|old| old.sps_list != parsed.sps_list || old.pps_list != parsed.pps_list)
            .unwrap_or(true);
        if changed {
            if let Some((_, _, trim)) = parsed.coded_dimensions {
                self.expected_trim = trim;
            }
            self.vui_color_info = parsed
                .sps_list
                .first()
                .and_then(|sps| parse_h264_sps_vui_color(sps));
            self.color_info = resolve_deterministic_bt709_color(self.vui_color_info);
            let next_expected_vis = coded_tuple_to_visible(parsed.coded_dimensions);
            let prev_expected_vis = self.expected_coded_dims;
            self.avcc_cfg = Some(parsed);
            if !had_prior {
                self.awaiting_idr = false;
                self.awaiting_idr_empty_dpb = false;
            } else {
                let dims_changed = match (prev_expected_vis, next_expected_vis) {
                    (Some(o), Some(n)) => o != n,
                    _ => match (old_dims, new_dims) {
                        (Some(o), Some(n)) => o != n,
                        _ => false,
                    },
                };
                if dims_changed {
                    let (nw, nh) = new_dims.unwrap_or((1920, 1080));
                    self.reconfigure_for_sps_change(true, nw, nh)?;
                }
            }
            if let Some(vis_dims) = next_expected_vis {
                self.expected_coded_dims = Some(vis_dims);
            }
        }
        self.last_raw_avcc = Some(avcc.to_vec());
        Ok(changed)
    }

    /// Converts an AirPlay strictly length-prefixed AVCC access unit to Annex-B (prepending SPS/PPS on IDR
    /// and refreshing valid inline SPS/PPS NALUs without triggering MFT rebuilds from P-frame noise).
    pub fn feed_avcc_sample(
        &mut self,
        avcc_sample: &[u8],
        pts_us: u64,
        is_keyframe: bool,
        ring: &D3d11Nv12VramRing,
    ) -> Result<LiveDecodeStats, DecoderError> {
        let (
            effective_keyframe,
            params_changed,
            mut annex_b,
            has_small_256_511,
            new_trim,
            new_vui,
            new_vis_dims,
            open_gop_resume,
        ) = {
            let cfg = self.avcc_cfg.as_mut().ok_or_else(|| {
                DecoderError::InvalidConfig(
                    "update_avcc() must be called before feed_avcc_sample()".to_string(),
                )
            })?;
            let (has_idr, params_changed) =
                refresh_inline_avcc_params_and_detect_idr(cfg, avcc_sample);
            let effective_keyframe = is_keyframe || has_idr;
            let open_gop_resume = detect_h264_open_gop_resume_strict(cfg, avcc_sample);
            let (annex_b, has_small_256_511) =
                avcc_strict_sample_to_annex_b(cfg, avcc_sample, effective_keyframe);
            let new_vis_dims = coded_tuple_to_visible(cfg.coded_dimensions);
            let new_trim = cfg.coded_dimensions.map(|(_, _, tr)| tr);
            let new_vui = if params_changed {
                cfg.sps_list
                    .first()
                    .and_then(|sps| parse_h264_sps_vui_color(sps))
            } else {
                None
            };
            (
                effective_keyframe,
                params_changed,
                annex_b,
                has_small_256_511,
                new_trim,
                new_vui,
                new_vis_dims,
                open_gop_resume,
            )
        };
        if has_small_256_511 {
            self.avcc_small_256_511_frames = self.avcc_small_256_511_frames.saturating_add(1);
        }
        if params_changed {
            if let Some(tr) = new_trim {
                self.expected_trim = tr;
            }
            self.vui_color_info = new_vui;
            self.color_info = resolve_deterministic_bt709_color(self.vui_color_info);
            if let Some(vd) = new_vis_dims {
                self.expected_coded_dims = Some(vd);
            }
        }
        if effective_keyframe || open_gop_resume {
            self.idr_keyframes_detected = self.idr_keyframes_detected.saturating_add(1);
        }
        if self.awaiting_idr {
            if effective_keyframe || open_gop_resume {
                self.awaiting_idr = false;
                self.awaiting_idr_empty_dpb = false;
                if !effective_keyframe {
                    if let Some(ref cfg) = self.avcc_cfg {
                        annex_b = avcc_strict_sample_to_annex_b(cfg, avcc_sample, true).0;
                    }
                }
            } else {
                self.dropped_pre_idr_frames = self.dropped_pre_idr_frames.saturating_add(1);
                return Ok(self.current_stats(0));
            }
        }
        if self.force_prepend_next_sps_pps {
            self.force_prepend_next_sps_pps = false;
            if !effective_keyframe {
                if let Some(ref cfg) = self.avcc_cfg {
                    annex_b = avcc_strict_sample_to_annex_b(cfg, avcc_sample, true).0;
                }
            }
        }
        self.feed_access_unit(
            &annex_b,
            pts_us,
            effective_keyframe,
            true,
            Some((0, 0, 0, 0)),
            ring,
        )
    }

    /// Feeds one Annex-B access unit (HEVC or H.264) into the real COM `IMFTransform`,
    /// handles `MF_E_NOTACCEPTING` and `MF_E_TRANSFORM_STREAM_CHANGE`, and publishes decoded
    /// NV12 frames into the 6-slot `D3d11Nv12VramRing` (`0` GPU→CPU copy when hardware MFT is active).
    pub fn feed_access_unit(
        &mut self,
        annex_b: &[u8],
        pts_us: u64,
        is_keyframe: bool,
        is_airplay: bool,
        trim_override: Option<(u32, u32, u32, u32)>,
        ring: &D3d11Nv12VramRing,
    ) -> Result<LiveDecodeStats, DecoderError> {
        if annex_b.is_empty() {
            return Ok(self.current_stats(0));
        }

        // Track SPS dimensions/conformance window and VUI color space if present in this access unit
        match self.codec {
            VideoCodecKind::Hevc => {
                if let Some((_w, _h, tr)) = parse_hevc_sps_dimensions(annex_b) {
                    self.expected_trim = tr;
                }
                if let Some(vui_col) = parse_hevc_sps_vui_color(annex_b) {
                    self.vui_color_info = Some(vui_col);
                    self.color_info = vui_col;
                }
            }
            VideoCodecKind::H264 => {
                if let Some(dims) = parse_h264_sps_dimensions(annex_b) {
                    self.expected_trim = dims.2;
                    if let Some(vd) = coded_tuple_to_visible(Some(dims)) {
                        self.expected_coded_dims = Some(vd);
                    }
                    self.vui_color_info = parse_h264_sps_vui_color(annex_b);
                }
                self.color_info = resolve_deterministic_bt709_color(self.vui_color_info);
            }
        }

        let sample = self.create_input_sample(annex_b, pts_us, is_keyframe)?;
        let mut emitted = 0u32;

        let mut input_accepted = false;
        for _ in 0..8 {
            let hr = unsafe {
                let mft = self.mft.as_ref().unwrap();
                let vt = mft.vtable();
                let proc_in: unsafe extern "system" fn(*mut c_void, u32, *mut c_void, u32) -> i32 =
                    std::mem::transmute(*vt.add(24));
                proc_in(mft.as_ptr(), 0, sample.as_ptr(), 0)
            };
            if hr >= 0 {
                self.consecutive_input_errors = 0;
                self.last_hresult = 0;
                input_accepted = true;
                break;
            }
            if hr == MF_E_NOTACCEPTING {
                self.not_accepting_drains = self.not_accepting_drains.saturating_add(1);
                emitted += self.drain_outputs(pts_us, is_airplay, trim_override, ring)?;
                continue;
            }
            // If MFT rejected input after a mid-stream resolution change, refresh MFT once on keyframe.
            if is_keyframe {
                let (rw, rh) = match self.codec {
                    VideoCodecKind::H264 => parse_h264_sps_dimensions(annex_b)
                        .map(|(w, h, _)| (w, h))
                        .unwrap_or((1920, 1080)),
                    VideoCodecKind::Hevc => parse_hevc_sps_dimensions(annex_b)
                        .map(|(w, h, _)| (w, h))
                        .unwrap_or((1920, 1080)),
                };
                if should_retry_hevc_software_mft_fallback(
                    self.codec,
                    self.hardware_d3d11,
                    self.total_decoded_frames,
                    self.hevc_sw_fallback_attempted,
                ) {
                    self.hevc_sw_fallback_attempted = true;
                    self.activate_mft_with_options(rw, rh, false)?;
                } else {
                    self.activate_mft(rw, rh)?;
                }
                self.mft_reconfigs = self.mft_reconfigs.saturating_add(1);
                let hr2 = unsafe {
                    let mft = self.mft.as_ref().unwrap();
                    let vt = mft.vtable();
                    let proc_in: unsafe extern "system" fn(
                        *mut c_void,
                        u32,
                        *mut c_void,
                        u32,
                    ) -> i32 = std::mem::transmute(*vt.add(24));
                    proc_in(mft.as_ptr(), 0, sample.as_ptr(), 0)
                };
                if hr2 >= 0 {
                    self.consecutive_input_errors = 0;
                    self.last_hresult = 0;
                    input_accepted = true;
                    break;
                }
                let _ = self.drain_outputs(pts_us, is_airplay, trim_override, ring);
                if self.record_nonfatal_input_error_preserving_dpb(hr2) {
                    self.flush();
                    self.arm_empty_dpb_idr_wait();
                    self.consecutive_input_errors = 0;
                }
                self.last_hresult = hr2;
                return Err(DecoderError::ComError {
                    op: "IMFTransform::ProcessInput(retry)",
                    hr: hr2,
                });
            }
            emitted += self
                .drain_outputs(pts_us, is_airplay, trim_override, ring)
                .unwrap_or(0);
            self.last_hresult = hr;
            if self.record_nonfatal_input_error_preserving_dpb(hr) {
                self.flush();
                self.arm_empty_dpb_idr_wait();
                self.consecutive_input_errors = 0;
                return Err(DecoderError::ComError {
                    op: "IMFTransform::ProcessInput",
                    hr,
                });
            }
            return Ok(self.current_stats(emitted));
        }

        if input_accepted {
            match self.drain_outputs(pts_us, is_airplay, trim_override, ring) {
                Ok(n) => emitted += n,
                Err(err)
                    if should_retry_hevc_software_mft_fallback(
                        self.codec,
                        self.hardware_d3d11,
                        self.total_decoded_frames,
                        self.hevc_sw_fallback_attempted,
                    ) =>
                {
                    let (rw, rh) = parse_hevc_sps_dimensions(annex_b)
                        .map(|(w, h, _)| (w, h))
                        .unwrap_or((1920, 1080));
                    self.hevc_sw_fallback_attempted = true;
                    self.activate_mft_with_options(rw, rh, false)?;
                    let retry_in = unsafe {
                        let mft = self.mft.as_ref().unwrap();
                        let vt = mft.vtable();
                        let proc_in: unsafe extern "system" fn(
                            *mut c_void,
                            u32,
                            *mut c_void,
                            u32,
                        ) -> i32 = std::mem::transmute(*vt.add(24));
                        proc_in(mft.as_ptr(), 0, sample.as_ptr(), 0)
                    };
                    if retry_in < 0 {
                        return Err(err);
                    }
                    emitted += self.drain_outputs(pts_us, is_airplay, trim_override, ring)?;
                }
                Err(err) => return Err(err),
            }
        }

        Ok(self.current_stats(emitted))
    }

    /// Sends `MFT_MESSAGE_COMMAND_DRAIN` and drains all remaining frames out of the MFT.
    pub fn drain_eos(
        &mut self,
        is_airplay: bool,
        trim_override: Option<(u32, u32, u32, u32)>,
        ring: &D3d11Nv12VramRing,
    ) -> Result<LiveDecodeStats, DecoderError> {
        let _ = self.send_mft_message(MFT_MESSAGE_COMMAND_DRAIN, 0);
        let emitted = self.drain_outputs(0, is_airplay, trim_override, ring)?;
        Ok(self.current_stats(emitted))
    }

    /// Drains any pending decoded frames out of the active MFT WITHOUT destroying DPB or calling `COMMAND_FLUSH`.
    pub fn drain_pending_outputs_only(
        &mut self,
        is_airplay: bool,
        ring: &D3d11Nv12VramRing,
    ) -> Result<u32, DecoderError> {
        let pts = (self.last_input_pts_100ns.max(0) as u64) / 10;
        self.drain_outputs(pts, is_airplay, Some((0, 0, 0, 0)), ring)
    }

    fn create_input_sample(
        &mut self,
        data: &[u8],
        pts_us: u64,
        is_keyframe: bool,
    ) -> Result<ComPtr, DecoderError> {
        unsafe {
            let mut p_buf: *mut c_void = std::ptr::null_mut();
            let hr_b = (self.fn_create_memory_buffer)(data.len() as u32, &mut p_buf);
            if hr_b < 0 || p_buf.is_null() {
                return Err(DecoderError::ComError {
                    op: "MFCreateMemoryBuffer(in)",
                    hr: hr_b,
                });
            }
            let buf = ComPtr::from_raw(p_buf).ok_or(DecoderError::ComError {
                op: "MFCreateMemoryBuffer(in)",
                hr: -1,
            })?;
            let bvt = buf.vtable();
            let lock: unsafe extern "system" fn(
                *mut c_void,
                *mut *mut u8,
                *mut u32,
                *mut u32,
            ) -> i32 = std::mem::transmute(*bvt.add(3));
            let unlock: unsafe extern "system" fn(*mut c_void) -> i32 =
                std::mem::transmute(*bvt.add(4));
            let set_len: unsafe extern "system" fn(*mut c_void, u32) -> i32 =
                std::mem::transmute(*bvt.add(6));

            let mut dst_ptr: *mut u8 = std::ptr::null_mut();
            let hr_l = lock(buf.as_ptr(), &mut dst_ptr, std::ptr::null_mut(), std::ptr::null_mut());
            if hr_l < 0 || dst_ptr.is_null() {
                return Err(DecoderError::ComError {
                    op: "IMFMediaBuffer::Lock(in)",
                    hr: hr_l,
                });
            }
            std::ptr::copy_nonoverlapping(data.as_ptr(), dst_ptr, data.len());
            let _ = unlock(buf.as_ptr());
            let hr_len = set_len(buf.as_ptr(), data.len() as u32);
            if hr_len < 0 {
                return Err(DecoderError::ComError {
                    op: "IMFMediaBuffer::SetCurrentLength",
                    hr: hr_len,
                });
            }

            let mut p_samp: *mut c_void = std::ptr::null_mut();
            let hr_s = (self.fn_create_sample)(&mut p_samp);
            if hr_s < 0 || p_samp.is_null() {
                return Err(DecoderError::ComError {
                    op: "MFCreateSample(in)",
                    hr: hr_s,
                });
            }
            let samp = ComPtr::from_raw(p_samp).ok_or(DecoderError::ComError {
                op: "MFCreateSample(in)",
                hr: -1,
            })?;
            let svt = samp.vtable();
            let add_buf: unsafe extern "system" fn(*mut c_void, *mut c_void) -> i32 =
                std::mem::transmute(*svt.add(42));
            let set_time: unsafe extern "system" fn(*mut c_void, i64) -> i32 =
                std::mem::transmute(*svt.add(36));
            let set_dur: unsafe extern "system" fn(*mut c_void, i64) -> i32 =
                std::mem::transmute(*svt.add(38));

            let hr_ab = add_buf(samp.as_ptr(), buf.as_ptr());
            if hr_ab < 0 {
                return Err(DecoderError::ComError {
                    op: "IMFSample::AddBuffer(in)",
                    hr: hr_ab,
                });
            }
            if self.codec == VideoCodecKind::H264 {
                let pts_100ns = next_h264_zero_based_pts_100ns(self.last_input_pts_100ns);
                self.last_input_pts_100ns = pts_100ns;
                let _ = set_time(samp.as_ptr(), pts_100ns);
                let _ = set_dur(samp.as_ptr(), FRAME_DURATION_100NS);
            } else {
                let candidate_100ns = (pts_us as i64).saturating_mul(10);
                let pts_100ns =
                    clamp_monotonic_pts_delta_100ns(self.last_input_pts_100ns, candidate_100ns);
                self.last_input_pts_100ns = pts_100ns;
                let _ = set_time(samp.as_ptr(), pts_100ns);
                let _ = set_dur(samp.as_ptr(), FRAME_DURATION_100NS);
                if is_keyframe {
                    let _ = mf_attr_set_u32(&samp, &MF_SAMPLE_EXTENSION_CLEAN_POINT, 1);
                }
            }
            Ok(samp)
        }
    }

    fn create_output_sample(&self) -> Result<ComPtr, DecoderError> {
        let min_nv12 = (self.coded_width.max(64) as usize) * (self.coded_height.max(64) as usize) * 3 / 2;
        let alloc_bytes = (self.out_buf_size as usize).max(min_nv12).max(65_536) as u32;
        unsafe {
            let mut p_buf: *mut c_void = std::ptr::null_mut();
            let hr_b = (self.fn_create_memory_buffer)(alloc_bytes, &mut p_buf);
            if hr_b < 0 || p_buf.is_null() {
                return Err(DecoderError::ComError {
                    op: "MFCreateMemoryBuffer(out)",
                    hr: hr_b,
                });
            }
            let buf = ComPtr::from_raw(p_buf).ok_or(DecoderError::ComError {
                op: "MFCreateMemoryBuffer(out)",
                hr: -1,
            })?;
            let mut p_samp: *mut c_void = std::ptr::null_mut();
            let hr_s = (self.fn_create_sample)(&mut p_samp);
            if hr_s < 0 || p_samp.is_null() {
                return Err(DecoderError::ComError {
                    op: "MFCreateSample(out)",
                    hr: hr_s,
                });
            }
            let samp = ComPtr::from_raw(p_samp).ok_or(DecoderError::ComError {
                op: "MFCreateSample(out)",
                hr: -1,
            })?;
            let svt = samp.vtable();
            let add_buf: unsafe extern "system" fn(*mut c_void, *mut c_void) -> i32 =
                std::mem::transmute(*svt.add(42));
            let hr_ab = add_buf(samp.as_ptr(), buf.as_ptr());
            if hr_ab < 0 {
                return Err(DecoderError::ComError {
                    op: "IMFSample::AddBuffer(out)",
                    hr: hr_ab,
                });
            }
            Ok(samp)
        }
    }

    fn drain_outputs(
        &mut self,
        pts_us: u64,
        is_airplay: bool,
        trim_override: Option<(u32, u32, u32, u32)>,
        ring: &D3d11Nv12VramRing,
    ) -> Result<u32, DecoderError> {
        let mut emitted = 0u32;
        for _ in 0..64 {
            // If MFT does NOT provide samples, we allocate and retain ownership via `_owned_out`
            // until ProcessOutput completes.
            let _owned_out = if self.provides_samples {
                None
            } else {
                Some(self.create_output_sample()?)
            };
            let mut out_buf = MftOutputDataBuffer {
                dw_stream_id: 0,
                p_sample: _owned_out
                    .as_ref()
                    .map_or(std::ptr::null_mut(), |s| s.as_ptr()),
                dw_status: 0,
                p_events: std::ptr::null_mut(),
            };
            let mut status = 0u32;
            let hr = unsafe {
                let mft = self.mft.as_ref().unwrap();
                let vt = mft.vtable();
                let proc_out: unsafe extern "system" fn(
                    *mut c_void,
                    u32,
                    u32,
                    *mut MftOutputDataBuffer,
                    *mut u32,
                ) -> i32 = std::mem::transmute(*vt.add(25));
                proc_out(mft.as_ptr(), 0, 1, &mut out_buf, &mut status)
            };

            // Always release p_events if returned by MFT
            if !out_buf.p_events.is_null() {
                let _ev_guard = unsafe { ComPtr::from_raw(out_buf.p_events) };
            }

            // If provides_samples == true, MFT transferred ownership of out_buf.p_sample to us.
            // Wrap it in ComPtr immediately so it is ALWAYS released on drop!
            let mft_provided_sample = if self.provides_samples && !out_buf.p_sample.is_null() {
                unsafe { ComPtr::from_raw(out_buf.p_sample) }
            } else {
                None
            };

            if hr == MF_E_TRANSFORM_NEED_MORE_INPUT {
                return Ok(emitted);
            }
            if hr == MF_E_TRANSFORM_STREAM_CHANGE {
                self.stream_changes = self.stream_changes.saturating_add(1);
                self.pick_nv12_output_type()?;
                continue;
            }
            if hr < 0 {
                self.last_hresult = hr;
                return Err(DecoderError::ComError {
                    op: "IMFTransform::ProcessOutput",
                    hr,
                });
            }

            let sample_ref = mft_provided_sample.as_ref().or(_owned_out.as_ref());
            if let Some(sample) = sample_ref {
                if self.publish_decoded_sample(sample, pts_us, is_airplay, trim_override, ring)? {
                    self.total_decoded_frames = self.total_decoded_frames.saturating_add(1);
                    emitted += 1;
                }
            }
        }
        Ok(emitted)
    }

    fn publish_decoded_sample(
        &mut self,
        sample: &ComPtr,
        pts_us: u64,
        is_airplay: bool,
        trim_override: Option<(u32, u32, u32, u32)>,
        ring: &D3d11Nv12VramRing,
    ) -> Result<bool, DecoderError> {
        let w = self.coded_width;
        let h = self.coded_height;
        if w == 0 || h == 0 {
            return Ok(false);
        }

        let sample_trim = if is_airplay {
            (0, 0, 0, 0)
        } else {
            trim_override.unwrap_or(self.expected_trim)
        };
        let mft_ap = self.last_mft_aperture.or_else(|| {
            let r = self.visible_rect;
            if r.right > r.left
                && r.bottom > r.top
                && r.left >= 0
                && r.top >= 0
                && (r.right as u32) <= w
                && (r.bottom as u32) <= h
                && (r.left > 0
                    || r.top > 0
                    || (r.right as u32) < w
                    || (r.bottom as u32) < h)
            {
                Some(r)
            } else {
                None
            }
        });
        let (vis, used_mft_aperture) = resolve_reconciled_aperture_rect(
            w,
            h,
            mft_ap,
            self.codec,
            sample_trim,
            self.expected_coded_dims,
            is_airplay,
        );

        // IMFSample::GetBufferByIndex(0) -> vtable[40]
        let buf = unsafe {
            let svt = sample.vtable();
            let get_buf: unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> i32 =
                std::mem::transmute(*svt.add(40));
            let mut p_buf: *mut c_void = std::ptr::null_mut();
            let hr = get_buf(sample.as_ptr(), 0, &mut p_buf);
            if hr < 0 || p_buf.is_null() {
                return Err(DecoderError::ComError {
                    op: "IMFSample::GetBufferByIndex",
                    hr,
                });
            }
            ComPtr::from_raw(p_buf).ok_or(DecoderError::ComError {
                op: "IMFSample::GetBufferByIndex",
                hr: -1,
            })?
        };

        // Tahap 275 (Pilar 2): Inspect IMFDXGIBuffer's actual GPU surface dims BEFORE acquiring a
        // ring slot so adaptive resolution switches (`990x2160 <-> 888x1920 <-> 640x1388`) commit
        // exact surface bounds and never leak uninitialized black bottom/right margins.
        let dxgi_src = if let Ok(dxgi_buf) = buf.query_interface(&IID_IMF_DXGI_BUFFER) {
            unsafe {
                let dvt = dxgi_buf.vtable();
                let get_res: unsafe extern "system" fn(
                    *mut c_void,
                    *const Guid,
                    *mut *mut c_void,
                ) -> i32 = std::mem::transmute(*dvt.add(3));
                let get_sub: unsafe extern "system" fn(*mut c_void, *mut u32) -> i32 =
                    std::mem::transmute(*dvt.add(4));

                let mut p_src: *mut c_void = std::ptr::null_mut();
                let hr_res = get_res(dxgi_buf.as_ptr(), &IID_ID3D11_TEXTURE2D, &mut p_src);
                if hr_res >= 0 && !p_src.is_null() {
                    if let Some(src_tex) = ComPtr::from_raw(p_src) {
                        let mut sub_idx = 0u32;
                        let _ = get_sub(dxgi_buf.as_ptr(), &mut sub_idx);
                        let src_desc = LiveD3d11Gpu::get_texture2d_desc(&src_tex);
                        Some((src_tex, sub_idx, src_desc))
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
        } else {
            None
        };

        let (commit_w, commit_h) = if let Some((_, _, ref src_desc)) = dxgi_src {
            (
                w.min(src_desc.width).max(2) & !1,
                h.min(src_desc.height).max(2) & !1,
            )
        } else {
            (w, h)
        };

        let (slot_idx, dst_tex) = ring
            .acquire_write_slot(&self.gpu, commit_w, commit_h)
            .map_err(DecoderError::InvalidConfig)?;
        let dst_desc = LiveD3d11Gpu::get_texture2d_desc(&dst_tex);
        let mut cpu_nv12_backup: Option<Vec<u8>> = None;

        // Fast Path: Zero-Copy IMFDXGIBuffer -> ID3D11Texture2D CopySubresourceRegion!
        if let Some((src_tex, sub_idx, src_desc)) = dxgi_src {
            let copy_plan = resolve_dxva_subresource_copy_plan(sub_idx, &src_desc, &dst_desc);
            let src_box = D3d11Box {
                left: 0,
                top: 0,
                front: 0,
                right: copy_plan.copy_w,
                bottom: copy_plan.copy_h,
                back: 1,
            };
            self.gpu.copy_subresource_region(
                &dst_tex,
                0,
                0,
                0,
                &src_tex,
                copy_plan.src_subresource,
                Some(&src_box),
            );
            self.zero_copy_gpu_frames = self.zero_copy_gpu_frames.saturating_add(1);
        } else {
            // Software Fallback Path: IMF2DBuffer::Lock2D (or IMFMediaBuffer::Lock) -> Dynamic NV12 upload
            let upload_w = dst_desc.width;
            let upload_h = dst_desc.height;
            let need_up = !matches!(&self.upload_tex, Some((_, uw, uh)) if *uw == upload_w && *uh == upload_h);
            if need_up {
                let up = self
                    .gpu
                    .create_nv12_texture(upload_w, upload_h, true)
                    .map_err(|hr| DecoderError::ComError {
                        op: "CreateTexture2D(NV12 dynamic upload)",
                        hr,
                    })?;
                self.upload_tex = Some((up, upload_w, upload_h));
            }
            let up_tex = &self.upload_tex.as_ref().unwrap().0;
            let rows = (upload_h as usize) + (upload_h as usize) / 2;

            if let Ok(buf2d) = buf.query_interface(&IID_IMF_2D_BUFFER) {
                unsafe {
                    let vt2 = buf2d.vtable();
                    let lock2d: unsafe extern "system" fn(*mut c_void, *mut *mut u8, *mut i32) -> i32 =
                        std::mem::transmute(*vt2.add(3));
                    let unlock2d: unsafe extern "system" fn(*mut c_void) -> i32 =
                        std::mem::transmute(*vt2.add(4));
                    let mut src_ptr: *mut u8 = std::ptr::null_mut();
                    let mut pitch = 0i32;
                    let hr_l = lock2d(buf2d.as_ptr(), &mut src_ptr, &mut pitch);
                    if hr_l >= 0 && !src_ptr.is_null() {
                        let layout = compute_nv12_cpu_plane_layout(
                            w,
                            h,
                            upload_w,
                            upload_h,
                            Some(pitch),
                            self.default_stride,
                            None,
                        );
                        if let Ok(mapped) = self.gpu.map_texture(up_tex, 0, D3D11_MAP_WRITE_DISCARD) {
                            let dst_pitch = mapped.row_pitch as usize;
                            let row_w = layout.row_copy_bytes.min(layout.src_stride).min(dst_pitch);
                            let mut cpu_buf = if self.keep_cpu_nv12_backup {
                                Some(vec![0u8; (w as usize) * rows])
                            } else {
                                None
                            };
                            let copy_plane_row =
                                |src_row: *const u8, dst_y: usize, cb: &mut Option<Vec<u8>>| {
                                    let dp = (mapped.p_data as *mut u8).add(dst_y * dst_pitch);
                                    std::ptr::copy_nonoverlapping(src_row, dp, row_w);
                                    if let Some(ref mut buf_vec) = cb {
                                        let dst_w = (w as usize).min(row_w);
                                        let dst_off = dst_y * (w as usize);
                                        if dst_off + dst_w <= buf_vec.len() {
                                            std::ptr::copy_nonoverlapping(
                                                src_row,
                                                buf_vec.as_mut_ptr().add(dst_off),
                                                dst_w,
                                            );
                                        }
                                    }
                                };
                            for y in 0..layout.y_plane_rows {
                                let sp = if pitch >= 0 {
                                    src_ptr.add(y * layout.src_stride)
                                } else {
                                    src_ptr.sub(y * layout.src_stride)
                                };
                                copy_plane_row(sp, y, &mut cpu_buf);
                            }
                            for y in 0..layout.uv_plane_rows {
                                let dst_y = layout.y_plane_rows + y;
                                let sp = if pitch >= 0 {
                                    src_ptr.add(layout.uv_plane_offset_bytes + y * layout.src_stride)
                                } else {
                                    src_ptr.sub(layout.uv_plane_offset_bytes + y * layout.src_stride)
                                };
                                copy_plane_row(sp, dst_y, &mut cpu_buf);
                            }
                            self.gpu.unmap_texture(up_tex, 0);
                            self.gpu.copy_resource(&dst_tex, up_tex);
                            cpu_nv12_backup = cpu_buf;
                        }
                        let _ = unlock2d(buf2d.as_ptr());
                    }
                }
            } else {
                unsafe {
                    let bvt = buf.vtable();
                    let lock: unsafe extern "system" fn(
                        *mut c_void,
                        *mut *mut u8,
                        *mut u32,
                        *mut u32,
                    ) -> i32 = std::mem::transmute(*bvt.add(3));
                    let unlock: unsafe extern "system" fn(*mut c_void) -> i32 =
                        std::mem::transmute(*bvt.add(4));
                    let mut src_ptr: *mut u8 = std::ptr::null_mut();
                    let mut cur_len = 0u32;
                    let hr_l = lock(buf.as_ptr(), &mut src_ptr, std::ptr::null_mut(), &mut cur_len);
                    if hr_l >= 0 && !src_ptr.is_null() {
                        let total_len = cur_len as usize;
                        let layout = compute_nv12_cpu_plane_layout(
                            w,
                            h,
                            upload_w,
                            upload_h,
                            None,
                            self.default_stride,
                            Some(total_len),
                        );
                        if let Ok(mapped) = self.gpu.map_texture(up_tex, 0, D3D11_MAP_WRITE_DISCARD) {
                            let dst_pitch = mapped.row_pitch as usize;
                            let row_w = layout.row_copy_bytes.min(layout.src_stride).min(dst_pitch);
                            let y_avail =
                                (total_len / layout.src_stride.max(1)).min(layout.y_plane_rows);
                            for y in 0..y_avail {
                                let off = y * layout.src_stride;
                                if off + row_w <= total_len {
                                    let sp = src_ptr.add(off);
                                    let dp = (mapped.p_data as *mut u8).add(y * dst_pitch);
                                    std::ptr::copy_nonoverlapping(sp, dp, row_w);
                                }
                            }
                            let uv_avail = if total_len > layout.uv_plane_offset_bytes {
                                ((total_len - layout.uv_plane_offset_bytes)
                                    / layout.src_stride.max(1))
                                .min(layout.uv_plane_rows)
                            } else {
                                0
                            };
                            for y in 0..uv_avail {
                                let off = layout.uv_plane_offset_bytes + y * layout.src_stride;
                                if off + row_w <= total_len {
                                    let sp = src_ptr.add(off);
                                    let dp = (mapped.p_data as *mut u8)
                                        .add((layout.y_plane_rows + y) * dst_pitch);
                                    std::ptr::copy_nonoverlapping(sp, dp, row_w);
                                }
                            }
                            self.gpu.unmap_texture(up_tex, 0);
                            self.gpu.copy_resource(&dst_tex, up_tex);
                        }
                        let _ = unlock(buf.as_ptr());
                    }
                }
            }
            self.software_upload_frames = self.software_upload_frames.saturating_add(1);
        }

        let clamped_vis = Rect {
            left: vis.left.clamp(0, commit_w.saturating_sub(2) as i32),
            top: vis.top.clamp(0, commit_h.saturating_sub(2) as i32),
            right: vis.right.clamp(2.min(commit_w as i32), commit_w as i32),
            bottom: vis.bottom.clamp(2.min(commit_h as i32), commit_h as i32),
        };
        let vis_already_cropped = clamped_vis.left > 0
            || clamped_vis.top > 0
            || (clamped_vis.right as u32) < commit_w
            || (clamped_vis.bottom as u32) < commit_h;
        let effective_trim = if is_airplay || used_mft_aperture || vis_already_cropped {
            (0, 0, 0, 0)
        } else {
            sample_trim
        };

        let slot_color = if self.codec == VideoCodecKind::H264 {
            resolve_deterministic_bt709_color(self.vui_color_info)
        } else {
            self.color_info
        };

        ring.commit_write_slot_with_color(
            slot_idx,
            commit_w,
            commit_h,
            clamped_vis,
            effective_trim,
            pts_us,
            is_airplay,
            slot_color,
            cpu_nv12_backup,
        );
        Ok(true)
    }

    pub fn current_stats(&self, frames_emitted: u32) -> LiveDecodeStats {
        let vis_w = (self.visible_rect.right - self.visible_rect.left).max(0) as u32;
        let vis_h = (self.visible_rect.bottom - self.visible_rect.top).max(0) as u32;
        LiveDecodeStats {
            frames_emitted,
            total_decoded_frames: self.total_decoded_frames,
            stream_changes: self.stream_changes,
            coded_width: self.coded_width,
            coded_height: self.coded_height,
            visible_width: if vis_w > 0 { vis_w } else { self.coded_width },
            visible_height: if vis_h > 0 { vis_h } else { self.coded_height },
            hardware_d3d11: self.hardware_d3d11,
            zero_copy_gpu_frames: self.zero_copy_gpu_frames,
            software_upload_frames: self.software_upload_frames,
        }
    }

    pub fn codec(&self) -> VideoCodecKind {
        self.codec
    }

    pub fn expected_coded_dims(&self) -> Option<(u32, u32)> {
        self.expected_coded_dims
    }

    pub fn is_hardware_d3d11(&self) -> bool {
        self.hardware_d3d11
    }

    pub fn mf_low_latency_enabled(&self) -> bool {
        self.mf_low_latency_enabled
    }

    pub fn avcc_small_256_511_frames(&self) -> u64 {
        self.avcc_small_256_511_frames
    }

    pub fn mft_reconfigs(&self) -> u64 {
        self.mft_reconfigs
    }

    pub fn last_input_pts_100ns(&self) -> i64 {
        self.last_input_pts_100ns
    }

    pub fn has_avcc_config(&self) -> bool {
        self.avcc_cfg.is_some()
    }

    pub fn d3d11_multithread_protected(&self) -> bool {
        self.d3d11_multithread_protected
    }

    pub fn default_stride(&self) -> i32 {
        self.default_stride
    }

    pub fn consecutive_input_errors(&self) -> u32 {
        self.consecutive_input_errors
    }

    pub fn dpb_preserved_flushes_skipped(&self) -> u64 {
        self.dpb_preserved_flushes_skipped
    }

    pub fn idr_keyframes_detected(&self) -> u64 {
        self.idr_keyframes_detected
    }

    pub fn not_accepting_drains(&self) -> u64 {
        self.not_accepting_drains
    }

    pub fn last_hresult(&self) -> i32 {
        self.last_hresult
    }

    pub fn awaiting_idr(&self) -> bool {
        self.awaiting_idr
    }

    pub fn awaiting_idr_empty_dpb(&self) -> bool {
        self.awaiting_idr_empty_dpb
    }

    pub fn dropped_pre_idr_frames(&self) -> u64 {
        self.dropped_pre_idr_frames
    }

    pub fn stream_changes(&self) -> u32 {
        self.stream_changes
    }

    pub fn stale_dim_dropped(&self) -> u64 {
        self.stale_dim_dropped
    }

    pub fn last_mft_dims(&self) -> Option<(u32, u32)> {
        if self.coded_width > 0 && self.coded_height > 0 {
            Some((self.coded_width, self.coded_height))
        } else {
            None
        }
    }

    pub fn last_mft_aperture(&self) -> Option<Rect> {
        self.last_mft_aperture
    }

    pub fn expected_trim(&self) -> (u32, u32, u32, u32) {
        self.expected_trim
    }

    pub fn nal_length_size(&self) -> usize {
        self.avcc_cfg
            .as_ref()
            .map(|c| c.nal_length_size.clamp(1, 4))
            .unwrap_or(4)
    }
}

impl Drop for LiveMfVideoDecoder {
    fn drop(&mut self) {
        self.clean_teardown();
        unsafe {
            if self.mf_started {
                let _ = (self.fn_mf_shutdown)();
            }
            if !self.h_mfplat.is_null() {
                FreeLibrary(self.h_mfplat);
                self.h_mfplat = std::ptr::null_mut();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_media_foundation_probe() {
        let res = probe_media_foundation();
        assert!(res.is_ok(), "MF probe failed: {:?}", res.err());
        assert!(res.unwrap());
    }

    #[test]
    fn test_output_dimension_calculations() {
        let cfg_native = HevcPipelineConfig::default();
        let (w1, h1) = cfg_native.calculate_output_dimensions();
        assert_eq!(w1, 1202);
        assert_eq!(h1, 2598);

        let mut cfg_downscale = HevcPipelineConfig::default();
        cfg_downscale.downscale = DownscaleFactor::Scale75;
        let (w2, h2) = cfg_downscale.calculate_output_dimensions();
        assert_eq!(w2, (1202 * 3) / 4); // 901
        assert_eq!(h2, (2598 * 3) / 4); // 1948
    }

    #[test]
    fn test_inverse_touch_matrix_rotations() {
        // Test Deg0 (Portrait)
        assert_eq!(inverse_touch_transform(1000, 2000, RotationAngle::Deg0), (1000, 2000));

        // Test Deg270 (eFootball 2027 Landscape Lock)
        // pc_x = 1000, pc_y = 2000 -> ios_x = 65535 - 2000 = 63535, ios_y = 1000
        let (ios_x, ios_y) = inverse_touch_transform(1000, 2000, RotationAngle::Deg270);
        assert_eq!(ios_x, 63535);
        assert_eq!(ios_y, 1000);

        // Test Deg90 (Landscape Left)
        // pc_x = 1000, pc_y = 2000 -> ios_x = 2000, ios_y = 65535 - 1000 = 64535
        let (ios90_x, ios90_y) = inverse_touch_transform(1000, 2000, RotationAngle::Deg90);
        assert_eq!(ios90_x, 2000);
        assert_eq!(ios90_y, 64535);

        // Test Deg180 (Inverted Portrait)
        let (ios180_x, ios180_y) = inverse_touch_transform(1000, 2000, RotationAngle::Deg180);
        assert_eq!(ios180_x, 65535 - 1000);
        assert_eq!(ios180_y, 65535 - 2000);
    }

    #[test]
    fn test_tahap233_annex_b_pli_trigger_and_hevc_sps_extraction() {
        let ring = SharedFrameRing::new();
        let mut dec = MfHevcDecoder::new();
        assert!(dec.low_latency_mode);
        assert!(dec.d3d11_video_support);

        // 1. Feed non-IDR TRAIL_R before VPS/SPS/PPS/IDR -> must request keyframe PLI!
        let trail = build_synthetic_hevc_annex_b_trail_r();
        let out_trail = dec.feed_annex_b(&trail, 16_667, 0, &ring);
        assert!(!out_trail.decoded);
        assert!(out_trail.needs_keyframe_pli);
        assert_eq!(dec.pli_triggers, 1);

        // 2. Feed VPS + SPS (1216x2656) + PPS + IDR -> clears PLI flag & publishes to SharedFrameRing!
        let idr = build_synthetic_hevc_annex_b_idr(1216, 2656);
        let parsed = parse_hevc_sps_dimensions(&idr).expect("must parse synthetic SPS");
        assert_eq!(parsed, (1216, 2656, (0, 0, 58, 14)));

        let out_idr = dec.feed_annex_b(&idr, 33_334, 0, &ring);
        assert!(out_idr.decoded);
        assert!(out_idr.is_keyframe);
        assert!(!out_idr.needs_keyframe_pli);
        assert_eq!((out_idr.clean_width, out_idr.clean_height), (1202, 2598));

        // 3. Subsequent TRAIL_R now decodes cleanly without PLI trigger
        let out_trail_ok = dec.feed_annex_b(&trail, 50_001, 0, &ring);
        assert!(out_trail_ok.decoded);
        assert!(!out_trail_ok.needs_keyframe_pli);
        assert_eq!(dec.frames_decoded, 2);
    }

    #[test]
    fn test_tahap233_nv12_to_bgra_trim_preserves_dynamic_island_and_zero_alloc_probe() {
        let ring = SharedFrameRing::new();
        let mut dec = MfHevcDecoder::new();
        let idr = build_synthetic_hevc_annex_b_idr(1216, 2656);

        // Build an NV12 eFootball-style landscape frame (bright Dynamic Island/status bar at y=2,
        // blue back button at probe y: 11..=26 x: 4..=24, vivid turf green (25, 130, 50) in center, black macroblock padding y>=2598, x>=1202)
        let out = dec.feed_annex_b_with_nv12_writer(&idr, 16_667, 0, &ring, |w, h, buf| {
            write_nv12_from_rgb_sampler(w, h, buf, |x, y| {
                if x >= 1202 || y >= 2598 {
                    (0, 0, 0) // H.265 macroblock black padding (must be cropped!)
                } else if y < 24 && x >= 100 && x < 200 {
                    (245, 245, 245) // Top Dynamic Island status bar icon at y=2 (MUST NOT be cropped!)
                } else if (110..=260).contains(&y) && (40..=240).contains(&x) {
                    (30, 110, 220) // eFootball formation blue back button -> triggers is_efootball_formation
                } else if y < 440 || y >= 2000 {
                    (6, 6, 6) // Portrait letterbox bars
                } else {
                    (25, 130, 50) // eFootball turf pitch green
                }
            });
        });
        assert!(out.decoded);

        // 1. Export clean cropped BGRA8 (1202 x 2598) and verify top Dynamic Island intact & bottom 58px trimmed
        let (cw, ch, bgra) = ring.with_latest_slot(export_clean_cropped_bgra_frame).unwrap();
        assert_eq!((cw, ch), (1202, 2598));
        assert_eq!(bgra.len(), 1202 * 2598 * 4);
        let di_idx = (2 * 1202 + 120) * 4; // y=2, x=120
        assert!(bgra[di_idx] > 200 && bgra[di_idx + 1] > 200 && bgra[di_idx + 2] > 200, "Top Dynamic Island pixel was lost!");

        // 2. Zero-allocation 120x260 probe extraction -> 32-scenario AutoRotate analysis
        let mut probe_buf = vec![0u8; GRID_WIDTH * GRID_HEIGHT * 4];
        let cap_before = probe_buf.capacity();
        let ptr_before = probe_buf.as_ptr();
        let extracted = ring
            .with_latest_slot(|slot| extract_downscaled_probe_120x260_into(slot, &mut probe_buf))
            .unwrap_or(false);
        assert!(extracted);
        assert_eq!(probe_buf.capacity(), cap_before);
        assert_eq!(probe_buf.as_ptr(), ptr_before);

        let mut grid = ProbeGrid120x260::new();
        populate_probe_grid_120x260_in_place(&probe_buf, &mut grid);
        let analysis = crate::autorotate::analyze_frame_grid(&grid, false);
        assert_eq!(analysis.suggested_orientation, crate::autorotate::OrientationMode::Landscape270);
        assert!(analysis.is_landscape_game_frame || analysis.is_efootball_formation);
    }

    #[test]
    fn test_tahap262_real_com_h264_mft_decodes_valid_ipcm_idr_annexb_and_avcc() {
        let frame = build_synthetic_h264_ipcm_idr(64, 64, 0x80, 0);
        assert_eq!((frame.width, frame.height), (64, 64));

        // Verify H.264 SPS parser & avcC -> Annex-B converter
        let parsed_sps = parse_h264_sps_dimensions(&frame.sps).expect("parse H.264 SPS");
        assert_eq!(parsed_sps, (64, 64, (0, 0, 0, 0)));
        let avcc_cfg = parse_avcc_sps_pps(&frame.avcc_config).expect("parse avcC");
        assert_eq!(avcc_cfg.nal_length_size, 4);
        assert_eq!(avcc_cfg.sps_list.len(), 1);
        assert_eq!(avcc_cfg.pps_list.len(), 1);
        let converted = avcc_sample_to_annex_b(&avcc_cfg, &frame.avcc_sample, true);
        assert_eq!(split_annex_b_nalus(&converted).len(), 3);

        // Real COM decode #1: Annex-B H.264 I_PCM 64x64 IDR on Windows CMSH264DecoderMFT
        let gpu = Arc::new(LiveD3d11Gpu::new().expect("LiveD3d11Gpu"));
        let ring = D3d11Nv12VramRing::new();
        let mut dec_ab =
            LiveMfVideoDecoder::new(gpu.clone(), VideoCodecKind::H264).expect("H.264 MFT");
        let st1 = dec_ab
            .feed_access_unit(&frame.annex_b, 16_667, true, false, None, &ring)
            .expect("Annex-B decode");
        let st1_final = if st1.total_decoded_frames == 0 {
            dec_ab
                .drain_eos(false, None, &ring)
                .expect("Annex-B drain_eos")
        } else {
            st1
        };
        assert!(
            st1_final.total_decoded_frames >= 1,
            "expected >= 1 decoded Annex-B frame, got {:?}",
            st1_final
        );
        assert!(st1_final.coded_width >= 64 && st1_final.coded_height >= 64);
        assert_eq!((st1_final.visible_width, st1_final.visible_height), (64, 64));

        // Real COM decode #2: AirPlay avcC + AVCC 4-byte length-prefixed IDR
        let ring2 = D3d11Nv12VramRing::new();
        let mut dec_avcc =
            LiveMfVideoDecoder::new(gpu.clone(), VideoCodecKind::H264).expect("H.264 MFT avcC");
        let changed = dec_avcc.update_avcc(&frame.avcc_config).expect("update_avcc");
        assert!(changed);
        let st2 = dec_avcc
            .feed_avcc_sample(&frame.avcc_sample, 33_334, true, &ring2)
            .expect("AVCC decode");
        let st2_final = if st2.total_decoded_frames == 0 {
            dec_avcc
                .drain_eos(true, Some((0, 0, 0, 0)), &ring2)
                .expect("AVCC drain_eos")
        } else {
            st2
        };
        assert!(
            st2_final.total_decoded_frames >= 1,
            "expected >= 1 decoded AVCC frame, got {:?}",
            st2_final
        );
        assert!(st2_final.coded_width >= 64 && st2_final.coded_height >= 64);
        assert_eq!((st2_final.visible_width, st2_final.visible_height), (64, 64));
        let snap = ring2.latest_slot().expect("VRAM slot committed");
        assert!(snap.is_airplay);
        assert_eq!(snap.trim, (0, 0, 0, 0));
        assert_eq!((snap.visible_width(), snap.visible_height()), (64, 64));
        println!(
            "Real COM H.264 MFT PASS: hw_d3d11={} coded={}x{} vis={}x{} zero_copy={} sw={}",
            st2_final.hardware_d3d11,
            st2_final.coded_width,
            st2_final.coded_height,
            st2_final.visible_width,
            st2_final.visible_height,
            st2_final.zero_copy_gpu_frames,
            st2_final.software_upload_frames,
        );
    }

    #[test]
    fn test_tahap262_real_com_h264_midstream_resolution_change_96x64_to_64x96() {
        let gpu = Arc::new(LiveD3d11Gpu::new().expect("LiveD3d11Gpu"));
        let ring = D3d11Nv12VramRing::new();
        let mut dec = LiveMfVideoDecoder::new(gpu.clone(), VideoCodecKind::H264).expect("H.264 MFT");

        // Phase 1: Landscape-like 96x64 IDR via AirPlay avcC
        let f_land = build_synthetic_h264_ipcm_idr(96, 64, 0x90, 0);
        assert!(dec.update_avcc(&f_land.avcc_config).expect("avcc 96x64"));
        let mut st_land = dec
            .feed_avcc_sample(&f_land.avcc_sample, 16_667, true, &ring)
            .expect("decode 96x64");
        if st_land.total_decoded_frames == 0 {
            st_land = dec
                .drain_eos(true, Some((0, 0, 0, 0)), &ring)
                .expect("drain 96x64");
        }
        assert!(st_land.total_decoded_frames >= 1);
        assert_eq!((st_land.visible_width, st_land.visible_height), (96, 64));

        // Phase 2: Mid-stream rotation to Portrait-like 64x96 IDR via new avcC
        let f_port = build_synthetic_h264_ipcm_idr(64, 96, 0xA0, 1);
        assert!(dec.update_avcc(&f_port.avcc_config).expect("avcc 64x96"));
        let before_count = st_land.total_decoded_frames;
        let mut st_port = dec
            .feed_avcc_sample(&f_port.avcc_sample, 50_000, true, &ring)
            .expect("decode 64x96");
        if st_port.total_decoded_frames <= before_count {
            st_port = dec
                .drain_eos(true, Some((0, 0, 0, 0)), &ring)
                .expect("drain 64x96");
        }
        assert!(
            st_port.total_decoded_frames > before_count,
            "expected new decoded frame after 96x64 -> 64x96 rotation"
        );
        assert_eq!((st_port.visible_width, st_port.visible_height), (64, 96));
        let snap = ring.latest_slot().expect("latest 64x96 VRAM slot");
        assert_eq!((snap.visible_width(), snap.visible_height()), (64, 96));
        println!(
            "Mid-stream H.264 rotation PASS: 96x64 -> 64x96 stream_changes={} decoded={}",
            st_port.stream_changes, st_port.total_decoded_frames
        );
    }

    #[test]
    fn test_tahap267_h264_airplay_120_frames_stream_change_and_not_accepting_drain_zero_freeze() {
        let gpu = Arc::new(LiveD3d11Gpu::new().expect("LiveD3d11Gpu"));
        let ring = D3d11Nv12VramRing::new();
        let mut dec = LiveMfVideoDecoder::new(gpu.clone(), VideoCodecKind::H264).expect("H.264 MFT");

        // Feed 1 IDR (64x64) + 119 continuous P_SKIP delta frames (including duplicate wall PTS)
        let idr = build_synthetic_h264_ipcm_idr(64, 64, 0x84, 0);
        assert!(dec.update_avcc(&idr.avcc_config).expect("avcc"));
        let _ = dec
            .feed_avcc_sample(&idr.avcc_sample, 1_000_000, true, &ring)
            .expect("feed IDR frame 0");

        let mut max_decoded = 0u64;
        let mut stream_changes = 0u32;
        for idx in 1..120u32 {
            // Intentional duplicate timestamps on bursts test strict monotonic 100ns guard.
            let pts_us = 1_000_000 + u64::from(idx / 3) * 16_667;
            let p_skip = build_synthetic_h264_p_skip_avcc_sample(64, 64, idx);
            let st = dec
                .feed_avcc_sample(&p_skip, pts_us, false, &ring)
                .expect("feed P_SKIP frame must not fail or freeze");
            max_decoded = st.total_decoded_frames;
            stream_changes = st.stream_changes;
        }
        let drained = dec
            .drain_eos(true, Some((0, 0, 0, 0)), &ring)
            .expect("drain EOS");
        let total = drained.total_decoded_frames.max(max_decoded);
        assert!(
            total >= 115,
            "expected continuous 120-frame H.264 AirPlay decode without Frame #1 freeze, got total={total}"
        );
        assert!(
            stream_changes >= 1 || drained.stream_changes >= 1,
            "expected MFT STREAM_CHANGE handled cleanly, got sc={}",
            stream_changes.max(drained.stream_changes)
        );
        assert_eq!(dec.last_hresult(), 0, "unexpected non-zero MFT HRESULT");
        ring.clear();
        drop(dec);
    }

    #[test]
    fn test_tahap269_airplay_h264_portrait_to_landscape_sps_switch_flushes_and_waits_idr_zero_corruption() {
        let gpu = Arc::new(LiveD3d11Gpu::new().expect("LiveD3d11Gpu"));
        let ring = D3d11Nv12VramRing::new();
        let mut dec =
            LiveMfVideoDecoder::new(gpu.clone(), VideoCodecKind::H264).expect("H.264 MFT");

        // 1. Portrait 64x96 session (IDR + 3 P_SKIP deltas)
        let f_port = build_synthetic_h264_ipcm_idr(64, 96, 0x90, 0);
        assert!(dec.update_avcc(&f_port.avcc_config).expect("initial Portrait avcC"));
        assert!(!dec.awaiting_idr());
        let mut pts = 1_000_000u64;
        let _ = dec
            .feed_avcc_sample(&f_port.avcc_sample, pts, true, &ring)
            .expect("Portrait IDR");
        for i in 1..=3u32 {
            pts += 16_667;
            let p = build_synthetic_h264_p_skip_avcc_sample(64, 96, i);
            let _ = dec
                .feed_avcc_sample(&p, pts, false, &ring)
                .expect("Portrait P_SKIP");
        }
        let mut st_port = dec
            .drain_eos(true, Some((0, 0, 0, 0)), &ring)
            .expect("Portrait drain");
        assert!(st_port.total_decoded_frames >= 1);
        assert_eq!((st_port.visible_width, st_port.visible_height), (64, 96));

        // 2. Mid-stream switch to eFootball Landscape 96x64 avcC -> MFT drains/flushes/re-activates & latches awaiting_idr!
        let f_land = build_synthetic_h264_ipcm_idr(96, 64, 0xA8, 1);
        assert!(dec.update_avcc(&f_land.avcc_config).expect("Landscape avcC"));
        assert!(
            dec.awaiting_idr(),
            "mid-stream SPS switch MUST latch awaiting_idr=true"
        );
        let decoded_before_switch = st_port.total_decoded_frames;

        // 3. Feed 3 stray non-IDR P-frames BEFORE the new IDR -> ALL 3 MUST be dropped (0 DPB corruption!)
        for stray in 1..=3u32 {
            pts += 16_667;
            let p_stray = build_synthetic_h264_p_skip_avcc_sample(96, 64, stray);
            let st_drop = dec
                .feed_avcc_sample(&p_stray, pts, false, &ring)
                .expect("stray pre-IDR P-frame must drop safely without COM error");
            assert_eq!(st_drop.frames_emitted, 0);
            assert_eq!(st_drop.total_decoded_frames, decoded_before_switch);
            assert!(dec.awaiting_idr());
        }
        assert_eq!(dec.dropped_pre_idr_frames(), 3);
        assert_eq!(dec.last_hresult(), 0);

        // 4. Feed fresh Landscape 96x64 IDR + 5 Landscape P_SKIP -> decodes cleanly to 96x64 with 0 errors!
        pts += 16_667;
        let _ = dec
            .feed_avcc_sample(&f_land.avcc_sample, pts, true, &ring)
            .expect("Landscape IDR");
        assert!(!dec.awaiting_idr(), "Landscape IDR clears awaiting_idr");
        for i in 1..=5u32 {
            pts += 16_667;
            let p_land = build_synthetic_h264_p_skip_avcc_sample(96, 64, i);
            let _ = dec
                .feed_avcc_sample(&p_land, pts, false, &ring)
                .expect("Landscape P_SKIP");
        }
        st_port = dec
            .drain_eos(true, Some((0, 0, 0, 0)), &ring)
            .expect("Landscape drain");
        assert!(
            st_port.total_decoded_frames >= decoded_before_switch + 5,
            "expected Landscape IDR + P frames decoded cleanly, got {:?}",
            st_port
        );
        assert_eq!((st_port.visible_width, st_port.visible_height), (96, 64));
        let snap = ring.latest_slot().expect("Landscape VRAM slot");
        assert!(snap.is_airplay);
        assert_eq!(snap.trim, (0, 0, 0, 0));
        assert_eq!((snap.visible_width(), snap.visible_height()), (96, 64));
        assert_eq!(dec.last_hresult(), 0);

        drop(snap);
        ring.clear();
        drop(dec);
    }

    #[test]
    fn test_tahap270_h264_decoder_same_dim_avcc_no_freeze_and_failsafe_unlatch_after_18_frames() {
        // 1. Open-GOP helpers distinguish SEI Recovery Point & I-slice vs P-slice
        let sei_recovery = [0x06u8, 0x06, 0x01, 0x00, 0x80];
        let sei_other = [0x06u8, 0x05, 0x02, 0xAA, 0xBB, 0x80];
        assert!(is_h264_sei_recovery_point(&sei_recovery));
        assert!(!is_h264_sei_recovery_point(&sei_other));

        let mut i_slice_bits = Vec::new();
        push_ue_bits(&mut i_slice_bits, 0); // first_mb_in_slice = 0
        push_ue_bits(&mut i_slice_bits, 7); // slice_type = 7 (I slice)
        let mut i_slice_nal = vec![0x41u8];
        i_slice_nal.extend_from_slice(&bits_to_bytes(&mut i_slice_bits));
        assert!(is_h264_i_slice(&i_slice_nal));

        let mut p_slice_bits = Vec::new();
        push_ue_bits(&mut p_slice_bits, 0); // first_mb_in_slice = 0
        push_ue_bits(&mut p_slice_bits, 0); // slice_type = 0 (P slice)
        let mut p_slice_nal = vec![0x41u8];
        p_slice_nal.extend_from_slice(&bits_to_bytes(&mut p_slice_bits));
        assert!(!is_h264_i_slice(&p_slice_nal));

        // 2. Real COM H.264 MFT: same-dim 64x64 avcC resend MUST NOT latch awaiting_idr or drop P-frames
        let gpu = Arc::new(LiveD3d11Gpu::new().expect("LiveD3d11Gpu"));
        let ring = D3d11Nv12VramRing::new();
        let mut dec = LiveMfVideoDecoder::new(gpu.clone(), VideoCodecKind::H264).expect("H.264 MFT");

        let f_init = build_synthetic_h264_ipcm_idr(64, 64, 0x88, 0);
        assert!(dec.update_avcc(&f_init.avcc_config).expect("initial 64x64 avcC"));
        assert!(!dec.awaiting_idr());
        let mut pts = 1_000_000u64;
        let _ = dec
            .feed_avcc_sample(&f_init.avcc_sample, pts, true, &ring)
            .expect("Frame #1 IDR");

        // Build valid 64x64 avcC variant with duplicated 2nd PPS entry (same coded dims, diff bytes)
        let mut variant_avcc = f_init.avcc_config.clone();
        let sps_len = u16::from_be_bytes([variant_avcc[6], variant_avcc[7]]) as usize;
        let num_pps_idx = 8 + sps_len;
        assert_eq!(variant_avcc[num_pps_idx], 1);
        variant_avcc[num_pps_idx] = 2;
        variant_avcc.extend_from_slice(&(f_init.pps.len() as u16).to_be_bytes());
        variant_avcc.extend_from_slice(&f_init.pps);
        assert!(
            dec.update_avcc(&variant_avcc).expect("same-dim variant avcC"),
            "PPS change must refresh cache"
        );
        assert!(
            !dec.awaiting_idr(),
            "same-dim avcC MUST NOT latch awaiting_idr (no Frame #1 freeze)"
        );

        for i in 1..=40u32 {
            pts += 16_667;
            let p_skip = build_synthetic_h264_p_skip_avcc_sample(64, 64, i);
            let _ = dec
                .feed_avcc_sample(&p_skip, pts, false, &ring)
                .expect("continuous P_SKIP decode");
        }
        assert_eq!(dec.dropped_pre_idr_frames(), 0);
        let st_same = dec.drain_eos(true, Some((0, 0, 0, 0)), &ring).expect("drain same-dim");
        assert!(
            st_same.total_decoded_frames >= 38,
            "expected continuous 64x64 decode >= 38, got {}",
            st_same.total_decoded_frames
        );
        assert_eq!(dec.last_hresult(), 0);

        // 3. True 64x64 -> 96x64 empty-DPB switch latches BOTH awaiting_idr & awaiting_idr_empty_dpb
        //    and holds ALL orphan P-frames strictly until a genuine IDR / Open-GOP I-slice arrives
        //    (Tahap 279 Opsi A: zero force-unlatch to empty DPB).
        let f_land = build_synthetic_h264_ipcm_idr(96, 64, 0x9C, 1);
        assert!(dec.update_avcc(&f_land.avcc_config).expect("96x64 avcC"));
        assert!(dec.awaiting_idr());
        assert!(dec.awaiting_idr_empty_dpb());
        for drop_idx in 1..=35u32 {
            pts += 16_667;
            let p_stray = build_synthetic_h264_p_skip_avcc_sample(96, 64, drop_idx);
            let st_drop = dec
                .feed_avcc_sample(&p_stray, pts, false, &ring)
                .expect("empty-dpb strict drop");
            assert_eq!(st_drop.frames_emitted, 0);
            assert!(
                dec.awaiting_idr(),
                "empty-DPB decoder MUST still await IDR at drop #{drop_idx}"
            );
            assert!(dec.awaiting_idr_empty_dpb());
            assert_eq!(dec.dropped_pre_idr_frames(), u64::from(drop_idx));
        }

        // Genuine 96x64 IDR immediately clears awaiting_idr & awaiting_idr_empty_dpb and decodes cleanly.
        pts += 16_667;
        let _ = dec
            .feed_avcc_sample(&f_land.avcc_sample, pts, true, &ring)
            .expect("96x64 IDR unlatch");
        assert!(!dec.awaiting_idr());
        assert!(!dec.awaiting_idr_empty_dpb());
        assert_eq!(dec.dropped_pre_idr_frames(), 35);

        // Soft reset (dims_changed = false) also waits cleanly for real IDR / Open-GOP I-slice without force-unlatch.
        dec.reconfigure_for_sps_change(false, 96, 64)
            .expect("non-empty-dpb reset");
        assert!(dec.awaiting_idr());
        assert!(!dec.awaiting_idr_empty_dpb());
        let base_drops = dec.dropped_pre_idr_frames();
        for drop_idx in 1..=20u32 {
            pts += 16_667;
            let p_stray = build_synthetic_h264_p_skip_avcc_sample(96, 64, drop_idx);
            let _ = dec
                .feed_avcc_sample(&p_stray, pts, false, &ring)
                .expect("strict pre-IDR drop");
            assert!(dec.awaiting_idr());
            assert_eq!(dec.dropped_pre_idr_frames(), base_drops + u64::from(drop_idx));
        }
        // Genuine Open-GOP I-slice (without IDR flag) unlatches cleanly
        pts += 16_667;
        let _ = dec
            .feed_avcc_sample(&f_land.avcc_sample, pts, true, &ring)
            .expect("real IDR unlatch after soft reset");
        assert!(!dec.awaiting_idr());
        assert_eq!(dec.dropped_pre_idr_frames(), base_drops + 20);

        ring.clear();
        dec.clean_teardown();
        drop(dec);
    }

    #[test]
    fn test_tahap271_h264_sps_dim_change_clean_recreate_resumes_continuous_frames() {
        let gpu = Arc::new(LiveD3d11Gpu::new().expect("LiveD3d11Gpu"));
        let ring = D3d11Nv12VramRing::new();
        let mut dec = LiveMfVideoDecoder::new(gpu.clone(), VideoCodecKind::H264).expect("H.264 MFT");

        // 1. Portrait 64x96 stream
        let f_port = build_synthetic_h264_ipcm_idr(64, 96, 0x70, 0);
        assert!(dec.update_avcc(&f_port.avcc_config).expect("64x96 avcC"));
        let mut pts = 1_000_000u64;
        let _ = dec
            .feed_avcc_sample(&f_port.avcc_sample, pts, true, &ring)
            .expect("Portrait IDR");
        for i in 1..=6u32 {
            pts += 16_667;
            let p = build_synthetic_h264_p_skip_avcc_sample(64, 96, i);
            let _ = dec.feed_avcc_sample(&p, pts, false, &ring).expect("Portrait P_SKIP");
        }
        let st1 = dec.drain_eos(true, Some((0, 0, 0, 0)), &ring).expect("drain portrait");
        assert!(st1.total_decoded_frames >= 5);

        // 2. True dim switch -> Landscape 96x64 triggers clean MFT re-instantiation + stream_changes++
        let f_land = build_synthetic_h264_ipcm_idr(96, 64, 0x84, 1);
        assert!(dec.update_avcc(&f_land.avcc_config).expect("96x64 avcC"));
        assert!(dec.awaiting_idr());
        assert!(dec.stream_changes() >= 1);

        // 3. Even if PTS stagnated after switch, clamp_monotonic_pts_delta_100ns keeps decoding monotonic!
        let _ = dec
            .feed_avcc_sample(&f_land.avcc_sample, pts, true, &ring)
            .expect("Landscape IDR with duplicate PTS must not stall");
        for i in 1..=8u32 {
            let p = build_synthetic_h264_p_skip_avcc_sample(96, 64, i);
            let _ = dec
                .feed_avcc_sample(&p, pts, false, &ring)
                .expect("Landscape P_SKIP with duplicate PTS");
        }
        let st2 = dec.drain_eos(true, Some((0, 0, 0, 0)), &ring).expect("drain landscape");
        assert!(
            st2.total_decoded_frames >= st1.total_decoded_frames + 7,
            "Clean MFT recreate must decode all post-switch frames continuously: {:?} -> {:?}",
            st1,
            st2
        );
        let snap = ring.latest_slot().expect("Landscape VRAM slot");
        assert_eq!((snap.visible_width(), snap.visible_height()), (96, 64));

        // 4. Test force_next_sps_pps_prepend() clears awaiting_idr immediately
        dec.reconfigure_for_sps_change(false, 96, 64).unwrap();
        assert!(dec.awaiting_idr());
        dec.force_next_sps_pps_prepend();
        assert!(!dec.awaiting_idr());

        // Strict COM teardown order before gpu drops
        drop(snap);
        ring.clear();
        drop(dec);
    }

    #[test]
    fn test_tahap272_airplay_deterministic_bt709_color_and_stale_dim_gate() {
        let studio = resolve_deterministic_bt709_color(None);
        assert_eq!(studio.matrix, VideoColorMatrix::Bt709);
        assert!(!studio.full_range);
        assert_eq!(studio.legacy_vp_input_color_space_flags(), 0x14);

        let vui_bt601_studio = resolve_deterministic_bt709_color(Some(VideoColorInfo {
            matrix: VideoColorMatrix::Bt601,
            full_range: false,
        }));
        assert_eq!(vui_bt601_studio.matrix, VideoColorMatrix::Bt709);
        assert!(!vui_bt601_studio.full_range);

        let vui_full = resolve_deterministic_bt709_color(Some(VideoColorInfo {
            matrix: VideoColorMatrix::Bt601,
            full_range: true,
        }));
        assert_eq!(vui_full.matrix, VideoColorMatrix::Bt709);
        assert!(vui_full.full_range);
        assert_eq!(vui_full.legacy_vp_input_color_space_flags(), 0x24);

        // AVCC slice filter (AUD=9, SEI=6, SPS=7, empty rejected; IDR=5 & Slice=1 accepted)
        assert!(!h264_avcc_has_video_slice_nal(&[], 4));
        assert!(!h264_avcc_has_video_slice_nal(&[0, 0, 0, 2, 0x09, 0xF0], 4));
        assert!(!h264_avcc_has_video_slice_nal(&[0, 0, 0, 4, 0x06, 0x01, 0x02, 0x80], 4));
        assert!(h264_avcc_has_video_slice_nal(&[0, 0, 0, 3, 0x65, 0x88, 0x80], 4));
        assert!(h264_avcc_has_video_slice_nal(&[0, 0, 0, 3, 0x41, 0x9A, 0x21], 4));
        // Annex-B fallback scan
        assert!(h264_avcc_has_video_slice_nal(&[0, 0, 0, 1, 0x65, 0x88], 4));
    }

    #[test]
    fn test_tahap273_non_mod16_888x1920_and_990x2160_stream_change_never_false_drops_frames() {
        let gpu = Arc::new(LiveD3d11Gpu::new().expect("LiveD3d11Gpu"));
        let ring = D3d11Nv12VramRing::new();
        let mut dec = LiveMfVideoDecoder::new(gpu, VideoCodecKind::H264).expect("H264 MFT");

        // 1. Non-mod-16 AirPlay Portrait 888x1920 (coded 896x1920, trim right = 8)
        let f888 = build_synthetic_h264_ipcm_idr_cropped(888, 1920, 0x88, 0);
        let parsed888 = parse_h264_sps_dimensions(&f888.sps).expect("parse 888x1920 SPS");
        assert_eq!(parsed888, (896, 1920, (0, 0, 0, 8)));

        assert!(dec.update_avcc(&f888.avcc_config).expect("update_avcc 888x1920"));
        assert_eq!(dec.expected_coded_dims(), Some((888, 1920)));
        assert_eq!(dec.nal_length_size(), 4);

        let mut pts = 16_667u64;
        let _ = dec
            .feed_avcc_sample(&wrap_avcc_with_aud_sei(&f888.avcc_sample), pts, true, &ring)
            .expect("feed 888x1920 IDR");
        drop(f888);
        for i in 1..=60u32 {
            pts += 16_667;
            let p_au = build_synthetic_h264_p_skip_cropped_with_aud_sei(888, 1920, i);
            let _ = dec
                .feed_avcc_sample(&p_au, pts, false, &ring)
                .expect("feed 888x1920 P_SKIP");
        }
        let st1 = dec
            .drain_eos(true, Some((0, 0, 0, 0)), &ring)
            .expect("drain 888x1920");
        assert!(
            st1.total_decoded_frames >= 55,
            "888x1920 non-mod-16 stream must decode continuously without stale drops: {:?}",
            st1
        );
        assert_eq!(dec.stale_dim_dropped(), 0);
        let slot1 = ring.latest_slot().expect("latest_slot after 888x1920");
        assert_eq!((slot1.visible_width(), slot1.visible_height()), (888, 1920));
        drop(slot1);

        // 2. Non-mod-16 Retina Portrait 990x2160 (coded 992x2160, trim right = 2)
        let f990 = build_synthetic_h264_ipcm_idr_cropped(990, 2160, 0x90, 1);
        let parsed990 = parse_h264_sps_dimensions(&f990.sps).expect("parse 990x2160 SPS");
        assert_eq!(parsed990, (992, 2160, (0, 0, 0, 2)));
        assert!(dec.update_avcc(&f990.avcc_config).expect("update_avcc 990x2160"));
        assert_eq!(dec.expected_coded_dims(), Some((990, 2160)));

        pts += 16_667;
        let _ = dec
            .feed_avcc_sample(&wrap_avcc_with_aud_sei(&f990.avcc_sample), pts, true, &ring)
            .expect("feed 990x2160 IDR");
        drop(f990);
        for i in 1..=15u32 {
            pts += 16_667;
            let p_au = build_synthetic_h264_p_skip_cropped_with_aud_sei(990, 2160, i);
            let _ = dec
                .feed_avcc_sample(&p_au, pts, false, &ring)
                .expect("feed 990x2160 P_SKIP");
        }
        let st2 = dec
            .drain_eos(true, Some((0, 0, 0, 0)), &ring)
            .expect("drain 990x2160");
        assert!(
            st2.total_decoded_frames >= st1.total_decoded_frames + 14,
            "990x2160 stream must continue decoding: {:?} -> {:?}",
            st1,
            st2
        );
        assert_eq!(dec.stale_dim_dropped(), 0);

        ring.clear();
        drop(dec);
    }

    #[test]
    fn test_tahap273_full_au_nal_scanner_detects_slices_after_aud_sei_sps_pps_avcc_and_annexb() {
        fn avcc_pack(nals: &[&[u8]], len_bytes: usize) -> Vec<u8> {
            let mut out = Vec::new();
            for n in nals {
                if len_bytes == 2 {
                    out.extend_from_slice(&(n.len() as u16).to_be_bytes());
                } else {
                    out.extend_from_slice(&(n.len() as u32).to_be_bytes());
                }
                out.extend_from_slice(n);
            }
            out
        }

        let aud = &[0x09u8, 0xF0];
        let sei = &[0x06u8, 0x05, 0x01, 0xAA, 0x80];
        let sps = &[0x67u8, 0x42, 0x80, 0x1E, 0x9A];
        let pps = &[0x68u8, 0xCE, 0x3C, 0x80];
        let idr = &[0x65u8, 0x88, 0x84];
        let p_slice = &[0x41u8, 0x9A, 0x21];
        let min_slice = &[0x41u8];

        // Non-VCL-only multi-NAL AU -> false
        let non_vcl = avcc_pack(&[aud, sei, sps, pps], 4);
        assert!(!h264_avcc_has_video_slice_nal(&non_vcl, 4));

        // Real AirPlay multi-NAL AUs -> true
        let au_idr = avcc_pack(&[aud, sei, idr], 4);
        assert!(h264_avcc_has_video_slice_nal(&au_idr, 4));
        let au_full = avcc_pack(&[aud, sei, sps, pps, p_slice], 4);
        assert!(h264_avcc_has_video_slice_nal(&au_full, 4));
        let au_min = avcc_pack(&[aud, sei, min_slice], 4);
        assert!(h264_avcc_has_video_slice_nal(&au_min, 4));

        // 2-byte length prefix support
        let len2_vcl = avcc_pack(&[aud, p_slice], 2);
        assert!(h264_avcc_has_video_slice_nal(&len2_vcl, 2));
        let len2_non_vcl = avcc_pack(&[aud, sei], 2);
        assert!(!h264_avcc_has_video_slice_nal(&len2_non_vcl, 2));

        // Annex-B multi-NAL
        let mut annex_non_vcl = Vec::new();
        annex_non_vcl.extend_from_slice(&[0, 0, 0, 1]);
        annex_non_vcl.extend_from_slice(aud);
        annex_non_vcl.extend_from_slice(&[0, 0, 0, 1]);
        annex_non_vcl.extend_from_slice(sei);
        assert!(!h264_avcc_has_video_slice_nal(&annex_non_vcl, 4));

        let mut annex_vcl = annex_non_vcl.clone();
        annex_vcl.extend_from_slice(&[0, 0, 0, 1]);
        annex_vcl.extend_from_slice(p_slice);
        assert!(h264_avcc_has_video_slice_nal(&annex_vcl, 4));

        // Opaque >=6-byte fallback when length framing isn't clean non-VCL
        assert!(h264_avcc_has_video_slice_nal(&[0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC], 4));
    }

    #[test]
    fn test_tahap273_h264_pick_nv12_keeps_mft_media_type_pure_and_preserves_bt709_color_info() {
        let gpu = Arc::new(LiveD3d11Gpu::new().expect("LiveD3d11Gpu"));
        let ring = D3d11Nv12VramRing::new();
        let mut dec = LiveMfVideoDecoder::new(gpu, VideoCodecKind::H264).expect("H264 MFT");

        dec.pick_nv12_output_type()
            .expect("pick_nv12_output_type must accept clean unmodified MFT NV12 media type");
        assert_eq!(dec.color_info.matrix, VideoColorMatrix::Bt709);

        let f128 = build_synthetic_h264_ipcm_idr(128, 128, 0x7E, 0);
        dec.update_avcc(&f128.avcc_config).expect("update_avcc 128x128");
        let _ = dec
            .feed_avcc_sample(&f128.avcc_sample, 16_667, true, &ring)
            .expect("feed 128x128 IDR");
        let _ = dec.drain_eos(true, Some((0, 0, 0, 0)), &ring).expect("drain");
        assert!(dec.last_mft_dims().is_some());
        assert_eq!(dec.color_info.matrix, VideoColorMatrix::Bt709);
        assert_eq!(dec.stale_dim_dropped(), 0);

        ring.clear();
        drop(dec);
    }

    #[test]
    fn test_tahap274_empty_dpb_waits_for_idr_without_18frame_pframe_glitch_and_clean_teardown() {
        let gpu = Arc::new(LiveD3d11Gpu::new().expect("LiveD3d11Gpu"));
        let ring = D3d11Nv12VramRing::new();
        let mut dec = LiveMfVideoDecoder::new(gpu, VideoCodecKind::H264).expect("H264 MFT");

        // 1. Decode clean initial 888x1920 Portrait IDR + 5 P_SKIP frames
        let f_port = build_synthetic_h264_ipcm_idr_cropped(888, 1920, 0x80, 0);
        assert!(dec.update_avcc(&f_port.avcc_config).expect("888x1920 avcC"));
        let mut pts = 16_667u64;
        let _ = dec
            .feed_avcc_sample(&wrap_avcc_with_aud_sei(&f_port.avcc_sample), pts, true, &ring)
            .expect("888x1920 IDR");
        drop(f_port);
        for i in 1..=5u32 {
            pts += 16_667;
            let p_au = build_synthetic_h264_p_skip_cropped_with_aud_sei(888, 1920, i);
            let _ = dec
                .feed_avcc_sample(&p_au, pts, false, &ring)
                .expect("888x1920 P_SKIP");
        }
        let st1 = dec
            .drain_eos(true, Some((0, 0, 0, 0)), &ring)
            .expect("drain 888x1920");
        assert!(st1.total_decoded_frames >= 1, "must decode portrait batch");

        // 2. Orientation switch 888x1920 -> 1920x888 recreates MFT cleanly (empty DPB)
        let f_land = build_synthetic_h264_ipcm_idr_cropped(1920, 888, 0x90, 1);
        assert!(dec.update_avcc(&f_land.avcc_config).expect("1920x888 avcC"));
        assert!(dec.awaiting_idr());
        assert!(dec.awaiting_idr_empty_dpb());
        let pre_drop_base = dec.dropped_pre_idr_frames();

        // Feed 35 stray delta P_SKIP frames:
        // NONE may reach the empty MFT DPB -> 0 emitted, 0 HRESULT errors, still awaiting IDR!
        for i in 1..=35u32 {
            pts += 16_667;
            let p_stray = build_synthetic_h264_p_skip_cropped_with_aud_sei(1920, 888, i);
            let st_drop = dec
                .feed_avcc_sample(&p_stray, pts, false, &ring)
                .expect("stray P_SKIP while empty-DPB awaits IDR");
            assert_eq!(st_drop.frames_emitted, 0);
            assert!(
                dec.awaiting_idr(),
                "empty-DPB MFT must keep awaiting_idr at stray P-frame #{i}"
            );
            assert!(dec.awaiting_idr_empty_dpb());
        }
        assert_eq!(dec.dropped_pre_idr_frames(), pre_drop_base + 35);

        // 3. Real 1920x888 IDR unlatches BOTH flags and decodes cleanly with 5 P_SKIPs
        pts += 16_667;
        let _ = dec
            .feed_avcc_sample(&wrap_avcc_with_aud_sei(&f_land.avcc_sample), pts, true, &ring)
            .expect("1920x888 IDR");
        drop(f_land);
        assert!(!dec.awaiting_idr());
        assert!(!dec.awaiting_idr_empty_dpb());
        for i in 1..=5u32 {
            pts += 16_667;
            let p_au = build_synthetic_h264_p_skip_cropped_with_aud_sei(1920, 888, i);
            let _ = dec
                .feed_avcc_sample(&p_au, pts, false, &ring)
                .expect("1920x888 P_SKIP");
        }
        let st2 = dec
            .drain_eos(true, Some((0, 0, 0, 0)), &ring)
            .expect("drain 1920x888");
        assert!(
            st2.total_decoded_frames > st1.total_decoded_frames,
            "landscape batch must decode cleanly after IDR: {:?} vs {:?}",
            st2,
            st1
        );
        assert_eq!(dec.stale_dim_dropped(), 0);
        let slot2 = ring.latest_slot().expect("latest_slot 1920x888");
        assert_eq!((slot2.visible_width(), slot2.visible_height()), (1920, 888));
        drop(slot2);

        // 4. arm_empty_dpb_idr_wait & idempotent double clean_teardown
        dec.arm_empty_dpb_idr_wait();
        assert!(dec.awaiting_idr());
        assert!(dec.awaiting_idr_empty_dpb());
        dec.clean_teardown();
        dec.clean_teardown();

        ring.clear();
        drop(dec);
    }

    #[test]
    fn test_tahap276_h264_low_latency_mode_and_8frame_250ms_vfr_failsafe_with_sps_pps_prepend() {
        let gpu = Arc::new(LiveD3d11Gpu::new().expect("LiveD3d11Gpu"));
        let ring = D3d11Nv12VramRing::new();
        let mut dec = LiveMfVideoDecoder::new(gpu, VideoCodecKind::H264).expect("H264 MFT");

        // 1. MF_LOW_LATENCY / CODECAPI_AVLowLatencyMode must be enabled cleanly on CMSH264DecoderMFT
        assert!(
            dec.mf_low_latency_enabled(),
            "CMSH264DecoderMFT must report MF_LOW_LATENCY / CODECAPI_AVLowLatencyMode enabled"
        );

        // 2. VFR sparse burst: 1 IDR + 1 P_SKIP + 60 ms pause + 1 P_SKIP -> immediate output without 4-16 frame buffering
        let f_port = build_synthetic_h264_ipcm_idr(64, 64, 0x70, 0);
        assert!(dec.update_avcc(&f_port.avcc_config).expect("64x64 avcC"));
        let mut pts = 16_667u64;
        let s0 = dec
            .feed_avcc_sample(&f_port.avcc_sample, pts, true, &ring)
            .expect("VFR frame #1 IDR");
        pts += 16_667;
        let p1 = build_synthetic_h264_p_skip_avcc_sample(64, 64, 1);
        let s1 = dec
            .feed_avcc_sample(&p1, pts, false, &ring)
            .expect("VFR frame #2 P_SKIP");
        std::thread::sleep(std::time::Duration::from_millis(60));
        pts += 60_000;
        let p2 = build_synthetic_h264_p_skip_avcc_sample(64, 64, 2);
        let s2 = dec
            .feed_avcc_sample(&p2, pts, false, &ring)
            .expect("VFR frame #3 P_SKIP after pause");
        let immediate_out = s0.frames_emitted + s1.frames_emitted + s2.frames_emitted;
        assert!(
            immediate_out >= 2,
            "VFR low-latency H.264 MFT MUST emit frames immediately without reorder-buffer stall, got {immediate_out} ({s0:?}, {s1:?}, {s2:?})"
        );

        // 3. Empty-DPB strictly waits for genuine IDR / Open-GOP I-slice (0 mid-GOP force-unlatch).
        let f_land = build_synthetic_h264_ipcm_idr(96, 64, 0x90, 1);
        assert!(dec.update_avcc(&f_land.avcc_config).expect("96x64 avcC"));
        assert!(dec.awaiting_idr());
        assert!(dec.awaiting_idr_empty_dpb());
        for idx in 1..=35u32 {
            pts += 16_667;
            let p_k = build_synthetic_h264_p_skip_avcc_sample(96, 64, idx + 10);
            let _ = dec.feed_avcc_sample(&p_k, pts, false, &ring).expect("strict hold");
            assert!(dec.awaiting_idr());
        }
        pts += 16_667;
        let _ = dec
            .feed_avcc_sample(&f_land.avcc_sample, pts, true, &ring)
            .expect("release on genuine IDR");
        assert!(!dec.awaiting_idr());
        assert!(!dec.awaiting_idr_empty_dpb());

        ring.clear();
        dec.clean_teardown();
        drop(dec);
    }

    #[test]
    fn test_tahap277_d3d11_multithread_protected_and_dxva_subresource_stride_safety() {
        let gpu = Arc::new(LiveD3d11Gpu::new().expect("LiveD3d11Gpu"));
        assert!(ensure_d3d11_multithread_protected(&gpu));
        let dec = LiveMfVideoDecoder::new(gpu, VideoCodecKind::H264).expect("H264 MFT");
        assert!(
            dec.d3d11_multithread_protected(),
            "LiveMfVideoDecoder MUST enforce ID3D10Multithread::SetMultithreadProtected(TRUE)"
        );

        let src_array = D3d11Texture2dDesc {
            width: 896,
            height: 1920,
            array_size: 17,
            ..Default::default()
        };
        let dst_single = D3d11Texture2dDesc {
            width: 896,
            height: 1920,
            array_size: 1,
            ..Default::default()
        };
        let plan_multi = resolve_dxva_subresource_copy_plan(9, &src_array, &dst_single);
        assert_eq!(plan_multi.src_subresource, 9);
        assert_eq!((plan_multi.copy_w, plan_multi.copy_h), (896, 1920));

        let plan_clamped = resolve_dxva_subresource_copy_plan(5, &dst_single, &dst_single);
        assert_eq!(plan_clamped.src_subresource, 0);

        let layout_2d = compute_nv12_cpu_plane_layout(
            888,
            1920,
            888,
            1920,
            Some(1024),
            0,
            Some(1024 * 2880),
        );
        assert_eq!(layout_2d.src_stride, 1024);
        assert_eq!(layout_2d.y_plane_rows, 1920);
        assert_eq!(layout_2d.uv_plane_offset_bytes, 1024 * 1920);
        assert_eq!(layout_2d.uv_plane_rows, 960);
        assert_eq!(layout_2d.row_copy_bytes, 888);

        // Aligned plane height > visible height (1088 vs 1080) must place UV offset at 1088*stride
        let layout_aligned = compute_nv12_cpu_plane_layout(
            1920,
            1088,
            1920,
            1080,
            None,
            1920,
            Some(1920 * (1088 * 3 / 2)),
        );
        assert_eq!(layout_aligned.src_stride, 1920);
        assert_eq!(layout_aligned.y_plane_rows, 1080);
        assert_eq!(layout_aligned.uv_plane_offset_bytes, 1920 * 1088);
    }

    #[test]
    fn test_tahap277_full_au_keyframe_parser_detects_idr_open_gop_islice_and_sei_recovery() {
        let mut au_idr = Vec::new();
        let append_nal = |buf: &mut Vec<u8>, nal: &[u8]| {
            buf.extend_from_slice(&(nal.len() as u32).to_be_bytes());
            buf.extend_from_slice(nal);
        };
        append_nal(&mut au_idr, &[0x09, 0xF0]); // AUD
        append_nal(&mut au_idr, &[0x67, 0x42, 0xC0, 0x1E]); // SPS
        append_nal(&mut au_idr, &[0x68, 0xCE, 0x3C, 0x80]); // PPS
        append_nal(&mut au_idr, &[0x0C, 0xFF, 0x80]); // Filler
        append_nal(&mut au_idr, &[0x65, 0x88, 0x84, 0x21]); // IDR
        assert!(h264_avcc_payload_is_clean_random_access(&au_idr, 4));

        // Open-GOP I-slice (slice_type=2 -> 0xBF, and slice_type=7 -> 0x88) after AUD + Filler
        let mut au_islice2 = Vec::new();
        append_nal(&mut au_islice2, &[0x09, 0xF0]);
        append_nal(&mut au_islice2, &[0x0C, 0xFF]);
        append_nal(&mut au_islice2, &[0x41, 0xBF, 0x11, 0x22]);
        assert!(h264_avcc_payload_is_clean_random_access(&au_islice2, 4));

        let mut au_islice7 = Vec::new();
        append_nal(&mut au_islice7, &[0x09, 0xF0]);
        append_nal(&mut au_islice7, &[0x41, 0x88, 0x11, 0x22]);
        assert!(h264_avcc_payload_is_clean_random_access(&au_islice7, 4));

        // SEI Recovery Point (payload_type = 6) before P-slice
        let mut au_sei = Vec::new();
        append_nal(&mut au_sei, &[0x09, 0xF0]);
        append_nal(&mut au_sei, &[0x06, 0x06, 0x01, 0x00, 0x80]);
        append_nal(&mut au_sei, &[0x41, 0xE0, 0x01, 0x22]);
        assert!(h264_avcc_payload_is_clean_random_access(&au_sei, 4));

        // Pure P-slice after AUD+Filler must return false
        let mut au_p = Vec::new();
        append_nal(&mut au_p, &[0x09, 0xF0]);
        append_nal(&mut au_p, &[0x0C, 0xFF]);
        append_nal(&mut au_p, &[0x41, 0xE0, 0x01, 0x22]);
        assert!(!h264_avcc_payload_is_clean_random_access(&au_p, 4));
    }

    #[test]
    fn test_tahap277_empty_dpb_blocks_premature_pframe_for_30pkts_1800ms_and_never_flushes_on_not_accepting() {
        assert_eq!(H264_MAX_CONSECUTIVE_INPUT_ERRORS_BEFORE_FLUSH, 5);

        let gpu = Arc::new(LiveD3d11Gpu::new().expect("LiveD3d11Gpu"));
        let ring = D3d11Nv12VramRing::new();
        let mut dec = LiveMfVideoDecoder::new(gpu, VideoCodecKind::H264).expect("H264 MFT");

        let f_init = build_synthetic_h264_ipcm_idr(64, 64, 0x7A, 0);
        assert!(dec.update_avcc(&f_init.avcc_config).expect("64x64 avcC"));
        let mut pts = 16_667u64;
        let _ = dec
            .feed_avcc_sample(&f_init.avcc_sample, pts, true, &ring)
            .expect("initial IDR");
        let idrs_after_init = dec.idr_keyframes_detected();
        assert!(idrs_after_init >= 1);

        // 4 sporadic non-fatal errors skip flush & preserve healthy DPB; P-frame right after still decodes!
        for _ in 0..4 {
            assert!(!dec.record_nonfatal_input_error_preserving_dpb(-1072873802));
        }
        assert_eq!(dec.dpb_preserved_flushes_skipped(), 4);
        assert!(!dec.awaiting_idr());
        pts += 16_667;
        let p_ok = build_synthetic_h264_p_skip_avcc_sample(64, 64, 1);
        let st_p = dec
            .feed_avcc_sample(&p_ok, pts, false, &ring)
            .expect("healthy DPB P-frame after <5 sporadic errors");
        assert_eq!(dec.consecutive_input_errors(), 0);
        assert!(st_p.total_decoded_frames >= 1);

        // Same-dim avcC never flushes or arms empty DPB wait
        assert!(!dec.update_avcc(&f_init.avcc_config).expect("same avcC"));
        assert!(!dec.awaiting_idr());

        // 5th consecutive error authorizes reset
        for _ in 0..4 {
            assert!(!dec.record_nonfatal_input_error_preserving_dpb(-1));
        }
        assert!(dec.record_nonfatal_input_error_preserving_dpb(-1));

        // Switch to 96x64 -> empty DPB blocks premature P-frames indefinitely until real IDR!
        let f_land = build_synthetic_h264_ipcm_idr(96, 64, 0x9A, 1);
        assert!(dec.update_avcc(&f_land.avcc_config).expect("96x64 avcC"));
        assert!(dec.awaiting_idr());
        assert!(dec.awaiting_idr_empty_dpb());
        for i in 1..=35u32 {
            pts += 16_667;
            let p_k = build_synthetic_h264_p_skip_avcc_sample(96, 64, i);
            let st_drop = dec.feed_avcc_sample(&p_k, pts, false, &ring).expect("blocked P");
            assert_eq!(st_drop.frames_emitted, 0);
            assert!(
                dec.awaiting_idr(),
                "empty DPB MUST NOT release premature P-frame at #{i}"
            );
        }

        // Real IDR cleanly clears awaiting_idr immediately and increments idrKeyframesDetected
        dec.arm_empty_dpb_idr_wait();
        pts += 16_667;
        let _ = dec
            .feed_avcc_sample(&f_land.avcc_sample, pts, true, &ring)
            .expect("96x64 real IDR");
        assert!(!dec.awaiting_idr());
        assert!(dec.idr_keyframes_detected() > idrs_after_init);

        ring.clear();
        dec.clean_teardown();
        drop(dec);
    }

    #[test]
    fn test_tahap279_exhaustive_avcc_256_to_511_and_1byte_zero_collision_with_annex_b() {
        let f = build_synthetic_h264_ipcm_idr_cropped(994, 2160, 0x7E, 0);
        let orig_cfg = parse_avcc_sps_pps(&f.avcc_config).expect("parse 994x2160 avcC");

        // Test exact 1-byte AVCC prefix collision (00 00 00 01 41) + all 256 lengths 256..=511 (00 00 01 xx ...)
        // Explicitly exercises all 8 fake-SPS lengths (263,295,327,359,391,423,455,487),
        // all 8 fake-IDR lengths (261,293,325,357,389,421,453,485), and 504 (real k0=#5220 len=508 B).
        let mut all_lengths = vec![1usize];
        all_lengths.extend(256usize..=511usize);
        let fake_sps_lens = [263usize, 295, 327, 359, 391, 423, 455, 487];
        let fake_idr_lens = [261usize, 293, 325, 357, 389, 421, 453, 485];
        for &l in fake_sps_lens.iter().chain(fake_idr_lens.iter()).chain([504usize].iter()) {
            assert!(all_lengths.contains(&l));
        }

        for nal_len in all_lengths {
            let mut sample = Vec::with_capacity(4 + nal_len);
            sample.extend_from_slice(&(nal_len as u32).to_be_bytes());
            sample.push(0x41); // non-IDR P-slice
            if nal_len > 1 {
                // Inject deceptive fake-SPS pattern (00 00 01 67 64 00 1E ...) inside P-slice payload
                let tail = nal_len - 1;
                for i in 0..tail {
                    let b = match i {
                        0 => 0x67,
                        1 => 0x42,
                        2 => 0x80,
                        3 => 0x1E,
                        4 => 0xDA,
                        5 => 0x8F,
                        _ => 0x99u8.wrapping_add((i & 0x3F) as u8),
                    };
                    sample.push(b);
                }
            }
            assert_eq!(sample.len(), 4 + nal_len);

            // 1. Strict AVCC walker MUST parse exact 1 NALU starting with 0x41 (NEVER 256..511 collision)
            let walked = try_walk_strict_avcc_nalus(&sample, 4)
                .unwrap_or_else(|| panic!("strict AVCC failed for nal_len={nal_len}"));
            assert_eq!(walked.len(), 1, "nal_len={nal_len}");
            assert_eq!(walked[0].len(), nal_len, "nal_len={nal_len}");
            assert_eq!(walked[0][0], 0x41, "nal_len={nal_len}");

            // 2. Annex-B conversion MUST produce exact [0, 0, 0, 1] + payload (4 + nal_len)
            let annex = avcc_sample_to_annex_b(&orig_cfg, &sample, false);
            assert_eq!(annex.len(), 4 + nal_len, "nal_len={nal_len}");
            assert_eq!(&annex[..5], &[0, 0, 0, 1, 0x41], "nal_len={nal_len}");
            assert_eq!(&annex[4..], &sample[4..], "nal_len={nal_len}");

            // 3. Inline refresh MUST return (false, false) and keep sps_list/pps_list/coded_dimensions 0-diff
            let mut cfg = orig_cfg.clone();
            let (has_idr, params_changed) = refresh_inline_avcc_params_and_detect_idr(&mut cfg, &sample);
            assert!(!has_idr, "fake IDR on nal_len={nal_len}");
            assert!(!params_changed, "fake SPS/PPS mutation on nal_len={nal_len}");
            assert_eq!(cfg.sps_list, orig_cfg.sps_list);
            assert_eq!(cfg.pps_list, orig_cfg.pps_list);
            assert_eq!(cfg.coded_dimensions, orig_cfg.coded_dimensions);

            // 4. Keyframe detector = false; video slice detector = true (skip len==1 single-byte payload check if < 2? 1-byte NAL [0x41] has length 5 >= 2!)
            assert!(
                !h264_avcc_payload_is_clean_random_access(&sample, 4),
                "false random access on nal_len={nal_len}"
            );
            assert!(
                h264_avcc_has_video_slice_nal(&sample, 4),
                "missed video slice on nal_len={nal_len}"
            );
        }

        // Real Annex-B input (that is NOT valid AVCC) still falls back cleanly in public helpers
        let real_annex = [0u8, 0, 0, 1, 0x65, 0x88, 0x84, 0x21];
        assert!(try_walk_strict_avcc_nalus(&real_annex, 4).is_none());
        assert!(h264_avcc_payload_is_clean_random_access(&real_annex, 4));
        assert!(h264_avcc_has_video_slice_nal(&real_annex, 4));
        let conv = avcc_sample_to_annex_b(&orig_cfg, &real_annex, false);
        assert_eq!(conv, real_annex);
    }

    #[test]
    fn test_tahap279_real_com_mft_decodes_994x2160_with_263_to_504b_pframes_never_drops_to_64x64() {
        let gpu = Arc::new(LiveD3d11Gpu::new().expect("LiveD3d11Gpu"));
        let ring = D3d11Nv12VramRing::new();
        let mut dec = LiveMfVideoDecoder::new(gpu, VideoCodecKind::H264).expect("H264 MFT");

        let f_idr = build_synthetic_h264_ipcm_idr_cropped(994, 2160, 0x7A, 0);
        assert!(dec.update_avcc(&f_idr.avcc_config).expect("update_avcc 994x2160"));
        assert_eq!(dec.expected_coded_dims(), Some((994, 2160)));

        let idr_au = wrap_avcc_with_aud_sei(&f_idr.avcc_sample);
        let _ = dec
            .feed_avcc_sample(&idr_au, 0, true, &ring)
            .expect("994x2160 IDR");

        // Exercise ALL notorious collision lengths (263, 295, 327, 359, 487, 504) plus normal P_SKIPs across 300 frames
        // + periodic identical pkt_k1 resend without any new IDR!
        let small_lens = [263usize, 295, 327, 359, 487, 504];
        for idx in 1..=300u32 {
            if idx % 60 == 0 {
                assert!(
                    !dec.update_avcc(&f_idr.avcc_config).expect("identical pkt_k1 resend"),
                    "identical avcC resend MUST stay NO-OP"
                );
            }
            let p_au = if idx as usize <= small_lens.len() || (idx >= 100 && ((idx - 100) as usize) < small_lens.len()) {
                let sl_idx = if idx as usize <= small_lens.len() {
                    (idx - 1) as usize
                } else {
                    (idx - 100) as usize
                };
                build_synthetic_h264_p_exact_len_avcc_sample(
                    994,
                    2160,
                    idx,
                    small_lens[sl_idx],
                )
            } else {
                build_synthetic_h264_p_skip_cropped_with_aud_sei(994, 2160, idx)
            };
            let _ = dec
                .feed_avcc_sample(&p_au, u64::from(idx) * 16_667, false, &ring)
                .unwrap_or_else(|e| panic!("P-frame #{idx} decode failed: {e:?}"));
        }
        let st = dec
            .drain_eos(true, Some((0, 0, 0, 0)), &ring)
            .expect("drain_eos 300 frames");

        assert_eq!(
            dec.last_mft_dims(),
            Some((1008, 2160)),
            "lastMftDims MUST stay 1008x2160 (NEVER 64x64 from 256..511B collision): {st:?}"
        );
        assert_eq!(
            dec.expected_coded_dims(),
            Some((994, 2160)),
            "expectedCodedDims MUST stay 994x2160 (NEVER 16x32): {st:?}"
        );
        assert_eq!(dec.mft_reconfigs(), 0, "0 spurious MFT rebuilds after init");
        assert_eq!(dec.dropped_pre_idr_frames(), 0, "0 dropped awaiting IDR");
        assert!(
            st.total_decoded_frames >= 295,
            "expected >= 295 decoded frames across 301 AUs, got {}",
            st.total_decoded_frames
        );
        assert!(
            dec.avcc_small_256_511_frames() >= 6,
            "expected >= 6 counted 256..511B frames, got {}",
            dec.avcc_small_256_511_frames()
        );
        assert_eq!(dec.last_hresult(), 0);

        ring.clear();
        dec.clean_teardown();
        drop(dec);
    }

    #[test]
    fn test_tahap279_parse_h264_sps_rejects_16x32_and_forbidden_zero_bit_and_zero_based_h264_pts() {
        // 1. Synthesize real Baseline SPS for invalid <64 dims (16x32, 32x32, 48x48) -> MUST reject!
        let make_sps = |hdr: u8, mb_w: u32, mb_h: u32| -> Vec<u8> {
            let mut bits = Vec::new();
            push_fixed_bits(&mut bits, 66, 8);
            push_fixed_bits(&mut bits, 0x80, 8);
            push_fixed_bits(&mut bits, 30, 8);
            push_ue_bits(&mut bits, 0);
            push_ue_bits(&mut bits, 0);
            push_ue_bits(&mut bits, 2);
            push_ue_bits(&mut bits, 1);
            bits.push(0);
            push_ue_bits(&mut bits, mb_w - 1);
            push_ue_bits(&mut bits, mb_h - 1);
            bits.push(1);
            bits.push(1);
            bits.push(0);
            bits.push(0);
            bits.push(1);
            let rbsp = bits_to_bytes(&mut bits);
            let mut sps = vec![hdr];
            sps.extend_from_slice(&escape_rbsp_to_nal_payload(&rbsp));
            sps
        };
        assert!(parse_h264_sps_dimensions(&make_sps(0x67, 1, 2)).is_none(), "16x32 MUST be rejected");
        assert!(parse_h264_sps_dimensions(&make_sps(0x67, 2, 2)).is_none(), "32x32 MUST be rejected");
        assert!(parse_h264_sps_dimensions(&make_sps(0x67, 3, 3)).is_none(), "48x48 MUST be rejected");
        // forbidden_zero_bit == 1 (0xE7 = 0x80 | 0x67) MUST be rejected even on >= 64x64!
        assert!(
            parse_h264_sps_dimensions(&make_sps(0xE7, 63, 135)).is_none(),
            "forbidden_zero_bit == 1 MUST be rejected"
        );
        // Valid 64x64 and 994x2160 MUST parse cleanly
        assert_eq!(parse_h264_sps_dimensions(&make_sps(0x67, 4, 4)), Some((64, 64, (0, 0, 0, 0))));
        let f994 = build_synthetic_h264_ipcm_idr_cropped(994, 2160, 0x70, 0);
        assert_eq!(
            parse_h264_sps_dimensions(&f994.sps),
            Some((1008, 2160, (0, 0, 0, 14)))
        );

        // 2. Verify 0-based H.264 PTS cadence helper & real MFT input timestamp progression
        assert_eq!(next_h264_zero_based_pts_100ns(-1), 0);
        assert_eq!(next_h264_zero_based_pts_100ns(0), 166_667);
        assert_eq!(next_h264_zero_based_pts_100ns(166_667), 333_334);

        let gpu = Arc::new(LiveD3d11Gpu::new().expect("LiveD3d11Gpu"));
        let ring = D3d11Nv12VramRing::new();
        let mut dec = LiveMfVideoDecoder::new(gpu, VideoCodecKind::H264).expect("H264 MFT");
        assert!(dec.mf_low_latency_enabled());
        dec.update_avcc(&f994.avcc_config).expect("update_avcc");
        let _ = dec
            .feed_avcc_sample(&f994.avcc_sample, 9_999_999_000, true, &ring)
            .expect("frame 0");
        assert_eq!(dec.last_input_pts_100ns(), 0, "first H.264 sample PTS MUST be 0");
        let p1 = build_synthetic_h264_p_skip_avcc_sample(994, 2160, 1);
        let _ = dec
            .feed_avcc_sample(&p1, 9_999_999_999, false, &ring)
            .expect("frame 1");
        assert_eq!(dec.last_input_pts_100ns(), FRAME_DURATION_100NS);
        let p2 = build_synthetic_h264_p_skip_avcc_sample(994, 2160, 2);
        let _ = dec
            .feed_avcc_sample(&p2, 42, false, &ring)
            .expect("frame 2 ignores erratic caller wall-clock pts_us");
        assert_eq!(dec.last_input_pts_100ns(), FRAME_DURATION_100NS * 2);

        // HEVC monotonic clamp path stays unchanged
        assert_eq!(clamp_monotonic_pts_delta_100ns(-1, 500_000), 500_000);
        assert_eq!(clamp_monotonic_pts_delta_100ns(500_000, 500_000), 500_000 + DEFAULT_PTS_DELTA_100NS);

        ring.clear();
        dec.clean_teardown();
        drop(dec);
    }

    #[test]
    fn test_tahap285_mf_decoder_old_gpu_dxva_reject_software_fallback() {
        // 1. Decision helper: ONLY retries HEVC 1x before first decoded frame; H264 is 100% untouched
        assert!(should_retry_hevc_software_mft_fallback(VideoCodecKind::Hevc, true, 0, false));
        assert!(!should_retry_hevc_software_mft_fallback(VideoCodecKind::Hevc, true, 0, true));
        assert!(!should_retry_hevc_software_mft_fallback(VideoCodecKind::Hevc, true, 1, false));
        assert!(!should_retry_hevc_software_mft_fallback(VideoCodecKind::Hevc, false, 0, false));
        assert!(!should_retry_hevc_software_mft_fallback(VideoCodecKind::H264, true, 0, false));

        // 2. Real COM MFT activation with `allow_d3d11 = false` forces software CPU path (`hardware_d3d11 == false`)
        let gpu = Arc::new(LiveD3d11Gpu::new().expect("LiveD3d11Gpu"));
        match LiveMfVideoDecoder::new_with_d3d11_option(gpu.clone(), VideoCodecKind::Hevc, false) {
            Ok(mut hevc_sw) => {
                assert!(
                    !hevc_sw.is_hardware_d3d11(),
                    "software HEVC fallback MUST skip IMFDXGIDeviceManager"
                );
                hevc_sw.clean_teardown();
            }
            Err(DecoderError::HevcExtensionMissing) => {
                // Valid on PCs without HEVC Video Extension installed
            }
            Err(other) => panic!("unexpected HEVC software activation error: {other:?}"),
        }

        // 3. H.264 Inbox MFT software activation + real decode publish into VRAM ring stays 100% intact
        let f = build_synthetic_h264_ipcm_idr(64, 64, 0x60, 0);
        let mut h264_sw = LiveMfVideoDecoder::new_with_d3d11_option(gpu, VideoCodecKind::H264, false)
            .expect("H.264 MFT SW");
        assert!(!h264_sw.is_hardware_d3d11());
        let ring = D3d11Nv12VramRing::new();
        h264_sw.update_avcc(&f.avcc_config).unwrap();
        let st = h264_sw.feed_avcc_sample(&f.avcc_sample, 0, true, &ring).unwrap();
        assert!(!st.hardware_d3d11);
        ring.clear();
        h264_sw.clean_teardown();
    }
}
