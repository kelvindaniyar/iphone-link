//! Direct3D 11 hardware context, delay-load safe Media Foundation loader,
//! IMFDXGIDeviceManager bridge, and 6-slot Latest-Frame-Wins SharedFrameRing.

use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const D3D11_SDK_VERSION: u32 = 7;
pub const D3D_DRIVER_TYPE_HARDWARE: u32 = 1;
pub const D3D_DRIVER_TYPE_WARP: u32 = 5;
pub const D3D11_CREATE_DEVICE_BGRA_SUPPORT: u32 = 0x20;
pub const D3D11_CREATE_DEVICE_VIDEO_SUPPORT: u32 = 0x800;

pub const D3D_FEATURE_LEVEL_11_0: u32 = 0xb000;
pub const D3D_FEATURE_LEVEL_11_1: u32 = 0xb100;
pub const MF_VERSION: u32 = 0x0002_0070;
pub const MFSTARTUP_NOSOCKET: u32 = 1;

pub const DXGI_FORMAT_UNKNOWN: u32 = 0;
pub const DXGI_FORMAT_R8G8_UNORM: u32 = 49;
pub const DXGI_FORMAT_R8_UNORM: u32 = 61;
pub const DXGI_FORMAT_B8G8R8A8_UNORM_VAL: u32 = 87;
pub const DXGI_FORMAT_NV12: u32 = 103;
pub const DXGI_USAGE_RENDER_TARGET_OUTPUT: u32 = 0x0000_0020;
pub const DXGI_SCALING_STRETCH: u32 = 0;
pub const DXGI_MWA_NO_ALT_ENTER: u32 = 2;

pub const D3D11_USAGE_DEFAULT: u32 = 0;
pub const D3D11_USAGE_DYNAMIC: u32 = 2;
pub const D3D11_USAGE_STAGING: u32 = 3;
pub const D3D11_BIND_CONSTANT_BUFFER: u32 = 0x4;
pub const D3D11_BIND_SHADER_RESOURCE: u32 = 0x8;
pub const D3D11_BIND_RENDER_TARGET: u32 = 0x20;
pub const D3D11_BIND_DECODER: u32 = 0x200;
pub const D3D11_SRV_DIMENSION_TEXTURE2D: u32 = 4;
pub const D3D11_CPU_ACCESS_WRITE: u32 = 0x0001_0000;
pub const D3D11_CPU_ACCESS_READ: u32 = 0x0002_0000;
pub const D3D11_MAP_READ: u32 = 1;
pub const D3D11_MAP_WRITE_DISCARD: u32 = 4;

pub const D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE: u32 = 0;
pub const D3D11_VIDEO_USAGE_PLAYBACK_NORMAL: u32 = 0;
pub const D3D11_VIDEO_USAGE_OPTIMAL_QUALITY: u32 = 2;
pub const D3D11_VPIV_DIMENSION_TEXTURE2D: u32 = 1;
pub const D3D11_VPOV_DIMENSION_TEXTURE2D: u32 = 1;
pub const D3D11_VIDEO_PROCESSOR_FEATURE_CAPS_ROTATION: u32 = 0x40;
pub const D3D11_VIDEO_PROCESSOR_ROTATION_IDENTITY: u32 = 0;
pub const D3D11_VIDEO_PROCESSOR_ROTATION_90: u32 = 1;
pub const D3D11_VIDEO_PROCESSOR_ROTATION_180: u32 = 2;
pub const D3D11_VIDEO_PROCESSOR_ROTATION_270: u32 = 3;

pub const MFT_ENUM_FLAG_SYNCMFT: u32 = 0x0000_0001;
pub const MFT_ENUM_FLAG_ASYNCMFT: u32 = 0x0000_0002;
pub const MFT_ENUM_FLAG_HARDWARE: u32 = 0x0000_0004;
pub const MFT_ENUM_FLAG_LOCALMFT: u32 = 0x0000_0010;
pub const MFT_ENUM_FLAG_SORTANDFILTER: u32 = 0x0000_0040;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Guid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

// IID_IDXGIFactory2 = {50c83a1c-e072-4c48-87b0-3630fa36a6d0}
pub const IID_IDXGI_FACTORY2: Guid = Guid {
    data1: 0x50c83a1c,
    data2: 0xe072,
    data3: 0x4c48,
    data4: [0x87, 0xb0, 0x36, 0x30, 0xfa, 0x36, 0xa6, 0xd0],
};

// IID_ID3D11Texture2D = {6f15aaf2-d208-4e89-9ab4-489535d34f9c}
pub const IID_ID3D11_TEXTURE2D: Guid = Guid {
    data1: 0x6f15aaf2,
    data2: 0xd208,
    data3: 0x4e89,
    data4: [0x9a, 0xb4, 0x48, 0x95, 0x35, 0xd3, 0x4f, 0x9c],
};

// IID_ID3D11VideoDevice = {10ec4d5b-975a-4689-b9e4-d0aac30fe333}
pub const IID_ID3D11_VIDEO_DEVICE: Guid = Guid {
    data1: 0x10ec4d5b,
    data2: 0x975a,
    data3: 0x4689,
    data4: [0xb9, 0xe4, 0xd0, 0xaa, 0xc3, 0x0f, 0xe3, 0x33],
};

// IID_ID3D11VideoContext = {61f21c45-3c0e-4a74-9cea-67100d9ad5e4}
pub const IID_ID3D11_VIDEO_CONTEXT: Guid = Guid {
    data1: 0x61f21c45,
    data2: 0x3c0e,
    data3: 0x4a74,
    data4: [0x9c, 0xea, 0x67, 0x10, 0x0d, 0x9a, 0xd5, 0xe4],
};

// IID_ID3D11VideoContext1 = {a7f026da-a5f8-4487-a564-15e34357651e}
pub const IID_ID3D11_VIDEO_CONTEXT1: Guid = Guid {
    data1: 0xa7f026da,
    data2: 0xa5f8,
    data3: 0x4487,
    data4: [0xa5, 0x64, 0x15, 0xe3, 0x43, 0x57, 0x65, 0x1e],
};

pub const DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709: u32 = 0;
pub const DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P601: u32 = 6;
pub const DXGI_COLOR_SPACE_YCBCR_FULL_G22_LEFT_P601: u32 = 7;
pub const DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709: u32 = 8;
pub const DXGI_COLOR_SPACE_YCBCR_FULL_G22_LEFT_P709: u32 = 9;
pub const DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P2020: u32 = 10;
pub const DXGI_COLOR_SPACE_YCBCR_FULL_G22_LEFT_P2020: u32 = 11;

/// Color matrix and nominal range metadata carried per frame from decoder SPS VUI / MFT attributes
/// into `D3d11Nv12VramRing` and `LiveVideoProcessor` (`ID3D11VideoContext1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoColorMatrix {
    Bt709,
    Bt601,
    Bt2020,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoColorInfo {
    pub matrix: VideoColorMatrix,
    pub full_range: bool,
}

impl Default for VideoColorInfo {
    fn default() -> Self {
        Self {
            matrix: VideoColorMatrix::Bt709,
            full_range: false,
        }
    }
}

impl VideoColorInfo {
    pub fn dxgi_ycbcr_color_space(self) -> u32 {
        match (self.matrix, self.full_range) {
            (VideoColorMatrix::Bt601, false) => DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P601,
            (VideoColorMatrix::Bt601, true) => DXGI_COLOR_SPACE_YCBCR_FULL_G22_LEFT_P601,
            (VideoColorMatrix::Bt709, false) => DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709,
            (VideoColorMatrix::Bt709, true) => DXGI_COLOR_SPACE_YCBCR_FULL_G22_LEFT_P709,
            (VideoColorMatrix::Bt2020, false) => DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P2020,
            (VideoColorMatrix::Bt2020, true) => DXGI_COLOR_SPACE_YCBCR_FULL_G22_LEFT_P2020,
        }
    }

    pub fn legacy_vp_input_color_space_flags(self) -> u32 {
        // D3D11_VIDEO_PROCESSOR_COLOR_SPACE bitfield:
        // bit 2 (0x04): YCbCr_Matrix (0 = BT.601, 1 = BT.709)
        // bits 4..5: Nominal_Range (1 = 16_235 Studio -> 0x10, 2 = 0_255 Full -> 0x20)
        let matrix_bit = match self.matrix {
            VideoColorMatrix::Bt601 => 0u32,
            VideoColorMatrix::Bt709 | VideoColorMatrix::Bt2020 => 0x04u32,
        };
        let range_bits = if self.full_range { 0x20u32 } else { 0x10u32 };
        matrix_bit | range_bits
    }

    /// Resolves `VideoColorInfo` from Media Foundation `MF_MT_YUV_MATRIX` &
    /// `MF_MT_VIDEO_NOMINAL_RANGE` (`mfobjects.h`: `1 = MFNominalRange_0_255` Full,
    /// `2 = MFNominalRange_16_235` Limited), falling back to SPS VUI metadata when Unknown (`0`).
    pub fn from_mf_attributes(
        yuv_matrix: Option<u32>,
        nominal_range: Option<u32>,
        fallback: Self,
    ) -> Self {
        let matrix = match yuv_matrix {
            Some(1) => VideoColorMatrix::Bt709,
            Some(2) => VideoColorMatrix::Bt601,
            Some(4) | Some(5) => VideoColorMatrix::Bt2020,
            _ => fallback.matrix,
        };
        let full_range = match nominal_range {
            Some(1) => true,  // MFNominalRange_Normal / MFNominalRange_0_255
            Some(2) => false, // MFNominalRange_Wide / MFNominalRange_16_235
            _ => fallback.full_range,
        };
        Self { matrix, full_range }
    }
}

// IID_IDXGIFactory1 = {770aae78-f26f-4dba-a829-253c83d1b387}
pub const IID_IDXGI_FACTORY1: Guid = Guid {
    data1: 0x770aae78,
    data2: 0xf26f,
    data3: 0x4dba,
    data4: [0xa8, 0x29, 0x25, 0x3c, 0x83, 0xd1, 0xb3, 0x87],
};

// IID_ID3D10Multithread = {9b7e4e00-342c-4106-a19f-4f2704f689f0}
pub const IID_ID3D10_MULTITHREAD: Guid = Guid {
    data1: 0x9b7e4e00,
    data2: 0x342c,
    data3: 0x4106,
    data4: [0xa1, 0x9f, 0x4f, 0x27, 0x04, 0xf6, 0x89, 0xf0],
};

// MFT_CATEGORY_VIDEO_DECODER = {d6c02d4b-6833-45b4-971a-05a4b04bab91}
pub const MFT_CATEGORY_VIDEO_DECODER: Guid = Guid {
    data1: 0xd6c02d4b,
    data2: 0x6833,
    data3: 0x45b4,
    data4: [0x97, 0x1a, 0x05, 0xa4, 0xb0, 0x4b, 0xab, 0x91],
};

// MFMediaType_Video = {73646976-0000-0010-8000-00aa00389b71}
pub const MF_MEDIA_TYPE_VIDEO: Guid = Guid {
    data1: 0x73646976,
    data2: 0x0000,
    data3: 0x0010,
    data4: [0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71],
};

// MFVideoFormat_HEVC = {43564548-0000-0010-8000-00aa00389b71} ('HEVC')
pub const MF_VIDEO_FORMAT_HEVC: Guid = Guid {
    data1: 0x43564548,
    data2: 0x0000,
    data3: 0x0010,
    data4: [0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71],
};

// MFVideoFormat_H264 = {34363248-0000-0010-8000-00aa00389b71} ('H264')
pub const MF_VIDEO_FORMAT_H264: Guid = Guid {
    data1: 0x34363248,
    data2: 0x0000,
    data3: 0x0010,
    data4: [0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71],
};

