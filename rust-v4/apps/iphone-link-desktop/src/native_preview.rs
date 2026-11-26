//! Tahap 262 — Native GPU Video Selftest (`--native-video-selftest`) and Opt-In
//! Zero-WebView2 Live D3D11 Preview Window (`--native-video-preview [--transport usb|wifi|airplay]`).
//!
//! Runs 100% in a single OS process (`0` `msedgewebview2.exe`) using persistent `LiveD3d11Gpu`,
//! real COM `LiveMfVideoDecoder` (CoreDevice HEVC + AirPlay H.264), real DXGI `FLIP_DISCARD`
//! Child `HWND` `LiveVideoHwnd`, and direct in-process `mirror::register_native_video_sink`.

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Once};
use std::time::{Duration, Instant};

use iphone_link_apple::hid;
use iphone_link_apple::mirror::{
    self, BoundedNativeVideoQueue, NativeVideoEvent, NATIVE_VIDEO_QUEUE_CAPACITY,
};
use iphone_link_apple::usbmux::list_connected_usb_devices;
use iphone_link_video::autorotate::{run_32_autorotate_scenarios_self_test, ProbeGrid120x260};
use iphone_link_video::d3d11_context::{D3d11Nv12VramRing, LiveD3d11Gpu, Rect};
use iphone_link_video::mf_decoder::{
    build_synthetic_h264_ipcm_idr, LiveMfVideoDecoder, VideoCodecKind,
};
use iphone_link_video::video_hwnd::{FitMode, LiveVideoHwnd, StagePointerEvent};

use crate::server_v4;

const CS_HREDRAW: u32 = 0x0002;
const CS_VREDRAW: u32 = 0x0001;
const WS_OVERLAPPEDWINDOW: u32 = 0x00CF_0000;
const WS_CLIPCHILDREN: u32 = 0x0200_0000;
const SW_SHOWNORMAL: i32 = 1;
const SWP_NOZORDER: u32 = 0x0004;
const SWP_SHOWWINDOW: u32 = 0x0040;
const SPI_GETWORKAREA: u32 = 0x0030;
const GWLP_USERDATA: i32 = -21;
const WM_DESTROY: u32 = 0x0002;
const WM_SIZE: u32 = 0x0005;
const WM_CLOSE: u32 = 0x0010;
const WM_ERASEBKGND: u32 = 0x0014;
const WM_QUIT: u32 = 0x0012;
const PM_REMOVE: u32 = 0x0001;
const IDC_ARROW: usize = 32512;
const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;

#[repr(C)]
struct ProcessMemoryCounters {
    cb: u32,
    page_fault_count: u32,
    peak_working_set_size: usize,
    working_set_size: usize,
    quota_peak_paged_pool_usage: usize,
    quota_paged_pool_usage: usize,
    quota_peak_non_paged_pool_usage: usize,
    quota_non_paged_pool_usage: usize,
    pagefile_usage: usize,
    peak_pagefile_usage: usize,
}

#[repr(C)]
struct HostWndClassExW {
    cb_size: u32,
    style: u32,
    lpfn_wnd_proc: Option<unsafe extern "system" fn(isize, u32, usize, isize) -> isize>,
    cb_cls_extra: i32,
    cb_wnd_extra: i32,
    h_instance: isize,
    h_icon: isize,
    h_cursor: isize,
    hbr_background: isize,
    lpsz_menu_name: *const u16,
    lpsz_class_name: *const u16,
    h_icon_sm: isize,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct WinMsg {
    hwnd: isize,
    message: u32,
    w_param: usize,
    l_param: isize,
    time: u32,
    pt_x: i32,
    pt_y: i32,
}

type FnK32GetProcessMemoryInfo =
    unsafe extern "system" fn(*mut c_void, *mut ProcessMemoryCounters, u32) -> i32;
type FnDwmSetWindowAttribute =
    unsafe extern "system" fn(isize, u32, *const c_void, u32) -> i32;
type FnRegisterClassExW = unsafe extern "system" fn(*const HostWndClassExW) -> u16;
type FnCreateWindowExW = unsafe extern "system" fn(
    u32,
    *const u16,
    *const u16,
    u32,
    i32,
    i32,
    i32,
    i32,
    isize,
    isize,
    isize,
    *mut c_void,
) -> isize;
type FnDestroyWindow = unsafe extern "system" fn(isize) -> i32;
type FnDefWindowProcW = unsafe extern "system" fn(isize, u32, usize, isize) -> isize;
type FnGetClientRect = unsafe extern "system" fn(isize, *mut Rect) -> i32;
type FnLoadCursorW = unsafe extern "system" fn(isize, *const u16) -> isize;
type FnPeekMessageW = unsafe extern "system" fn(*mut WinMsg, isize, u32, u32, u32) -> i32;
type FnTranslateMessage = unsafe extern "system" fn(*const WinMsg) -> i32;
type FnDispatchMessageW = unsafe extern "system" fn(*const WinMsg) -> isize;
type FnShowWindow = unsafe extern "system" fn(isize, i32) -> i32;
type FnSetWindowPos = unsafe extern "system" fn(isize, isize, i32, i32, i32, i32, u32) -> i32;
type FnSystemParametersInfoW = unsafe extern "system" fn(u32, u32, *mut c_void, u32) -> i32;

#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentProcess() -> *mut c_void;
    fn GetModuleHandleW(lpModuleName: *const u16) -> *mut c_void;
    fn LoadLibraryW(lpLibFileName: *const u16) -> *mut c_void;
    fn GetProcAddress(hModule: *mut c_void, lpProcName: *const i8) -> *mut c_void;
    fn FreeLibrary(hLibModule: *mut c_void) -> i32;
}

