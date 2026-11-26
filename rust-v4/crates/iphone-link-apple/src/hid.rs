//! CoreDevice Indigo HID & UniversalHID Engine:
//! - Inverse Touch Matrix (0° / 90° / 180° / 270°, including eFootball 2027 Asymmetric Lock)
//! - System Swipes (Back, Home, Recent 180ms Hold, Directional) & 2-Finger Pinch Multitouch
//! - 4-Step Multi-Step Shift Latching, Twin-Key 12ms Release Pause, Smart Quotes & Em/En-Dash Normalization
//! - iOS Cmd Shortcuts (0xE3 + A/C/V/X/Z) & Pure Eject Onscreen Keyboard Toggle (0x0C, 0x00B8)
//! - Single-Channel Consumer 0x0070 Stealth Dim 0% (16 steps) & 4-Bar ~25% (0x006F) Restore with Idempotent Guard
//! - Synchronous 0.5s Lock Hold (0x0C, 0x0030) with Stealth Dim Auto-Restore
//! - Smart Wake + Face ID Bypass + Raw Digit PIN Injection (Zero Leakage to Shadow Typing Buffer)
//! - Win32 DPAPI (`crypt32.dll`) Passcode Encryption / Decryption
//! - Safe >=50s Idle Keep-Awake Micro-Pulse (`[0xE0]`)
//! - Live Async `HidSession` over `UniversalHidServiceClient` & `IndigoHidClient`

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use idevice::{
    core_device::{
        IndigoHidClient, KeyboardUsage, MainKeyboardService, TouchscreenContact,
        UniversalHidServiceClient, TOUCHSCREEN_STATE_CONTACT, TOUCHSCREEN_STATE_RELEASE,
    },
    rsd::RsdHandshake,
    tcp::handle::AdapterHandle,
    ReadWrite,
};
use serde_json::{Map, Number, Value};

pub use crate::physical_stream::{
    enqueue_physical_hid_command, is_physical_hid_active, physical_hid_telemetry, PhysicalHidCommand,
};

pub const HID_USAGE_PAGE_CONSUMER: u16 = 12; // 0x000C
pub const HID_USAGE_POWER_BUTTON: u16 = 0x0030;
pub const HID_USAGE_WAKE_BUTTON: u16 = 0x0040;
pub const HID_USAGE_EJECT: u16 = 0x00B8;
pub const HID_USAGE_BRIGHTNESS_DOWN: u16 = 0x0070;
pub const HID_USAGE_BRIGHTNESS_UP: u16 = 0x006F;
pub const HID_USAGE_VOLUME_UP: u16 = 0x00E9;
pub const HID_USAGE_VOLUME_DOWN: u16 = 0x00EA;
pub const HID_USAGE_MUTE: u16 = 0x00E2;

pub const KEY_LEFT_CTRL: u8 = 0xE0;
pub const KEY_SHIFT_LEFT: u8 = 0xE1;
pub const KEY_CMD_LEFT: u8 = 0xE3;
pub const KEY_ENTER: u8 = 0x28;
pub const KEY_TAB: u8 = 0x2B;
pub const KEY_SPACE: u8 = 0x2C;

pub const LOCK_HOLD_DURATION_MS: u64 = 500; // Calibrated iOS SpringBoard 500 ms press-and-hold (below 1000 ms Siri)
pub const LOCK_POST_STEALTH_SETTLE_MS: u64 = 80;
pub const LOCK_DISPATCH_DEBOUNCE_MS: u64 = 600;
pub const STEALTH_DIM_STEPS: usize = 16;
pub const STEALTH_RESTORE_DEFAULT_STEPS: usize = 4;
pub const IDLE_KEEP_AWAKE_THRESHOLD_SECS: f64 = 50.0;
pub const MAX_TYPED_TEXT_LEN: usize = 4000;
pub const MAX_PASSCODE_LEN: usize = 32;

/// Indigo consumer button descriptors matching iOS Consumer Page (0x0C).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndigoButton {
    PowerLock,
    Wake,
    Eject,
    BrightnessDown,
    BrightnessUp,
    VolumeUp,
    VolumeDown,
    Mute,
}

impl IndigoButton {
    pub fn usage(&self) -> (u16, u16) {
        match self {
            Self::PowerLock => (HID_USAGE_PAGE_CONSUMER, HID_USAGE_POWER_BUTTON),
            Self::Wake => (HID_USAGE_PAGE_CONSUMER, HID_USAGE_WAKE_BUTTON),
            Self::Eject => (HID_USAGE_PAGE_CONSUMER, HID_USAGE_EJECT),
            Self::BrightnessDown => (HID_USAGE_PAGE_CONSUMER, HID_USAGE_BRIGHTNESS_DOWN),
            Self::BrightnessUp => (HID_USAGE_PAGE_CONSUMER, HID_USAGE_BRIGHTNESS_UP),
            Self::VolumeUp => (HID_USAGE_PAGE_CONSUMER, HID_USAGE_VOLUME_UP),
            Self::VolumeDown => (HID_USAGE_PAGE_CONSUMER, HID_USAGE_VOLUME_DOWN),
            Self::Mute => (HID_USAGE_PAGE_CONSUMER, HID_USAGE_MUTE),
        }
    }

    /// Resolve a textual button name into `(IndigoButton, hold_ms)` matching Indigo HID conventions.
    pub fn from_name(name: &str) -> Option<(Self, u64)> {
        match name.trim().to_ascii_lowercase().as_str() {
            "home" | "wake" => Some((Self::Wake, 50)),
            "lock" | "power" => Some((Self::PowerLock, LOCK_HOLD_DURATION_MS)),
            "eject" => Some((Self::Eject, 50)),
            "brightness-down" => Some((Self::BrightnessDown, 25)),
            "brightness-up" => Some((Self::BrightnessUp, 25)),
            "volume-up" => Some((Self::VolumeUp, 50)),
            "volume-down" => Some((Self::VolumeDown, 50)),
            "mute" => Some((Self::Mute, 50)),
            _ => None,
        }
    }
}

/// A recorded or dispatched button event: (page, usage, is_down).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ButtonEvent {
    pub page: u16,
    pub usage: u16,
    pub is_down: bool,
}

/// A recorded or dispatched keyboard event: (usages_pressed, duration_ms).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyStep {
    pub keys: Vec<u8>,
    pub duration_ms: u64,
}

/// Touch swipe step in normalized 0..65535 coordinate space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TouchStep {
    pub contact: bool,
    pub x: u16,
    pub y: u16,
    pub sleep_ms: u64,
}

const PLAIN_CHARS: &str = "1234567890-=[]\\;',./`";
const SHIFT_CHARS: &str = "!@#$%^&*()_+{}|:\"<>?~";
const PLAIN_CODES: [u8; 21] = [
    0x1E, 0x1F, 0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x2D, 0x2E, 0x2F, 0x30, 0x31, 0x33,
    0x34, 0x36, 0x37, 0x38, 0x35,
];

/// Map a character to (HID usage code, needs_shift) on a US QWERTY layout.
pub fn char_usage(ch: char) -> Option<(u8, bool)> {
    match ch {
        'a'..='z' => Some((0x04 + (ch as u8 - b'a'), false)),
        'A'..='Z' => Some((0x04 + (ch as u8 - b'A'), true)),
        ' ' => Some((KEY_SPACE, false)),
        '\n' | '\r' => Some((KEY_ENTER, false)),
        '\t' => Some((KEY_TAB, false)),
        _ => {
            if let Some(i) = PLAIN_CHARS.find(ch) {
                Some((PLAIN_CODES[i], false))
            } else if let Some(i) = SHIFT_CHARS.find(ch) {
                Some((PLAIN_CODES[i], true))
            } else {
                None
            }
        }
    }
}

/// Map normalized PC touch coordinates `(nx, ny)` in `0.0..=1.0` to iOS physical digitizer `(0..=65535)`
/// accounting for display rotation (`0°`, `90°`, `180°`, `270°` — matching `mf_decoder::inverse_touch_transform`).
pub fn map_normalized_touch_to_ios(nx: f64, ny: f64, rotation_deg: u32) -> (u16, u16) {
    let x = nx.clamp(0.0, 1.0);
    let y = ny.clamp(0.0, 1.0);
    let (ix, iy) = match rotation_deg % 360 {
        90 => (y, 1.0 - x),
        180 => (1.0 - x, 1.0 - y),
        270 => (1.0 - y, x),
        _ => (x, y),
    };
    let to_u16 = |v: f64| -> u16 { (v * 65535.0).round().clamp(0.0, 65535.0) as u16 };
    (to_u16(ix), to_u16(iy))
}

/// Inverse touch coordinate transform for Hybrid AirPlay H.264 streams (1:1 `zenlink_core::api::device_point`).
///
/// AirPlay automatically rotates the encoded video buffer (`1920x1080` landscape vs portrait),
/// while Apple's CoreDevice HID digitizer ALWAYS stays anchored in physical portrait space (`0..=65535`).
/// Maps normalized AirPlay viewport `(nx, ny)` in `0.0..=1.0` back into physical portrait digitizer
/// coordinates according to SpringBoard rotation (`0`, `90` = LandscapeLeft, `180`, `270` = LandscapeRight).
pub fn map_airplay_touch_to_ios_digitizer(nx: f64, ny: f64, springboard_rot_deg: u32) -> (u32, u32) {
    let (dx, dy) = map_normalized_touch_to_ios(nx, ny, springboard_rot_deg);
    (u32::from(dx), u32::from(dy))
}

/// Map 0..65535 PC viewport coordinates into iOS portrait digitizer coordinates using `rotation_deg`.
pub fn inverse_touch_coords(pc_x: u16, pc_y: u16, rotation_deg: u32) -> (u16, u16) {
    match rotation_deg % 360 {
        90 => (pc_y, 65535u16.saturating_sub(pc_x)),
        180 => (
            65535u16.saturating_sub(pc_x),
            65535u16.saturating_sub(pc_y),
        ),
        270 => (65535u16.saturating_sub(pc_y), pc_x),
        _ => (pc_x, pc_y),
    }
}

fn linear_touch_swipe(
    start_x: u16,
    start_y: u16,
    end_x: u16,
    end_y: u16,
    steps_count: usize,
    step_ms: u64,
    hold_end_ms: u64,
) -> Vec<TouchStep> {
    let count = steps_count.max(2);
    let mut steps = Vec::with_capacity(count + 3);
    steps.push(TouchStep {
        contact: true,
        x: start_x,
        y: start_y,
        sleep_ms: 15,
    });
    for i in 1..=count {
        let t = (i as f64) / (count as f64);
        let cur_x = ((start_x as f64) + ((end_x as f64) - (start_x as f64)) * t).round() as u16;
        let cur_y = ((start_y as f64) + ((end_y as f64) - (start_y as f64)) * t).round() as u16;
        let sleep_ms = if i == count && hold_end_ms > 0 {
            hold_end_ms
        } else {
            step_ms
        };
        steps.push(TouchStep {
            contact: true,
            x: cur_x,
            y: cur_y,
            sleep_ms,
        });
    }
    steps.push(TouchStep {
        contact: false,
        x: end_x,
        y: end_y,
        sleep_ms: 15,
    });
    steps
}

/// Generate left-edge iOS Back swipe (`x: 2200 -> 38000`, `y: 32768`).
pub fn swipe_back_steps(steps_count: usize) -> Vec<TouchStep> {
    linear_touch_swipe(2200, 32768, 38000, 32768, steps_count, 12, 0)
}

/// Generate bottom-edge iOS Home flick (`y: 65535 -> 18000`, `x: 32768`):
/// 5 fast 9 ms steps (`45 ms` total, `0 ms` pause) so iOS ALWAYS goes Home and never triggers Recent.
pub fn swipe_home_steps() -> Vec<TouchStep> {
    linear_touch_swipe(32768, 65535, 32768, 18000, 5, 9, 0)
}

/// Generate bottom-edge App Switcher swipe (`y: 64500 -> 34000`) with `180 ms` hold before release.
pub fn swipe_recent_app_switcher_steps() -> Vec<TouchStep> {
    linear_touch_swipe(32768, 64500, 32768, 34000, 10, 14, 180)
}

/// Builds exact `y = 0` top-edge swipe steps for iOS Notification Center (`x = 16000`) and
/// Control Center (`x = 58000`) in 8 smooth 10 ms segments (`0 -> 46000`), rotated via
/// `inverse_touch_coords` so Landscape pulls from the physical top edge too.
pub fn build_system_top_edge_swipe_steps(kind: &str, rotation_deg: u32) -> Option<Vec<TouchStep>> {
    let x0 = match kind.trim().to_ascii_lowercase().as_str() {
        "notifications" | "notif" => 16000u16,
        "control" | "control-center" | "control_center" => 58000u16,
        _ => return None,
    };
    let steps_count = 8usize;
    let y_end = 46000.0f64;
    let rot = rotation_deg % 360;
    let mut out = Vec::with_capacity(steps_count + 2);
    for i in 0..=steps_count {
        let t = (i as f64) / (steps_count as f64);
        let ry = (y_end * t).round().clamp(0.0, 65535.0) as u16;
        let (ix, iy) = inverse_touch_coords(x0, ry, rot);
        out.push(TouchStep {
            contact: true,
            x: ix,
            y: iy,
            sleep_ms: if i < steps_count { 10 } else { 0 },
        });
    }
    let (ex, ey) = inverse_touch_coords(x0, 46000, rot);
    out.push(TouchStep {
        contact: false,
        x: ex,
        y: ey,
        sleep_ms: 0,
    });
    Some(out)
}

/// Generate named system/directional swipe steps (`back`, `home`, `recent`, `notifications`, `control_center`, `left`, `right`, `up`, `down`).
pub fn swipe_direction_steps(dir: &str) -> Option<Vec<TouchStep>> {
    let norm = dir.trim().to_ascii_lowercase();
    match norm.as_str() {
        "back" => Some(swipe_back_steps(10)),
        "home" => Some(swipe_home_steps()),
        "recent" | "switcher" | "app-switcher" => Some(swipe_recent_app_switcher_steps()),
        "notifications" | "notif" | "control" | "control-center" | "control_center" => {
            build_system_top_edge_swipe_steps(&norm, get_active_rotation_deg())
        }
        "left" => Some(linear_touch_swipe(49000, 32768, 16000, 32768, 8, 16, 0)),
        "right" => Some(linear_touch_swipe(16000, 32768, 49000, 32768, 8, 16, 0)),
        "up" => Some(linear_touch_swipe(32768, 49000, 32768, 16000, 8, 16, 0)),
        "down" => Some(linear_touch_swipe(32768, 16000, 32768, 49000, 8, 16, 0)),
        _ => None,
    }
}

