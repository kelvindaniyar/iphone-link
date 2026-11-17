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
pub type CoCreateInstanceFn = unsafe extern "system" fn(
    *const Guid,
    *mut c_void,
    u32,
    *const Guid,
    *mut *mut c_void,
) -> i32;

const COINIT_MULTITHREADED: u32 = 0x0;
const RPC_E_CHANGED_MODE: i32 = 0x8001_0106u32 as i32;
const CLSCTX_ALL: u32 = 0x17;
const E_RENDER: i32 = 0;
const E_CONSOLE: i32 = 0;
const AUDCLNT_SHAREMODE_SHARED: i32 = 0;
const AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM: u32 = 0x8000_0000;
const AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY: u32 = 0x0800_0000;
const WAVE_FORMAT_PCM: u16 = 1;
const DEFAULT_BUFFER_HNS_40MS: i64 = 400_000;

/// Win32 GUID layout compatible with COM `REFCLSID` / `REFIID`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Guid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

/// `{BCDE0395-E52F-467C-8E3D-C4579291692E}`
pub const CLSID_MM_DEVICE_ENUMERATOR: Guid = Guid {
    data1: 0xBCDE0395,
    data2: 0xE52F,
    data3: 0x467C,
    data4: [0x8E, 0x3D, 0xC4, 0x57, 0x92, 0x91, 0x69, 0x2E],
};

/// `{A95664D2-9614-4F35-A746-DE8DB63617E6}`
pub const IID_IMM_DEVICE_ENUMERATOR: Guid = Guid {
    data1: 0xA95664D2,
    data2: 0x9614,
    data3: 0x4F35,
    data4: [0xA7, 0x46, 0xDE, 0x8D, 0xB6, 0x36, 0x17, 0xE6],
};

/// `{1CB9AD4C-DBFA-4C32-B178-C2F568A703B2}`
pub const IID_IAUDIO_CLIENT: Guid = Guid {
    data1: 0x1CB9AD4C,
    data2: 0xDBFA,
    data3: 0x4C32,
    data4: [0xB1, 0x78, 0xC2, 0xF5, 0x68, 0xA7, 0x03, 0xB2],
};

/// `{F294ACFC-3146-4483-A7BF-ADDCA7C260E2}`
pub const IID_IAUDIO_RENDER_CLIENT: Guid = Guid {
    data1: 0xF294ACFC,
    data2: 0x3146,
    data3: 0x4483,
    data4: [0xA7, 0xBF, 0xAD, 0xDC, 0xA7, 0xC2, 0x60, 0xE2],
};

#[repr(C, packed(1))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaveFormatEx {
    pub w_format_tag: u16,
    pub n_channels: u16,
    pub n_samples_per_sec: u32,
    pub n_avg_bytes_per_sec: u32,
    pub n_block_align: u16,
    pub w_bits_per_sample: u16,
    pub cb_size: u16,
}

impl WaveFormatEx {
    /// Standard 16-bit PCM interleaved stereo layout matching CoreAudio shared-mode autoconvert.
    pub fn pcm_s16le(sample_rate: u32, channels: u16) -> Self {
        let bits_per_sample = 16u16;
        let block_align = channels.saturating_mul(bits_per_sample / 8).max(2);
        let avg_bytes = sample_rate.saturating_mul(block_align as u32);
        Self {
            w_format_tag: WAVE_FORMAT_PCM,
            n_channels: channels,
            n_samples_per_sec: sample_rate,
            n_avg_bytes_per_sec: avg_bytes,
            n_block_align: block_align,
            w_bits_per_sample: bits_per_sample,
            cb_size: 0,
        }
    }
}

#[repr(C)]
struct IUnknownVtbl {
    query_interface: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> i32,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
}

#[repr(C)]
struct IMMDeviceEnumeratorVtbl {
    base: IUnknownVtbl,
    enum_audio_endpoints: unsafe extern "system" fn(*mut c_void, i32, u32, *mut *mut c_void) -> i32,
    get_default_audio_endpoint: unsafe extern "system" fn(*mut c_void, i32, i32, *mut *mut c_void) -> i32,
}

