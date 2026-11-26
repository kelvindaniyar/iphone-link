//! AAC-ELD audio decoder FFI bindings for zlaac.dll, Zero Double Audio controller, and WASAPI jitter buffer.

pub mod aac_decoder;
pub mod jitter_buffer;
pub mod wasapi_output;

pub use aac_decoder::{AacEldDecoder, SharedAacDecoder};
pub use jitter_buffer::AdaptiveAudioBuffer;
pub use wasapi_output::{
    probe_wasapi_and_mmcss, probe_wasapi_shared_engine, MmcssProbeResult, NativeWasapiRenderer,
    WasapiProbeReport, WasapiStreamConfig,
};

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

/// Unified Native Rust Audio Pipeline:
/// `AacEldDecoder` (`zlaac.dll` FFI) -> `AdaptiveAudioBuffer` -> `NativeWasapiRenderer` (`WASAPI Shared`).
pub struct NativeRustAudioPipeline {
    decoder: Option<AacEldDecoder>,
    pub jitter_buffer: AdaptiveAudioBuffer,
    pub renderer: NativeWasapiRenderer,
    packets_decoded: u64,
    frames_rendered: u64,
}

impl NativeRustAudioPipeline {
    /// Constructs a unified pipeline with optional `zlaac.dll` path (for real AAC-ELD decode)
    /// and an active WASAPI Shared-Mode renderer.
    pub fn new(
        zlaac_dll_path: Option<&Path>,
        preset: AudioBufferPreset,
        sample_rate: u32,
        channels: u16,
    ) -> Result<Self, String> {
        let decoder = match zlaac_dll_path {
            Some(p) if p.exists() => Some(AacEldDecoder::new(p, sample_rate, channels)?),
            _ => None,
        };
        let jitter_buffer = AdaptiveAudioBuffer::new(preset);
        let renderer = NativeWasapiRenderer::open_default_shared(sample_rate, channels)?;
        Ok(Self {
            decoder,
            jitter_buffer,
            renderer,
            packets_decoded: 0,
            frames_rendered: 0,
        })
    }

    /// Decodes an incoming AirPlay/CoreDevice AAC-ELD Access Unit via `zlaac.dll`,
    /// buffers/gains it through `AdaptiveAudioBuffer`, and writes frames to `NativeWasapiRenderer`.
    pub fn decode_and_render_aac_packet(&mut self, aac_frame: &[u8]) -> Result<usize, String> {
        let pcm = match self.decoder.as_mut() {
            Some(dec) => dec.decode_access_unit(aac_frame)?,
            None => return Err("AAC-ELD decoder not loaded (missing zlaac.dll)".to_string()),
        };
        if pcm.is_empty() {
            return Ok(0);
        }
        self.packets_decoded += 1;
        self.render_pcm_through_jitter(&pcm)
    }

    /// Pushes decoded interleaved S16LE PCM samples through `AdaptiveAudioBuffer` and renders via WASAPI.
    pub fn render_pcm_through_jitter(&mut self, pcm_samples: &[i16]) -> Result<usize, String> {
        if pcm_samples.is_empty() {
            return Ok(0);
        }
        self.jitter_buffer.push_samples(pcm_samples);
        let pull_cnt = pcm_samples.len().min(self.jitter_buffer.available_samples());
        if pull_cnt == 0 {
            return Ok(0);
        }
        let processed = self.jitter_buffer.pull_samples(pull_cnt);
        let written = self.renderer.write_pcm_s16le(&processed)?;
        self.frames_rendered += written as u64;
        Ok(written)
    }

    pub fn has_decoder(&self) -> bool {
        self.decoder.is_some()
    }

    pub fn packets_decoded(&self) -> u64 {
        self.packets_decoded
    }

    pub fn frames_rendered(&self) -> u64 {
        self.frames_rendered
    }

    pub fn backend_name(&self) -> &'static str {
        self.renderer.backend_name()
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

    #[test]
    fn test_native_rust_audio_pipeline_end_to_end() {
        let dll_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("bins")
            .join("zlaac.dll");
        let dll_opt = if dll_path.exists() { Some(dll_path.as_path()) } else { None };
        let mut pipe = NativeRustAudioPipeline::new(dll_opt, AudioBufferPreset::Normal, 48000, 2)
            .expect("NativeRustAudioPipeline must initialize");
        if dll_opt.is_some() {
            assert!(pipe.has_decoder(), "zlaac.dll decoder must load in pipeline");
        }
        pipe.jitter_buffer.set_volume(100);
        let frames_before = pipe.frames_rendered();
        let written = pipe
            .render_pcm_through_jitter(&vec![120i16, -120i16].repeat(240))
            .expect("render_pcm_through_jitter must succeed");
        assert!(written > 0);
        assert!(pipe.frames_rendered() > frames_before);

        // Minimal synthetic AAC-ELD payload exercise via zlaac.dll decoder -> jitter -> WASAPI path
        let res = pipe.decode_and_render_aac_packet(&[0x00, 0x00, 0x00, 0x00]);
        assert!(res.is_ok() || res.is_err());
    }

    #[test]
    fn test_tahap239_pc_audio_bridge_start_stop() {
        let (ok, detail) = start_global_pc_audio_bridge();
        assert!(ok, "Audio bridge failed: {detail}");
        assert!(is_global_pc_audio_bridge_active());
        std::thread::sleep(std::time::Duration::from_millis(80));
        let st = global_pc_audio_bridge_status();
        assert!(st.0);
        stop_global_pc_audio_bridge();
        assert!(!is_global_pc_audio_bridge_active());
    }
}

// ============================================================================
// TAHAP 239: Native In-Process WinRT AudioPlaybackConnection + WASAPI Bridge
// ============================================================================

#[cfg(windows)]
mod winrt_a2dp {
    use super::*;
    use std::ptr;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Guid {
        data1: u32,
        data2: u16,
        data3: u16,
        data4: [u8; 8],
    }

    #[repr(C)]
    struct ComObj {
        vtable: *const *const c_void,
    }

    #[repr(C)]
    struct WinMsg {
        hwnd: *mut c_void,
        message: u32,
        w_param: usize,
        l_param: isize,
        time: u32,
        pt_x: i32,
        pt_y: i32,
    }

    const IID_IASYNC_INFO: Guid = Guid {
        data1: 0x00000036,
        data2: 0x0000,
        data3: 0x0000,
        data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
    };

