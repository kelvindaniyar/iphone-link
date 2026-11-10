//! Thread-safe AAC-ELD audio decoder wrapping zlaac.dll native FFI.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use crate::{patch_aac_eld_cookie, ZlAacLib, DEFAULT_AAC_ELD_COOKIE, MAX_FRAMES_PER_AU};

/// AAC-ELD audio decoder instance.
pub struct AacEldDecoder {
    lib: ZlAacLib,
    context: *mut std::ffi::c_void,
    sample_rate: u32,
    channels: u16,
    dll_path: PathBuf,
}

unsafe impl Send for AacEldDecoder {}
unsafe impl Sync for AacEldDecoder {}

impl AacEldDecoder {
    /// Load zlaac.dll and initialize AAC-ELD decoder context.
    pub fn new(dll_path: &Path, sample_rate: u32, channels: u16) -> Result<Self, String> {
        let lib = ZlAacLib::load(dll_path)?;
        let cookie = patch_aac_eld_cookie(&DEFAULT_AAC_ELD_COOKIE);
        let context = unsafe {
            (lib.open)(
                cookie.as_ptr(),
                cookie.len() as i32,
                sample_rate as i32,
                channels as i32,
                sample_rate as i32,
            )
        };

        if context.is_null() {
            return Err("zl_aac_open returned null context handle".to_string());
        }

        Ok(Self {
            lib,
            context,
            sample_rate,
            channels,
            dll_path: dll_path.to_path_buf(),
        })
    }

    /// Decode an AAC-ELD Access Unit payload into interleaved 16-bit PCM stereo samples.
    pub fn decode_access_unit(&mut self, payload: &[u8]) -> Result<Vec<i16>, String> {
        if payload.is_empty() {
            return Ok(Vec::new());
        }

        // Buffer sized for stereo 16-bit PCM frames
        let mut out_samples = vec![0i16; MAX_FRAMES_PER_AU * (self.channels as usize)];
        let samples_decoded = unsafe {
            (self.lib.decode)(
                self.context,
                payload.as_ptr(),
                payload.len() as i32,
                out_samples.as_mut_ptr(),
                out_samples.len() as i32,
            )
        };

        if samples_decoded < 0 {
            return Err(format!("AAC decode error code: {}", samples_decoded));
        }

        out_samples.truncate(samples_decoded as usize);
        Ok(out_samples)
    }

    /// Reset internal decoder state by reopening decoder context.
    pub fn reset(&mut self) -> Result<(), String> {
        if !self.context.is_null() {
            unsafe {
                (self.lib.close)(self.context);
            }
            self.context = std::ptr::null_mut();
        }

        let cookie = patch_aac_eld_cookie(&DEFAULT_AAC_ELD_COOKIE);
        let ctx = unsafe {
            (self.lib.open)(
                cookie.as_ptr(),
                cookie.len() as i32,
                self.sample_rate as i32,
                self.channels as i32,
                self.sample_rate as i32,
            )
        };

        if ctx.is_null() {
            return Err("Failed to re-initialize AAC-ELD decoder context on reset".to_string());
        }

        self.context = ctx;
        Ok(())
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }

    pub fn dll_path(&self) -> &Path {
        &self.dll_path
    }
}

impl Drop for AacEldDecoder {
    fn drop(&mut self) {
        if !self.context.is_null() {
            unsafe {
                (self.lib.close)(self.context);
            }
            self.context = std::ptr::null_mut();
        }
    }
}

/// Thread-safe shared wrapper for AAC-ELD decoder.
pub struct SharedAacDecoder {
    inner: Mutex<Option<AacEldDecoder>>,
}

impl Default for SharedAacDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl SharedAacDecoder {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(None),
        }
    }

    pub fn initialize(&self, dll_path: &Path, sample_rate: u32, channels: u16) -> Result<(), String> {
        let decoder = AacEldDecoder::new(dll_path, sample_rate, channels)?;
        let mut guard = self.inner.lock().map_err(|e| e.to_string())?;
        *guard = Some(decoder);
        Ok(())
    }

    pub fn is_ready(&self) -> bool {
        self.inner.lock().map(|g| g.is_some()).unwrap_or(false)
    }

    pub fn decode(&self, payload: &[u8]) -> Result<Vec<i16>, String> {
        let mut guard = self.inner.lock().map_err(|e| e.to_string())?;
        match guard.as_mut() {
            Some(decoder) => decoder.decode_access_unit(payload),
            None => Err("AAC decoder not initialized".to_string()),
        }
    }

    pub fn reset(&self) -> Result<(), String> {
        let mut guard = self.inner.lock().map_err(|e| e.to_string())?;
        if let Some(decoder) = guard.as_mut() {
            decoder.reset()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shared_decoder_initial_state() {
        let shared = SharedAacDecoder::new();
        assert!(!shared.is_ready());
        let res = shared.decode(&[0x00, 0x01]);
        assert!(res.is_err());
    }

    #[test]
    fn test_patch_cookie_logic() {
        let patched = patch_aac_eld_cookie(&DEFAULT_AAC_ELD_COOKIE);
        assert_eq!(patched[2] & 0x10, 0x10);
    }
}