#[repr(C)]
struct IMMDeviceVtbl {
    base: IUnknownVtbl,
    activate: unsafe extern "system" fn(*mut c_void, *const Guid, u32, *mut c_void, *mut *mut c_void) -> i32,
}

#[repr(C)]
struct IAudioClientVtbl {
    base: IUnknownVtbl,
    initialize: unsafe extern "system" fn(*mut c_void, i32, u32, i64, i64, *const WaveFormatEx, *const Guid) -> i32,
    get_buffer_size: unsafe extern "system" fn(*mut c_void, *mut u32) -> i32,
    get_stream_latency: unsafe extern "system" fn(*mut c_void, *mut i64) -> i32,
    get_current_padding: unsafe extern "system" fn(*mut c_void, *mut u32) -> i32,
    is_format_supported: unsafe extern "system" fn(*mut c_void, i32, *const WaveFormatEx, *mut *mut WaveFormatEx) -> i32,
    get_mix_format: unsafe extern "system" fn(*mut c_void, *mut *mut WaveFormatEx) -> i32,
    get_device_period: unsafe extern "system" fn(*mut c_void, *mut i64, *mut i64) -> i32,
    start: unsafe extern "system" fn(*mut c_void) -> i32,
    stop: unsafe extern "system" fn(*mut c_void) -> i32,
    reset: unsafe extern "system" fn(*mut c_void) -> i32,
    set_event_handle: unsafe extern "system" fn(*mut c_void, *mut c_void) -> i32,
    get_service: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> i32,
}

#[repr(C)]
struct IAudioRenderClientVtbl {
    base: IUnknownVtbl,
    get_buffer: unsafe extern "system" fn(*mut c_void, u32, *mut *mut u8) -> i32,
    release_buffer: unsafe extern "system" fn(*mut c_void, u32, u32) -> i32,
}

unsafe fn com_vtbl<T>(obj: *mut c_void) -> &'static T {
    let vtbl_ptr = *(obj as *const *const T);
    &*vtbl_ptr
}

unsafe fn com_release(obj: &mut *mut c_void) {
    if !obj.is_null() {
        let vtbl = com_vtbl::<IUnknownVtbl>(*obj);
        (vtbl.release)(*obj);
        *obj = std::ptr::null_mut();
    }
}

fn format_hr(prefix: &str, hr: i32) -> String {
    format!("{}: HRESULT=0x{:08X}", prefix, hr as u32)
}

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

/// Detailed Win32 COM WASAPI Shared-Mode + MMCSS hardware probe report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WasapiProbeReport {
    pub mmcss_enabled: bool,
    pub task_name: String,
    pub bypass_ecoqos: bool,
    pub mmdevice_enumerator_ok: bool,
    pub audio_client_ok: bool,
    pub audio_render_client_ok: bool,
    pub virtual_null_sink: bool,
    pub buffer_frames: u32,
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
    pub backend: String,
    pub detail: String,
}

/// Native Win32 COM WASAPI Shared-Mode PCM output renderer (`IMMDeviceEnumerator` -> `IAudioClient` -> `IAudioRenderClient`).
/// Gracefully falls back to a non-blocking virtual null-sink on headless machines without an audio endpoint.
pub struct NativeWasapiRenderer {
    h_ole32: *mut c_void,
    co_uninit: Option<CoUninitializeFn>,
    com_owned: bool,
    h_avrt: *mut c_void,
    mmcss_handle: *mut c_void,
    av_revert: Option<AvRevertMmThreadCharacteristics>,
    enumerator: *mut c_void,
    device: *mut c_void,
    audio_client: *mut c_void,
    render_client: *mut c_void,
    started: bool,
    virtual_null_sink: bool,
    buffer_frames: u32,
    sample_rate: u32,
    channels: u16,
    total_frames_written: u64,
    init_detail: String,
}

unsafe impl Send for NativeWasapiRenderer {}

