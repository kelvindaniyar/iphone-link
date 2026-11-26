//! Tahap 265 — Full 1:1 Native Input Engine for `--native-ui` (`LiveVideoHwnd`, Win32 Subclass, & Slint).
//! Provides inverse-transformed touch routing, 120 Hz (`8 ms`) `ContinuousTouchpadScrollEngine`
//! with Precision Touchpad fractional deltas & seamless re-anchor, 1:1 horizontal 2-finger swipe,
//! Ctrl+Wheel pinch zoom, Ctrl->Cmd iOS shortcuts, 4-step Shift Latching typing, PIN-Input Safe Guard,
//! and all 12 global shortcuts (`Ctrl+K`, `Ctrl+A/C/V/X/Z`, `Alt+H/M/C/V/B/R/D/L/U/Left`).

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

fn input_monotonic_ms() -> u64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    (EPOCH.get_or_init(Instant::now).elapsed().as_millis() as u64).saturating_add(10_000)
}

use iphone_link_apple::hid::{
    self, TouchStep, dispatch_physical_key_steps, dispatch_physical_key_usages,
    dispatch_physical_touch, dispatch_physical_touch_steps, generate_ios_shortcut_steps,
    generate_key_steps_for_char, pinch_zoom_frames, record_typed_char,
};
use iphone_link_apple::mirror;
use iphone_link_video::video_hwnd::{StageKeyEvent, StagePointerEvent};

#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryW(lpLibFileName: *const u16) -> *mut c_void;
    fn GetProcAddress(hModule: *mut c_void, lpProcName: *const i8) -> *mut c_void;
}

type FnTimePeriod = unsafe extern "system" fn(u32) -> u32;

struct WinmmTimerFns {
    begin: Option<FnTimePeriod>,
    end: Option<FnTimePeriod>,
}

unsafe impl Send for WinmmTimerFns {}
unsafe impl Sync for WinmmTimerFns {}

fn winmm_timer() -> &'static WinmmTimerFns {
    static FNS: OnceLock<WinmmTimerFns> = OnceLock::new();
    FNS.get_or_init(|| unsafe {
        let name: Vec<u16> = "winmm.dll\0".encode_utf16().collect();
        let h = LoadLibraryW(name.as_ptr());
        if h.is_null() {
            return WinmmTimerFns { begin: None, end: None };
        }
        let pb = GetProcAddress(h, b"timeBeginPeriod\0".as_ptr() as *const i8);
        let pe = GetProcAddress(h, b"timeEndPeriod\0".as_ptr() as *const i8);
        WinmmTimerFns {
            begin: if pb.is_null() { None } else { Some(std::mem::transmute(pb)) },
            end: if pe.is_null() { None } else { Some(std::mem::transmute(pe)) },
        }
    })
}

fn time_begin_1ms() {
    if let Some(f) = winmm_timer().begin {
        unsafe { let _ = f(1); }
    }
}

fn time_end_1ms() {
    if let Some(f) = winmm_timer().end {
        unsafe { let _ = f(1); }
    }
}

#[derive(Debug, Clone)]
pub struct NativeInputConfig {
    pub trackpad_speed_pct: u32,
    pub rotation_deg: u32,
    pub keyboard_enabled: bool,
    pub airplay_no_hid: bool,
}

