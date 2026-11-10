//! Smart Auto-Rotate 32 Scenarios State Machine and Pixel Probe Grid.
//! Ported directly from handset/web/mirror.js (Tahap 77-94).
//! Features zero-allocation grid analysis, eFootball 2027 hard horizontal lock (270 deg),
//! YouTube fullscreen dual clean corridors with Ambient Mode glow, and absolute portrait guards.

pub const GRID_WIDTH: usize = 120;
pub const GRID_HEIGHT: usize = 260;

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
    GoogleTranslate,
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
        } else if low.contains("translate") {
            Self::GoogleTranslate
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
                | Self::GoogleTranslate
                | Self::ControlCenter
                | Self::HomeScreenFolder
        )
    }
}

/// Single pixel data containing RGB, ITU-R BT.601 Luma, and max-min chromatic Saturation.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Pixel {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub luma: u8,
    pub sat: u8,
}

impl Pixel {
    #[inline(always)]
    pub fn new(r: u8, g: u8, b: u8) -> Self {
        let luma = ((r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000) as u8;
        let max_val = r.max(g).max(b);
        let min_val = r.min(g).min(b);
        let sat = max_val - min_val;
        Self { r, g, b, luma, sat }
    }
}

/// Zero-allocation 120 x 260 pixel probe grid.
pub struct ProbeGrid120x260 {
    pub pixels: Vec<Pixel>,
}

impl Default for ProbeGrid120x260 {
    fn default() -> Self {
        Self::new()
    }
}

impl ProbeGrid120x260 {
    pub fn new() -> Self {
        Self {
            pixels: vec![Pixel::default(); GRID_WIDTH * GRID_HEIGHT],
        }
    }

    #[inline(always)]
    pub fn set_pixel(&mut self, x: usize, y: usize, r: u8, g: u8, b: u8) {
        if x < GRID_WIDTH && y < GRID_HEIGHT {
            self.pixels[y * GRID_WIDTH + x] = Pixel::new(r, g, b);
        }
    }

    #[inline(always)]
    pub fn get_pixel(&self, x: usize, y: usize) -> Pixel {
        if x < GRID_WIDTH && y < GRID_HEIGHT {
            self.pixels[y * GRID_WIDTH + x]
        } else {
            Pixel::default()
        }
    }

    pub fn fill_all(&mut self, r: u8, g: u8, b: u8) {
        let p = Pixel::new(r, g, b);
        for px in self.pixels.iter_mut() {
            *px = p;
        }
    }

    pub fn fill_rect(&mut self, x1: usize, y1: usize, x2: usize, y2: usize, r: u8, g: u8, b: u8) {
        let p = Pixel::new(r, g, b);
        let max_x = x2.min(GRID_WIDTH - 1);
        let max_y = y2.min(GRID_HEIGHT - 1);
        for y in y1..=max_y {
            for x in x1..=max_x {
                self.pixels[y * GRID_WIDTH + x] = p;
            }
        }
    }

    /// Average luma and saturation in a rectangular region.
    pub fn region_stats(&self, x1: usize, y1: usize, x2: usize, y2: usize) -> (u32, u32, usize) {
        let mut sum_luma = 0u32;
        let mut sum_sat = 0u32;
        let mut count = 0usize;
        let max_x = x2.min(GRID_WIDTH - 1);
        let max_y = y2.min(GRID_HEIGHT - 1);
        for y in y1..=max_y {
            for x in x1..=max_x {
                let p = self.get_pixel(x, y);
                sum_luma += p.luma as u32;
                sum_sat += p.sat as u32;
                count += 1;
            }
        }
        let avg_luma = if count > 0 { sum_luma / count as u32 } else { 0 };
        let avg_sat = if count > 0 { sum_sat / count as u32 } else { 0 };
        (avg_luma, avg_sat, count)
    }
}

/// Result of detailed frame analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameAnalysisResult {
    pub is_landscape_game_frame: bool,
    pub is_efootball_neon_poster: bool,
    pub is_efootball_formation: bool,
    pub is_landscape_menu: bool,
    pub is_youtube_fullscreen: bool,
    pub has_youtube_landscape_osd: bool,
    pub has_notch_colored_clock_pill: bool,
    pub is_tiktok_comment_sheet: bool,
    pub is_tiktok_splash_screen: bool,
    pub is_instagram_reels_dark_chrome: bool,
    pub is_google_translate_dark: bool,
    pub is_control_center_view: bool,
    pub is_ios_folder_view: bool,
    pub has_ios_home_screen_dock: bool,
    pub strict_portrait_exit_proof: bool,
    pub suggested_orientation: OrientationMode,
}

