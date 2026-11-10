//! iPhone Link v4.0 Native Rust Desktop Engine Entry Point.

use std::env;
use std::path::PathBuf;

use iphone_link_apple::models::{resolve_apple_model, screen_type_for_model, ScreenType};
use iphone_link_audio::probe_zlaac_dll;
use iphone_link_video::autorotate::{evaluate_auto_rotate, AppProfile, OrientationMode};
use iphone_link_video::decoder::H265Trim;

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
    println!("=== iPhone Link v4.0 Native Rust Self-Test ===");

    // 1. Model Resolution & Screen Type Detection
    let model_18_1 = resolve_apple_model("iPhone18,1");
    let model_12_5 = resolve_apple_model("iPhone12,5");
    let screen_18_1 = screen_type_for_model("iPhone18,1");
    let screen_12_5 = screen_type_for_model("iPhone12,5");

    let model_test_passed = model_18_1 == Some("iPhone 17 Pro")
        && model_12_5 == Some("iPhone 11 Pro Max")
        && screen_18_1 == ScreenType::DynamicIsland
        && screen_12_5 == ScreenType::Notch;

    // 2. Real FFI call to zlaac.dll
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

    // 3. Auto-Rotate State Machine & H.265 Macroblock Trim Evaluation
    let trim = H265Trim::default();
    let (clean_w, clean_h) = trim.trim_dimensions(1216, 2656);
    let trim_test_passed = clean_w == 1202 && clean_h == 2598;

    let efootball_decision = evaluate_auto_rotate(AppProfile::EFootball, clean_w, clean_h, 0);
    let instagram_decision = evaluate_auto_rotate(AppProfile::Instagram, clean_w, clean_h, 270);
    let autorotate_passed = efootball_decision.orientation == OrientationMode::Landscape270
        && efootball_decision.is_locked
        && instagram_decision.orientation == OrientationMode::Portrait0
        && instagram_decision.is_locked;

    let all_passed = model_test_passed && ffi_test_passed && trim_test_passed && autorotate_passed;

    // 4. Print structured JSON result
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
    println!("    \"ffi_zlaac_probe\": {{");
    println!("      \"passed\": {},", ffi_test_passed);
    println!("      \"dll_path\": \"{}\",", zlaac_resolved_path.replace('\\', "\\\\"));
    println!("      \"avcodec_version_raw\": {},", avcodec_ver_raw);
    println!("      \"avcodec_version\": \"{}\",", avcodec_ver_str);
    println!("      \"decoder_open_close\": {}", ffi_test_passed);
    println!("    }},");
    println!("    \"video_pipeline\": {{");
    println!("      \"h265_trim_passed\": {},", trim_test_passed);
    println!("      \"raw_resolution\": \"1216x2656\",");
    println!("      \"clean_resolution\": \"{}x{}\",", clean_w, clean_h);
    println!("      \"autorotate_passed\": {},", autorotate_passed);
    println!("      \"efootball_lock\": \"{:?}\",", efootball_decision.orientation);
    println!("      \"instagram_guard\": \"{:?}\"", instagram_decision.orientation);
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

    println!("iPhone Link v4.0 Native Rust Engine (Tahap 151)");
    println!("Usage: iphone-link-v4.exe [--self-test]");
}