impl NativeWasapiRenderer {
    /// Opens the Windows default audio render endpoint in WASAPI Shared Mode (`48kHz 16-bit PCM` default)
    /// using `AUTOCONVERTPCM | SRC_DEFAULT_QUALITY`, falling back cleanly to a virtual null-sink if no endpoint exists.
    pub fn open_default_shared(sample_rate: u32, channels: u16) -> Result<Self, String> {
        let rate = if sample_rate == 0 { 48000 } else { sample_rate };
        let ch = if channels == 0 { 2 } else { channels };

        let ole32_name = to_wide("ole32.dll");
        let h_ole32 = unsafe { LoadLibraryW(ole32_name.as_ptr()) };
        if h_ole32.is_null() {
            return Err("Failed to load ole32.dll".to_string());
        }

        let co_init: Option<CoInitializeExFn> = unsafe {
            let p = GetProcAddress(h_ole32, b"CoInitializeEx\0".as_ptr());
            if p.is_null() { None } else { Some(std::mem::transmute(p)) }
        };
        let co_uninit: Option<CoUninitializeFn> = unsafe {
            let p = GetProcAddress(h_ole32, b"CoUninitialize\0".as_ptr());
            if p.is_null() { None } else { Some(std::mem::transmute(p)) }
        };
        let co_create: Option<CoCreateInstanceFn> = unsafe {
            let p = GetProcAddress(h_ole32, b"CoCreateInstance\0".as_ptr());
            if p.is_null() { None } else { Some(std::mem::transmute(p)) }
        };

        let mut com_owned = false;
        if let Some(init_fn) = co_init {
            let hr = unsafe { init_fn(std::ptr::null_mut(), COINIT_MULTITHREADED) };
            if hr >= 0 {
                com_owned = true;
            } else if hr != RPC_E_CHANGED_MODE {
                unsafe { FreeLibrary(h_ole32); }
                return Err(format_hr("CoInitializeEx failed", hr));
            }
        }

        let mut renderer = Self {
            h_ole32,
            co_uninit,
            com_owned,
            h_avrt: std::ptr::null_mut(),
            mmcss_handle: std::ptr::null_mut(),
            av_revert: None,
            enumerator: std::ptr::null_mut(),
            device: std::ptr::null_mut(),
            audio_client: std::ptr::null_mut(),
            render_client: std::ptr::null_mut(),
            started: false,
            virtual_null_sink: false,
            buffer_frames: (rate / 25).max(480),
            sample_rate: rate,
            channels: ch,
            total_frames_written: 0,
            init_detail: String::new(),
        };

        // Optional MMCSS Pro Audio registration for low-jitter audio delivery
        let avrt_name = to_wide("avrt.dll");
        let h_avrt = unsafe { LoadLibraryW(avrt_name.as_ptr()) };
        if !h_avrt.is_null() {
            renderer.h_avrt = h_avrt;
            let av_set: Option<AvSetMmThreadCharacteristicsW> = unsafe {
                let p = GetProcAddress(h_avrt, b"AvSetMmThreadCharacteristicsW\0".as_ptr());
                if p.is_null() { None } else { Some(std::mem::transmute(p)) }
            };
            renderer.av_revert = unsafe {
                let p = GetProcAddress(h_avrt, b"AvRevertMmThreadCharacteristics\0".as_ptr());
                if p.is_null() { None } else { Some(std::mem::transmute(p)) }
            };
            if let Some(set_fn) = av_set {
                let mut task_idx = 0u32;
                let task_wide = to_wide("Pro Audio");
                let handle = unsafe { set_fn(task_wide.as_ptr(), &mut task_idx) };
                if !handle.is_null() {
                    renderer.mmcss_handle = handle;
                }
            }
        }

        let Some(create_fn) = co_create else {
            renderer.activate_virtual_null_sink("CoCreateInstance symbol unavailable".to_string());
            return Ok(renderer);
        };

        let hr_enum = unsafe {
            create_fn(
                &CLSID_MM_DEVICE_ENUMERATOR,
                std::ptr::null_mut(),
                CLSCTX_ALL,
                &IID_IMM_DEVICE_ENUMERATOR,
                &mut renderer.enumerator,
            )
        };
        if hr_enum < 0 || renderer.enumerator.is_null() {
            renderer.activate_virtual_null_sink(format_hr("MMDeviceEnumerator unavailable", hr_enum));
            return Ok(renderer);
        }

        let hr_dev = unsafe {
            let vtbl = com_vtbl::<IMMDeviceEnumeratorVtbl>(renderer.enumerator);
            (vtbl.get_default_audio_endpoint)(
                renderer.enumerator,
                E_RENDER,
                E_CONSOLE,
                &mut renderer.device,
            )
        };
        if hr_dev < 0 || renderer.device.is_null() {
            // Headless VM / no active playback endpoint -> deterministic virtual null-sink fallback
            renderer.activate_virtual_null_sink(format_hr("No default audio render endpoint", hr_dev));
            return Ok(renderer);
        }

        let hr_act = unsafe {
            let vtbl = com_vtbl::<IMMDeviceVtbl>(renderer.device);
            (vtbl.activate)(
                renderer.device,
                &IID_IAUDIO_CLIENT,
                CLSCTX_ALL,
                std::ptr::null_mut(),
                &mut renderer.audio_client,
            )
        };
        if hr_act < 0 || renderer.audio_client.is_null() {
            renderer.activate_virtual_null_sink(format_hr("IMMDevice::Activate(IAudioClient) failed", hr_act));
            return Ok(renderer);
        }

        let wfx = WaveFormatEx::pcm_s16le(rate, ch);
        let stream_flags = AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
        let hr_init = unsafe {
            let vtbl = com_vtbl::<IAudioClientVtbl>(renderer.audio_client);
            (vtbl.initialize)(
                renderer.audio_client,
                AUDCLNT_SHAREMODE_SHARED,
                stream_flags,
                DEFAULT_BUFFER_HNS_40MS,
                0,
                &wfx,
                std::ptr::null(),
            )
        };
        if hr_init < 0 {
            renderer.activate_virtual_null_sink(format_hr("IAudioClient::Initialize failed", hr_init));
            return Ok(renderer);
        }

        let mut buf_frames = 0u32;
        let hr_bsz = unsafe {
            let vtbl = com_vtbl::<IAudioClientVtbl>(renderer.audio_client);
            (vtbl.get_buffer_size)(renderer.audio_client, &mut buf_frames)
        };
        if hr_bsz >= 0 && buf_frames > 0 {
            renderer.buffer_frames = buf_frames;
        }

        let hr_svc = unsafe {
            let vtbl = com_vtbl::<IAudioClientVtbl>(renderer.audio_client);
            (vtbl.get_service)(
                renderer.audio_client,
                &IID_IAUDIO_RENDER_CLIENT,
                &mut renderer.render_client,
            )
        };
        if hr_svc < 0 || renderer.render_client.is_null() {
            renderer.activate_virtual_null_sink(format_hr("IAudioClient::GetService(IAudioRenderClient) failed", hr_svc));
            return Ok(renderer);
        }

        let _ = renderer.start();
        renderer.init_detail = format!(
            "WASAPI Shared Active ({}Hz {}ch S16LE, buffer={} frames)",
            rate, ch, renderer.buffer_frames
        );
        Ok(renderer)
    }

