//! AAC-ELD audio decoder FFI bindings for zlaac.dll, Zero Double Audio controller, and WASAPI jitter buffer.

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryW(lpLibFileName: *const u16) -> *mut c_void;
    fn GetProcAddress(hModule: *mut c_void, lpProcName: *const u8) -> *const c_void;
    fn FreeLibrary(hLibModule: *mut c_void) -> i32;
}

pub type ZlAacOpen = unsafe extern "C" fn(*const u8, i32, i32, i32, i32) -> *mut c_void;
pub type ZlAacDecode = unsafe extern "C" fn(*mut c_void, *const u8, i32, *mut i16, i32) -> i32;
pub type ZlAacClose = unsafe extern "C" fn(*mut c_void);
pub type ZlAacVersion = unsafe extern "C" fn() -> u32;

pub const DEFAULT_AAC_ELD_COOKIE: [u8; 4] = [0xF8, 0xE8, 0x50, 0x00];
pub const MAX_FRAMES_PER_AU: usize = 8192;

/// Patch Apple AudioSpecificConfig cookie with frameLengthFlag bit 0x10.
pub fn patch_aac_eld_cookie(raw: &[u8]) -> Vec<u8> {
    let mut cookie = if raw.is_empty() {
        DEFAULT_AAC_ELD_COOKIE.to_vec()
    } else {
        raw.to_vec()
    };
    if cookie.len() >= 3 {
        cookie[2] |= 0x10;
    }
    cookie
}

/// Convert Path to null-terminated UTF-16 wide string for Win32 API.
fn to_wide(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// Native dynamic loader and wrapper for zlaac.dll.
pub struct ZlAacLib {
    h_module: *mut c_void,
    pub open: ZlAacOpen,
    pub decode: ZlAacDecode,
    pub close: ZlAacClose,
    pub version: ZlAacVersion,
}

unsafe impl Send for ZlAacLib {}
unsafe impl Sync for ZlAacLib {}

impl ZlAacLib {
    pub fn load(dll_path: &Path) -> Result<Self, String> {
        if !dll_path.exists() {
            return Err(format!("zlaac.dll not found at: {}", dll_path.display()));
        }
        let wide = to_wide(dll_path);
        let h_module = unsafe { LoadLibraryW(wide.as_ptr()) };
        if h_module.is_null() {
            return Err(format!("Failed to load library: {}", dll_path.display()));
        }

        unsafe {
            let open_ptr = GetProcAddress(h_module, b"zl_aac_open\0".as_ptr());
            let decode_ptr = GetProcAddress(h_module, b"zl_aac_decode\0".as_ptr());
            let close_ptr = GetProcAddress(h_module, b"zl_aac_close\0".as_ptr());
            let ver_ptr = GetProcAddress(h_module, b"zl_aac_avcodec_version\0".as_ptr());

            if open_ptr.is_null() || decode_ptr.is_null() || close_ptr.is_null() || ver_ptr.is_null() {
                FreeLibrary(h_module);
                return Err("Failed to resolve one or more zlaac.dll export symbols".to_string());
            }

            Ok(Self {
                h_module,
                open: std::mem::transmute(open_ptr),
                decode: std::mem::transmute(decode_ptr),
                close: std::mem::transmute(close_ptr),
                version: std::mem::transmute(ver_ptr),
            })
        }
    }
}

impl Drop for ZlAacLib {
    fn drop(&mut self) {
        if !self.h_module.is_null() {
            unsafe {
                FreeLibrary(self.h_module);
            }
            self.h_module = std::ptr::null_mut();
        }
    }
}

/// Probe zlaac.dll: load, call avcodec_version, open AAC-ELD decoder, close it, and return version.
pub fn probe_zlaac_dll(dll_path: &Path) -> Result<u32, String> {
    let lib = ZlAacLib::load(dll_path)?;
    let ver = unsafe { (lib.version)() };

    let cookie = patch_aac_eld_cookie(&DEFAULT_AAC_ELD_COOKIE);
    let ctx = unsafe { (lib.open)(cookie.as_ptr(), cookie.len() as i32, 48000, 2, 48000) };
    if ctx.is_null() {
        return Err("zl_aac_open returned null handle for AAC-ELD cookie".to_string());
    }
    unsafe {
        (lib.close)(ctx);
    }
    Ok(ver)
}

/// Zero Double Audio controller: Auto-mute speaker fisik iPhone saat stream aktif,
/// dan restore volume saat streaming berhenti.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZeroDoubleAudioController;

impl ZeroDoubleAudioController {
    pub const MUTE_USAGE_CODE: u16 = 0x00EA;    // Volume Decrement
    pub const RESTORE_USAGE_CODE: u16 = 0x00E9; // Volume Increment
    pub const MUTE_STEP_COUNT: usize = 16;
    pub const RESTORE_STEP_COUNT: usize = 5;

    /// Return 16 volume decrement usages to mute device hardware speaker.
    pub fn on_stream_start() -> Vec<u16> {
        vec![Self::MUTE_USAGE_CODE; Self::MUTE_STEP_COUNT]
    }

    /// Return 5 volume increment usages to restore audible speaker volume.
    pub fn on_stream_stop() -> Vec<u16> {
        vec![Self::RESTORE_USAGE_CODE; Self::RESTORE_STEP_COUNT]
    }
}

/// Buffer preset latency configuration for WASAPI jitter buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioBufferPreset {
    Low,    // ~30ms latency (Gaming, Responsive)
    Normal, // ~60ms latency (Standard Mirroring)
    High,   // ~120ms latency (High Jitter Network)
}

impl AudioBufferPreset {
    pub fn latency_ms(&self) -> u32 {
        match self {
            Self::Low => 30,
            Self::Normal => 60,
            Self::High => 120,
        }
    }
}

/// WASAPI configuration profile.
#[derive(Debug, Clone)]
pub struct WasapiAudioConfig {
    pub preset: AudioBufferPreset,
    pub lip_sync_enabled: bool,
    pub sample_rate: u32,
    pub channels: u16,
}

impl Default for WasapiAudioConfig {
    fn default() -> Self {
        Self {
            preset: AudioBufferPreset::Normal,
            lip_sync_enabled: true,
            sample_rate: 48000,
            channels: 2,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_patch_cookie() {
        let cookie = patch_aac_eld_cookie(&[0xF8, 0xE6, 0x40, 0x00]);
        assert_eq!(cookie[2], 0x50); // 0x40 | 0x10 = 0x50
    }

    #[test]
    fn test_zero_double_audio() {
        let mute = ZeroDoubleAudioController::on_stream_start();
        assert_eq!(mute.len(), 16);
        assert_eq!(mute[0], 0x00EA);

        let restore = ZeroDoubleAudioController::on_stream_stop();
        assert_eq!(restore.len(), 5);
        assert_eq!(restore[0], 0x00E9);
    }

    #[test]
    fn test_buffer_presets() {
        assert_eq!(AudioBufferPreset::Low.latency_ms(), 30);
        assert_eq!(AudioBufferPreset::Normal.latency_ms(), 60);
        assert_eq!(AudioBufferPreset::High.latency_ms(), 120);
    }
}