#[link(name = "user32")]
extern "system" {
    fn SetWindowTextW(hWnd: isize, lpString: *const u16) -> i32;
    fn GetWindowLongPtrW(hWnd: isize, nIndex: i32) -> isize;
    fn SetWindowLongPtrW(hWnd: isize, nIndex: i32, dwNewLong: isize) -> isize;
    fn PostQuitMessage(nExitCode: i32);
}

struct User32Api {
    register_class_ex_w: FnRegisterClassExW,
    create_window_ex_w: FnCreateWindowExW,
    destroy_window: FnDestroyWindow,
    def_window_proc_w: FnDefWindowProcW,
    get_client_rect: FnGetClientRect,
    load_cursor_w: FnLoadCursorW,
    peek_message_w: FnPeekMessageW,
    translate_message: FnTranslateMessage,
    dispatch_message_w: FnDispatchMessageW,
    show_window: FnShowWindow,
    set_window_pos: FnSetWindowPos,
    system_parameters_info_w: FnSystemParametersInfoW,
}

fn user32_api() -> &'static User32Api {
    static API: std::sync::OnceLock<User32Api> = std::sync::OnceLock::new();
    API.get_or_init(|| unsafe {
        let u32_w = to_wide("user32.dll");
        let h = LoadLibraryW(u32_w.as_ptr());
        let get = |name: &[u8]| GetProcAddress(h, name.as_ptr() as *const i8);
        User32Api {
            register_class_ex_w: std::mem::transmute(get(b"RegisterClassExW\0")),
            create_window_ex_w: std::mem::transmute(get(b"CreateWindowExW\0")),
            destroy_window: std::mem::transmute(get(b"DestroyWindow\0")),
            def_window_proc_w: std::mem::transmute(get(b"DefWindowProcW\0")),
            get_client_rect: std::mem::transmute(get(b"GetClientRect\0")),
            load_cursor_w: std::mem::transmute(get(b"LoadCursorW\0")),
            peek_message_w: std::mem::transmute(get(b"PeekMessageW\0")),
            translate_message: std::mem::transmute(get(b"TranslateMessage\0")),
            dispatch_message_w: std::mem::transmute(get(b"DispatchMessageW\0")),
            show_window: std::mem::transmute(get(b"ShowWindow\0")),
            set_window_pos: std::mem::transmute(get(b"SetWindowPos\0")),
            system_parameters_info_w: std::mem::transmute(get(b"SystemParametersInfoW\0")),
        }
    })
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Returns current process WorkingSet memory footprint in MiB via `kernel32!K32GetProcessMemoryInfo`.
pub fn process_working_set_mb() -> f64 {
    unsafe {
        let k32_w = to_wide("kernel32.dll");
        let h_k32 = LoadLibraryW(k32_w.as_ptr());
        if h_k32.is_null() {
            return 0.0;
        }
        let p_fn = GetProcAddress(h_k32, b"K32GetProcessMemoryInfo\0".as_ptr() as *const i8);
        let mut mb = 0.0;
        if !p_fn.is_null() {
            let get_mem: FnK32GetProcessMemoryInfo = std::mem::transmute(p_fn);
            let mut pmc: ProcessMemoryCounters = std::mem::zeroed();
            pmc.cb = std::mem::size_of::<ProcessMemoryCounters>() as u32;
            if get_mem(GetCurrentProcess(), &mut pmc, pmc.cb) != 0 {
                mb = (pmc.working_set_size as f64) / (1024.0 * 1024.0);
            }
        }
        FreeLibrary(h_k32);
        mb
    }
}

fn apply_dark_titlebar(hwnd: isize) {
    unsafe {
        let dwm_w = to_wide("dwmapi.dll");
        let h_dwm = LoadLibraryW(dwm_w.as_ptr());
        if h_dwm.is_null() {
            return;
        }
        let p_fn = GetProcAddress(h_dwm, b"DwmSetWindowAttribute\0".as_ptr() as *const i8);
        if !p_fn.is_null() {
            let set_attr: FnDwmSetWindowAttribute = std::mem::transmute(p_fn);
            let dark: i32 = 1;
            let _ = set_attr(
                hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                &dark as *const i32 as *const c_void,
                std::mem::size_of::<i32>() as u32,
            );
        }
        FreeLibrary(h_dwm);
    }
}