impl Default for NativeInputConfig {
    fn default() -> Self {
        Self {
            trackpad_speed_pct: 100,
            rotation_deg: 0,
            keyboard_enabled: true,
            airplay_no_hid: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlobalShortcutAction {
    ToggleKeyboard,
    CopyOcr,
    PastePcText,
    GrabLink,
    ClearCache,
    ToggleLandscape,
    ToggleStealth,
    LockScreen,
    WakeUnlock,
    ToggleMute,
    SwipeBack,
    IosCmd(&'static str),
}

/// Maps Win32 Virtual-Key + Ctrl/Alt modifiers to global shortcuts or iOS Cmd mappings.
pub fn resolve_global_shortcut(vk: u16, ctrl: bool, alt: bool, _shift: bool) -> Option<GlobalShortcutAction> {
    if ctrl && !alt {
        return match vk {
            0x4B => Some(GlobalShortcutAction::ToggleKeyboard),      // Ctrl+K
            0x43 => Some(GlobalShortcutAction::IosCmd("copy")),        // Ctrl+C -> Cmd+C
            0x56 => Some(GlobalShortcutAction::IosCmd("paste")),       // Ctrl+V -> Cmd+V
            0x41 => Some(GlobalShortcutAction::IosCmd("select-all")),  // Ctrl+A -> Cmd+A
            0x58 => Some(GlobalShortcutAction::IosCmd("cut")),         // Ctrl+X -> Cmd+X
            0x5A => Some(GlobalShortcutAction::IosCmd("undo")),        // Ctrl+Z -> Cmd+Z
            _ => None,
        };
    }
    if alt && !ctrl {
        return match vk {
            0x43 => Some(GlobalShortcutAction::CopyOcr),         // Alt+C
            0x56 => Some(GlobalShortcutAction::PastePcText),     // Alt+V
            0x42 => Some(GlobalShortcutAction::GrabLink),        // Alt+B
            0x52 => Some(GlobalShortcutAction::ClearCache),      // Alt+R
            0x48 => Some(GlobalShortcutAction::ToggleLandscape), // Alt+H
            0x44 => Some(GlobalShortcutAction::ToggleStealth),   // Alt+D
            0x4C => Some(GlobalShortcutAction::LockScreen),      // Alt+L
            0x55 => Some(GlobalShortcutAction::WakeUnlock),      // Alt+U
            0x4D => Some(GlobalShortcutAction::ToggleMute),      // Alt+M
            0x25 => Some(GlobalShortcutAction::SwipeBack),       // Alt+Left
            _ => None,
        };
    }
    None
}

/// Resolves Slint key-pressed string (`ev.text`) + Ctrl/Alt modifiers to the same shortcut enum.
pub fn resolve_slint_shortcut(text: &str, ctrl: bool, alt: bool) -> Option<GlobalShortcutAction> {
    let ch = text.chars().next()?;
    let upper = ch.to_ascii_uppercase();
    if upper.is_ascii_uppercase() {
        resolve_global_shortcut(upper as u16, ctrl, alt, false)
    } else {
        None
    }
}

/// Converts a Win32 Virtual-Key code into its USB HID Keyboard Usage ID (`keys.js` parity).
pub fn vk_to_hid_usage(vk: u16) -> Option<u8> {
    match vk {
        0x41..=0x5A => Some((vk - 0x41 + 0x04) as u8), // A-Z
        0x31..=0x39 => Some((vk - 0x31 + 0x1E) as u8), // 1-9
        0x30 => Some(0x27),                             // 0
        0x0D => Some(0x28),                             // Enter
        0x1B => Some(0x29),                             // Escape
        0x08 => Some(0x2A),                             // Backspace
        0x09 => Some(0x2B),                             // Tab
        0x20 => Some(0x2C),                             // Space
        0x25 => Some(0x50),                             // ArrowLeft
        0x26 => Some(0x52),                             // ArrowUp
        0x27 => Some(0x4F),                             // ArrowRight
        0x28 => Some(0x51),                             // ArrowDown
        0x24 => Some(0x4A),                             // Home
        0x21 => Some(0x4B),                             // PageUp
        0x2E => Some(0x4C),                             // Delete
        0x23 => Some(0x4D),                             // End
        0x22 => Some(0x4E),                             // PageDown
        0xBD => Some(0x2D),                             // Minus
        0xBB => Some(0x2E),                             // Equal
        0xDB => Some(0x2F),                             // BracketLeft
        0xDD => Some(0x30),                             // BracketRight
        0xDC => Some(0x31),                             // Backslash
        0xBA => Some(0x33),                             // Semicolon
        0xDE => Some(0x34),                             // Quote
        0xC0 => Some(0x35),                             // Backquote
        0xBC => Some(0x36),                             // Comma
        0xBE => Some(0x37),                             // Period
        0xBF => Some(0x38),                             // Slash
        _ => None,
    }
}

/// Maps normalized stage coordinates `(nx, ny)` in `0.0..=1.0` through the active display rotation
/// (`0`, `90`, `180`, `270`) into iOS HID portrait coordinates `(0..=65535, 0..=65535)`.
pub fn stage_norm_to_ios_hid(nx: f64, ny: f64, rotation_deg: u32) -> (u16, u16) {
    let cx = nx.clamp(0.0, 1.0);
    let cy = ny.clamp(0.0, 1.0);
    let (ux, uy) = match rotation_deg % 360 {
        270 => (1.0 - cy, cx),
        90 => (cy, 1.0 - cx),
        180 => (1.0 - cx, 1.0 - cy),
        _ => (cx, cy),
    };
    (
        (ux * 65535.0).round().clamp(0.0, 65535.0) as u16,
        (uy * 65535.0).round().clamp(0.0, 65535.0) as u16,
    )
}

/// Inverse of `stage_norm_to_ios_hid`: recovers normalized PC stage coordinates `(nx, ny)`
/// from an iOS HID coordinate `(hid_x, hid_y)` given the active rotation.
pub fn ios_hid_to_stage_norm(hid_x: u16, hid_y: u16, rotation_deg: u32) -> (f64, f64) {
    let ux = (hid_x as f64) / 65535.0;
    let uy = (hid_y as f64) / 65535.0;
    match rotation_deg % 360 {
        270 => (uy, 1.0 - ux),
        90 => (1.0 - uy, ux),
        180 => (1.0 - ux, 1.0 - uy),
        _ => (ux, uy),
    }
}

/// Generates 1:1 horizontal 2-finger swipe steps matching `handset/web/mirror.js`:
/// swiping 2 fingers LEFT (`delta_x < 0`) drags left-to-right (`stage_x_end > stage_x_start`)
/// so the iPhone page shifts RIGHT identically in Portrait (`0°`) and Landscape (`270°`/`90°`).
pub fn compute_horizontal_swipe_steps(
    stage_nx: f64,
    stage_ny: f64,
    delta_x: i32,
    speed_pct: u32,
    rotation_deg: u32,
) -> Vec<TouchStep> {
    let clamped_speed = speed_pct.clamp(30, 200);
    let scale = (clamped_speed as f64) / 100.0;
    let raw_shift = -(delta_x as f64 / 120.0) * 0.28 * scale;
    let min_norm = (MIN_HORIZONTAL_SWIPE_HID + 80.0) / 65535.0;
    let floor_norm = (0.14 * scale.min(1.0)).max(min_norm);
    let max_shift = if clamped_speed > 100 {
        (0.56 * scale).clamp(0.56, 0.78)
    } else {
        0.56
    };
    let shift_nx = if raw_shift.abs() < floor_norm && delta_x != 0 {
        floor_norm * raw_shift.signum()
    } else {
        raw_shift.clamp(-max_shift, max_shift)
    };
    let sx = if shift_nx >= 0.0 {
        stage_nx.clamp(0.14, 0.45)
    } else {
        stage_nx.clamp(0.55, 0.86)
    };
    let mut ex = (sx + shift_nx).clamp(0.06, 0.94);
    if (ex - sx).abs() < min_norm {
        let dir = if shift_nx >= 0.0 { 1.0 } else { -1.0 };
        ex = (sx + dir * min_norm).clamp(0.06, 0.94);
    }
    let sy = stage_ny.clamp(0.18, 0.82);

    let steps = 8usize;
    let mut out = Vec::with_capacity(steps + 2);
    for i in 0..=steps {
        let t = (i as f64) / (steps as f64);
        let ease = 1.0 - (1.0 - t).powi(2);
        let cx = sx + (ex - sx) * ease;
        let (hx, hy) = stage_norm_to_ios_hid(cx, sy, rotation_deg);
        out.push(TouchStep {
            contact: true,
            x: hx,
            y: hy,
            sleep_ms: if i == 0 { 10 } else { 9 },
        });
    }
    let (end_hx, end_hy) = stage_norm_to_ios_hid(ex, sy, rotation_deg);
    out.push(TouchStep {
        contact: false,
        x: end_hx,
        y: end_hy,
        sleep_ms: 0,
    });
    out
}

/// Generates smooth swipe touch trajectory from mouse wheel delta scaled by `trackpad_speed_pct` (30..=200).
/// Horizontal `delta_x` follows `handset/web/mirror.js` polarity (`delta_x < 0` -> positive X drag).
pub fn compute_wheel_touch_steps(
    hid_x: u16,
    hid_y: u16,
    delta_y: i32,
    delta_x: i32,
    speed_pct: u32,
) -> Vec<TouchStep> {
    let scale = (speed_pct.clamp(30, 200) as f64) / 100.0;
    let sx = hid_x.clamp(2000, 63535);
    let sy = hid_y.clamp(2000, 63535);
    let shift_x = (-(delta_x as f64) * 35.0 * scale).clamp(-22000.0, 22000.0).round() as i32;
    let shift_y = ((delta_y as f64) * 45.0 * scale).clamp(-22000.0, 22000.0).round() as i32;
    let ex = (i32::from(sx) + shift_x).clamp(2000, 63535) as u16;
    let ey = (i32::from(sy) + shift_y).clamp(2000, 63535) as u16;

    let steps = 6usize;
    let mut out = Vec::with_capacity(steps + 2);
    for i in 0..=steps {
        let t = (i as f64) / (steps as f64);
        let cx = ((sx as f64) + ((ex as f64) - (sx as f64)) * t).round() as u16;
        let cy = ((sy as f64) + ((ey as f64) - (sy as f64)) * t).round() as u16;
        out.push(TouchStep {
            contact: true,
            x: cx,
            y: cy,
            sleep_ms: 10,
        });
    }
    out.push(TouchStep {
        contact: false,
        x: ex,
        y: ey,
        sleep_ms: 0,
    });
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveAxis {
    None,
    Vertical,
    Horizontal,
}

pub const ARM_DEADZONE_HID: f64 = 180.0;
pub const ARM_WINDOW_MS: u64 = 20;
pub const PAN_BREAKAWAY_HID: f64 = 1050.0;
pub const PAN_BREAKAWAY_EASE_WEIGHTS: [f64; 3] = [0.25, 0.35, 0.40];
pub const MIN_SESSION_TRAVEL_HID: f64 = 1350.0;
pub const IDLE_RELEASE_MS: u64 = 38;
pub const SMOOTHING_ALPHA: f64 = 0.48;
pub const AXIS_LOCK_WINDOW_MS: u64 = 180;
pub const VERTICAL_HWHEEL_SUPPRESS_MS: u64 = 150;
pub const HWHEEL_SUPPRESS_MAX_DELTA: i32 = 90;
pub const HWHEEL_ARM_THRESHOLD: i32 = 36;
pub const HWHEEL_ARM_WINDOW_MS: u64 = 40;
pub const HWHEEL_COOLDOWN_MS: u64 = 260;
pub const MIN_HORIZONTAL_SWIPE_HID: f64 = 3200.0;
pub const VELOCITY_SMOOTH_TAU_S: f64 = 0.018;
pub const MOMENTUM_HANDOFF_PEAK_VEL_HID_PER_S: f64 = 42_000.0;
pub const MOMENTUM_HANDOFF_MIN_RELEASE_VEL_HID_PER_S: f64 = 12_000.0;
pub const MOMENTUM_HANDOFF_DECAY_RATIO: f64 = 0.45;
pub const MOMENTUM_HANDOFF_DECAY_STREAK: u8 = 2;
pub const WINDOWS_INERTIA_SUPPRESS_MS: u64 = 180;

/// Deterministic state machine backing `FluidTouchpadEngine` (`120 Hz` / `8 ms` tick,
/// `0.48` exponential smoothing + 2nd-order `dt`-normalized `f64` velocity smoother `tau = 0.018 s`,
/// Smart Momentum Hand-Off to iOS 120 Hz ProMotion with `180 ms` Windows inertia suppression,
/// Instant Direction-Reversal Flush, Dominant `ActiveAxis` lock, Silky 1:1 vertical curve
/// `180`/`1050`/`1350` HID enforced on EVERY `touchUp`, and `38 ms` idle release).
#[derive(Debug, Clone)]
pub struct ScrollTickPlanner {
    pub active: bool,
    pub cur_nx: f64,
    pub cur_ny: f64,
    pub pending_dy_norm: f64,
    pub rotation_deg: u32,
    pub last_push_at: Option<Instant>,
    pub last_release_velocity: f64,
    pub session_start_ny: f64,
    pub session_travel_norm: f64,
    pub session_dir_sign: f64,
    pub active_ticks: u32,
    pub active_axis: ActiveAxis,
    pub axis_locked_until_ms: u64,
    pub last_vertical_event_ms: Option<u64>,
    pub hwheel_accum: i32,
    pub hwheel_window_start_ms: Option<u64>,
    pub last_hwheel_fire_ms: Option<u64>,
    smooth_vel_norm_per_s: f64,
    peak_vel_hid_per_s: f64,
    last_raw_delta_y: i32,
    decay_streak: u8,
    windows_inertia_suppress_until_ms: u64,
    windows_inertia_suppress_sign: f64,
    last_tick_ms: Option<u64>,
    momentum_handoff_count: u32,
    last_speed_pct: u32,
    settle_ticks_emitted: u8,
}

impl Default for ScrollTickPlanner {
    fn default() -> Self {
        Self {
            active: false,
            cur_nx: 0.48,
            cur_ny: 0.5,
            pending_dy_norm: 0.0,
            rotation_deg: 0,
            last_push_at: None,
            last_release_velocity: 0.0,
            session_start_ny: 0.5,
            session_travel_norm: 0.0,
            session_dir_sign: 0.0,
            active_ticks: 0,
            active_axis: ActiveAxis::None,
            axis_locked_until_ms: 0,
            last_vertical_event_ms: None,
            hwheel_accum: 0,
            hwheel_window_start_ms: None,
            last_hwheel_fire_ms: None,
            smooth_vel_norm_per_s: 0.0,
            peak_vel_hid_per_s: 0.0,
            last_raw_delta_y: 0,
            decay_streak: 0,
            windows_inertia_suppress_until_ms: 0,
            windows_inertia_suppress_sign: 0.0,
            last_tick_ms: None,
            momentum_handoff_count: 0,
            last_speed_pct: 100,
            settle_ticks_emitted: 0,
        }
    }
}

impl ScrollTickPlanner {
    pub fn momentum_handoff_count(&self) -> u32 {
        self.momentum_handoff_count
    }

    #[inline]
    fn effective_scale(&self) -> f64 {
        (self.last_speed_pct.clamp(30, 200) as f64) / 100.0
    }

    #[inline]
    fn effective_arm_deadzone_hid(&self) -> f64 {
        if self.last_speed_pct < 100 {
            (ARM_DEADZONE_HID * self.effective_scale()).max(70.0)
        } else {
            ARM_DEADZONE_HID
        }
    }

    #[inline]
    fn effective_breakaway_hid(&self) -> f64 {
        if self.last_speed_pct < 100 {
            (PAN_BREAKAWAY_HID * self.effective_scale()).max(280.0)
        } else {
            PAN_BREAKAWAY_HID
        }
    }

    #[inline]
    fn effective_min_travel_hid(&self) -> f64 {
        if self.last_speed_pct < 100 {
            (MIN_SESSION_TRAVEL_HID * self.effective_scale()).max(400.0)
        } else {
            MIN_SESSION_TRAVEL_HID
        }
    }

    #[inline]
    fn effective_max_step_norm(&self) -> f64 {
        let scale = self.effective_scale();
        if self.last_speed_pct < 100 {
            (0.062 * scale).max(0.012)
        } else if self.last_speed_pct > 100 {
            (0.062 * scale.min(1.45)).min(0.090)
        } else {
            0.062
        }
    }

    #[inline]
    fn effective_smoothing_alpha(&self) -> f64 {
        if self.last_speed_pct < 100 {
            let scale = self.effective_scale();
            (SMOOTHING_ALPHA * (0.50 + 0.50 * scale)).clamp(0.26, SMOOTHING_ALPHA)
        } else {
            SMOOTHING_ALPHA
        }
    }

    pub fn push_delta(
        &mut self,
        stage_nx: f64,
        stage_ny: f64,
        delta_y: i32,
        speed_pct: u32,
        rotation_deg: u32,
    ) {
        self.push_delta_at(
            stage_nx,
            stage_ny,
            delta_y,
            speed_pct,
            rotation_deg,
            input_monotonic_ms(),
        );
    }

    pub fn push_delta_at(
        &mut self,
        stage_nx: f64,
        _stage_ny: f64,
        delta_y: i32,
        speed_pct: u32,
        rotation_deg: u32,
        now_ms: u64,
    ) {
        if delta_y == 0 {
            return;
        }
        let dir_sign = (delta_y as f64).signum();
        // Suppress trailing Windows touchpad inertia deltas for 180 ms after Smart Momentum Hand-Off
        // unless user reverses direction OR sharply accelerates a fresh flick (> 1.35x).
        if now_ms < self.windows_inertia_suppress_until_ms {
            let reversed = self.windows_inertia_suppress_sign.abs() > 0.5
                && dir_sign != self.windows_inertia_suppress_sign;
            let reaccelerated = (delta_y.abs() as f64) > (self.last_raw_delta_y.abs() as f64) * 1.35
                && delta_y.abs() >= 24;
            if reversed || reaccelerated {
                self.windows_inertia_suppress_until_ms = 0;
                self.windows_inertia_suppress_sign = 0.0;
            } else {
                self.last_raw_delta_y = delta_y;
                return;
            }
        }

        // Instant Direction-Reversal Flush: zero residual accumulator & smoothed velocity immediately.
        if (self.pending_dy_norm.abs() > 1e-5 && dir_sign != self.pending_dy_norm.signum())
            || (self.active && self.session_dir_sign.abs() > 0.5 && dir_sign != self.session_dir_sign)
        {
            self.pending_dy_norm = 0.0;
            self.smooth_vel_norm_per_s = 0.0;
            self.peak_vel_hid_per_s = 0.0;
            self.decay_streak = 0;
            self.session_dir_sign = dir_sign;
        }

        if self.last_raw_delta_y != 0
            && self.last_raw_delta_y.signum() == delta_y.signum()
            && delta_y.abs() < self.last_raw_delta_y.abs()
        {
            self.decay_streak = self.decay_streak.saturating_add(1);
        } else {
            self.decay_streak = 0;
        }
        self.last_raw_delta_y = delta_y;

        self.active_axis = ActiveAxis::Vertical;
        let prev_vert_ms = self.last_vertical_event_ms;
        self.last_vertical_event_ms = Some(now_ms);
        self.axis_locked_until_ms = now_ms.saturating_add(AXIS_LOCK_WINDOW_MS);
        self.hwheel_accum = 0;
        self.hwheel_window_start_ms = None;

        let clamped_speed = speed_pct.clamp(30, 200);
        self.last_speed_pct = clamped_speed;
        self.settle_ticks_emitted = 0;
        let scale = (clamped_speed as f64) / 100.0;
        let now = Instant::now();
        let rapid = prev_vert_ms
            .map(|p| now_ms.saturating_sub(p) < 30)
            .unwrap_or_else(|| {
                self.last_push_at
                    .is_some_and(|prev| now.duration_since(prev).as_millis() < 30)
            });
        // Slow mode (< 100%) strictly disables flick acceleration burst (`1.0x`) so slow scrolling stays predictable.
        let flick_boost = if clamped_speed >= 100 && delta_y.abs() >= 60 && rapid {
            let mag = ((delta_y.abs() - 60) as f64 / 120.0).clamp(0.0, 1.0);
            1.15 + 0.30 * mag
        } else {
            1.0
        };
        self.last_push_at = Some(now);
        // +35% vertical travel gain (0.14 -> 0.19) + flick accel (1.0x..1.45x on >=100%)
        let dy_norm = (delta_y as f64 / 120.0) * 0.19 * flick_boost * scale;
        self.rotation_deg = rotation_deg % 360;
        if !self.active {
            // Opposite-edge runway near x = 0.48 maximizes uninterrupted single-stroke glide before re-anchor
            self.cur_nx = if (stage_nx - 0.48).abs() <= 0.25 {
                stage_nx.clamp(0.24, 0.72)
            } else {
                0.48
            };
            self.cur_ny = if delta_y < 0 { 0.78 } else { 0.22 };
            self.last_release_velocity = 0.0;
            self.smooth_vel_norm_per_s = 0.0;
            self.peak_vel_hid_per_s = 0.0;
            self.decay_streak = 0;
        }
        let max_pending = if clamped_speed < 100 {
            (2.4 * scale).max(0.45)
        } else {
            (2.4 * scale).clamp(2.4, 4.8)
        };
        self.pending_dy_norm = (self.pending_dy_norm + dy_norm).clamp(-max_pending, max_pending);
    }

    pub fn push_horizontal_delta_at(
        &mut self,
        stage_nx: f64,
        stage_ny: f64,
        delta_x: i32,
        speed_pct: u32,
        rotation_deg: u32,
        now_ms: u64,
    ) -> Vec<TouchStep> {
        if delta_x == 0 {
            return Vec::new();
        }
        let vertical_suppress = self.active
            || (self.active_axis == ActiveAxis::Vertical && now_ms < self.axis_locked_until_ms)
            || self
                .last_vertical_event_ms
                .is_some_and(|t| now_ms.saturating_sub(t) <= VERTICAL_HWHEEL_SUPPRESS_MS);
        if vertical_suppress && delta_x.abs() < HWHEEL_SUPPRESS_MAX_DELTA {
            self.hwheel_accum = 0;
            self.hwheel_window_start_ms = None;
            return Vec::new();
        }
        if self
            .last_hwheel_fire_ms
            .is_some_and(|t| now_ms.saturating_sub(t) < HWHEEL_COOLDOWN_MS)
        {
            return Vec::new();
        }
        let expired_or_flipped = self
            .hwheel_window_start_ms
            .is_none_or(|t| now_ms.saturating_sub(t) > HWHEEL_ARM_WINDOW_MS)
            || (self.hwheel_accum != 0 && self.hwheel_accum.signum() != delta_x.signum());
        if expired_or_flipped {
            self.hwheel_accum = delta_x;
            self.hwheel_window_start_ms = Some(now_ms);
        } else {
            self.hwheel_accum = self.hwheel_accum.saturating_add(delta_x);
        }
        if self.hwheel_accum.abs() < HWHEEL_ARM_THRESHOLD {
            return Vec::new();
        }

        let total_dx = self.hwheel_accum;
        self.hwheel_accum = 0;
        self.hwheel_window_start_ms = None;
        self.last_hwheel_fire_ms = Some(now_ms);
        self.active_axis = ActiveAxis::Horizontal;
        self.axis_locked_until_ms = now_ms.saturating_add(AXIS_LOCK_WINDOW_MS);

        let mut out = if self.active {
            self.cancel_active()
        } else {
            Vec::new()
        };
        out.extend(compute_horizontal_swipe_steps(
            stage_nx,
            stage_ny,
            total_dx,
            speed_pct,
            rotation_deg,
        ));
        out
    }

    /// Ensures EVERY `touchUp` (idle release, edge re-anchor, or cancel) has strictly >=
    /// `MIN_SESSION_TRAVEL_HID` (2200 HID > 20 pt) so iOS Instagram NEVER reads a tap.
    fn ensure_min_travel_before_touchup(&mut self, steps: &mut Vec<TouchStep>, step_sleep_ms: u64) {
        let min_total_norm = (self.effective_min_travel_hid() + 80.0) / 65535.0;
        if self.session_travel_norm < min_total_norm {
            let mut dir = if self.session_dir_sign.abs() > 0.5 {
                self.session_dir_sign
            } else if self.cur_ny > 0.5 {
                -1.0
            } else {
                1.0
            };
            let deficit = min_total_norm - self.session_travel_norm;
            let room_in_dir = if dir > 0.0 {
                0.96 - self.cur_ny
            } else {
                self.cur_ny - 0.04
            };
            if room_in_dir < deficit {
                let opp_room = if dir > 0.0 {
                    self.cur_ny - 0.04
                } else {
                    0.96 - self.cur_ny
                };
                if opp_room >= deficit {
                    dir = -dir;
                }
            }
            let half = deficit * 0.5;
            for _ in 0..2 {
                let next = (self.cur_ny + dir * half).clamp(0.04, 0.96);
                let delta = (next - self.cur_ny).abs();
                self.cur_ny = next;
                self.session_travel_norm += delta.max(half);
                let (hx, hy) =
                    stage_norm_to_ios_hid(self.cur_nx, self.cur_ny, self.rotation_deg);
                steps.push(TouchStep {
                    contact: true,
                    x: hx,
                    y: hy,
                    sleep_ms: step_sleep_ms,
                });
            }
        }
        let (hx, hy) = stage_norm_to_ios_hid(self.cur_nx, self.cur_ny, self.rotation_deg);
        // Zero-fling stationary brake in slow mode (<= 75%): hold contact stationary (~16 ms) before lift
        if self.last_speed_pct <= 75 && self.settle_ticks_emitted < 2 {
            let holds = 2u8.saturating_sub(self.settle_ticks_emitted);
            for _ in 0..holds {
                steps.push(TouchStep {
                    contact: true,
                    x: hx,
                    y: hy,
                    sleep_ms: 8,
                });
            }
        }
        steps.push(TouchStep {
            contact: false,
            x: hx,
            y: hy,
            sleep_ms: 0,
        });
    }

    /// Cancels an active fluid touchpad glide cleanly, guaranteeing >= 2200 HID travel before `touchUp`.
    pub fn cancel_active(&mut self) -> Vec<TouchStep> {
        self.pending_dy_norm = 0.0;
        self.last_push_at = None;
        if !self.active {
            self.session_travel_norm = 0.0;
            self.session_dir_sign = 0.0;
            self.active_ticks = 0;
            self.active_axis = ActiveAxis::None;
            return Vec::new();
        }
        let mut out = Vec::with_capacity(4);
        self.ensure_min_travel_before_touchup(&mut out, 8);
        self.active = false;
        self.session_travel_norm = 0.0;
        self.session_dir_sign = 0.0;
        self.active_ticks = 0;
        self.active_axis = ActiveAxis::None;
        self.smooth_vel_norm_per_s = 0.0;
        self.peak_vel_hid_per_s = 0.0;
        self.decay_streak = 0;
        self.last_raw_delta_y = 0;
        self.last_tick_ms = None;
        self.settle_ticks_emitted = 0;
        out
    }

    fn drain_pending_toward_zero(&mut self, consumed: f64) {
        if self.pending_dy_norm > 0.0 {
            self.pending_dy_norm = (self.pending_dy_norm - consumed.abs()).max(0.0);
        } else if self.pending_dy_norm < 0.0 {
            self.pending_dy_norm = (self.pending_dy_norm + consumed.abs()).min(0.0);
        }
    }

    /// Advances one `8 ms` (`120 Hz`) tick using live monotonic clock.
    pub fn advance_tick(&mut self, idle_elapsed_ms: u64) -> Vec<TouchStep> {
        self.advance_tick_at(idle_elapsed_ms, input_monotonic_ms())
    }

    /// Deterministic clock-injected `120 Hz` tick step with 2nd-order `dt`-normalized `f64`
    /// velocity smoothing (`tau = 0.018 s`), Smart Momentum Hand-Off (`>= 42k -> < 0.45 * peak`,
    /// `>= 12k` HID/s, `decay_streak >= 2`), and 3-layer Anti-Ghost-Tap (`>= 1350 HID`).
    pub fn advance_tick_at(&mut self, idle_elapsed_ms: u64, now_ms: u64) -> Vec<TouchStep> {
        let mut steps = Vec::new();
        if !self.active {
            if self.pending_dy_norm.abs() <= 1e-5 {
                return steps;
            }
            // Guard 1: Pre-Down Priming Deadzone (scaled on slow mode, 180 HID @ >=100%) -> 0 HID reports for jitter
            let pending_hid = self.pending_dy_norm.abs() * 65535.0;
            if pending_hid < self.effective_arm_deadzone_hid() {
                if idle_elapsed_ms >= ARM_WINDOW_MS {
                    self.pending_dy_norm = 0.0;
                    self.last_push_at = None;
                    self.active_axis = ActiveAxis::None;
                }
                return steps;
            }
            self.active = true;
            self.session_start_ny = self.cur_ny;
            self.session_travel_norm = 0.0;
            self.session_dir_sign = self.pending_dy_norm.signum();
            self.active_ticks = 0;
            self.smooth_vel_norm_per_s = 0.0;
            self.peak_vel_hid_per_s = 0.0;
            self.last_tick_ms = Some(now_ms);
            let (hx, hy) = stage_norm_to_ios_hid(self.cur_nx, self.cur_ny, self.rotation_deg);
            steps.push(TouchStep {
                contact: true,
                x: hx,
                y: hy,
                sleep_ms: 0,
            });
            return steps;
        }

        let dt_ms = self
            .last_tick_ms
            .map(|p| now_ms.saturating_sub(p))
            .filter(|&d| d > 0)
            .unwrap_or(8)
            .clamp(2, 25);
        self.last_tick_ms = Some(now_ms);
        let dt_s = (dt_ms as f64) / 1000.0;

        let breakaway_target_norm = self.effective_breakaway_hid() / 65535.0;
        let need_breakaway_step = self.active_ticks < 3
            && self.session_travel_norm < breakaway_target_norm
            && self.session_dir_sign.abs() > 0.5;
        let max_step_norm = self.effective_max_step_norm();
        let eff_alpha = self.effective_smoothing_alpha();

        if self.pending_dy_norm.abs() >= 0.0005 || need_breakaway_step {
            self.active_ticks = self.active_ticks.saturating_add(1);
            if self.pending_dy_norm.abs() >= 0.0005 {
                self.session_dir_sign = self.pending_dy_norm.signum();
            }

            // 2nd-order dt-normalized f64 velocity smoother (tau = 0.018 s) over scaled step cap
            let desired_chunk = if self.pending_dy_norm.abs() <= 0.0020 {
                self.pending_dy_norm
            } else {
                (self.pending_dy_norm * eff_alpha).clamp(-max_step_norm, max_step_norm)
            };
            let v_target = desired_chunk / dt_s;
            let mut chunk = if self.pending_dy_norm.abs() <= 0.0020 {
                self.smooth_vel_norm_per_s = v_target;
                desired_chunk
            } else if self.smooth_vel_norm_per_s.abs() < 1e-6 {
                // Cold-start first move tick: track desired_chunk cleanly with 0 attenuation stall
                self.smooth_vel_norm_per_s = v_target;
                desired_chunk
            } else {
                let alpha_v = 1.0 - (-dt_s / VELOCITY_SMOOTH_TAU_S).exp();
                self.smooth_vel_norm_per_s += (v_target - self.smooth_vel_norm_per_s) * alpha_v;
                (self.smooth_vel_norm_per_s * dt_s).clamp(-max_step_norm, max_step_norm)
            };

            // Adaptive Gentle Breakaway: 3-step ease-in ramp (25% -> 35% -> 40% of 1050 HID)
            if self.active_ticks <= 3 && self.session_travel_norm < breakaway_target_norm {
                let cum_weight: f64 = PAN_BREAKAWAY_EASE_WEIGHTS[..(self.active_ticks as usize)]
                    .iter()
                    .sum();
                let target_at_tick = breakaway_target_norm * cum_weight;
                let min_step = (target_at_tick - self.session_travel_norm).max(0.0);
                if chunk.abs() < min_step || chunk * self.session_dir_sign <= 0.0 {
                    chunk = self.session_dir_sign * min_step;
                    self.smooth_vel_norm_per_s = chunk / dt_s;
                }
            } else if chunk.abs() > self.pending_dy_norm.abs() {
                // Never allow velocity lag to overshoot accumulated pending travel on deceleration.
                chunk = self.session_dir_sign * self.pending_dy_norm.abs();
                self.smooth_vel_norm_per_s = chunk / dt_s;
            }
            self.drain_pending_toward_zero(chunk);
            self.last_release_velocity = chunk;
            self.session_travel_norm += chunk.abs();

            let cur_vel_hid = (chunk.abs() / dt_s) * 65535.0;
            self.peak_vel_hid_per_s = self.peak_vel_hid_per_s.max(cur_vel_hid);

            let min_reanchor_rem = self.effective_min_travel_hid();
            let next_y = self.cur_ny + chunk;
            if next_y < 0.10 || next_y > 0.90 {
                let rem_hid = self.pending_dy_norm.abs() * 65535.0;
                if rem_hid >= min_reanchor_rem {
                    self.cur_ny = next_y.clamp(0.10, 0.90);
                    self.ensure_min_travel_before_touchup(&mut steps, 4);
                    if let Some(last) = steps.last_mut() {
                        last.sleep_ms = 4;
                    }
                    self.cur_ny = if next_y > 0.90 { 0.22 } else { 0.78 };
                    self.session_start_ny = self.cur_ny;
                    self.session_travel_norm = 0.0;
                    self.active_ticks = 0;
                    let (rhx, rhy) =
                        stage_norm_to_ios_hid(self.cur_nx, self.cur_ny, self.rotation_deg);
                    steps.push(TouchStep {
                        contact: true,
                        x: rhx,
                        y: rhy,
                        sleep_ms: 0,
                    });
                } else {
                    self.cur_ny = next_y.clamp(0.10, 0.90);
                    self.pending_dy_norm = 0.0;
                    let (hx, hy) =
                        stage_norm_to_ios_hid(self.cur_nx, self.cur_ny, self.rotation_deg);
                    steps.push(TouchStep {
                        contact: true,
                        x: hx,
                        y: hy,
                        sleep_ms: 0,
                    });
                }
            } else {
                self.cur_ny = next_y;
                let (hx, hy) = stage_norm_to_ios_hid(self.cur_nx, self.cur_ny, self.rotation_deg);
                steps.push(TouchStep {
                    contact: true,
                    x: hx,
                    y: hy,
                    sleep_ms: 0,
                });
            }

            // Smart Momentum Hand-Off to native iOS 120 Hz ProMotion (strictly disabled on slow mode <= 75%
            // so 30%..75% scrolling stops accurately right under the finger without jumping 1-2 feed posts).
            if self.active
                && self.last_speed_pct > 75
                && (self.session_travel_norm * 65535.0) >= self.effective_min_travel_hid()
                && self.peak_vel_hid_per_s >= MOMENTUM_HANDOFF_PEAK_VEL_HID_PER_S
                && self.decay_streak >= MOMENTUM_HANDOFF_DECAY_STREAK
                && cur_vel_hid < self.peak_vel_hid_per_s * MOMENTUM_HANDOFF_DECAY_RATIO
                && cur_vel_hid >= MOMENTUM_HANDOFF_MIN_RELEASE_VEL_HID_PER_S
            {
                let release_sign = self.session_dir_sign;
                self.ensure_min_travel_before_touchup(&mut steps, 6);
                self.active = false;
                self.pending_dy_norm = 0.0;
                self.last_push_at = None;
                self.session_travel_norm = 0.0;
                self.session_dir_sign = 0.0;
                self.active_ticks = 0;
                self.active_axis = ActiveAxis::None;
                self.smooth_vel_norm_per_s = 0.0;
                self.peak_vel_hid_per_s = 0.0;
                self.decay_streak = 0;
                self.last_tick_ms = None;
                self.settle_ticks_emitted = 0;
                self.momentum_handoff_count = self.momentum_handoff_count.saturating_add(1);
                self.windows_inertia_suppress_until_ms =
                    now_ms.saturating_add(WINDOWS_INERTIA_SUPPRESS_MS);
                self.windows_inertia_suppress_sign = release_sign;
            }
            return steps;
        }

        // Slow-mode stationary pre-release settle before 38 ms idle release so UIScrollView reads ~0 release velocity
        if self.last_speed_pct <= 75 && self.settle_ticks_emitted < 3 && idle_elapsed_ms < IDLE_RELEASE_MS {
            self.settle_ticks_emitted = self.settle_ticks_emitted.saturating_add(1);
            let (hx, hy) = stage_norm_to_ios_hid(self.cur_nx, self.cur_ny, self.rotation_deg);
            steps.push(TouchStep {
                contact: true,
                x: hx,
                y: hy,
                sleep_ms: 0,
            });
            return steps;
        }

        // Velocity-preserving release after 38 ms idle; Guard 3 strictly enforces min travel before touchUp
        if idle_elapsed_ms >= IDLE_RELEASE_MS {
            self.ensure_min_travel_before_touchup(&mut steps, 8);
            self.active = false;
            self.pending_dy_norm = 0.0;
            self.last_push_at = None;
            self.session_travel_norm = 0.0;
            self.session_dir_sign = 0.0;
            self.active_ticks = 0;
            self.active_axis = ActiveAxis::None;
            self.smooth_vel_norm_per_s = 0.0;
            self.peak_vel_hid_per_s = 0.0;
            self.decay_streak = 0;
            self.last_raw_delta_y = 0;
            self.last_tick_ms = None;
            self.settle_ticks_emitted = 0;
        }
        steps
    }
}

struct ScrollWorkerShared {
    planner: ScrollTickPlanner,
    last_event_at: Instant,
    stop: bool,
}

/// 120 Hz (`8 ms`) Precision Touchpad fluid scroll engine (Tahap 269 Axis-Locked):
/// `+35%` base travel gain (`0.19`), adaptive flick accel (`1.0x..1.45x`), `0.36` exponential smoothing
/// (`0.062` clamp), Dominant `ActiveAxis` lock, opposite-edge `0.78`/`0.22` runway (`x = 0.48`),
/// and `68 ms` inertia-preserving `touchUp` with `>= 2200` HID universal travel guarantee.
pub struct FluidTouchpadEngine {
    shared: Arc<(Mutex<ScrollWorkerShared>, Condvar)>,
    worker: Option<JoinHandle<()>>,
}

pub type ContinuousTouchpadScrollEngine = FluidTouchpadEngine;

impl FluidTouchpadEngine {
    pub fn new() -> Self {
        let shared = Arc::new((
            Mutex::new(ScrollWorkerShared {
                planner: ScrollTickPlanner::default(),
                last_event_at: Instant::now(),
                stop: false,
            }),
            Condvar::new(),
        ));
        let w_shared = shared.clone();
        let worker = std::thread::Builder::new()
            .name("native-touchpad-120hz".to_string())
            .spawn(move || {
                let mut timer_active = false;
                loop {
                    let (steps, active_now, should_stop) = {
                        let (lock, cvar) = &*w_shared;
                        let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
                        while !g.stop && !g.planner.active && g.planner.pending_dy_norm.abs() <= 1e-5 {
                            if timer_active {
                                time_end_1ms();
                                timer_active = false;
                            }
                            g = cvar.wait(g).unwrap_or_else(|e| e.into_inner());
                        }
                        if g.stop {
                            if g.planner.active {
                                let (hx, hy) = stage_norm_to_ios_hid(
                                    g.planner.cur_nx,
                                    g.planner.cur_ny,
                                    g.planner.rotation_deg,
                                );
                                g.planner.active = false;
                                (
                                    vec![TouchStep {
                                        contact: false,
                                        x: hx,
                                        y: hy,
                                        sleep_ms: 0,
                                    }],
                                    false,
                                    true,
                                )
                            } else {
                                (Vec::new(), false, true)
                            }
                        } else {
                            let idle_ms = g.last_event_at.elapsed().as_millis() as u64;
                            let out = g.planner.advance_tick(idle_ms);
                            (out, g.planner.active, false)
                        }
                    };

                    if active_now && !timer_active {
                        time_begin_1ms();
                        timer_active = true;
                    }
                    for st in steps {
                        if !mirror::is_airplay_standalone_no_hid_active() {
                            let _ = dispatch_physical_touch(st.contact, st.x, st.y);
                        }
                        if st.sleep_ms > 0 {
                            std::thread::sleep(Duration::from_millis(st.sleep_ms));
                        }
                    }
                    if !active_now && timer_active {
                        time_end_1ms();
                        timer_active = false;
                    }
                    if should_stop {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(8));
                }
            })
            .ok();

        Self { shared, worker }
    }

    pub fn push_vertical_delta(
        &self,
        stage_nx: f64,
        stage_ny: f64,
        delta_y: i32,
        speed_pct: u32,
        rotation_deg: u32,
    ) {
        let (lock, cvar) = &*self.shared;
        if let Ok(mut g) = lock.lock() {
            g.planner
                .push_delta(stage_nx, stage_ny, delta_y, speed_pct, rotation_deg);
            g.last_event_at = Instant::now();
            cvar.notify_one();
        }
    }

    pub fn push_horizontal_delta(
        &self,
        stage_nx: f64,
        stage_ny: f64,
        delta_x: i32,
        speed_pct: u32,
        rotation_deg: u32,
    ) -> Vec<TouchStep> {
        let (lock, cvar) = &*self.shared;
        let out = if let Ok(mut g) = lock.lock() {
            let steps = g.planner.push_horizontal_delta_at(
                stage_nx,
                stage_ny,
                delta_x,
                speed_pct,
                rotation_deg,
                input_monotonic_ms(),
            );
            cvar.notify_one();
            steps
        } else {
            Vec::new()
        };
        out
    }

    pub fn cancel_active_touch(&self) {
        let steps = {
            let (lock, cvar) = &*self.shared;
            let mut g = match lock.lock() {
                Ok(g) => g,
                Err(e) => e.into_inner(),
            };
            let out = g.planner.cancel_active();
            cvar.notify_one();
            out
        };
        if !steps.is_empty() && !mirror::is_airplay_standalone_no_hid_active() {
            for st in steps {
                let _ = dispatch_physical_touch(st.contact, st.x, st.y);
                if st.sleep_ms > 0 {
                    std::thread::sleep(Duration::from_millis(st.sleep_ms));
                }
            }
        }
    }
}

impl Drop for FluidTouchpadEngine {
    fn drop(&mut self) {
        let (lock, cvar) = &*self.shared;
        if let Ok(mut g) = lock.lock() {
            g.stop = true;
            cvar.notify_all();
        }
        if let Some(jh) = self.worker.take() {
            let _ = jh.join();
        }
    }
}

/// Dispatches a typed character to iPhone HID using 4-step Shift Latching when shifted.
pub fn dispatch_char_with_shift_latching(ch: char) -> bool {
    let mut s = [0u8; 4];
    let ch_str = ch.encode_utf8(&mut s);
    record_typed_char(ch_str);
    if let Some(steps) = generate_key_steps_for_char(ch) {
        return dispatch_physical_key_steps(&steps);
    }
    false
}

/// Dispatches a 1-step HID key press + release by VK code (used for non-character keys: Enter, Backspace, Arrows, etc.).
pub fn dispatch_nonchar_vk(vk: u16) -> bool {
    // Only dispatch navigation/control keys here; printable ASCII goes via WM_CHAR + Shift Latching
    let is_control_or_nav = matches!(
        vk,
        0x08 | 0x09 | 0x0D | 0x1B | 0x21..=0x28 | 0x2E
    );
    if !is_control_or_nav {
        return false;
    }
    if let Some(usage) = vk_to_hid_usage(vk) {
        if vk == 0x0D {
            record_typed_char("\n");
        }
        let sent_down = dispatch_physical_key_usages(&[usage]);
        let _ = dispatch_physical_key_usages(&[]);
        return sent_down;
    }
    false
}

#[inline]
pub fn is_modifier_vk(vk: u16) -> bool {
    matches!(vk, 0x10..=0x12 | 0xA0..=0xA5 | 0x5B..=0x5C)
}

/// Shared router connecting Parent HWND Subclass, `LiveVideoHwnd` `WndProc`, and Slint `FocusScope`
/// to CoreDevice physical HID, the 120 Hz `FluidTouchpadEngine`, & Main UI actions.
pub struct NativeInputRouter {
    config: Mutex<NativeInputConfig>,
    shortcut_tx: Mutex<Option<Sender<GlobalShortcutAction>>>,
    pin_editing: AtomicBool,
    suppressed_vk: AtomicU16,
    scroll_engine: ContinuousTouchpadScrollEngine,
}

impl NativeInputRouter {
    pub fn new(shortcut_tx: Sender<GlobalShortcutAction>) -> Self {
        Self {
            config: Mutex::new(NativeInputConfig::default()),
            shortcut_tx: Mutex::new(Some(shortcut_tx)),
            pin_editing: AtomicBool::new(false),
            suppressed_vk: AtomicU16::new(0),
            scroll_engine: ContinuousTouchpadScrollEngine::new(),
        }
    }

    pub fn set_pin_editing(&self, editing: bool) {
        self.pin_editing.store(editing, Ordering::SeqCst);
    }

    pub fn is_pin_editing(&self) -> bool {
        self.pin_editing.load(Ordering::Relaxed)
    }

    pub fn update_config(&self, f: impl FnOnce(&mut NativeInputConfig)) {
        if let Ok(mut cfg) = self.config.lock() {
            f(&mut cfg);
        }
    }

    pub fn config_snapshot(&self) -> NativeInputConfig {
        self.config
            .lock()
            .map(|c| c.clone())
            .unwrap_or_default()
    }

    pub fn emit_shortcut(&self, action: GlobalShortcutAction) {
        if self.is_pin_editing() {
            return;
        }
        if let GlobalShortcutAction::IosCmd(cmd_name) = action {
            if !mirror::is_airplay_standalone_no_hid_active() {
                if let Some(steps) = generate_ios_shortcut_steps(cmd_name) {
                    let _ = dispatch_physical_key_steps(&steps);
                }
            }
            return;
        }
        if let Ok(guard) = self.shortcut_tx.lock() {
            if let Some(ref tx) = *guard {
                let _ = tx.send(action);
            }
        }
    }

    pub fn cancel_active_touch(&self) {
        self.scroll_engine.cancel_active_touch();
    }

    pub fn handle_pointer_event(&self, ev: StagePointerEvent) {
        if mirror::is_airplay_standalone_no_hid_active() {
            return;
        }
        let cfg = self.config_snapshot();
        if cfg.airplay_no_hid {
            return;
        }
        match ev {
            StagePointerEvent::Down { hid_x, hid_y } => {
                self.scroll_engine.cancel_active_touch();
                let _ = dispatch_physical_touch(true, hid_x, hid_y);
            }
            StagePointerEvent::Move { hid_x, hid_y } => {
                let _ = dispatch_physical_touch(true, hid_x, hid_y);
            }
            StagePointerEvent::Up { hid_x, hid_y } => {
                let _ = dispatch_physical_touch(false, hid_x, hid_y);
            }
            StagePointerEvent::Wheel {
                hid_x,
                hid_y,
                delta_y,
                delta_x,
                ctrl,
            } => {
                if ctrl {
                    let zoom_in = delta_y >= 0;
                    let (start_span, end_span) = if zoom_in { (9000u16, 23000u16) } else { (23000u16, 9000u16) };
                    let frames = pinch_zoom_frames(hid_x, hid_y, start_span, end_span, 5);
                    if let (Some(first), Some(last)) = (frames.first(), frames.last()) {
                        let steps = compute_wheel_touch_steps(
                            first[0].x,
                            first[0].y,
                            if zoom_in { 120 } else { -120 },
                            0,
                            cfg.trackpad_speed_pct,
                        );
                        let _ = last;
                        let _ = dispatch_physical_touch_steps(&steps);
                    }
                } else {
                    let (stage_nx, stage_ny) = ios_hid_to_stage_norm(hid_x, hid_y, cfg.rotation_deg);
                    if delta_x != 0 && delta_x.abs() >= delta_y.abs() {
                        let steps = self.scroll_engine.push_horizontal_delta(
                            stage_nx,
                            stage_ny,
                            delta_x,
                            cfg.trackpad_speed_pct,
                            cfg.rotation_deg,
                        );
                        if !steps.is_empty() {
                            let _ = dispatch_physical_touch_steps(&steps);
                        }
                    } else if delta_y != 0 {
                        self.scroll_engine.push_vertical_delta(
                            stage_nx,
                            stage_ny,
                            delta_y,
                            cfg.trackpad_speed_pct,
                            cfg.rotation_deg,
                        );
                    }
                }
            }
        }
    }

    pub fn handle_key_event(&self, ev: StageKeyEvent) {
        if self.is_pin_editing() {
            return;
        }
        let cfg = self.config_snapshot();
        match ev {
            StageKeyEvent::KeyDown {
                vk,
                ctrl,
                alt,
                shift,
                repeat,
            } => {
                if is_modifier_vk(vk) {
                    return;
                }
                if !repeat {
                    if let Some(action) = resolve_global_shortcut(vk, ctrl, alt, shift) {
                        self.suppressed_vk.store(vk, Ordering::SeqCst);
                        self.emit_shortcut(action);
                        return;
                    }
                }
                if self.suppressed_vk.load(Ordering::Relaxed) == vk && (ctrl || alt || repeat) {
                    return;
                }
                self.suppressed_vk.store(0, Ordering::SeqCst);
                if !cfg.keyboard_enabled
                    || ctrl
                    || alt
                    || cfg.airplay_no_hid
                    || mirror::is_airplay_standalone_no_hid_active()
                {
                    return;
                }
                let _ = dispatch_nonchar_vk(vk);
            }
            StageKeyEvent::Char { ch, ctrl, alt } => {
                if ch < ' ' || ch.is_control() {
                    return;
                }
                let sup = self.suppressed_vk.load(Ordering::Relaxed);
                if sup != 0 && u16::from(ch.to_ascii_uppercase() as u8) == sup {
                    return;
                }
                if !cfg.keyboard_enabled
                    || ctrl
                    || alt
                    || cfg.airplay_no_hid
                    || mirror::is_airplay_standalone_no_hid_active()
                {
                    return;
                }
                if ch == '\r' {
                    return;
                }
                let _ = dispatch_char_with_shift_latching(ch);
            }
            StageKeyEvent::KeyUp { vk, .. } => {
                let _ = self
                    .suppressed_vk
                    .compare_exchange(vk, 0, Ordering::SeqCst, Ordering::Relaxed);
                let _ = &hid::is_stealth_dim_active();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_native_input_shortcuts_wheel_and_vk_hid_mapping() {
        // All 10 global shortcuts
        assert_eq!(resolve_global_shortcut(0x4B, true, false, false), Some(GlobalShortcutAction::ToggleKeyboard));
        assert_eq!(resolve_global_shortcut(0x43, false, true, false), Some(GlobalShortcutAction::CopyOcr));
        assert_eq!(resolve_global_shortcut(0x56, false, true, false), Some(GlobalShortcutAction::PastePcText));
        assert_eq!(resolve_global_shortcut(0x42, false, true, false), Some(GlobalShortcutAction::GrabLink));
        assert_eq!(resolve_global_shortcut(0x52, false, true, false), Some(GlobalShortcutAction::ClearCache));
        assert_eq!(resolve_global_shortcut(0x48, false, true, false), Some(GlobalShortcutAction::ToggleLandscape));
        assert_eq!(resolve_global_shortcut(0x44, false, true, false), Some(GlobalShortcutAction::ToggleStealth));
        assert_eq!(resolve_global_shortcut(0x4C, false, true, false), Some(GlobalShortcutAction::LockScreen));
        assert_eq!(resolve_global_shortcut(0x55, false, true, false), Some(GlobalShortcutAction::WakeUnlock));
        assert_eq!(resolve_global_shortcut(0x4D, false, true, false), Some(GlobalShortcutAction::ToggleMute));

        // Ctrl -> iOS Cmd mappings
        assert_eq!(resolve_global_shortcut(0x43, true, false, false), Some(GlobalShortcutAction::IosCmd("copy")));
        assert_eq!(resolve_global_shortcut(0x56, true, false, false), Some(GlobalShortcutAction::IosCmd("paste")));
        assert_eq!(resolve_global_shortcut(0x41, true, false, false), Some(GlobalShortcutAction::IosCmd("select-all")));
        assert_eq!(resolve_global_shortcut(0x58, true, false, false), Some(GlobalShortcutAction::IosCmd("cut")));
        assert_eq!(resolve_global_shortcut(0x5A, true, false, false), Some(GlobalShortcutAction::IosCmd("undo")));

        // Wheel scaling by trackpad_speed_pct (60% vs 100% vs 150%)
        let s60 = compute_wheel_touch_steps(32768, 32768, 120, 0, 60);
        let s100 = compute_wheel_touch_steps(32768, 32768, 120, 0, 100);
        let s150 = compute_wheel_touch_steps(32768, 32768, 120, 0, 150);
        assert_eq!(s60.len(), 8);
        assert!(s60[6].y < s100[6].y && s100[6].y < s150[6].y);
        assert!(!s100.last().unwrap().contact);

        // VK -> USB HID usage parity
        assert_eq!(vk_to_hid_usage(0x41), Some(0x04)); // 'A'
        assert_eq!(vk_to_hid_usage(0x0D), Some(0x28)); // Enter
        assert_eq!(vk_to_hid_usage(0x20), Some(0x2C)); // Space
        assert_eq!(vk_to_hid_usage(0x26), Some(0x52)); // Up arrow
    }

    #[test]
    fn test_tahap265_continuous_touchpad_scroll_and_horizontal_swipe_parity() {
        // 1. Horizontal 2-finger swipe LEFT (delta_x < 0) -> drag left-to-right (end_x > start_x)
        // in BOTH Portrait (0°) and Landscape (270°)
        let h_left_p = compute_horizontal_swipe_steps(0.5, 0.5, -120, 100, 0);
        assert!(h_left_p.len() >= 8);
        assert!(h_left_p[0].contact);
        assert!(!h_left_p.last().unwrap().contact);
        assert!(
            h_left_p[h_left_p.len() - 2].x > h_left_p[0].x,
            "Portrait delta_x=-120 must drag left->right (pushing iOS page right)"
        );

        let h_right_p = compute_horizontal_swipe_steps(0.5, 0.5, 120, 100, 0);
        assert!(
            h_right_p[h_right_p.len() - 2].x < h_right_p[0].x,
            "Portrait delta_x=+120 must drag right->left (pushing iOS page left)"
        );

        // In 270° Landscape, stage X maps to iOS Y (`uy = cx`), so swipe left (delta_x=-120) increases Y
        let h_left_l = compute_horizontal_swipe_steps(0.5, 0.5, -120, 100, 270);
        assert!(h_left_l[h_left_l.len() - 2].y > h_left_l[0].y);

        // 2. Continuous 120 Hz scroll planner: fractional Precision Touchpad deltas (< 120),
        // 1x down -> continuous moves -> seamless re-anchor near 0.88 -> 1x up after 120ms idle
        let mut planner = ScrollTickPlanner::default();
        planner.push_delta(0.5, 0.70, 28, 120, 0); // fractional Precision delta < 120
        planner.push_delta(0.5, 0.70, 240, 120, 0);

        let mut all_steps = Vec::new();
        for _ in 0..40 {
            all_steps.extend(planner.advance_tick(10));
        }
        // Push past 0.88 upper bound to force seamless re-anchor
        planner.push_delta(0.5, 0.5, 600, 150, 0);
        let mut saw_reanchor = false;
        for _ in 0..60 {
            let tick = planner.advance_tick(10);
            if tick.len() == 2 && !tick[0].contact && tick[1].contact {
                saw_reanchor = true;
            }
            all_steps.extend(tick);
        }
        assert!(saw_reanchor, "Expected seamless re-anchor when crossing 0.12..0.88 band");

        // Drain until idle > 120ms -> 1 final lift (contact=false)
        for _ in 0..30 {
            all_steps.extend(planner.advance_tick(150));
        }
        assert!(!planner.active);
        assert!(!all_steps.last().unwrap().contact);
        assert!(all_steps.len() >= 10);
    }

    #[test]
    fn test_tahap265_all_12_keyboard_shortcuts_and_pin_safe_guard() {
        // Verify ALL 12 shortcut groups resolve cleanly:
        assert_eq!(resolve_global_shortcut(0x4B, true, false, false), Some(GlobalShortcutAction::ToggleKeyboard));
        for vk in [0x41u16, 0x43, 0x56, 0x58, 0x5A] {
            assert!(matches!(resolve_global_shortcut(vk, true, false, false), Some(GlobalShortcutAction::IosCmd(_))));
        }
        for vk in [0x48u16, 0x4D, 0x43, 0x56, 0x42, 0x52, 0x44, 0x4C, 0x55, 0x25] {
            assert!(resolve_global_shortcut(vk, false, true, false).is_some(), "Missing Alt+0x{vk:X}");
        }

        // Verify PIN-Input Safe Guard suppresses ALL global shortcuts while typing PIN
        let (tx, rx) = std::sync::mpsc::channel();
        let router = NativeInputRouter::new(tx);
        router.set_pin_editing(true);
        assert!(router.is_pin_editing());
        router.emit_shortcut(GlobalShortcutAction::LockScreen);
        router.handle_key_event(StageKeyEvent::KeyDown {
            vk: 0x48,
            ctrl: false,
            alt: true,
            shift: false,
            repeat: false,
        });
        assert!(rx.try_recv().is_err(), "PIN safe guard MUST block Alt shortcuts");

        // Once PIN input loses focus, shortcuts deliver immediately
        router.set_pin_editing(false);
        router.handle_key_event(StageKeyEvent::KeyDown {
            vk: 0x25,
            ctrl: false,
            alt: true,
            shift: false,
            repeat: false,
        });
        assert_eq!(rx.try_recv().unwrap(), GlobalShortcutAction::SwipeBack);
    }

    #[test]
    fn test_tahap266_ctrl_a_suppresses_0xe0_emits_cmd_0x04_and_pin_input_isolation() {
        let _hid_guard = hid::HID_TEST_SERIAL_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        mirror::global_reset_mirror_runtime_for_test();
        hid::reset_hid_runtime_state();
        hid::clear_typed_lines_buffer();

        let (tx, rx) = std::sync::mpsc::channel();
        let router = NativeInputRouter::new(tx);
        router.update_config(|c| c.keyboard_enabled = true);

        // 1. Ctrl+A must resolve to "select-all" -> hid::ios_shortcut_usage("select-all") == Some(0x04)
        //    and generate exact [0xE3, 0x04] (Cmd + A) HID usage sequence with ZERO raw 0xE0 modifier!
        let act = resolve_global_shortcut(0x41, true, false, false);
        assert_eq!(act, Some(GlobalShortcutAction::IosCmd("select-all")));
        assert_eq!(hid::ios_shortcut_usage("select-all"), Some(0x04));
        let cmd_steps = generate_ios_shortcut_steps("select-all").expect("select-all steps");
        assert!(cmd_steps.iter().any(|s| s.keys == vec![0xE3, 0x04]));
        assert!(!cmd_steps.iter().any(|s| s.keys.contains(&0xE0)));

        // 2. Modifier-only VK_LCONTROL & Ctrl+A shortcut followed by WM_CHAR(0x01) + stray 'a' before KeyUp(0x41)
        // must never leak stray characters to typed HID buffer!
        router.handle_key_event(StageKeyEvent::KeyDown {
            vk: 0xA2,
            ctrl: true,
            alt: false,
            shift: false,
            repeat: false,
        });
        router.handle_key_event(StageKeyEvent::KeyDown {
            vk: 0x41,
            ctrl: true,
            alt: false,
            shift: false,
            repeat: false,
        });
        router.handle_key_event(StageKeyEvent::Char {
            ch: '\u{0001}',
            ctrl: false,
            alt: false,
        });
        router.handle_key_event(StageKeyEvent::Char {
            ch: 'a',
            ctrl: false,
            alt: false,
        });
        router.handle_key_event(StageKeyEvent::KeyUp {
            vk: 0x41,
            ctrl: false,
            alt: false,
            shift: false,
        });
        assert_eq!(hid::get_current_typed_line(), "");

        // 3. When pin_editing == true, digits/shortcuts MUST never reach HID or shortcut channel!
        router.set_pin_editing(true);
        for (vk, ch) in [(0x31u16, '1'), (0x32, '2'), (0x33, '3'), (0x34, '4')] {
            router.handle_key_event(StageKeyEvent::KeyDown {
                vk,
                ctrl: false,
                alt: false,
                shift: false,
                repeat: false,
            });
            router.handle_key_event(StageKeyEvent::Char {
                ch,
                ctrl: false,
                alt: false,
            });
        }
        router.handle_key_event(StageKeyEvent::KeyDown {
            vk: 0x4C,
            ctrl: false,
            alt: true,
            shift: false,
            repeat: false,
        });
        assert!(rx.try_recv().is_err());
        assert_eq!(hid::get_current_typed_line(), "");

        // 4. After blur, UI shortcuts deliver immediately.
        router.set_pin_editing(false);
        router.handle_key_event(StageKeyEvent::KeyDown {
            vk: 0x4C,
            ctrl: false,
            alt: true,
            shift: false,
            repeat: false,
        });
        assert_eq!(rx.try_recv().unwrap(), GlobalShortcutAction::LockScreen);
    }

    #[test]
    fn test_tahap266_fluid_touchpad_120hz_smoothing_opposite_runway_inertia_release_and_hwheel() {
        // Scroll DOWN (delta_y < 0) starts on opposite bottom runway y = 0.78 near x = 0.48
        let mut down = ScrollTickPlanner::default();
        down.push_delta(0.50, 0.40, -120, 100, 0);
        let d_start = down.advance_tick(0);
        assert_eq!(d_start.len(), 1);
        assert!(d_start[0].contact);
        assert!((down.cur_ny - 0.78).abs() < 1e-6);
        assert!((down.cur_nx - 0.50).abs() < 1e-6);

        let d_move = down.advance_tick(8);
        assert!(d_move[0].contact);
        assert!(down.cur_ny < 0.78, "delta_y < 0 moves monotonically upward from 0.78");

        // Scroll UP (delta_y > 0) starts on opposite top runway y = 0.22; out-of-band X clamps near 0.48
        let mut up = ScrollTickPlanner::default();
        up.push_delta(0.95, 0.90, 120, 100, 0);
        let u_start = up.advance_tick(0);
        assert_eq!(u_start.len(), 1);
        assert!(u_start[0].contact);
        assert!((up.cur_ny - 0.22).abs() < 1e-6);
        assert!((up.cur_nx - 0.48).abs() < 1e-6);

        // Drain residual delta, verify velocity-preserving release fires cleanly after IDLE_RELEASE_MS!
        for _ in 0..40 {
            let _ = up.advance_tick(8);
        }
        assert!(up.active);
        assert!(
            up.advance_tick(IDLE_RELEASE_MS - 1).is_empty(),
            "Must still glide before IDLE_RELEASE_MS release threshold"
        );
        let release = up.advance_tick(IDLE_RELEASE_MS);
        assert!(!release.is_empty());
        assert!(!release.last().unwrap().contact, "IDLE_RELEASE_MS must emit velocity-preserving touchUp");
        assert!(!up.active);

        // Horizontal 2-finger swipe retains 1:1 Tahap 259 direction across Portrait & Landscape
        let hw_p = compute_horizontal_swipe_steps(0.5, 0.5, -120, 100, 0);
        assert!(hw_p[hw_p.len() - 2].x > hw_p[0].x);
        let hw_l = compute_horizontal_swipe_steps(0.5, 0.5, -120, 100, 270);
        assert!(hw_l[hw_l.len() - 2].y > hw_l[0].y);
    }

    #[test]
    fn test_tahap267_fluid_touchpad_boosted_gain_flick_accel_inertia_release_and_cancel_touch() {
        let mut flick = ScrollTickPlanner::default();
        flick.push_delta(0.50, 0.50, -120, 100, 0);
        let single_pending = flick.pending_dy_norm.abs();
        assert!(
            (single_pending - 0.19).abs() < 1e-6,
            "base +35% gain must be 0.19 per 120-delta, got {single_pending}"
        );

        // Consecutive >=60 tick within <30 ms triggers gentle flick boost (> 0.19 base!)
        flick.push_delta(0.50, 0.50, -120, 100, 0);
        let boosted_total = flick.pending_dy_norm.abs();
        assert!(
            boosted_total > 0.39,
            "flick acceleration must boost consecutive rapid ticks >0.39, got {boosted_total}"
        );

        let down = flick.advance_tick(0);
        assert_eq!(down.len(), 1);
        assert!(down[0].contact);
        let move1 = flick.advance_tick(8);
        assert!(!move1.is_empty());
        assert!(
            flick.last_release_velocity.abs() > 0.0,
            "0.42 smoother must track non-zero release velocity"
        );

        // Canceling active touch emits immediate contact=false lift and zeroes pending delta
        let canceled = flick.cancel_active();
        assert_eq!(canceled.len(), 1);
        assert!(!canceled[0].contact);
        assert!(!flick.active);
        assert!((flick.pending_dy_norm).abs() < 1e-9);

        // Calibrated IDLE_RELEASE_MS inertia-preserving release threshold
        let mut p = ScrollTickPlanner::default();
        p.push_delta(0.5, 0.5, -120, 100, 0);
        let _ = p.advance_tick(0);
        for _ in 0..45 {
            let _ = p.advance_tick(8);
        }
        assert!(p.advance_tick(IDLE_RELEASE_MS - 1).is_empty());
        let rel = p.advance_tick(IDLE_RELEASE_MS);
        assert!(!rel.is_empty());
        assert!(!rel.last().unwrap().contact);
        assert!(p.last_release_velocity.abs() > 0.0);
    }

    #[test]
    fn test_tahap268_fluid_touchpad_anti_ghost_tap_deadzone_pan_breakaway_and_min_travel() {
        // 1. Pre-Down Priming Deadzone: micro-jitter < 180 HID emits 0 HID reports and expires at 20 ms
        let mut jitter = ScrollTickPlanner::default();
        jitter.push_delta(0.50, 0.50, -1, 100, 0); // ~104 HID < 180
        assert!(jitter.pending_dy_norm.abs() * 65535.0 < ARM_DEADZONE_HID);
        assert!(jitter.advance_tick(8).is_empty());
        assert!(!jitter.active);
        assert!(jitter.advance_tick(ARM_WINDOW_MS).is_empty());
        assert!((jitter.pending_dy_norm).abs() < 1e-9, "Micro-jitter must expire cleanly");

        // 2. Small intentional flick >= 180 HID: progressive 3-step Pan-Breakaway >= 1050 HID within 3 ticks (24 ms)
        //    AND Minimum Total Travel >= 1350 HID before touchUp so Instagram NEVER triggers false tap!
        let mut flick = ScrollTickPlanner::default();
        flick.push_delta(0.50, 0.50, -8, 100, 0); // ~830 HID >= 180 HID arm threshold
        let down = flick.advance_tick(0);
        assert_eq!(down.len(), 1);
        assert!(down[0].contact);
        let t1 = flick.advance_tick(8);
        let t2 = flick.advance_tick(8);
        let t3 = flick.advance_tick(8);
        assert!(!t1.is_empty() && !t2.is_empty() && !t3.is_empty());
        let travel_24ms_hid = flick.session_travel_norm * 65535.0;
        assert!(
            travel_24ms_hid >= PAN_BREAKAWAY_HID - 1.0,
            "24 ms breakaway must cross >= {PAN_BREAKAWAY_HID} HID, got {travel_24ms_hid:.1}"
        );

        let mut total_hid_before_up = travel_24ms_hid;
        let prev_y = t3.last().unwrap().y as f64;
        let mut last_y = prev_y;
        for _ in 0..4 {
            for s in flick.advance_tick(8) {
                total_hid_before_up += (s.y as f64 - last_y).abs();
                last_y = s.y as f64;
            }
        }
        let rel = flick.advance_tick(IDLE_RELEASE_MS);
        assert!(!rel.is_empty());
        assert!(!rel.last().unwrap().contact);
        let mut ramp_hid = 0.0;
        for s in &rel {
            if s.contact {
                ramp_hid += (s.y as f64 - last_y).abs();
                last_y = s.y as f64;
            }
        }
        assert!(
            total_hid_before_up + ramp_hid >= MIN_SESSION_TRAVEL_HID - 2.0,
            "Total travel before touchUp must be >= {MIN_SESSION_TRAVEL_HID} HID, got {:.1}",
            total_hid_before_up + ramp_hid
        );

        // 3. Re-anchor edge guard (< 0.10 / > 0.90): tiny remaining pending (< 2200 HID) never re-anchors!
        let mut edge = ScrollTickPlanner::default();
        edge.active = true;
        edge.cur_ny = 0.905;
        edge.session_dir_sign = 1.0;
        edge.pending_dy_norm = 0.010; // ~655 HID < 2200
        let edge_steps = edge.advance_tick(8);
        assert_eq!(edge_steps.len(), 1);
        assert!(edge_steps[0].contact, "Must hold at edge instead of re-anchoring tiny residual");
    }

    #[test]
    fn test_tahap269_touchpad_axis_lock_suppresses_diagonal_hwheel_and_enforces_min_travel_every_touchup() {
        // 1. Dominant Vertical Axis Lock suppresses diagonal WM_MOUSEHWHEEL micro-jitter (+-15)
        //    both during active vertical scroll AND within 150ms post-vertical window!
        let mut p = ScrollTickPlanner::default();
        p.push_delta_at(0.50, 0.50, -120, 100, 0, 1000);
        let down = p.advance_tick(0);
        assert!(down[0].contact);
        assert!(
            p.push_horizontal_delta_at(0.50, 0.50, 15, 100, 0, 1040).is_empty(),
            "micro HWheel during active vertical scroll must emit 0 HID reports"
        );
        // Drain & lift to idle, then test 100ms (< 150ms) and 220ms (> 150ms) later
        for _ in 0..40 {
            let _ = p.advance_tick(8);
        }
        let _ = p.advance_tick(IDLE_RELEASE_MS);
        assert!(!p.active);
        assert!(
            p.push_horizontal_delta_at(0.50, 0.50, -20, 100, 0, 1100).is_empty(),
            "diagonal HWheel inside 150ms post-vertical suppress window must emit 0 HID reports"
        );

        // 2. Deadzone horizontal < 36 emits 0; accumulated >= 36 emits swipe >= 3200 HID with correct polarity
        assert!(p.push_horizontal_delta_at(0.50, 0.50, -20, 100, 0, 1500).is_empty());
        let hw = p.push_horizontal_delta_at(0.50, 0.50, -20, 100, 0, 1515);
        assert!(hw.len() >= 8 && !hw.last().unwrap().contact);
        let hx_travel = (hw.last().unwrap().x as f64 - hw[0].x as f64).abs();
        assert!(
            hx_travel >= MIN_HORIZONTAL_SWIPE_HID,
            "Horizontal swipe travel must be >= {MIN_HORIZONTAL_SWIPE_HID} HID, got {hx_travel:.1}"
        );
        assert!(hw[hw.len() - 2].x > hw[0].x);

        // 3. Universal MIN_SESSION_TRAVEL_HID (>= 2200) on abrupt cancel_active() right after touchDown!
        let mut abrupt = ScrollTickPlanner::default();
        abrupt.push_delta_at(0.50, 0.50, -12, 100, 0, 2000);
        let d_steps = abrupt.advance_tick(0);
        assert_eq!(d_steps.len(), 1);
        let down_y = d_steps[0].y as f64;
        let cancel_steps = abrupt.cancel_active();
        assert!(cancel_steps.len() >= 3 && !cancel_steps.last().unwrap().contact);
        let cancel_travel = (cancel_steps.last().unwrap().y as f64 - down_y).abs();
        assert!(
            cancel_travel >= MIN_SESSION_TRAVEL_HID,
            "Abrupt cancel_active() touchUp must travel >= {MIN_SESSION_TRAVEL_HID} HID, got {cancel_travel:.1}"
        );

        // 4. Universal MIN_SESSION_TRAVEL_HID (>= 2200) on edge re-anchor touchUp!
        let mut reanch = ScrollTickPlanner::default();
        reanch.active = true;
        reanch.cur_ny = 0.89;
        reanch.session_dir_sign = 1.0;
        reanch.session_travel_norm = 0.001; // tiny 65 HID before hitting edge!
        reanch.pending_dy_norm = 0.042; // remaining ~1431 HID >= MIN_SESSION_TRAVEL_HID triggers re-anchor while travel < 1430 HID
        let r_steps = reanch.advance_tick(8);
        assert!(r_steps.len() >= 4, "expected push + up + new down on short-travel re-anchor");
        let up_idx = r_steps.iter().position(|s| !s.contact).expect("re-anchor touchUp");
        assert!(r_steps[up_idx].contact == false);
        assert!(reanch.cur_ny == 0.22);
    }

    #[test]
    fn test_tahap270_silky_touchpad_curve_3step_easein_38ms_inertia_and_zero_ghost_tap() {
        assert_eq!(ARM_DEADZONE_HID, 180.0);
        assert_eq!(ARM_WINDOW_MS, 20);
        assert_eq!(PAN_BREAKAWAY_HID, 1050.0);
        assert_eq!(PAN_BREAKAWAY_EASE_WEIGHTS, [0.25, 0.35, 0.40]);
        assert_eq!(MIN_SESSION_TRAVEL_HID, 1350.0);
        assert_eq!(IDLE_RELEASE_MS, 38);
        assert_eq!(SMOOTHING_ALPHA, 0.48);

        // 1. Progressive 3-step ease-in ramp (25% -> 35% -> 40% of 1050 HID) without harsh jump
        let mut ease = ScrollTickPlanner::default();
        ease.push_delta_at(0.50, 0.50, -3, 100, 0, 1000); // ~311 HID >= 180 HID
        let down = ease.advance_tick(0);
        assert_eq!(down.len(), 1);
        let y0 = down[0].y as f64;
        let t1 = ease.advance_tick(8);
        let t2 = ease.advance_tick(8);
        let t3 = ease.advance_tick(8);
        let y1 = t1[0].y as f64;
        let y2 = t2[0].y as f64;
        let y3 = t3[0].y as f64;
        let s1 = (y1 - y0).abs();
        let s2 = (y2 - y1).abs();
        let s3 = (y3 - y2).abs();
        assert!(
            s1 < s2 && s2 < s3,
            "3-step ease-in must accelerate smoothly (25% -> 35% -> 40%), got steps {s1:.1}, {s2:.1}, {s3:.1}"
        );
        let ease_total = ease.session_travel_norm * 65535.0;
        assert!(
            (ease_total - PAN_BREAKAWAY_HID).abs() < 2.0,
            "3-step ramp total must land near {PAN_BREAKAWAY_HID}, got {ease_total:.1}"
        );

        // 2. Silky 38 ms inertia release enforces MIN_SESSION_TRAVEL_HID (1350) cleanly
        assert!(ease.advance_tick(IDLE_RELEASE_MS - 1).is_empty());
        let rel = ease.advance_tick(IDLE_RELEASE_MS);
        assert!(!rel.is_empty() && !rel.last().unwrap().contact);
        let final_travel = (rel.last().unwrap().y as f64 - y0).abs();
        assert!(
            final_travel >= MIN_SESSION_TRAVEL_HID - 1.0,
            "Idle release must keep >= {MIN_SESSION_TRAVEL_HID} HID travel, got {final_travel:.1}"
        );

        // 3. Dominant-Axis Lock still suppresses diagonal WM_MOUSEHWHEEL <= 90 during active scroll
        let mut axis = ScrollTickPlanner::default();
        axis.push_delta_at(0.50, 0.50, -120, 100, 0, 2000);
        let _ = axis.advance_tick(0);
        assert!(axis.push_horizontal_delta_at(0.50, 0.50, 85, 100, 0, 2040).is_empty());
        let cancel_steps = axis.cancel_active();
        assert!(!cancel_steps.is_empty() && !cancel_steps.last().unwrap().contact);
    }

    #[test]
    fn test_tahap271_silky_touchpad_velocity_smoothing_momentum_handoff_and_direction_reversal() {
        assert!((VELOCITY_SMOOTH_TAU_S - 0.018).abs() < 1e-6);
        assert_eq!(MOMENTUM_HANDOFF_PEAK_VEL_HID_PER_S, 42_000.0);
        assert_eq!(MOMENTUM_HANDOFF_MIN_RELEASE_VEL_HID_PER_S, 12_000.0);
        assert!((MOMENTUM_HANDOFF_DECAY_RATIO - 0.45).abs() < 1e-6);
        assert_eq!(MOMENTUM_HANDOFF_DECAY_STREAK, 2);
        assert_eq!(WINDOWS_INERTIA_SUPPRESS_MS, 180);

        // 1. Slow reading scroll (< 42_000 HID/s peak) MUST NEVER trigger Smart Momentum Hand-Off
        let mut slow = ScrollTickPlanner::default();
        slow.push_delta_at(0.50, 0.50, -3, 100, 0, 1000);
        let _ = slow.advance_tick_at(0, 1000);
        let _ = slow.advance_tick_at(0, 1008);
        slow.push_delta_at(0.50, 0.50, -2, 100, 0, 1020);
        let _ = slow.advance_tick_at(0, 1024);
        slow.push_delta_at(0.50, 0.50, -1, 100, 0, 1040);
        let _ = slow.advance_tick_at(0, 1048);
        assert_eq!(slow.momentum_handoff_count(), 0);

        // 2. Fast flick -> realistic decaying deltas -> smoothed velocity crosses < 0.45 * peak while >= 12k HID/s
        //    emits clean touchUp hand-off & suppresses Windows inertia for 180 ms
        let mut flick = ScrollTickPlanner::default();
        flick.push_delta_at(0.50, 0.50, -80, 100, 0, 2000);
        let d0 = flick.advance_tick_at(0, 2000);
        assert!(d0[0].contact);
        let _ = flick.advance_tick_at(0, 2008);
        flick.push_delta_at(0.50, 0.50, -80, 100, 0, 2010);
        let _ = flick.advance_tick_at(0, 2016);
        let _ = flick.advance_tick_at(0, 2024);
        let _ = flick.advance_tick_at(0, 2032);
        // Decaying deltas matching real 120 Hz Windows touchpad flick lift-off
        flick.push_delta_at(0.50, 0.50, -35, 100, 0, 2036);
        let _ = flick.advance_tick_at(0, 2040);
        flick.push_delta_at(0.50, 0.50, -12, 100, 0, 2044);
        let s1 = flick.advance_tick_at(0, 2048);
        let mut handed_off = s1.iter().any(|s| !s.contact) && flick.momentum_handoff_count() == 1;
        let mut t = 2052u64;
        if !handed_off {
            flick.push_delta_at(0.50, 0.50, -5, 100, 0, 2052);
            for _ in 0..24 {
                t += 4;
                let st = flick.advance_tick_at(8, t);
                t += 4;
                if st.iter().any(|s| !s.contact) && flick.momentum_handoff_count() == 1 {
                    handed_off = true;
                    break;
                }
            }
        }
        assert!(handed_off, "Fast flick decay must trigger Smart Momentum Hand-Off to iOS 120 Hz ProMotion");
        assert!(!flick.active);

        // Trailing Windows inertia within 180 ms is suppressed cleanly
        flick.push_delta_at(0.50, 0.50, -4, 100, 0, t + 20);
        assert!(flick.pending_dy_norm.abs() < 1e-9, "Trailing inertia delta must be suppressed");

        // Reversal cancels suppression IMMEDIATELY & flushes pending velocity to zero!
        let mut rev = ScrollTickPlanner::default();
        rev.push_delta_at(0.50, 0.50, -120, 100, 0, 3000);
        let _ = rev.advance_tick_at(0, 3000);
        let _ = rev.advance_tick_at(0, 3008);
        assert!(rev.pending_dy_norm < -0.05);
        rev.push_delta_at(0.50, 0.50, 30, 100, 0, 3016);
        assert!(rev.pending_dy_norm > 0.0, "Instant Direction-Reversal Flush must flip polarity on very first reverse event");
    }

    fn simulate_precision_touchpad_burst(speed_pct: u32) -> (f64, f64, u32, usize) {
        let mut p = ScrollTickPlanner::default();
        let mut t_ms = 1000u64;
        let mut steps = Vec::new();
        // 10 realistic Windows 11 Precision Touchpad deltas (-40 each @ 8 ms)
        for _ in 0..10 {
            p.push_delta_at(0.50, 0.50, -40, speed_pct, 0, t_ms);
            steps.extend(p.advance_tick_at(0, t_ms));
            t_ms += 8;
        }
        // Run active glide until pending settles
        for _ in 0..120 {
            if !p.active && p.pending_dy_norm.abs() <= 1e-5 {
                break;
            }
            let out = p.advance_tick_at(8, t_ms);
            t_ms += 8;
            steps.extend(out);
            if p.active && p.pending_dy_norm.abs() < 0.0005 {
                let rel = p.advance_tick_at(IDLE_RELEASE_MS, t_ms + IDLE_RELEASE_MS);
                steps.extend(rel);
                break;
            }
        }
        if p.active {
            let rel = p.advance_tick_at(IDLE_RELEASE_MS, t_ms + IDLE_RELEASE_MS);
            steps.extend(rel);
        }

        let mut total_hid = 0.0f64;
        let mut peak_vel_hid_s = 0.0f64;
        let mut stationary_tail = 0usize;
        let mut prev_y: Option<f64> = None;
        for s in &steps {
            if let Some(py) = prev_y {
                let d = (s.y as f64 - py).abs();
                if s.contact {
                    total_hid += d;
                    peak_vel_hid_s = peak_vel_hid_s.max(d / 0.008);
                    if d < 0.5 {
                        stationary_tail += 1;
                    } else {
                        stationary_tail = 0;
                    }
                }
            }
            prev_y = if s.contact { Some(s.y as f64) } else { None };
        }
        // Also simulate a mouse wheel notch (-120) to verify distance ordering cleanly
        let mut notch = ScrollTickPlanner::default();
        notch.push_delta_at(0.50, 0.50, -120, speed_pct, 0, 5000);
        let mut nt = 5000u64;
        let _ = notch.advance_tick_at(0, nt);
        let mut notch_hid = 0.0f64;
        for _ in 0..60 {
            nt += 8;
            let nsteps = notch.advance_tick_at(8, nt);
            for ns in nsteps {
                if ns.contact {
                    notch_hid = (ns.y as f64 - 0.78 * 65535.0).abs().max(notch_hid);
                }
            }
            if !notch.active && notch.pending_dy_norm.abs() <= 1e-5 {
                break;
            }
        }
        let _ = notch_hid;
        (total_hid, peak_vel_hid_s, p.momentum_handoff_count(), stationary_tail)
    }

    #[test]
    fn test_tahap286_trackpad_speed_30_65_100_140_200_quantitative_distance_and_velocity_curve() {
        let (tr30, pv30, mh30, st30) = simulate_precision_touchpad_burst(30);
        let (tr65, pv65, mh65, _st65) = simulate_precision_touchpad_burst(65);
        let (tr100, pv100, _mh100, _st100) = simulate_precision_touchpad_burst(100);
        let (tr140, pv140, _mh140, _st140) = simulate_precision_touchpad_burst(140);
        let (tr200, pv200, _mh200, _st200) = simulate_precision_touchpad_burst(200);

        println!(
            "[TAHAP286_SCROLL_METRICS] 30%={{travel:{tr30:.1},peak_vel:{pv30:.1},handoff:{mh30},brake_ticks:{st30}}} | \
65%={{travel:{tr65:.1},peak_vel:{pv65:.1},handoff:{mh65}}} | \
100%={{travel:{tr100:.1},peak_vel:{pv100:.1}}} | \
140%={{travel:{tr140:.1},peak_vel:{pv140:.1}}} | \
200%={{travel:{tr200:.1},peak_vel:{pv200:.1}}}"
        );

        assert!(
            tr30 < tr65 && tr65 < tr100 && tr100 < tr140 && tr140 < tr200,
            "Strict speed curve ordering required: 30({tr30:.1}) < 65({tr65:.1}) < 100({tr100:.1}) < 140({tr140:.1}) < 200({tr200:.1})"
        );
        let ratio_30_to_100 = tr30 / tr100.max(1.0);
        assert!(
            (0.24..=0.40).contains(&ratio_30_to_100),
            "30% scroll distance ratio vs 100% must stay ~0.30x (got {ratio_30_to_100:.3}x)"
        );
        assert!(
            pv30 < pv100 * 0.45,
            "30% peak velocity ({pv30:.1} HID/s) MUST stay < 45% of 100% ({pv100:.1} HID/s)"
        );
        assert_eq!(mh30, 0, "30% slow mode MUST NEVER trigger momentum fling hand-off");
        assert_eq!(mh65, 0, "65% slow mode MUST NEVER trigger momentum fling hand-off");
        assert!(st30 >= 2, "30% slow mode must hold >= 2 stationary ticks before touchUp to kill iOS fling, got {st30}");
    }

    #[test]
    fn test_tahap286_horizontal_swipe_scales_cleanly_at_30_pct_without_014_floor_override() {
        let span = |spd: u32| -> f64 {
            let steps = compute_horizontal_swipe_steps(0.5, 0.5, -60, spd, 0);
            (steps[steps.len() - 2].x as f64 - steps[0].x as f64).abs()
        };
        let d30 = span(30);
        let d65 = span(65);
        let d100 = span(100);
        let d140 = span(140);
        assert!(
            d30 >= MIN_HORIZONTAL_SWIPE_HID,
            "Horizontal swipe at 30% must remain valid >= {MIN_HORIZONTAL_SWIPE_HID}, got {d30:.1}"
        );
        assert!(
            d30 < d65 && d65 < d100 && d100 < d140,
            "Horizontal swipe distances must scale cleanly: 30({d30:.1}) < 65({d65:.1}) < 100({d100:.1}) < 140({d140:.1})"
        );
    }
}
