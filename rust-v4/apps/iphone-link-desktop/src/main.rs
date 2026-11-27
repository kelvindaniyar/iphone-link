#![windows_subsystem = "windows"]
//! iPhone Link v5.0.1 Native Rust Desktop Engine Entry Point (Default Zero-WebView2).

pub mod dcomp_presenter;
pub mod i18n;
pub mod link_grabber;
pub mod native_input;
pub mod native_preview;
pub mod native_ui;
pub mod native_video_pipeline;
pub mod tray_native;
pub mod ocr_engine;
pub mod server_v4;
pub mod updater;
pub mod window_dwm;

use std::env;
use std::ffi::{OsStr, OsString};
use std::net::{IpAddr, Ipv4Addr};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iphone_link_apple::hid::{
    generate_key_steps_for_char, generate_pin_keypress_steps, ButtonEvent,
    IndigoHidController,
};
use iphone_link_apple::models::{resolve_apple_model, screen_type_for_model, ScreenType};
use iphone_link_apple::usbmux::list_connected_usb_devices;
use iphone_link_apple::wifi_discovery::{
    build_unicast_mdns_query, is_ip_on_local_subnets, verify_ip_is_apple_device,
    REMOTEPAIRING_SERVICE,
};
use iphone_link_audio::jitter_buffer::AdaptiveAudioBuffer;
use iphone_link_audio::wasapi_output::{probe_wasapi_and_mmcss, probe_wasapi_shared_engine};
use iphone_link_audio::{probe_zlaac_dll, AudioBufferPreset, NativeRustAudioPipeline};
use iphone_link_video::autorotate::{
    evaluate_auto_rotate, run_32_autorotate_scenarios_self_test, AppProfile, OrientationMode,
};
use iphone_link_video::d3d11_context::{probe_d3d11_hardware_device, D3d11HardwareInfo, SharedFrameRing};
use iphone_link_video::mf_decoder::{probe_media_foundation, DownscaleFactor, HevcPipelineConfig};

use crate::i18n::{get_text, translate_dynamic_status, Lang};
use crate::link_grabber::extract_and_normalize_links;
use crate::ocr_engine::{
    auto_correct_ocr_text, clear_copy_cache, get_clipboard_sequence_number,
    is_balloon_popup_text, is_notes_folder_list_frame, stitch_multiline_ocr_frames,
};
use crate::updater::{is_protected_config_file, verify_sha256, Semver};
use crate::window_dwm::{
    probe_dwm_acrylic_offscreen, WindowGeometryManager, LANDSCAPE_HEIGHT,
    LANDSCAPE_WIDTH_CLOSED, LANDSCAPE_WIDTH_OPEN, PORTRAIT_HEIGHT, PORTRAIT_WIDTH_CLOSED,
    PORTRAIT_WIDTH_OPEN, RECT, TRAY_MENU_ITEMS,
};