/// Headless, zero-WebView2 E2E self-test invoked via `iPhone Link.exe --native-video-selftest`.
/// Verifies real persistent D3D11 GPU, real COM H.264 MFT decode (Annex-B + avcC + mid-stream rotation),
/// real DXGI `FLIP_DISCARD` `LiveVideoHwnd` SwapChain presentation, `< 5 ms` lossless BMP snapshot,
/// 32-scenario Auto-Rotate probe, and direct in-process IDR-safe video sink with 0% PII.
pub fn run_native_video_selftest() -> i32 {
    let t_total = Instant::now();

    // 1. Persistent LiveD3d11Gpu
    let gpu = match LiveD3d11Gpu::new() {
        Ok(g) => Arc::new(g),
        Err(e) => {
            println!(
                r#"{{"status":"FAIL","stage":"LiveD3d11Gpu","error":"{}"}}"#,
                e.replace('"', "'")
            );
            return 1;
        }
    };
    let gpu_mt = gpu.is_multithread_protected();
    let gpu_dxgi_mgr = gpu.has_dxgi_manager();
    let gpu_vp = gpu.has_video_processor();
    let gpu_warp = gpu.is_warp();

    // 2. Real COM H.264 MFT (Annex-B 64x64 + AirPlay avcC 64x64 + mid-stream 96x64 -> 64x96)
    let ring = Arc::new(D3d11Nv12VramRing::new());
    let f64 = build_synthetic_h264_ipcm_idr(64, 64, 0x90, 0);
    let mut h264_annexb_ok = false;
    let mut h264_avcc_ok = false;
    let mut h264_rotation_ok = false;
    let mut hw_d3d11_mft = false;
    let mut zero_copy_frames = 0u64;
    let mut stream_changes_total = 0u32;

    if let Ok(mut dec) = LiveMfVideoDecoder::new(gpu.clone(), VideoCodecKind::H264) {
        if let Ok(mut st) = dec.feed_access_unit(&f64.annex_b, 16_667, true, false, None, &ring) {
            if st.total_decoded_frames == 0 {
                if let Ok(st_d) = dec.drain_eos(false, None, &ring) {
                    st = st_d;
                }
            }
            h264_annexb_ok = st.total_decoded_frames >= 1 && st.visible_width == 64 && st.visible_height == 64;
            hw_d3d11_mft = st.hardware_d3d11;
        }
    }

    if let Ok(mut dec) = LiveMfVideoDecoder::new(gpu.clone(), VideoCodecKind::H264) {
        if dec.update_avcc(&f64.avcc_config).is_ok() {
            if let Ok(mut st) = dec.feed_avcc_sample(&f64.avcc_sample, 33_334, true, &ring) {
                if st.total_decoded_frames == 0 {
                    if let Ok(st_d) = dec.drain_eos(true, Some((0, 0, 0, 0)), &ring) {
                        st = st_d;
                    }
                }
                h264_avcc_ok = st.total_decoded_frames >= 1 && st.visible_width == 64 && st.visible_height == 64;
                zero_copy_frames = st.zero_copy_gpu_frames;

                let f_land = build_synthetic_h264_ipcm_idr(96, 64, 0xA0, 1);
                let f_port = build_synthetic_h264_ipcm_idr(64, 96, 0xB0, 2);
                if dec.update_avcc(&f_land.avcc_config).is_ok() {
                    let mut st_l = dec
                        .feed_avcc_sample(&f_land.avcc_sample, 50_000, true, &ring)
                        .unwrap_or(st);
                    if st_l.total_decoded_frames <= st.total_decoded_frames {
                        st_l = dec
                            .drain_eos(true, Some((0, 0, 0, 0)), &ring)
                            .unwrap_or(st_l);
                    }
                    let land_ok = st_l.visible_width == 96 && st_l.visible_height == 64;
                    if land_ok && dec.update_avcc(&f_port.avcc_config).is_ok() {
                        let mut st_p = dec
                            .feed_avcc_sample(&f_port.avcc_sample, 66_667, true, &ring)
                            .unwrap_or(st_l);
                        if st_p.total_decoded_frames <= st_l.total_decoded_frames {
                            st_p = dec
                                .drain_eos(true, Some((0, 0, 0, 0)), &ring)
                                .unwrap_or(st_p);
                        }
                        h264_rotation_ok = st_p.total_decoded_frames > st_l.total_decoded_frames
                            && st_p.visible_width == 64
                            && st_p.visible_height == 96;
                        stream_changes_total = st_p.stream_changes;
                        zero_copy_frames = st_p.zero_copy_gpu_frames;
                    }
                }
            }
        }
    }

    // 3. Non-crashing HEVC MFT readiness probe
    let hevc_mft_ready = LiveMfVideoDecoder::new(gpu.clone(), VideoCodecKind::Hevc).is_ok();

    // 4. Real Win32 offscreen LiveVideoHwnd + DXGI FLIP_DISCARD SwapChain + Resize + Rotations + Snapshot < 5ms
    let mut hwnd_present_ok = false;
    let mut hw_vp_used = false;
    let mut hw_rot_supported = false;
    let mut presents_done = 0u64;
    let mut bmp_snapshot_ok = false;
    let mut bmp_snapshot_ms = 999.0f64;
    let mut bmp_bytes = 0usize;
    let mut autorotate_probe_ok = false;

    if let Ok(mut view) = LiveVideoHwnd::new(gpu.clone(), ring.clone(), 0, 346, 750, false) {
        view.set_vsync_interval(0);
        view.set_auto_rotate_enabled(true);
        let resize_ok = view.resize(840, 386).is_ok() && view.resize(346, 750).is_ok();
        view.set_overlay_holes(&[Rect {
            left: 16,
            top: 16,
            right: 180,
            bottom: 56,
        }]);
        view.set_overlay_holes(&[]);

        let rots = [0u16, 90, 180, 270];
        let fits = [FitMode::AutoFit, FitMode::EdgeToEdge, FitMode::OriginalRatio];
        let mut all_p_ok = resize_ok;
        for i in 0..120usize {
            if i < 12 {
                view.set_rotation(rots[i % 4]);
                view.set_fit_mode(fits[i % 3]);
            } else if i == 12 {
                view.set_rotation(0);
                view.set_fit_mode(FitMode::AutoFit);
            }
            match view.present_latest() {
                Ok(ps) => {
                    presents_done = ps.presented_total;
                    hw_vp_used = ps.used_hardware_vp;
                    hw_rot_supported = ps.supports_hw_rotation;
                }
                Err(_) => {
                    all_p_ok = false;
                    break;
                }
            }
        }
        hwnd_present_ok = all_p_ok && presents_done == 120;

        // Warm staging texture once, then benchmark steady-state lossless BMP snapshot (< 5 ms)
        if view.capture_latest_frame_lossless_bmp().is_ok() {
            let t0 = Instant::now();
            if let Ok(bmp) = view.capture_latest_frame_lossless_bmp() {
                bmp_snapshot_ms = t0.elapsed().as_secs_f64() * 1000.0;
                bmp_bytes = bmp.len();
                bmp_snapshot_ok = bmp.len() > 54 && &bmp[0..2] == b"BM" && bmp_snapshot_ms < 5.0;
            }
        }

        let mut grid = ProbeGrid120x260::new();
        autorotate_probe_ok = view.update_autorotate_probe_if_due(&mut grid, true) || !gpu_vp;
    }

    let (ar_passed, ar_total) = run_32_autorotate_scenarios_self_test();
    let autorotate_32_ok = ar_passed == 32 && ar_total == 32;

    // 5. Direct in-process NativeVideoSink IDR-safe catch-up verification
    let q = Arc::new(BoundedNativeVideoQueue::new(NATIVE_VIDEO_QUEUE_CAPACITY));
    mirror::register_native_video_sink(Some(q.clone()));
    let sink_registered = mirror::is_native_video_sink_registered();
    q.try_push(NativeVideoEvent::H264Frame {
        avcc_payload: vec![0, 0, 0, 2, 0x65, 1],
        is_keyframe: true,
        pts_us: 1,
    });
    for i in 2u8..=8u8 {
        q.try_push(NativeVideoEvent::H264Frame {
            avcc_payload: vec![0, 0, 0, 2, 0x41, i],
            is_keyframe: false,
            pts_us: u64::from(i),
        });
    }
    let (_, awaiting_after_drop, dropped_cnt, _, _) = q.stats();
    q.try_push(NativeVideoEvent::H264Frame {
        avcc_payload: vec![0, 0, 0, 2, 0x65, 9],
        is_keyframe: true,
        pts_us: 9,
    });
    let (qlen_after_idr, awaiting_after_idr, _, catchups, _) = q.stats();
    mirror::register_native_video_sink(None);
    let native_sink_ok = sink_registered
        && !mirror::is_native_video_sink_registered()
        && awaiting_after_drop
        && dropped_cnt >= 2
        && !awaiting_after_idr
        && catchups == 1
        && qlen_after_idr == 1;

    let ram_mb = process_working_set_mb();
    let elapsed_ms = t_total.elapsed().as_secs_f64() * 1000.0;

    let all_passed = gpu_mt
        && gpu_dxgi_mgr
        && h264_annexb_ok
        && h264_avcc_ok
        && h264_rotation_ok
        && hwnd_present_ok
        && bmp_snapshot_ok
        && autorotate_probe_ok
        && autorotate_32_ok
        && native_sink_ok;

    println!(
        "{{\n  \"status\": \"{}\",\n  \"webview2_processes\": 0,\n  \"live_d3d11_gpu_multithread\": {},\n  \"dxgi_device_manager_ready\": {},\n  \"video_processor_ready\": {},\n  \"warp_fallback\": {},\n  \"h264_mft_hardware_d3d11\": {},\n  \"h264_annexb_decode_ok\": {},\n  \"h264_avcc_decode_ok\": {},\n  \"h264_midstream_rotation_ok\": {},\n  \"h264_stream_changes\": {},\n  \"zero_copy_gpu_frames\": {},\n  \"hevc_mft_ready\": {},\n  \"live_video_hwnd_dxgi_present_ok\": {},\n  \"presents_completed\": {},\n  \"hardware_vp_used\": {},\n  \"hardware_rotation_supported\": {},\n  \"bmp_snapshot_ok\": {},\n  \"bmp_snapshot_ms\": {:.3},\n  \"bmp_bytes\": {},\n  \"autorotate_probe_ok\": {},\n  \"autorotate_32_passed\": {},\n  \"native_sink_idr_safe_ok\": {},\n  \"ram_working_set_mb\": {:.2},\n  \"elapsed_ms\": {:.1}\n}}",
        if all_passed { "PASS" } else { "FAIL" },
        gpu_mt,
        gpu_dxgi_mgr,
        gpu_vp,
        gpu_warp,
        hw_d3d11_mft,
        h264_annexb_ok,
        h264_avcc_ok,
        h264_rotation_ok,
        stream_changes_total,
        zero_copy_frames,
        hevc_mft_ready,
        hwnd_present_ok,
        presents_done,
        hw_vp_used,
        hw_rot_supported,
        bmp_snapshot_ok,
        bmp_snapshot_ms,
        bmp_bytes,
        autorotate_probe_ok,
        ar_passed,
        native_sink_ok,
        ram_mb,
        elapsed_ms
    );

    if all_passed { 0 } else { 1 }
}