pub const HEVC_OEM_FREE_STORE_URL: &str = "ms-windows-store://pdp/?productid=9N4WGH0Z6VHQ";
pub const HEVC_PAID_FALLBACK_STORE_URL: &str = "ms-windows-store://pdp/?productid=9NMZLZ57R3T7";
pub const MEDIA_FEATURE_PACK_SETTINGS_URL: &str = "ms-settings:optionalfeatures";

#[repr(C)]
#[derive(Clone, Copy)]
pub struct MftRegisterTypeInfo {
    pub guid_major_type: Guid,
    pub guid_subtype: Guid,
}

#[repr(C)]
pub struct DxgiAdapterDesc1 {
    pub description: [u16; 128],
    pub vendor_id: u32,
    pub device_id: u32,
    pub subsys_id: u32,
    pub revision: u32,
    pub dedicated_video_memory: usize,
    pub dedicated_system_memory: usize,
    pub shared_system_memory: usize,
    pub adapter_luid_low: u32,
    pub adapter_luid_high: i32,
    pub flags: u32,
}

impl Default for DxgiAdapterDesc1 {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

type FnCreateDXGIFactory1 = unsafe extern "system" fn(*const Guid, *mut *mut c_void) -> i32;
type FnD3D11CreateDevice = unsafe extern "system" fn(
    *mut c_void, // pAdapter
    u32,         // DriverType
    *mut c_void, // Software
    u32,         // Flags
    *const u32,  // pFeatureLevels
    u32,         // FeatureLevels
    u32,         // SDKVersion
    *mut *mut c_void, // ppDevice
    *mut u32,    // pFeatureLevel
    *mut *mut c_void, // ppImmediateContext
) -> i32;
type FnMFStartup = unsafe extern "system" fn(u32, u32) -> i32;
type FnMFShutdown = unsafe extern "system" fn() -> i32;
type FnMFCreateDXGIDeviceManager = unsafe extern "system" fn(*mut u32, *mut *mut c_void) -> i32;
type FnMFTEnumEx = unsafe extern "system" fn(
    Guid,
    u32,
    *const MftRegisterTypeInfo,
    *const MftRegisterTypeInfo,
    *mut *mut *mut c_void,
    *mut u32,
) -> i32;

#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryW(lpLibFileName: *const u16) -> *mut c_void;
    fn GetProcAddress(hModule: *mut c_void, lpProcName: *const u8) -> *const c_void;
    fn FreeLibrary(hLibModule: *mut c_void) -> i32;
}

#[link(name = "ole32")]
extern "system" {
    fn CoInitializeEx(pvReserved: *mut c_void, dwCoInit: u32) -> i32;
    fn CoTaskMemFree(pv: *mut c_void);
}