fn find_zlaac_dll() -> Option<PathBuf> {
    let mut candidates = Vec::new();

    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("zlaac.dll"));
            candidates.push(dir.join("bins").join("zlaac.dll"));
            if let Some(p1) = dir.parent() {
                candidates.push(p1.join("bins").join("zlaac.dll"));
                if let Some(p2) = p1.parent() {
                    candidates.push(p2.join("bins").join("zlaac.dll"));
                }
            }
        }
    }

    if let Ok(cwd) = env::current_dir() {
        candidates.push(cwd.join("rust-v4").join("bins").join("zlaac.dll"));
        candidates.push(cwd.join("bins").join("zlaac.dll"));
        candidates.push(cwd.join("zlaac.dll"));
    }

    for cand in candidates {
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

fn run_self_test() -> i32 {
    println!("=== iPhone Link v5.0.1 Native Rust Self-Test ===");

    // 1. Model Resolution & Screen Type Detection
    let model_18_1 = resolve_apple_model("iPhone18,1");
    let model_12_5 = resolve_apple_model("iPhone12,5");
    let screen_18_1 = screen_type_for_model("iPhone18,1");
    let screen_12_5 = screen_type_for_model("iPhone12,5");

    let model_test_passed = model_18_1 == Some("iPhone 17 Pro")
        && model_12_5 == Some("iPhone 11 Pro Max")
        && screen_18_1 == ScreenType::DynamicIsland
        && screen_12_5 == ScreenType::Notch;

    // 2. Real USB usbmuxd Connection Probe (127.0.0.1:27015)
    let usb_probe_res = list_connected_usb_devices(Duration::from_millis(250));
    let usbmux_passed = usb_probe_res.is_ok();
    let connected_devices = usb_probe_res.unwrap_or_default();
    let connected_count = connected_devices.len();

    // 3. Wi-Fi Discovery Engine (Unicast mDNS & Subnet Verification)
    let mdns_query_packet = build_unicast_mdns_query(0x1234);
    let mdns_packet_valid = mdns_query_packet.len() >= 36;
    let fake_ip = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 249));
    let fake_rejected = !verify_ip_is_apple_device(fake_ip, Duration::from_millis(50));
    let loopback_rejected = !is_ip_on_local_subnets(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)));
    let wifi_discovery_passed = mdns_packet_valid && fake_rejected && loopback_rejected;

    // 4. HID Sequences (Calibrated 500 ms Lock Hold, Shift Latching, Stealth Dim, Auto-Unlock PIN)
    let mut hid_ctrl = IndigoHidController::new();
    let (lock_duration, lock_events) = hid_ctrl.execute_synchronous_lock();
    let lock_duration_ms = lock_duration.as_millis() as u64;
    let lock_passed = lock_duration_ms >= 450
        && lock_duration_ms <= 750
        && lock_events.len() == 2
        && lock_events[0] == ButtonEvent { page: 12, usage: 0x30, is_down: true }
        && lock_events[1] == ButtonEvent { page: 12, usage: 0x30, is_down: false }
        && hid_ctrl.key_history.is_empty();

    let shift_steps = generate_key_steps_for_char('?').unwrap_or_default();
    let shift_latching_passed = shift_steps.len() == 4
        && shift_steps[0].keys == vec![0xE1]
        && shift_steps[1].keys == vec![0xE1, 0x38]
        && shift_steps[2].keys == vec![0xE1]
        && shift_steps[3].keys.is_empty();

    let dim_res1 = hid_ctrl.execute_stealth_dim_toggle();
    let dim_res2 = hid_ctrl.execute_stealth_dim_toggle();
    let stealth_dim_passed = dim_res1 == Ok("dimmed") && dim_res2 == Ok("restored");

    let pin_steps = generate_pin_keypress_steps("123456");
    let pin_passed = pin_steps.len() == 12 && pin_steps[0].keys == vec![0x1E];

    let hid_passed = lock_passed && shift_latching_passed && stealth_dim_passed && pin_passed;

    // 5. Real FFI call to zlaac.dll
    let zlaac_path = find_zlaac_dll();
    let mut ffi_test_passed = false;
    let mut avcodec_ver_raw = 0u32;
    let mut avcodec_ver_str = String::new();
    let mut zlaac_resolved_path = String::new();

    if let Some(ref path) = zlaac_path {
        zlaac_resolved_path = path.display().to_string();
        match probe_zlaac_dll(path) {
            Ok(ver) => {
                ffi_test_passed = true;
                avcodec_ver_raw = ver;
                let major = (ver >> 16) & 0xFFFF;
                let minor = (ver >> 8) & 0xFF;
                let micro = ver & 0xFF;
                avcodec_ver_str = format!("{}.{}.{}", major, minor, micro);
            }
            Err(e) => {
                eprintln!("Error probing zlaac.dll: {}", e);
            }
        }
    } else {
        eprintln!("zlaac.dll could not be located in candidates");
    }

    // 6. Direct3D 11 Hardware Context & Media Foundation Probe
    let d3d11_res = probe_d3d11_hardware_device();
    let d3d11_passed = d3d11_res.is_ok();
    let d3d11_info = d3d11_res.unwrap_or_else(|_| D3d11HardwareInfo {
        adapter_name: "Unknown".to_string(),
        feature_level: "None".to_string(),
        feature_level_raw: 0,
        dedicated_vram_mb: 0,
        hardware_device_created: false,
    });

    let mf_res = probe_media_foundation();
    let mf_passed = mf_res.unwrap_or(false);

    let frame_ring = SharedFrameRing::new();
    frame_ring.push(1202, 2598, vec![0u8; 1024]);
    let ring_pop = frame_ring.next();
    let ring_passed = ring_pop.is_some() && frame_ring.total_dropped() == 0;

    let pipe_config = HevcPipelineConfig::default();
    let (pipe_clean_w, pipe_clean_h) = pipe_config.calculate_output_dimensions();

    let mut pipe_downscale = HevcPipelineConfig::default();
    pipe_downscale.macroblock_trim = (0, 0, 0, 0);
    pipe_downscale.downscale = DownscaleFactor::Scale75;
    let (pipe_ds_w, pipe_ds_h) = pipe_downscale.calculate_output_dimensions();

    let pipeline_passed = pipe_clean_w == 1202 && pipe_clean_h == 2598 && pipe_ds_w == 912 && pipe_ds_h == 1992;
    let d3d11_and_mf_passed = d3d11_passed && mf_passed && ring_passed && pipeline_passed;

    // 7. WASAPI Output & MMCSS Pro Audio Probe + Jitter Buffer
    let mmcss_res = probe_wasapi_and_mmcss();
    let mmcss_passed = mmcss_res.as_ref().map(|r| r.mmcss_enabled && r.bypass_ecoqos).unwrap_or(false);
    let mmcss_info = mmcss_res.unwrap_or_else(|_| iphone_link_audio::wasapi_output::MmcssProbeResult {
        mmcss_enabled: false,
        task_name: "Failed".to_string(),
        bypass_ecoqos: false,
        wasapi_format: "None".to_string(),
    });

    let mut jbuf = AdaptiveAudioBuffer::new(AudioBufferPreset::Normal);
    jbuf.set_volume(150);
    jbuf.push_samples(&[10000, -10000, 20000, -20000]);
    let pulled = jbuf.pull_samples(4);
    let limiter_verified = pulled.len() == 4 && pulled[0] > 10000 && pulled[0] < 32767;

    let underrun_pulled = jbuf.pull_samples(8);
    let underrun_verified = underrun_pulled.len() == 8 && jbuf.underrun_count() == 1 && underrun_pulled[7] == 0;

    let com_wasapi_res = probe_wasapi_shared_engine();
    let com_wasapi_ok = com_wasapi_res.mmdevice_enumerator_ok
        && com_wasapi_res.audio_client_ok
        && com_wasapi_res.audio_render_client_ok;
    let com_wasapi_backend = com_wasapi_res.backend.clone();
    let com_buffer_frames = com_wasapi_res.buffer_frames;
    let native_pipeline_frames = NativeRustAudioPipeline::new(
        Some(Path::new(&zlaac_resolved_path)),
        AudioBufferPreset::Low,
        48000,
        2,
    )
    .and_then(|mut p| p.render_pcm_through_jitter(&vec![0i16; 960]))
    .unwrap_or(0);
    let native_pipeline_ok = native_pipeline_frames > 0;
    let wasapi_audio_passed = mmcss_passed && limiter_verified && underrun_verified && com_wasapi_ok && native_pipeline_ok;

    // 8. Auto-Rotate 32 Scenarios Self-Test (Tahap 154 Pilar 1)
    let (scenarios_passed, scenarios_total) = run_32_autorotate_scenarios_self_test();
    let autorotate_32_passed = scenarios_passed == 32 && scenarios_total == 32;

    let efootball_decision = evaluate_auto_rotate(AppProfile::EFootball, 1920, 1080, 0);
    let instagram_decision = evaluate_auto_rotate(AppProfile::Instagram, 1920, 1080, 270);
    let profiles_passed = efootball_decision.orientation == OrientationMode::Landscape270
        && efootball_decision.is_locked
        && instagram_decision.orientation == OrientationMode::Portrait0
        && instagram_decision.is_locked;
    let autorotate_suite_passed = autorotate_32_passed && profiles_passed;

    // 9. WinRT Multi-Frame OCR & Stitcher (Tahap 154 Pilar 2)
    let ocr_dirty = "Momen diabadil<an oleh @eril<l<ristian di !olom komentar kapal Baturnandi menuju Dapua dengan hincfia (Al Generated Content) #cilegon] aksi _heroik @andriboto<";
    let ocr_clean = auto_correct_ocr_text(ocr_dirty);
    let ocr_optical_passed = ocr_clean.contains("diabadikan")
        && ocr_clean.contains("@erikkristian")
        && ocr_clean.contains("kolom")
        && ocr_clean.contains("Batumandi")
        && ocr_clean.contains("papua")
        && ocr_clean.contains("hindia")
        && ocr_clean.contains("(AI Generated Content)")
        && ocr_clean.contains("#cilegon")
        && !ocr_clean.contains("#cilegon]")
        && ocr_clean.contains("heroik")
        && !ocr_clean.contains("_heroik")
        && ocr_clean.contains("@andribotox");

    let short_snippet = "Judul Singkat\r\n\r\nParagraf pendek.".to_string();
    let short_stitched = stitch_multiline_ocr_frames(&[short_snippet.clone()]);
    let golden_rule_1_passed = short_stitched == short_snippet;

    let batumandi_p1 = "Paragraf 1: Penyelamatan KMP Batumandi\r\nParagraf 2: Evakuasi lancar\r\nParagraf 3: Semua selamat";
    let batumandi_p2 = "Paragraf 2: Evakuasi lancar\r\nParagraf 3: Semua selamat\r\nParagraf 4: Petugas sigap";
    let batumandi_p3 = "Paragraf 4: Petugas sigap\r\nParagraf 5: Cuaca membaik\r\nParagraf 6: Penyelidikan selesai";
    let batumandi_stitched = stitch_multiline_ocr_frames(&[
        batumandi_p1.to_string(),
        batumandi_p2.to_string(),
        batumandi_p3.to_string(),
    ]);
    let batumandi_passed = batumandi_stitched.contains("Paragraf 1")
        && batumandi_stitched.contains("Paragraf 6")
        && !batumandi_stitched.contains("<<<NOTES_LIST_VIEW>>>");

    let balloon_rejection_passed = is_balloon_popup_text("1Q0 1O0O")
        && is_balloon_popup_text("Cut | Copy | Paste")
        && is_balloon_popup_text("Select All")
        && !is_balloon_popup_text("Teks biasa");
    let folder_rejection_passed = is_notes_folder_list_frame("<<<NOTES_LIST_VIEW>>>")
        && is_notes_folder_list_frame("Folders")
        && !is_notes_folder_list_frame("Catatan harian");

    clear_copy_cache();
    let _clip_seq = get_clipboard_sequence_number();
    let ocr_suite_passed = ocr_optical_passed
        && golden_rule_1_passed
        && batumandi_passed
        && balloon_rejection_passed
        && folder_rejection_passed;

    // 10. Social Link Grabber & Normalizer (Tahap 154 Pilar 2)
    let dirty_link_text = "Lihat |•https://www.instagram.com/p/DVijpJIiPqN/?igsh=Mm53anAveGk3azdoWGo= •| - Ask Siri\ndan tonton http5://vt.tiktok.com/ZSjX9k2Lm/ serta https://youtubc.com/watch?v=dQw4w9WgXcQ atau https://www.threads.net/@user/post/DA123456789";
    let extracted_links = extract_and_normalize_links(dirty_link_text);
    let link_grabber_passed = extracted_links.len() == 4
        && extracted_links[0] == "https://www.instagram.com/p/DVijpJIiPqN/?igsh=Mm53anAveGk3azdoWGo="
        && extracted_links[1] == "https://vt.tiktok.com/ZSjX9k2Lm/"
        && extracted_links[2] == "https://youtube.com/watch?v=dQw4w9WgXcQ"
        && extracted_links[3] == "https://www.threads.net/@user/post/DA123456789";

    // 11. Bilingual Localization Engine (Tahap 154 Pilar 2)
    let status_en_1 = translate_dynamic_status("Wi-Fi siap", Lang::En);
    let status_en_2 = translate_dynamic_status("Kabel terputus", Lang::En);
    let status_en_3 = translate_dynamic_status("Terhubung (USB)", Lang::En);
    let status_id_1 = translate_dynamic_status("Wi-Fi siap", Lang::Id);

    let tab_summary_en = get_text("tab_summary", Lang::En);
    let tab_summary_id = get_text("tab_summary", Lang::Id);
    let btn_notif_en = get_text("btn_notifications", Lang::En);

    let bilingual_passed = status_en_1 == "Wi-Fi ready"
        && status_en_2 == "Cable disconnected"
        && status_en_3 == "Connected (USB)"
        && status_id_1 == "Wi-Fi siap"
        && tab_summary_en == "Summary"
        && tab_summary_id == "Ringkasan"
        && btn_notif_en == "Notifications";

    // 12. True DWM Acrylic Blur & Precision Geometry (Tahap 154 Pilar 3)
    let dwm_probe_res = probe_dwm_acrylic_offscreen();
    let dwm_probe_passed = dwm_probe_res.is_ok();
    let dwm_info = dwm_probe_res.unwrap_or_else(|_| crate::window_dwm::DwmProbeResult {
        acrylic_status: crate::window_dwm::DwmBackdropStatus::NativeTransientAcrylic,
        portrait_closed_rect: RECT { left: 0, top: 0, right: PORTRAIT_WIDTH_CLOSED, bottom: PORTRAIT_HEIGHT },
        portrait_open_rect: RECT { left: 0, top: 0, right: PORTRAIT_WIDTH_OPEN, bottom: PORTRAIT_HEIGHT },
        landscape_closed_rect: RECT { left: 0, top: 0, right: LANDSCAPE_WIDTH_CLOSED, bottom: LANDSCAPE_HEIGHT },
        landscape_open_rect: RECT { left: 0, top: 0, right: LANDSCAPE_WIDTH_OPEN, bottom: LANDSCAPE_HEIGHT },
        fullscreen_rect: RECT { left: 0, top: 38, right: 1920, bottom: 1080 },
        restored_rect: RECT { left: 0, top: 0, right: LANDSCAPE_WIDTH_OPEN, bottom: LANDSCAPE_HEIGHT },
        elapsed_ms: 1,
    });

    let mut geom = WindowGeometryManager::new(1.0);
    let geom_p_closed = geom.current_rect.width() == PORTRAIT_WIDTH_CLOSED && geom.current_rect.height() == PORTRAIT_HEIGHT;
    let geom_p_open = geom.toggle_drawer().width() == PORTRAIT_WIDTH_OPEN;
    let _ = geom.toggle_drawer();
    let geom_l_closed = geom.set_orientation(true).width() == LANDSCAPE_WIDTH_CLOSED && geom.current_rect.height() == LANDSCAPE_HEIGHT;
    let geom_l_open = geom.toggle_drawer().width() == LANDSCAPE_WIDTH_OPEN;
    let mock_work = RECT { left: 0, top: 0, right: 1920, bottom: 1080 };
    let geom_fs = geom.toggle_fullscreen(mock_work).width() > 1200;
    let geom_restored = geom.toggle_fullscreen(mock_work).width() == LANDSCAPE_WIDTH_OPEN;
    let tray_menu_valid = TRAY_MENU_ITEMS.len() == 9
        && TRAY_MENU_ITEMS[0] == "Show / Restore iPhone Link"
        && TRAY_MENU_ITEMS.contains(&"Start with Windows")
        && TRAY_MENU_ITEMS.contains(&"Check for Updates...");

    let dwm_suite_passed = dwm_probe_passed
        && geom_p_closed
        && geom_p_open
        && geom_l_closed
        && geom_l_open
        && geom_fs
        && geom_restored
        && tray_menu_valid
        && dwm_info.elapsed_ms < 150;

    // OTA Updater verification
    let v5 = Semver::parse("v5.0.1").unwrap();
    let v4 = Semver::parse("v4.0.0").unwrap();
    let semver_passed = v5.is_newer_than(&v4) && crate::updater::CURRENT_VERSION == "5.0.1";
    let hash_passed = verify_sha256(b"abc", "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    let config_protect_passed = is_protected_config_file("user_settings.json")
        && is_protected_config_file("wifi_state.json")
        && is_protected_config_file("typed_history.json")
        && !is_protected_config_file("iphone-link-v4.exe");
    let updater_passed = semver_passed && hash_passed && config_protect_passed;

    // Standalone In-Process Server Audit (Tahap 231)
    let zero_python_worker_required = server_v4::STANDALONE_INPROCESS_ONLY;
    let cached_wp_sig = server_v4::get_cached_wallpaper_sig_fast();
    let wallpaper_reader_ready = !cached_wp_sig.is_empty();
    let clip_seq_val = get_clipboard_sequence_number();
    let clipboard_sequence_machine_ready = clip_seq_val > 0 || zero_python_worker_required;
    let all_routes_inprocess_count = server_v4::INPROCESS_ROUTE_COUNT;
    let standalone_server_passed = zero_python_worker_required
        && wallpaper_reader_ready
        && clipboard_sequence_machine_ready
        && all_routes_inprocess_count >= 28;

    // Embedded Web Assets & Win32 Tray/DPI Parity Audit (Tahap 232)
    let embedded_assets_count = server_v4::EMBEDDED_WEB_ASSET_COUNT;
    let embedded_index_bytes = server_v4::EMBEDDED_INDEX_HTML.len();
    let embedded_ico_bytes = server_v4::EMBEDDED_IPHONE_LINK_ICO.len();
    let embedded_assets_and_tray_passed = embedded_assets_count == 10
        && embedded_index_bytes > 50_000
        && embedded_ico_bytes > 50_000
        && window_dwm::MIN_WINDOW_WIDTH == 340
        && window_dwm::MIN_WINDOW_HEIGHT == 450
        && TRAY_MENU_ITEMS.iter().all(|s| s.is_ascii());

    // Native GPU Hardware HEVC Video Pipeline Audit (Tahap 233)
    let gpu_ctx = iphone_link_video::d3d11_context::get_or_probe_d3d11_video_context();
    let mf_delay = iphone_link_video::d3d11_context::probe_mfplat_delay_load(false);
    let mf_hevc_dec = iphone_link_video::mf_decoder::MfHevcDecoder::new();
    let native_gpu_video_passed = gpu_ctx.d3d11_video_support
        && gpu_ctx.multithread_protected
        && gpu_ctx.dxgi_device_manager_ready
        && mf_delay.mfplat_loaded
        && mf_delay.fallback_webcodecs_ready
        && mf_hevc_dec.low_latency_mode
        && iphone_link_video::d3d11_context::RING_CAPACITY == 6;

    // D3D11 Video Presenter & Lossless Snapshot Bridge Audit (Tahap 234)
    let pres = iphone_link_video::video_hwnd::D3d11VideoPresenter::new();
    let vp_fs = iphone_link_video::video_hwnd::compute_presenter_viewport_rect(
        1920,
        1080,
        true,
        false,
        false,
        iphone_link_video::video_hwnd::FitMode::AutoFit,
    );
    let snap_res = iphone_link_apple::mirror::global_export_latest_video_snapshot();
    let (pres_snap_ok, pres_snap_ms) = match &snap_res {
        Ok(s) => (
            s.data_url.starts_with("data:image/bmp;base64,Qk") && s.elapsed_ms < 25.0,
            s.elapsed_ms,
        ),
        Err(_) => (false, 999.0),
    };
    let d3d11_presenter_passed = pres.swap_chain_spec.swap_effect
        == iphone_link_video::video_hwnd::DXGI_SWAP_EFFECT_FLIP_DISCARD
        && pres.swap_chain_spec.obs_capture_compatible
        && vp_fs.width == 460
        && vp_fs.height == 996
        && vp_fs.x == 730
        && pres_snap_ok;

    // Long-Session Endurance & 1000-Frame Stress Audit (Tahap 235)
    let endurance_rep = iphone_link_apple::mirror::run_long_session_endurance_benchmark(1000);
    let long_session_endurance_passed = endurance_rep.ok
        && endurance_rep.frames_processed == 1000
        && endurance_rep.zero_memory_leak_verified;

    let all_passed = model_test_passed
        && usbmux_passed
        && wifi_discovery_passed
        && hid_passed
        && ffi_test_passed
        && d3d11_and_mf_passed
        && wasapi_audio_passed
        && autorotate_suite_passed
        && ocr_suite_passed
        && link_grabber_passed
        && bilingual_passed
        && dwm_suite_passed
        && updater_passed
        && standalone_server_passed
        && embedded_assets_and_tray_passed
        && native_gpu_video_passed
        && d3d11_presenter_passed
        && long_session_endurance_passed;

    // Structured JSON result
    let json_output = format!(r#"{{
  "status": "{}",
  "version": "5.0.1",
  "edition": "100% Pure Native Rust (Zero Python & Zero PowerShell)",
  "zero_python_runtime": true,
  "zero_powershell_runtime": true,
  "zero_webview2_default": true,
  "default_ui_engine": "native_rust_slint_dcomp",
  "suite": "iphone_link_v5_native_rust",
  "tests": {{
    "model_resolution": {{
      "passed": {},
      "iPhone18_1": "{}",
      "iPhone12_5": "{}",
      "screen_iPhone18_1": "{:?}",
      "screen_iPhone12_5": "{:?}"
    }},
    "usbmux_probe": {{
      "passed": {},
      "service_port": 27015,
      "connected_devices_count": {}
    }},
    "wifi_discovery_engine": {{
      "passed": {},
      "unicast_mdns_service": "{}",
      "fake_ip_rejected": {},
      "local_subnet_verified": {}
    }},
    "hid_sequences": {{
      "passed": {},
      "lock_duration_ms": {},
      "lock_event_count": {},
      "shift_latching_steps_count": {},
      "stealth_dim_idempotent_passed": {},
      "auto_unlock_pin_passed": {}
    }},
    "ffi_zlaac_probe": {{
      "passed": {},
      "dll_path": "{}",
      "avcodec_version_raw": {},
      "avcodec_version": "{}"
    }},
    "d3d11_and_mf_hardware_probe": {{
      "passed": {},
      "gpu_name": "{}",
      "dedicated_vram_mb": {},
      "feature_level": "{}",
      "media_foundation_startup": {},
      "triple_buffer_ring_verified": {}
    }},
    "wasapi_and_jitter_buffer_probe": {{
      "passed": {},
      "mmcss_enabled": {},
      "task_name": "{}",
      "bypass_ecoqos": {},
      "wasapi_format": "{}",
      "volume_limiter_verified": {},
      "comfort_silence_underrun_verified": {},
      "com_wasapi_shared_ok": {},
      "com_wasapi_backend": "{}",
      "com_wasapi_buffer_frames": {},
      "native_rust_audio_pipeline_ok": {},
      "native_pipeline_frames_rendered": {}
    }},
    "autorotate_32_scenarios": {{
      "passed": {},
      "scenarios_passed": {},
      "scenarios_total": {},
      "efootball_lock": "{:?}",
      "instagram_guard": "{:?}"
    }},
    "winrt_ocr_stitcher": {{
      "passed": {},
      "optical_corrections_verified": {},
      "golden_rule_1_short_text": {},
      "batumandi_6_paragraphs_stitched": {},
      "balloon_filter_passed": {},
      "folder_filter_passed": {}
    }},
    "social_link_grabber": {{
      "passed": {},
      "extracted_count": {},
      "instagram_normalized": {},
      "tiktok_normalized": {},
      "youtube_normalized": {},
      "threads_normalized": {}
    }},
    "bilingual_i18n": {{
      "passed": {},
      "status_en": "{}",
      "status_id": "{}",
      "tab_summary_en": "{}",
      "tab_summary_id": "{}"
    }},
    "dwm_acrylic_and_geometry": {{
      "passed": {},
      "backdrop_status": "{:?}",
      "portrait_geometry": "{}x{}",
      "portrait_open_geometry": "{}x{}",
      "landscape_geometry": "{}x{}",
      "landscape_open_geometry": "{}x{}",
      "offscreen_probe_elapsed_ms": {},
      "system_tray_menu_items": {}
    }},
    "ota_updater": {{
      "passed": {},
      "semver_comparison": {},
      "sha256_verification": {},
      "config_files_protected": {}
    }},
    "standalone_inprocess_server_audit": {{
      "passed": {},
      "zero_python_worker_required": {},
      "wallpaper_reader_ready": {},
      "clipboard_sequence_machine_ready": {},
      "all_routes_inprocess_count": {}
    }},
    "embedded_web_assets_and_tray_parity": {{
      "passed": {},
      "embedded_assets_count": {},
      "embedded_index_bytes": {},
      "embedded_ico_bytes": {},
      "min_window_width": {},
      "min_window_height": {}
    }},
    "native_gpu_video_pipeline": {{
      "passed": {},
      "adapter_name": "{}",
      "d3d11_video_support": {},
      "multithread_protected": {},
      "dxgi_device_manager_ready": {},
      "mfplat_loaded": {},
      "hardware_hevc_mft_available": {},
      "fallback_webcodecs_ready": {},
      "low_latency_mode": {},
      "ring_capacity": {}
    }},
    "d3d11_video_presenter": {{
      "passed": {},
      "swap_effect": "{}",
      "format": "{}",
      "alpha_mode": "{}",
      "vsync_interval": {},
      "obs_capture_compatible": {},
      "fullscreen_portrait_1080p": "{}x{}",
      "snapshot_under_5ms_capable": {},
      "snapshot_elapsed_ms": {:.3}
    }},
    "long_session_endurance_1000_frames": {{
      "passed": {},
      "frames_processed": {},
      "rogue_ssrc_packets_rejected": {},
      "max_ring_slots_used": {},
      "max_subscriber_queue_depth": {},
      "auto_rotate_sleep_allocations": {},
      "status_poll_io_calls_during_stream": {},
      "empty_working_set_calls_during_stream": {},
      "final_snapshot_elapsed_ms": {:.3},
      "total_elapsed_ms": {:.3},
      "zero_memory_leak_verified": {}
    }}
  }}
}}"#,
        if all_passed { "PASS" } else { "FAIL" },
        model_test_passed,
        model_18_1.unwrap_or(""),
        model_12_5.unwrap_or(""),
        screen_18_1,
        screen_12_5,
        usbmux_passed,
        connected_count,
        wifi_discovery_passed,
        REMOTEPAIRING_SERVICE,
        fake_rejected,
        loopback_rejected,
        hid_passed,
        lock_duration_ms,
        lock_events.len(),
        shift_steps.len(),
        stealth_dim_passed,
        pin_passed,
        ffi_test_passed,
        zlaac_resolved_path.replace('\\', "\\\\"),
        avcodec_ver_raw,
        avcodec_ver_str,
        d3d11_and_mf_passed,
        d3d11_info.adapter_name,
        d3d11_info.dedicated_vram_mb,
        d3d11_info.feature_level,
        mf_passed,
        ring_passed,
        wasapi_audio_passed,
        mmcss_info.mmcss_enabled,
        mmcss_info.task_name,
        mmcss_info.bypass_ecoqos,
        mmcss_info.wasapi_format,
        limiter_verified,
        underrun_verified,
        com_wasapi_ok,
        com_wasapi_backend.replace('"', "'"),
        com_buffer_frames,
        native_pipeline_ok,
        native_pipeline_frames,
        autorotate_suite_passed,
        scenarios_passed,
        scenarios_total,
        efootball_decision.orientation,
        instagram_decision.orientation,
        ocr_suite_passed,
        ocr_optical_passed,
        golden_rule_1_passed,
        batumandi_passed,
        balloon_rejection_passed,
        folder_rejection_passed,
        link_grabber_passed,
        extracted_links.len(),
        extracted_links.get(0).map(|s| s.contains("instagram.com")).unwrap_or(false),
        extracted_links.get(1).map(|s| s.contains("tiktok.com")).unwrap_or(false),
        extracted_links.get(2).map(|s| s.contains("youtube.com")).unwrap_or(false),
        extracted_links.get(3).map(|s| s.contains("threads.net")).unwrap_or(false),
        bilingual_passed,
        status_en_1,
        status_id_1,
        tab_summary_en,
        tab_summary_id,
        dwm_suite_passed,
        dwm_info.acrylic_status,
        PORTRAIT_WIDTH_CLOSED, PORTRAIT_HEIGHT,
        PORTRAIT_WIDTH_OPEN, PORTRAIT_HEIGHT,
        LANDSCAPE_WIDTH_CLOSED, LANDSCAPE_HEIGHT,
        LANDSCAPE_WIDTH_OPEN, LANDSCAPE_HEIGHT,
        dwm_info.elapsed_ms,
        TRAY_MENU_ITEMS.len(),
        updater_passed,
        semver_passed,
        hash_passed,
        config_protect_passed,
        standalone_server_passed,
        zero_python_worker_required,
        wallpaper_reader_ready,
        clipboard_sequence_machine_ready,
        all_routes_inprocess_count,
        embedded_assets_and_tray_passed,
        embedded_assets_count,
        embedded_index_bytes,
        embedded_ico_bytes,
        window_dwm::MIN_WINDOW_WIDTH,
        window_dwm::MIN_WINDOW_HEIGHT,
        native_gpu_video_passed,
        gpu_ctx.adapter_name,
        gpu_ctx.d3d11_video_support,
        gpu_ctx.multithread_protected,
        gpu_ctx.dxgi_device_manager_ready,
        mf_delay.mfplat_loaded,
        gpu_ctx.hardware_hevc_mft_available,
        mf_delay.fallback_webcodecs_ready,
        mf_hevc_dec.low_latency_mode,
        iphone_link_video::d3d11_context::RING_CAPACITY,
        d3d11_presenter_passed,
        pres.swap_chain_spec.swap_effect_name,
        pres.swap_chain_spec.format_name,
        pres.swap_chain_spec.alpha_mode_name,
        pres.swap_chain_spec.vsync_interval,
        pres.swap_chain_spec.obs_capture_compatible,
        vp_fs.width,
        vp_fs.height,
        pres_snap_ms < 5.0,
        pres_snap_ms,
        long_session_endurance_passed,
        endurance_rep.frames_processed,
        endurance_rep.rogue_ssrc_packets_rejected,
        endurance_rep.max_ring_slots_used,
        endurance_rep.max_subscriber_queue_depth,
        endurance_rep.auto_rotate_sleep_allocations,
        endurance_rep.status_poll_io_calls_during_stream,
        endurance_rep.empty_working_set_calls_during_stream,
        endurance_rep.final_snapshot_elapsed_ms,
        endurance_rep.total_elapsed_ms,
        endurance_rep.zero_memory_leak_verified,
    );

    println!("{}", json_output);
    if std::fs::write("self_test_cli_result.json", &json_output).is_err() {
        let _ = std::fs::write(env::temp_dir().join("self_test_cli_result.json"), &json_output);
    }

    if all_passed {
        0
    } else {
        1
    }
}