/// Generate symmetric 2-finger pinch zoom multitouch contact frames centered at `(center_x, center_y)`.
pub fn pinch_zoom_frames(
    center_x: u16,
    center_y: u16,
    start_span: u16,
    end_span: u16,
    steps: usize,
) -> Vec<[TouchscreenContact; 2]> {
    let count = steps.max(2);
    let mut frames = Vec::with_capacity(count + 2);
    for i in 0..=count {
        let t = (i as f64) / (count as f64);
        let span = ((start_span as f64) + ((end_span as f64) - (start_span as f64)) * t)
            .round()
            .max(200.0) as u16;
        let half = span / 2;
        let x0 = center_x.saturating_sub(half);
        let y0 = center_y.saturating_sub(half / 2);
        let x1 = center_x.saturating_add(half);
        let y1 = center_y.saturating_add(half / 2);
        frames.push([
            TouchscreenContact {
                identity: 0,
                touching: true,
                x: x0,
                y: y0,
            },
            TouchscreenContact {
                identity: 1,
                touching: true,
                x: x1,
                y: y1,
            },
        ]);
    }
    if let Some(last) = frames.last().copied() {
        frames.push([
            TouchscreenContact {
                identity: 0,
                touching: false,
                x: last[0].x,
                y: last[0].y,
            },
            TouchscreenContact {
                identity: 1,
                touching: false,
                x: last[1].x,
                y: last[1].y,
            },
        ]);
    }
    frames
}

/// Normalize smart quotes, em/en dashes, and CRLF line endings identically to `handset/screen.py`.
pub fn normalize_text_for_hid(text: &str) -> String {
    text.replace('\u{201C}', "\"")
        .replace('\u{201D}', "\"")
        .replace('\u{2018}', "'")
        .replace('\u{2019}', "'")
        .replace('\u{2014}', "-")
        .replace('\u{2013}', "-")
        .replace("\r\n", "\n")
        .replace('\r', "\n")
}

/// Generate Multi-Step Shift Latching sequence for a single character (Tahap 53):
/// - Shifted: `[0xE1] (14ms) -> [0xE1, key] (18ms) -> [0xE1] (8ms) -> [] (12ms)`
/// - Unshifted: `[key] (20ms) -> [] (15ms)`
pub fn generate_key_steps_for_char(ch: char) -> Option<Vec<KeyStep>> {
    let (code, needs_shift) = char_usage(ch)?;
    if needs_shift {
        Some(vec![
            KeyStep {
                keys: vec![KEY_SHIFT_LEFT],
                duration_ms: 14,
            },
            KeyStep {
                keys: vec![KEY_SHIFT_LEFT, code],
                duration_ms: 18,
            },
            KeyStep {
                keys: vec![KEY_SHIFT_LEFT],
                duration_ms: 8,
            },
            KeyStep {
                keys: vec![],
                duration_ms: 12,
            },
        ])
    } else {
        Some(vec![
            KeyStep {
                keys: vec![code],
                duration_ms: 20,
            },
            KeyStep {
                keys: vec![],
                duration_ms: 15,
            },
        ])
    }
}

/// Generate full HID typing sequence for a text block (matching `handset/screen.py::type_text_to_iphone`):
/// - Normalizes smart quotes & em/en dashes
/// - `\n` -> `[0x28] (25ms) -> [] (25ms)`
/// - 4-step Shift latching for shifted symbols/uppercase
/// - `12 ms` twin-key release pause when repeating unshifted key (e.g. `"messi"`, `"ll"`)
/// Returns `(steps, typed_char_count)`.
pub fn generate_key_steps_for_text(text: &str) -> (Vec<KeyStep>, usize) {
    if text.is_empty() {
        return (Vec::new(), 0);
    }
    let normalized = normalize_text_for_hid(text);
    let mut steps = Vec::new();
    let mut typed_count = 0usize;
    let mut prev_unshifted_code: Option<u8> = None;

    // Initial clean modifier release pause (matching send_keys_direct([]) + 50ms)
    steps.push(KeyStep {
        keys: vec![],
        duration_ms: 50,
    });

    for ch in normalized.chars().take(MAX_TYPED_TEXT_LEN) {
        if ch == '\n' {
            steps.push(KeyStep {
                keys: vec![KEY_ENTER],
                duration_ms: 25,
            });
            steps.push(KeyStep {
                keys: vec![],
                duration_ms: 25,
            });
            prev_unshifted_code = None;
            typed_count += 1;
            continue;
        }

        let resolved = char_usage(ch).or_else(|| ch.to_lowercase().next().and_then(char_usage));
        let Some((code, needs_shift)) = resolved else {
            continue;
        };

        if needs_shift {
            steps.push(KeyStep {
                keys: vec![KEY_SHIFT_LEFT],
                duration_ms: 14,
            });
            steps.push(KeyStep {
                keys: vec![KEY_SHIFT_LEFT, code],
                duration_ms: 18,
            });
            steps.push(KeyStep {
                keys: vec![KEY_SHIFT_LEFT],
                duration_ms: 8,
            });
            steps.push(KeyStep {
                keys: vec![],
                duration_ms: 12,
            });
            prev_unshifted_code = None;
        } else {
            if prev_unshifted_code == Some(code) {
                steps.push(KeyStep {
                    keys: vec![],
                    duration_ms: 12,
                });
            }
            steps.push(KeyStep {
                keys: vec![code],
                duration_ms: 10,
            });
            steps.push(KeyStep {
                keys: vec![],
                duration_ms: 10,
            });
            prev_unshifted_code = Some(code);
        }
        typed_count += 1;
    }

    (steps, typed_count)
}

/// Resolve an iOS Cmd shortcut action name into its HID usage byte (`A=0x04, C=0x06, V=0x19, X=0x1B, Z=0x1D`).
pub fn ios_shortcut_usage(action: &str) -> Option<u8> {
    match action.trim().to_ascii_lowercase().as_str() {
        "select-all" | "select_all" => Some(0x04),
        "copy" => Some(0x06),
        "paste-ios" | "paste" => Some(0x19),
        "cut" => Some(0x1B),
        "undo" => Some(0x1D),
        _ => None,
    }
}

/// Generate clean sequential Cmd + key HID reports for iOS shortcuts (`[] -> [0xE3] -> [0xE3, key] -> [0xE3] -> []`).
pub fn generate_ios_shortcut_steps(action: &str) -> Option<Vec<KeyStep>> {
    let key = ios_shortcut_usage(action)?;
    Some(vec![
        KeyStep {
            keys: vec![],
            duration_ms: 10,
        },
        KeyStep {
            keys: vec![KEY_CMD_LEFT],
            duration_ms: 30,
        },
        KeyStep {
            keys: vec![KEY_CMD_LEFT, key],
            duration_ms: 50,
        },
        KeyStep {
            keys: vec![KEY_CMD_LEFT],
            duration_ms: 20,
        },
        KeyStep {
            keys: vec![],
            duration_ms: 10,
        },
    ])
}

/// Generate pure Consumer HID Eject `(0x0C, 0x00B8)` events for iOS onscreen keyboard toggle (never `0x029D`).
pub fn generate_onscreen_keyboard_toggle_events() -> Vec<ButtonEvent> {
    let (page, usage) = IndigoButton::Eject.usage();
    vec![
        ButtonEvent {
            page,
            usage,
            is_down: true,
        },
        ButtonEvent {
            page,
            usage,
            is_down: false,
        },
    ]
}

/// Clean passcode to up to 32 typeable US-keyboard characters (excluding control chars).
pub fn clean_passcode(raw: &str) -> String {
    raw.chars()
        .filter(|c| !"\r\n\t".contains(*c) && char_usage(*c).is_some())
        .take(MAX_PASSCODE_LEN)
        .collect()
}

/// Convert passcode into raw key reports; 4- or 6-digit numeric unlocks automatically, others append Enter.
pub fn passcode_keys(raw: &str) -> Vec<Vec<u8>> {
    let code = clean_passcode(raw);
    let mut out: Vec<Vec<u8>> = code
        .chars()
        .filter_map(|c| {
            char_usage(c).map(|(u, shift)| if shift { vec![KEY_SHIFT_LEFT, u] } else { vec![u] })
        })
        .collect();
    let auto_submit_digits = code.chars().all(|c| c.is_ascii_digit()) && matches!(code.len(), 4 | 6);
    if !code.is_empty() && !auto_submit_digits {
        out.push(vec![KEY_ENTER]);
    }
    out
}

pub const WAKE_SCREEN_SETTLE_MS: u64 = 450;
pub const FACE_ID_PRE_BYPASS_WAIT_MS: u64 = 600;
pub const FACE_ID_POST_BYPASS_WAIT_MS: u64 = 550;
pub const PIN_DIGIT_PRESS_MS: u64 = 22;
pub const PIN_DIGIT_RELEASE_MS: u64 = 38;

/// Convert PIN string (digits 0..9 or unshifted chars) to PIN HID keypress steps with calibrated 22ms press and 38ms pause (60ms/digit).
pub fn generate_pin_keypress_steps(pin: &str) -> Vec<KeyStep> {
    let mut steps = Vec::new();
    for ch in pin.chars() {
        if let Some((code, false)) = char_usage(ch) {
            steps.push(KeyStep {
                keys: vec![code],
                duration_ms: PIN_DIGIT_PRESS_MS,
            });
            steps.push(KeyStep {
                keys: vec![],
                duration_ms: PIN_DIGIT_RELEASE_MS,
            });
        }
    }
    steps
}

/// Generate smooth 8-step upward swipe sequence from bottom edge (`y: 65535 -> 22000`) over ~200 ms.
pub fn generate_lockscreen_swipe_steps() -> Vec<TouchStep> {
    let mut steps = Vec::with_capacity(10);
    steps.push(TouchStep {
        contact: true,
        x: 32768,
        y: 65535,
        sleep_ms: 20,
    });
    for i in 1..=8u32 {
        let t = (i as f64) / 8.0;
        let ease = 1.0 - (1.0 - t).powi(2);
        let y = (65535.0 - (43535.0 * ease)).round().clamp(22000.0, 65535.0);
        steps.push(TouchStep {
            contact: true,
            x: 32768,
            y: y as u16,
            sleep_ms: 22,
        });
    }
    steps.push(TouchStep {
        contact: false,
        x: 32768,
        y: 22000,
        sleep_ms: 0,
    });
    steps
}

/// Complete Indigo HID Controller managing physical device actions and state guards.
#[derive(Debug)]
pub struct IndigoHidController {
    pub is_dimmed: bool,
    pub button_history: Vec<ButtonEvent>,
    pub key_history: Vec<KeyStep>,
}

impl Default for IndigoHidController {
    fn default() -> Self {
        Self {
            is_dimmed: false,
            button_history: Vec::new(),
            key_history: Vec::new(),
        }
    }
}

impl IndigoHidController {
    pub fn new() -> Self {
        Self::default()
    }

    /// Execute synchronous lock screen with 0.5s (500 ms) power hold.
    /// Standar Emas Tahap 143 V2: exactly 2 button events, 0 keyboard events!
    pub fn execute_synchronous_lock(&mut self) -> (Duration, Vec<ButtonEvent>) {
        let (page, usage) = IndigoButton::PowerLock.usage();
        let start = Instant::now();

        let ev_down = ButtonEvent {
            page,
            usage,
            is_down: true,
        };
        self.button_history.push(ev_down);

        std::thread::sleep(Duration::from_millis(LOCK_HOLD_DURATION_MS));

        let ev_up = ButtonEvent {
            page,
            usage,
            is_down: false,
        };
        self.button_history.push(ev_up);

        let elapsed = start.elapsed();
        (elapsed, vec![ev_down, ev_up])
    }

    /// Execute Mac-Style Stealth Dim:
    /// - Dimming: 16 steps of Brightness Down (reaches 0%)
    /// - Restoring: exactly 4 steps of Brightness Up (~25%)
    /// - Strict Idempotent Guard: no duplicate dims, only restores if dimmed!
    pub fn execute_stealth_dim_toggle(&mut self) -> Result<&'static str, &'static str> {
        let (page_down, usage_down) = IndigoButton::BrightnessDown.usage();
        let (page_up, usage_up) = IndigoButton::BrightnessUp.usage();

        if self.is_dimmed {
            for _ in 0..STEALTH_RESTORE_DEFAULT_STEPS {
                self.button_history.push(ButtonEvent {
                    page: page_up,
                    usage: usage_up,
                    is_down: true,
                });
                self.button_history.push(ButtonEvent {
                    page: page_up,
                    usage: usage_up,
                    is_down: false,
                });
            }
            self.is_dimmed = false;
            Ok("restored")
        } else {
            for _ in 0..STEALTH_DIM_STEPS {
                self.button_history.push(ButtonEvent {
                    page: page_down,
                    usage: usage_down,
                    is_down: true,
                });
                self.button_history.push(ButtonEvent {
                    page: page_down,
                    usage: usage_down,
                    is_down: false,
                });
            }
            self.is_dimmed = true;
            Ok("dimmed")
        }
    }

    /// Auto-Restore brightness on session exit if dimmed.
    pub fn auto_restore_on_exit(&mut self) -> bool {
        if self.is_dimmed {
            let _ = self.execute_stealth_dim_toggle();
            true
        } else {
            false
        }
    }

    /// Execute Smart Wake & Auto-Unlock PIN 6-Digit.
    pub fn execute_wake_and_unlock_pin(
        &mut self,
        pin_6digit: &str,
    ) -> (ButtonEvent, Vec<TouchStep>, Vec<KeyStep>) {
        let (page, usage) = IndigoButton::Wake.usage();
        let wake_event = ButtonEvent {
            page,
            usage,
            is_down: true,
        };
        self.button_history.push(wake_event);
        self.button_history.push(ButtonEvent {
            page,
            usage,
            is_down: false,
        });

        let swipe_steps = generate_lockscreen_swipe_steps();
        let key_steps = generate_pin_keypress_steps(pin_6digit);
        self.key_history.extend(key_steps.clone());

        (wake_event, swipe_steps, key_steps)
    }

    /// Type text with Multi-Step Shift Latching.
    pub fn type_text(&mut self, text: &str) -> Vec<KeyStep> {
        let mut steps = Vec::new();
        for ch in text.chars() {
            if let Some(s) = generate_key_steps_for_char(ch) {
                steps.extend(s);
            }
        }
        self.key_history.extend(steps.clone());
        steps
    }
}

// ============================================================================
// Process-Wide Thread-Safe HID Runtime State, Shadow Typing Buffer & DPAPI
// ============================================================================

#[derive(Debug)]
struct HidRuntimeState {
    stealth_dim_active: bool,
    device_intentionally_locked: bool,
    is_touch_dragging: bool,
    last_user_input_ts: f64,
    last_rotation_deg: u32,
    encrypted_pin_b64: Option<String>,
    recent_button_events: Vec<ButtonEvent>,
    recent_key_steps: Vec<KeyStep>,
}

impl Default for HidRuntimeState {
    fn default() -> Self {
        Self {
            stealth_dim_active: false,
            device_intentionally_locked: false,
            is_touch_dragging: false,
            last_user_input_ts: current_epoch_secs(),
            last_rotation_deg: 0,
            encrypted_pin_b64: None,
            recent_button_events: Vec::new(),
            recent_key_steps: Vec::new(),
        }
    }
}

#[derive(Debug, Default)]
struct TypedBufferState {
    current_line: String,
    lines: Vec<String>,
    last_active_line: String,
    last_timestamp: f64,
    history_path_override: Option<PathBuf>,
}