unsafe fn release_com(ptr: *mut c_void) {
    if !ptr.is_null() {
        let vt = *(ptr as *const *const *const c_void);
        let rel: unsafe extern "system" fn(*mut c_void) -> u32 = std::mem::transmute(*vt.add(2));
        let _ = rel(ptr);
    }
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Delay-load safe status of Windows Media Foundation (`mfplat.dll`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MfplatProbeStatus {
    pub mfplat_loaded: bool,
    pub mfstartup_ok: bool,
    pub symbols_resolved: usize,
    pub fallback_webcodecs_ready: bool,
    pub diagnostic: String,
}

/// Probes `mfplat.dll` and all core Media Foundation procedures dynamically via `LoadLibraryW`/`GetProcAddress`.
/// Safely handles Windows 10/11 "N" editions (and simulated missing DLL) without crashing.
pub fn probe_mfplat_delay_load(simulate_missing: bool) -> MfplatProbeStatus {
    let dll_name = if simulate_missing {
        "mfplat_missing_edition_n_probe.dll"
    } else {
        "mfplat.dll"
    };
    let wide = to_wide(dll_name);
    let h_mod = unsafe { LoadLibraryW(wide.as_ptr()) };
    if h_mod.is_null() {
        return MfplatProbeStatus {
            mfplat_loaded: false,
            mfstartup_ok: false,
            symbols_resolved: 0,
            fallback_webcodecs_ready: true,
            diagnostic: "mfplat.dll unavailable (Windows N/KN edition); fallback WebCodecs ready".to_string(),
        };
    }

    let required_procs: [&[u8]; 7] = [
        b"MFStartup\0",
        b"MFShutdown\0",
        b"MFTEnumEx\0",
        b"MFCreateDXGIDeviceManager\0",
        b"MFCreateSample\0",
        b"MFCreateMemoryBuffer\0",
        b"MFCreateMediaType\0",
    ];
    let mut resolved = 0usize;
    let mut mfstartup_ok = false;
    unsafe {
        for sym in required_procs {
            if !GetProcAddress(h_mod, sym.as_ptr()).is_null() {
                resolved += 1;
            }
        }
        let p_startup = GetProcAddress(h_mod, b"MFStartup\0".as_ptr());
        let p_shutdown = GetProcAddress(h_mod, b"MFShutdown\0".as_ptr());
        if !p_startup.is_null() && !p_shutdown.is_null() {
            let startup: FnMFStartup = std::mem::transmute(p_startup);
            let shutdown: FnMFShutdown = std::mem::transmute(p_shutdown);
            if startup(MF_VERSION, 0) == 0 {
                mfstartup_ok = true;
                let _ = shutdown();
            }
        }
        FreeLibrary(h_mod);
    }

    MfplatProbeStatus {
        mfplat_loaded: true,
        mfstartup_ok,
        symbols_resolved: resolved,
        fallback_webcodecs_ready: true,
        diagnostic: if mfstartup_ok && resolved == required_procs.len() {
            "mfplat.dll delay-loaded and all 7 MF symbols active".to_string()
        } else {
            format!("mfplat.dll loaded ({resolved}/7 symbols); WebCodecs fallback active")
        },
    }
}

fn env_flag_enabled(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(|v| {
            let t = v.trim();
            t == "1" || t.eq_ignore_ascii_case("true") || t.eq_ignore_ascii_case("yes")
        })
        .unwrap_or(false)
}

/// Checks via delay-loaded `MFTEnumEx` whether any registered video decoder MFT (HW or SW)
/// supports the specified Media Foundation subtype (`MFVideoFormat_HEVC`, `MFVideoFormat_H264`, etc.).
/// Safe on Windows N/KN editions (`mfplat.dll` missing) and COM/MTA-safe.
fn probe_mfplat_decoder_mft(subtype: Guid) -> bool {
    let wide = to_wide("mfplat.dll");
    let h_mod = unsafe { LoadLibraryW(wide.as_ptr()) };
    if h_mod.is_null() {
        return false;
    }
    let mut found = false;
    unsafe {
        let _ = CoInitializeEx(std::ptr::null_mut(), 0); // COINIT_MULTITHREADED
        let p_startup = GetProcAddress(h_mod, b"MFStartup\0".as_ptr());
        let p_shutdown = GetProcAddress(h_mod, b"MFShutdown\0".as_ptr());
        let p_enum = GetProcAddress(h_mod, b"MFTEnumEx\0".as_ptr());
        if !p_startup.is_null() && !p_shutdown.is_null() && !p_enum.is_null() {
            let startup: FnMFStartup = std::mem::transmute(p_startup);
            let shutdown: FnMFShutdown = std::mem::transmute(p_shutdown);
            let mft_enum: FnMFTEnumEx = std::mem::transmute(p_enum);
            if startup(MF_VERSION, 0) == 0 {
                let input = MftRegisterTypeInfo {
                    guid_major_type: MF_MEDIA_TYPE_VIDEO,
                    guid_subtype: subtype,
                };
                let mut pp_activates: *mut *mut c_void = std::ptr::null_mut();
                let mut count = 0u32;
                let flags = MFT_ENUM_FLAG_SYNCMFT
                    | MFT_ENUM_FLAG_ASYNCMFT
                    | MFT_ENUM_FLAG_HARDWARE
                    | MFT_ENUM_FLAG_LOCALMFT
                    | MFT_ENUM_FLAG_SORTANDFILTER;
                let hr = mft_enum(
                    MFT_CATEGORY_VIDEO_DECODER,
                    flags,
                    &input,
                    std::ptr::null(),
                    &mut pp_activates,
                    &mut count,
                );
                if hr == 0 && count > 0 {
                    found = true;
                }
                if !pp_activates.is_null() {
                    let slice = std::slice::from_raw_parts_mut(pp_activates, count as usize);
                    for act in slice {
                        release_com(*act);
                    }
                    CoTaskMemFree(pp_activates as *mut c_void);
                }
                let _ = shutdown();
            }
        }
        FreeLibrary(h_mod);
    }
    found
}

/// Single-pass `mfplat.dll` + `MFTEnumEx(HEVC)` + `MFTEnumEx(H264)` probe (loads & starts MF once).
/// Returns `(mfplat_loaded, hevc_mft_available, h264_mft_available)`.
pub fn probe_mfplat_all_video_mfts() -> (bool, bool, bool) {
    if env_flag_enabled("IPHONE_LINK_MOCK_H264_MISSING") {
        return (false, false, false);
    }
    let mock_hevc_missing = env_flag_enabled("IPHONE_LINK_MOCK_HEVC_MISSING");
    let wide = to_wide("mfplat.dll");
    let h_mod = unsafe { LoadLibraryW(wide.as_ptr()) };
    if h_mod.is_null() {
        return (false, false, false);
    }
    let mut hevc_found = false;
    let mut h264_found = false;
    unsafe {
        let _ = CoInitializeEx(std::ptr::null_mut(), 0);
        let p_startup = GetProcAddress(h_mod, b"MFStartup\0".as_ptr());
        let p_shutdown = GetProcAddress(h_mod, b"MFShutdown\0".as_ptr());
        let p_enum = GetProcAddress(h_mod, b"MFTEnumEx\0".as_ptr());
        if !p_startup.is_null() && !p_shutdown.is_null() && !p_enum.is_null() {
            let startup: FnMFStartup = std::mem::transmute(p_startup);
            let shutdown: FnMFShutdown = std::mem::transmute(p_shutdown);
            let mft_enum: FnMFTEnumEx = std::mem::transmute(p_enum);
            if startup(MF_VERSION, 0) == 0 {
                let flags = MFT_ENUM_FLAG_SYNCMFT
                    | MFT_ENUM_FLAG_ASYNCMFT
                    | MFT_ENUM_FLAG_HARDWARE
                    | MFT_ENUM_FLAG_LOCALMFT
                    | MFT_ENUM_FLAG_SORTANDFILTER;
                let probe_sub = |subtype: Guid| -> bool {
                    let input = MftRegisterTypeInfo {
                        guid_major_type: MF_MEDIA_TYPE_VIDEO,
                        guid_subtype: subtype,
                    };
                    let mut pp_activates: *mut *mut c_void = std::ptr::null_mut();
                    let mut count = 0u32;
                    let hr = mft_enum(
                        MFT_CATEGORY_VIDEO_DECODER,
                        flags,
                        &input,
                        std::ptr::null(),
                        &mut pp_activates,
                        &mut count,
                    );
                    let ok = hr == 0 && count > 0;
                    if !pp_activates.is_null() {
                        let slice = std::slice::from_raw_parts_mut(pp_activates, count as usize);
                        for act in slice {
                            release_com(*act);
                        }
                        CoTaskMemFree(pp_activates as *mut c_void);
                    }
                    ok
                };
                if !mock_hevc_missing {
                    hevc_found = probe_sub(MF_VIDEO_FORMAT_HEVC);
                }
                h264_found = probe_sub(MF_VIDEO_FORMAT_H264);
                let _ = shutdown();
            }
        }
        FreeLibrary(h_mod);
    }
    (true, hevc_found, h264_found)
}

/// Probes via `MFTEnumEx` whether an HEVC video decoder MFT (HW or SW) is registered on Windows.
/// Honors diagnostic test override `IPHONE_LINK_MOCK_HEVC_MISSING=1` or `IPHONE_LINK_MOCK_H264_MISSING=1`.
pub fn probe_mfplat_hevc_decoder_mft() -> bool {
    if env_flag_enabled("IPHONE_LINK_MOCK_HEVC_MISSING")
        || env_flag_enabled("IPHONE_LINK_MOCK_H264_MISSING")
    {
        return false;
    }
    probe_mfplat_decoder_mft(MF_VIDEO_FORMAT_HEVC)
}

/// Probes via `MFTEnumEx` whether an H.264/AVC video decoder MFT is registered on Windows
/// (used to detect Windows 10/11 N/KN editions missing Media Feature Pack).
/// Honors diagnostic test override `IPHONE_LINK_MOCK_H264_MISSING=1`.
pub fn probe_mfplat_h264_decoder_mft() -> bool {
    if env_flag_enabled("IPHONE_LINK_MOCK_H264_MISSING") {
        return false;
    }
    probe_mfplat_decoder_mft(MF_VIDEO_FORMAT_H264)
}

/// Backwards-compatible alias for `probe_mfplat_hevc_decoder_mft`.
pub fn probe_mfplat_hardware_hevc_mft() -> bool {
    probe_mfplat_hevc_decoder_mft()
}

/// Pure, unit-testable summary of Windows Media Foundation + HEVC + H.264 decoder availability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodecCapabilitiesReport {
    pub mfplat_loaded: bool,
    pub hevc_mft_available: bool,
    pub h264_mft_available: bool,
    pub installed: bool,
    pub media_feature_pack_missing: bool,
    pub version: String,
    pub package: String,
    pub store_url: &'static str,
    pub store_fallback_url: &'static str,
    pub media_feature_pack_url: &'static str,
}

/// Evaluates real HEVC / H.264 / Media Feature Pack readiness with zero false positives.
/// Specifically: `mfplat_loaded == true` alone NEVER marks HEVC installed if both
/// `hevc_mft == false` and `appx_ver.is_none()`.
pub fn evaluate_codec_capabilities(
    mfplat_loaded: bool,
    hevc_mft: bool,
    h264_mft: bool,
    appx_ver: Option<&str>,
) -> CodecCapabilitiesReport {
    let media_feature_pack_missing = !mfplat_loaded || !h264_mft;
    let clean_ver = appx_ver
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let installed = mfplat_loaded && (hevc_mft || clean_ver.is_some());
    let (version, package) = if !installed {
        (String::new(), String::new())
    } else if let Some(v) = clean_ver {
        (v, "Microsoft.HEVCVideoExtension".to_string())
    } else {
        ("MFT".to_string(), "MediaFoundation.HEVC.MFT".to_string())
    };
    CodecCapabilitiesReport {
        mfplat_loaded,
        hevc_mft_available: mfplat_loaded && hevc_mft,
        h264_mft_available: mfplat_loaded && h264_mft,
        installed,
        media_feature_pack_missing,
        version,
        package,
        store_url: HEVC_OEM_FREE_STORE_URL,
        store_fallback_url: HEVC_PAID_FALLBACK_STORE_URL,
        media_feature_pack_url: MEDIA_FEATURE_PACK_SETTINGS_URL,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct D3d11HardwareInfo {
    pub adapter_name: String,
    pub feature_level: String,
    pub feature_level_raw: u32,
    pub dedicated_vram_mb: usize,
    pub hardware_device_created: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct D3d11ContextSummary {
    pub adapter_name: String,
    pub vendor_id: u32,
    pub device_id: u32,
    pub dedicated_vram_mb: usize,
    pub feature_level: String,
    pub feature_level_raw: u32,
    pub d3d11_video_support: bool,
    pub multithread_protected: bool,
    pub dxgi_device_manager_ready: bool,
    pub mfplat_loaded: bool,
    pub hardware_hevc_mft_available: bool,
    pub fallback_webcodecs_ready: bool,
}

pub struct D3d11Context;

impl D3d11Context {
    pub fn probe_video_context() -> D3d11ContextSummary {
        let mf_status = probe_mfplat_delay_load(false);
        let dxgi_wide = to_wide("dxgi.dll");
        let d3d11_wide = to_wide("d3d11.dll");
        let mfplat_wide = to_wide("mfplat.dll");

        let h_dxgi = unsafe { LoadLibraryW(dxgi_wide.as_ptr()) };
        let h_d3d11 = unsafe { LoadLibraryW(d3d11_wide.as_ptr()) };
        let h_mfplat = if mf_status.mfplat_loaded {
            unsafe { LoadLibraryW(mfplat_wide.as_ptr()) }
        } else {
            std::ptr::null_mut()
        };

        let mut adapter_name = "Unknown Adapter".to_string();
        let mut vendor_id = 0u32;
        let mut device_id = 0u32;
        let mut vram_bytes = 0usize;

        if !h_dxgi.is_null() {
            unsafe {
                let p_create_factory1 = GetProcAddress(h_dxgi, b"CreateDXGIFactory1\0".as_ptr());
                if !p_create_factory1.is_null() {
                    let create_factory1: FnCreateDXGIFactory1 = std::mem::transmute(p_create_factory1);
                    let mut p_factory: *mut c_void = std::ptr::null_mut();
                    if create_factory1(&IID_IDXGI_FACTORY1, &mut p_factory) == 0 && !p_factory.is_null() {
                        let vtable = *(p_factory as *const *const *const c_void);
                        let enum_adapters1: unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> i32 =
                            std::mem::transmute(*vtable.add(12));
                        let mut p_adapter: *mut c_void = std::ptr::null_mut();
                        if enum_adapters1(p_factory, 0, &mut p_adapter) == 0 && !p_adapter.is_null() {
                            let adapter_vt = *(p_adapter as *const *const *const c_void);
                            let get_desc1: unsafe extern "system" fn(*mut c_void, *mut DxgiAdapterDesc1) -> i32 =
                                std::mem::transmute(*adapter_vt.add(10));
                            let mut desc = DxgiAdapterDesc1::default();
                            if get_desc1(p_adapter, &mut desc) == 0 {
                                let len = desc.description.iter().position(|&c| c == 0).unwrap_or(128);
                                adapter_name = String::from_utf16_lossy(&desc.description[..len]);
                                vendor_id = desc.vendor_id;
                                device_id = desc.device_id;
                                vram_bytes = desc.dedicated_video_memory;
                            }
                            release_com(p_adapter);
                        }
                        release_com(p_factory);
                    }
                }
            }
        }

        let mut feature_level_raw = 0u32;
        let mut hw_ok = false;
        let mut mt_ok = false;
        let mut dxgi_mgr_ok = false;

        if !h_d3d11.is_null() {
            unsafe {
                let p_create_device = GetProcAddress(h_d3d11, b"D3D11CreateDevice\0".as_ptr());
                if !p_create_device.is_null() {
                    let create_device: FnD3D11CreateDevice = std::mem::transmute(p_create_device);
                    let mut p_device: *mut c_void = std::ptr::null_mut();
                    let mut p_context: *mut c_void = std::ptr::null_mut();
                    let levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
                    let flags = D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT;
                    let hr = create_device(
                        std::ptr::null_mut(),
                        D3D_DRIVER_TYPE_HARDWARE,
                        std::ptr::null_mut(),
                        flags,
                        levels.as_ptr(),
                        levels.len() as u32,
                        D3D11_SDK_VERSION,
                        &mut p_device,
                        &mut feature_level_raw,
                        &mut p_context,
                    );
                    if hr == 0 && !p_device.is_null() {
                        hw_ok = true;
                        let target_qi = if !p_context.is_null() { p_context } else { p_device };
                        let qi_vt = *(target_qi as *const *const *const c_void);
                        let qi: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> i32 =
                            std::mem::transmute(*qi_vt.add(0));
                        let mut p_mt: *mut c_void = std::ptr::null_mut();
                        if qi(target_qi, &IID_ID3D10_MULTITHREAD, &mut p_mt) == 0 && !p_mt.is_null() {
                            let mt_vt = *(p_mt as *const *const *const c_void);
                            let set_mt: unsafe extern "system" fn(*mut c_void, i32) -> i32 =
                                std::mem::transmute(*mt_vt.add(5)); // ID3D10Multithread::SetMultithreadProtected
                            let _ = set_mt(p_mt, 1);
                            mt_ok = true;
                            release_com(p_mt);
                        }

                        if !h_mfplat.is_null() {
                            let p_startup = GetProcAddress(h_mfplat, b"MFStartup\0".as_ptr());
                            let p_shutdown = GetProcAddress(h_mfplat, b"MFShutdown\0".as_ptr());
                            let p_create_mgr = GetProcAddress(h_mfplat, b"MFCreateDXGIDeviceManager\0".as_ptr());
                            if !p_startup.is_null() && !p_shutdown.is_null() && !p_create_mgr.is_null() {
                                let startup: FnMFStartup = std::mem::transmute(p_startup);
                                let shutdown: FnMFShutdown = std::mem::transmute(p_shutdown);
                                let create_mgr: FnMFCreateDXGIDeviceManager = std::mem::transmute(p_create_mgr);
                                if startup(MF_VERSION, 0) == 0 {
                                    let mut reset_token = 0u32;
                                    let mut p_mgr: *mut c_void = std::ptr::null_mut();
                                    if create_mgr(&mut reset_token, &mut p_mgr) == 0 && !p_mgr.is_null() {
                                        let mgr_vt = *(p_mgr as *const *const *const c_void);
                                        let reset_dev: unsafe extern "system" fn(*mut c_void, *mut c_void, u32) -> i32 =
                                            std::mem::transmute(*mgr_vt.add(7)); // IMFDXGIDeviceManager::ResetDevice
                                        if reset_dev(p_mgr, p_device, reset_token) == 0 {
                                            dxgi_mgr_ok = true;
                                        }
                                        release_com(p_mgr);
                                    }
                                    let _ = shutdown();
                                }
                            }
                        }

                        release_com(p_context);
                        release_com(p_device);
                    }
                }
            }
        }

        unsafe {
            if !h_mfplat.is_null() {
                FreeLibrary(h_mfplat);
            }
            if !h_d3d11.is_null() {
                FreeLibrary(h_d3d11);
            }
            if !h_dxgi.is_null() {
                FreeLibrary(h_dxgi);
            }
        }

        let feature_level = match feature_level_raw {
            0xb100 => "Direct3D 11.1 (0xb100)".to_string(),
            0xb000 => "Direct3D 11.0 (0xb000)".to_string(),
            0xa100 => "Direct3D 10.1 (0xa100)".to_string(),
            0xa000 => "Direct3D 10.0 (0xa000)".to_string(),
            other => format!("0x{:x}", other),
        };

        let mft_hevc = if mf_status.mfplat_loaded {
            probe_mfplat_hardware_hevc_mft()
        } else {
            false
        };

        D3d11ContextSummary {
            adapter_name,
            vendor_id,
            device_id,
            dedicated_vram_mb: vram_bytes / (1024 * 1024),
            feature_level,
            feature_level_raw,
            d3d11_video_support: hw_ok,
            multithread_protected: mt_ok,
            dxgi_device_manager_ready: dxgi_mgr_ok,
            mfplat_loaded: mf_status.mfplat_loaded,
            hardware_hevc_mft_available: mft_hevc,
            fallback_webcodecs_ready: mf_status.fallback_webcodecs_ready,
        }
    }
}

pub fn get_or_probe_d3d11_video_context() -> &'static D3d11ContextSummary {
    static CTX_CACHE: OnceLock<D3d11ContextSummary> = OnceLock::new();
    CTX_CACHE.get_or_init(D3d11Context::probe_video_context)
}

/// Probe real Direct3D 11 and DXGI hardware adapter on Windows.
pub fn probe_d3d11_hardware_device() -> Result<D3d11HardwareInfo, String> {
    let summary = get_or_probe_d3d11_video_context();
    if !summary.d3d11_video_support {
        return Err("D3D11 hardware video device unavailable".to_string());
    }
    Ok(D3d11HardwareInfo {
        adapter_name: summary.adapter_name.clone(),
        feature_level: summary.feature_level.clone(),
        feature_level_raw: summary.feature_level_raw,
        dedicated_vram_mb: summary.dedicated_vram_mb,
        hardware_device_created: summary.d3d11_video_support,
    })
}

/// A decoded frame slot in the shared 6-slot ring buffer (holds NV12 + BGRA staging and H.265 trim metadata).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedFrameSlot {
    pub slot_idx: usize,
    pub coded_width: u32,
    pub coded_height: u32,
    pub trim_top: u32,
    pub trim_left: u32,
    pub trim_bottom: u32,
    pub trim_right: u32,
    pub pts_us: u64,
    pub frame_seq: u64,
    pub rotation_deg: u32,
    pub timestamp: Instant,
    pub nv12_data: Vec<u8>,
    pub bgra_data: Vec<u8>,
}

impl SharedFrameSlot {
    pub fn empty(slot_idx: usize) -> Self {
        Self {
            slot_idx,
            coded_width: 0,
            coded_height: 0,
            trim_top: 0,
            trim_left: 0,
            trim_bottom: 0,
            trim_right: 0,
            pts_us: 0,
            frame_seq: 0,
            rotation_deg: 0,
            timestamp: Instant::now(),
            nv12_data: Vec::new(),
            bgra_data: Vec::new(),
        }
    }

    pub fn clean_width(&self) -> u32 {
        self.coded_width.saturating_sub(self.trim_left + self.trim_right)
    }

    pub fn clean_height(&self) -> u32 {
        self.coded_height.saturating_sub(self.trim_top + self.trim_bottom)
    }
}

/// Legacy compatibility snapshot returned by `SharedFrameRing::next()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameSlot {
    pub slot_idx: usize,
    pub width: u32,
    pub height: u32,
    pub sequence: u64,
    pub timestamp: Instant,
    pub data: Vec<u8>,
}

pub const RING_CAPACITY: usize = 6;
pub const RING_SLOT_COUNT: usize = RING_CAPACITY;
pub const MAX_RING_QUEUE: usize = 3;

#[derive(Debug)]
struct RingInner {
    slots: Vec<SharedFrameSlot>,
    queue: VecDeque<usize>,
    showing: Option<usize>,
    latest_ready_slot: Option<usize>,
    write_cursor: usize,
    next_seq: u64,
    frames_decoded: u64,
    total_dropped: u64,
}

impl Default for RingInner {
    fn default() -> Self {
        let mut slots = Vec::with_capacity(RING_CAPACITY);
        for i in 0..RING_CAPACITY {
            slots.push(SharedFrameSlot::empty(i));
        }
        Self {
            slots,
            queue: VecDeque::with_capacity(RING_CAPACITY),
            showing: None,
            latest_ready_slot: None,
            write_cursor: 0,
            next_seq: 1,
            frames_decoded: 0,
            total_dropped: 0,
        }
    }
}

/// Thread-safe Latest-Frame-Wins 6-slot video buffering ring (`RING_CAPACITY = 6`).
/// Guarantees decoder is never blocked, reuses slot buffers in-place, and exposes zero-allocation readbacks.
#[derive(Debug, Clone)]
pub struct SharedFrameRing {
    inner: Arc<Mutex<RingInner>>,
}

impl Default for SharedFrameRing {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(RingInner::default())),
        }
    }
}