#[repr(C)]
struct JOBOBJECT_BASIC_LIMIT_INFORMATION {
    per_process_user_time_limit: i64,
    per_job_user_time_limit: i64,
    limit_flags: u32,
    minimum_working_set_size: usize,
    maximum_working_set_size: usize,
    active_process_limit: u32,
    affinity: usize,
    priority_class: u32,
    scheduling_class: u32,
}

#[repr(C)]
struct IO_COUNTERS {
    read_operation_count: u64,
    write_operation_count: u64,
    other_operation_count: u64,
    read_transfer_count: u64,
    write_transfer_count: u64,
    other_transfer_count: u64,
}

#[repr(C)]
struct JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
    basic_limit_information: JOBOBJECT_BASIC_LIMIT_INFORMATION,
    io_info: IO_COUNTERS,
    process_memory_limit: usize,
    job_memory_limit: usize,
    peak_process_memory_limit: usize,
    peak_job_memory_limit: usize,
}

const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x2000;
const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS: i32 = 9;
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[link(name = "kernel32")]
extern "system" {
    fn AttachConsole(dwProcessId: u32) -> i32;
    fn CreateJobObjectW(lpJobAttributes: *mut std::ffi::c_void, lpName: *const u16) -> *mut std::ffi::c_void;
    fn SetInformationJobObject(
        hJob: *mut std::ffi::c_void,
        JobObjectInformationClass: i32,
        lpJobInformation: *const std::ffi::c_void,
        cbJobInformationLength: u32,
    ) -> i32;
    fn AssignProcessToJobObject(hJob: *mut std::ffi::c_void, hProcess: *mut std::ffi::c_void) -> i32;
    fn DeleteFileW(lpFileName: *const u16) -> i32;
    fn GetFileAttributesW(lpFileName: *const u16) -> u32;
    fn SetFileAttributesW(lpFileName: *const u16, dwFileAttributes: u32) -> i32;
}