static HID_RUNTIME_STATE: OnceLock<Mutex<HidRuntimeState>> = OnceLock::new();
static TYPED_BUFFER_STATE: OnceLock<Mutex<TypedBufferState>> = OnceLock::new();
pub static HID_TEST_SERIAL_LOCK: Mutex<()> = Mutex::new(());

fn runtime_state() -> &'static Mutex<HidRuntimeState> {
    HID_RUNTIME_STATE.get_or_init(|| Mutex::new(HidRuntimeState::default()))
}

fn typed_buffer_state() -> &'static Mutex<TypedBufferState> {
    TYPED_BUFFER_STATE.get_or_init(|| Mutex::new(TypedBufferState::default()))
}

pub fn current_epoch_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

pub fn is_stealth_dim_active() -> bool {
    runtime_state()
        .lock()
        .map(|g| g.stealth_dim_active)
        .unwrap_or(false)
}

pub fn set_stealth_dim_active(active: bool) {
    if let Ok(mut g) = runtime_state().lock() {
        g.stealth_dim_active = active;
    }
}

pub fn is_device_intentionally_locked() -> bool {
    runtime_state()
        .lock()
        .map(|g| g.device_intentionally_locked)
        .unwrap_or(false)
}

pub fn set_device_intentionally_locked(locked: bool) {
    if let Ok(mut g) = runtime_state().lock() {
        g.device_intentionally_locked = locked;
    }
}

pub fn set_touch_dragging(dragging: bool) {
    if let Ok(mut g) = runtime_state().lock() {
        g.is_touch_dragging = dragging;
    }
}

pub fn set_active_rotation_deg(rotation_deg: u32) {
    if let Ok(mut g) = runtime_state().lock() {
        g.last_rotation_deg = rotation_deg % 360;
    }
}

pub fn get_active_rotation_deg() -> u32 {
    runtime_state()
        .lock()
        .map(|g| g.last_rotation_deg)
        .unwrap_or(0)
}

pub fn record_user_input_activity(is_drag: bool) {
    let now = current_epoch_secs();
    if let Ok(mut g) = runtime_state().lock() {
        g.last_user_input_ts = now;
        if is_drag {
            g.is_touch_dragging = true;
        }
    }
}

pub fn set_last_user_input_ts_for_test(ts_epoch_secs: f64) {
    if let Ok(mut g) = runtime_state().lock() {
        g.last_user_input_ts = ts_epoch_secs;
    }
}

static LAST_LOCK_DISPATCH_MS: AtomicU64 = AtomicU64::new(0);
static HID_MONO_EPOCH: OnceLock<Instant> = OnceLock::new();

fn hid_monotonic_ms() -> u64 {
    (HID_MONO_EPOCH.get_or_init(Instant::now).elapsed().as_millis() as u64).saturating_add(10_000)
}

pub fn try_acquire_lock_debounce() -> bool {
    let now = hid_monotonic_ms();
    loop {
        let prev = LAST_LOCK_DISPATCH_MS.load(Ordering::Relaxed);
        if prev != 0 && now.saturating_sub(prev) < LOCK_DISPATCH_DEBOUNCE_MS {
            return false;
        }
        if LAST_LOCK_DISPATCH_MS
            .compare_exchange_weak(prev, now, Ordering::SeqCst, Ordering::Relaxed)
            .is_ok()
        {
            return true;
        }
    }
}

pub fn reset_hid_runtime_state() {
    if let Ok(mut g) = runtime_state().lock() {
        *g = HidRuntimeState::default();
    }
    LAST_LOCK_DISPATCH_MS.store(0, Ordering::SeqCst);
    clear_typed_lines_buffer();
}

