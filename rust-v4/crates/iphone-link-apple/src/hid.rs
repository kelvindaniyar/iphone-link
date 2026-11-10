//! CoreDevice Indigo HID controller: Touch, Scroll 120Hz, Synchronous 0.5s Lock,
//! Smart Wake & Auto-Unlock PIN 6-Digit, Stealth Dim, and Multi-Step Shift Latching.

use std::time::{Duration, Instant};

pub const HID_USAGE_PAGE_CONSUMER: u16 = 12; // 0x000C
pub const HID_USAGE_POWER_BUTTON: u16 = 0x0030;
pub const HID_USAGE_WAKE_BUTTON: u16 = 0x0040;
pub const HID_USAGE_BRIGHTNESS_DOWN: u16 = 0x0070;
pub const HID_USAGE_BRIGHTNESS_UP: u16 = 0x006F;
pub const HID_USAGE_VOLUME_UP: u16 = 0x00E9;
pub const HID_USAGE_VOLUME_DOWN: u16 = 0x00EA;
pub const HID_USAGE_MUTE: u16 = 0x00E2;

pub const KEY_SHIFT_LEFT: u8 = 0xE1;
pub const KEY_ENTER: u8 = 0x28;
pub const KEY_SPACE: u8 = 0x2C;

pub const LOCK_HOLD_DURATION_MS: u64 = 500; // 0.5s synchronous hold

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndigoButton {
    PowerLock,
    Wake,
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
            Self::BrightnessDown => (HID_USAGE_PAGE_CONSUMER, HID_USAGE_BRIGHTNESS_DOWN),
            Self::BrightnessUp => (HID_USAGE_PAGE_CONSUMER, HID_USAGE_BRIGHTNESS_UP),
            Self::VolumeUp => (HID_USAGE_PAGE_CONSUMER, HID_USAGE_VOLUME_UP),
            Self::VolumeDown => (HID_USAGE_PAGE_CONSUMER, HID_USAGE_VOLUME_DOWN),
            Self::Mute => (HID_USAGE_PAGE_CONSUMER, HID_USAGE_MUTE),
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

/// Map a character to (HID usage code, needs_shift).
pub fn char_usage(ch: char) -> Option<(u8, bool)> {
    match ch {
        'a'..='z' => Some((0x04 + (ch as u8 - b'a'), false)),
        'A'..='Z' => Some((0x04 + (ch as u8 - b'A'), true)),
        ' ' => Some((KEY_SPACE, false)),
        '\n' | '\r' => Some((KEY_ENTER, false)),
        '\t' => Some((0x2B, false)),
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

/// Generate Multi-Step Shift Latching sequence for a character (Tahap 53):
/// If shifted:
///   Step 1: [0xE1] (14ms)
///   Step 2: [0xE1, key] (18ms)
///   Step 3: [0xE1] (8ms)
///   Step 4: [] (12ms)
/// If unshifted:
///   Step 1: [key] (20ms)
///   Step 2: [] (15ms)
pub fn generate_key_steps_for_char(ch: char) -> Option<Vec<KeyStep>> {
    let (code, needs_shift) = char_usage(ch)?;
    if needs_shift {
        Some(vec![
            KeyStep { keys: vec![KEY_SHIFT_LEFT], duration_ms: 14 },
            KeyStep { keys: vec![KEY_SHIFT_LEFT, code], duration_ms: 18 },
            KeyStep { keys: vec![KEY_SHIFT_LEFT], duration_ms: 8 },
            KeyStep { keys: vec![], duration_ms: 12 },
        ])
    } else {
        Some(vec![
            KeyStep { keys: vec![code], duration_ms: 20 },
            KeyStep { keys: vec![], duration_ms: 15 },
        ])
    }
}

/// Convert PIN string (digits 0..9) to PIN HID keypress steps with 40ms press and 45ms pause.
pub fn generate_pin_keypress_steps(pin: &str) -> Vec<KeyStep> {
    let mut steps = Vec::new();
    for ch in pin.chars() {
        if let Some((code, false)) = char_usage(ch) {
            steps.push(KeyStep { keys: vec![code], duration_ms: 40 });
            steps.push(KeyStep { keys: vec![], duration_ms: 45 });
        }
    }
    steps
}

/// Generate smooth 8-step swipe up sequence (y: 58000 -> 12000) over 220 ms.
pub fn generate_lockscreen_swipe_steps() -> Vec<TouchStep> {
    let mut steps = Vec::with_capacity(10);
    // Initial contact
    steps.push(TouchStep { contact: true, x: 32768, y: 58000, sleep_ms: 20 });
    // 8 intermediate steps
    for i in 1..=8u32 {
        let y = 58000 - (46000 * i / 8);
        steps.push(TouchStep { contact: true, x: 32768, y: y as u16, sleep_ms: 25 });
    }
    // Release
    steps.push(TouchStep { contact: false, x: 32768, y: 12000, sleep_ms: 20 });
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

        // 1. Power Down
        let ev_down = ButtonEvent { page, usage, is_down: true };
        self.button_history.push(ev_down);

        // 2. Synchronous hold for 500 ms
        std::thread::sleep(Duration::from_millis(LOCK_HOLD_DURATION_MS));

        // 3. Power Up
        let ev_up = ButtonEvent { page, usage, is_down: false };
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
            // Restore brightness via 4 steps
            for _ in 0..4 {
                self.button_history.push(ButtonEvent { page: page_up, usage: usage_up, is_down: true });
                self.button_history.push(ButtonEvent { page: page_up, usage: usage_up, is_down: false });
            }
            self.is_dimmed = false;
            Ok("restored")
        } else {
            // Dim brightness to 0% via 16 steps
            for _ in 0..16 {
                self.button_history.push(ButtonEvent { page: page_down, usage: usage_down, is_down: true });
                self.button_history.push(ButtonEvent { page: page_down, usage: usage_down, is_down: false });
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
    pub fn execute_wake_and_unlock_pin(&mut self, pin_6digit: &str) -> (ButtonEvent, Vec<TouchStep>, Vec<KeyStep>) {
        let (page, usage) = IndigoButton::Wake.usage();
        let wake_event = ButtonEvent { page, usage, is_down: true };
        self.button_history.push(wake_event);
        self.button_history.push(ButtonEvent { page, usage, is_down: false });

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_synchronous_lock_duration_and_events() {
        let mut ctrl = IndigoHidController::new();
        let initial_keys = ctrl.key_history.len();
        let (duration, events) = ctrl.execute_synchronous_lock();

        // Must take at least 480ms and at most 650ms
        assert!(duration >= Duration::from_millis(480), "Lock hold too short: {:?}", duration);
        assert!(duration <= Duration::from_millis(650), "Lock hold too long: {:?}", duration);

        // Exactly 2 button events: Power down, Power up
        assert_eq!(events.len(), 2);
        assert_eq!(events[0], ButtonEvent { page: 12, usage: 0x30, is_down: true });
        assert_eq!(events[1], ButtonEvent { page: 12, usage: 0x30, is_down: false });

        // Zero keyboard events sent!
        assert_eq!(ctrl.key_history.len(), initial_keys);
    }

    #[test]
    fn test_shift_latching_question_mark() {
        // '?' must produce 4-step shift latching with Shift (0xE1) and key (0x38)
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

        // First dim: reaches 0% (16 steps = 32 events)
        let res1 = ctrl.execute_stealth_dim_toggle().unwrap();
        assert_eq!(res1, "dimmed");
        assert!(ctrl.is_dimmed);
        assert_eq!(ctrl.button_history.len(), 32);

        // Second toggle: restores to ~25% (4 steps = 8 events)
        let res2 = ctrl.execute_stealth_dim_toggle().unwrap();
        assert_eq!(res2, "restored");
        assert!(!ctrl.is_dimmed);
        assert_eq!(ctrl.button_history.len(), 40);
    }

    #[test]
    fn test_wake_and_auto_unlock_pin() {
        let mut ctrl = IndigoHidController::new();
        let (wake_ev, swipe, pin_steps) = ctrl.execute_wake_and_unlock_pin("123456");

        assert_eq!(wake_ev.usage, 0x0040); // Wake
        assert_eq!(swipe.len(), 10); // 1 start + 8 intermediate + 1 release
        assert_eq!(pin_steps.len(), 12); // 6 digits * (press + pause)
        assert_eq!(pin_steps[0].keys, vec![0x1E]); // '1'
        assert_eq!(pin_steps[10].keys, vec![0x23]); // '6'
    }
}