/// Analyze frame orientation from ProbeGrid120x260 matching mirror.js logic.
pub fn analyze_frame_grid(grid: &ProbeGrid120x260, is_currently_landscape: bool) -> FrameAnalysisResult {
    // 1. eFootball Green Pitch Detection (y: 45..195, x: 22..96)
    let mut pitch_green_count = 0usize;
    for y in (45..=195).step_by(4) {
        for x in (22..=96).step_by(4) {
            let p = grid.get_pixel(x, y);
            if p.g >= 30 && p.g <= 155 && (p.g as i32 - p.r as i32) >= 10 && (p.g as i32 - p.b as i32) >= 12 {
                pitch_green_count += 1;
            }
        }
    }

    // 2. eFootball Formation Blue Back Button (y: 11..26, x: 4..24)
    let mut blue_back_btn_count = 0usize;
    for y in 11..=26 {
        for x in 4..=24 {
            let p = grid.get_pixel(x, y);
            if p.b >= 100 && (p.b as i32 - p.r as i32) >= 30 && (p.b as i32 - p.g as i32) >= 20 {
                blue_back_btn_count += 1;
            }
        }
    }

    let (island_avg_luma, _, _) = grid.region_stats(40, 2, 80, 11);
    let (bot_right_luma, _, _) = grid.region_stats(80, 200, 115, 250);

    let is_efootball_formation = island_avg_luma < 35
        && blue_back_btn_count >= 6
        && pitch_green_count >= 75
        && bot_right_luma < 60;

    // 3. eFootball Neon Poster (Messi / Yamal electric flanks & jersey / Konami Red)
    let (_left_flank_luma, left_flank_sat, _) = grid.region_stats(6, 2, 42, 12);
    let (_right_flank_luma, right_flank_sat, _) = grid.region_stats(78, 2, 114, 12);
    let avg_flank_sat = (left_flank_sat + right_flank_sat) / 2;

    // Check for Messi pink jersey with blue component b >= 110 and navy belt b > g + 5
    let mut messi_pink_jersey_pixels = 0usize;
    let mut navy_belt_pixels = 0usize;
    let mut pure_red_pixels = 0usize;
    for y in 20..=220 {
        for x in 10..=110 {
            let p = grid.get_pixel(x, y);
            if p.r >= 140 && p.b >= 110 && (p.r as i32 - p.g as i32) >= 30 {
                messi_pink_jersey_pixels += 1;
            }
            if p.b >= 70 && (p.b as i32 - p.g as i32) > 5 && (p.b as i32 - p.r as i32) > 10 {
                navy_belt_pixels += 1;
            }
            if p.r >= 180 && p.g <= 40 && p.b <= 40 {
                pure_red_pixels += 1;
            }
        }
    }

    let is_efootball_neon_poster = (left_flank_sat >= 45 || right_flank_sat >= 45 || avg_flank_sat >= 48)
        && (messi_pink_jersey_pixels >= 15 || navy_belt_pixels >= 25 || avg_flank_sat >= 55 || pure_red_pixels >= 50);

    // 4. Landscape Menu Layout (Dark Edge Strips with Bright Center and Game Graphics)
    let (left_strip_luma, _, _) = grid.region_stats(1, 20, 8, 240);
    let (right_strip_luma, _, _) = grid.region_stats(112, 20, 119, 240);
    let (center_luma, _, _) = grid.region_stats(30, 60, 90, 200);

    let mut vivid_game_pixels = 0usize;
    for y in (30..=220).step_by(4) {
        for x in (10..=110).step_by(4) {
            let p = grid.get_pixel(x, y);
            if p.sat >= 35 && p.luma >= 35 {
                vivid_game_pixels += 1;
            }
        }
    }

    let is_landscape_menu = (right_strip_luma <= 34 || left_strip_luma <= 38)
        && center_luma >= 35
        && (vivid_game_pixels >= 8 || is_efootball_formation || is_efootball_neon_poster);

    // 5. YouTube Fullscreen Dual Clean Corridors & Ambient Mode
    // Top Pillarbox 6-blocks on y: 2..8
    let pillar_blocks = [
        (6, 23), (24, 41), (42, 59), (60, 77), (78, 95), (96, 113)
    ];
    let mut top_pure_blocks = 0usize;
    for &(bx1, bx2) in &pillar_blocks {
        let (b_luma, b_sat, _) = grid.region_stats(bx1, 2, bx2, 8);
        if (b_luma <= 18 && b_sat <= 18) || (b_luma <= 34 && b_sat <= 25) {
            top_pure_blocks += 1;
        }
    }
    let top_pillar_is_black = top_pure_blocks >= 5;

    // Bottom Dual Corridors on y: 244..252 (Left: x=24..43, Right: x=76..102)
    let (bot_left_luma, bot_left_sat, _) = grid.region_stats(24, 244, 43, 252);
    let (bot_right_luma, bot_right_sat, _) = grid.region_stats(76, 244, 102, 252);
    let is_left_corr_valid = (bot_left_luma <= 18 && bot_left_sat <= 18) || (bot_left_luma <= 34 && bot_left_sat <= 25);
    let is_right_corr_valid = (bot_right_luma <= 18 && bot_right_sat <= 18) || (bot_right_luma <= 34 && bot_right_sat <= 25);
    let has_clean_bottom_corridors = is_left_corr_valid && is_right_corr_valid;

    // YouTube Landscape Red Progress Bar OSD at x=18..34, y=16..225
    let mut red_osd_pixels = 0usize;
    for y in (16..=225).step_by(2) {
        for x in (18..=34).step_by(2) {
            let p = grid.get_pixel(x, y);
            if p.r >= 160 && (p.r as i32 - p.g as i32) >= 60 && (p.r as i32 - p.b as i32) >= 60 {
                red_osd_pixels += 1;
            }
        }
    }
    let has_youtube_landscape_osd = red_osd_pixels >= 4;

    // TikTok Splash Screen: black backdrop with white/cyan/magenta logo at y=18..70 & y=182..232
    let (tt_splash_top, _, _) = grid.region_stats(40, 18, 80, 70);
    let (tt_splash_bot, _, _) = grid.region_stats(40, 182, 80, 232);
    let (tt_splash_bg, _, _) = grid.region_stats(2, 12, 20, 30);
    let is_tiktok_splash_screen = tt_splash_bg <= 10 && tt_splash_top >= 30 && tt_splash_bot >= 30 && !is_efootball_neon_poster && !is_efootball_formation;

    // Center video content luma: YouTube fullscreen has bright video in between black bars
    let (center_vid_luma, _, _) = grid.region_stats(20, 40, 100, 210);
    let is_youtube_fullscreen = ((top_pillar_is_black && has_clean_bottom_corridors && center_vid_luma >= 35) || has_youtube_landscape_osd) && !is_tiktok_splash_screen;

    // 6. Explicit Landscape Trigger flag
    let is_explicit_landscape = is_efootball_neon_poster
        || is_efootball_formation
        || is_landscape_menu
        || is_youtube_fullscreen
        || has_youtube_landscape_osd;

    // 7. Portrait Protections & Guards (Must not false-trigger on explicit landscape game/video!)
    // iPhone 11 Pro Max Notch Colored Clock Pill at x=4..30, y=2..10
    let mut notch_pill_colored_pixels = 0usize;
    for y in 2..=10 {
        for x in 4..=30 {
            let p = grid.get_pixel(x, y);
            let is_green = p.g >= 120 && (p.g as i32 - p.r as i32) >= 30 && (p.g as i32 - p.b as i32) >= 30;
            let is_blue = p.b >= 120 && (p.b as i32 - p.r as i32) >= 30 && (p.b as i32 - p.g as i32) >= 20;
            let is_red = p.r >= 140 && (p.r as i32 - p.g as i32) >= 40 && (p.r as i32 - p.b as i32) >= 40;
            if is_green || is_blue || is_red {
                notch_pill_colored_pixels += 1;
            }
        }
    }
    let has_notch_colored_clock_pill = notch_pill_colored_pixels >= 15 && !is_explicit_landscape;

    // TikTok OLED Comment Sheet: pure black flanks at y=12..46 + thumbnail top-center
    let (tt_flank_luma1, _, _) = grid.region_stats(4, 12, 28, 46);
    let (tt_flank_luma2, _, _) = grid.region_stats(92, 12, 116, 46);
    let (tt_center_luma, _, _) = grid.region_stats(45, 12, 75, 46);
    let is_tiktok_comment_sheet = tt_flank_luma1 <= 5 && tt_flank_luma2 <= 5 && tt_center_luma >= 25 && !is_explicit_landscape;

    // Instagram Reels / Dark Chrome (1:1 square video, feed, or story viewer with top header & bottom nav)
    let (reels_header_luma, reels_header_sat, _) = grid.region_stats(12, 12, 72, 22);
    let (reels_bot_luma, reels_bot_sat, _) = grid.region_stats(10, 235, 110, 255);
    let is_instagram_reels_dark_chrome = reels_header_luma >= 45
        && reels_header_sat <= 35
        && reels_bot_luma >= 15
        && reels_bot_luma <= 85
        && reels_bot_sat <= 35
        && !is_explicit_landscape;

    // Google Translate Dark Mode (4-quadrants around G watermark + iOS pastel blue mic)
    let mut blue_mic_pixels = 0usize;
    for y in 210..=245 {
        for x in 45..=75 {
            let p = grid.get_pixel(x, y);
            if p.b >= 160 && (p.b as i32 - p.r as i32) >= 40 && p.g >= 120 {
                blue_mic_pixels += 1;
            }
        }
    }
    let (gt_quad1, _, _) = grid.region_stats(10, 30, 45, 80);
    let (gt_quad2, _, _) = grid.region_stats(75, 30, 110, 80);
    let is_google_translate_dark = blue_mic_pixels >= 12 && gt_quad1 <= 35 && gt_quad2 <= 35 && !is_explicit_landscape;

    // iOS Control Center View
    let (cc_top_luma, _, _) = grid.region_stats(10, 10, 110, 40);
    let (cc_bot_luma, _, _) = grid.region_stats(10, 180, 110, 240);
    let is_control_center_view = cc_top_luma >= 30 && cc_bot_luma >= 30 && (cc_top_luma as i32 - cc_bot_luma as i32).abs() <= 18 && !top_pillar_is_black && avg_flank_sat < 42 && !is_explicit_landscape;

    // iOS Folder View: top blur y=14..48 and bottom blur y=198..238 similarity with low sat <= 30
    let (folder_top_luma, folder_top_sat, _) = grid.region_stats(15, 14, 105, 48);
    let (folder_bot_luma, folder_bot_sat, _) = grid.region_stats(15, 198, 105, 238);
    let is_ios_folder_view = folder_top_luma >= 35
        && folder_bot_luma >= 35
        && (folder_top_luma as i32 - folder_bot_luma as i32).abs() <= 20
        && folder_top_sat <= 30
        && folder_bot_sat <= 30
        && !top_pillar_is_black
        && !is_explicit_landscape;

    // iOS Home Screen Dock (y=218..248, colorful app icons)
    let mut dock_colorful_pixels = 0usize;
    for y in (218..=248).step_by(2) {
        for x in (12..=108).step_by(2) {
            let p = grid.get_pixel(x, y);
            if p.sat >= 45 && p.luma >= 45 {
                dock_colorful_pixels += 1;
            }
        }
    }
    let has_ios_home_screen_dock = dock_colorful_pixels >= 15 && !is_efootball_formation && !is_efootball_neon_poster;

    // Determine Strict Portrait Exit Proof
    let strict_portrait_exit_proof = has_notch_colored_clock_pill
        || is_tiktok_comment_sheet
        || is_tiktok_splash_screen
        || is_instagram_reels_dark_chrome
        || is_google_translate_dark
        || is_control_center_view
        || is_ios_folder_view
        || has_ios_home_screen_dock;

    // Final Landscape determination
    let is_landscape_game_frame = if strict_portrait_exit_proof {
        false
    } else {
        is_explicit_landscape || is_currently_landscape
    };

    let suggested_orientation = if is_landscape_game_frame {
        OrientationMode::Landscape270 // Hard Horizontal Lock 270 deg
    } else {
        OrientationMode::Portrait0
    };

    FrameAnalysisResult {
        is_landscape_game_frame,
        is_efootball_neon_poster,
        is_efootball_formation,
        is_landscape_menu,
        is_youtube_fullscreen,
        has_youtube_landscape_osd,
        has_notch_colored_clock_pill,
        is_tiktok_comment_sheet,
        is_tiktok_splash_screen,
        is_instagram_reels_dark_chrome,
        is_google_translate_dark,
        is_control_center_view,
        is_ios_folder_view,
        has_ios_home_screen_dock,
        strict_portrait_exit_proof,
        suggested_orientation,
    }
}