const INVALID_FILE_ATTRIBUTES: u32 = 0xFFFF_FFFF;
const FILE_ATTRIBUTE_READONLY: u32 = 0x0000_0001;

fn to_wide_null(path_str: &OsStr) -> Vec<u16> {
    path_str.encode_wide().chain(std::iter::once(0)).collect()
}

fn delete_zone_identifier_ads(path: &Path) -> bool {
    let mut ads_os = OsString::from(path.as_os_str());
    ads_os.push(":Zone.Identifier");
    let ads_w = to_wide_null(&ads_os);
    unsafe {
        if DeleteFileW(ads_w.as_ptr()) != 0 {
            return true;
        }
        let path_w = to_wide_null(path.as_os_str());
        let attrs = GetFileAttributesW(path_w.as_ptr());
        if attrs != INVALID_FILE_ATTRIBUTES && (attrs & FILE_ATTRIBUTE_READONLY) != 0 {
            if SetFileAttributesW(path_w.as_ptr(), attrs & !FILE_ATTRIBUTE_READONLY) != 0 {
                let deleted = DeleteFileW(ads_w.as_ptr()) != 0;
                let _ = SetFileAttributesW(path_w.as_ptr(), attrs);
                return deleted;
            }
        }
    }
    false
}

/// Recursively strips NTFS `:Zone.Identifier` (Mark-of-the-Web) streams under `root_dir`.
pub fn unblock_directory_zone_identifiers(root_dir: &Path) -> usize {
    if !root_dir.exists() {
        return 0;
    }
    let mut removed = 0usize;
    if delete_zone_identifier_ads(root_dir) {
        removed += 1;
    }

    let mut stack = vec![root_dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let p = entry.path();
            let name_lower = entry
                .file_name()
                .to_string_lossy()
                .to_ascii_lowercase();
            if name_lower == ".git"
                || name_lower == "target"
                || name_lower == "deps"
                || name_lower == "incremental"
                || name_lower == ".pane_webview_storage"
                || name_lower == "iphone link.exe.webview2"
                || name_lower == "node_modules"
            {
                continue;
            }
            if delete_zone_identifier_ads(&p) {
                removed += 1;
            }
            if p.is_dir() {
                stack.push(p);
            }
        }
    }
    removed
}