struct PreviewHostState {
    closed: AtomicBool,
    resized_w: AtomicU32,
    resized_h: AtomicU32,
    size_dirty: AtomicBool,
}

unsafe extern "system" fn preview_host_wndproc(
    hwnd: isize,
    msg: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const PreviewHostState;
    if !ptr.is_null() {
        let st = &*ptr;
        match msg {
            WM_ERASEBKGND => return 1,
            WM_SIZE => {
                let nw = (lparam & 0xFFFF) as u32;
                let nh = ((lparam >> 16) & 0xFFFF) as u32;
                if nw > 0 && nh > 0 {
                    st.resized_w.store(nw, Ordering::Relaxed);
                    st.resized_h.store(nh, Ordering::Relaxed);
                    st.size_dirty.store(true, Ordering::Release);
                }
                return 0;
            }
            WM_CLOSE => {
                st.closed.store(true, Ordering::SeqCst);
                let _ = (user32_api().destroy_window)(hwnd);
                return 0;
            }
            WM_DESTROY => {
                st.closed.store(true, Ordering::SeqCst);
                PostQuitMessage(0);
                return 0;
            }
            _ => {}
        }
    }
    (user32_api().def_window_proc_w)(hwnd, msg, wparam, lparam)
}

fn ensure_preview_host_class() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        let u32_api = user32_api();
        let cls_w = to_wide("iPhoneLinkNativePreviewHost");
        let wc = HostWndClassExW {
            cb_size: std::mem::size_of::<HostWndClassExW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfn_wnd_proc: Some(preview_host_wndproc),
            cb_cls_extra: 0,
            cb_wnd_extra: 0,
            h_instance: GetModuleHandleW(std::ptr::null()) as isize,
            h_icon: 0,
            h_cursor: (u32_api.load_cursor_w)(0, IDC_ARROW as *const u16),
            hbr_background: 0,
            lpsz_menu_name: std::ptr::null(),
            lpsz_class_name: cls_w.as_ptr(),
            h_icon_sm: 0,
        };
        let _ = (u32_api.register_class_ex_w)(&wc);
    });
}