/// Evaluates rotation decision given app profile and orientation mode.
pub fn evaluate_auto_rotate(
    app_profile: AppProfile,
    width: u32,
    height: u32,
    sensor_angle: u32,
) -> RotationDecision {
    if app_profile.is_portrait_guarded() {
        return RotationDecision {
            orientation: OrientationMode::Portrait0,
            is_locked: true,
            reason: "Portrait guard active (Social / System Overlay)",
        };
    }

    if app_profile == AppProfile::EFootball {
        let orientation = if sensor_angle == 90 {
            OrientationMode::Landscape90
        } else {
            OrientationMode::Landscape270
        };
        return RotationDecision {
            orientation,
            is_locked: true,
            reason: "eFootball horizontal match/formation lock",
        };
    }

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RotationDecision {
    pub orientation: OrientationMode,
    pub is_locked: bool,
    pub reason: &'static str,
}

// ============================================================================
// 32 SYNTHETIC TEST SCENARIOS SELF-TEST
// ============================================================================

/// Runs all 32 smart auto-rotate scenarios and returns (passed_count, total_count).
pub fn run_32_autorotate_scenarios_self_test() -> (usize, usize) {
    let mut passed = 0usize;
    let total = 32usize;

    let check = |idx: usize, name: &str, grid: &ProbeGrid120x260, is_land: bool, expected: OrientationMode| -> bool {
        let res = analyze_frame_grid(grid, is_land);
        if res.suggested_orientation == expected {
            true
        } else {
            println!("FAIL Scenario {}: {} (is_land={}) -> Got {:?}, Expected {:?}", idx, name, is_land, res.suggested_orientation, expected);
            false
        }
    };

    // --- Skenario 1..12: eFootball 2027 Games (Expected: Landscape270) ---
    // 1. Intro Merah KONAMI
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(210, 15, 20); // Pure vivid red
        grid.fill_rect(6, 2, 42, 12, 220, 20, 20);
        grid.fill_rect(78, 2, 114, 12, 220, 20, 20);
        if check(1, "eFootball_Intro_Red_Konami", &grid, false, OrientationMode::Landscape270) { passed += 1; }
    }

    // 2. Intro Hitam Logo 'e'
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(10, 10, 12);
        grid.fill_rect(50, 100, 70, 160, 240, 220, 10); // Yellow e-logo
        if check(2, "eFootball_Intro_Black_eLogo", &grid, true, OrientationMode::Landscape270) { passed += 1; }
    }

    // 3. Intro Biru "THE PITCH IS OURS"
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(15, 25, 95);
        grid.fill_rect(1, 20, 8, 240, 10, 12, 25);
        if check(3, "eFootball_Intro_Blue_ThePitchIsOurs", &grid, true, OrientationMode::Landscape270) { passed += 1; }
    }

    // 4. Poster Neon Lionel Messi & Lamine Yamal
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(12, 14, 25);
        grid.fill_rect(6, 2, 42, 12, 20, 180, 210); // Cyan electric flank
        grid.fill_rect(78, 2, 114, 12, 210, 30, 190); // Magenta electric flank
        grid.fill_rect(40, 70, 80, 130, 220, 40, 140); // Messi pink jersey (b=140 >= 110)
        grid.fill_rect(40, 140, 80, 170, 20, 30, 90); // Navy belt (b=90 > g=30 + 5)
        if check(4, "eFootball_Neon_Poster_Messi_Yamal", &grid, false, OrientationMode::Landscape270) { passed += 1; }
    }

    // 5. Menu Utama / Beranda Brasileirao
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(60, 70, 90);
        grid.fill_rect(112, 20, 119, 240, 12, 14, 20); // Right edge strip luma <= 30
        grid.fill_rect(1, 20, 8, 240, 15, 18, 25); // Left edge strip
        grid.fill_rect(20, 50, 100, 120, 220, 180, 20); // Vivid banner
        if check(5, "eFootball_MainMenu_Brasileirao", &grid, false, OrientationMode::Landscape270) { passed += 1; }
    }

    // 6. Menu Utama J.LEAGUE
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(50, 60, 80);
        grid.fill_rect(112, 20, 119, 240, 14, 16, 22);
        grid.fill_rect(1, 20, 8, 240, 16, 18, 24);
        grid.fill_rect(20, 50, 100, 120, 220, 180, 20); // Vivid banner
        if check(6, "eFootball_MainMenu_JLeague", &grid, false, OrientationMode::Landscape270) { passed += 1; }
    }

    // 7. Taktik / Formasi 3-2-2-3 (Green pitch + blue back button)
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(20, 20, 25);
        grid.fill_rect(22, 45, 96, 195, 30, 110, 40); // Green pitch (g=110 <= 155)
        grid.fill_rect(4, 11, 24, 26, 20, 50, 160); // Blue "< Kembali" button
        grid.fill_rect(40, 2, 80, 11, 10, 10, 10); // Island zone dark
        grid.fill_rect(80, 200, 115, 250, 15, 15, 20); // Bottom right dark
        if check(7, "eFootball_Formation_3223", &grid, false, OrientationMode::Landscape270) { passed += 1; }
    }

    // 8. Match Gameplay (Wide Green Pitch)
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(20, 20, 25);
        grid.fill_rect(22, 45, 96, 195, 25, 115, 35);
        grid.fill_rect(4, 11, 24, 26, 20, 50, 160);
        grid.fill_rect(40, 2, 80, 11, 10, 10, 10);
        grid.fill_rect(80, 200, 115, 250, 15, 15, 20);
        if check(8, "eFootball_Match_Gameplay", &grid, false, OrientationMode::Landscape270) { passed += 1; }
    }

    // 9. Match Goal Celebration
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(40, 50, 70);
        grid.fill_rect(1, 20, 8, 240, 10, 12, 18);
        grid.fill_rect(112, 20, 119, 240, 12, 14, 20);
        grid.fill_rect(20, 60, 100, 180, 30, 120, 40); // Stadium grass
        if check(9, "eFootball_Match_GoalCelebration", &grid, true, OrientationMode::Landscape270) { passed += 1; }
    }

    // 10. Match Half-Time Menu
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(55, 65, 80);
        grid.fill_rect(1, 20, 8, 240, 12, 15, 20);
        grid.fill_rect(112, 20, 119, 240, 15, 18, 22);
        grid.fill_rect(20, 50, 100, 120, 210, 170, 25); // Stats banner
        if check(10, "eFootball_Match_HalfTimeMenu", &grid, false, OrientationMode::Landscape270) { passed += 1; }
    }

    // 11. Penalty Shootout
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(20, 22, 28);
        grid.fill_rect(22, 45, 96, 195, 35, 120, 45);
        grid.fill_rect(4, 11, 24, 26, 25, 60, 170);
        grid.fill_rect(40, 2, 80, 11, 10, 10, 10);
        grid.fill_rect(80, 200, 115, 250, 15, 15, 20);
        if check(11, "eFootball_Match_PenaltyShootout", &grid, false, OrientationMode::Landscape270) { passed += 1; }
    }

    // 12. Victory Screen
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(14, 16, 24);
        grid.fill_rect(6, 2, 42, 12, 220, 25, 180);
        grid.fill_rect(78, 2, 114, 12, 20, 210, 220);
        grid.fill_rect(40, 70, 80, 130, 210, 30, 150);
        grid.fill_rect(40, 140, 80, 170, 20, 30, 90);
        if check(12, "eFootball_Victory_Screen", &grid, false, OrientationMode::Landscape270) { passed += 1; }
    }

    // --- Skenario 13..19: YouTube Fullscreen Landscape (Expected: Landscape270) ---
    // 13. YouTube 16:9 Fullscreen (Dual Clean Corridors)
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(100, 110, 120);
        grid.fill_rect(0, 0, 119, 10, 5, 5, 5); // Pure black top
        grid.fill_rect(0, 240, 119, 259, 5, 5, 5); // Pure black bottom
        if check(13, "YouTube_16_9_Fullscreen_Landscape", &grid, false, OrientationMode::Landscape270) { passed += 1; }
    }

    // 14. YouTube 21:9 Ultrawide
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(90, 95, 105);
        grid.fill_rect(0, 0, 119, 20, 2, 2, 2);
        grid.fill_rect(0, 230, 119, 259, 2, 2, 2);
        if check(14, "YouTube_21_9_Ultrawide_Landscape", &grid, false, OrientationMode::Landscape270) { passed += 1; }
    }

    // 15. YouTube 4:3 Pillarbox
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(80, 85, 90);
        grid.fill_rect(0, 0, 119, 12, 0, 0, 0);
        grid.fill_rect(0, 240, 119, 259, 0, 0, 0);
        if check(15, "YouTube_4_3_Classic_Pillarbox", &grid, false, OrientationMode::Landscape270) { passed += 1; }
    }

    // 16. YouTube Ambient Mode Glow (avg luma <= 34, sat <= 25)
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(110, 120, 130);
        grid.fill_rect(0, 0, 119, 10, 25, 26, 30); // Ambient glow
        grid.fill_rect(0, 240, 119, 259, 24, 25, 29);
        if check(16, "YouTube_Ambient_Mode_Glow", &grid, false, OrientationMode::Landscape270) { passed += 1; }
    }

    // 17. YouTube Zoom-to-Fill with Red Progress Bar OSD
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(85, 90, 95);
        grid.fill_rect(20, 20, 26, 200, 220, 15, 15); // Red bar
        if check(17, "YouTube_Zoom_To_Fill_OSD", &grid, false, OrientationMode::Landscape270) { passed += 1; }
    }

    // 18. YouTube Landscape with Channel Watermark at x=4..21
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(90, 90, 90);
        grid.fill_rect(0, 0, 119, 10, 5, 5, 5);
        grid.fill_rect(0, 240, 119, 259, 5, 5, 5);
        grid.fill_rect(4, 244, 21, 252, 200, 200, 200); // Watermark at x=4..21 (corridor x=24..43 clean!)
        if check(18, "YouTube_Landscape_With_Watermark", &grid, false, OrientationMode::Landscape270) { passed += 1; }
    }

    // 19. YouTube Landscape with AssistiveTouch at x=45..74
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(90, 90, 90);
        grid.fill_rect(0, 0, 119, 10, 5, 5, 5);
        grid.fill_rect(0, 240, 119, 259, 5, 5, 5);
        grid.fill_rect(45, 244, 74, 252, 140, 140, 140); // AssistiveTouch avoided by corridors!
        if check(19, "YouTube_Landscape_With_AssistiveTouch", &grid, false, OrientationMode::Landscape270) { passed += 1; }
    }

    // --- Skenario 20..32: Portrait Guard Screens (Expected: Portrait0) ---
    // 20. iPhone 11 Pro Max Notch Colored Clock Pill (Blue)
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(15, 15, 18);
        grid.fill_rect(4, 2, 28, 9, 20, 80, 220); // Blue clock pill
        if check(20, "Portrait_iPhone11ProMax_Notch_ClockPill", &grid, true, OrientationMode::Portrait0) { passed += 1; }
    }

    // 21. iPhone 17 Pro Dynamic Island Clock (Home Dock present)
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(20, 22, 26);
        grid.fill_rect(12, 220, 108, 246, 210, 60, 180); // Colorful icons
        if check(21, "Portrait_iPhone17Pro_DynamicIsland_Clock", &grid, true, OrientationMode::Portrait0) { passed += 1; }
    }

    // 22. TikTok OLED Comment Sheet
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(12, 12, 14);
        grid.fill_rect(4, 12, 28, 46, 2, 2, 2); // Black flank 1
        grid.fill_rect(92, 12, 116, 46, 2, 2, 2); // Black flank 2
        grid.fill_rect(45, 12, 75, 46, 80, 80, 85); // Thumbnail
        grid.fill_rect(10, 242, 110, 256, 50, 50, 55); // Bottom comment bar
        if check(22, "Portrait_TikTok_OLED_CommentSheet", &grid, true, OrientationMode::Portrait0) { passed += 1; }
    }

    // 23. TikTok Splash Screen
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(4, 4, 4);
        grid.fill_rect(0, 0, 119, 10, 30, 30, 30); // Top status row
        grid.fill_rect(40, 18, 80, 70, 200, 200, 200);
        grid.fill_rect(40, 182, 80, 232, 200, 200, 200);
        if check(23, "Portrait_TikTok_SplashScreen", &grid, true, OrientationMode::Portrait0) { passed += 1; }
    }

    // 24. TikTok Feed Dark
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(10, 10, 12);
        grid.fill_rect(4, 12, 28, 46, 2, 2, 2);
        grid.fill_rect(92, 12, 116, 46, 2, 2, 2);
        grid.fill_rect(45, 12, 75, 46, 75, 75, 80);
        grid.fill_rect(10, 242, 110, 256, 45, 45, 50); // Bottom tab bar
        if check(24, "Portrait_TikTok_Feed_Dark", &grid, true, OrientationMode::Portrait0) { passed += 1; }
    }

    // 25. Instagram Reels 1:1 Square Video
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(10, 10, 10);
        grid.fill_rect(12, 12, 72, 22, 180, 180, 180); // Reels header
        grid.fill_rect(10, 244, 110, 256, 50, 50, 50); // Bottom nav bar
        if check(25, "Portrait_Instagram_Reels_1_1_Square", &grid, true, OrientationMode::Portrait0) { passed += 1; }
    }

    // 26. Instagram Dark Mode Feed
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(12, 12, 14);
        grid.fill_rect(12, 12, 72, 22, 190, 190, 190);
        grid.fill_rect(10, 244, 110, 256, 50, 50, 50); // Bottom nav bar
        if check(26, "Portrait_Instagram_DarkMode_Feed", &grid, true, OrientationMode::Portrait0) { passed += 1; }
    }

    // 27. Instagram Story Viewer
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(15, 15, 18);
        grid.fill_rect(12, 12, 72, 22, 195, 195, 195);
        grid.fill_rect(10, 244, 110, 256, 50, 50, 50); // Bottom reply bar
        if check(27, "Portrait_Instagram_Story_Viewer", &grid, true, OrientationMode::Portrait0) { passed += 1; }
    }

    // 28. Google Translate Dark Mode
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(20, 20, 22);
        grid.fill_rect(45, 210, 75, 245, 40, 140, 230); // Pastel blue mic
        if check(28, "Portrait_GoogleTranslate_DarkMode", &grid, true, OrientationMode::Portrait0) { passed += 1; }
    }

    // 29. iOS Control Center View
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(45, 45, 50); // Blurred platter
        if check(29, "Portrait_iOS_ControlCenter", &grid, true, OrientationMode::Portrait0) { passed += 1; }
    }

    // 30. iOS Folder View
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(50, 52, 56);
        grid.fill_rect(15, 14, 105, 48, 52, 54, 58);
        grid.fill_rect(15, 198, 105, 238, 51, 53, 57);
        if check(30, "Portrait_iOS_Folder_View", &grid, true, OrientationMode::Portrait0) { passed += 1; }
    }

    // 31. iOS Home Screen Dock
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(30, 32, 35);
        grid.fill_rect(12, 218, 108, 248, 220, 45, 160); // Colorful icons
        if check(31, "Portrait_iOS_HomeScreen_Dock", &grid, true, OrientationMode::Portrait0) { passed += 1; }
    }

    // 32. Apple Notes Light Mode
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(245, 245, 240);
        grid.fill_rect(12, 218, 108, 248, 220, 45, 160); // Dock icons
        if check(32, "Portrait_Apple_Notes_LightMode", &grid, true, OrientationMode::Portrait0) { passed += 1; }
    }

    (passed, total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_grid_pixel_setting_and_retrieval() {
        let mut grid = ProbeGrid120x260::new();
        grid.set_pixel(10, 20, 100, 150, 200);
        let p = grid.get_pixel(10, 20);
        assert_eq!(p.r, 100);
        assert_eq!(p.g, 150);
        assert_eq!(p.b, 200);
        assert!(p.luma > 0);
        assert_eq!(p.sat, 100); // 200 - 100 = 100
    }

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

    #[test]
    fn test_run_32_autorotate_scenarios() {
        let (passed, total) = run_32_autorotate_scenarios_self_test();
        assert_eq!(passed, total, "All 32 scenarios must pass 100%!");
        assert_eq!(passed, 32);
    }
}