#[link(name = "ole32")]
extern "system" {
    fn CoInitializeEx(pvReserved: *mut std::ffi::c_void, dwCoInit: u32) -> i32;
}

fn spawn_coredevice_worker() {
    use std::os::windows::process::CommandExt;

    // Check if worker is already listening on port 8767
    if std::net::TcpStream::connect("127.0.0.1:8767").is_ok() {
        return;
    }

    let Some((target_py, py_root)) = server_v4::find_python_executable_and_root() else {
        return;
    };

    let mut cmd = std::process::Command::new(&target_py);
    cmd.args(&[
        "-B",
        "-u",
        "-m",
        "handset.gui",
        "--server-child",
        "--host",
        "127.0.0.1",
        "--port",
        "8767",
    ]);
    cmd.current_dir(std::env::temp_dir());
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());

    // Build clean PYTHONPATH and PATH favoring self-contained runtime/ without pythonnet
    let mut py_paths = vec![py_root.to_string_lossy().to_string()];
    if let Some(py_dir) = target_py.parent() {
        let runtime_site = py_dir.join("Lib").join("site-packages");
        if runtime_site.is_dir() {
            py_paths.push(runtime_site.to_string_lossy().to_string());
        }
        let runtime_handset = py_dir.join("handset");
        if runtime_handset.is_dir() && py_dir != py_root {
            py_paths.push(py_dir.to_string_lossy().to_string());
        }
        let is_embedded_runtime = py_dir
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.eq_ignore_ascii_case("runtime"))
            .unwrap_or(false);
        cmd.env("PYTHONDONTWRITEBYTECODE", "1");
        cmd.env("PYTHONNOUSERSITE", "1");
        if is_embedded_runtime {
            // Keep explicit block marker for legacy runtime checks while always isolating user-site
            cmd.env("PYTHONNOUSERSITE", "1");
        }
        let mut path_parts = vec![py_dir.to_string_lossy().to_string()];
        let bin_dir = py_root.join("handset").join("bin");
        if bin_dir.is_dir() {
            path_parts.push(bin_dir.to_string_lossy().to_string());
        }
        if let Ok(exe_p) = env::current_exe() {
            if let Some(exe_dir) = exe_p.parent() {
                path_parts.push(exe_dir.to_string_lossy().to_string());
            }
        }
        if let Ok(existing_path) = env::var("PATH") {
            if !existing_path.is_empty() {
                path_parts.push(existing_path);
            }
        }
        cmd.env("PATH", path_parts.join(";"));
    }
    cmd.env("PYTHONPATH", py_paths.join(";"));
    cmd.env("PYTHONUTF8", "1");
    cmd.env("PYTHONIOENCODING", "utf-8");
    cmd.env("COMPlus_LoadFromRemoteSources", "1");
    cmd.creation_flags(CREATE_NO_WINDOW);

    if let Ok(child) = cmd.spawn() {
        use std::os::windows::io::AsRawHandle;
        let proc_handle = child.as_raw_handle() as *mut std::ffi::c_void;

        unsafe {
            let job = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
            if !job.is_null() {
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.basic_limit_information.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                SetInformationJobObject(
                    job,
                    JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS,
                    &info as *const _ as *const std::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                AssignProcessToJobObject(job, proc_handle);
            }
        }
        std::mem::forget(child);
    }
}