impl SharedFrameRing {
    pub fn new() -> Self {
        Self::default()
    }

    /// Writes a new NV12 frame into the next free slot in-place (zero allocation after warm-up).
    /// Returns `(slot_idx, frame_seq, dropped_count)`.
    pub fn push_nv12_slot(
        &self,
        coded_width: u32,
        coded_height: u32,
        trim: (u32, u32, u32, u32),
        pts_us: u64,
        rotation_deg: u32,
        fill_nv12: impl FnOnce(&mut Vec<u8>),
    ) -> (usize, u64, u64) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let seq = g.next_seq;
        g.next_seq = g.next_seq.saturating_add(1);
        g.frames_decoded = g.frames_decoded.saturating_add(1);

        let mut target_slot = g.write_cursor % RING_CAPACITY;
        for step in 0..RING_CAPACITY {
            let cand = (g.write_cursor + step) % RING_CAPACITY;
            if g.showing != Some(cand) && !g.queue.contains(&cand) {
                target_slot = cand;
                break;
            }
        }
        g.write_cursor = (target_slot + 1) % RING_CAPACITY;

        {
            let slot = &mut g.slots[target_slot];
            slot.slot_idx = target_slot;
            slot.coded_width = coded_width;
            slot.coded_height = coded_height;
            slot.trim_top = trim.0;
            slot.trim_left = trim.1;
            slot.trim_bottom = trim.2;
            slot.trim_right = trim.3;
            slot.pts_us = pts_us;
            slot.frame_seq = seq;
            slot.rotation_deg = rotation_deg % 360;
            slot.timestamp = Instant::now();
            slot.nv12_data.clear();
            fill_nv12(&mut slot.nv12_data);
        }

        g.queue.retain(|&idx| idx != target_slot);
        g.queue.push_back(target_slot);
        g.latest_ready_slot = Some(target_slot);

        let mut dropped = 0u64;
        while g.queue.len() > MAX_RING_QUEUE {
            if let Some(stale) = g.queue.pop_front() {
                if Some(stale) != g.latest_ready_slot {
                    dropped += 1;
                    g.total_dropped += 1;
                }
            }
        }

        (target_slot, seq, dropped)
    }

    /// Reads the latest ready slot in-place under lock (`0` heap allocations).
    pub fn with_latest_slot<R>(&self, f: impl FnOnce(&SharedFrameSlot) -> R) -> Option<R> {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let idx = g.latest_ready_slot?;
        let slot = g.slots.get(idx)?;
        if slot.frame_seq == 0 {
            return None;
        }
        Some(f(slot))
    }

    /// Pushes a generic byte payload (backward compatible with legacy triple-buffer tests).
    pub fn push(&self, width: u32, height: u32, data: Vec<u8>) -> (u64, u64) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let seq = g.next_seq;
        g.next_seq = g.next_seq.saturating_add(1);
        g.frames_decoded = g.frames_decoded.saturating_add(1);

        let target_slot = (0..RING_CAPACITY)
            .find(|&i| !g.queue.contains(&i) && g.showing != Some(i))
            .unwrap_or(g.write_cursor % RING_CAPACITY);
        g.write_cursor = (target_slot + 1) % RING_CAPACITY;

        {
            let slot = &mut g.slots[target_slot];
            slot.slot_idx = target_slot;
            slot.coded_width = width;
            slot.coded_height = height;
            slot.trim_top = 0;
            slot.trim_left = 0;
            slot.trim_bottom = 0;
            slot.trim_right = 0;
            slot.frame_seq = seq;
            slot.timestamp = Instant::now();
            slot.nv12_data = data;
        }

        g.queue.retain(|&idx| idx != target_slot);
        g.queue.push_back(target_slot);
        g.latest_ready_slot = Some(target_slot);

        let mut dropped = 0u64;
        while g.queue.len() > MAX_RING_QUEUE {
            if g.queue.pop_front().is_some() {
                dropped += 1;
                g.total_dropped += 1;
            }
        }

        (seq, dropped)
    }

    /// Retrieves the newest waiting frame and discards older queued frames (Latest-Frame-Wins).
    pub fn next(&self) -> Option<FrameSlot> {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(newest_idx) = g.queue.pop_back() {
            while g.queue.pop_front().is_some() {
                g.total_dropped += 1;
            }
            g.showing = Some(newest_idx);
            let s = &g.slots[newest_idx];
            return Some(FrameSlot {
                slot_idx: s.slot_idx,
                width: s.coded_width,
                height: s.coded_height,
                sequence: s.frame_seq,
                timestamp: s.timestamp,
                data: s.nv12_data.clone(),
            });
        }
        g.showing.map(|idx| {
            let s = &g.slots[idx];
            FrameSlot {
                slot_idx: s.slot_idx,
                width: s.coded_width,
                height: s.coded_height,
                sequence: s.frame_seq,
                timestamp: s.timestamp,
                data: s.nv12_data.clone(),
            }
        })
    }

    pub fn stats_snapshot(&self) -> (u64, u64, Option<usize>, u64) {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let latest_seq = g
            .latest_ready_slot
            .and_then(|i| g.slots.get(i))
            .map(|s| s.frame_seq)
            .unwrap_or(0);
        (g.frames_decoded, g.total_dropped, g.latest_ready_slot, latest_seq)
    }

    pub fn total_dropped(&self) -> u64 {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).total_dropped
    }

    pub fn clear(&self) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.queue.clear();
        g.showing = None;
        g.latest_ready_slot = None;
        for s in g.slots.iter_mut() {
            s.frame_seq = 0;
            s.nv12_data.clear();
            s.bgra_data.clear();
        }
    }
}

// ============================================================================
// TAHAP 262 — PILAR 1: RAII ComPtr, Persistent LiveD3d11Gpu, & 6-Slot VRAM Ring
// ============================================================================