pub fn recent_button_events_snapshot() -> Vec<ButtonEvent> {
    runtime_state()
        .lock()
        .map(|g| g.recent_button_events.clone())
        .unwrap_or_default()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StealthDimResult {
    pub ok: bool,
    pub stealth: bool,
    pub level: String,
    pub skipped: bool,
    pub steps: usize,
    pub button_events: Vec<ButtonEvent>,
}

impl StealthDimResult {
    pub fn to_json_string(&self) -> String {
        let mut obj = Map::new();
        obj.insert("ok".into(), Value::Bool(self.ok));
        obj.insert("stealth".into(), Value::Bool(self.stealth));
        obj.insert("stealthDimActive".into(), Value::Bool(self.stealth));
        obj.insert("level".into(), Value::String(self.level.clone()));
        obj.insert("skipped".into(), Value::Bool(self.skipped));
        obj.insert("steps".into(), Value::Number(Number::PosInt(self.steps as u64)));
        obj.insert(
            "usagePage".into(),
            Value::Number(Number::PosInt(u64::from(HID_USAGE_PAGE_CONSUMER))),
        );
        let usage = if self.level == "min" {
            HID_USAGE_BRIGHTNESS_DOWN
        } else {
            HID_USAGE_BRIGHTNESS_UP
        };
        obj.insert("usage".into(), Value::Number(Number::PosInt(u64::from(usage))));
        serde_json::to_string(&Value::Object(obj)).unwrap_or_else(|_| "{}".to_string())
    }
}

/// Adjust physical OLED/LCD brightness of iPhone using Single-Channel Consumer Page `0x0C`:
/// - `"min"`: 16 steps of `0x0070` (`brightness-down`) -> 0% pitch dark (idempotent if already active)
/// - `"restore"`: `min(steps, 4)` steps of `0x006F` (`brightness-up`) -> ~25% 4-bar restore (strict guard if not active)
pub fn set_iphone_physical_brightness(
    level: &str,
    steps: usize,
    force: bool,
) -> StealthDimResult {
    record_user_input_activity(false);
    let Ok(mut g) = runtime_state().lock() else {
        return StealthDimResult {
            ok: false,
            stealth: false,
            level: level.to_string(),
            skipped: true,
            steps: 0,
            button_events: Vec::new(),
        };
    };

    if level == "min" {
        if g.stealth_dim_active && !force {
            return StealthDimResult {
                ok: true,
                stealth: true,
                level: "min".to_string(),
                skipped: true,
                steps: 0,
                button_events: Vec::new(),
            };
        }
        let actual_steps = if steps > 0 { steps } else { STEALTH_DIM_STEPS };
        let (page, usage) = IndigoButton::BrightnessDown.usage();
        let mut events = Vec::with_capacity(actual_steps * 2);
        for _ in 0..actual_steps {
            events.push(ButtonEvent {
                page,
                usage,
                is_down: true,
            });
            events.push(ButtonEvent {
                page,
                usage,
                is_down: false,
            });
        }
        g.stealth_dim_active = true;
        g.recent_button_events.extend(events.iter().copied());
        drop(g);
        if is_physical_hid_active() {
            let seq: Vec<(u16, u16, bool, u64)> = events
                .iter()
                .map(|ev| (ev.page, ev.usage, ev.is_down, 25))
                .collect();
            let _ = enqueue_physical_hid_command(PhysicalHidCommand::ButtonSequence { events: seq });
        }
        StealthDimResult {
            ok: true,
            stealth: true,
            level: "min".to_string(),
            skipped: false,
            steps: actual_steps,
            button_events: events,
        }
    } else {
        if !g.stealth_dim_active && !force {
            return StealthDimResult {
                ok: true,
                stealth: false,
                level: "restore".to_string(),
                skipped: true,
                steps: 0,
                button_events: Vec::new(),
            };
        }
        g.stealth_dim_active = false;
        let actual_steps = if steps > 0 {
            steps.min(STEALTH_RESTORE_DEFAULT_STEPS)
        } else {
            STEALTH_RESTORE_DEFAULT_STEPS
        };
        let (page, usage) = IndigoButton::BrightnessUp.usage();
        let mut events = Vec::with_capacity(actual_steps * 2);
        for _ in 0..actual_steps {
            events.push(ButtonEvent {
                page,
                usage,
                is_down: true,
            });
            events.push(ButtonEvent {
                page,
                usage,
                is_down: false,
            });
        }
        g.recent_button_events.extend(events.iter().copied());
        drop(g);
        if is_physical_hid_active() {
            let seq: Vec<(u16, u16, bool, u64)> = events
                .iter()
                .map(|ev| (ev.page, ev.usage, ev.is_down, 25))
                .collect();
            let _ = enqueue_physical_hid_command(PhysicalHidCommand::ButtonSequence { events: seq });
        }
        StealthDimResult {
            ok: true,
            stealth: false,
            level: "restore".to_string(),
            skipped: false,
            steps: actual_steps,
            button_events: events,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockScreenResult {
    pub ok: bool,
    pub locked: bool,
    pub dispatched: bool,
    pub stealth_restored: bool,
    pub restore_steps: usize,
    pub duration_ms: u64,
    pub button_events: Vec<ButtonEvent>,
}

impl LockScreenResult {
    pub fn to_json_string(&self) -> String {
        let mut obj = Map::new();
        obj.insert("ok".into(), Value::Bool(self.ok));
        obj.insert("locked".into(), Value::Bool(self.locked));
        obj.insert("dispatched".into(), Value::Bool(self.dispatched));
        obj.insert("stealthRestored".into(), Value::Bool(self.stealth_restored));
        obj.insert(
            "restoreSteps".into(),
            Value::Number(Number::PosInt(self.restore_steps as u64)),
        );
        obj.insert("holdMs".into(), Value::Number(Number::PosInt(self.duration_ms)));
        obj.insert(
            "eventsCount".into(),
            Value::Number(Number::PosInt(self.button_events.len() as u64)),
        );
        serde_json::to_string(&Value::Object(obj)).unwrap_or_else(|_| "{}".to_string())
    }
}

#[derive(Debug, Clone)]
pub struct LockTraceEntry {
    pub seq: u64,
    pub ts_ms: u64,
    pub channel: String,
    pub hold_ms: u64,
    pub usage_page: u16,
    pub usage_code: u16,
    pub stealth_was_active: bool,
    pub stealth_restored: bool,
    pub dispatched: bool,
    pub ok: bool,
    pub elapsed_ms: u64,
    pub protocol_steps: Vec<String>,
    pub wire_outcome: String,
    pub error: String,
}

static LOCK_TRACE_SEQ: AtomicU64 = AtomicU64::new(0);
static LOCK_TRACE_LOG: OnceLock<Mutex<VecDeque<LockTraceEntry>>> = OnceLock::new();

fn lock_trace_store() -> &'static Mutex<VecDeque<LockTraceEntry>> {
    LOCK_TRACE_LOG.get_or_init(|| Mutex::new(VecDeque::with_capacity(32)))
}

/// Clears the in-memory lock diagnostic trace ring buffer (test helper).
pub fn clear_lock_trace_for_test() {
    if let Ok(mut g) = lock_trace_store().lock() {
        g.clear();
    }
}

/// Appends a non-PII lock dispatch entry to the 32-entry trace ring buffer.
#[allow(clippy::too_many_arguments)]
pub fn record_lock_trace(
    channel: &str,
    hold_ms: u64,
    usage_page: u16,
    usage_code: u16,
    stealth_was_active: bool,
    stealth_restored: bool,
    dispatched: bool,
    ok: bool,
    elapsed_ms: u64,
    protocol_steps: Vec<String>,
    error: &str,
) {
    let seq = LOCK_TRACE_SEQ.fetch_add(1, Ordering::SeqCst) + 1;
    let entry = LockTraceEntry {
        seq,
        ts_ms: (current_epoch_secs() * 1000.0).max(0.0) as u64,
        channel: channel.to_string(),
        hold_ms,
        usage_page,
        usage_code,
        stealth_was_active,
        stealth_restored,
        dispatched,
        ok,
        elapsed_ms,
        protocol_steps,
        wire_outcome: String::new(),
        error: error.to_string(),
    };
    if let Ok(mut g) = lock_trace_store().lock() {
        while g.len() >= 32 {
            g.pop_front();
        }
        g.push_back(entry);
    }
}

/// Annotates the most recent power-lock trace entry with the actual physical worker wire result
/// (`indigo_xpc` vs `uhid_consumer`) without ever holding any lock across `.await`.
pub fn record_lock_wire_outcome(wire_channel: &str, ok: bool, error: Option<&str>) {
    if let Ok(mut g) = lock_trace_store().lock() {
        if let Some(last) = g.back_mut() {
            last.wire_outcome = if ok {
                format!("{wire_channel}:ok")
            } else {
                format!("{wire_channel}:err")
            };
            if let Some(e) = error {
                if !e.is_empty() {
                    last.error = e.to_string();
                }
            }
        }
    }
}

/// Returns strictly PII-free JSON telemetry for recent `Lock Screen` attempts (`GET /api/debug/lock-trace`).
pub fn lock_trace_json() -> String {
    let entries: Vec<LockTraceEntry> = lock_trace_store()
        .lock()
        .map(|g| g.iter().cloned().collect())
        .unwrap_or_default();
    let entry_to_val = |e: &LockTraceEntry| -> Value {
        let mut o = Map::new();
        o.insert("seq".into(), Value::Number(Number::PosInt(e.seq)));
        o.insert("tsMs".into(), Value::Number(Number::PosInt(e.ts_ms)));
        o.insert("channel".into(), Value::String(e.channel.clone()));
        o.insert("holdMs".into(), Value::Number(Number::PosInt(e.hold_ms)));
        o.insert(
            "usagePage".into(),
            Value::Number(Number::PosInt(u64::from(e.usage_page))),
        );
        o.insert(
            "usageCode".into(),
            Value::Number(Number::PosInt(u64::from(e.usage_code))),
        );
        o.insert(
            "stealthWasActive".into(),
            Value::Bool(e.stealth_was_active),
        );
        o.insert("stealthRestored".into(), Value::Bool(e.stealth_restored));
        o.insert("dispatched".into(), Value::Bool(e.dispatched));
        o.insert("ok".into(), Value::Bool(e.ok));
        o.insert("elapsedMs".into(), Value::Number(Number::PosInt(e.elapsed_ms)));
        o.insert(
            "protocolSteps".into(),
            Value::Array(
                e.protocol_steps
                    .iter()
                    .map(|s| Value::String(s.clone()))
                    .collect(),
            ),
        );
        o.insert("wireOutcome".into(), Value::String(e.wire_outcome.clone()));
        o.insert("error".into(), Value::String(e.error.clone()));
        Value::Object(o)
    };
    let mut root = Map::new();
    root.insert("ok".into(), Value::Bool(true));
    root.insert(
        "lockHoldDurationMs".into(),
        Value::Number(Number::PosInt(LOCK_HOLD_DURATION_MS)),
    );
    root.insert(
        "lockClampMinMs".into(),
        Value::Number(Number::PosInt(480)),
    );
    root.insert(
        "lockClampMaxMs".into(),
        Value::Number(Number::PosInt(520)),
    );
    root.insert(
        "count".into(),
        Value::Number(Number::PosInt(entries.len() as u64)),
    );
    root.insert(
        "latest".into(),
        entries.last().map(entry_to_val).unwrap_or(Value::Null),
    );
    root.insert(
        "history".into(),
        Value::Array(entries.iter().map(entry_to_val).collect()),
    );
    serde_json::to_string(&Value::Object(root)).unwrap_or_else(|_| "{}".to_string())
}

/// Lock physical iPhone screen via calibrated `500 ms` press-and-hold on Consumer `0x0C, 0x0030`
/// (with flush barrier, single-session `indigo_xpc`, and `600 ms` double-dispatch debounce).
/// If Stealth Dim was active, restores 4 `brightness-up` steps + `80 ms` settle first.
pub fn lock_iphone_screen_with_hold(perform_sleep_hold: bool) -> LockScreenResult {
    record_user_input_activity(false);
    let (page, usage) = IndigoButton::PowerLock.usage();
    let stealth_was_active = is_stealth_dim_active();
    if perform_sleep_hold && !try_acquire_lock_debounce() {
        record_lock_trace(
            "debounced",
            LOCK_HOLD_DURATION_MS,
            page,
            usage,
            stealth_was_active,
            false,
            false,
            true,
            0,
            vec!["debounced_within_600ms".to_string()],
            "",
        );
        return LockScreenResult {
            ok: true,
            locked: true,
            dispatched: false,
            stealth_restored: false,
            restore_steps: 0,
            duration_ms: 0,
            button_events: Vec::new(),
        };
    }
    let mut events = Vec::new();
    let mut protocol_steps: Vec<String> = Vec::new();
    let mut stealth_restored = false;
    let mut restore_steps = 0usize;

    if stealth_was_active {
        let restore = set_iphone_physical_brightness("restore", STEALTH_RESTORE_DEFAULT_STEPS, false);
        stealth_restored = !restore.skipped;
        restore_steps = restore.steps;
        events.extend(restore.button_events);
        if stealth_restored {
            protocol_steps.push(format!("stealth_restore_{restore_steps}_steps"));
        }
        if perform_sleep_hold && stealth_restored {
            std::thread::sleep(Duration::from_millis(LOCK_POST_STEALTH_SETTLE_MS));
            protocol_steps.push("post_stealth_settle_80ms".to_string());
        }
    }

    let start = Instant::now();
    let ev_down = ButtonEvent {
        page,
        usage,
        is_down: true,
    };
    let phys_active = is_physical_hid_active();
    let live_dispatched = if perform_sleep_hold && phys_active {
        let dispatched = enqueue_physical_hid_command(PhysicalHidCommand::ButtonPress {
            page,
            usage,
            hold_ms: LOCK_HOLD_DURATION_MS,
        });
        if !dispatched {
            LAST_LOCK_DISPATCH_MS.store(0, Ordering::SeqCst);
        }
        dispatched
    } else {
        if perform_sleep_hold {
            std::thread::sleep(Duration::from_millis(LOCK_HOLD_DURATION_MS));
        }
        false
    };
    let ev_up = ButtonEvent {
        page,
        usage,
        is_down: false,
    };
    let raw_elapsed_ms = if perform_sleep_hold {
        start.elapsed().as_millis() as u64
    } else {
        LOCK_HOLD_DURATION_MS
    };
    let elapsed_ms = if live_dispatched {
        LOCK_HOLD_DURATION_MS
    } else {
        raw_elapsed_ms
    };

    events.push(ev_down);
    events.push(ev_up);

    let locked_ok = !perform_sleep_hold || !phys_active || live_dispatched;
    let (channel, err_msg) = if !perform_sleep_hold {
        protocol_steps.push("symbolic_500ms_hold".to_string());
        ("symbolic", "")
    } else if phys_active && live_dispatched {
        protocol_steps.extend([
            "indigo_button_down(0x0C,0x0030)".to_string(),
            "flush_barrier".to_string(),
            "hold_500ms".to_string(),
            "indigo_button_up(0x0C,0x0030)".to_string(),
        ]);
        ("indigo_xpc_or_uhid", "")
    } else if phys_active {
        protocol_steps.push("enqueue_failed".to_string());
        ("physical_enqueue_failed", "Physical HID worker queue unavailable")
    } else {
        protocol_steps.push("offline_mock_hold_500ms".to_string());
        ("mock_self_test", "")
    };

    record_lock_trace(
        channel,
        LOCK_HOLD_DURATION_MS,
        page,
        usage,
        stealth_was_active,
        stealth_restored,
        live_dispatched,
        locked_ok,
        raw_elapsed_ms,
        protocol_steps,
        err_msg,
    );

    if locked_ok {
        if let Ok(mut g) = runtime_state().lock() {
            g.device_intentionally_locked = true;
            g.recent_button_events.push(ev_down);
            g.recent_button_events.push(ev_up);
        }
    }

    LockScreenResult {
        ok: locked_ok,
        locked: locked_ok,
        dispatched: live_dispatched,
        stealth_restored,
        restore_steps,
        duration_ms: elapsed_ms,
        button_events: events,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeUnlockResult {
    pub ok: bool,
    pub woken: bool,
    pub unlocked: bool,
    pub dispatched: bool,
    pub wake_event: ButtonEvent,
    pub swipe_steps: Vec<TouchStep>,
    pub face_id_bypass_tap: TouchStep,
    pub keypad_wake_key: KeyStep,
    pub pin_steps: Vec<KeyStep>,
}

impl WakeUnlockResult {
    pub fn to_json_string(&self) -> String {
        let mut obj = Map::new();
        obj.insert("ok".into(), Value::Bool(self.ok));
        obj.insert("woken".into(), Value::Bool(self.woken));
        obj.insert("unlocked".into(), Value::Bool(self.unlocked));
        obj.insert("dispatched".into(), Value::Bool(self.dispatched));
        obj.insert(
            "swipeSteps".into(),
            Value::Number(Number::PosInt(self.swipe_steps.len() as u64)),
        );
        obj.insert(
            "pinSteps".into(),
            Value::Number(Number::PosInt(self.pin_steps.len() as u64)),
        );
        serde_json::to_string(&Value::Object(obj)).unwrap_or_else(|_| "{}".to_string())
    }
}

/// Wake iPhone, swipe lockscreen up (y: 58000 -> 12000), bypass Face ID scanning delay with
/// `(32768, 41000)` tap + Space `[0x2C]`, and inject raw digit HID keycodes **without touching
/// `_TYPED_BUFFER` / `typed_history.json`** (Zero Passcode Leakage).
pub fn wake_and_unlock_iphone(passcode: &str) -> WakeUnlockResult {
    record_user_input_activity(false);
    crate::mirror::request_live_pli();
    let clean_digits: String = passcode
        .trim()
        .chars()
        .filter(|c| c.is_ascii_digit())
        .take(MAX_PASSCODE_LEN)
        .collect();
    let (page, usage) = IndigoButton::Wake.usage();
    let wake_event = ButtonEvent {
        page,
        usage,
        is_down: true,
    };
    let swipe_steps = generate_lockscreen_swipe_steps();
    let face_id_bypass_tap = TouchStep {
        contact: true,
        x: 32768,
        y: 41000,
        sleep_ms: 40,
    };
    let keypad_wake_key = KeyStep {
        keys: vec![KEY_SPACE],
        duration_ms: 20,
    };
    let pin_steps = generate_pin_keypress_steps(&clean_digits);

    if let Ok(mut g) = runtime_state().lock() {
        g.device_intentionally_locked = false;
        g.recent_button_events.push(wake_event);
        g.recent_button_events.push(ButtonEvent {
            page,
            usage,
            is_down: false,
        });
        g.recent_key_steps.push(keypad_wake_key.clone());
        g.recent_key_steps.extend(pin_steps.iter().cloned());
    }

    let mut dispatched = false;
    if is_physical_hid_active() {
        // 1. Wake screen (ButtonPress + gentle center tap), then 450 ms settle for iPhone 17 Pro iOS 27 display wake
        let btn_ok = enqueue_physical_hid_command(PhysicalHidCommand::ButtonPress {
            page,
            usage,
            hold_ms: 50,
        });
        let wake_tap = vec![
            (true, 32768u16, 42000u16, 20u64),
            (false, 32768u16, 42000u16, 0u64),
        ];
        let _ = enqueue_physical_hid_command(PhysicalHidCommand::TouchSequence { steps: wake_tap });
        std::thread::sleep(Duration::from_millis(WAKE_SCREEN_SETTLE_MS));

        // 2. Smooth 8-step bottom-edge swipe-up (y: 65535 -> 22000, ~200 ms)
        let swipe_seq: Vec<(bool, u16, u16, u64)> = swipe_steps
            .iter()
            .map(|s| (s.contact, s.x, s.y, s.sleep_ms))
            .collect();
        let swipe_ok = enqueue_physical_hid_command(PhysicalHidCommand::TouchSequence { steps: swipe_seq });

        // 3. Face ID bypass + passcode sheet settle (600 ms -> bypass nudge + Space -> 550 ms = 1150 ms total)
        //    followed by isolated 6-digit PIN HID injection (22ms down + 38ms up = 60ms/digit)
        let key_ok = if !clean_digits.is_empty() {
            std::thread::sleep(Duration::from_millis(FACE_ID_PRE_BYPASS_WAIT_MS));
            let bypass_touch = vec![
                (true, face_id_bypass_tap.x, face_id_bypass_tap.y, 30u64),
                (false, face_id_bypass_tap.x, face_id_bypass_tap.y, 30u64),
            ];
            let _ = enqueue_physical_hid_command(PhysicalHidCommand::TouchSequence { steps: bypass_touch });
            let space_seq = vec![
                (keypad_wake_key.keys.clone(), keypad_wake_key.duration_ms),
                (Vec::new(), 30),
            ];
            let _ = enqueue_physical_hid_command(PhysicalHidCommand::KeyboardSequence { steps: space_seq });
            std::thread::sleep(Duration::from_millis(FACE_ID_POST_BYPASS_WAIT_MS));

            let pin_seq: Vec<(Vec<u8>, u64)> = pin_steps
                .iter()
                .map(|ks| (ks.keys.clone(), ks.duration_ms))
                .collect();
            enqueue_physical_hid_command(PhysicalHidCommand::KeyboardSequence { steps: pin_seq })
        } else {
            true
        };
        dispatched = btn_ok && swipe_ok && key_ok;

        // 4. Fire PLI burst (+0ms, +150ms, +400ms) so unlocked Home Screen appears immediately
        crate::mirror::request_live_pli();
        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(150));
            crate::mirror::request_live_pli();
            std::thread::sleep(Duration::from_millis(250));
            crate::mirror::request_live_pli();
        });
    }

    WakeUnlockResult {
        ok: !is_physical_hid_active() || dispatched,
        woken: true,
        unlocked: !clean_digits.is_empty(),
        dispatched,
        wake_event,
        swipe_steps,
        face_id_bypass_tap,
        keypad_wake_key,
        pin_steps,
    }
}

/// Safe `>=50s` Idle Keep-Awake Micro-Pulse (`[0xE0]` Left Ctrl press/release) — fires ONLY when:
/// - `stealth_dim_active == true`
/// - `device_intentionally_locked == false`
/// - `is_touch_dragging == false`
/// - `now_epoch_secs - last_user_input_ts >= 50.0`
pub fn check_and_build_idle_keep_awake_pulse(now_epoch_secs: f64) -> Option<Vec<KeyStep>> {
    let Ok(mut g) = runtime_state().lock() else {
        return None;
    };
    if !g.stealth_dim_active || g.device_intentionally_locked || g.is_touch_dragging {
        return None;
    }
    if (now_epoch_secs - g.last_user_input_ts) < IDLE_KEEP_AWAKE_THRESHOLD_SECS {
        return None;
    }
    g.last_user_input_ts = now_epoch_secs;
    let pulse = vec![
        KeyStep {
            keys: vec![KEY_LEFT_CTRL],
            duration_ms: 15,
        },
        KeyStep {
            keys: vec![],
            duration_ms: 15,
        },
    ];
    g.recent_key_steps.extend(pulse.iter().cloned());
    Some(pulse)
}

// ============================================================================
// Win32 DPAPI Encryption / Decryption (crypt32.dll + kernel32.dll)
// ============================================================================

#[cfg(windows)]
#[repr(C)]
struct CryptDataBlob {
    cb_data: u32,
    pb_data: *mut u8,
}

#[cfg(windows)]
const CRYPTPROTECT_UI_FORBIDDEN: u32 = 0x1;

#[cfg(windows)]
#[link(name = "crypt32")]
extern "system" {
    fn CryptProtectData(
        p_data_in: *const CryptDataBlob,
        sz_data_descr: *const u16,
        p_optional_entropy: *const CryptDataBlob,
        pv_reserved: *mut core::ffi::c_void,
        p_prompt_struct: *mut core::ffi::c_void,
        dw_flags: u32,
        p_data_out: *mut CryptDataBlob,
    ) -> i32;

    fn CryptUnprotectData(
        p_data_in: *const CryptDataBlob,
        ppsz_data_descr: *mut *mut u16,
        p_optional_entropy: *const CryptDataBlob,
        pv_reserved: *mut core::ffi::c_void,
        p_prompt_struct: *mut core::ffi::c_void,
        dw_flags: u32,
        p_data_out: *mut CryptDataBlob,
    ) -> i32;
}

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn LocalFree(hmem: *mut core::ffi::c_void) -> *mut core::ffi::c_void;
}

/// Encrypt sensitive bytes (such as device Auto-Unlock PIN) with Windows DPAPI bound to current user.
pub fn dpapi_encrypt(plaintext: &[u8]) -> Result<Vec<u8>, String> {
    if plaintext.is_empty() {
        return Ok(Vec::new());
    }
    #[cfg(windows)]
    unsafe {
        let mut in_buf = plaintext.to_vec();
        let data_in = CryptDataBlob {
            cb_data: in_buf.len() as u32,
            pb_data: in_buf.as_mut_ptr(),
        };
        let mut data_out = CryptDataBlob {
            cb_data: 0,
            pb_data: std::ptr::null_mut(),
        };
        let ok = CryptProtectData(
            &data_in,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut data_out,
        );
        if ok == 0 || data_out.pb_data.is_null() {
            return Err("Win32 CryptProtectData failed".to_string());
        }
        let slice = std::slice::from_raw_parts(data_out.pb_data, data_out.cb_data as usize);
        let out = slice.to_vec();
        LocalFree(data_out.pb_data.cast());
        Ok(out)
    }
    #[cfg(not(windows))]
    {
        Ok(plaintext.to_vec())
    }
}

/// Decrypt Windows DPAPI-protected bytes back into plaintext.
pub fn dpapi_decrypt(ciphertext: &[u8]) -> Result<Vec<u8>, String> {
    if ciphertext.is_empty() {
        return Ok(Vec::new());
    }
    #[cfg(windows)]
    unsafe {
        let mut in_buf = ciphertext.to_vec();
        let data_in = CryptDataBlob {
            cb_data: in_buf.len() as u32,
            pb_data: in_buf.as_mut_ptr(),
        };
        let mut data_out = CryptDataBlob {
            cb_data: 0,
            pb_data: std::ptr::null_mut(),
        };
        let ok = CryptUnprotectData(
            &data_in,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut data_out,
        );
        if ok == 0 || data_out.pb_data.is_null() {
            return Err("Win32 CryptUnprotectData failed".to_string());
        }
        let slice = std::slice::from_raw_parts(data_out.pb_data, data_out.cb_data as usize);
        let out = slice.to_vec();
        LocalFree(data_out.pb_data.cast());
        Ok(out)
    }
    #[cfg(not(windows))]
    {
        Ok(ciphertext.to_vec())
    }
}

pub fn dpapi_encrypt_pin_b64(pin: &str) -> Result<String, String> {
    let trimmed = pin.trim();
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    let encrypted = dpapi_encrypt(trimmed.as_bytes())?;
    Ok(plist::base64_encode(&encrypted))
}

pub fn dpapi_decrypt_pin_b64(b64: &str) -> Result<String, String> {
    let trimmed = b64.trim();
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    let raw = plist::base64_decode(trimmed)
        .map_err(|e| format!("invalid base64 DPAPI payload: {e}"))?;
    let decrypted = dpapi_decrypt(&raw)?;
    String::from_utf8(decrypted).map_err(|e| format!("decrypted PIN is not valid UTF-8: {e}"))
}

/// Encrypt and store Auto-Unlock PIN in process memory using Windows DPAPI. Never stores plaintext!
pub fn store_encrypted_pin(pin: &str) -> Result<String, String> {
    let cleaned: String = pin
        .trim()
        .chars()
        .filter(|c| c.is_ascii_digit())
        .take(MAX_PASSCODE_LEN)
        .collect();
    if cleaned.is_empty() {
        clear_stored_pin();
        return Ok(String::new());
    }
    let b64 = dpapi_encrypt_pin_b64(&cleaned)?;
    if let Ok(mut g) = runtime_state().lock() {
        g.encrypted_pin_b64 = Some(b64.clone());
    }
    Ok(b64)
}

pub fn load_decrypted_pin() -> Result<Option<String>, String> {
    let blob = runtime_state()
        .lock()
        .ok()
        .and_then(|g| g.encrypted_pin_b64.clone());
    match blob {
        Some(b64) if !b64.is_empty() => Ok(Some(dpapi_decrypt_pin_b64(&b64)?)),
        _ => Ok(None),
    }
}

pub fn has_stored_pin() -> bool {
    runtime_state()
        .lock()
        .map(|g| g.encrypted_pin_b64.as_ref().is_some_and(|s| !s.is_empty()))
        .unwrap_or(false)
}

pub fn clear_stored_pin() {
    if let Ok(mut g) = runtime_state().lock() {
        g.encrypted_pin_b64 = None;
    }
}

// ============================================================================
// Shadow Typing Buffer (Strict Parity with handset/screen.py)
// ============================================================================

/// Validate typed history entries: forbid long paste blocks (`>= 35` chars) or news boilerplate.
pub fn is_valid_typed_history_entry(line: &str) -> bool {
    if line.is_empty() {
        return true;
    }
    if line.chars().count() >= 35 {
        return false;
    }
    let low = line.to_ascii_lowercase();
    if low.contains("rekaman video amatir") || low.contains("unggahan bertajuk") {
        return false;
    }
    true
}

fn push_line_dedup(lines: &mut Vec<String>, line: String) {
    if lines.last().map(|last| last == &line).unwrap_or(false) {
        return;
    }
    lines.push(line);
    if lines.len() > 60 {
        lines.remove(0);
    }
}

fn persist_typed_buffer_if_configured(state: &TypedBufferState) {
    let Some(path) = state.history_path_override.as_ref() else {
        return;
    };
    let mut obj = Map::new();
    obj.insert(
        "last_active".into(),
        Value::String(state.last_active_line.clone()),
    );
    obj.insert("timestamp".into(), Value::Number(Number::Float(state.last_timestamp)));
    let arr = state
        .lines
        .iter()
        .map(|l| Value::String(l.clone()))
        .collect();
    obj.insert("buffer".into(), Value::Array(arr));
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(&Value::Object(obj)) {
        let _ = fs::write(path, json);
    }
}

pub fn set_typed_history_path_override(path: Option<PathBuf>) {
    if let Ok(mut g) = typed_buffer_state().lock() {
        g.history_path_override = path;
    }
}

pub fn save_typed_history_to(path: &Path) -> Result<(), String> {
    let Ok(mut g) = typed_buffer_state().lock() else {
        return Err("typed buffer lock poisoned".to_string());
    };
    g.history_path_override = Some(path.to_path_buf());
    persist_typed_buffer_if_configured(&g);
    Ok(())
}

/// Commit current typed line accumulator into shadow history buffer and reset accumulator.
pub fn commit_and_reset_current_typed_line() {
    if let Ok(mut g) = typed_buffer_state().lock() {
        let trimmed = g.current_line.trim().to_string();
        if trimmed.chars().count() >= 3 && is_valid_typed_history_entry(&trimmed) {
            push_line_dedup(&mut g.lines, trimmed);
        }
        g.current_line.clear();
        g.last_active_line.clear();
        persist_typed_buffer_if_configured(&g);
    }
}

/// Record a typed character, `Backspace`, `Space`, or `Enter` into the shadow typing buffer.
pub fn record_typed_char(ch: &str) {
    if ch.is_empty() {
        return;
    }
    let now = current_epoch_secs();
    let should_commit_timeout = typed_buffer_state()
        .lock()
        .map(|g| g.last_timestamp > 0.0 && (now - g.last_timestamp) > 6.0)
        .unwrap_or(false);
    if should_commit_timeout {
        commit_and_reset_current_typed_line();
    }

    if let Ok(mut g) = typed_buffer_state().lock() {
        if ch == "Backspace" {
            g.current_line.pop();
            let trimmed = g.current_line.trim().to_string();
            if trimmed.chars().count() >= 3 && is_valid_typed_history_entry(&trimmed) {
                g.last_active_line = trimmed;
            }
            g.last_timestamp = current_epoch_secs();
        } else if ch == "\n" || ch == "\r" || ch == "Enter" {
            let trimmed = g.current_line.trim().to_string();
            if trimmed.chars().count() >= 3 && is_valid_typed_history_entry(&trimmed) {
                push_line_dedup(&mut g.lines, trimmed.clone());
                g.last_active_line = trimmed;
                g.last_timestamp = current_epoch_secs();
            } else if trimmed.is_empty()
                && !g.lines.is_empty()
                && g.lines.last().map(|l| !l.is_empty()).unwrap_or(false)
            {
                g.lines.push(String::new());
                if g.lines.len() > 60 {
                    g.lines.remove(0);
                }
            }
            g.current_line.clear();
        } else if ch == "Space" || ch == " " {
            g.current_line.push(' ');
            let trimmed = g.current_line.trim().to_string();
            if trimmed.chars().count() >= 3 && is_valid_typed_history_entry(&trimmed) {
                g.last_active_line = trimmed;
            }
            g.last_timestamp = current_epoch_secs();
        } else if ch.chars().count() == 1 {
            g.current_line.push_str(ch);
            let trimmed = g.current_line.trim().to_string();
            if trimmed.chars().count() >= 3 && is_valid_typed_history_entry(&trimmed) {
                g.last_active_line = trimmed;
            }
            g.last_timestamp = current_epoch_secs();
        }
        persist_typed_buffer_if_configured(&g);
    }
}

/// Record text block or typed snippet into shadow buffer (ignoring passcode unlock completely).
pub fn record_typed_text(text: &str) {
    if text.is_empty() {
        return;
    }
    let now = current_epoch_secs();
    let should_commit_timeout = typed_buffer_state()
        .lock()
        .map(|g| g.last_timestamp > 0.0 && (now - g.last_timestamp) > 6.0)
        .unwrap_or(false);
    if should_commit_timeout {
        commit_and_reset_current_typed_line();
    }

    if let Ok(mut g) = typed_buffer_state().lock() {
        if text.contains('\n') || text.contains('\r') {
            let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
            let combined = format!("{}{}", g.current_line, normalized);
            let parts: Vec<&str> = combined.split('\n').collect();
            for part in &parts[..parts.len().saturating_sub(1)] {
                let trimmed = part.trim().to_string();
                if trimmed.chars().count() >= 3 && is_valid_typed_history_entry(&trimmed) {
                    push_line_dedup(&mut g.lines, trimmed.clone());
                    g.last_active_line = trimmed;
                    g.last_timestamp = current_epoch_secs();
                } else if trimmed.is_empty()
                    && !g.lines.is_empty()
                    && g.lines.last().map(|l| !l.is_empty()).unwrap_or(false)
                {
                    g.lines.push(String::new());
                    if g.lines.len() > 60 {
                        g.lines.remove(0);
                    }
                }
            }
            let tail_part = parts.last().copied().unwrap_or("");
            let tail_trimmed = tail_part.trim().to_string();
            if parts.len() >= 3
                && tail_trimmed.chars().count() >= 3
                && is_valid_typed_history_entry(&tail_trimmed)
            {
                push_line_dedup(&mut g.lines, tail_trimmed.clone());
                g.last_active_line = tail_trimmed;
                g.last_timestamp = current_epoch_secs();
                g.current_line.clear();
            } else {
                g.current_line = tail_part.to_string();
                if tail_trimmed.chars().count() >= 3 && is_valid_typed_history_entry(&tail_trimmed)
                {
                    g.last_active_line = tail_trimmed;
                    g.last_timestamp = current_epoch_secs();
                }
            }
        } else {
            g.current_line.push_str(text);
            let trimmed = g.current_line.trim().to_string();
            if trimmed.chars().count() >= 3 && is_valid_typed_history_entry(&trimmed) {
                g.last_active_line = trimmed;
            }
            g.last_timestamp = current_epoch_secs();
        }
        persist_typed_buffer_if_configured(&g);
    }
}

pub fn get_typed_lines_buffer() -> Vec<String> {
    typed_buffer_state()
        .lock()
        .map(|g| g.lines.clone())
        .unwrap_or_default()
}

pub fn get_last_typed_active_line() -> String {
    typed_buffer_state()
        .lock()
        .map(|g| g.last_active_line.clone())
        .unwrap_or_default()
}

pub fn get_current_typed_line() -> String {
    typed_buffer_state()
        .lock()
        .map(|g| g.current_line.clone())
        .unwrap_or_default()
}

pub fn clear_typed_lines_buffer() {
    if let Ok(mut g) = typed_buffer_state().lock() {
        g.current_line.clear();
        g.lines.clear();
        g.last_active_line.clear();
        g.last_timestamp = 0.0;
        if let Some(ref path) = g.history_path_override {
            let _ = fs::remove_file(path);
        }
    }
}

// ============================================================================
// Live Async CoreDevice HID Session (UniversalHidServiceClient + IndigoHidClient)
// ============================================================================

#[derive(Debug)]
pub enum HidCmd {
    Touch { contact: bool, x: u16, y: u16 },
    Fingers { touching: bool, points: [(u16, u16); 2] },
    Button(&'static str),
    Keys(Vec<u8>),
    Action { name: String, text: String },
}

type UniClient = UniversalHidServiceClient<Box<dyn ReadWrite>>;
type IndigoClient = IndigoHidClient<Box<dyn ReadWrite>>;

/// Active CoreDevice HID session over an RSD software tunnel (`AdapterHandle` + `RsdHandshake`).
#[derive(Debug)]
pub struct HidSession {
    adapter: AdapterHandle,
    rsd: RsdHandshake,
    ipad: bool,
    uni: Option<UniClient>,
    keyboard: Option<MainKeyboardService>,
    indigo: Option<IndigoClient>,
    pub dimmed: bool,
}

async fn async_sleep_ms(ms: u64) {
    if ms > 0 {
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }
}

impl HidSession {
    pub fn new(adapter: AdapterHandle, rsd: RsdHandshake, ipad: bool) -> Self {
        Self {
            adapter,
            rsd,
            ipad,
            uni: None,
            keyboard: None,
            indigo: None,
            dimmed: false,
        }
    }

    pub async fn run(&mut self, cmd: HidCmd) -> Result<String, String> {
        let res = match cmd {
            HidCmd::Touch { contact, x, y } => {
                self.touch(contact, x, y).await.map(|_| String::new())
            }
            HidCmd::Fingers { touching, points } => {
                let contacts = [0u8, 1].map(|i| TouchscreenContact {
                    identity: i,
                    touching,
                    x: points[i as usize].0,
                    y: points[i as usize].1,
                });
                let uni = self.ensure_uni().await?;
                uni.send_multitouch(&contacts, None)
                    .await
                    .map(|_| String::new())
                    .map_err(|e| format!("pinch: {e}"))
            }
            HidCmd::Button(name) => self.button(name).await.map(|_| String::new()),
            HidCmd::Keys(usages) => self.keys(&usages).await.map(|_| String::new()),
            HidCmd::Action { name, text } => self.action(&name, &text).await,
        };
        if res.is_err() {
            self.uni = None;
            self.keyboard = None;
            self.indigo = None;
        }
        res
    }

    async fn ensure_uni(&mut self) -> Result<&mut UniClient, String> {
        if self.uni.is_none() {
            let client = UniClient::connect_rsd(&mut self.adapter, &mut self.rsd)
                .await
                .map_err(|e| format!("touch service: {e}"))?;
            self.uni = Some(client);
        }
        Ok(self.uni.as_mut().expect("set above"))
    }

    async fn ensure_indigo(&mut self) -> Result<&mut IndigoClient, String> {
        if self.indigo.is_none() {
            let client = IndigoClient::connect_rsd(&mut self.adapter, &mut self.rsd)
                .await
                .map_err(|e| format!("button service: {e}"))?;
            self.indigo = Some(client);
        }
        Ok(self.indigo.as_mut().expect("set above"))
    }

    async fn touch(&mut self, contact: bool, x: u16, y: u16) -> Result<(), String> {
        let state = if contact {
            TOUCHSCREEN_STATE_CONTACT
        } else {
            TOUCHSCREEN_STATE_RELEASE
        };
        self.ensure_uni()
            .await?
            .send_touchscreen(state, x, y, None)
            .await
            .map_err(|e| format!("touch: {e}"))
    }

    async fn keys(&mut self, usages: &[u8]) -> Result<(), String> {
        self.ensure_uni().await?;
        let uni = self.uni.as_mut().expect("opened above");
        if self.keyboard.is_none() {
            let kb = uni
                .create_main_keyboard()
                .await
                .map_err(|e| format!("keyboard: {e}"))?;
            self.keyboard = Some(kb);
        }
        let kb = self.keyboard.as_mut().expect("set above");
        let pressed = usages
            .iter()
            .filter_map(|u| KeyboardUsage::new(u16::from(*u)).ok());
        uni.set_main_keyboard_usages(kb, pressed)
            .await
            .map_err(|e| format!("key: {e}"))
    }

    async fn button(&mut self, name: &str) -> Result<(), String> {
        let (btn, hold) = IndigoButton::from_name(name)
            .ok_or_else(|| format!("unknown button {name}"))?;
        let (page, code) = btn.usage();
        let indigo = self.ensure_indigo().await?;
        indigo
            .press_button(u64::from(page), u64::from(code), Duration::from_millis(hold))
            .await
            .map_err(|e| format!("button: {e}"))
    }

    async fn tap_steps(&mut self, steps: &[KeyStep]) -> Result<(), String> {
        for step in steps {
            self.keys(&step.keys).await?;
            async_sleep_ms(step.duration_ms).await;
        }
        Ok(())
    }

    pub async fn action(&mut self, name: &str, text: &str) -> Result<String, String> {
        if let Some(steps) = generate_ios_shortcut_steps(name) {
            self.tap_steps(&steps).await?;
            return Ok(String::new());
        }
        match name {
            "toggle-keyboard" | "toggle-onscreen" => {
                self.button("eject").await?;
                Ok(String::new())
            }
            "stealth" => {
                let (step, count) = if self.dimmed {
                    ("brightness-up", STEALTH_RESTORE_DEFAULT_STEPS)
                } else {
                    ("brightness-down", STEALTH_DIM_STEPS)
                };
                for _ in 0..count {
                    self.button(step).await?;
                    async_sleep_ms(25).await;
                }
                self.dimmed = !self.dimmed;
                Ok(if self.dimmed { "dimmed" } else { "restored" }.to_string())
            }
            "lock" => {
                if self.dimmed {
                    Box::pin(self.action("stealth", "")).await?;
                }
                self.button("lock").await?;
                Ok("locked".to_string())
            }
            "wake" => {
                if self.ipad {
                    self.button("home").await?;
                    async_sleep_ms(300).await;
                    self.keys(&[KEY_SPACE]).await?;
                    async_sleep_ms(40).await;
                    self.keys(&[]).await?;
                } else {
                    self.button("home").await?;
                    async_sleep_ms(200).await;
                    for step in generate_lockscreen_swipe_steps() {
                        self.touch(step.contact, step.x, step.y).await?;
                        async_sleep_ms(step.sleep_ms).await;
                    }
                    self.touch(true, 32768, 41000).await?;
                    async_sleep_ms(20).await;
                    self.touch(false, 32768, 41000).await?;
                    self.keys(&[KEY_SPACE]).await?;
                    async_sleep_ms(20).await;
                    self.keys(&[]).await?;
                }
                let keys = passcode_keys(text);
                for rep in &keys {
                    self.keys(rep).await?;
                    async_sleep_ms(40).await;
                    self.keys(&[]).await?;
                    async_sleep_ms(45).await;
                }
                Ok(if keys.is_empty() {
                    "woken".to_string()
                } else {
                    "unlocked".to_string()
                })
            }
            "paste-pc" | "type-text" => {
                let (steps, _typed) = generate_key_steps_for_text(text);
                self.tap_steps(&steps).await?;
                Ok(String::new())
            }
            _ => Err(format!("Unknown action: {name}")),
        }
    }
}

pub fn dispatch_physical_touch(contact: bool, x: u16, y: u16) -> bool {
    record_user_input_activity(contact);
    if !is_physical_hid_active() {
        return false;
    }
    enqueue_physical_hid_command(PhysicalHidCommand::Touch { contact, x, y })
}

pub fn dispatch_physical_tap(x: u16, y: u16) -> bool {
    record_user_input_activity(false);
    if !is_physical_hid_active() {
        return false;
    }
    enqueue_physical_hid_command(PhysicalHidCommand::TouchSequence {
        steps: vec![(true, x, y, 24), (false, x, y, 0)],
    })
}

pub fn dispatch_physical_touch_steps(steps: &[TouchStep]) -> bool {
    record_user_input_activity(false);
    if !is_physical_hid_active() || steps.is_empty() {
        return false;
    }
    let raw: Vec<(bool, u16, u16, u64)> = steps
        .iter()
        .map(|s| (s.contact, s.x, s.y, s.sleep_ms))
        .collect();
    enqueue_physical_hid_command(PhysicalHidCommand::TouchSequence { steps: raw })
}

/// Resolves iOS Consumer Page (0x0C) button name -> `(usage_page, usage_code, hold_ms)`.
pub fn resolve_named_button(name: &str) -> Option<(u16, u16, u64)> {
    match name.trim().to_ascii_lowercase().as_str() {
        "home" | "wake" => Some((HID_USAGE_PAGE_CONSUMER, HID_USAGE_WAKE_BUTTON, 50)),
        "lock" | "power" => Some((
            HID_USAGE_PAGE_CONSUMER,
            HID_USAGE_POWER_BUTTON,
            LOCK_HOLD_DURATION_MS,
        )),
        "volume-up" | "vol-up" | "volume_up" => {
            Some((HID_USAGE_PAGE_CONSUMER, HID_USAGE_VOLUME_UP, 30))
        }
        "volume-down" | "vol-down" | "volume_down" => {
            Some((HID_USAGE_PAGE_CONSUMER, HID_USAGE_VOLUME_DOWN, 30))
        }
        "mute" => Some((HID_USAGE_PAGE_CONSUMER, HID_USAGE_MUTE, 50)),
        "brightness-down" | "brightness_down" => {
            Some((HID_USAGE_PAGE_CONSUMER, HID_USAGE_BRIGHTNESS_DOWN, 25))
        }
        "brightness-up" | "brightness_up" => {
            Some((HID_USAGE_PAGE_CONSUMER, HID_USAGE_BRIGHTNESS_UP, 25))
        }
        "eject" | "keyboard-toggle" | "keyboard_toggle" => {
            Some((HID_USAGE_PAGE_CONSUMER, HID_USAGE_EJECT, 50))
        }
        "siri" => Some((HID_USAGE_PAGE_CONSUMER, 0x00CF, 1000)),
        _ => None,
    }
}

pub fn dispatch_physical_button_name(name: &str, state: &str) -> Result<bool, String> {
    record_user_input_activity(false);
    let n = name.trim().to_ascii_lowercase();
    let st = state.trim().to_ascii_lowercase();

    if matches!(n.as_str(), "recent" | "app-switcher" | "switcher") {
        let mut steps: Vec<(bool, u16, u16, u64)> = Vec::with_capacity(26);
        // Pre-gesture force touch release clears any stale contact on iOS dtuhidd
        steps.push((false, 32768, 32768, 8));
        steps.push((true, 32768, 65500, 12));
        for i in 1..=12u32 {
            let y = (65500u32 - ((65500u32 - 45000u32) * i / 12u32)) as u16;
            steps.push((true, 32768, y, 12));
        }
        // Hold pause 270 ms (9 reports @ 30 ms) with 1px micro-drift so iOS 27.2 always opens App Switcher
        for idx in 0..9u32 {
            let hold_y = if idx % 2 == 0 { 45000u16 } else { 44950u16 };
            steps.push((true, 32768, hold_y, 30));
        }
        steps.push((false, 32768, 44950, 0));

        if let Ok(mut g) = runtime_state().lock() {
            g.recent_button_events.push(ButtonEvent {
                page: HID_USAGE_PAGE_CONSUMER,
                usage: HID_USAGE_WAKE_BUTTON,
                is_down: true,
            });
            g.recent_button_events.push(ButtonEvent {
                page: HID_USAGE_PAGE_CONSUMER,
                usage: HID_USAGE_WAKE_BUTTON,
                is_down: false,
            });
        }
        if !is_physical_hid_active() {
            return Ok(false);
        }
        return Ok(enqueue_physical_hid_command(
            PhysicalHidCommand::TouchSequence { steps },
        ));
    }

    if matches!(
        n.as_str(),
        "notifications" | "notif" | "control" | "control-center" | "control_center"
    ) {
        return dispatch_physical_swipe_direction(&n);
    }

    let (page, usage, hold_ms) = resolve_named_button(&n)
        .ok_or_else(|| format!("Tombol HID tidak dikenal: {name}"))?;
    let ev_down = ButtonEvent {
        page,
        usage,
        is_down: true,
    };
    let ev_up = ButtonEvent {
        page,
        usage,
        is_down: false,
    };
    if let Ok(mut g) = runtime_state().lock() {
        match st.as_str() {
            "down" => g.recent_button_events.push(ev_down),
            "up" => g.recent_button_events.push(ev_up),
            _ => {
                g.recent_button_events.push(ev_down);
                g.recent_button_events.push(ev_up);
            }
        }
    }
    if !is_physical_hid_active() {
        return Ok(false);
    }
    let ok = match st.as_str() {
        "down" => enqueue_physical_hid_command(PhysicalHidCommand::Button {
            page,
            usage,
            is_down: true,
        }),
        "up" => enqueue_physical_hid_command(PhysicalHidCommand::Button {
            page,
            usage,
            is_down: false,
        }),
        _ if n == "home" => enqueue_physical_hid_command(PhysicalHidCommand::HomeCombo),
        _ => enqueue_physical_hid_command(PhysicalHidCommand::ButtonPress {
            page,
            usage,
            hold_ms,
        }),
    };
    Ok(ok)
}

pub fn dispatch_physical_key_usages(usages: &[u8]) -> bool {
    record_user_input_activity(false);
    if !is_physical_hid_active() {
        return false;
    }
    enqueue_physical_hid_command(PhysicalHidCommand::KeyboardUsages {
        usages: usages.to_vec(),
    })
}

pub fn dispatch_physical_key_steps(steps: &[KeyStep]) -> bool {
    record_user_input_activity(false);
    if !is_physical_hid_active() || steps.is_empty() {
        return false;
    }
    let raw: Vec<(Vec<u8>, u64)> = steps
        .iter()
        .map(|s| (s.keys.clone(), s.duration_ms))
        .collect();
    enqueue_physical_hid_command(PhysicalHidCommand::KeyboardSequence { steps: raw })
}

/// Golden `v4.0.0` parity (`handset/screen.py::spotlight_paste_clipboard_on_iphone`):
/// Opens iOS Spotlight (`Cmd+Space`), settles 820 ms, clears residual query with single Backspace
/// (`0x2A`, NO `Cmd+A` so iOS 27 never renders selection handles/balloons), and executes 4-step
/// Command latching `Cmd+V` followed by `request_live_pli()` for a crisp frame.
pub fn dispatch_spotlight_paste() -> bool {
    let steps = vec![
        KeyStep { keys: vec![], duration_ms: 20 },
        KeyStep { keys: vec![0xE3], duration_ms: 25 },
        KeyStep { keys: vec![0xE3, 0x2C], duration_ms: 45 },
        KeyStep { keys: vec![0xE3], duration_ms: 20 },
        KeyStep { keys: vec![], duration_ms: 820 },
        KeyStep { keys: vec![0x2A], duration_ms: 45 },
        KeyStep { keys: vec![], duration_ms: 65 },
        KeyStep { keys: vec![0xE3], duration_ms: 25 },
        KeyStep { keys: vec![0xE3, 0x19], duration_ms: 40 },
        KeyStep { keys: vec![0xE3], duration_ms: 18 },
        KeyStep { keys: vec![], duration_ms: 450 },
    ];
    let sent = dispatch_physical_key_steps(&steps);
    crate::mirror::request_live_pli();
    sent
}

/// Golden `v4.0.0` parity (`handset/screen.py::spotlight_move_cursor_to_start_on_iphone`):
/// Moves cursor to head of single-line Spotlight query via Up Arrow (`0x52`) + `Cmd+Left` (`[0xE3, 0x50]`)
/// strictly without `Cmd+A`.
pub fn dispatch_spotlight_move_head() -> bool {
    let steps = vec![
        KeyStep { keys: vec![0x52], duration_ms: 35 },
        KeyStep { keys: vec![], duration_ms: 35 },
        KeyStep { keys: vec![0xE3], duration_ms: 20 },
        KeyStep { keys: vec![0xE3, 0x50], duration_ms: 40 },
        KeyStep { keys: vec![0xE3], duration_ms: 20 },
        KeyStep { keys: vec![], duration_ms: 180 },
    ];
    let sent = dispatch_physical_key_steps(&steps);
    crate::mirror::request_live_pli();
    sent
}

/// Golden `v4.0.0` parity (`handset/screen.py::spotlight_cleanup_and_close_on_iphone`):
/// Releases modifiers and closes Spotlight via a single `Cmd+Space` toggle (NO Escape, NO `Cmd+A`/Backspace).
pub fn dispatch_spotlight_close() -> bool {
    let steps = vec![
        KeyStep { keys: vec![], duration_ms: 25 },
        KeyStep { keys: vec![0xE3], duration_ms: 20 },
        KeyStep { keys: vec![0xE3, 0x2C], duration_ms: 55 },
        KeyStep { keys: vec![0xE3], duration_ms: 20 },
        KeyStep { keys: vec![], duration_ms: 80 },
    ];
    dispatch_physical_key_steps(&steps)
}

pub fn dispatch_physical_swipe_direction(direction: &str) -> Result<bool, String> {
    let dir = direction.trim().to_ascii_lowercase();
    match dir.as_str() {
        "home" => dispatch_physical_button_name("home", "press"),
        "recent" | "switcher" | "app-switcher" => {
            dispatch_physical_button_name("recent", "press")
        }
        "back" => {
            let steps = linear_touch_swipe(300, 32768, 28000, 32768, 8, 12, 0);
            Ok(dispatch_physical_touch_steps(&steps))
        }
        "left" => {
            let steps = linear_touch_swipe(52000, 32768, 13000, 32768, 8, 12, 0);
            Ok(dispatch_physical_touch_steps(&steps))
        }
        "right" => {
            let steps = linear_touch_swipe(13000, 32768, 52000, 32768, 8, 12, 0);
            Ok(dispatch_physical_touch_steps(&steps))
        }
        "up" => {
            let steps = linear_touch_swipe(32768, 50000, 32768, 15000, 8, 12, 0);
            Ok(dispatch_physical_touch_steps(&steps))
        }
        "down" => {
            let steps = linear_touch_swipe(32768, 15000, 32768, 50000, 8, 12, 0);
            Ok(dispatch_physical_touch_steps(&steps))
        }
        "notifications" | "notif" | "control" | "control-center" | "control_center" => {
            let steps = build_system_top_edge_swipe_steps(&dir, get_active_rotation_deg())
                .ok_or_else(|| format!("Arah gestur tidak dikenal: {direction}"))?;
            let sent = dispatch_physical_touch_steps(&steps);
            crate::mirror::request_live_pli();
            Ok(sent)
        }
        _ => Err(format!("Arah gestur tidak dikenal: {direction}")),
    }
}

/// Resolves a single JSON touch object (`{type, x, y}` or `{nx, ny}`) to `(contact, ix, iy, is_tap)`.
fn resolve_touch_event_obj(obj: &Map<String, Value>, rotation_deg: u32) -> (bool, u16, u16, bool) {
    let ev_ty = obj
        .get("type")
        .and_then(|v| v.as_str())
        .or_else(|| obj.get("action").and_then(|v| v.as_str()))
        .unwrap_or("contact")
        .to_ascii_lowercase();
    let is_tap = ev_ty == "tap" || ev_ty == "click";
    let contact = !matches!(ev_ty.as_str(), "release" | "up" | "end");

    let has_nx = obj.contains_key("nx") || obj.contains_key("ny");
    let raw_x = obj
        .get("nx")
        .and_then(|v| v.as_f64())
        .or_else(|| obj.get("x").and_then(|v| v.as_f64()))
        .unwrap_or(32768.0);
    let raw_y = obj
        .get("ny")
        .and_then(|v| v.as_f64())
        .or_else(|| obj.get("y").and_then(|v| v.as_f64()))
        .unwrap_or(32768.0);

    let (ix, iy) = if has_nx
        || ((0.0..=1.0).contains(&raw_x)
            && (0.0..=1.0).contains(&raw_y)
            && (raw_x.fract() != 0.0 || raw_y.fract() != 0.0))
    {
        map_normalized_touch_to_ios(raw_x, raw_y, rotation_deg)
    } else {
        (
            raw_x.round().clamp(0.0, 65535.0) as u16,
            raw_y.round().clamp(0.0, 65535.0) as u16,
        )
    };
    (contact, ix, iy, is_tap)
}

/// Parses `/mirror/touch` (`/touch`) JSON body in Hybrid AirPlay mode, applying
/// `map_airplay_touch_to_ios_digitizer` (1:1 `zenlink_core::api::device_point`) for both
/// normalized `(nx, ny)` and `0..=65535` `(x, y)` viewport coordinates.
pub fn dispatch_airplay_touch_payload_json(
    body: &str,
    springboard_rot_deg: u32,
) -> (bool, u16, u16, bool, usize) {
    let map_obj = |obj: &Map<String, Value>| -> (bool, u16, u16, bool) {
        let ev_ty = obj
            .get("type")
            .and_then(|v| v.as_str())
            .or_else(|| obj.get("action").and_then(|v| v.as_str()))
            .unwrap_or("contact")
            .to_ascii_lowercase();
        let is_tap = ev_ty == "tap" || ev_ty == "click";
        let contact = !matches!(ev_ty.as_str(), "release" | "up" | "end");
        let has_nx = obj.contains_key("nx") || obj.contains_key("ny");
        let raw_x = obj
            .get("nx")
            .and_then(|v| v.as_f64())
            .or_else(|| obj.get("x").and_then(|v| v.as_f64()))
            .unwrap_or(0.5);
        let raw_y = obj
            .get("ny")
            .and_then(|v| v.as_f64())
            .or_else(|| obj.get("y").and_then(|v| v.as_f64()))
            .unwrap_or(0.5);
        let (nx, ny) = if has_nx || ((0.0..=1.0).contains(&raw_x) && (0.0..=1.0).contains(&raw_y))
        {
            (raw_x, raw_y)
        } else {
            (raw_x / 65535.0, raw_y / 65535.0)
        };
        let (dx, dy) = map_airplay_touch_to_ios_digitizer(nx, ny, springboard_rot_deg);
        (contact, dx as u16, dy as u16, is_tap)
    };

    let Ok(val) = serde_json::from_str(body) else {
        return (false, 32768, 32768, false, 0);
    };
    if let Some(arr) = val.get("events").and_then(|v| v.as_array()) {
        if arr.is_empty() {
            return (true, 32768, 32768, false, 0);
        }
        let mut seq = Vec::with_capacity(arr.len() * 2);
        let mut last_xy = (32768u16, 32768u16);
        for item in arr {
            if let Some(obj) = item.as_object() {
                let (contact, ix, iy, is_tap) = map_obj(obj);
                last_xy = (ix, iy);
                if is_tap {
                    seq.push((true, ix, iy, 24));
                    seq.push((false, ix, iy, 8));
                } else {
                    seq.push((contact, ix, iy, 8));
                }
            }
        }
        let count = seq.len();
        let sent = if is_physical_hid_active() && !seq.is_empty() {
            enqueue_physical_hid_command(PhysicalHidCommand::TouchSequence { steps: seq })
        } else {
            false
        };
        return (true, last_xy.0, last_xy.1, sent, count);
    }

    if let Some(obj) = val.as_object() {
        let (contact, ix, iy, is_tap) = map_obj(obj);
        let sent = if is_tap {
            dispatch_physical_tap(ix, iy)
        } else {
            dispatch_physical_touch(contact, ix, iy)
        };
        return (true, ix, iy, sent, 1);
    }

    (false, 32768, 32768, false, 0)
}

/// Parses `/mirror/touch` (`/touch`) JSON body (`{events:[...]}` or single `{type,x,y}` / `{nx,ny}`),
/// dispatches to live physical HID if active, and returns `(ok, last_x, last_y, was_sent, events_count)`.
pub fn dispatch_touch_payload_json(body: &str, rotation_deg: u32) -> (bool, u16, u16, bool, usize) {
    let Ok(val) = serde_json::from_str(body) else {
        return (false, 32768, 32768, false, 0);
    };
    if let Some(arr) = val.get("events").and_then(|v| v.as_array()) {
        if arr.is_empty() {
            return (true, 32768, 32768, false, 0);
        }
        let mut seq = Vec::with_capacity(arr.len() * 2);
        let mut last_xy = (32768u16, 32768u16);
        for item in arr {
            if let Some(obj) = item.as_object() {
                let (contact, ix, iy, is_tap) = resolve_touch_event_obj(obj, rotation_deg);
                last_xy = (ix, iy);
                if is_tap {
                    seq.push((true, ix, iy, 24));
                    seq.push((false, ix, iy, 8));
                } else {
                    seq.push((contact, ix, iy, 8));
                }
            }
        }
        let count = seq.len();
        let sent = if is_physical_hid_active() && !seq.is_empty() {
            enqueue_physical_hid_command(PhysicalHidCommand::TouchSequence { steps: seq })
        } else {
            false
        };
        return (true, last_xy.0, last_xy.1, sent, count);
    }

    if let Some(obj) = val.as_object() {
        let (contact, ix, iy, is_tap) = resolve_touch_event_obj(obj, rotation_deg);
        let sent = if is_tap {
            dispatch_physical_tap(ix, iy)
        } else {
            dispatch_physical_touch(contact, ix, iy)
        };
        return (true, ix, iy, sent, 1);
    }

    (false, 32768, 32768, false, 0)
}

/// Converts a wheel/scroll JSON payload (`{dy, dx, x, y}`) into a smooth multi-step swipe on physical HID.
pub fn dispatch_scroll_payload_json(body: &str) -> (bool, bool, usize) {
    let Ok(val) = serde_json::from_str(body) else {
        return (false, false, 0);
    };
    let dy = val
        .get("dy")
        .or_else(|| val.get("deltaY"))
        .and_then(|v| v.as_f64())
        .unwrap_or(-120.0);
    let dx = val
        .get("dx")
        .or_else(|| val.get("deltaX"))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let start_x = val
        .get("x")
        .and_then(|v| v.as_f64())
        .map(|v| v.round().clamp(1000.0, 64535.0) as u16)
        .unwrap_or(32768);
    let start_y = val
        .get("y")
        .and_then(|v| v.as_f64())
        .map(|v| v.round().clamp(1000.0, 64535.0) as u16)
        .unwrap_or(32768);
    let shift_x = (dx * 35.0).clamp(-22000.0, 22000.0) as i32;
    let shift_y = (-dy * 45.0).clamp(-22000.0, 22000.0) as i32;
    let end_x = (i32::from(start_x) + shift_x).clamp(2000, 63535) as u16;
    let end_y = (i32::from(start_y) + shift_y).clamp(2000, 63535) as u16;
    let steps = linear_touch_swipe(start_x, start_y, end_x, end_y, 6, 10, 0);
    let len = steps.len();
    let sent = dispatch_physical_touch_steps(&steps);
    (true, sent, len)
}

/// Parses `{"usages": [4, ...]}` integer HID usage array cleanly.
pub fn parse_u8_usages_from_json(body: &str) -> Option<Vec<u8>> {
    let val = serde_json::from_str(body).ok()?;
    let arr = val.get("usages")?.as_array()?;
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        if let Some(u) = item.as_u64() {
            if u <= 255 {
                out.push(u as u8);
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use idevice::services::rsd::RsdService;
    use idevice::tcp::adapter::Adapter;
    use std::collections::HashMap;
    use std::io::Read as _;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener};
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

    #[derive(Debug)]
    struct DummyTunnelRw;

    impl AsyncRead for DummyTunnelRw {
        fn poll_read(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    impl AsyncWrite for DummyTunnelRw {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            Poll::Ready(Ok(buf.len()))
        }
        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    static STATE_TEST_LOCK: &Mutex<()> = &crate::hid::HID_TEST_SERIAL_LOCK;

    #[test]
    fn test_synchronous_lock_duration_and_events() {
        let mut ctrl = IndigoHidController::new();
        let initial_keys = ctrl.key_history.len();
        let (duration, events) = ctrl.execute_synchronous_lock();

        assert_eq!(LOCK_HOLD_DURATION_MS, 500);
        assert!(
            duration >= Duration::from_millis(450),
            "Lock hold too short (iOS bounce filter risk): {:?}",
            duration
        );
        assert!(
            duration <= Duration::from_millis(750),
            "Lock hold too long (Siri risk): {:?}",
            duration
        );

        assert_eq!(events.len(), 2);
        assert_eq!(
            events[0],
            ButtonEvent {
                page: 12,
                usage: 0x30,
                is_down: true
            }
        );
        assert_eq!(
            events[1],
            ButtonEvent {
                page: 12,
                usage: 0x30,
                is_down: false
            }
        );
        assert_eq!(ctrl.key_history.len(), initial_keys);
    }

    #[test]
    fn test_shift_latching_question_mark() {
        let steps = generate_key_steps_for_char('?').unwrap();
        assert_eq!(steps.len(), 4);
        assert_eq!(steps[0].keys, vec![0xE1]);
        assert_eq!(steps[1].keys, vec![0xE1, 0x38]);
        assert_eq!(steps[2].keys, vec![0xE1]);
        assert!(steps[3].keys.is_empty());
    }

    #[test]
    fn test_stealth_dim_idempotent() {
        let mut ctrl = IndigoHidController::new();
        assert!(!ctrl.is_dimmed);

        let res1 = ctrl.execute_stealth_dim_toggle().unwrap();
        assert_eq!(res1, "dimmed");
        assert!(ctrl.is_dimmed);
        assert_eq!(ctrl.button_history.len(), 32);

        let res2 = ctrl.execute_stealth_dim_toggle().unwrap();
        assert_eq!(res2, "restored");
        assert!(!ctrl.is_dimmed);
        assert_eq!(ctrl.button_history.len(), 40);
    }

    #[test]
    fn test_wake_and_auto_unlock_pin() {
        let mut ctrl = IndigoHidController::new();
        let (wake_ev, swipe, pin_steps) = ctrl.execute_wake_and_unlock_pin("123456");

        assert_eq!(wake_ev.usage, 0x0040);
        assert_eq!(swipe.len(), 10);
        assert_eq!(pin_steps.len(), 12);
        assert_eq!(pin_steps[0].keys, vec![0x1E]);
        assert_eq!(pin_steps[10].keys, vec![0x23]);
    }

    #[test]
    fn test_inverse_touch_matrix_and_gestures() {
        // 0° portrait
        assert_eq!(map_normalized_touch_to_ios(0.25, 0.75, 0), (16384, 49151));
        // 90° landscape left
        assert_eq!(map_normalized_touch_to_ios(0.25, 0.75, 90), (49151, 49151));
        // 180° inverted portrait
        assert_eq!(
            map_normalized_touch_to_ios(0.25, 0.75, 180),
            (49151, 16384)
        );
        // 270° eFootball 2027 Asymmetric Lock: ios_x = 1 - ny, ios_y = nx
        assert_eq!(
            map_normalized_touch_to_ios(0.25, 0.75, 270),
            (16384, 16384)
        );
        assert_eq!(
            map_normalized_touch_to_ios(0.0, 1.0, 270),
            (0, 0)
        );
        assert_eq!(
            map_normalized_touch_to_ios(1.0, 0.0, 270),
            (65535, 65535)
        );

        let back = swipe_back_steps(10);
        assert!(back.first().unwrap().x <= 2500);
        assert!(!back.last().unwrap().contact);

        let recent = swipe_recent_app_switcher_steps();
        assert!(recent.iter().any(|s| s.sleep_ms >= 180));

        let pinch = pinch_zoom_frames(32768, 32768, 8000, 24000, 6);
        assert_eq!(pinch.len(), 8);
        assert!(pinch[0][0].touching && pinch[0][1].touching);
        assert!(!pinch.last().unwrap()[0].touching);
    }

    #[test]
    fn test_typing_shift_latching_twin_key_shortcuts_and_eject() {
        let (steps, typed) = generate_key_steps_for_text("karhutla aman wok?");
        assert_eq!(typed, 18);
        assert!(steps.windows(4).any(|w| {
            w[0].keys == vec![0xE1]
                && w[1].keys == vec![0xE1, 0x38]
                && w[2].keys == vec![0xE1]
                && w[3].keys.is_empty()
        }));

        // Smart quotes & em/en dashes normalization
        let norm = normalize_text_for_hid("\u{201C}Hello\u{201D} \u{2014} \u{2018}ok\u{2019}\r\n");
        assert_eq!(norm, "\"Hello\" - 'ok'\n");

        // Twin-key release pause on "messi" (two consecutive 's' = 0x16)
        let (messi_steps, _) = generate_key_steps_for_text("messi");
        assert!(messi_steps.windows(4).any(|w| {
            w[0].keys == vec![0x16]
                && w[1].keys.is_empty()
                && w[2].keys.is_empty()
                && w[2].duration_ms == 12
                && w[3].keys == vec![0x16]
        }));

        // iOS Cmd shortcuts
        let paste_cmd = generate_ios_shortcut_steps("paste-ios").unwrap();
        assert_eq!(paste_cmd[1].keys, vec![0xE3]);
        assert_eq!(paste_cmd[2].keys, vec![0xE3, 0x19]);

        // Pure Eject 0x0C, 0x00B8 (never 0x029D)
        let eject = generate_onscreen_keyboard_toggle_events();
        assert_eq!(eject.len(), 2);
        assert!(eject.iter().all(|e| e.page == 0x0C && e.usage == 0x00B8));

        // Zenith passcode_keys parity
        assert_eq!(passcode_keys("12345").len(), 6);
        assert_eq!(passcode_keys(&"1".repeat(40)).len(), 33);
    }

    #[test]
    fn test_runtime_stealth_lock_wake_dpapi_idle_and_zero_passcode_leak() {
        let _guard = STATE_TEST_LOCK.lock().unwrap();
        reset_hid_runtime_state();

        // Stealth Dim strict idempotency (16 -> 0 -> 4 -> 0)
        let d1 = set_iphone_physical_brightness("min", 16, false);
        assert!(!d1.skipped && d1.steps == 16 && d1.stealth);
        assert!(d1.button_events.iter().all(|e| e.page == 12 && e.usage == 0x0070));
        let d2 = set_iphone_physical_brightness("min", 16, false);
        assert!(d2.skipped && d2.steps == 0);
        let r1 = set_iphone_physical_brightness("restore", 99, false);
        assert!(!r1.skipped && r1.steps == 4 && !r1.stealth);
        assert!(r1.button_events.iter().all(|e| e.page == 12 && e.usage == 0x006F));
        let r2 = set_iphone_physical_brightness("restore", 4, false);
        assert!(r2.skipped && r2.steps == 0);

        // Dim again, then synchronous 500ms hold Lock must auto-restore 4 steps first!
        let _ = set_iphone_physical_brightness("min", 16, false);
        let lock_res = lock_iphone_screen_with_hold(true);
        assert!(lock_res.locked && lock_res.stealth_restored && lock_res.restore_steps == 4);
        assert!(lock_res.duration_ms >= 450 && lock_res.duration_ms <= 750);
        assert!(!is_stealth_dim_active());
        assert!(is_device_intentionally_locked());

        // Safe idle keep-awake must NOT fire when locked
        set_last_user_input_ts_for_test(current_epoch_secs() - 60.0);
        assert!(check_and_build_idle_keep_awake_pulse(current_epoch_secs()).is_none());

        // Wake + Auto-Unlock PIN with Win32 DPAPI & Zero Leakage to Typed Buffer
        record_typed_text("pesan rahasia aman\n");
        let b64 = store_encrypted_pin("123456").unwrap();
        assert!(!b64.contains("123456"));
        let pin = load_decrypted_pin().unwrap().unwrap();
        assert_eq!(pin, "123456");

        let wake = wake_and_unlock_iphone(&pin);
        assert!(wake.unlocked && wake.pin_steps.len() == 12);
        let buf = get_typed_lines_buffer();
        assert_eq!(buf, vec!["pesan rahasia aman".to_string()]);
        assert!(!buf.iter().any(|l| l.contains("123456")));

        // Idle keep-awake fires at >= 50s ONLY when stealth dim is active and unlocked & not dragging
        let _ = set_iphone_physical_brightness("min", 16, false);
        let now = current_epoch_secs();
        set_last_user_input_ts_for_test(now - 30.0);
        assert!(check_and_build_idle_keep_awake_pulse(now).is_none());
        set_last_user_input_ts_for_test(now - 52.0);
        let pulse = check_and_build_idle_keep_awake_pulse(now).unwrap();
        assert_eq!(pulse[0].keys, vec![0xE0]);
        assert!(pulse[1].keys.is_empty());

        reset_hid_runtime_state();
    }

    #[test]
    fn test_live_hid_session_over_mock_rsd_socket() {
        let uni_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let uni_port = uni_listener.local_addr().unwrap().port();
        let indigo_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let indigo_port = indigo_listener.local_addr().unwrap().port();

        let uni_task = std::thread::spawn(move || {
            let (mut sock, _) = uni_listener.accept().unwrap();
            let mut count = 0usize;
            for _ in 0..3 {
                let mut len_buf = [0u8; 4];
                if sock.read_exact(&mut len_buf).is_err() {
                    break;
                }
                let len = u32::from_be_bytes(len_buf) as usize;
                let mut payload = vec![0u8; len];
                sock.read_exact(&mut payload).unwrap();
                count += 1;
            }
            count
        });

        let indigo_task = std::thread::spawn(move || {
            let (mut sock, _) = indigo_listener.accept().unwrap();
            let mut count = 0usize;
            for _ in 0..2 {
                let mut len_buf = [0u8; 4];
                if sock.read_exact(&mut len_buf).is_err() {
                    break;
                }
                let len = u32::from_be_bytes(len_buf) as usize;
                let mut payload = vec![0u8; len];
                sock.read_exact(&mut payload).unwrap();
                count += 1;
            }
            count
        });

        let loopback = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), uni_port);
        let adapter = Adapter::new(
            Box::new(DummyTunnelRw),
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V4(Ipv4Addr::LOCALHOST),
        )
        .with_loopback_target(Some(loopback))
        .to_async_handle();

        let mut services = HashMap::new();
        services.insert(
            "com.apple.coredevice.hid".to_string(),
            RsdService {
                entitlement: "com.apple.coredevice.hid".to_string(),
                port: uni_port,
                uses_remote_xpc: true,
                features: None,
                service_version: None,
            },
        );
        services.insert(
            "com.apple.coredevice.indigohid".to_string(),
            RsdService {
                entitlement: "com.apple.coredevice.indigohid".to_string(),
                port: indigo_port,
                uses_remote_xpc: true,
                features: None,
                service_version: None,
            },
        );
        let rsd = RsdHandshake {
            services,
            protocol_version: 2,
            uuid: "TEST-RSD-HID".to_string(),
        };

        tokio::block_on_future(async move {
            let mut session = HidSession::new(adapter, rsd, false);
            session
                .run(HidCmd::Touch {
                    contact: true,
                    x: 32768,
                    y: 32768,
                })
                .await
                .unwrap();
            session.run(HidCmd::Keys(vec![0x1E])).await.unwrap();
            session.run(HidCmd::Button("eject")).await.unwrap();
        });

        assert_eq!(uni_task.join().unwrap(), 3); // Touch + CreateMainKeyboard + KeyboardUsages
        assert_eq!(indigo_task.join().unwrap(), 2); // Down + Up
    }

    #[test]
    fn test_dpapi_roundtrip_and_auto_restore_controller() {
        let encrypted = dpapi_encrypt_pin_b64("045199").unwrap();
        assert!(!encrypted.is_empty() && !encrypted.contains("045199"));
        assert_eq!(dpapi_decrypt_pin_b64(&encrypted).unwrap(), "045199");

        let mut ctrl = IndigoHidController::new();
        assert!(!ctrl.auto_restore_on_exit());
        let _ = ctrl.execute_stealth_dim_toggle().unwrap();
        assert!(ctrl.auto_restore_on_exit());
        assert!(!ctrl.is_dimmed);
    }

    #[test]
    fn test_system_swipes_and_controller_type_text() {
        let home = swipe_home_steps();
        assert_eq!(home.first().unwrap().y, 65535);
        assert!(!home.last().unwrap().contact);

        let dir_up = swipe_direction_steps("up").unwrap();
        assert!(dir_up.len() >= 10);

        let mut ctrl = IndigoHidController::new();
        let steps = ctrl.type_text("Hi!");
        // 'H' (4) + 'i' (2) + '!' (4) = 10 steps
        assert_eq!(steps.len(), 10);
    }

    #[test]
    fn test_tahap268_wake_unlock_faceid_bypass_timing_and_6digit_pin_isolation() {
        let _guard = STATE_TEST_LOCK.lock().unwrap();
        reset_hid_runtime_state();

        // 1. Calibrated iOS 27 iPhone 17 Pro timing constants
        assert_eq!(WAKE_SCREEN_SETTLE_MS, 450);
        assert_eq!(FACE_ID_PRE_BYPASS_WAIT_MS + FACE_ID_POST_BYPASS_WAIT_MS, 1150);
        assert_eq!(PIN_DIGIT_PRESS_MS + PIN_DIGIT_RELEASE_MS, 60);

        // 2. Smooth 8-step lockscreen swipe starts at bottom edge 65535 -> 22000
        let swipe = generate_lockscreen_swipe_steps();
        assert_eq!(swipe.len(), 10);
        assert!(swipe[0].contact && swipe[0].y == 65535);
        assert!(!swipe.last().unwrap().contact && swipe.last().unwrap().y == 22000);

        // 3. 6-digit PIN generates 12 isolated steps (22ms down / 38ms up) with 0 leakage to typed buffer
        record_typed_text("kalibrasi pin aman\n");
        let wake = wake_and_unlock_iphone("749201");
        assert!(wake.woken && wake.unlocked);
        assert_eq!(wake.pin_steps.len(), 12);
        assert_eq!(wake.pin_steps[0].duration_ms, 22);
        assert_eq!(wake.pin_steps[1].duration_ms, 38);
        let typed = get_typed_lines_buffer();
        assert_eq!(typed, vec!["kalibrasi pin aman".to_string()]);
        assert!(!typed.iter().any(|l| l.contains("749201")));

        reset_hid_runtime_state();
    }

    #[test]
    fn test_tahap269_lock_power_uses_short_tap_25ms_anti_siri_and_single_dispatch() {
        let _guard = STATE_TEST_LOCK.lock().unwrap();
        reset_hid_runtime_state();

        // 1. Calibrated Power/Lock hold strictly in 480..=520ms (iOS SpringBoard 0.5s press-and-hold, <1000ms Siri)
        assert!((480..=520).contains(&LOCK_HOLD_DURATION_MS));
        assert_eq!(LOCK_HOLD_DURATION_MS, 500);
        for name in ["lock", "power"] {
            let (page, usage, hold) = resolve_named_button(name).expect("named button");
            assert_eq!((page, usage), (HID_USAGE_PAGE_CONSUMER, HID_USAGE_POWER_BUTTON));
            assert!((480..=520).contains(&hold) && hold < 750);
            let (btn, bhold) = IndigoButton::from_name(name).expect("IndigoButton::from_name");
            assert_eq!(btn, IndigoButton::PowerLock);
            assert!((480..=520).contains(&bhold) && bhold < 750);
        }

        // 2. Stealth restore + single down/up pair, plus 600ms debounce rejects rapid 2nd call
        let _ = set_iphone_physical_brightness("min", 16, false);
        let first = lock_iphone_screen_with_hold(true);
        assert!(first.ok && first.locked);
        assert!(first.stealth_restored && first.restore_steps == 4);
        assert!(first.duration_ms >= 450 && first.duration_ms <= 750);
        assert!(
            first
                .button_events
                .iter()
                .filter(|e| e.usage == HID_USAGE_POWER_BUTTON)
                .count()
                == 2
        );
        let second = lock_iphone_screen_with_hold(true);
        assert!(second.ok && second.locked && !second.dispatched);
        assert!(second.button_events.is_empty(), "debounce must block double-dispatch");

        reset_hid_runtime_state();
    }

    #[test]
    fn test_tahap270_lock_power_sweetspot_70ms_clamp_60_85_and_post_stealth_80ms_settle() {
        let _guard = STATE_TEST_LOCK.lock().unwrap();
        reset_hid_runtime_state();

        assert_eq!(LOCK_HOLD_DURATION_MS, 500);
        assert_eq!(LOCK_POST_STEALTH_SETTLE_MS, 80);
        assert_eq!(LOCK_DISPATCH_DEBOUNCE_MS, 600);
        assert_eq!(crate::physical_stream::clamp_power_lock_hold_ms(10), 480);
        assert_eq!(crate::physical_stream::clamp_power_lock_hold_ms(25), 480);
        assert_eq!(crate::physical_stream::clamp_power_lock_hold_ms(500), 500);
        assert_eq!(crate::physical_stream::clamp_power_lock_hold_ms(1200), 520);

        let _ = set_iphone_physical_brightness("min", 16, false);
        let wall_start = Instant::now();
        let first = lock_iphone_screen_with_hold(true);
        let total_ms = wall_start.elapsed().as_millis() as u64;
        assert!(first.ok && first.locked);
        assert!(first.stealth_restored && first.restore_steps == 4);
        assert!(
            total_ms >= 550,
            "Stealth restore + 80ms settle + 500ms hold expected >=550ms total, got {total_ms}ms"
        );
        assert!(
            first.duration_ms >= 450 && first.duration_ms <= 750,
            "Power/Lock hold duration outside 500ms sleep band (450..=750ms): {}ms",
            first.duration_ms
        );
        assert_eq!(
            first
                .button_events
                .iter()
                .filter(|e| e.usage == HID_USAGE_POWER_BUTTON)
                .count(),
            2
        );

        let blocked = lock_iphone_screen_with_hold(true);
        assert!(blocked.ok && blocked.locked && !blocked.dispatched);
        assert!(blocked.button_events.is_empty());

        reset_hid_runtime_state();
    }

    #[test]
    fn test_tahap271_lock_screen_exact_tahap259_parity_single_dispatch_and_stealth_sync() {
        let _guard = STATE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_hid_runtime_state();

        // 1. Fresh-open indigo XPC handshake settle constant & calibrated 500ms sleep hold (< 1000ms Siri)
        assert_eq!(crate::physical_stream::INDIGO_XPC_FRESH_OPEN_SETTLE_MS, 180);
        assert_eq!(LOCK_HOLD_DURATION_MS, 500);
        assert!((480..=520).contains(&crate::physical_stream::clamp_power_lock_hold_ms(500)));

        // 2. Active Stealth Dim is restored (4 brightness-up steps + stealth_restored=true) and
        //    is_stealth_dim_active() flips false synchronously BEFORE PowerLock press.
        let _ = set_iphone_physical_brightness("min", 16, false);
        assert!(is_stealth_dim_active());
        let res = lock_iphone_screen_with_hold(true);
        assert!(res.ok && res.locked);
        assert!(res.stealth_restored);
        assert_eq!(res.restore_steps, 4);
        assert!(!is_stealth_dim_active(), "Stealth Dim state must sync OFF when Lock auto-restores brightness");
        assert_eq!(
            res.button_events
                .iter()
                .filter(|e| e.usage == HID_USAGE_POWER_BUTTON)
                .count(),
            2
        );

        // 3. When live physical HID is marked active WITHOUT worker tx slot, dispatch fails honestly
        //    and clears debounce so user can immediately retry without waiting 600ms!
        crate::physical_stream::set_physical_hid_active_for_test(true);
        LAST_LOCK_DISPATCH_MS.store(0, Ordering::SeqCst);
        let failed = lock_iphone_screen_with_hold(true);
        assert!(!failed.dispatched);
        assert!(!failed.locked);
        assert!(!failed.ok);
        // Debounce was cleared on live failure, so next call is NOT blocked by debounce!
        assert!(try_acquire_lock_debounce());

        crate::physical_stream::set_physical_hid_active_for_test(false);
        reset_hid_runtime_state();
    }

    #[test]
    fn test_tahap272_lock_screen_bulletproof_500ms_single_dispatch_stealth_restore_and_trace() {
        let _guard = STATE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_hid_runtime_state();
        clear_lock_trace_for_test();

        assert_eq!(LOCK_HOLD_DURATION_MS, 500);
        assert_eq!(crate::physical_stream::clamp_power_lock_hold_ms(10), 480);
        assert_eq!(crate::physical_stream::clamp_power_lock_hold_ms(500), 500);
        assert_eq!(crate::physical_stream::clamp_power_lock_hold_ms(1200), 520);

        let _ = set_iphone_physical_brightness("min", 16, false);
        let first = lock_iphone_screen_with_hold(true);
        assert!(first.ok && first.locked && first.stealth_restored && first.restore_steps == 4);
        assert!(first.duration_ms >= 450 && first.duration_ms <= 750);
        assert!(!is_stealth_dim_active());

        // Immediate second call hits 600ms debounce so double-click cannot stack into Siri (>=1000ms)
        let second = lock_iphone_screen_with_hold(true);
        assert!(second.ok && second.locked && !second.dispatched);
        assert!(second.button_events.is_empty());

        record_lock_wire_outcome("indigo_xpc", true, None);
        let json = lock_trace_json();
        assert!(json.contains(r#""lockHoldDurationMs":500"#), "{json}");
        assert!(json.contains(r#""usagePage":12"#), "{json}");
        assert!(json.contains(r#""usageCode":48"#), "{json}");
        assert!(json.contains(r#""holdMs":500"#), "{json}");
        assert!(json.contains(r#""wireOutcome":"indigo_xpc:ok""#), "{json}");

        clear_lock_trace_for_test();
        reset_hid_runtime_state();
    }
}