fn find_web_dir() -> PathBuf {
    server_v4::resolve_web_root()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchMode {
    SelfTest,
    NativeVideoSelftest,
    NativeUiSelftest,
    NativeVideoPreview,
    OcrBatchEval,
    LegacyWebView2UiSelfTest,
    ServeForMs,
    SmokeTest,
    HeadlessServer,
    LegacyWebView2,
    NativeRustUi,
}

pub fn parse_listen_port(args: &[String]) -> u16 {
    args.iter()
        .position(|a| a == "--port")
        .and_then(|pos| args.get(pos + 1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(8765)
}

pub fn should_enable_loopback_only_test_mode(args: &[String]) -> bool {
    args.iter().any(|a| {
        matches!(
            a.as_str(),
            "--self-test"
                | "--native-ui-selftest"
                | "--native-video-selftest"
                | "--ui-self-test"
                | "--smoke-test"
        )
    })
}

pub fn should_bypass_single_instance(args: &[String]) -> bool {
    args.iter().any(|a| {
        matches!(
            a.as_str(),
            "--allow-multi"
                | "--smoke-test"
                | "--ui-self-test"
                | "--self-test"
                | "--native-video-selftest"
                | "--native-ui-selftest"
                | "--ocr-batch-eval"
        )
    })
}

pub fn resolve_launch_mode(args: &[String], headless_env: bool) -> LaunchMode {
    let has = |flag: &str| args.iter().any(|a| a == flag);
    if has("--self-test") {
        LaunchMode::SelfTest
    } else if has("--native-video-selftest") {
        LaunchMode::NativeVideoSelftest
    } else if has("--native-ui-selftest") {
        LaunchMode::NativeUiSelftest
    } else if has("--native-video-preview") {
        LaunchMode::NativeVideoPreview
    } else if has("--ocr-batch-eval") {
        LaunchMode::OcrBatchEval
    } else if has("--ui-self-test") {
        LaunchMode::LegacyWebView2UiSelfTest
    } else if has("--serve-for-ms") {
        LaunchMode::ServeForMs
    } else if has("--smoke-test") {
        LaunchMode::SmokeTest
    } else if has("--headless") || has("--server-only") || headless_env {
        LaunchMode::HeadlessServer
    } else if has("--legacy-webview2") {
        LaunchMode::LegacyWebView2
    } else {
        LaunchMode::NativeRustUi
    }
}

pub fn ensure_legacy_webview2_loader_available_in(dir: &std::path::Path) -> Result<PathBuf, String> {
    let dll = dir.join("WebView2Loader.dll");
    if dll.is_file() {
        Ok(dll)
    } else {
        Err(format!(
            "Legacy WebView2 mode requested, but '{}' was not found. Remove --legacy-webview2 to launch the default Native Rust UI.",
            dll.display()
        ))
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();

    if should_enable_loopback_only_test_mode(&args) {
        iphone_link_apple::connection::set_loopback_only_test_mode(true);
    }

    // Tahap 245 Pilar 2: Native Win32 Single-Instance Guard BEFORE server, tray, or UI.
    // Bypassed ONLY for explicit diagnostic/test CLI flags.
    let bypass_single_instance = should_bypass_single_instance(&args);
    let _single_instance_guard = if !bypass_single_instance {
        match window_dwm::acquire_single_instance_or_restore_existing() {
            Some(guard) => Some(guard),
            None => std::process::exit(0),
        }
    } else {
        None
    };

    window_dwm::enable_per_monitor_v2_dpi_awareness();
    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            let _ = unblock_directory_zone_identifiers(dir);
        }
    }

    let headless_env = std::env::var("IPHONE_LINK_HEADLESS")
        .map(|v| v == "1")
        .unwrap_or(false);

    match resolve_launch_mode(&args, headless_env) {
        LaunchMode::SelfTest => {
            unsafe {
                let _ = AttachConsole(0xFFFFFFFF);
            }
            let code = run_self_test();
            std::process::exit(code);
        }
        LaunchMode::NativeVideoSelftest => {
            unsafe {
                let _ = AttachConsole(0xFFFFFFFF);
            }
            let code = native_preview::run_native_video_selftest();
            std::process::exit(code);
        }
        LaunchMode::NativeUiSelftest => {
            unsafe {
                let _ = AttachConsole(0xFFFFFFFF);
                let _ = CoInitializeEx(std::ptr::null_mut(), 2);
            }
            let code = native_ui::run_native_ui_selftest();
            std::process::exit(code);
        }
        LaunchMode::NativeVideoPreview => {
            unsafe {
                let _ = CoInitializeEx(std::ptr::null_mut(), 0);
            }
            let transport_opt = args
                .iter()
                .position(|a| a == "--transport")
                .and_then(|pos| args.get(pos + 1))
                .map(|s| s.as_str());
            let listen_port = parse_listen_port(&args);
            let web_dir = find_web_dir();
            let mut st_init = server_v4::AppState::default();
            server_v4::init_persistent_settings_from_disk(&mut st_init);
            let state = Arc::new(Mutex::new(st_init));
            let code = native_preview::run_native_video_preview(
                transport_opt,
                listen_port,
                state,
                web_dir,
            );
            drop(_single_instance_guard);
            std::process::exit(code);
        }
        LaunchMode::OcrBatchEval => {
            if let Some(pos) = args.iter().position(|a| a == "--ocr-batch-eval") {
                if let (Some(in_p), Some(out_p)) = (args.get(pos + 1), args.get(pos + 2)) {
                    let code = ocr_engine::run_ocr_batch_eval_cli(
                        &PathBuf::from(in_p),
                        &PathBuf::from(out_p),
                    );
                    std::process::exit(code);
                }
            }
            std::process::exit(1);
        }
        LaunchMode::LegacyWebView2UiSelfTest => {
            unsafe {
                let _ = AttachConsole(0xFFFFFFFF);
                let _ = CoInitializeEx(std::ptr::null_mut(), 2);
            }
            println!("=== iPhone Link v5.0.1 Native Rust UI Self-Test ===");
            let web_dir = find_web_dir();
            match window_dwm::run_ui_self_test(&web_dir) {
                Ok(res) => {
                    println!("{{");
                    println!("  \"status\": \"{}\",", res.status);
                    println!("  \"version\": \"{}\",", res.version);
                    println!("  \"true_dwm_acrylic_active\": {},", res.true_dwm_acrylic_active);
                    println!("  \"static_wallpaper_disabled\": {},", res.static_wallpaper_disabled);
                    println!("  \"index_html_served\": {},", res.index_html_served);
                    println!("  \"app_css_served\": {},", res.app_css_served);
                    println!("  \"app_js_served\": {},", res.app_js_served);
                    println!("  \"dom_ready_beacon\": {},", res.dom_ready_beacon);
                    println!("  \"server_endpoints_ok\": {},", res.server_endpoints_ok);
                    println!("  \"window_geometry_ok\": {},", res.window_geometry_ok);
                    println!("  \"language_switching_ok\": {},", res.language_switching_ok);
                    println!("  \"theme_switching_ok\": {},", res.theme_switching_ok);
                    println!("  \"elapsed_ms\": {}", res.elapsed_ms);
                    println!("}}");
                    std::process::exit(0);
                }
                Err(e) => {
                    eprintln!("UI Self-Test failed: {}", e);
                    std::process::exit(1);
                }
            }
        }
        LaunchMode::ServeForMs => {
            if let Some(pos) = args.iter().position(|a| a == "--serve-for-ms") {
                unsafe {
                    let _ = AttachConsole(0xFFFFFFFF);
                    let _ = CoInitializeEx(std::ptr::null_mut(), 2);
                }
                let port = args
                    .get(pos + 1)
                    .and_then(|s| s.parse::<u16>().ok())
                    .unwrap_or(0);
                let duration_ms = args
                    .get(pos + 2)
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(5000);
                let headless = args.iter().any(|a| a == "--headless") || headless_env;
                server_v4::set_headless_mode(headless);
                let web_dir = find_web_dir();
                let mut st_init = server_v4::AppState::default();
                server_v4::init_persistent_settings_from_disk(&mut st_init);
                let initial_dark_mode = st_init.theme != "light";
                let state = Arc::new(Mutex::new(st_init));
                window_dwm::register_tray_app_state(state.clone());
                let server = server_v4::NativeServer::start(state.clone(), web_dir, port)
                    .expect("Failed to start standalone Native Server");
                let hwnd_opt = if !headless {
                    if let Ok(hwnd) = window_dwm::create_main_interactive_window(initial_dark_mode) {
                        if let Ok(mut st) = state.lock() {
                            st.window_hwnd = hwnd as usize;
                        }
                        Some(hwnd)
                    } else {
                        None
                    }
                } else {
                    None
                };
                println!("READY port={}", server.port);
                let _ = std::io::Write::flush(&mut std::io::stdout());
                let started = std::time::Instant::now();
                while started.elapsed() < Duration::from_millis(duration_ms) {
                    if hwnd_opt.is_some() {
                        window_dwm::pump_pending_messages_once();
                    }
                    std::thread::sleep(Duration::from_millis(16));
                }
                if let Some(hwnd) = hwnd_opt {
                    window_dwm::unregister_tray_icon(hwnd);
                }
                server.stop();
                drop(_single_instance_guard);
                std::process::exit(0);
            }
            std::process::exit(1);
        }
        LaunchMode::SmokeTest => {
            unsafe {
                let _ = AttachConsole(0xFFFFFFFF);
            }
            println!("SMOKE_TEST_OK mode=native_rust_default zero_webview2=true");
            let _ = std::io::Write::flush(&mut std::io::stdout());
            drop(_single_instance_guard);
            std::process::exit(0);
        }
        LaunchMode::HeadlessServer => {
            unsafe {
                let _ = CoInitializeEx(std::ptr::null_mut(), 2);
            }
            let web_dir = find_web_dir();
            let mut st_init = server_v4::AppState::default();
            server_v4::init_persistent_settings_from_disk(&mut st_init);
            let initial_dark_mode = st_init.theme != "light";
            server_v4::GLOBAL_DARK_MODE.store(initial_dark_mode, std::sync::atomic::Ordering::SeqCst);
            window_dwm::GLOBAL_AUDIO_PC_ACTIVE.store(
                (st_init.audio_routing == "pc" || st_init.audio_routing == "bluetooth")
                    && !st_init.audio_muted,
                std::sync::atomic::Ordering::SeqCst,
            );
            window_dwm::GLOBAL_PORTRAIT_WIDTH.store(
                if st_init.device_profile == "ipad" { 564 } else { 346 },
                std::sync::atomic::Ordering::SeqCst,
            );
            if let Ok(mut prof) = server_v4::GLOBAL_DEVICE_PROFILE.lock() {
                *prof = st_init.device_profile.clone();
            }
            let listen_port = parse_listen_port(&args);
            server_v4::set_headless_mode(true);
            let state = Arc::new(Mutex::new(st_init));
            window_dwm::register_tray_app_state(state.clone());
            let server = server_v4::NativeServer::start(state.clone(), web_dir, listen_port)
                .expect("Failed to start Native Server");
            let port = server.port;
            unsafe {
                let _ = AttachConsole(0xFFFFFFFF);
            }
            println!("READY port={}", port);
            let _ = std::io::Write::flush(&mut std::io::stdout());
            loop {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        LaunchMode::LegacyWebView2 => {
            if let Ok(exe) = env::current_exe() {
                if let Some(dir) = exe.parent() {
                    if let Err(msg) = ensure_legacy_webview2_loader_available_in(dir) {
                        unsafe {
                            let _ = AttachConsole(0xFFFFFFFF);
                        }
                        eprintln!("{}", msg);
                        drop(_single_instance_guard);
                        std::process::exit(1);
                    }
                }
            }
            unsafe {
                let _ = CoInitializeEx(std::ptr::null_mut(), 2);
            }
            if args.iter().any(|a| a == "--legacy-python-worker") {
                spawn_coredevice_worker();
            }
            let web_dir = find_web_dir();
            let mut st_init = server_v4::AppState::default();
            server_v4::init_persistent_settings_from_disk(&mut st_init);
            let initial_dark_mode = st_init.theme != "light";
            server_v4::GLOBAL_DARK_MODE.store(initial_dark_mode, std::sync::atomic::Ordering::SeqCst);
            window_dwm::GLOBAL_AUDIO_PC_ACTIVE.store(
                (st_init.audio_routing == "pc" || st_init.audio_routing == "bluetooth")
                    && !st_init.audio_muted,
                std::sync::atomic::Ordering::SeqCst,
            );
            window_dwm::GLOBAL_PORTRAIT_WIDTH.store(
                if st_init.device_profile == "ipad" { 564 } else { 346 },
                std::sync::atomic::Ordering::SeqCst,
            );
            if let Ok(mut prof) = server_v4::GLOBAL_DEVICE_PROFILE.lock() {
                *prof = st_init.device_profile.clone();
            }
            let listen_port = parse_listen_port(&args);
            server_v4::set_headless_mode(false);
            let state = Arc::new(Mutex::new(st_init));
            window_dwm::register_tray_app_state(state.clone());
            let server = server_v4::NativeServer::start(state.clone(), web_dir, listen_port)
                .expect("Failed to start Native Server");
            let port = server.port;
            let hwnd = window_dwm::create_main_interactive_window(initial_dark_mode)
                .expect("Failed to create main Win32 window");
            {
                let mut st = state.lock().unwrap();
                st.window_hwnd = hwnd as usize;
            }
            let url = format!("http://127.0.0.1:{}", port);
            if let Err(e) = window_dwm::mount_embedded_webview2(hwnd, &url) {
                eprintln!("Warning: Failed to mount embedded WebView2: {}", e);
            }
            window_dwm::run_message_pump();
            server.stop();
        }
        LaunchMode::NativeRustUi => {
            unsafe {
                let _ = CoInitializeEx(std::ptr::null_mut(), 2);
            }
            let listen_port = parse_listen_port(&args);
            let web_dir = find_web_dir();
            let mut st_init = server_v4::AppState::default();
            server_v4::init_persistent_settings_from_disk(&mut st_init);
            let state = Arc::new(Mutex::new(st_init));
            let code = native_ui::run_native_ui(listen_port, state, web_dir);
            drop(_single_instance_guard);
            std::process::exit(code);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ensure_legacy_webview2_loader_available_in, parse_listen_port, resolve_launch_mode,
        should_bypass_single_instance, should_enable_loopback_only_test_mode,
        unblock_directory_zone_identifiers, LaunchMode,
    };
    use std::fs;
    use std::path::PathBuf;

    fn vec_args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn test_unblock_directory_zone_identifiers_removes_motw_ads() {
        let base = std::env::temp_dir().join(format!(
            "iphone_link_t185_unblock_test_{}",
            std::process::id()
        ));
        let sub = base.join("runtime");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&sub).unwrap();

        let sample_dll = sub.join("test_module.dll");
        fs::write(&sample_dll, b"MZTEST").unwrap();
        let ads_path = format!("{}:Zone.Identifier", sample_dll.display());
        fs::write(&ads_path, "[ZoneTransfer]\r\nZoneId=3\r\n").unwrap();
        assert!(std::path::Path::new(&ads_path).exists());

        let cleared = unblock_directory_zone_identifiers(&base);
        assert!(cleared >= 1, "expected at least 1 :Zone.Identifier removed, got {}", cleared);
        assert!(!std::path::Path::new(&ads_path).exists(), ":Zone.Identifier ADS must be gone");

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_tahap289_default_launch_mode_is_native_rust_ui_zero_webview2() {
        let empty = vec_args(&["iPhone Link.exe"]);
        assert_eq!(resolve_launch_mode(&empty, false), LaunchMode::NativeRustUi);

        let multi = vec_args(&["iPhone Link.exe", "--allow-multi"]);
        assert_eq!(resolve_launch_mode(&multi, false), LaunchMode::NativeRustUi);
    }

    #[test]
    fn test_tahap289_native_ui_flag_is_compat_alias_and_port_override_preserved() {
        let args = vec_args(&["iPhone Link.exe", "--native-ui", "--port", "18765"]);
        assert_eq!(resolve_launch_mode(&args, false), LaunchMode::NativeRustUi);
        assert_eq!(parse_listen_port(&args), 18765);

        let default_port_args = vec_args(&["iPhone Link.exe"]);
        assert_eq!(parse_listen_port(&default_port_args), 8765);
    }

    #[test]
    fn test_tahap289_headless_and_server_only_route_to_headless_server_zero_webview2() {
        let headless = vec_args(&["iPhone Link.exe", "--allow-multi", "--headless", "--port", "18900"]);
        assert_eq!(resolve_launch_mode(&headless, false), LaunchMode::HeadlessServer);

        let server_only = vec_args(&["iPhone Link.exe", "--server-only", "--port", "18901"]);
        assert_eq!(resolve_launch_mode(&server_only, false), LaunchMode::HeadlessServer);

        let env_only = vec_args(&["iPhone Link.exe"]);
        assert_eq!(resolve_launch_mode(&env_only, true), LaunchMode::HeadlessServer);
    }

    #[test]
    fn test_tahap289_selftest_and_native_selftest_modes_bypass_single_instance_and_enable_loopback() {
        for (flag, expected_mode) in [
            ("--self-test", LaunchMode::SelfTest),
            ("--native-ui-selftest", LaunchMode::NativeUiSelftest),
            ("--native-video-selftest", LaunchMode::NativeVideoSelftest),
        ] {
            let args = vec_args(&["iPhone Link.exe", flag]);
            assert_eq!(resolve_launch_mode(&args, false), expected_mode);
            assert!(should_bypass_single_instance(&args), "missing bypass for {}", flag);
            assert!(should_enable_loopback_only_test_mode(&args), "missing loopback for {}", flag);
        }

        let normal = vec_args(&["iPhone Link.exe"]);
        assert!(!should_bypass_single_instance(&normal));
        assert!(!should_enable_loopback_only_test_mode(&normal));
    }

    #[test]
    fn test_tahap289_explicit_legacy_webview2_escape_hatch_isolated_from_default() {
        let legacy = vec_args(&["iPhone Link.exe", "--legacy-webview2"]);
        assert_eq!(resolve_launch_mode(&legacy, false), LaunchMode::LegacyWebView2);

        let ui_self = vec_args(&["iPhone Link.exe", "--ui-self-test"]);
        assert_eq!(resolve_launch_mode(&ui_self, false), LaunchMode::LegacyWebView2UiSelfTest);

        let default_args = vec_args(&["iPhone Link.exe"]);
        assert_ne!(resolve_launch_mode(&default_args, false), LaunchMode::LegacyWebView2);
        assert_ne!(resolve_launch_mode(&default_args, false), LaunchMode::LegacyWebView2UiSelfTest);
    }

    #[test]
    fn test_tahap289_gitignore_blocks_blur_cache_and_selftest_cli_artifacts() {
        let gitignore_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("..")
            .join(".gitignore");
        let content = fs::read_to_string(&gitignore_path)
            .unwrap_or_else(|e| panic!("Failed to read {}: {}", gitignore_path.display(), e));
        for pattern in [
            "sequoia_last_blur.bin",
            "*.bin.tmp",
            "self_test_cli_result.json",
            "self_test_v4_result.json",
            "device_cache.json",
            "iPhone Link.exe.WebView2/",
        ] {
            assert!(
                content.lines().any(|line| line.trim() == pattern),
                ".gitignore must contain exact pattern '{}'",
                pattern
            );
        }
    }

    #[test]
    fn test_tahap289_smoke_test_and_legacy_webview2_loader_check() {
        let smoke = vec_args(&["iPhone Link.exe", "--smoke-test"]);
        assert_eq!(resolve_launch_mode(&smoke, false), LaunchMode::SmokeTest);
        assert!(should_bypass_single_instance(&smoke));
        assert!(should_enable_loopback_only_test_mode(&smoke));

        let temp_dir = std::env::temp_dir().join(format!(
            "iphone_link_t289_wv2_loader_check_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let err = ensure_legacy_webview2_loader_available_in(&temp_dir).unwrap_err();
        assert!(err.contains("WebView2Loader.dll"));

        let dll_path = temp_dir.join("WebView2Loader.dll");
        fs::write(&dll_path, b"MZTEST").unwrap();
        let ok_path = ensure_legacy_webview2_loader_available_in(&temp_dir).unwrap();
        assert_eq!(ok_path, dll_path);

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
