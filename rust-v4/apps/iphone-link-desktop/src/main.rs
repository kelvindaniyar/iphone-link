#![windows_subsystem = "windows"]
//! iPhone Link v4.0 Native Rust Desktop Engine Entry Point.

pub mod i18n;
pub mod link_grabber;
pub mod native_ui;
pub mod ocr_engine;
pub mod server_v4;
pub mod updater;
pub mod window_dwm;

use std::env;
use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;
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
use iphone_link_audio::wasapi_output::probe_wasapi_and_mmcss;
use iphone_link_audio::{probe_zlaac_dll, AudioBufferPreset};
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

    candidates.push(PathBuf::from(r"D:\ZenLink\zlaac.dll"));

    for cand in candidates {
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

fn run_self_test() -> i32 {
    println!("=== iPhone Link v4.0 Native Rust Self-Test (Tahap 154) ===");

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
    let fake_ip = IpAddr::V4(Ipv4Addr::new(192, 168, 100, 249));
    let fake_rejected = !verify_ip_is_apple_device(fake_ip, Duration::from_millis(50));
    let loopback_rejected = !is_ip_on_local_subnets(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)));
    let wifi_discovery_passed = mdns_packet_valid && fake_rejected && loopback_rejected;

    // 4. HID Sequences (Synchronous Lock 0.5s, Shift Latching, Stealth Dim, Auto-Unlock PIN)
    let mut hid_ctrl = IndigoHidController::new();
    let (lock_duration, lock_events) = hid_ctrl.execute_synchronous_lock();
    let lock_duration_ms = lock_duration.as_millis() as u64;
    let lock_passed = lock_duration_ms >= 480
        && lock_duration_ms <= 650
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
    let wasapi_audio_passed = mmcss_passed && limiter_verified && underrun_verified;

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
    let dirty_link_text = "Lihat |•https://www.instagram.com/p/DVijpJIiPqN/?igsh=Mm53anAveGk3azdoWGo= •| - Ask Siri dan tonton http5://vt.tiktok.com/ZSjX9k2Lm/ serta https://youtubc.com/watch?v=dQw4w9WgXcQ atau https://www.threads.net/@user/post/DA123456789";
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
    let tray_menu_valid = TRAY_MENU_ITEMS.len() == 5 && TRAY_MENU_ITEMS[0] == "Show iPhone Link";

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
    let v4 = Semver::parse("v4.0.0").unwrap();
    let v3 = Semver::parse("v3.0.0").unwrap();
    let semver_passed = v4.is_newer_than(&v3);
    let hash_passed = verify_sha256(b"abc", "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    let config_protect_passed = is_protected_config_file("user_settings.json")
        && is_protected_config_file("wifi_state.json")
        && is_protected_config_file("typed_history.json")
        && !is_protected_config_file("iphone-link-v4.exe");
    let updater_passed = semver_passed && hash_passed && config_protect_passed;

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
        && updater_passed;

    // Structured JSON result
    let json_output = format!(r#"{{
  "status": "{}",
  "suite": "iphone_link_v4_native_rust",
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
      "comfort_silence_underrun_verified": {}
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
    );

    println!("{}", json_output);
    let _ = std::fs::write("self_test_cli_result.json", &json_output);

    if all_passed {
        0
    } else {
        1
    }
}

#[link(name = "kernel32")]
extern "system" {
    fn AttachConsole(dwProcessId: u32) -> i32;
}

fn main() {
    window_dwm::enable_per_monitor_v2_dpi_awareness();
    let args: Vec<String> = env::args().collect();

    if args.iter().any(|a| a == "--self-test") {
        unsafe {
            let _ = AttachConsole(0xFFFFFFFF);
        }
        let code = run_self_test();
        std::process::exit(code);
    }

    if args.iter().any(|a| a == "--ui-self-test") {
        unsafe {
            let _ = AttachConsole(0xFFFFFFFF);
        }
        println!("=== iPhone Link v4.0 Native Rust UI Self-Test ===");
        let web_dir = PathBuf::from(".");
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

    // 100% Full Native Rust Win32 GDI+ UI Launch (0% WebView2, 0% Python, 1 Process)
    native_ui::init_gdi_plus();

    // In-process TCP responder for port 8767 (0 child processes, 0 python, 0 conhost)
    std::thread::spawn(|| {
        if let Ok(listener) = std::net::TcpListener::bind("127.0.0.1:8767") {
            for stream in listener.incoming() {
                if let Ok(mut s) = stream {
                    use std::io::{Read, Write};
                    let mut buf = [0u8; 1024];
                    let _ = s.read(&mut buf);
                    let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 29\r\nConnection: close\r\n\r\n{\"ok\":true,\"status\":\"ready\"}");
                    let _ = s.flush();
                }
            }
        }
    });

    let state = Arc::new(Mutex::new(server_v4::AppState::default()));
    let _server = server_v4::NativeServer::start(state.clone(), PathBuf::from("."), 8765)
        .expect("Failed to start Native Server");

    let hwnd = window_dwm::create_main_interactive_window(true)
        .expect("Failed to create main Win32 window");
    {
        let mut st = state.lock().unwrap();
        st.window_hwnd = hwnd as usize;
    }

    window_dwm::run_message_pump();
}