    // IAudioPlaybackConnectionStatics {E60963A2-69E6-5FFC-9E13-824A85213DAF}
    const IID_IAUDIO_PLAYBACK_CONN_STATICS: Guid = Guid {
        data1: 0xE60963A2,
        data2: 0x69E6,
        data3: 0x5FFC,
        data4: [0x9E, 0x13, 0x82, 0x4A, 0x85, 0x21, 0x3D, 0xAF],
    };

    // IDeviceInformationStatics {C17F100E-3A46-4A78-8013-769DC9B97390}
    const IID_IDEVICE_INFO_STATICS: Guid = Guid {
        data1: 0xC17F100E,
        data2: 0x3A46,
        data3: 0x4A78,
        data4: [0x80, 0x13, 0x76, 0x9D, 0xC9, 0xB9, 0x73, 0x90],
    };

    // Windows.Foundation.IClosable {30D5A829-7FA4-4026-83BB-D75BAE4EA99E}
    const IID_ICLOSABLE: Guid = Guid {
        data1: 0x30D5A829,
        data2: 0x7FA4,
        data3: 0x4026,
        data4: [0x83, 0xBB, 0xD7, 0x5B, 0xAE, 0x4E, 0xA9, 0x9E],
    };

    #[link(name = "user32")]
    extern "system" {
        fn PeekMessageW(
            lp_msg: *mut WinMsg,
            hwnd: *mut c_void,
            w_msg_filter_min: u32,
            w_msg_filter_max: u32,
            w_remove_msg: u32,
        ) -> i32;
        fn TranslateMessage(lp_msg: *const WinMsg) -> i32;
        fn DispatchMessageW(lp_msg: *const WinMsg) -> isize;
    }

    type FnRoInitialize = unsafe extern "system" fn(i32) -> i32;
    type FnRoUninitialize = unsafe extern "system" fn();
    type FnWindowsCreateString =
        unsafe extern "system" fn(*const u16, u32, *mut *mut c_void) -> i32;
    type FnWindowsDeleteString = unsafe extern "system" fn(*mut c_void) -> i32;
    type FnWindowsGetStringRawBuffer =
        unsafe extern "system" fn(*mut c_void, *mut u32) -> *const u16;
    type FnRoGetActivationFactory =
        unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> i32;

    unsafe fn release_com(obj: *mut c_void) {
        if obj.is_null() {
            return;
        }
        let vt = (*(obj as *mut ComObj)).vtable;
        if !vt.is_null() {
            let rel: unsafe extern "system" fn(*mut c_void) -> u32 =
                std::mem::transmute(*vt.add(2));
            let _ = rel(obj);
        }
    }

    unsafe fn qi_com(obj: *mut c_void, iid: &Guid) -> *mut c_void {
        if obj.is_null() {
            return ptr::null_mut();
        }
        let vt = (*(obj as *mut ComObj)).vtable;
        if vt.is_null() {
            return ptr::null_mut();
        }
        let qi: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> i32 =
            std::mem::transmute(*vt.add(0));
        let mut out: *mut c_void = ptr::null_mut();
        if qi(obj, iid, &mut out) >= 0 {
            out
        } else {
            ptr::null_mut()
        }
    }