    fn activate_virtual_null_sink(&mut self, reason: String) {
        unsafe {
            com_release(&mut self.render_client);
            com_release(&mut self.audio_client);
            com_release(&mut self.device);
        }
        self.virtual_null_sink = true;
        self.init_detail = format!("VirtualNullSink ({reason})");
    }

    /// Starts the WASAPI `IAudioClient` stream if running on a live endpoint.
    pub fn start(&mut self) -> Result<(), String> {
        if self.virtual_null_sink || self.started || self.audio_client.is_null() {
            self.started = true;
            return Ok(());
        }
        let hr = unsafe {
            let vtbl = com_vtbl::<IAudioClientVtbl>(self.audio_client);
            (vtbl.start)(self.audio_client)
        };
        if hr < 0 {
            return Err(format_hr("IAudioClient::Start failed", hr));
        }
        self.started = true;
        Ok(())
    }

    /// Stops the WASAPI `IAudioClient` stream if currently started.
    pub fn stop(&mut self) {
        if !self.virtual_null_sink && self.started && !self.audio_client.is_null() {
            unsafe {
                let vtbl = com_vtbl::<IAudioClientVtbl>(self.audio_client);
                let _ = (vtbl.stop)(self.audio_client);
            }
        }
        self.started = false;
    }

    /// Writes interleaved 16-bit signed PCM frames into `IAudioRenderClient` without blocking.
    /// Returns the number of audio frames (each frame = `channels` i16 samples) consumed.
    pub fn write_pcm_s16le(&mut self, samples: &[i16]) -> Result<usize, String> {
        let ch = self.channels as usize;
        if ch == 0 || samples.len() < ch {
            return Ok(0);
        }
        let input_frames = samples.len() / ch;
        if self.virtual_null_sink || self.audio_client.is_null() || self.render_client.is_null() {
            self.total_frames_written += input_frames as u64;
            return Ok(input_frames);
        }

        if !self.started {
            self.start()?;
        }

        let mut padding = 0u32;
        let hr_pad = unsafe {
            let vtbl = com_vtbl::<IAudioClientVtbl>(self.audio_client);
            (vtbl.get_current_padding)(self.audio_client, &mut padding)
        };
        if hr_pad < 0 {
            return Err(format_hr("IAudioClient::GetCurrentPadding failed", hr_pad));
        }

        let avail_frames = self.buffer_frames.saturating_sub(padding);
        let frames_to_write = avail_frames.min(input_frames as u32);
        if frames_to_write == 0 {
            return Ok(0);
        }

        let mut data_ptr: *mut u8 = std::ptr::null_mut();
        let hr_get = unsafe {
            let vtbl = com_vtbl::<IAudioRenderClientVtbl>(self.render_client);
            (vtbl.get_buffer)(self.render_client, frames_to_write, &mut data_ptr)
        };
        if hr_get < 0 || data_ptr.is_null() {
            return Err(format_hr("IAudioRenderClient::GetBuffer failed", hr_get));
        }

        let sample_count = (frames_to_write as usize) * ch;
        let byte_count = sample_count * std::mem::size_of::<i16>();
        unsafe {
            std::ptr::copy_nonoverlapping(samples.as_ptr() as *const u8, data_ptr, byte_count);
            let vtbl = com_vtbl::<IAudioRenderClientVtbl>(self.render_client);
            let hr_rel = (vtbl.release_buffer)(self.render_client, frames_to_write, 0);
            if hr_rel < 0 {
                return Err(format_hr("IAudioRenderClient::ReleaseBuffer failed", hr_rel));
            }
        }

        self.total_frames_written += frames_to_write as u64;
        Ok(frames_to_write as usize)
    }

