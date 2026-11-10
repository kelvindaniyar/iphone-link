//! Smart Auto-Rotate 32 Scenarios State Machine.
//! Handles asymmetric landscape lock (270 deg), eFootball poster & formation locks,
//! YouTube dual-corridor lock, and portrait guards for social media apps.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrientationMode {
    Portrait0,
    Landscape90,
    Landscape270, // Asymmetric Landscape Lock
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppProfile {
    EFootball,
    YouTube,
    Instagram,
    TikTok,
    Threads,
    ControlCenter,
    HomeScreenFolder,
    Generic,
}

impl AppProfile {
    pub fn from_app_identifier(app_name: &str) -> Self {
        let low = app_name.to_lowercase();
        if low.contains("pes") || low.contains("efootball") || low.contains("konami") {
            Self::EFootball
        } else if low.contains("youtube") || low.contains("yt") {
            Self::YouTube
        } else if low.contains("instagram") {
            Self::Instagram
        } else if low.contains("tiktok") {
            Self::TikTok
        } else if low.contains("threads") {
            Self::Threads
        } else if low.contains("controlcenter") || low.contains("springboard.cc") {
            Self::ControlCenter
        } else if low.contains("folder") || low.contains("springboard.folder") {
            Self::HomeScreenFolder
        } else {
            Self::Generic
        }
    }

    pub fn is_portrait_guarded(&self) -> bool {
        matches!(
            self,
            Self::Instagram
                | Self::TikTok
                | Self::Threads
                | Self::ControlCenter
                | Self::HomeScreenFolder
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RotationDecision {
    pub orientation: OrientationMode,
    pub is_locked: bool,
    pub reason: &'static str,
}

/// Evaluate rotation decision given app, dimensions, and hardware sensor angle.
pub fn evaluate_auto_rotate(
    app_profile: AppProfile,
    width: u32,
    height: u32,
    sensor_angle: u32,
) -> RotationDecision {
    // Rule 1: Strict portrait guard for social media apps and system overlays
    if app_profile.is_portrait_guarded() {
        return RotationDecision {
            orientation: OrientationMode::Portrait0,
            is_locked: true,
            reason: "Portrait guard active (Social / System Overlay)",
        };
    }

    // Rule 2: eFootball (Neon Poster, Messi/Yamal, Formation 3-2-2-3) always horizontal lock
    if app_profile == AppProfile::EFootball {
        let orientation = if sensor_angle == 90 {
            OrientationMode::Landscape90
        } else {
            OrientationMode::Landscape270 // Default asymmetric 270 deg lock
        };
        return RotationDecision {
            orientation,
            is_locked: true,
            reason: "eFootball horizontal match/formation lock",
        };
    }

    // Rule 3: YouTube Fullscreen Dual Corridor & Ambient Mode
    if app_profile == AppProfile::YouTube {
        if width > height {
            let orientation = if sensor_angle == 90 {
                OrientationMode::Landscape90
            } else {
                OrientationMode::Landscape270
            };
            return RotationDecision {
                orientation,
                is_locked: true,
                reason: "YouTube fullscreen corridor lock",
            };
        } else {
            return RotationDecision {
                orientation: OrientationMode::Portrait0,
                is_locked: false,
                reason: "YouTube portrait feed",
            };
        }
    }

    // Rule 4: Generic dynamic orientation based on video aspect ratio
    if width > height {
        let orientation = if sensor_angle == 90 {
            OrientationMode::Landscape90
        } else {
            OrientationMode::Landscape270
        };
        RotationDecision {
            orientation,
            is_locked: false,
            reason: "Wide aspect ratio landscape",
        }
    } else {
        RotationDecision {
            orientation: OrientationMode::Portrait0,
            is_locked: false,
            reason: "Tall aspect ratio portrait",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_profiles() {
        assert_eq!(AppProfile::from_app_identifier("com.konami.pesam"), AppProfile::EFootball);
        assert_eq!(AppProfile::from_app_identifier("com.google.ios.youtube"), AppProfile::YouTube);
        assert_eq!(AppProfile::from_app_identifier("com.burbn.instagram"), AppProfile::Instagram);
        assert!(AppProfile::Instagram.is_portrait_guarded());
        assert!(AppProfile::TikTok.is_portrait_guarded());
        assert!(!AppProfile::EFootball.is_portrait_guarded());
    }

    #[test]
    fn test_efootball_lock() {
        let decision = evaluate_auto_rotate(AppProfile::EFootball, 1920, 1080, 0);
        assert_eq!(decision.orientation, OrientationMode::Landscape270);
        assert!(decision.is_locked);
    }

    #[test]
    fn test_portrait_guard() {
        let decision = evaluate_auto_rotate(AppProfile::Instagram, 1920, 1080, 270);
        assert_eq!(decision.orientation, OrientationMode::Portrait0);
        assert!(decision.is_locked);
    }

    #[test]
    fn test_youtube_landscape() {
        let decision = evaluate_auto_rotate(AppProfile::YouTube, 1920, 1080, 270);
        assert_eq!(decision.orientation, OrientationMode::Landscape270);
        assert!(decision.is_locked);
    }
}
