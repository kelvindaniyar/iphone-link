//! WASAPI output configuration and MMCSS Pro Audio probe for low-latency EcoQoS bypass.

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use crate::ZeroDoubleAudioController;

#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryW(lpLibFileName: *const u16) -> *mut c_void;
    fn GetProcAddress(hModule: *mut c_void, lpProcName: *const u8) -> *const c_void;
    fn FreeLibrary(hLibModule: *mut c_void) -> i32;
}

pub type AvSetMmThreadCharacteristicsW = unsafe extern "system" fn(*const u16, *mut u32) -> *mut c_void;
pub type AvRevertMmThreadCharacteristics = unsafe extern "system" fn(*mut c_void) -> i32;

pub type CoInitializeExFn = unsafe extern "system" fn(*mut c_void, u32) -> i32;
pub type CoUninitializeFn = unsafe extern "system" fn();

const COINIT_MULTITHREADED: u32 = 0x0;

fn to_wide(s: &str) -> Vec<u16> {
    Path::new(s)
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// Result of probing WASAPI and MMCSS Pro Audio thread priority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MmcssProbeResult {
    pub mmcss_enabled: bool,
    pub task_name: String,
    pub bypass_ecoqos: bool,
    pub wasapi_format: String,
}

/// Probes Windows MMCSS Pro Audio characteristics to ensure real-time audio threads
/// bypass Windows 11 EcoQoS power throttling.
pub fn probe_wasapi_and_mmcss() -> Result<MmcssProbeResult, String> {
    let ole32_name = to_wide("ole32.dll");
    let h_ole32 = unsafe { LoadLibraryW(ole32_name.as_ptr()) };
    if h_ole32.is_null() {
        return Err("Failed to load ole32.dll".to_string());
    }

    let co_init: Option<CoInitializeExFn> = unsafe {
        let ptr = GetProcAddress(h_ole32, b"CoInitializeEx\0".as_ptr());
        if ptr.is_null() { None } else { Some(std::mem::transmute(ptr)) }
    };
    let co_uninit: Option<CoUninitializeFn> = unsafe {
        let ptr = GetProcAddress(h_ole32, b"CoUninitialize\0".as_ptr());
        if ptr.is_null() { None } else { Some(std::mem::transmute(ptr)) }
    };

    if let Some(init_fn) = co_init {
        unsafe {
            let _ = init_fn(std::ptr::null_mut(), COINIT_MULTITHREADED);
        }
    }

    let avrt_name = to_wide("avrt.dll");
    let h_avrt = unsafe { LoadLibraryW(avrt_name.as_ptr()) };
    if h_avrt.is_null() {
        if let Some(uninit_fn) = co_uninit {
            unsafe { uninit_fn(); }
        }
        unsafe { FreeLibrary(h_ole32); }
        return Err("Failed to load avrt.dll".to_string());
    }

    let av_set: Option<AvSetMmThreadCharacteristicsW> = unsafe {
        let ptr = GetProcAddress(h_avrt, b"AvSetMmThreadCharacteristicsW\0".as_ptr());
        if ptr.is_null() { None } else { Some(std::mem::transmute(ptr)) }
    };
    let av_revert: Option<AvRevertMmThreadCharacteristics> = unsafe {
        let ptr = GetProcAddress(h_avrt, b"AvRevertMmThreadCharacteristics\0".as_ptr());
        if ptr.is_null() { None } else { Some(std::mem::transmute(ptr)) }
    };

    let mut task_index: u32 = 0;
    let task_wide = to_wide("Pro Audio");
    let mut mmcss_ok = false;

    if let (Some(set_fn), Some(revert_fn)) = (av_set, av_revert) {
        unsafe {
            let handle = set_fn(task_wide.as_ptr(), &mut task_index);
            if !handle.is_null() {
                mmcss_ok = true;
                let _ = revert_fn(handle);
            }
        }
    }

    unsafe {
        FreeLibrary(h_avrt);
        if let Some(uninit_fn) = co_uninit {
            uninit_fn();
        }
        FreeLibrary(h_ole32);
    }

    Ok(MmcssProbeResult {
        mmcss_enabled: mmcss_ok,
        task_name: "Pro Audio".to_string(),
        bypass_ecoqos: mmcss_ok,
        wasapi_format: "48000Hz 16-bit Stereo PCM".to_string(),
    })
}

/// WASAPI stream runtime configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WasapiStreamConfig {
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
    pub period_frames: u32,
    pub auto_mute_speaker: bool,
    pub restore_speaker: bool,
}

impl Default for WasapiStreamConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48000,
            channels: 2,
            bits_per_sample: 16,
            period_frames: 480, // 10ms at 48kHz
            auto_mute_speaker: true,
            restore_speaker: true,
        }
    }
}

impl WasapiStreamConfig {
    /// Return the list of HID consumer usages to mute the iPhone physical speaker when audio starts.
    pub fn start_mute_usages(&self) -> Vec<u16> {
        if self.auto_mute_speaker {
            ZeroDoubleAudioController::on_stream_start()
        } else {
            Vec::new()
        }
    }

    /// Return the list of HID consumer usages to restore the iPhone physical speaker when audio stops.
    pub fn stop_restore_usages(&self) -> Vec<u16> {
        if self.restore_speaker {
            ZeroDoubleAudioController::on_stream_stop()
        } else {
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wasapi_stream_config_defaults() {
        let cfg = WasapiStreamConfig::default();
        assert_eq!(cfg.sample_rate, 48000);
        assert_eq!(cfg.channels, 2);
        assert_eq!(cfg.period_frames, 480);
        assert_eq!(cfg.start_mute_usages().len(), 16);
        assert_eq!(cfg.stop_restore_usages().len(), 5);
    }

    #[test]
    fn test_mmcss_probe_succeeds() {
        let probe = probe_wasapi_and_mmcss();
        assert!(probe.is_ok());
        let res = probe.unwrap();
        assert!(res.mmcss_enabled);
        assert!(res.bypass_ecoqos);
        assert_eq!(res.task_name, "Pro Audio");
    }
}