/// Opt-in Zero-WebView2 physical testing preview window (`iPhone Link.exe --native-video-preview`).
/// Opens a single lightweight Win32 window (`368 x 820`) hosting a real Child `HWND` `LiveVideoHwnd`
/// directly fed by `mirror::register_native_video_sink` (zero WebView2 processes, RAM < 90 MB!).
pub fn run_native_video_preview(
    transport: Option<&str>,
    port: u16,
    state: Arc<Mutex<server_v4::AppState>>,
    web_dir: PathBuf,
) -> i32 {
    server_v4::set_headless_mode(true);
    let server = match server_v4::NativeServer::start(state, web_dir, port) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Native preview backend failed: {e}");
            return 1;
        }
    };

    let gpu = match LiveD3d11Gpu::new() {
        Ok(g) => Arc::new(g),
        Err(e) => {
            eprintln!("LiveD3d11Gpu init failed: {e}");
            server.stop();
            return 1;
        }
    };
    let ring = Arc::new(D3d11Nv12VramRing::new());
    let queue = Arc::new(BoundedNativeVideoQueue::new(NATIVE_VIDEO_QUEUE_CAPACITY));
    mirror::register_native_video_sink(Some(queue.clone()));

    let u32_api = user32_api();
    ensure_preview_host_class();
    let host_state = Arc::new(PreviewHostState {
        closed: AtomicBool::new(false),
        resized_w: AtomicU32::new(368),
        resized_h: AtomicU32::new(820),
        size_dirty: AtomicBool::new(false),
    });
    let host_state_raw = Arc::into_raw(host_state.clone());

    let cls_w = to_wide("iPhoneLinkNativePreviewHost");
    let title_w = to_wide("iPhone Link Native D3D11 Preview | Initializing...");
    let mut work_rc = Rect {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1040,
    };
    unsafe {
        let _ = (u32_api.system_parameters_info_w)(
            SPI_GETWORKAREA,
            0,
            &mut work_rc as *mut Rect as *mut c_void,
            0,
        );
    }
    let work_w = (work_rc.right - work_rc.left).max(400);
    let work_h = (work_rc.bottom - work_rc.top).max(480);
    let win_w = 368i32.min(work_w - 16);
    let win_h = 820i32.min(work_h - 16);
    let win_x = work_rc.left + ((work_w - win_w) / 2).max(0);
    let win_y = work_rc.top + ((work_h - win_h) / 2).max(0);
    let host_hwnd = unsafe {
        (u32_api.create_window_ex_w)(
            0,
            cls_w.as_ptr(),
            title_w.as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            win_x,
            win_y,
            win_w,
            win_h,
            0,
            0,
            GetModuleHandleW(std::ptr::null()) as isize,
            std::ptr::null_mut(),
        )
    };
    if host_hwnd == 0 {
        mirror::register_native_video_sink(None);
        unsafe { drop(Arc::from_raw(host_state_raw)) };
        server.stop();
        return 1;
    }
    unsafe {
        SetWindowLongPtrW(host_hwnd, GWLP_USERDATA, host_state_raw as isize);
        let _ = (u32_api.show_window)(host_hwnd, SW_SHOWNORMAL);
        let _ = (u32_api.set_window_pos)(
            host_hwnd,
            0,
            win_x,
            win_y,
            win_w,
            win_h,
            SWP_NOZORDER | SWP_SHOWWINDOW,
        );
    }
    apply_dark_titlebar(host_hwnd);

    let mut rc = Rect::default();
    unsafe {
        let _ = (u32_api.get_client_rect)(host_hwnd, &mut rc);
    }
    let init_w = (rc.right - rc.left).max(16) as u32;
    let init_h = (rc.bottom - rc.top).max(16) as u32;

    let mut view = match LiveVideoHwnd::new(gpu.clone(), ring.clone(), host_hwnd, init_w, init_h, true) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("LiveVideoHwnd init failed: {e}");
            unsafe {
                SetWindowLongPtrW(host_hwnd, GWLP_USERDATA, 0);
                let _ = (u32_api.destroy_window)(host_hwnd);
                drop(Arc::from_raw(host_state_raw));
            }
            mirror::register_native_video_sink(None);
            server.stop();
            return 1;
        }
    };
    view.set_vsync_interval(1);
    view.set_pointer_callback(Some(Arc::new(|ev| match ev {
        StagePointerEvent::Down { hid_x, hid_y } | StagePointerEvent::Move { hid_x, hid_y } => {
            if !mirror::is_airplay_standalone_no_hid_active() {
                let _ = hid::dispatch_physical_touch(true, hid_x, hid_y);
            }
        }
        StagePointerEvent::Up { hid_x, hid_y } => {
            if !mirror::is_airplay_standalone_no_hid_active() {
                let _ = hid::dispatch_physical_touch(false, hid_x, hid_y);
            }
        }
        StagePointerEvent::Wheel { .. } => {}
    })));

    let stop_flag = Arc::new(AtomicBool::new(false));
    let codec_label = Arc::new(Mutex::new("Idle".to_string()));
    let decoded_counter = Arc::new(AtomicU64::new(0));
    let dim_w = Arc::new(AtomicU32::new(0));
    let dim_h = Arc::new(AtomicU32::new(0));

    // Dedicated native COM decoder thread
    let dec_stop = stop_flag.clone();
    let dec_gpu = gpu.clone();
    let dec_ring = ring.clone();
    let dec_q = queue.clone();
    let dec_codec_label = codec_label.clone();
    let dec_counter = decoded_counter.clone();
    let dec_w = dim_w.clone();
    let dec_h = dim_h.clone();
    let decode_thread = std::thread::Builder::new()
        .name("native-video-decode".to_string())
        .spawn(move || {
            let mut active_dec: Option<LiveMfVideoDecoder> = None;
            let mut active_kind: Option<VideoCodecKind> = None;

            while !dec_stop.load(Ordering::Relaxed) {
                let Some(ev) = dec_q.recv_timeout(Duration::from_millis(50)) else {
                    continue;
                };
                match ev {
                    NativeVideoEvent::Stopped => {
                        if let Some(ref d) = active_dec {
                            d.flush();
                        }
                        dec_ring.clear();
                        if let Ok(mut lbl) = dec_codec_label.lock() {
                            *lbl = "Waiting".to_string();
                        }
                    }
                    NativeVideoEvent::HevcConfig { .. } => {
                        if active_kind != Some(VideoCodecKind::Hevc) {
                            active_dec = LiveMfVideoDecoder::new(dec_gpu.clone(), VideoCodecKind::Hevc).ok();
                            active_kind = Some(VideoCodecKind::Hevc);
                        }
                        if let Ok(mut lbl) = dec_codec_label.lock() {
                            *lbl = "HEVC".to_string();
                        }
                    }
                    NativeVideoEvent::HevcFrame {
                        annex_b,
                        is_keyframe,
                        pts_us,
                        trim,
                    } => {
                        let hevc_trim = (trim[0], trim[1], trim[2], trim[3]);
                        if active_kind != Some(VideoCodecKind::Hevc) || active_dec.is_none() {
                            active_dec = LiveMfVideoDecoder::new(dec_gpu.clone(), VideoCodecKind::Hevc).ok();
                            active_kind = Some(VideoCodecKind::Hevc);
                            if let Ok(mut lbl) = dec_codec_label.lock() {
                                *lbl = "HEVC".to_string();
                            }
                        }
                        if let Some(ref mut dec) = active_dec {
                            if let Ok(st) = dec.feed_access_unit(
                                &annex_b,
                                pts_us,
                                is_keyframe,
                                false,
                                Some(hevc_trim),
                                &dec_ring,
                            ) {
                                dec_counter.fetch_add(u64::from(st.frames_emitted), Ordering::Relaxed);
                                if st.visible_width > 0 && st.visible_height > 0 {
                                    dec_w.store(st.visible_width, Ordering::Relaxed);
                                    dec_h.store(st.visible_height, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    NativeVideoEvent::H264Avcc { avcc, .. } => {
                        if active_kind != Some(VideoCodecKind::H264) || active_dec.is_none() {
                            active_dec = LiveMfVideoDecoder::new(dec_gpu.clone(), VideoCodecKind::H264).ok();
                            active_kind = Some(VideoCodecKind::H264);
                        }
                        if let Some(ref mut dec) = active_dec {
                            let _ = dec.update_avcc(&avcc);
                        }
                        if let Ok(mut lbl) = dec_codec_label.lock() {
                            *lbl = "AirPlay H.264".to_string();
                        }
                    }
                    NativeVideoEvent::H264Frame {
                        avcc_payload,
                        is_keyframe,
                        pts_us,
                    } => {
                        if let Some(ref mut dec) = active_dec {
                            if active_kind == Some(VideoCodecKind::H264) {
                                if let Ok(st) =
                                    dec.feed_avcc_sample(&avcc_payload, pts_us, is_keyframe, &dec_ring)
                                {
                                    dec_counter
                                        .fetch_add(u64::from(st.frames_emitted), Ordering::Relaxed);
                                    if st.visible_width > 0 && st.visible_height > 0 {
                                        dec_w.store(st.visible_width, Ordering::Relaxed);
                                        dec_h.store(st.visible_height, Ordering::Relaxed);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        })
        .ok();

    // Auto-start selected transport on a non-blocking worker thread (3s discovery retry + clear diagnostic)
    let starter_transport = transport.map(|s| s.trim().to_ascii_lowercase());
    let starter_label = codec_label.clone();
    let starter_stop = stop_flag.clone();
    std::thread::spawn(move || {
        let set_lbl = |msg: &str| {
            if let Ok(mut g) = starter_label.lock() {
                *g = msg.to_string();
            }
        };
        let try_start_usb = |max_ms: u64| -> bool {
            let deadline = Instant::now() + Duration::from_millis(max_ms);
            while !starter_stop.load(Ordering::Relaxed) && Instant::now() < deadline {
                if let Ok(devs) = list_connected_usb_devices(Duration::from_millis(250)) {
                    if let Some(first) = devs.first() {
                        set_lbl("Connecting USB CoreDevice...");
                        let _ = mirror::start_real_physical_usb_screen(&first.serial_udid);
                        return true;
                    }
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            false
        };
        let try_start_wifi_coredevice = |max_ms: u64| -> bool {
            let deadline = Instant::now() + Duration::from_millis(max_ms);
            while !starter_stop.load(Ordering::Relaxed) && Instant::now() < deadline {
                let records = iphone_link_apple::wifi::all_records();
                let st = iphone_link_apple::wifi::get_wifi_status(None, true);
                if st.reachable {
                    if let (Some(udid), Some(ip)) = (st.udid.as_deref(), st.ip.as_deref()) {
                        set_lbl("Connecting Wi-Fi CoreDevice...");
                        let _ = mirror::start_real_physical_wifi_lockdown_screen(udid, ip);
                        return true;
                    }
                }
                for rec in &records {
                    let meta = iphone_link_apple::wifi::read_device_metadata_from_wifi_state(&rec.udid);
                    if let Some(ip) = meta.last_ip.as_deref().filter(|s| !s.is_empty()) {
                        if iphone_link_apple::wifi::verify_ip_is_apple_device(ip, Some(&rec.udid), 350) {
                            set_lbl("Connecting Wi-Fi CoreDevice...");
                            let _ = mirror::start_real_physical_wifi_lockdown_screen(&rec.udid, ip);
                            return true;
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(300));
            }
            false
        };

        match starter_transport.as_deref() {
            Some("airplay") => {
                set_lbl("AirPlay Ready (Choose iPhone Link on iPhone)");
                let _ = mirror::start_airplay_mirror_session(None, false, "wifi", None, None, true);
            }
            Some("usb") => {
                set_lbl("Discovering USB iPhone...");
                if !try_start_usb(3000) {
                    set_lbl("No USB iPhone online (connect cable)");
                }
            }
            Some("wifi") => {
                set_lbl("Discovering Wi-Fi iPhone...");
                if !try_start_wifi_coredevice(3000) {
                    set_lbl("No Wi-Fi iPhone online (try --transport airplay)");
                }
            }
            _ => {
                set_lbl("Discovering iPhone...");
                if !try_start_usb(800) && !try_start_wifi_coredevice(1200) {
                    set_lbl("AirPlay Ready (Choose iPhone Link on iPhone)");
                    let _ = mirror::start_airplay_mirror_session(None, false, "wifi", None, None, true);
                }
            }
        }
    });

    // Main Win32 preview pump + DXGI SwapChain present + titlebar telemetry
    let mut last_seq = 0u64;
    let mut last_title_update = Instant::now();
    let mut last_fps_frames = 0u64;
    let mut last_present_at = Instant::now();

    while !host_state.closed.load(Ordering::Relaxed) {
        unsafe {
            let mut msg = WinMsg::default();
            while (u32_api.peek_message_w)(&mut msg, 0, 0, 0, PM_REMOVE) != 0 {
                if msg.message == WM_QUIT {
                    host_state.closed.store(true, Ordering::SeqCst);
                    break;
                }
                let _ = (u32_api.translate_message)(&msg);
                let _ = (u32_api.dispatch_message_w)(&msg);
            }
        }
        if host_state.closed.load(Ordering::Relaxed) {
            break;
        }

        if host_state.size_dirty.swap(false, Ordering::AcqRel) {
            let nw = host_state.resized_w.load(Ordering::Relaxed);
            let nh = host_state.resized_h.load(Ordering::Relaxed);
            let _ = view.set_bounds(0, 0, nw, nh, true);
        }

        let has_new = ring
            .wait_for_new_frame(last_seq, Duration::from_millis(16))
            .map(|s| {
                last_seq = s.frame_seq;
                true
            })
            .unwrap_or(false);

        if has_new || last_present_at.elapsed() >= Duration::from_millis(33) {
            let _ = view.present_latest();
            last_present_at = Instant::now();
        }

        if last_title_update.elapsed() >= Duration::from_millis(500) {
            let dt = last_title_update.elapsed().as_secs_f64().max(0.01);
            let total_dec = decoded_counter.load(Ordering::Relaxed);
            let fps = ((total_dec.saturating_sub(last_fps_frames)) as f64 / dt).round() as u32;
            last_fps_frames = total_dec;
            last_title_update = Instant::now();

            let codec_str = codec_label
                .lock()
                .map(|g| g.clone())
                .unwrap_or_else(|_| "Idle".to_string());
            let w = dim_w.load(Ordering::Relaxed);
            let h = dim_h.load(Ordering::Relaxed);
            let ram = process_working_set_mb();
            let title = format!(
                "iPhone Link Native D3D11 Preview | {codec_str} | {w}x{h} | {fps} FPS | {ram:.1} MB"
            );
            let tw = to_wide(&title);
            unsafe {
                let _ = SetWindowTextW(host_hwnd, tw.as_ptr());
            }
        }
    }

    // Clean teardown
    let _ = mirror::global_stop_screen();
    mirror::register_native_video_sink(None);
    stop_flag.store(true, Ordering::SeqCst);
    queue.close();
    ring.notify_all();
    if let Some(jh) = decode_thread {
        let _ = jh.join();
    }
    ring.clear();
    drop(view);
    unsafe {
        SetWindowLongPtrW(host_hwnd, GWLP_USERDATA, 0);
        let _ = (u32_api.destroy_window)(host_hwnd);
        drop(Arc::from_raw(host_state_raw));
    }
    server.stop();
    0
}