/// Win32 `RECT` compatible with D3D11 VideoProcessor & `SetWindowRgn`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DxgiSampleDesc {
    pub count: u32,
    pub quality: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DxgiRational {
    pub numerator: u32,
    pub denominator: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct D3d11Texture2dDesc {
    pub width: u32,
    pub height: u32,
    pub mip_levels: u32,
    pub array_size: u32,
    pub format: u32,
    pub sample_desc: DxgiSampleDesc,
    pub usage: u32,
    pub bind_flags: u32,
    pub cpu_access_flags: u32,
    pub misc_flags: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct D3d11Box {
    pub left: u32,
    pub top: u32,
    pub front: u32,
    pub right: u32,
    pub bottom: u32,
    pub back: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct D3d11ShaderResourceViewDesc {
    pub format: u32,
    pub view_dimension: u32,
    pub most_detailed_mip: u32,
    pub mip_levels: u32,
    pub _pad: [u32; 2],
}
const _: () = assert!(std::mem::size_of::<D3d11ShaderResourceViewDesc>() == 24);

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct D3d11MappedSubresource {
    pub p_data: *mut c_void,
    pub row_pitch: u32,
    pub depth_pitch: u32,
}

impl Default for D3d11MappedSubresource {
    fn default() -> Self {
        Self {
            p_data: std::ptr::null_mut(),
            row_pitch: 0,
            depth_pitch: 0,
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct D3d11VideoProcessorContentDesc {
    pub input_frame_format: u32,
    pub input_frame_rate: DxgiRational,
    pub input_width: u32,
    pub input_height: u32,
    pub output_frame_rate: DxgiRational,
    pub output_width: u32,
    pub output_height: u32,
    pub usage: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct D3d11VideoProcessorCaps {
    pub device_caps: u32,
    pub feature_caps: u32,
    pub filter_caps: u32,
    pub input_format_caps: u32,
    pub auto_stream_caps: u32,
    pub stereo_caps: u32,
    pub rate_conversion_caps_count: u32,
    pub max_input_streams: u32,
    pub max_stream_states: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct D3d11VideoProcessorInputViewDesc {
    pub fourcc: u32,
    pub view_dimension: u32,
    pub mip_slice: u32,
    pub array_slice: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct D3d11VideoProcessorOutputViewDesc {
    pub view_dimension: u32,
    pub mip_slice: u32,
    pub first_array_slice: u32,
    pub array_size: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct D3d11VideoProcessorStream {
    pub enable: i32,
    pub output_index: u32,
    pub input_frame_or_field: u32,
    pub past_frames: u32,
    pub future_frames: u32,
    pub pp_past_surfaces: *mut *mut c_void,
    pub p_input_surface: *mut c_void,
    pub pp_future_surfaces: *mut *mut c_void,
    pub pp_past_surfaces_right: *mut *mut c_void,
    pub p_input_surface_right: *mut c_void,
    pub pp_future_surfaces_right: *mut *mut c_void,
}

impl Default for D3d11VideoProcessorStream {
    fn default() -> Self {
        Self {
            enable: 0,
            output_index: 0,
            input_frame_or_field: 0,
            past_frames: 0,
            future_frames: 0,
            pp_past_surfaces: std::ptr::null_mut(),
            p_input_surface: std::ptr::null_mut(),
            pp_future_surfaces: std::ptr::null_mut(),
            pp_past_surfaces_right: std::ptr::null_mut(),
            p_input_surface_right: std::ptr::null_mut(),
            pp_future_surfaces_right: std::ptr::null_mut(),
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct D3d11VideoColor {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct DxgiSwapChainDesc1 {
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub stereo: i32,
    pub sample_desc: DxgiSampleDesc,
    pub buffer_usage: u32,
    pub buffer_count: u32,
    pub scaling: u32,
    pub swap_effect: u32,
    pub alpha_mode: u32,
    pub flags: u32,
}

/// Ensures COM MTA is initialized on the calling thread (`S_OK`, `S_FALSE`, or `RPC_E_CHANGED_MODE`).
pub fn ensure_com_mta_initialized() -> i32 {
    unsafe { CoInitializeEx(std::ptr::null_mut(), 0) }
}

/// Strict RAII wrapper for Windows COM interface pointers (`AddRef` on `Clone`, `Release` on `Drop`).
/// Guarantees zero COM handle / VRAM leaks across D3D11, DXGI, and Media Foundation objects.
pub struct ComPtr(*mut c_void);

// D3D11 devices/textures/contexts (with ID3D10Multithread::SetMultithreadProtected(TRUE))
// and Media Foundation MTA objects are safe to share across worker threads.
unsafe impl Send for ComPtr {}
unsafe impl Sync for ComPtr {}

impl ComPtr {
    /// Takes ownership of an already-AddRef'd COM pointer (does NOT call `AddRef`).
    ///
    /// # Safety
    /// Caller must own a valid COM reference to `ptr` (or pass null).
    pub unsafe fn from_raw(ptr: *mut c_void) -> Option<Self> {
        if ptr.is_null() {
            None
        } else {
            Some(Self(ptr))
        }
    }

    #[inline]
    pub fn as_ptr(&self) -> *mut c_void {
        self.0
    }

    /// Returns the raw vtable pointer array for this COM object.
    ///
    /// # Safety
    /// Valid as long as `self` is alive.
    #[inline]
    pub unsafe fn vtable(&self) -> *const *const c_void {
        *(self.0 as *const *const *const c_void)
    }

    /// Calls `IUnknown::QueryInterface` (`vtable[0]`) and wraps the resulting COM pointer in RAII `ComPtr`.
    pub fn query_interface(&self, iid: &Guid) -> Result<ComPtr, i32> {
        unsafe {
            let vt = self.vtable();
            let qi: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> i32 =
                std::mem::transmute(*vt.add(0));
            let mut out: *mut c_void = std::ptr::null_mut();
            let hr = qi(self.0, iid as *const Guid, &mut out);
            if hr >= 0 && !out.is_null() {
                Ok(ComPtr(out))
            } else {
                Err(hr)
            }
        }
    }
}

impl Clone for ComPtr {
    fn clone(&self) -> Self {
        unsafe {
            let vt = self.vtable();
            let add_ref: unsafe extern "system" fn(*mut c_void) -> u32 =
                std::mem::transmute(*vt.add(1));
            let _ = add_ref(self.0);
        }
        Self(self.0)
    }
}

impl Drop for ComPtr {
    fn drop(&mut self) {
        unsafe {
            release_com(self.0);
        }
    }
}

impl std::fmt::Debug for ComPtr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ComPtr({:p})", self.0)
    }
}

/// Persistent Direct3D 11 GPU device, immediate context (`ID3D10Multithread`-protected),
/// optional `ID3D11VideoDevice`/`ID3D11VideoContext`, and optional `IMFDXGIDeviceManager`
/// (delay-loaded safely via `LoadLibraryW("mfplat.dll")` for Windows N/KN compatibility).
pub struct LiveD3d11Gpu {
    device: Option<ComPtr>,
    context: Option<ComPtr>,
    video_device: Option<ComPtr>,
    video_context: Option<ComPtr>,
    video_context1: Option<ComPtr>,
    dxgi_manager: Option<ComPtr>,
    reset_token: u32,
    feature_level_raw: u32,
    is_warp: bool,
    multithread_protected: bool,
    mf_started: bool,
    h_d3d11: *mut c_void,
    h_dxgi: *mut c_void,
    h_mfplat: *mut c_void,
}

unsafe impl Send for LiveD3d11Gpu {}
unsafe impl Sync for LiveD3d11Gpu {}

impl LiveD3d11Gpu {
    /// Creates a persistent D3D11 hardware GPU context (with automatic `D3D_DRIVER_TYPE_WARP`
    /// fallback for headless/VM environments) and binds `IMFDXGIDeviceManager` when `mfplat.dll` is present.
    pub fn new() -> Result<Self, String> {
        let _ = ensure_com_mta_initialized();

        let d3d11_w = to_wide("d3d11.dll");
        let dxgi_w = to_wide("dxgi.dll");
        let mfplat_w = to_wide("mfplat.dll");

        let h_d3d11 = unsafe { LoadLibraryW(d3d11_w.as_ptr()) };
        if h_d3d11.is_null() {
            return Err("Failed to load d3d11.dll".to_string());
        }
        let h_dxgi = unsafe { LoadLibraryW(dxgi_w.as_ptr()) };
        let h_mfplat = unsafe { LoadLibraryW(mfplat_w.as_ptr()) };

        let p_create_device = unsafe { GetProcAddress(h_d3d11, b"D3D11CreateDevice\0".as_ptr()) };
        if p_create_device.is_null() {
            unsafe {
                if !h_mfplat.is_null() {
                    FreeLibrary(h_mfplat);
                }
                if !h_dxgi.is_null() {
                    FreeLibrary(h_dxgi);
                }
                FreeLibrary(h_d3d11);
            }
            return Err("D3D11CreateDevice export not found".to_string());
        }

        let create_device: FnD3D11CreateDevice = unsafe { std::mem::transmute(p_create_device) };
        let levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
        let attempts: [(u32, u32, bool); 3] = [
            (
                D3D_DRIVER_TYPE_HARDWARE,
                D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                false,
            ),
            (
                D3D_DRIVER_TYPE_WARP,
                D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                true,
            ),
            (
                D3D_DRIVER_TYPE_WARP,
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                true,
            ),
        ];

        let mut device_opt: Option<ComPtr> = None;
        let mut context_opt: Option<ComPtr> = None;
        let mut feature_level_raw = 0u32;
        let mut is_warp = false;
        let mut last_hr = 0i32;

        for (driver_type, flags, warp_flag) in attempts {
            let mut p_dev: *mut c_void = std::ptr::null_mut();
            let mut p_ctx: *mut c_void = std::ptr::null_mut();
            let mut fl = 0u32;
            let hr = unsafe {
                create_device(
                    std::ptr::null_mut(),
                    driver_type,
                    std::ptr::null_mut(),
                    flags,
                    levels.as_ptr(),
                    levels.len() as u32,
                    D3D11_SDK_VERSION,
                    &mut p_dev,
                    &mut fl,
                    &mut p_ctx,
                )
            };
            if hr >= 0 && !p_dev.is_null() && !p_ctx.is_null() {
                device_opt = unsafe { ComPtr::from_raw(p_dev) };
                context_opt = unsafe { ComPtr::from_raw(p_ctx) };
                feature_level_raw = fl;
                is_warp = warp_flag;
                break;
            }
            unsafe {
                release_com(p_ctx);
                release_com(p_dev);
            }
            last_hr = hr;
        }

        let (Some(device), Some(context)) = (device_opt, context_opt) else {
            unsafe {
                if !h_mfplat.is_null() {
                    FreeLibrary(h_mfplat);
                }
                if !h_dxgi.is_null() {
                    FreeLibrary(h_dxgi);
                }
                FreeLibrary(h_d3d11);
            }
            return Err(format!(
                "D3D11CreateDevice failed (HW & WARP), HRESULT=0x{:08X}",
                last_hr as u32
            ));
        };

        // Enable ID3D10Multithread protection so Media Foundation decoder + DXGI presenter share safely.
        let mut multithread_protected = false;
        if let Ok(mt) = context
            .query_interface(&IID_ID3D10_MULTITHREAD)
            .or_else(|_| device.query_interface(&IID_ID3D10_MULTITHREAD))
        {
            unsafe {
                let vt = mt.vtable();
                let set_mt: unsafe extern "system" fn(*mut c_void, i32) -> i32 =
                    std::mem::transmute(*vt.add(5));
                let _ = set_mt(mt.as_ptr(), 1);
            }
            multithread_protected = true;
        }

        let video_device = device.query_interface(&IID_ID3D11_VIDEO_DEVICE).ok();
        let video_context = context.query_interface(&IID_ID3D11_VIDEO_CONTEXT).ok();
        let video_context1 = context.query_interface(&IID_ID3D11_VIDEO_CONTEXT1).ok();

        let mut mf_started = false;
        let mut dxgi_manager: Option<ComPtr> = None;
        let mut reset_token = 0u32;

        if !h_mfplat.is_null() {
            unsafe {
                let p_startup = GetProcAddress(h_mfplat, b"MFStartup\0".as_ptr());
                let p_create_mgr =
                    GetProcAddress(h_mfplat, b"MFCreateDXGIDeviceManager\0".as_ptr());
                if !p_startup.is_null() && !p_create_mgr.is_null() {
                    let startup: FnMFStartup = std::mem::transmute(p_startup);
                    let create_mgr: FnMFCreateDXGIDeviceManager =
                        std::mem::transmute(p_create_mgr);
                    if startup(MF_VERSION, MFSTARTUP_NOSOCKET) >= 0 {
                        mf_started = true;
                        let mut p_mgr: *mut c_void = std::ptr::null_mut();
                        let mut tok = 0u32;
                        if create_mgr(&mut tok, &mut p_mgr) >= 0 && !p_mgr.is_null() {
                            if let Some(mgr) = ComPtr::from_raw(p_mgr) {
                                let mgr_vt = mgr.vtable();
                                let reset_dev: unsafe extern "system" fn(
                                    *mut c_void,
                                    *mut c_void,
                                    u32,
                                ) -> i32 = std::mem::transmute(*mgr_vt.add(7));
                                if reset_dev(mgr.as_ptr(), device.as_ptr(), tok) >= 0 {
                                    reset_token = tok;
                                    dxgi_manager = Some(mgr);
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(Self {
            device: Some(device),
            context: Some(context),
            video_device,
            video_context,
            video_context1,
            dxgi_manager,
            reset_token,
            feature_level_raw,
            is_warp,
            multithread_protected,
            mf_started,
            h_d3d11,
            h_dxgi,
            h_mfplat,
        })
    }

    #[inline]
    pub fn device(&self) -> &ComPtr {
        self.device.as_ref().expect("LiveD3d11Gpu device alive")
    }

    #[inline]
    pub fn context(&self) -> &ComPtr {
        self.context.as_ref().expect("LiveD3d11Gpu context alive")
    }

    #[inline]
    pub fn video_device(&self) -> Option<&ComPtr> {
        self.video_device.as_ref()
    }

    #[inline]
    pub fn video_context(&self) -> Option<&ComPtr> {
        self.video_context.as_ref()
    }

    #[inline]
    pub fn video_context1(&self) -> Option<&ComPtr> {
        self.video_context1.as_ref()
    }

    #[inline]
    pub fn dxgi_manager(&self) -> Option<&ComPtr> {
        self.dxgi_manager.as_ref()
    }

    #[inline]
    pub fn reset_token(&self) -> u32 {
        self.reset_token
    }

    #[inline]
    pub fn feature_level_raw(&self) -> u32 {
        self.feature_level_raw
    }

    #[inline]
    pub fn is_warp(&self) -> bool {
        self.is_warp
    }

    #[inline]
    pub fn is_multithread_protected(&self) -> bool {
        self.multithread_protected
    }

    #[inline]
    pub fn has_video_processor(&self) -> bool {
        self.video_device.is_some() && self.video_context.is_some()
    }

    #[inline]
    pub fn has_dxgi_manager(&self) -> bool {
        self.dxgi_manager.is_some()
    }

    /// Creates an `IDXGIFactory2` via `dxgi.dll!CreateDXGIFactory1` for `CreateSwapChainForHwnd`.
    pub fn create_dxgi_factory2(&self) -> Result<ComPtr, i32> {
        if self.h_dxgi.is_null() {
            return Err(-1);
        }
        unsafe {
            let p_create = GetProcAddress(self.h_dxgi, b"CreateDXGIFactory1\0".as_ptr());
            if p_create.is_null() {
                return Err(-1);
            }
            let create_f1: FnCreateDXGIFactory1 = std::mem::transmute(p_create);
            let mut p_fac: *mut c_void = std::ptr::null_mut();
            let hr = create_f1(&IID_IDXGI_FACTORY2, &mut p_fac);
            if hr >= 0 && !p_fac.is_null() {
                ComPtr::from_raw(p_fac).ok_or(-1)
            } else {
                Err(hr)
            }
        }
    }

    /// Creates a `DXGI_FORMAT_NV12` `ID3D11Texture2D` in VRAM (`dynamic = false`) or CPU-writable dynamic
    /// upload texture (`dynamic = true`). Automatically tries compatible bind flags across NVIDIA/AMD/Intel/WARP.
    pub fn create_nv12_texture(&self, width: u32, height: u32, dynamic: bool) -> Result<ComPtr, i32> {
        let w = (width.max(2) + 1) & !1;
        let h = (height.max(2) + 1) & !1;
        let dev = self.device();
        let vt = unsafe { dev.vtable() };
        let create_tex2d: unsafe extern "system" fn(
            *mut c_void,
            *const D3d11Texture2dDesc,
            *const c_void,
            *mut *mut c_void,
        ) -> i32 = unsafe { std::mem::transmute(*vt.add(5)) };

        let bind_candidates: &[u32] = if dynamic {
            &[D3D11_BIND_SHADER_RESOURCE, 0]
        } else {
            &[
                D3D11_BIND_RENDER_TARGET | D3D11_BIND_SHADER_RESOURCE,
                D3D11_BIND_DECODER | D3D11_BIND_SHADER_RESOURCE,
                D3D11_BIND_SHADER_RESOURCE,
                0,
            ]
        };

        let mut last_hr = -1i32;
        for &bind_flags in bind_candidates {
            let desc = D3d11Texture2dDesc {
                width: w,
                height: h,
                mip_levels: 1,
                array_size: 1,
                format: DXGI_FORMAT_NV12,
                sample_desc: DxgiSampleDesc { count: 1, quality: 0 },
                usage: if dynamic {
                    D3D11_USAGE_DYNAMIC
                } else {
                    D3D11_USAGE_DEFAULT
                },
                bind_flags,
                cpu_access_flags: if dynamic { D3D11_CPU_ACCESS_WRITE } else { 0 },
                misc_flags: 0,
            };
            let mut p_tex: *mut c_void = std::ptr::null_mut();
            let hr = unsafe { create_tex2d(dev.as_ptr(), &desc, std::ptr::null(), &mut p_tex) };
            if hr >= 0 && !p_tex.is_null() {
                if let Some(tex) = unsafe { ComPtr::from_raw(p_tex) } {
                    return Ok(tex);
                }
            }
            last_hr = hr;
        }
        Err(last_hr)
    }

    /// Creates zero-copy VRAM `(srv_y: R8_UNORM, srv_uv: R8G8_UNORM)` shader resource views
    /// for a `DXGI_FORMAT_NV12` `ID3D11Texture2D` created with `D3D11_BIND_SHADER_RESOURCE`.
    pub fn create_nv12_y_uv_srvs(&self, nv12_tex: &ComPtr) -> Result<(ComPtr, ComPtr), i32> {
        let tex_desc = Self::get_texture2d_desc(nv12_tex);
        if (tex_desc.bind_flags & D3D11_BIND_SHADER_RESOURCE) == 0 {
            return Err(-1);
        }
        let dev = self.device();
        unsafe {
            let vt = dev.vtable();
            let create_srv: unsafe extern "system" fn(
                *mut c_void,
                *mut c_void,
                *const D3d11ShaderResourceViewDesc,
                *mut *mut c_void,
            ) -> i32 = std::mem::transmute(*vt.add(7));

            let y_desc = D3d11ShaderResourceViewDesc {
                format: DXGI_FORMAT_R8_UNORM,
                view_dimension: D3D11_SRV_DIMENSION_TEXTURE2D,
                most_detailed_mip: 0,
                mip_levels: 1,
                _pad: [0; 2],
            };
            let mut p_y: *mut c_void = std::ptr::null_mut();
            let hr_y = create_srv(dev.as_ptr(), nv12_tex.as_ptr(), &y_desc, &mut p_y);
            if hr_y < 0 || p_y.is_null() {
                return Err(hr_y);
            }
            let srv_y = ComPtr::from_raw(p_y).ok_or(-1i32)?;

            let uv_desc = D3d11ShaderResourceViewDesc {
                format: DXGI_FORMAT_R8G8_UNORM,
                view_dimension: D3D11_SRV_DIMENSION_TEXTURE2D,
                most_detailed_mip: 0,
                mip_levels: 1,
                _pad: [0; 2],
            };
            let mut p_uv: *mut c_void = std::ptr::null_mut();
            let hr_uv = create_srv(dev.as_ptr(), nv12_tex.as_ptr(), &uv_desc, &mut p_uv);
            if hr_uv < 0 || p_uv.is_null() {
                return Err(hr_uv);
            }
            let srv_uv = ComPtr::from_raw(p_uv).ok_or(-1i32)?;

            Ok((srv_y, srv_uv))
        }
    }

    /// Creates a `DXGI_FORMAT_B8G8R8A8_UNORM` `ID3D11Texture2D` (for staging snapshot, 120x260 probe, or render targets).
    pub fn create_bgra_texture(
        &self,
        width: u32,
        height: u32,
        usage: u32,
        bind_flags: u32,
        cpu_access_flags: u32,
    ) -> Result<ComPtr, i32> {
        let dev = self.device();
        let desc = D3d11Texture2dDesc {
            width: width.max(1),
            height: height.max(1),
            mip_levels: 1,
            array_size: 1,
            format: DXGI_FORMAT_B8G8R8A8_UNORM_VAL,
            sample_desc: DxgiSampleDesc { count: 1, quality: 0 },
            usage,
            bind_flags,
            cpu_access_flags,
            misc_flags: 0,
        };
        unsafe {
            let vt = dev.vtable();
            let create_tex2d: unsafe extern "system" fn(
                *mut c_void,
                *const D3d11Texture2dDesc,
                *const c_void,
                *mut *mut c_void,
            ) -> i32 = std::mem::transmute(*vt.add(5));
            let mut p_tex: *mut c_void = std::ptr::null_mut();
            let hr = create_tex2d(dev.as_ptr(), &desc, std::ptr::null(), &mut p_tex);
            if hr >= 0 && !p_tex.is_null() {
                ComPtr::from_raw(p_tex).ok_or(-1)
            } else {
                Err(hr)
            }
        }
    }

    /// Reads `D3D11_TEXTURE2D_DESC` from an `ID3D11Texture2D`.
    pub fn get_texture2d_desc(tex: &ComPtr) -> D3d11Texture2dDesc {
        let mut desc = D3d11Texture2dDesc::default();
        unsafe {
            let vt = tex.vtable();
            let get_desc: unsafe extern "system" fn(*mut c_void, *mut D3d11Texture2dDesc) =
                std::mem::transmute(*vt.add(10));
            get_desc(tex.as_ptr(), &mut desc);
        }
        desc
    }

    /// `ID3D11DeviceContext::CopySubresourceRegion` (`vtable[46]`) — zero GPU→CPU copy!
    pub fn copy_subresource_region(
        &self,
        dst: &ComPtr,
        dst_sub: u32,
        dst_x: u32,
        dst_y: u32,
        src: &ComPtr,
        src_sub: u32,
        src_box: Option<&D3d11Box>,
    ) {
        let ctx = self.context();
        unsafe {
            let vt = ctx.vtable();
            let copy_fn: unsafe extern "system" fn(
                *mut c_void,
                *mut c_void,
                u32,
                u32,
                u32,
                u32,
                *mut c_void,
                u32,
                *const D3d11Box,
            ) = std::mem::transmute(*vt.add(46));
            let box_ptr = src_box.map_or(std::ptr::null(), |b| b as *const D3d11Box);
            copy_fn(
                ctx.as_ptr(),
                dst.as_ptr(),
                dst_sub,
                dst_x,
                dst_y,
                0,
                src.as_ptr(),
                src_sub,
                box_ptr,
            );
        }
    }

    /// `ID3D11DeviceContext::CopyResource` (`vtable[47]`).
    pub fn copy_resource(&self, dst: &ComPtr, src: &ComPtr) {
        let ctx = self.context();
        unsafe {
            let vt = ctx.vtable();
            let copy_res: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void) =
                std::mem::transmute(*vt.add(47));
            copy_res(ctx.as_ptr(), dst.as_ptr(), src.as_ptr());
        }
    }

    /// `ID3D11DeviceContext::UpdateSubresource` (`vtable[48]`).
    pub fn update_subresource(
        &self,
        dst: &ComPtr,
        dst_sub: u32,
        dst_box: Option<&D3d11Box>,
        src_data: *const c_void,
        src_row_pitch: u32,
        src_depth_pitch: u32,
    ) {
        let ctx = self.context();
        unsafe {
            let vt = ctx.vtable();
            let upd: unsafe extern "system" fn(
                *mut c_void,
                *mut c_void,
                u32,
                *const D3d11Box,
                *const c_void,
                u32,
                u32,
            ) = std::mem::transmute(*vt.add(48));
            let box_ptr = dst_box.map_or(std::ptr::null(), |b| b as *const D3d11Box);
            upd(
                ctx.as_ptr(),
                dst.as_ptr(),
                dst_sub,
                box_ptr,
                src_data,
                src_row_pitch,
                src_depth_pitch,
            );
        }
    }

    /// `ID3D11DeviceContext::Map` (`vtable[14]`).
    pub fn map_texture(
        &self,
        tex: &ComPtr,
        subresource: u32,
        map_type: u32,
    ) -> Result<D3d11MappedSubresource, i32> {
        let ctx = self.context();
        let mut mapped = D3d11MappedSubresource::default();
        unsafe {
            let vt = ctx.vtable();
            let map_fn: unsafe extern "system" fn(
                *mut c_void,
                *mut c_void,
                u32,
                u32,
                u32,
                *mut D3d11MappedSubresource,
            ) -> i32 = std::mem::transmute(*vt.add(14));
            let hr = map_fn(ctx.as_ptr(), tex.as_ptr(), subresource, map_type, 0, &mut mapped);
            if hr >= 0 && !mapped.p_data.is_null() {
                Ok(mapped)
            } else {
                Err(hr)
            }
        }
    }

    /// `ID3D11DeviceContext::Unmap` (`vtable[15]`).
    pub fn unmap_texture(&self, tex: &ComPtr, subresource: u32) {
        let ctx = self.context();
        unsafe {
            let vt = ctx.vtable();
            let unmap_fn: unsafe extern "system" fn(*mut c_void, *mut c_void, u32) =
                std::mem::transmute(*vt.add(15));
            unmap_fn(ctx.as_ptr(), tex.as_ptr(), subresource);
        }
    }

    /// Creates an `ID3D11RenderTargetView` (`vtable[9]`) for clearing backbuffers cleanly.
    pub fn create_rtv(&self, tex: &ComPtr) -> Result<ComPtr, i32> {
        let dev = self.device();
        unsafe {
            let vt = dev.vtable();
            let create_rtv: unsafe extern "system" fn(
                *mut c_void,
                *mut c_void,
                *const c_void,
                *mut *mut c_void,
            ) -> i32 = std::mem::transmute(*vt.add(9));
            let mut p_rtv: *mut c_void = std::ptr::null_mut();
            let hr = create_rtv(dev.as_ptr(), tex.as_ptr(), std::ptr::null(), &mut p_rtv);
            if hr >= 0 && !p_rtv.is_null() {
                ComPtr::from_raw(p_rtv).ok_or(-1)
            } else {
                Err(hr)
            }
        }
    }

    /// Clears a render target view (`ID3D11DeviceContext::ClearRenderTargetView`, `vtable[50]`).
    pub fn clear_rtv(&self, rtv: &ComPtr, rgba: [f32; 4]) {
        let ctx = self.context();
        unsafe {
            let vt = ctx.vtable();
            let clear_fn: unsafe extern "system" fn(*mut c_void, *mut c_void, *const [f32; 4]) =
                std::mem::transmute(*vt.add(50));
            clear_fn(ctx.as_ptr(), rtv.as_ptr(), &rgba as *const [f32; 4]);
        }
    }
}

impl Drop for LiveD3d11Gpu {
    fn drop(&mut self) {
        // Strictly release COM objects BEFORE MFShutdown() and FreeLibrary()!
        self.dxgi_manager = None;
        self.video_context1 = None;
        self.video_context = None;
        self.video_device = None;
        self.context = None;
        self.device = None;

        unsafe {
            if self.mf_started && !self.h_mfplat.is_null() {
                let p_shutdown = GetProcAddress(self.h_mfplat, b"MFShutdown\0".as_ptr());
                if !p_shutdown.is_null() {
                    let shutdown: FnMFShutdown = std::mem::transmute(p_shutdown);
                    let _ = shutdown();
                }
            }
            if !self.h_mfplat.is_null() {
                FreeLibrary(self.h_mfplat);
                self.h_mfplat = std::ptr::null_mut();
            }
            if !self.h_d3d11.is_null() {
                FreeLibrary(self.h_d3d11);
                self.h_d3d11 = std::ptr::null_mut();
            }
            if !self.h_dxgi.is_null() {
                FreeLibrary(self.h_dxgi);
                self.h_dxgi = std::ptr::null_mut();
            }
        }
    }
}

/// Snapshot metadata + cloned VRAM `ID3D11Texture2D` handle (`AddRef`, zero GPU copy!)
/// retrieved from `D3d11Nv12VramRing`.
#[derive(Debug, Clone)]
pub struct VramFrameSnapshot {
    pub slot_idx: usize,
    pub texture: ComPtr,
    pub coded_width: u32,
    pub coded_height: u32,
    pub visible_rect: Rect,
    pub trim: (u32, u32, u32, u32), // (top, left, bottom, right)
    pub frame_seq: u64,
    pub pts_us: u64,
    pub is_airplay: bool,
    pub color_info: VideoColorInfo,
    pub timestamp: Instant,
    pub cpu_nv12_backup: Option<Arc<Vec<u8>>>,
}

impl VramFrameSnapshot {
    pub fn visible_width(&self) -> u32 {
        (self.visible_rect.right - self.visible_rect.left).max(1) as u32
    }

    pub fn visible_height(&self) -> u32 {
        (self.visible_rect.bottom - self.visible_rect.top).max(1) as u32
    }
}

struct VramSlotEntry {
    texture: Option<ComPtr>,
    coded_width: u32,
    coded_height: u32,
    visible_rect: Rect,
    trim: (u32, u32, u32, u32),
    frame_seq: u64,
    pts_us: u64,
    is_airplay: bool,
    color_info: VideoColorInfo,
    timestamp: Instant,
    cpu_nv12_backup: Option<Arc<Vec<u8>>>,
}

impl VramSlotEntry {
    fn empty() -> Self {
        Self {
            texture: None,
            coded_width: 0,
            coded_height: 0,
            visible_rect: Rect::default(),
            trim: (0, 0, 0, 0),
            frame_seq: 0,
            pts_us: 0,
            is_airplay: false,
            color_info: VideoColorInfo::default(),
            timestamp: Instant::now(),
            cpu_nv12_backup: None,
        }
    }
}

struct VramRingInner {
    slots: Vec<VramSlotEntry>,
    latest_ready: Option<usize>,
    showing: Option<usize>,
    write_cursor: usize,
    next_seq: u64,
    frames_published: u64,
    reallocations: u64,
    last_width: u32,
    last_height: u32,
}

impl Default for VramRingInner {
    fn default() -> Self {
        let mut slots = Vec::with_capacity(RING_CAPACITY);
        for _ in 0..RING_CAPACITY {
            slots.push(VramSlotEntry::empty());
        }
        Self {
            slots,
            latest_ready: None,
            showing: None,
            write_cursor: 0,
            next_seq: 1,
            frames_published: 0,
            reallocations: 0,
            last_width: 0,
            last_height: 0,
        }
    }
}

/// 6-Slot Zero-Copy VRAM `ID3D11Texture2D` (NV12) Ring bridging `LiveMfVideoDecoder` and `LiveVideoHwnd`.
/// Supports atomic mid-stream dimension reallocation (`994x2160 ↔ 2160x994 ↔ 1216x2656`) with zero lock contention during GPU blits.
#[derive(Clone)]
pub struct D3d11Nv12VramRing {
    inner: Arc<(Mutex<VramRingInner>, Condvar)>,
}

impl Default for D3d11Nv12VramRing {
    fn default() -> Self {
        Self::new()
    }
}

impl D3d11Nv12VramRing {
    pub fn new() -> Self {
        Self {
            inner: Arc::new((Mutex::new(VramRingInner::default()), Condvar::new())),
        }
    }

    /// Acquires a free VRAM NV12 slot (`RING_CAPACITY = 6`), reallocating its `ID3D11Texture2D`
    /// atomically whenever `(coded_w, coded_h)` changes. GPU copy runs AFTER this returns (lock-free!).
    pub fn acquire_write_slot(
        &self,
        gpu: &LiveD3d11Gpu,
        coded_w: u32,
        coded_h: u32,
    ) -> Result<(usize, ComPtr), String> {
        let w = (coded_w.max(2) + 1) & !1;
        let h = (coded_h.max(2) + 1) & !1;

        let (slot_idx, needs_alloc) = {
            let mut g = self.inner.0.lock().unwrap_or_else(|e| e.into_inner());
            let mut chosen = g.write_cursor % RING_CAPACITY;
            for step in 0..RING_CAPACITY {
                let cand = (g.write_cursor + step) % RING_CAPACITY;
                if g.showing != Some(cand) && g.latest_ready != Some(cand) {
                    chosen = cand;
                    break;
                }
            }
            g.write_cursor = (chosen + 1) % RING_CAPACITY;
            let entry = &g.slots[chosen];
            let need = entry.texture.is_none() || entry.coded_width != w || entry.coded_height != h;
            if !need {
                return Ok((chosen, entry.texture.as_ref().unwrap().clone()));
            }
            (chosen, true)
        };

        if needs_alloc {
            let tex = gpu
                .create_nv12_texture(w, h, false)
                .map_err(|hr| format!("CreateTexture2D NV12 {w}x{h} failed: HRESULT=0x{:08X}", hr as u32))?;
            let mut g = self.inner.0.lock().unwrap_or_else(|e| e.into_inner());
            let entry = &mut g.slots[slot_idx];
            entry.texture = Some(tex.clone());
            entry.coded_width = w;
            entry.coded_height = h;
            g.reallocations = g.reallocations.saturating_add(1);
            return Ok((slot_idx, tex));
        }

        Err("Unreachable VRAM slot state".to_string())
    }

    /// Commits a decoded frame in `slot_idx` and wakes any waiting presenter via `Condvar`.
    #[allow(clippy::too_many_arguments)]
    pub fn commit_write_slot(
        &self,
        slot_idx: usize,
        coded_w: u32,
        coded_h: u32,
        visible_rect: Rect,
        trim: (u32, u32, u32, u32),
        pts_us: u64,
        is_airplay: bool,
        cpu_nv12_backup: Option<Vec<u8>>,
    ) -> u64 {
        self.commit_write_slot_with_color(
            slot_idx,
            coded_w,
            coded_h,
            visible_rect,
            trim,
            pts_us,
            is_airplay,
            VideoColorInfo::default(),
            cpu_nv12_backup,
        )
    }

    /// Commits a decoded frame with explicit `VideoColorInfo` (BT.709/601/2020 + Full/Limited range).
    #[allow(clippy::too_many_arguments)]
    pub fn commit_write_slot_with_color(
        &self,
        slot_idx: usize,
        coded_w: u32,
        coded_h: u32,
        visible_rect: Rect,
        trim: (u32, u32, u32, u32),
        pts_us: u64,
        is_airplay: bool,
        color_info: VideoColorInfo,
        cpu_nv12_backup: Option<Vec<u8>>,
    ) -> u64 {
        let mut g = self.inner.0.lock().unwrap_or_else(|e| e.into_inner());
        let seq = g.next_seq;
        g.next_seq = g.next_seq.saturating_add(1);
        g.frames_published = g.frames_published.saturating_add(1);
        g.last_width = coded_w;
        g.last_height = coded_h;

        if let Some(entry) = g.slots.get_mut(slot_idx) {
            entry.coded_width = coded_w;
            entry.coded_height = coded_h;
            entry.visible_rect = visible_rect;
            entry.trim = trim;
            entry.frame_seq = seq;
            entry.pts_us = pts_us;
            entry.is_airplay = is_airplay;
            entry.color_info = color_info;
            entry.timestamp = Instant::now();
            entry.cpu_nv12_backup = cpu_nv12_backup.map(Arc::new);
            g.latest_ready = Some(slot_idx);
        }
        self.inner.1.notify_all();
        seq
    }

    /// Returns the latest ready VRAM frame snapshot (`AddRef` only, zero GPU→CPU copy).
    pub fn latest_slot(&self) -> Option<VramFrameSnapshot> {
        let mut g = self.inner.0.lock().unwrap_or_else(|e| e.into_inner());
        let idx = g.latest_ready?;
        g.showing = Some(idx);
        let entry = g.slots.get(idx)?;
        let tex = entry.texture.as_ref()?.clone();
        if entry.frame_seq == 0 {
            return None;
        }
        Some(VramFrameSnapshot {
            slot_idx: idx,
            texture: tex,
            coded_width: entry.coded_width,
            coded_height: entry.coded_height,
            visible_rect: entry.visible_rect,
            trim: entry.trim,
            frame_seq: entry.frame_seq,
            pts_us: entry.pts_us,
            is_airplay: entry.is_airplay,
            color_info: entry.color_info,
            timestamp: entry.timestamp,
            cpu_nv12_backup: entry.cpu_nv12_backup.clone(),
        })
    }

    /// Waits on `Condvar` until a frame newer than `last_seq` arrives or `timeout` elapses.
    pub fn wait_for_new_frame(&self, last_seq: u64, timeout: Duration) -> Option<VramFrameSnapshot> {
        let mut g = self.inner.0.lock().unwrap_or_else(|e| e.into_inner());
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(idx) = g.latest_ready {
                let is_newer = g
                    .slots
                    .get(idx)
                    .map(|entry| entry.frame_seq > last_seq && entry.texture.is_some())
                    .unwrap_or(false);
                if is_newer {
                    g.showing = Some(idx);
                    let entry = &g.slots[idx];
                    if let Some(tex) = entry.texture.as_ref() {
                        return Some(VramFrameSnapshot {
                            slot_idx: idx,
                            texture: tex.clone(),
                            coded_width: entry.coded_width,
                            coded_height: entry.coded_height,
                            visible_rect: entry.visible_rect,
                            trim: entry.trim,
                            frame_seq: entry.frame_seq,
                            pts_us: entry.pts_us,
                            is_airplay: entry.is_airplay,
                            color_info: entry.color_info,
                            timestamp: entry.timestamp,
                            cpu_nv12_backup: entry.cpu_nv12_backup.clone(),
                        });
                    }
                }
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

    /// Wakes any thread blocked in `wait_for_new_frame` (e.g. during shutdown).
    pub fn notify_all(&self) {
        self.inner.1.notify_all();
    }

    /// Clears ready frame pointers and releases all cached VRAM textures cleanly.
    pub fn clear(&self) {
        let mut g = self.inner.0.lock().unwrap_or_else(|e| e.into_inner());
        g.latest_ready = None;
        g.showing = None;
        for entry in g.slots.iter_mut() {
            entry.texture = None;
            entry.coded_width = 0;
            entry.coded_height = 0;
            entry.frame_seq = 0;
            entry.cpu_nv12_backup = None;
        }
        self.inner.1.notify_all();
    }

    /// Returns `(frames_published, reallocations, last_width, last_height)`.
    pub fn stats(&self) -> (u64, u64, u32, u32) {
        let g = self.inner.0.lock().unwrap_or_else(|e| e.into_inner());
        (g.frames_published, g.reallocations, g.last_width, g.last_height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_d3d11_hardware_probe() {
        let res = probe_d3d11_hardware_device();
        assert!(res.is_ok(), "Probe failed: {:?}", res.err());
        let info = res.unwrap();
        assert!(info.hardware_device_created);
        assert!(!info.adapter_name.is_empty());
        assert!(info.feature_level_raw >= D3D_FEATURE_LEVEL_11_0);

        let ctx = get_or_probe_d3d11_video_context();
        assert!(ctx.d3d11_video_support);
        assert!(ctx.multithread_protected);
        assert!(ctx.mfplat_loaded);
        assert!(ctx.dxgi_device_manager_ready);
        println!("Tested hardware GPU: {} (Feature: {})", info.adapter_name, info.feature_level);
    }

    #[test]
    fn test_tahap233_delay_load_mfplat_windows_n_safe_and_6slot_wraparound() {
        let present = probe_mfplat_delay_load(false);
        assert!(present.mfplat_loaded);
        assert!(present.mfstartup_ok);
        assert_eq!(present.symbols_resolved, 7);
        assert!(present.fallback_webcodecs_ready);

        // Simulated Windows N/KN missing mfplat.dll must never panic/crash
        let missing = probe_mfplat_delay_load(true);
        assert!(!missing.mfplat_loaded);
        assert!(!missing.mfstartup_ok);
        assert!(missing.fallback_webcodecs_ready);

        // Push 20 frames into 6-slot SharedFrameRing -> latest_ready_slot must always point to frame 20
        assert_eq!(RING_CAPACITY, 6);
        let ring = SharedFrameRing::new();
        for n in 1u8..=20u8 {
            let (slot_idx, seq, _) = ring.push_nv12_slot(
                1216,
                2656,
                (0, 0, 58, 14),
                u64::from(n) * 16_667,
                0,
                |buf| {
                    buf.resize(64, n);
                },
            );
            assert!(slot_idx < RING_CAPACITY);
            assert_eq!(seq, u64::from(n));
        }

        let (decoded, _dropped, latest_slot, latest_seq) = ring.stats_snapshot();
        assert_eq!(decoded, 20);
        assert_eq!(latest_seq, 20);
        assert!(latest_slot.is_some());
        let verified = ring
            .with_latest_slot(|s| (s.frame_seq, s.clean_width(), s.clean_height(), s.nv12_data[0]))
            .unwrap();
        assert_eq!(verified, (20, 1202, 2598, 20));
    }

    #[test]
    fn test_latest_frame_wins_ring() {
        let ring = SharedFrameRing::new();

        let (seq1, _) = ring.push(1216, 2656, vec![1, 1, 1]);
        let (seq2, _) = ring.push(1216, 2656, vec![2, 2, 2]);
        let (seq3, _) = ring.push(1216, 2656, vec![3, 3, 3]);

        assert_eq!(seq1, 1);
        assert_eq!(seq2, 2);
        assert_eq!(seq3, 3);

        let latest = ring.next();
        assert!(latest.is_some());
        let frame = latest.unwrap();
        assert_eq!(frame.sequence, 3);
        assert_eq!(frame.data, vec![3, 3, 3]);
    }

    #[test]
    fn test_tahap248_m4_evaluate_codec_capabilities_zero_false_positive_and_windows_n() {
        // 1. Standard Windows with mfplat.dll + H.264 MFT, WITHOUT HEVC MFT or Appx -> MUST report installed=false (zero false positive!)
        let no_hevc = evaluate_codec_capabilities(true, false, true, None);
        assert!(no_hevc.mfplat_loaded);
        assert!(!no_hevc.hevc_mft_available);
        assert!(no_hevc.h264_mft_available);
        assert!(!no_hevc.installed, "mfplat.dll alone must NEVER report HEVC installed");
        assert!(!no_hevc.media_feature_pack_missing);
        assert_eq!(no_hevc.store_url, HEVC_OEM_FREE_STORE_URL);
        assert!(no_hevc.store_url.contains("9N4WGH0Z6VHQ"));
        assert!(no_hevc.store_fallback_url.contains("9NMZLZ57R3T7"));

        // 2. Windows 10/11 N/KN Edition (mfplat.dll missing or H.264 MFT missing) -> media_feature_pack_missing=true
        let win_n_no_dll = evaluate_codec_capabilities(false, false, false, None);
        assert!(!win_n_no_dll.installed);
        assert!(win_n_no_dll.media_feature_pack_missing);
        assert_eq!(win_n_no_dll.media_feature_pack_url, MEDIA_FEATURE_PACK_SETTINGS_URL);

        let win_n_no_h264 = evaluate_codec_capabilities(true, false, false, None);
        assert!(!win_n_no_h264.installed);
        assert!(win_n_no_h264.media_feature_pack_missing);

        // 3. PC with real HEVC MFT (HW or SW) -> installed=true
        let hevc_mft_only = evaluate_codec_capabilities(true, true, true, None);
        assert!(hevc_mft_only.installed);
        assert!(hevc_mft_only.hevc_mft_available);
        assert!(!hevc_mft_only.media_feature_pack_missing);

        // 4. PC with Appx + HEVC MFT -> installed=true with version preserved
        let hevc_appx = evaluate_codec_capabilities(true, true, true, Some("2.5.48.0"));
        assert!(hevc_appx.installed);
        assert_eq!(hevc_appx.version, "2.5.48.0");
    }

    #[test]
    fn test_tahap262_live_d3d11_gpu_persistent_and_vram_ring_reallocation_zero_leak() {
        let gpu = Arc::new(LiveD3d11Gpu::new().expect("LiveD3d11Gpu must initialize on Windows"));
        assert!(gpu.is_multithread_protected(), "ID3D10Multithread must be enabled");
        assert!(gpu.feature_level_raw() >= D3D_FEATURE_LEVEL_11_0);
        assert!(gpu.has_dxgi_manager(), "IMFDXGIDeviceManager must bind cleanly");

        let ring = D3d11Nv12VramRing::new();
        let dims = [
            (994u32, 2160u32, true, (0u32, 0u32, 0u32, 0u32)),
            (2160u32, 994u32, true, (0u32, 0u32, 0u32, 0u32)),
            (1216u32, 2656u32, false, (0u32, 0u32, 58u32, 14u32)),
        ];

        let mut expected_seq = 0u64;
        for (w, h, is_airplay, trim) in dims {
            for i in 0..8u64 {
                let (slot_idx, tex) = ring
                    .acquire_write_slot(&gpu, w, h)
                    .expect("acquire VRAM NV12 slot");
                assert!(slot_idx < RING_CAPACITY);
                let desc = LiveD3d11Gpu::get_texture2d_desc(&tex);
                assert_eq!(desc.format, DXGI_FORMAT_NV12);
                assert_eq!(desc.width, (w + 1) & !1);
                assert_eq!(desc.height, (h + 1) & !1);

                let vis = Rect {
                    left: 0,
                    top: 0,
                    right: w as i32,
                    bottom: h as i32,
                };
                let seq = ring.commit_write_slot(
                    slot_idx,
                    w,
                    h,
                    vis,
                    trim,
                    (expected_seq + 1) * 16_667,
                    is_airplay,
                    None,
                );
                expected_seq += 1;
                assert_eq!(seq, expected_seq);
                if i == 7 {
                    let latest = ring.latest_slot().expect("latest VRAM slot present");
                    assert_eq!(latest.frame_seq, expected_seq);
                    assert_eq!(latest.coded_width, w);
                    assert_eq!(latest.coded_height, h);
                    assert_eq!(latest.is_airplay, is_airplay);
                    assert_eq!(latest.trim, trim);
                }
            }
        }

        let (published, reallocs, last_w, last_h) = ring.stats();
        assert_eq!(published, 24);
        assert!(reallocs >= 3, "expected VRAM reallocation across 3 resolutions, got {reallocs}");
        assert_eq!((last_w, last_h), (1216, 2656));

        ring.clear();
        assert!(ring.latest_slot().is_none());
        drop(ring);
        drop(gpu);
    }
}