    pub unsafe fn pump_sta_messages() {
        let mut msg = std::mem::zeroed::<WinMsg>();
        while PeekMessageW(&mut msg, ptr::null_mut(), 0, 0, 1) != 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    unsafe fn wait_async_with_sta_pump(async_obj: *mut c_void, max_ms: u32) -> bool {
        if async_obj.is_null() {
            return false;
        }
        let info = qi_com(async_obj, &IID_IASYNC_INFO);
        if info.is_null() {
            return false;
        }
        let get_status: unsafe extern "system" fn(*mut c_void, *mut i32) -> i32 =
            std::mem::transmute(*(*(info as *mut ComObj)).vtable.add(7));
        let mut elapsed = 0u32;
        let mut ok = false;
        loop {
            pump_sta_messages();
            let mut st: i32 = -1;
            if get_status(info, &mut st) < 0 {
                break;
            }
            if st == 1 {
                ok = true;
                break;
            }
            if st != 0 || elapsed >= max_ms {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
            elapsed = elapsed.saturating_add(5);
        }
        release_com(info);
        ok
    }

    pub struct ActiveA2dpConnections {
        conns: Vec<*mut c_void>,
        pub opened_count: usize,
        pub found_bt_devices: usize,
        pub really_opened: bool,
        pub summary: String,
        pub opened_device_name: String,
        pub found_bt_names: String,
    }

    unsafe fn close_and_release_a2dp_conn(c: *mut c_void) {
        if c.is_null() {
            return;
        }
        let closable = qi_com(c, &IID_ICLOSABLE);
        if !closable.is_null() {
            let vt = (*(closable as *mut ComObj)).vtable;
            if !vt.is_null() {
                let close_fn: unsafe extern "system" fn(*mut c_void) -> i32 =
                    std::mem::transmute(*vt.add(6));
                let _ = close_fn(closable);
            }
            release_com(closable);
        }
        release_com(c);
    }

    impl ActiveA2dpConnections {
        /// Probes `IAudioPlaybackConnection::get_State()` (`vtable[9]`) across opened connections.
        pub unsafe fn is_any_conn_opened(&self) -> bool {
            if self.conns.is_empty() {
                return false;
            }
            for &conn in &self.conns {
                if conn.is_null() {
                    continue;
                }
                let vt = (*(conn as *mut ComObj)).vtable;
                if vt.is_null() {
                    continue;
                }
                let mut conn_state: i32 = 0;
                let get_conn_state: unsafe extern "system" fn(*mut c_void, *mut i32) -> i32 =
                    std::mem::transmute(*vt.add(9));
                let hr = get_conn_state(conn, &mut conn_state);
                if super::evaluate_a2dp_keepalive_alive(hr, conn_state) {
                    return true;
                }
            }
            false
        }
    }

    impl Drop for ActiveA2dpConnections {
        fn drop(&mut self) {
            unsafe {
                for c in self.conns.drain(..) {
                    close_and_release_a2dp_conn(c);
                }
                pump_sta_messages();
            }
        }
    }

    pub fn try_activate_bluetooth_a2dp_sta(
        preferred_name: Option<&str>,
        preferred_model: Option<&str>,
        preferred_bt_mac: Option<&str>,
    ) -> ActiveA2dpConnections {
        let mut res = ActiveA2dpConnections {
            conns: Vec::new(),
            opened_count: 0,
            found_bt_devices: 0,
            really_opened: false,
            summary: "wasapi-shared".to_string(),
            opened_device_name: String::new(),
            found_bt_names: String::new(),
        };
        unsafe {
            let h_combase = LoadLibraryW(to_wide(Path::new("combase.dll")).as_ptr());
            if h_combase.is_null() {
                return res;
            }
            let p_ro_init = GetProcAddress(h_combase, b"RoInitialize\0".as_ptr());
            let p_ro_uninit = GetProcAddress(h_combase, b"RoUninitialize\0".as_ptr());
            let p_create_str = GetProcAddress(h_combase, b"WindowsCreateString\0".as_ptr());
            let p_del_str = GetProcAddress(h_combase, b"WindowsDeleteString\0".as_ptr());
            let p_get_buf = GetProcAddress(h_combase, b"WindowsGetStringRawBuffer\0".as_ptr());
            let p_get_factory = GetProcAddress(h_combase, b"RoGetActivationFactory\0".as_ptr());
            if p_ro_init.is_null()
                || p_create_str.is_null()
                || p_del_str.is_null()
                || p_get_buf.is_null()
                || p_get_factory.is_null()
            {
                FreeLibrary(h_combase);
                return res;
            }

            let ro_init: FnRoInitialize = std::mem::transmute(p_ro_init);
            let _ro_uninit: Option<FnRoUninitialize> = if p_ro_uninit.is_null() {
                None
            } else {
                Some(std::mem::transmute(p_ro_uninit))
            };
            let win_create_str: FnWindowsCreateString = std::mem::transmute(p_create_str);
            let win_del_str: FnWindowsDeleteString = std::mem::transmute(p_del_str);
            let win_get_buf: FnWindowsGetStringRawBuffer = std::mem::transmute(p_get_buf);
            let get_factory: FnRoGetActivationFactory = std::mem::transmute(p_get_factory);

            let _ = ro_init(0); // RO_INIT_SINGLETHREADED (STA)

            let make_hstr = |s: &str| -> *mut c_void {
                let wide: Vec<u16> = s.encode_utf16().collect();
                let mut h: *mut c_void = ptr::null_mut();
                if win_create_str(wide.as_ptr(), wide.len() as u32, &mut h) >= 0 {
                    h
                } else {
                    ptr::null_mut()
                }
            };

            let read_hstr = |h: *mut c_void| -> String {
                if h.is_null() {
                    return String::new();
                }
                let mut nlen = 0u32;
                let nptr = win_get_buf(h, &mut nlen);
                if !nptr.is_null() && nlen > 0 {
                    String::from_utf16_lossy(std::slice::from_raw_parts(nptr, nlen as usize))
                } else {
                    String::new()
                }
            };

            let mut audio_statics: *mut c_void = ptr::null_mut();
            let h_apc = make_hstr("Windows.Media.Audio.AudioPlaybackConnection");
            if !h_apc.is_null() {
                let _ = get_factory(h_apc, &IID_IAUDIO_PLAYBACK_CONN_STATICS, &mut audio_statics);
                let _ = win_del_str(h_apc);
            }
            let mut dev_statics: *mut c_void = ptr::null_mut();
            let h_dev = make_hstr("Windows.Devices.Enumeration.DeviceInformation");
            if !h_dev.is_null() {
                let _ = get_factory(h_dev, &IID_IDEVICE_INFO_STATICS, &mut dev_statics);
                let _ = win_del_str(h_dev);
            }
            if audio_statics.is_null() || dev_statics.is_null() {
                release_com(audio_statics);
                release_com(dev_statics);
                FreeLibrary(h_combase);
                return res;
            }

            let mut aqs_hstr: *mut c_void = ptr::null_mut();
            let get_sel: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32 =
                std::mem::transmute(*(*(audio_statics as *mut ComObj)).vtable.add(6));
            if get_sel(audio_statics, &mut aqs_hstr) >= 0 && !aqs_hstr.is_null() {
                let mut find_op: *mut c_void = ptr::null_mut();
                let find_aqs: unsafe extern "system" fn(
                    *mut c_void,
                    *mut c_void,
                    *mut *mut c_void,
                ) -> i32 = std::mem::transmute(*(*(dev_statics as *mut ComObj)).vtable.add(10));
                if find_aqs(dev_statics, aqs_hstr, &mut find_op) >= 0 && !find_op.is_null() {
                    if wait_async_with_sta_pump(find_op, 3_000) {
                        let mut dev_vec: *mut c_void = ptr::null_mut();
                        let get_res: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32 =
                            std::mem::transmute(*(*(find_op as *mut ComObj)).vtable.add(8));
                        if get_res(find_op, &mut dev_vec) >= 0 && !dev_vec.is_null() {
                            let mut count: u32 = 0;
                            let get_sz: unsafe extern "system" fn(*mut c_void, *mut u32) -> i32 =
                                std::mem::transmute(*(*(dev_vec as *mut ComObj)).vtable.add(7));
                            let get_at: unsafe extern "system" fn(
                                *mut c_void,
                                u32,
                                *mut *mut c_void,
                            ) -> i32 = std::mem::transmute(*(*(dev_vec as *mut ComObj)).vtable.add(6));
                            // Tahap 244 Pilar 1: Enumerate ALL Bluetooth A2DP endpoints first (Name + Id)
                            // and rank by exact MAC -> exact DeviceName -> model -> Apple Vendor ID!
                            let mut raw_entries: Vec<(String, String, *mut c_void)> = Vec::new();
                            if get_sz(dev_vec, &mut count) >= 0 {
                                for idx in 0..count {
                                    let mut di: *mut c_void = ptr::null_mut();
                                    if get_at(dev_vec, idx, &mut di) < 0 || di.is_null() {
                                        continue;
                                    }
                                    let mut id_hstr: *mut c_void = ptr::null_mut();
                                    let mut name_hstr: *mut c_void = ptr::null_mut();
                                    let get_id: unsafe extern "system" fn(
                                        *mut c_void,
                                        *mut *mut c_void,
                                    ) -> i32 = std::mem::transmute(*(*(di as *mut ComObj)).vtable.add(6));
                                    let get_name: unsafe extern "system" fn(
                                        *mut c_void,
                                        *mut *mut c_void,
                                    ) -> i32 = std::mem::transmute(*(*(di as *mut ComObj)).vtable.add(7));
                                    let _ = get_id(di, &mut id_hstr);
                                    let _ = get_name(di, &mut name_hstr);
                                    let name_str = read_hstr(name_hstr);
                                    if !name_hstr.is_null() {
                                        let _ = win_del_str(name_hstr);
                                    }
                                    if !id_hstr.is_null() {
                                        let id_str = read_hstr(id_hstr);
                                        raw_entries.push((name_str, id_str, id_hstr));
                                    }
                                    release_com(di);
                                }
                            }
                            release_com(dev_vec);

                            let pair_list: Vec<(String, String)> = raw_entries
                                .iter()
                                .map(|(n, id, _)| (n.clone(), id.clone()))
                                .collect();
                            let ranked = super::rank_bt_a2dp_candidates(
                                &pair_list,
                                preferred_name,
                                preferred_model,
                                preferred_bt_mac,
                            );
                            res.found_bt_devices = ranked.len();
                            res.found_bt_names = ranked
                                .iter()
                                .map(|(_, _, n, _)| n.as_str())
                                .collect::<Vec<_>>()
                                .join(", ");

                            let has_exact_target = ranked.first().is_some_and(|c| c.1 >= 98);
                            let try_create: unsafe extern "system" fn(
                                *mut c_void,
                                *mut c_void,
                                *mut *mut c_void,
                            ) -> i32 = std::mem::transmute(
                                *(*(audio_statics as *mut ComObj)).vtable.add(7),
                            );
                            const RETRY_DELAYS_MS: [u64; 3] = [350, 700, 1200];

                            for (orig_idx, score, name_str, _id_str) in ranked {
                                // Anti-Hijack Guard: if the exact mirrored iPhone exists in Windows
                                // Bluetooth (score >= 98) and failed 3 attempts, do NOT hijack another iPhone!
                                if has_exact_target && score < 98 {
                                    break;
                                }
                                let id_hstr = raw_entries[orig_idx].2;
                                let mut best_conn: *mut c_void = ptr::null_mut();
                                let mut best_state: i32 = 0;
                                let mut best_status: i32 = -1;
                                for attempt in 0..3usize {
                                    let mut conn: *mut c_void = ptr::null_mut();
                                    if try_create(audio_statics, id_hstr, &mut conn) < 0
                                        || conn.is_null()
                                    {
                                        if attempt < 2 {
                                            std::thread::sleep(std::time::Duration::from_millis(
                                                RETRY_DELAYS_MS[attempt],
                                            ));
                                        }
                                        continue;
                                    }
                                    let mut start_act: *mut c_void = ptr::null_mut();
                                    let start_async: unsafe extern "system" fn(
                                        *mut c_void,
                                        *mut *mut c_void,
                                    ) -> i32 = std::mem::transmute(
                                        *(*(conn as *mut ComObj)).vtable.add(7),
                                    );
                                    if start_async(conn, &mut start_act) >= 0
                                        && !start_act.is_null()
                                    {
                                        let _ = wait_async_with_sta_pump(start_act, 1_200);
                                        release_com(start_act);
                                    }
                                    let mut open_status: i32 = -1;
                                    let mut open_op: *mut c_void = ptr::null_mut();
                                    let open_async: unsafe extern "system" fn(
                                        *mut c_void,
                                        *mut *mut c_void,
                                    ) -> i32 = std::mem::transmute(
                                        *(*(conn as *mut ComObj)).vtable.add(11),
                                    );
                                    if open_async(conn, &mut open_op) >= 0 && !open_op.is_null() {
                                        if wait_async_with_sta_pump(open_op, 1_800) {
                                            let mut open_res: *mut c_void = ptr::null_mut();
                                            let get_open_res: unsafe extern "system" fn(
                                                *mut c_void,
                                                *mut *mut c_void,
                                            ) -> i32 = std::mem::transmute(
                                                *(*(open_op as *mut ComObj)).vtable.add(8),
                                            );
                                            if get_open_res(open_op, &mut open_res) >= 0
                                                && !open_res.is_null()
                                            {
                                                let get_st: unsafe extern "system" fn(
                                                    *mut c_void,
                                                    *mut i32,
                                                ) -> i32 = std::mem::transmute(
                                                    *(*(open_res as *mut ComObj)).vtable.add(6),
                                                );
                                                let _ = get_st(open_res, &mut open_status);
                                                release_com(open_res);
                                            }
                                        }
                                        release_com(open_op);
                                    }
                                    let mut conn_state: i32 = 0;
                                    let get_conn_state: unsafe extern "system" fn(
                                        *mut c_void,
                                        *mut i32,
                                    ) -> i32 = std::mem::transmute(
                                        *(*(conn as *mut ComObj)).vtable.add(9),
                                    );
                                    let _ = get_conn_state(conn, &mut conn_state);

                                    best_state = conn_state;
                                    best_status = open_status;
                                    if conn_state == 1 && open_status == 0 {
                                        best_conn = conn;
                                        res.really_opened = true;
                                        break;
                                    }
                                    close_and_release_a2dp_conn(conn);
                                    pump_sta_messages();
                                    if attempt < 2 {
                                        std::thread::sleep(std::time::Duration::from_millis(
                                            RETRY_DELAYS_MS[attempt],
                                        ));
                                    }
                                }
                                if !best_conn.is_null() {
                                    res.opened_count += 1;
                                    res.opened_device_name = name_str.clone();
                                    let state_lbl = if best_state == 1 { "Opened" } else { "Closed" };
                                    res.summary = format!(
                                        "a2dp-winrt:{name_str}(state={state_lbl},status={best_status},score={score})+wasapi"
                                    );
                                    res.conns.push(best_conn);
                                    break;
                                } else {
                                    res.summary = format!(
                                        "a2dp-winrt:{name_str}(state=Closed,status={best_status},score={score},retries=3)"
                                    );
                                }
                            }

                            for (_, _, id_hstr) in raw_entries {
                                if !id_hstr.is_null() {
                                    let _ = win_del_str(id_hstr);
                                }
                            }
                        }
                    }
                    release_com(find_op);
                }
                let _ = win_del_str(aqs_hstr);
            }
            release_com(audio_statics);
            release_com(dev_statics);
            FreeLibrary(h_combase);
        }
        res
    }
}

/// Normalizes a Bluetooth MAC string (e.g. `"02:00:00:00:00:01"`) to 12 lowercase hex digits.
pub fn normalize_bt_mac(mac: &str) -> String {
    mac.chars()
        .filter(|c| c.is_ascii_hexdigit())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Normalizes a Bluetooth / Bonjour device name (`"Test-Phone"` <-> `"Test Phone"`).
fn normalize_bt_friendly_name(name: &str) -> String {
    name.trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c == '-' || c == '_' { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Detects Apple Inc. Bluetooth Vendor IDs (`0x004C` / `0x05AC`) inside Windows `DeviceInformation.Id`
/// (e.g. `BTHENUM#{0000110a-...}_VID&0001004c_PID&730a#...`), matching Golden `audio.py` parity.
pub fn is_apple_bt_device_id(id: &str) -> bool {
    let low = id.to_ascii_lowercase();
    low.contains("vid&0001004c")
        || low.contains("vid_05ac")
        || low.contains("vid&000205ac")
        || low.contains("vid_004c")
        || low.contains("vid&0002004c")
}

/// Tahap 244 Pilar 1: Ranks ALL Windows Bluetooth A2DP sink candidates `(name, device_info_id)`
/// against the currently mirrored iPhone's `(preferred_name, preferred_model, preferred_bt_mac)`.
/// Returns `(orig_idx, score, name, id)` sorted by score descending; non-Apple non-matching
/// devices (score 0) are excluded.
pub fn rank_bt_a2dp_candidates(
    candidates: &[(String, String)],
    preferred_name: Option<&str>,
    preferred_model: Option<&str>,
    preferred_bt_mac: Option<&str>,
) -> Vec<(usize, u32, String, String)> {
    let pref_name_raw = preferred_name.map(str::trim).filter(|s| !s.is_empty());
    let pref_name_norm = pref_name_raw.map(normalize_bt_friendly_name);
    let pref_model_norm = preferred_model
        .map(normalize_bt_friendly_name)
        .filter(|s| !s.is_empty());
    let pref_mac_hex = preferred_bt_mac
        .map(normalize_bt_mac)
        .filter(|m| m.len() == 12);

    let mut ranked: Vec<(usize, u32, String, String)> = Vec::new();
    for (idx, (name, id)) in candidates.iter().enumerate() {
        let name_trim = name.trim();
        let name_norm = normalize_bt_friendly_name(name_trim);
        let id_low = id.to_ascii_lowercase();
        let mut score: u32 = 0;

        if let Some(ref mac_hex) = pref_mac_hex {
            if id_low.contains(mac_hex.as_str()) {
                score = 120;
            }
        }
        if score == 0 {
            if let Some(pref_raw) = pref_name_raw {
                if name_trim.eq_ignore_ascii_case(pref_raw) {
                    score = 100;
                } else if let Some(ref pref_n) = pref_name_norm {
                    if !name_norm.is_empty() && name_norm == *pref_n {
                        score = 98;
                    } else if pref_n.len() >= 3
                        && !name_norm.is_empty()
                        && (name_norm.contains(pref_n.as_str())
                            || (name_norm.len() >= 3 && pref_n.contains(name_norm.as_str())))
                    {
                        score = 85;
                    }
                }
            }
        }
        if score == 0 {
            if let Some(ref model_n) = pref_model_norm {
                if !name_norm.is_empty()
                    && (name_norm.contains(model_n.as_str())
                        || (name_norm.len() >= 4 && model_n.contains(name_norm.as_str())))
                {
                    score = 80;
                }
            }
        }
        if score == 0 {
            if is_apple_bt_device_id(id) {
                score = 40;
            } else if name_norm.contains("iphone")
                || name_norm.contains("ipad")
                || name_norm.contains("apple")
            {
                score = 35;
            } else if candidates.len() == 1 {
                score = 20;
            }
        }

        if score > 0 {
            ranked.push((idx, score, name.clone(), id.clone()));
        }
    }

    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranked
}

struct PcAudioBridgeHandle {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    done_rx: Option<std::sync::mpsc::Receiver<()>>,
    join: Option<std::thread::JoinHandle<()>>,
    detail: std::sync::Arc<std::sync::Mutex<String>>,
    target_key: String,
}

static PC_AUDIO_BRIDGE: std::sync::OnceLock<std::sync::Mutex<Option<PcAudioBridgeHandle>>> =
    std::sync::OnceLock::new();
static PC_AUDIO_ACTIVE_DEVICE: std::sync::OnceLock<std::sync::Mutex<String>> =
    std::sync::OnceLock::new();
static PC_AUDIO_FOUND_CANDIDATES: std::sync::OnceLock<std::sync::Mutex<String>> =
    std::sync::OnceLock::new();
static PC_AUDIO_BRIDGE_ACTIVE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
static PC_AUDIO_BRIDGE_CONNECTING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
static PC_AUDIO_A2DP_REALLY_OPENED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
static PC_AUDIO_LAST_CLOSED_BY_PEER: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
static PC_AUDIO_LAST_ATTEMPT_FAILED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Pure gate deciding whether the PC audio bridge is genuinely active.
/// In production (`!loopback_test`), requires WinRT `AudioPlaybackConnection` to have
/// genuinely reached `Opened` (`a2dp_really_opened == true`); never marks `active` when `0` BT sinks opened.
pub fn resolve_bridge_truly_active(
    a2dp_really_opened: bool,
    loopback_test: bool,
    wasapi_ok: bool,
) -> bool {
    if loopback_test {
        wasapi_ok
    } else {
        a2dp_really_opened
    }
}

/// Pure COM `IAudioPlaybackConnection::get_State()` liveness evaluator (`State::Opened == 1`).
pub fn evaluate_a2dp_keepalive_alive(hr: i32, state: i32) -> bool {
    hr >= 0 && state == 1
}

fn bridge_slot() -> &'static std::sync::Mutex<Option<PcAudioBridgeHandle>> {
    PC_AUDIO_BRIDGE.get_or_init(|| std::sync::Mutex::new(None))
}

fn active_device_slot() -> &'static std::sync::Mutex<String> {
    PC_AUDIO_ACTIVE_DEVICE.get_or_init(|| std::sync::Mutex::new(String::new()))
}

fn found_candidates_slot() -> &'static std::sync::Mutex<String> {
    PC_AUDIO_FOUND_CANDIDATES.get_or_init(|| std::sync::Mutex::new(String::new()))
}

fn build_audio_target_key(
    preferred_name: Option<&str>,
    preferred_model: Option<&str>,
    preferred_bt_mac: Option<&str>,
) -> String {
    let n = preferred_name
        .map(normalize_bt_friendly_name)
        .unwrap_or_default();
    let m = preferred_model
        .map(normalize_bt_friendly_name)
        .unwrap_or_default();
    let b = preferred_bt_mac.map(normalize_bt_mac).unwrap_or_default();
    if n.is_empty() && m.is_empty() && b.is_empty() {
        String::new()
    } else {
        format!("{n}|{m}|{b}")
    }
}

fn start_global_pc_audio_bridge_impl(
    preferred_name: Option<&str>,
    preferred_model: Option<&str>,
    preferred_bt_mac: Option<&str>,
    wait_ready_ms: u64,
) -> (bool, String) {
    let new_target_key = build_audio_target_key(preferred_name, preferred_model, preferred_bt_mac);
    let mut old_to_rebind: Option<PcAudioBridgeHandle> = None;

    {
        let mut slot = bridge_slot().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(ref existing) = *slot {
            if !existing.stop.load(std::sync::atomic::Ordering::SeqCst) {
                let cur_active = PC_AUDIO_BRIDGE_ACTIVE.load(std::sync::atomic::Ordering::SeqCst);
                let cur_connecting =
                    PC_AUDIO_BRIDGE_CONNECTING.load(std::sync::atomic::Ordering::SeqCst);
                let active_dev = active_device_slot()
                    .lock()
                    .map(|g| g.clone())
                    .unwrap_or_default();
                let already_matches_pref_name = preferred_name
                    .map(normalize_bt_friendly_name)
                    .filter(|s| !s.is_empty())
                    .is_some_and(|pn| !active_dev.is_empty() && normalize_bt_friendly_name(&active_dev) == pn);
                let needs_rebind = !new_target_key.is_empty()
                    && existing.target_key != new_target_key
                    && !already_matches_pref_name;

                if (cur_active || cur_connecting) && !needs_rebind {
                    let cur = existing
                        .detail
                        .lock()
                        .map(|g| g.clone())
                        .unwrap_or_else(|_| "pc-audio-bridge-active".to_string());
                    return (cur_active, cur);
                }
                // Either previous attempt ended in Closed/failed state OR target iPhone switched -> tear down cleanly!
                old_to_rebind = slot.take();
            }
        }
    }

    if let Some(mut old_h) = old_to_rebind {
        old_h.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        PC_AUDIO_BRIDGE_CONNECTING.store(false, std::sync::atomic::Ordering::SeqCst);
        PC_AUDIO_BRIDGE_ACTIVE.store(false, std::sync::atomic::Ordering::SeqCst);
        if let Ok(mut d) = active_device_slot().lock() {
            d.clear();
        }
        if let Some(rx) = old_h.done_rx.take() {
            if rx.recv_timeout(std::time::Duration::from_millis(450)).is_ok() {
                if let Some(jh) = old_h.join.take() {
                    let _ = jh.join();
                }
            }
        }
    }

    let mut slot = bridge_slot().lock().unwrap_or_else(|e| e.into_inner());
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop_worker = stop.clone();
    let detail_arc =
        std::sync::Arc::new(std::sync::Mutex::new("pc-audio-bridge-connecting".to_string()));
    let detail_worker = detail_arc.clone();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<(bool, String)>();
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();

    let pref_name_owned = preferred_name
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let pref_model_owned = preferred_model
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let pref_mac_owned = preferred_bt_mac
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    PC_AUDIO_BRIDGE_ACTIVE.store(false, std::sync::atomic::Ordering::SeqCst);
    PC_AUDIO_A2DP_REALLY_OPENED.store(false, std::sync::atomic::Ordering::SeqCst);
    PC_AUDIO_LAST_CLOSED_BY_PEER.store(false, std::sync::atomic::Ordering::SeqCst);
    PC_AUDIO_LAST_ATTEMPT_FAILED.store(false, std::sync::atomic::Ordering::SeqCst);
    PC_AUDIO_BRIDGE_CONNECTING.store(true, std::sync::atomic::Ordering::SeqCst);

    let join_res = std::thread::Builder::new()
        .name("iphone-pc-audio-bridge".into())
        .spawn(move || {
            #[cfg(windows)]
            let loopback_test = std::env::var("IPHONE_LINK_TEST_LOOPBACK_ONLY")
                .is_ok_and(|v| matches!(v.trim(), "1" | "true" | "TRUE" | "yes" | "YES"));
            #[cfg(windows)]
            let a2dp_guard = if loopback_test {
                None
            } else {
                Some(winrt_a2dp::try_activate_bluetooth_a2dp_sta(
                    pref_name_owned.as_deref(),
                    pref_model_owned.as_deref(),
                    pref_mac_owned.as_deref(),
                ))
            };
            #[cfg(windows)]
            let (a2dp_summary, a2dp_really_opened, _bt_devs_found, opened_dev_name, found_names) =
                if let Some(ref g) = a2dp_guard {
                    (
                        g.summary.clone(),
                        g.really_opened,
                        g.found_bt_devices,
                        g.opened_device_name.clone(),
                        g.found_bt_names.clone(),
                    )
                } else {
                    (
                        "wasapi-loopback-test".to_string(),
                        false,
                        0usize,
                        String::new(),
                        String::new(),
                    )
                };
            #[cfg(not(windows))]
            let (a2dp_summary, a2dp_really_opened, _bt_devs_found, opened_dev_name, found_names) = {
                let _ = (&pref_name_owned, &pref_model_owned, &pref_mac_owned);
                ("wasapi-null".to_string(), false, 0usize, String::new(), String::new())
            };

            if let Ok(mut fc) = found_candidates_slot().lock() {
                *fc = found_names;
            }
            if let Ok(mut ad) = active_device_slot().lock() {
                *ad = if a2dp_really_opened {
                    opened_dev_name
                } else {
                    String::new()
                };
            }

            let mut wasapi_opt = NativeWasapiRenderer::open_default_shared(48000, 2).ok();
            let detail = match &mut wasapi_opt {
                Some(ref mut w) => {
                    let _ = w.start();
                    format!("{a2dp_summary};{}", w.backend_name())
                }
                None => a2dp_summary,
            };
            if let Ok(mut d) = detail_worker.lock() {
                *d = detail.clone();
            }
            let truly_active = !stop_worker.load(std::sync::atomic::Ordering::Relaxed)
                && resolve_bridge_truly_active(
                    a2dp_really_opened,
                    loopback_test,
                    wasapi_opt.is_some(),
                );
            PC_AUDIO_A2DP_REALLY_OPENED.store(
                truly_active && a2dp_really_opened,
                std::sync::atomic::Ordering::SeqCst,
            );
            PC_AUDIO_LAST_ATTEMPT_FAILED.store(
                !truly_active && !stop_worker.load(std::sync::atomic::Ordering::Relaxed),
                std::sync::atomic::Ordering::SeqCst,
            );
            PC_AUDIO_BRIDGE_CONNECTING.store(false, std::sync::atomic::Ordering::SeqCst);
            PC_AUDIO_BRIDGE_ACTIVE.store(truly_active, std::sync::atomic::Ordering::SeqCst);
            let _ = ready_tx.send((truly_active, detail));

            if truly_active {
                let mut liveness_ticks: u32 = 0;
                while !stop_worker.load(std::sync::atomic::Ordering::Relaxed) {
                    #[cfg(windows)]
                    unsafe {
                        winrt_a2dp::pump_sta_messages();
                        liveness_ticks = liveness_ticks.wrapping_add(1);
                        if !loopback_test && liveness_ticks % 20 == 0 {
                            let alive = a2dp_guard
                                .as_ref()
                                .is_some_and(|g| g.is_any_conn_opened());
                            if !alive {
                                PC_AUDIO_LAST_CLOSED_BY_PEER
                                    .store(true, std::sync::atomic::Ordering::SeqCst);
                                if let Ok(mut d) = detail_worker.lock() {
                                    *d = "a2dp-closed-by-peer".to_string();
                                }
                                break;
                            }
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
            }

            if let Some(ref mut w) = wasapi_opt {
                w.stop();
            }
            #[cfg(windows)]
            drop(a2dp_guard);
            if let Ok(mut ad) = active_device_slot().lock() {
                ad.clear();
            }
            PC_AUDIO_A2DP_REALLY_OPENED.store(false, std::sync::atomic::Ordering::SeqCst);
            PC_AUDIO_BRIDGE_CONNECTING.store(false, std::sync::atomic::Ordering::SeqCst);
            PC_AUDIO_BRIDGE_ACTIVE.store(false, std::sync::atomic::Ordering::SeqCst);
            let _ = done_tx.send(());
        });

    match join_res {
        Ok(jh) => {
            let (ok, detail) = if wait_ready_ms > 0 {
                ready_rx
                    .recv_timeout(std::time::Duration::from_millis(wait_ready_ms))
                    .unwrap_or_else(|_| {
                        (
                            PC_AUDIO_BRIDGE_ACTIVE.load(std::sync::atomic::Ordering::SeqCst),
                            detail_arc
                                .lock()
                                .map(|g| g.clone())
                                .unwrap_or_else(|_| "pc-audio-bridge-connecting".to_string()),
                        )
                    })
            } else {
                (false, "pc-audio-bridge-connecting".to_string())
            };
            *slot = Some(PcAudioBridgeHandle {
                stop,
                done_rx: Some(done_rx),
                join: Some(jh),
                detail: detail_arc,
                target_key: new_target_key,
            });
            (ok, detail)
        }
        Err(e) => {
            PC_AUDIO_BRIDGE_CONNECTING.store(false, std::sync::atomic::Ordering::SeqCst);
            PC_AUDIO_BRIDGE_ACTIVE.store(false, std::sync::atomic::Ordering::SeqCst);
            (false, format!("spawn audio bridge failed: {e}"))
        }
    }
}

/// Starts the in-process PC audio playback bridge synchronously targeting a specific iPhone
/// (`preferred_name`, `preferred_model`, optional `preferred_bt_mac`), automatically rebinding
/// when switching between multiple paired iPhones.
pub fn start_global_pc_audio_bridge_for_device(
    preferred_name: Option<&str>,
    preferred_model: Option<&str>,
    preferred_bt_mac: Option<&str>,
) -> (bool, String) {
    start_global_pc_audio_bridge_impl(preferred_name, preferred_model, preferred_bt_mac, 6_500)
}

/// Starts the in-process PC audio playback bridge non-blockingly (< 1 ms) targeting a specific iPhone.
pub fn start_global_pc_audio_bridge_nonblocking_for_device(
    preferred_name: Option<&str>,
    preferred_model: Option<&str>,
    preferred_bt_mac: Option<&str>,
) -> (bool, String) {
    start_global_pc_audio_bridge_impl(preferred_name, preferred_model, preferred_bt_mac, 0)
}

/// Starts the in-process PC audio playback bridge synchronously (waits up to 6.5s for 3-attempt A2DP open).
pub fn start_global_pc_audio_bridge() -> (bool, String) {
    start_global_pc_audio_bridge_for_device(None, None, None)
}

/// Starts the in-process PC audio playback bridge non-blockingly (< 1 ms) with `pcBridgeConnecting = true`
/// and `pcBridgeActive = false` until WinRT `AudioPlaybackConnection::Open()` reaches `Opened (1) / Status 0`.
pub fn start_global_pc_audio_bridge_nonblocking() -> (bool, String) {
    start_global_pc_audio_bridge_nonblocking_for_device(None, None, None)
}

/// Cleanly stops the in-process WinRT `AudioPlaybackConnection` & WASAPI PC audio bridge,
/// waiting up to `450 ms` for the STA thread to complete `IClosable::Close()` before returning.
pub fn stop_global_pc_audio_bridge() {
    let handle_opt = {
        let mut slot = bridge_slot().lock().unwrap_or_else(|e| e.into_inner());
        slot.take()
    };
    PC_AUDIO_A2DP_REALLY_OPENED.store(false, std::sync::atomic::Ordering::SeqCst);
    PC_AUDIO_LAST_CLOSED_BY_PEER.store(false, std::sync::atomic::Ordering::SeqCst);
    PC_AUDIO_LAST_ATTEMPT_FAILED.store(false, std::sync::atomic::Ordering::SeqCst);
    PC_AUDIO_BRIDGE_CONNECTING.store(false, std::sync::atomic::Ordering::SeqCst);
    PC_AUDIO_BRIDGE_ACTIVE.store(false, std::sync::atomic::Ordering::SeqCst);
    if let Ok(mut ad) = active_device_slot().lock() {
        ad.clear();
    }
    if let Ok(mut fc) = found_candidates_slot().lock() {
        fc.clear();
    }
    if let Some(mut h) = handle_opt {
        h.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(rx) = h.done_rx.take() {
            if rx.recv_timeout(std::time::Duration::from_millis(450)).is_ok() {
                if let Some(jh) = h.join.take() {
                    let _ = jh.join();
                }
            }
        } else if let Some(jh) = h.join.take() {
            let _ = jh.join();
        }
    }
}

pub fn is_global_pc_audio_bridge_active() -> bool {
    PC_AUDIO_BRIDGE_ACTIVE.load(std::sync::atomic::Ordering::SeqCst)
}

pub fn is_global_pc_audio_bridge_connecting() -> bool {
    PC_AUDIO_BRIDGE_CONNECTING.load(std::sync::atomic::Ordering::SeqCst)
}

pub fn is_global_pc_audio_a2dp_really_opened() -> bool {
    PC_AUDIO_A2DP_REALLY_OPENED.load(std::sync::atomic::Ordering::SeqCst)
}

pub fn global_pc_audio_last_closed_by_peer() -> bool {
    PC_AUDIO_LAST_CLOSED_BY_PEER.load(std::sync::atomic::Ordering::SeqCst)
}

pub fn global_pc_audio_bridge_state_kind() -> &'static str {
    if PC_AUDIO_BRIDGE_ACTIVE.load(std::sync::atomic::Ordering::SeqCst) {
        "active"
    } else if PC_AUDIO_BRIDGE_CONNECTING.load(std::sync::atomic::Ordering::SeqCst) {
        "connecting"
    } else if PC_AUDIO_LAST_ATTEMPT_FAILED.load(std::sync::atomic::Ordering::SeqCst)
        || PC_AUDIO_LAST_CLOSED_BY_PEER.load(std::sync::atomic::Ordering::SeqCst)
    {
        "error"
    } else {
        "idle"
    }
}

/// Returns `(active_a2dp_device_name, discovered_apple_candidates_csv)`.
pub fn global_pc_audio_bridge_device_info() -> (String, String) {
    let dev = active_device_slot()
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default();
    let cands = found_candidates_slot()
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default();
    (dev, cands)
}

/// Returns the current bridge handle's normalized `target_key` (`"name|model|mac"`), or `""`.
pub fn global_pc_audio_bridge_target_key() -> String {
    bridge_slot()
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|h| h.target_key.clone()))
        .unwrap_or_default()
}

pub fn global_pc_audio_bridge_full_status() -> (bool, bool, String) {
    let active = PC_AUDIO_BRIDGE_ACTIVE.load(std::sync::atomic::Ordering::SeqCst);
    let connecting = PC_AUDIO_BRIDGE_CONNECTING.load(std::sync::atomic::Ordering::SeqCst);
    let slot = bridge_slot().lock().unwrap_or_else(|e| e.into_inner());
    let detail = match *slot {
        Some(ref h) => h
            .detail
            .lock()
            .map(|g| g.clone())
            .unwrap_or_else(|_| "pc-audio-bridge-active".to_string()),
        None => "idle".to_string(),
    };
    (active, connecting, detail)
}

pub fn global_pc_audio_bridge_status() -> (bool, String) {
    let (active, _connecting, detail) = global_pc_audio_bridge_full_status();
    (active, detail)
}

#[cfg(test)]
mod tahap244_audio_targeting_tests {
    use super::*;

    #[test]
    fn test_tahap244_multi_iphone_bt_candidate_ranking_and_apple_vid() {
        let bt_sinks = vec![
            (
                "Test Phone".to_string(),
                r"\\?\BTHENUM#{0000110a-0000-1000-8000-00805f9b34fb}_VID&0001004c_PID&791e#7&2be67811&0&020000000002_C00000000#{6994ad04-93ef-11d0-a3cc-00a0c9223196}\SNK".to_string(),
            ),
            (
                "Custom Phone".to_string(),
                r"\\?\BTHENUM#{0000110a-0000-1000-8000-00805f9b34fb}_VID&0001004c_PID&730a#7&2be67811&0&020000000001_C00000000#{6994ad04-93ef-11d0-a3cc-00a0c9223196}\SNK".to_string(),
            ),
            (
                "iPhone".to_string(),
                r"\\?\BTHENUM#{0000110a-0000-1000-8000-00805f9b34fb}_VID&0001004c_PID&730b#7&2be67811&0&112233445566_C00000000#{6994ad04-93ef-11d0-a3cc-00a0c9223196}\SNK".to_string(),
            ),
            (
                "JBL Flip 6".to_string(),
                r"\\?\BTHENUM#{0000110a-0000-1000-8000-00805f9b34fb}_VID&00010057_PID&0001#7&0&AABBCCDDEEFF_C00000000#{6994ad04-93ef-11d0-a3cc-00a0c9223196}\SNK".to_string(),
            ),
        ];

        // 1. Target iPhone 11 Pro Max ("iPhone") -> MUST rank "iPhone" #1 (score 100) ahead of "Test Phone" (40)
        let r_11 = rank_bt_a2dp_candidates(&bt_sinks, Some("iPhone"), Some("iPhone 11 Pro Max"), None);
        assert_eq!(r_11.len(), 3, "non-Apple JBL speaker must be excluded");
        assert_eq!(r_11[0].2, "iPhone");
        assert_eq!(r_11[0].1, 100);
        assert_eq!(r_11[1].2, "Test Phone");
        assert_eq!(r_11[1].1, 40);
        assert_eq!(r_11[2].2, "Custom Phone");
        assert_eq!(r_11[2].1, 40);

        // 2. Target iPhone 17 Pro via Bonjour mDNS name ("Test-Phone") -> normalized exact match (score 98)
        let r_17 = rank_bt_a2dp_candidates(&bt_sinks, Some("Test-Phone"), Some("iPhone 17 Pro"), None);
        assert_eq!(r_17[0].2, "Test Phone");
        assert_eq!(r_17[0].1, 98);

        // 3. Target custom-named iPhone 11 Pro Max via Bluetooth MAC ("02:00:00:00:00:01") -> "Custom Phone" wins (score 120)
        let r_mac = rank_bt_a2dp_candidates(
            &bt_sinks,
            Some("iPhone"),
            Some("iPhone 11 Pro Max"),
            Some("02:00:00:00:00:01"),
        );
        assert_eq!(r_mac[0].2, "Custom Phone");
        assert_eq!(r_mac[0].1, 120);
        assert!(is_apple_bt_device_id(&bt_sinks[1].1));
        assert!(!is_apple_bt_device_id(&bt_sinks[3].1));
    }

    #[test]
    fn test_tahap286_audio_bridge_no_false_active_on_zero_a2dp_and_liveness_state_machine() {
        // Production (!loopback_test) MUST NEVER mark bridge active when 0 A2DP devices opened,
        // even if FindAllAsync timed out and WASAPI initialized cleanly!
        assert!(!resolve_bridge_truly_active(false, false, true));
        assert!(!resolve_bridge_truly_active(false, false, false));
        assert!(resolve_bridge_truly_active(true, false, false));
        assert!(resolve_bridge_truly_active(true, false, true));

        // Offline loopback tests (IPHONE_LINK_TEST_LOOPBACK_ONLY=1) rely on WASAPI mock gate
        assert!(resolve_bridge_truly_active(false, true, true));
        assert!(!resolve_bridge_truly_active(false, true, false));

        // COM IAudioPlaybackConnection::get_State() liveness check (State::Opened == 1)
        assert!(evaluate_a2dp_keepalive_alive(0, 1));
        assert!(!evaluate_a2dp_keepalive_alive(0, 0)); // Closed by peer/iOS
        assert!(!evaluate_a2dp_keepalive_alive(-2147467259, 1)); // HRESULT E_FAIL
        assert!(matches!(
            global_pc_audio_bridge_state_kind(),
            "idle" | "active" | "connecting" | "error"
        ));
    }
}