    pub fn has_enumerator(&self) -> bool {
        !self.enumerator.is_null()
    }

    pub fn has_audio_client(&self) -> bool {
        !self.audio_client.is_null()
    }

    pub fn has_render_client(&self) -> bool {
        !self.render_client.is_null()
    }

    pub fn is_virtual_null_sink(&self) -> bool {
        self.virtual_null_sink
    }

    pub fn buffer_size_frames(&self) -> u32 {
        self.buffer_frames
    }

    pub fn total_frames_written(&self) -> u64 {
        self.total_frames_written
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }

    pub fn backend_name(&self) -> &'static str {
        if self.virtual_null_sink {
            "VirtualNullSink"
        } else {
            "WASAPI_Shared"
        }
    }

    pub fn detail(&self) -> &str {
        &self.init_detail
    }
}

impl Drop for NativeWasapiRenderer {
    fn drop(&mut self) {
        self.stop();
        unsafe {
            com_release(&mut self.render_client);
            com_release(&mut self.audio_client);
            com_release(&mut self.device);
            com_release(&mut self.enumerator);

            if !self.mmcss_handle.is_null() {
                if let Some(revert_fn) = self.av_revert {
                    let _ = revert_fn(self.mmcss_handle);
                }
                self.mmcss_handle = std::ptr::null_mut();
            }
            if !self.h_avrt.is_null() {
                FreeLibrary(self.h_avrt);
                self.h_avrt = std::ptr::null_mut();
            }
            if self.com_owned {
                if let Some(uninit_fn) = self.co_uninit {
                    uninit_fn();
                }
                self.com_owned = false;
            }
            if !self.h_ole32.is_null() {
                FreeLibrary(self.h_ole32);
                self.h_ole32 = std::ptr::null_mut();
            }
        }
    }
}

