//! iPhone Link v4.0 Native Rust Desktop Engine Entry Point.

use std::env;
use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;
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
use iphone_link_video::autorotate::{evaluate_auto_rotate, AppProfile, OrientationMode};
use iphone_link_video::d3d11_context::{probe_d3d11_hardware_device, D3d11HardwareInfo, SharedFrameRing};
use iphone_link_video::decoder::H265Trim;
use iphone_link_video::mf_decoder::{probe_media_foundation, DownscaleFactor, HevcPipelineConfig};

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
    println!("=== iPhone Link v4.0 Native Rust Self-Test (Tahap 153) ===");

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

    // 6. Auto-Rotate State Machine & H.265 Macroblock Trim Evaluation
    let trim = H265Trim::default();
    let (clean_w, clean_h) = trim.trim_dimensions(1216, 2656);
    let trim_test_passed = clean_w == 1202 && clean_h == 2598;

    let efootball_decision = evaluate_auto_rotate(AppProfile::EFootball, clean_w, clean_h, 0);
    let instagram_decision = evaluate_auto_rotate(AppProfile::Instagram, clean_w, clean_h, 270);
    let autorotate_passed = efootball_decision.orientation == OrientationMode::Landscape270
        && efootball_decision.is_locked
        && instagram_decision.orientation == OrientationMode::Portrait0
        && instagram_decision.is_locked;

    // 7. Direct3D 11 Hardware Context & Media Foundation Probe (Tahap 153)
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

    // 8. WASAPI Output & MMCSS Pro Audio Probe + Jitter Buffer (Tahap 153)
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

    // Underrun comfort silence verification
    let underrun_pulled = jbuf.pull_samples(8);
    let underrun_verified = underrun_pulled.len() == 8 && jbuf.underrun_count() == 1 && underrun_pulled[7] == 0;

    let wasapi_audio_passed = mmcss_passed && limiter_verified && underrun_verified;

    let all_passed = model_test_passed
        && usbmux_passed
        && wifi_discovery_passed
        && hid_passed
        && ffi_test_passed
        && trim_test_passed
        && autorotate_passed
        && d3d11_and_mf_passed
        && wasapi_audio_passed;

    // 9. Print structured JSON result
    println!("{{");
    println!("  \"status\": \"{}\",", if all_passed { "PASS" } else { "FAIL" });
    println!("  \"suite\": \"iphone_link_v4_native_rust\",");
    println!("  \"tests\": {{");
    println!("    \"model_resolution\": {{");
    println!("      \"passed\": {},", model_test_passed);
    println!("      \"iPhone18_1\": \"{}\",", model_18_1.unwrap_or(""));
    println!("      \"iPhone12_5\": \"{}\",", model_12_5.unwrap_or(""));
    println!("      \"screen_iPhone18_1\": \"{:?}\",", screen_18_1);
    println!("      \"screen_iPhone12_5\": \"{:?}\"", screen_12_5);
    println!("    }},");
    println!("    \"usbmux_probe\": {{");
    println!("      \"passed\": {},", usbmux_passed);
    println!("      \"service_port\": 27015,");
    println!("      \"connected_devices_count\": {}", connected_count);
    println!("    }},");
    println!("    \"wifi_discovery_engine\": {{");
    println!("      \"passed\": {},", wifi_discovery_passed);
    println!("      \"unicast_mdns_service\": \"{}\",", REMOTEPAIRING_SERVICE);
    println!("      \"fake_ip_rejected\": {},", fake_rejected);
    println!("      \"local_subnet_verified\": {}", loopback_rejected);
    println!("    }},");
    println!("    \"hid_sequences\": {{");
    println!("      \"passed\": {},", hid_passed);
    println!("      \"lock_duration_ms\": {},", lock_duration_ms);
    println!("      \"lock_event_count\": {},", lock_events.len());
    println!("      \"lock_keyboard_events\": 0,");
    println!("      \"shift_latching_steps_count\": {},", shift_steps.len());
    println!("      \"stealth_dim_idempotent_passed\": {},", stealth_dim_passed);
    println!("      \"auto_unlock_pin_passed\": {}", pin_passed);
    println!("    }},");
    println!("    \"ffi_zlaac_probe\": {{");
    println!("      \"passed\": {},", ffi_test_passed);
    println!("      \"dll_path\": \"{}\",", zlaac_resolved_path.replace('\\', "\\\\"));
    println!("      \"avcodec_version_raw\": {},", avcodec_ver_raw);
    println!("      \"avcodec_version\": \"{}\",", avcodec_ver_str);
    println!("      \"decoder_open_close\": {}", ffi_test_passed);
    println!("    }},");
    println!("    \"video_pipeline\": {{");
    println!("      \"passed\": {},", trim_test_passed && autorotate_passed);
    println!("      \"h265_trim_passed\": {},", trim_test_passed);
    println!("      \"raw_resolution\": \"1216x2656\",");
    println!("      \"clean_resolution\": \"{}x{}\",", clean_w, clean_h);
    println!("      \"autorotate_passed\": {},", autorotate_passed);
    println!("      \"efootball_lock\": \"{:?}\",", efootball_decision.orientation);
    println!("      \"instagram_guard\": \"{:?}\"", instagram_decision.orientation);
    println!("    }},");
    println!("    \"d3d11_and_mf_hardware_probe\": {{");
    println!("      \"passed\": {},", d3d11_and_mf_passed);
    println!("      \"gpu_name\": \"{}\",", d3d11_info.adapter_name);
    println!("      \"dedicated_vram_mb\": {},", d3d11_info.dedicated_vram_mb);
    println!("      \"feature_level\": \"{}\",", d3d11_info.feature_level);
    println!("      \"media_foundation_startup\": {},", mf_passed);
    println!("      \"triple_buffer_ring_verified\": {},", ring_passed);
    println!("      \"hevc_clean_resolution\": \"{}x{}\",", pipe_clean_w, pipe_clean_h);
    println!("      \"hevc_downscaled_resolution\": \"{}x{}\"", pipe_ds_w, pipe_ds_h);
    println!("    }},");
    println!("    \"wasapi_and_jitter_buffer_probe\": {{");
    println!("      \"passed\": {},", wasapi_audio_passed);
    println!("      \"mmcss_enabled\": {},", mmcss_info.mmcss_enabled);
    println!("      \"task_name\": \"{}\",", mmcss_info.task_name);
    println!("      \"bypass_ecoqos\": {},", mmcss_info.bypass_ecoqos);
    println!("      \"wasapi_format\": \"{}\",", mmcss_info.wasapi_format);
    println!("      \"jitter_buffer_preset\": \"Normal (60ms)\",");
    println!("      \"volume_limiter_verified\": {},", limiter_verified);
    println!("      \"comfort_silence_underrun_verified\": {}", underrun_verified);
    println!("    }}");
    println!("  }}");
    println!("}}");

    if all_passed {
        0
    } else {
        1
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.iter().any(|a| a == "--self-test") {
        let code = run_self_test();
        std::process::exit(code);
    }

    println!("iPhone Link v4.0 Native Rust Engine (Tahap 153)");
    println!("Usage: iphone-link-v4.exe [--self-test]");
}
