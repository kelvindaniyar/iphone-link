//! CoreDevice Indigo HID controller: Touch, Scroll 120Hz, Smart Lock, Wake/PIN, and Stealth Dim.

pub const HID_USAGE_PAGE_CONSUMER: u16 = 12; // 0x000C
pub const HID_USAGE_POWER_BUTTON: u16 = 0x0030;
pub const HID_USAGE_VOLUME_UP: u16 = 0x00E9;
pub const HID_USAGE_VOLUME_DOWN: u16 = 0x00EA;
pub const HID_USAGE_MUTE: u16 = 0x00E2;

pub const LOCK_HOLD_DURATION_MS: u64 = 500; // 0.5s synchronous hold
pub const BUTTON_PRESS_DURATION_MS: u64 = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndigoButton {
    PowerLock,
    Home,
    VolumeUp,
    VolumeDown,
    Mute,
}

impl IndigoButton {
    pub fn usage(&self) -> (u16, u16) {
        match self {
            Self::PowerLock => (HID_USAGE_PAGE_CONSUMER, HID_USAGE_POWER_BUTTON),
            Self::VolumeUp => (HID_USAGE_PAGE_CONSUMER, HID_USAGE_VOLUME_UP),
            Self::VolumeDown => (HID_USAGE_PAGE_CONSUMER, HID_USAGE_VOLUME_DOWN),
            Self::Mute => (HID_USAGE_PAGE_CONSUMER, HID_USAGE_MUTE),
            Self::Home => (1, 0x0001),
        }
    }

    pub fn duration_ms(&self) -> u64 {
        match self {
            Self::PowerLock => LOCK_HOLD_DURATION_MS,
            _ => BUTTON_PRESS_DURATION_MS,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchPhase {
    Began,
    Moved,
    Ended,
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct TouchPoint {
    pub x: u16,       // 0..65535 normalized coordinate
    pub y: u16,       // 0..65535 normalized coordinate
    pub pressure: u16, // 0..65535
    pub phase: TouchPhase,
}

impl TouchPoint {
    pub fn new(x: u16, y: u16, phase: TouchPhase) -> Self {
        Self {
            x,
            y,
            pressure: if phase == TouchPhase::Ended { 0 } else { 32768 },
            phase,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ScrollEvent {
    pub delta_x: i16,
    pub delta_y: i16,
    pub rate_hz: u32,
}

impl Default for ScrollEvent {
    fn default() -> Self {
        Self {
            delta_x: 0,
            delta_y: 0,
            rate_hz: 120, // 120Hz smooth ProMotion scroll
        }
    }
}

/// Normalized center coordinates for iOS lock screen PIN keypad (digits 0..9).
pub fn pin_digit_coords(digit: char) -> Option<(u16, u16)> {
    // Keypad layout on iPhone screen in normalized 0..65535 coords:
    // row 1: 1, 2, 3 (~y: 35000)
    // row 2: 4, 5, 6 (~y: 41000)
    // row 3: 7, 8, 9 (~y: 47000)
    // row 4:    0    (~y: 53000)
    let col_left = 18000;
    let col_mid = 32768;
    let col_right = 47500;

    match digit {
        '1' => Some((col_left, 35000)),
        '2' => Some((col_mid, 35000)),
        '3' => Some((col_right, 35000)),
        '4' => Some((col_left, 41000)),
        '5' => Some((col_mid, 41000)),
        '6' => Some((col_right, 41000)),
        '7' => Some((col_left, 47000)),
        '8' => Some((col_mid, 47000)),
        '9' => Some((col_right, 47000)),
        '0' => Some((col_mid, 53000)),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StealthDimState {
    Normal100,
    Dimmed0,
    RestoringStep(u8),
}

impl StealthDimState {
    pub fn dim_steps() -> &'static [u8] {
        &[0, 25, 50, 75, 100] // 4-step ~25% brightness restore
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_button_timing() {
        assert_eq!(IndigoButton::PowerLock.duration_ms(), 500);
        assert_eq!(IndigoButton::Home.duration_ms(), 50);
        assert_eq!(IndigoButton::VolumeUp.duration_ms(), 50);
        assert_eq!(IndigoButton::VolumeDown.duration_ms(), 50);
    }

    #[test]
    fn test_pin_coords() {
        assert!(pin_digit_coords('0').is_some());
        assert!(pin_digit_coords('5').is_some());
        assert!(pin_digit_coords('9').is_some());
        assert!(pin_digit_coords('x').is_none());
    }

    #[test]
    fn test_touch_point() {
        let tp = TouchPoint::new(32768, 32768, TouchPhase::Began);
        assert_eq!(tp.phase, TouchPhase::Began);
        assert_eq!(tp.pressure, 32768);

        let tp_up = TouchPoint::new(32768, 32768, TouchPhase::Ended);
        assert_eq!(tp_up.pressure, 0);
    }
}