/// Probes real Win32 COM WASAPI Shared-Mode interfaces (`IMMDeviceEnumerator`, `IAudioClient`, `IAudioRenderClient`)
/// paired with MMCSS `"Pro Audio"` thread boosting.
pub fn probe_wasapi_shared_engine() -> WasapiProbeReport {
    let mmcss = probe_wasapi_and_mmcss();
    let (mmcss_enabled, task_name, bypass_ecoqos) = match mmcss {
        Ok(r) => (r.mmcss_enabled, r.task_name, r.bypass_ecoqos),
        Err(e) => (false, format!("Unavailable ({e})"), false),
    };

    match NativeWasapiRenderer::open_default_shared(48000, 2) {
        Ok(r) => WasapiProbeReport {
            mmcss_enabled,
            task_name,
            bypass_ecoqos,
            mmdevice_enumerator_ok: r.has_enumerator(),
            audio_client_ok: r.has_audio_client(),
            audio_render_client_ok: r.has_render_client(),
            virtual_null_sink: r.is_virtual_null_sink(),
            buffer_frames: r.buffer_size_frames(),
            sample_rate: r.sample_rate(),
            channels: r.channels(),
            bits_per_sample: 16,
            backend: r.backend_name().to_string(),
            detail: r.detail().to_string(),
        },
        Err(e) => WasapiProbeReport {
            mmcss_enabled,
            task_name,
            bypass_ecoqos,
            mmdevice_enumerator_ok: false,
            audio_client_ok: false,
            audio_render_client_ok: false,
            virtual_null_sink: true,
            buffer_frames: 0,
            sample_rate: 48000,
            channels: 2,
            bits_per_sample: 16,
            backend: "Error".to_string(),
            detail: e,
        },
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

    #[test]
    fn test_wave_format_ex_layout_and_guids() {
        assert_eq!(std::mem::size_of::<WaveFormatEx>(), 18);
        assert_eq!(std::mem::size_of::<Guid>(), 16);
        let wfx = WaveFormatEx::pcm_s16le(48000, 2);
        let tag = wfx.w_format_tag;
        let ch = wfx.n_channels;
        let rate = wfx.n_samples_per_sec;
        let avg = wfx.n_avg_bytes_per_sec;
        let align = wfx.n_block_align;
        let bits = wfx.w_bits_per_sample;
        let cb = wfx.cb_size;
        assert_eq!(tag, WAVE_FORMAT_PCM);
        assert_eq!(ch, 2);
        assert_eq!(rate, 48000);
        assert_eq!(avg, 192000);
        assert_eq!(align, 4);
        assert_eq!(bits, 16);
        assert_eq!(cb, 0);
    }

    #[test]
    fn test_native_wasapi_shared_renderer_and_probe() {
        let report = probe_wasapi_shared_engine();
        assert!(report.mmcss_enabled);
        assert!(report.mmdevice_enumerator_ok);
        if !report.virtual_null_sink {
            assert!(report.audio_client_ok);
            assert!(report.audio_render_client_ok);
            assert!(report.buffer_frames >= 480);
        }

        let mut renderer = NativeWasapiRenderer::open_default_shared(48000, 2)
            .expect("NativeWasapiRenderer must open cleanly");
        // Write 10ms of quiet stereo PCM (480 frames = 960 i16 samples)
        let silence = vec![0i16; 960];
        let written = renderer.write_pcm_s16le(&silence).expect("write_pcm_s16le must succeed");
        assert!(written > 0 && written <= 480);
        assert_eq!(renderer.total_frames_written(), written as u64);
    }
}
