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
#[derive(Debug, Clone)]
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

/// Result of detailed frame analysis (1:1 parity with `handset/web/mirror.js:17-2525`).
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
    pub is_konami_red_intro: bool,
    pub has_landscape_side_bar: bool,
    pub has_naked_dynamic_island: bool,
    pub has_plain_white_clock_status_bar: bool,
    pub is_calm_flank_status: bool,
    pub has_true_portrait_status_bar: bool,
    pub has_portrait_bottom_home_bar: bool,
    pub has_portrait_split_app_chrome: bool,
    pub navy_belt_ratio_pct: u8,
    pub upper_poster_neon_pixels: usize,
    pub lower_poster_neon_pixels: usize,
    pub has_top_white_chrome: bool,
    pub has_bottom_white_nav_bar: bool,
    pub has_light_portrait_app_chrome: bool,
    pub has_instagram_reels_dark_chrome: bool,
    pub has_instagram_dark_mode_chrome: bool,
    pub has_tiktok_center_plus_button: bool,
    pub has_tiktok_or_portrait_bottom_bar: bool,
    pub is_portrait_comment_sheet_screen: bool,
    pub is_app_splash_screen: bool,
    pub is_portrait_overlay_or_folder: bool,
    pub is_definite_portrait_app_screen: bool,
    pub top_pillar_is_black: bool,
    pub bot_pillar_is_black: bool,
    pub has_dual_black_pillarbox: bool,
    pub has_sticky_fullscreen_pillarbox_lock: bool,
    pub exit_by_light_portrait_chrome: bool,
    pub exit_by_youtube_portrait_watch_page: bool,
    pub exit_by_home_screen_portrait: bool,
    pub exit_by_ios_folder_portrait: bool,
    pub exit_by_instagram_dark_mode: bool,
    pub can_passive_exit_to_portrait: bool,
    pub last_reason: &'static str,
}

#[inline(always)]
fn px_luma(p: Pixel) -> f32 {
    (p.r as f32 * 299.0 + p.g as f32 * 587.0 + p.b as f32 * 114.0) / 1000.0
}

#[inline(always)]
fn px_sat(p: Pixel) -> i32 {
    let mx = p.r.max(p.g).max(p.b) as i32;
    let mn = p.r.min(p.g).min(p.b) as i32;
    mx - mn
}

fn check_video_zone_active(grid: &ProbeGrid120x260, y_start: usize, y_end: usize) -> bool {
    let mut bright_pix = 0usize;
    let mut z_sum = 0.0f32;
    let mut z_cnt = 0usize;
    let mut z_max = 0.0f32;
    let mut z_min = 255.0f32;
    for y in (y_start..=y_end).step_by(3) {
        for x in (12..=85).step_by(3) {
            let l = px_luma(grid.get_pixel(x, y));
            z_sum += l;
            z_cnt += 1;
            if l >= 40.0 {
                bright_pix += 1;
            }
            if l > z_max {
                z_max = l;
            }
            if l < z_min {
                z_min = l;
            }
        }
    }
    let avg_l = if z_cnt > 0 { z_sum / z_cnt as f32 } else { 0.0 };
    bright_pix >= 8 && ((z_max - z_min) >= 30.0 || avg_l >= 32.0)
}

/// Analyze frame orientation from `ProbeGrid120x260` matching `handset/web/mirror.js:89-2525` 1:1.
pub fn analyze_frame_grid(grid: &ProbeGrid120x260, is_currently_landscape: bool) -> FrameAnalysisResult {
    // 0. eFootball turf pitch green early count (mirror.js:146-154)
    let mut pitch_green_count = 0usize;
    for y in (45..=195).step_by(4) {
        for x in (22..=96).step_by(4) {
            let p = grid.get_pixel(x, y);
            if p.g >= 30
                && p.g <= 155
                && (p.g as i32 - p.r as i32) >= 10
                && (p.g as i32 - p.b as i32) >= 12
            {
                pitch_green_count += 1;
            }
        }
    }

    // 0b. iOS Home Screen Bottom Dock: 3 gaps + 4 slots (mirror.js:156-249)
    let mut gap_sums = [(0.0f32, 0.0f32, 0usize); 3];
    let dock_gap_coords = [(30usize, 34usize), (56, 61), (83, 87)];
    for y in 224..=242 {
        for (gi, &(gx1, gx2)) in dock_gap_coords.iter().enumerate() {
            for x in gx1..=gx2 {
                let p = grid.get_pixel(x, y);
                gap_sums[gi].0 += px_luma(p);
                gap_sums[gi].1 += px_sat(p) as f32;
                gap_sums[gi].2 += 1;
            }
        }
    }
    let gap_avgs: [(f32, f32); 3] = [0, 1, 2].map(|i| {
        let (ls, ss, c) = gap_sums[i];
        if c > 0 {
            (ls / c as f32, ss / c as f32)
        } else {
            (100.0, 100.0)
        }
    });
    let mut dock_gap_quiet_count = 0usize;
    for &(gl, gs) in &gap_avgs {
        if gl <= 85.0 && gs <= 38.0 {
            dock_gap_quiet_count += 1;
        }
    }
    let all_gaps_avg_luma = (gap_avgs[0].0 + gap_avgs[1].0 + gap_avgs[2].0) / 3.0;
    let all_gaps_avg_sat = (gap_avgs[0].1 + gap_avgs[1].1 + gap_avgs[2].1) / 3.0;

    let dock_slots_coords = [(11usize, 27usize), (37, 53), (64, 80), (90, 106)];
    let mut dock_green_slots = 0usize;
    let mut dock_safari_slot2 = false;
    let mut dock_active_slots = 0usize;
    for (s_idx, &(sx1, sx2)) in dock_slots_coords.iter().enumerate() {
        let mut s_luma = 0.0f32;
        let mut s_sat = 0.0f32;
        let mut s_cnt = 0usize;
        let mut s_green_count = 0usize;
        let mut s_blue_white_count = 0usize;
        for y in 222..=243 {
            for x in sx1..=sx2 {
                let p = grid.get_pixel(x, y);
                let l = px_luma(p);
                let sat = px_sat(p);
                s_luma += l;
                s_sat += sat as f32;
                s_cnt += 1;
                if p.g >= 140
                    && (p.g as i32 - p.r as i32) >= 38
                    && (p.g as i32 - p.b as i32) >= 25
                {
                    s_green_count += 1;
                }
                if (p.b >= 140 && (p.b as i32 - p.r as i32) >= 32) || (l >= 180.0 && sat <= 35) {
                    s_blue_white_count += 1;
                }
            }
        }
        let slot_avg_l = if s_cnt > 0 { s_luma / s_cnt as f32 } else { 0.0 };
        let slot_avg_s = if s_cnt > 0 { s_sat / s_cnt as f32 } else { 0.0 };
        if (s_idx == 0 || s_idx == 2 || s_idx == 3) && s_green_count >= 6 {
            dock_green_slots += 1;
        }
        if s_idx == 1 && s_blue_white_count >= 5 {
            dock_safari_slot2 = true;
        }
        if (slot_avg_l - all_gaps_avg_luma >= 18.0) || (slot_avg_s - all_gaps_avg_sat >= 18.0) {
            dock_active_slots += 1;
        }
    }
    let has_ios_home_screen_dock = pitch_green_count < 65
        && ((dock_green_slots >= 2 && dock_safari_slot2 && dock_gap_quiet_count >= 2)
            || (dock_green_slots >= 2 && dock_active_slots >= 3 && dock_gap_quiet_count >= 2));

    // 0c. Preliminary left-edge ONLY side bar with 4-side isolation (mirror.js:253-309)
    let mut inner_col_sum_98_162 = 0.0f32;
    let mut inner_col_cnt_98_162 = 0usize;
    for y in (98..=162).step_by(2) {
        for x in 8..=13 {
            inner_col_sum_98_162 += px_luma(grid.get_pixel(x, y));
            inner_col_cnt_98_162 += 1;
        }
    }
    let inner_col_avg_luma_98_162 = if inner_col_cnt_98_162 > 0 {
        inner_col_sum_98_162 / inner_col_cnt_98_162 as f32
    } else {
        0.0
    };

    let mut has_preliminary_side_bar = false;
    if !has_ios_home_screen_dock {
        let mut top_cap_sum = 0.0f32;
        let mut top_cap_cnt = 0usize;
        for y in (62..=80).step_by(2) {
            for x in 1..=3 {
                top_cap_sum += px_luma(grid.get_pixel(x, y));
                top_cap_cnt += 1;
            }
        }
        let top_cap_edge_luma = if top_cap_cnt > 0 { top_cap_sum / top_cap_cnt as f32 } else { 0.0 };

        let mut bot_cap_sum = 0.0f32;
        let mut bot_cap_cnt = 0usize;
        for y in (180..=198).step_by(2) {
            for x in 1..=3 {
                bot_cap_sum += px_luma(grid.get_pixel(x, y));
                bot_cap_cnt += 1;
            }
        }
        let bot_cap_edge_luma = if bot_cap_cnt > 0 { bot_cap_sum / bot_cap_cnt as f32 } else { 0.0 };

        let mut near_sum = 0.0f32;
        let mut near_cnt = 0usize;
        for y in (105..=155).step_by(2) {
            for x in 5..=7 {
                near_sum += px_luma(grid.get_pixel(x, y));
                near_cnt += 1;
            }
        }
        let near_inner_col_luma = if near_cnt > 0 { near_sum / near_cnt as f32 } else { 0.0 };

        for x_edge in 1..=3 {
            let mut mid_sum = 0.0f32;
            let mut mid_cnt = 0usize;
            for y in 105..=155 {
                mid_sum += px_luma(grid.get_pixel(x_edge, y));
                mid_cnt += 1;
            }
            let mid_bar_edge_luma = if mid_cnt > 0 { mid_sum / mid_cnt as f32 } else { 0.0 };
            if mid_bar_edge_luma >= 118.0
                && (mid_bar_edge_luma - top_cap_edge_luma) >= 24.0
                && (mid_bar_edge_luma - bot_cap_edge_luma) >= 24.0
                && (mid_bar_edge_luma - near_inner_col_luma) >= 22.0
                && (mid_bar_edge_luma - inner_col_avg_luma_98_162) >= 32.0
            {
                has_preliminary_side_bar = true;
                break;
            }
        }
    }

    // 1. Overall & Center video luma (mirror.js:312-330)
    let mut total_luma = 0.0f32;
    let mut sample_count = 0usize;
    for y in (15..245).step_by(8) {
        for x in (12..108).step_by(8) {
            total_luma += px_luma(grid.get_pixel(x, y));
            sample_count += 1;
        }
    }
    let overall_avg_luma = if sample_count > 0 {
        total_luma / sample_count as f32
    } else {
        0.0
    };

    let mut video_luma_sum = 0.0f32;
    let mut video_sample_count = 0usize;
    for y in (35..=225).step_by(10) {
        for x in (20..=100).step_by(10) {
            video_luma_sum += px_luma(grid.get_pixel(x, y));
            video_sample_count += 1;
        }
    }
    let center_video_avg_luma = if video_sample_count > 0 {
        video_luma_sum / video_sample_count as f32
    } else {
        0.0
    };

    // 2. Dynamic Island pill at top center (x=46..=74, y=4..=9) & Flanks (mirror.js:332-451)
    let mut island_luma_sum = 0.0f32;
    let mut island_pixel_count = 0usize;
    let mut island_max_luma = 0.0f32;
    for y in 4..=9 {
        for x in 46..=74 {
            let l = px_luma(grid.get_pixel(x, y));
            island_luma_sum += l;
            island_pixel_count += 1;
            if l > island_max_luma {
                island_max_luma = l;
            }
        }
    }
    let island_avg_luma = if island_pixel_count > 0 {
        island_luma_sum / island_pixel_count as f32
    } else {
        100.0
    };

    let mut left_flank_sum = 0.0f32;
    let mut left_flank_cnt = 0usize;
    let mut right_flank_sum = 0.0f32;
    let mut right_flank_cnt = 0usize;
    for y in 4..=10 {
        for x in 14..=38 {
            left_flank_sum += px_luma(grid.get_pixel(x, y));
            left_flank_cnt += 1;
        }
        for x in 82..=106 {
            right_flank_sum += px_luma(grid.get_pixel(x, y));
            right_flank_cnt += 1;
        }
    }
    let left_flank_avg_luma = if left_flank_cnt > 0 { left_flank_sum / left_flank_cnt as f32 } else { 0.0 };
    let right_flank_avg_luma = if right_flank_cnt > 0 { right_flank_sum / right_flank_cnt as f32 } else { 0.0 };
    let max_flank_luma = left_flank_avg_luma.max(right_flank_avg_luma);

    let mut flank_sat_sum = 0.0f32;
    let mut flank_pixel_cnt = 0usize;
    let mut left_flank_sat_sum = 0.0f32;
    let mut left_flank_sat_cnt = 0usize;
    let mut right_flank_sat_sum = 0.0f32;
    let mut right_flank_sat_cnt = 0usize;
    let mut vivid_flank_pixel_count = 0usize;
    for y in 3..=12 {
        for x in 12..=40 {
            let p = grid.get_pixel(x, y);
            let l = px_luma(p);
            let sat = px_sat(p);
            flank_sat_sum += sat as f32;
            flank_pixel_cnt += 1;
            left_flank_sat_sum += sat as f32;
            left_flank_sat_cnt += 1;
            if sat >= 55 && l >= 40.0 {
                vivid_flank_pixel_count += 1;
            }
        }
        for x in 80..=108 {
            let p = grid.get_pixel(x, y);
            let l = px_luma(p);
            let sat = px_sat(p);
            flank_sat_sum += sat as f32;
            flank_pixel_cnt += 1;
            right_flank_sat_sum += sat as f32;
            right_flank_sat_cnt += 1;
            if sat >= 55 && l >= 40.0 {
                vivid_flank_pixel_count += 1;
            }
        }
    }
    let avg_flank_sat = if flank_pixel_cnt > 0 { flank_sat_sum / flank_pixel_cnt as f32 } else { 0.0 };
    let left_flank_avg_sat = if left_flank_sat_cnt > 0 { left_flank_sat_sum / left_flank_sat_cnt as f32 } else { 0.0 };
    let right_flank_avg_sat = if right_flank_sat_cnt > 0 { right_flank_sat_sum / right_flank_sat_cnt as f32 } else { 0.0 };

    let mut top_flank_electric_blue_magenta_pixels = 0usize;
    for y in 2..=12 {
        for x in (6..=42).chain(78..=114) {
            let p = grid.get_pixel(x, y);
            let sat = px_sat(p);
            if sat >= 60 && p.b >= 110 && ((p.b as i32 - p.g as i32) >= 12 || p.r >= 120) {
                top_flank_electric_blue_magenta_pixels += 1;
            }
        }
    }

    let mut blue_back_btn_count = 0usize;
    for y in (11..=26).step_by(2) {
        for x in (3..=28).step_by(2) {
            let p = grid.get_pixel(x, y);
            if p.b >= 140 && p.g >= 90 && p.r <= 80 && (p.b as i32 - p.r as i32) >= 70 {
                blue_back_btn_count += 1;
            }
        }
    }

    let mut bg_luma_left_sum = 0.0f32;
    let mut bg_left_cnt = 0usize;
    let mut bg_luma_right_sum = 0.0f32;
    let mut bg_right_cnt = 0usize;
    for y in [1usize, 2, 11, 12] {
        for x in 4..=35 {
            bg_luma_left_sum += px_luma(grid.get_pixel(x, y));
            bg_left_cnt += 1;
        }
        for x in 80..=116 {
            bg_luma_right_sum += px_luma(grid.get_pixel(x, y));
            bg_right_cnt += 1;
        }
    }
    let bg_luma_left = if bg_left_cnt > 0 { bg_luma_left_sum / bg_left_cnt as f32 } else { 0.0 };
    let bg_luma_right = if bg_right_cnt > 0 { bg_luma_right_sum / bg_right_cnt as f32 } else { 0.0 };

    // 3. Status bar flanks & chromatic analysis (mirror.js:453-605)
    let mut edge_count_left = 0usize;
    let mut min_luma_left = 255.0f32;
    let mut max_luma_left = 0.0f32;
    let mut left_clock_min_luma = 255.0f32;
    let mut left_clock_max_luma = 0.0f32;
    let mut achromatic_clock_glyph_count = 0usize;
    let mut neon_left_count = 0usize;
    for y in 2..=10 {
        for x in 4..=35 {
            let p1 = grid.get_pixel(x, y);
            let p2 = grid.get_pixel(x + 1, y);
            let l1 = px_luma(p1);
            let l2 = px_luma(p2);
            if l1 < min_luma_left {
                min_luma_left = l1;
            }
            if l1 > max_luma_left {
                max_luma_left = l1;
            }
            if l1 < left_clock_min_luma {
                left_clock_min_luma = l1;
            }
            if l1 > left_clock_max_luma {
                left_clock_max_luma = l1;
            }
            if (l1 - l2).abs() > 40.0 {
                edge_count_left += 1;
            }
            let sat = px_sat(p1);
            if l1 > 95.0 && sat > 65 {
                neon_left_count += 1;
            }
            if y <= 9 {
                let is_glyph = if bg_luma_left < 65.0 {
                    l1 >= (bg_luma_left + 16.0).max(48.0) && sat <= 34
                } else if bg_luma_left < 145.0 {
                    l1 >= (bg_luma_left + 35.0).max(88.0) && sat <= 34
                } else {
                    (bg_luma_left - bg_luma_right).abs() <= 35.0
                        && l1 <= (bg_luma_left - 30.0).min(160.0)
                        && sat <= 34
                };
                if is_glyph {
                    achromatic_clock_glyph_count += 1;
                }
            }
        }
    }

    let mut top_white_pixel_count = 0usize;
    for y in 1..=11 {
        for x in (8..=40).chain(80..=112) {
            let p = grid.get_pixel(x, y);
            if px_luma(p) >= 170.0 && px_sat(p) <= 32 {
                top_white_pixel_count += 1;
            }
        }
    }

    let mut edge_count_right = 0usize;
    let mut right_status_min_luma = 255.0f32;
    let mut right_status_max_luma = 0.0f32;
    for y in 2..=10 {
        for x in 80..=116 {
            let l1 = px_luma(grid.get_pixel(x, y));
            let l2 = px_luma(grid.get_pixel(x + 1, y));
            if l1 < right_status_min_luma {
                right_status_min_luma = l1;
            }
            if l1 > right_status_max_luma {
                right_status_max_luma = l1;
            }
            if (l1 - l2).abs() > 40.0 {
                edge_count_right += 1;
            }
        }
    }

    let mut neon_near_island_count = 0usize;
    for y in 2..=11 {
        for x in 40..=80 {
            let p = grid.get_pixel(x, y);
            if px_luma(p) > 95.0 && px_sat(p) > 70 {
                neon_near_island_count += 1;
            }
        }
    }

    let mut colorful_left_zone_count = 0usize;
    for y in 11..=24 {
        for x in 8..=42 {
            let p = grid.get_pixel(x, y);
            if px_luma(p) > 75.0 && px_sat(p) > 45 {
                colorful_left_zone_count += 1;
            }
        }
    }

    let mut clock_pill_color_pixels = 0usize;
    for y in 2..=10 {
        for x in 4..=30 {
            let p = grid.get_pixel(x, y);
            let is_colored_pill = (p.b >= 135 && (p.b as i32 - p.r as i32) >= 55)
                || (p.g >= 135 && (p.g as i32 - p.r as i32) >= 45)
                || (p.r >= 155 && (p.r as i32 - p.b as i32) >= 65);
            if is_colored_pill {
                clock_pill_color_pixels += 1;
            }
        }
    }

    let mut below_clock_blue_pixels = 0usize;
    for y in 12..=25 {
        for x in 4..=28 {
            let p = grid.get_pixel(x, y);
            if p.b >= 135 && p.g >= 80 && (p.b as i32 - p.r as i32) >= 60 {
                below_clock_blue_pixels += 1;
            }
        }
    }

    let mut has_notch_colored_clock_pill = clock_pill_color_pixels >= 4
        && (left_clock_max_luma - left_clock_min_luma) >= 35.0
        && below_clock_blue_pixels <= 2
        && (right_status_max_luma - right_status_min_luma) >= 18.0
        && edge_count_right >= 2
        && bg_luma_right <= 60.0;

    let has_notch_or_island_status_bar = (bg_luma_left >= 145.0 && bg_luma_right >= 145.0)
        || (island_avg_luma < 32.0 && island_max_luma < 52.0);
    let has_left_clock_presence = (left_clock_max_luma - left_clock_min_luma) >= 18.0
        && left_clock_max_luma >= (bg_luma_left + 16.0);
    let has_right_status_presence = (right_status_max_luma - right_status_min_luma) >= 18.0;

    // YouTube Landscape thin red vertical progress bar OSD (mirror.js:620-645)
    let mut max_thin_red_col_rows = 0usize;
    if !has_ios_home_screen_dock {
        for x_col in 18..=34 {
            let mut thin_red_rows = 0usize;
            for y in 20..=225 {
                let p = grid.get_pixel(x_col, y);
                if p.r >= 170 && p.g <= 55 && p.b <= 70 && (p.r as i32 - p.g as i32) >= 115 {
                    let p_left = grid.get_pixel(x_col.saturating_sub(4), y);
                    let p_right = grid.get_pixel((x_col + 4).min(GRID_WIDTH - 1), y);
                    let left_is_red = (p_left.r as i32 - p_left.g as i32) >= 55;
                    let right_is_red = (p_right.r as i32 - p_right.g as i32) >= 55;
                    if !left_is_red && !right_is_red {
                        thin_red_rows += 1;
                    }
                }
            }
            if thin_red_rows > max_thin_red_col_rows {
                max_thin_red_col_rows = thin_red_rows;
            }
        }
    }
    let mut has_youtube_landscape_osd =
        max_thin_red_col_rows >= 35 && !has_ios_home_screen_dock && !has_notch_colored_clock_pill;

    // Universal Calm-Flank Status Bar (mirror.js:648-657)
    let is_calm_flank_status = !has_youtube_landscape_osd
        && left_flank_avg_sat <= 32.0
        && colorful_left_zone_count <= 8
        && (left_clock_max_luma - left_clock_min_luma) >= 32.0
        && left_clock_max_luma >= 175.0
        && edge_count_left >= 3
        && (right_status_max_luma - right_status_min_luma) >= 24.0
        && edge_count_right >= 3;

    // Status Bar Color & Plain White Clock Detector (mirror.js:660-722)
    let mut colorful_status_row_count = 0usize;
    for y in 3..=9 {
        for x in 6..=38 {
            let p = grid.get_pixel(x, y);
            if px_luma(p) > 75.0 && px_sat(p) > 45 {
                colorful_status_row_count += 1;
                break;
            }
        }
    }

    let mut white_clock_glyph_pixels = 0usize;
    let mut clock_horiz_edges = 0usize;
    for y in 3..=9 {
        for x in 6..=35 {
            let p = grid.get_pixel(x, y);
            let l = px_luma(p);
            let sat = px_sat(p);
            let l_left = px_luma(grid.get_pixel(x - 1, y));
            let l_right = px_luma(grid.get_pixel(x + 1, y));
            let l_up = px_luma(grid.get_pixel(x, y - 1));
            let l_down = px_luma(grid.get_pixel(x, y + 1));
            let min_neighbor = l_left.min(l_right).min(l_up).min(l_down);
            if l >= 145.0 && sat <= 32 && (l - min_neighbor) >= 12.0 {
                white_clock_glyph_pixels += 1;
            }
            if (l - l_left).abs() >= 12.0 {
                clock_horiz_edges += 1;
            }
        }
    }

    let mut right_status_glyph_pixels = 0usize;
    let mut right_horiz_edges = 0usize;
    for y in 3..=9 {
        for x in 84..=115 {
            let p = grid.get_pixel(x, y);
            let l = px_luma(p);
            let sat = px_sat(p);
            let l_left = px_luma(grid.get_pixel(x - 1, y));
            let l_right = px_luma(grid.get_pixel(x + 1, y));
            let l_up = px_luma(grid.get_pixel(x, y - 1));
            let l_down = px_luma(grid.get_pixel(x, y + 1));
            let min_neighbor = l_left.min(l_right).min(l_up).min(l_down);
            if l >= 115.0 && sat <= 42 && (l - min_neighbor) >= 12.0 {
                right_status_glyph_pixels += 1;
            }
            if (l - l_left).abs() >= 12.0 {
                right_horiz_edges += 1;
            }
        }
    }

    let has_plain_white_clock_status_bar = white_clock_glyph_pixels >= 4
        && clock_horiz_edges >= 4
        && right_status_glyph_pixels >= 3
        && right_horiz_edges >= 3
        && colorful_status_row_count <= 8;

    let mut has_true_portrait_status_bar = !has_youtube_landscape_osd
        && (has_notch_colored_clock_pill
            || has_plain_white_clock_status_bar
            || is_calm_flank_status
            || (has_notch_or_island_status_bar
                && (((max_luma_left - min_luma_left) >= 36.0
                    && edge_count_left >= 4
                    && achromatic_clock_glyph_count >= 3)
                    || (has_left_clock_presence
                        && edge_count_left >= 3
                        && achromatic_clock_glyph_count >= 3
                        && bg_luma_left < 65.0
                        && colorful_left_zone_count <= 5
                        && avg_flank_sat <= 30.0
                        && blue_back_btn_count <= 3))
                && (((right_status_max_luma - right_status_min_luma) >= 36.0 && edge_count_right >= 3)
                    || (has_right_status_presence && bg_luma_left < 65.0))
                && neon_left_count <= 8
                && neon_near_island_count <= 6
                && colorful_left_zone_count < 10));

    let mut top_bright_pixels_2_20 = 0usize;
    for y in 2..=20 {
        for x in 4..=116 {
            if px_luma(grid.get_pixel(x, y)) >= 45.0 {
                top_bright_pixels_2_20 += 1;
            }
        }
    }

    // 6-Block Segment Top Pillarbox: y=2..=8 (mirror.js:758-826)
    let pillar_blocks = [(6usize, 23usize), (24, 41), (42, 59), (60, 77), (78, 95), (96, 113)];
    let mut top_pure_black_blocks = 0usize;
    let mut top_block_0_is_pure = false;
    let mut top_block_5_is_pure = false;
    let mut top_left_sum = 0.0f32;
    let mut top_left_cnt = 0usize;
    let mut top_center_sum = 0.0f32;
    let mut top_center_cnt = 0usize;
    let mut top_right_sum = 0.0f32;
    let mut top_right_cnt = 0usize;
    for (bi, &(bx1, bx2)) in pillar_blocks.iter().enumerate() {
        let mut b_sum = 0.0f32;
        let mut b_sat_sum = 0.0f32;
        let mut b_cnt = 0usize;
        let mut b_max = 0.0f32;
        let mut b_min = 255.0f32;
        for y in 2..=8 {
            for x in bx1..=bx2 {
                let p = grid.get_pixel(x, y);
                let l = px_luma(p);
                let sat = px_sat(p) as f32;
                b_sum += l;
                b_sat_sum += sat;
                b_cnt += 1;
                if l > b_max {
                    b_max = l;
                }
                if l < b_min {
                    b_min = l;
                }
                if x <= 36 {
                    top_left_sum += l;
                    top_left_cnt += 1;
                } else if x <= 83 {
                    top_center_sum += l;
                    top_center_cnt += 1;
                } else {
                    top_right_sum += l;
                    top_right_cnt += 1;
                }
            }
        }
        let b_avg = if b_cnt > 0 { b_sum / b_cnt as f32 } else { 100.0 };
        let b_sat_avg = if b_cnt > 0 { b_sat_sum / b_cnt as f32 } else { 0.0 };
        let is_valid = (b_avg <= 18.0 && b_max <= 32.0)
            || (b_avg <= 34.0 && b_max <= 44.0 && (b_max - b_min) <= 18.0 && b_sat_avg <= 25.0);
        if is_valid {
            top_pure_black_blocks += 1;
            if bi == 0 {
                top_block_0_is_pure = true;
            }
            if bi == 5 {
                top_block_5_is_pure = true;
            }
        }
    }
    let top_pillar_is_black = top_pure_black_blocks >= 5
        && top_block_0_is_pure
        && top_block_5_is_pure
        && !has_notch_colored_clock_pill;
    if top_pillar_is_black {
        has_true_portrait_status_bar = false;
    }
    let top_pillar_avg_luma = ((if top_left_cnt > 0 { top_left_sum / top_left_cnt as f32 } else { 100.0 })
        + (if top_center_cnt > 0 { top_center_sum / top_center_cnt as f32 } else { 100.0 })
        + (if top_right_cnt > 0 { top_right_sum / top_right_cnt as f32 } else { 100.0 }))
        / 3.0;

    // Bottom Pillarbox Dual Clean Corridors y=244..=252 (mirror.js:833-896)
    let mut bl_sum = 0.0f32;
    let mut bl_sat_sum = 0.0f32;
    let mut bl_cnt = 0usize;
    let mut bl_max = 0.0f32;
    let mut bl_min = 255.0f32;
    let mut bl_black = 0usize;
    for y in 244..=252 {
        for x in 24..=43 {
            let p = grid.get_pixel(x, y);
            let l = px_luma(p);
            let sat = px_sat(p);
            bl_sum += l;
            bl_sat_sum += sat as f32;
            bl_cnt += 1;
            if l <= 18.0 && sat <= 18 {
                bl_black += 1;
            }
            if l > bl_max {
                bl_max = l;
            }
            if l < bl_min {
                bl_min = l;
            }
        }
    }
    let bl_avg = if bl_cnt > 0 { bl_sum / bl_cnt as f32 } else { 100.0 };
    let bl_black_ratio = if bl_cnt > 0 { bl_black as f32 / bl_cnt as f32 } else { 0.0 };
    let bl_sat_avg = if bl_cnt > 0 { bl_sat_sum / bl_cnt as f32 } else { 0.0 };

    let mut br_sum = 0.0f32;
    let mut br_sat_sum = 0.0f32;
    let mut br_cnt = 0usize;
    let mut br_max = 0.0f32;
    let mut br_min = 255.0f32;
    let mut br_black = 0usize;
    for y in 244..=252 {
        for x in 76..=102 {
            let p = grid.get_pixel(x, y);
            let l = px_luma(p);
            let sat = px_sat(p);
            br_sum += l;
            br_sat_sum += sat as f32;
            br_cnt += 1;
            if l <= 18.0 && sat <= 18 {
                br_black += 1;
            }
            if l > br_max {
                br_max = l;
            }
            if l < br_min {
                br_min = l;
            }
        }
    }
    let br_avg = if br_cnt > 0 { br_sum / br_cnt as f32 } else { 100.0 };
    let br_black_ratio = if br_cnt > 0 { br_black as f32 / br_cnt as f32 } else { 0.0 };
    let br_sat_avg = if br_cnt > 0 { br_sat_sum / br_cnt as f32 } else { 0.0 };

    let is_left_corr_valid = (bl_avg <= 16.0 && bl_max <= 30.0 && bl_black_ratio >= 0.84)
        || (bl_avg <= 32.0 && (bl_max - bl_min) <= 20.0 && bl_sat_avg <= 25.0);
    let is_right_corr_valid = (br_avg <= 16.0 && br_max <= 30.0 && br_black_ratio >= 0.84)
        || (br_avg <= 32.0 && (br_max - br_min) <= 20.0 && br_sat_avg <= 25.0);
    let has_clean_black_bottom_corridors = is_left_corr_valid && is_right_corr_valid;

    // 6-Block Bottom Pillarbox y=238..=250 (mirror.js:898-943)
    let mut bot_black_pixels = 0usize;
    let mut bot_total_pixels = 0usize;
    let mut bot_pure_black_blocks = 0usize;
    let mut bot_left_sum = 0.0f32;
    let mut bot_left_cnt = 0usize;
    let mut bot_right_sum = 0.0f32;
    let mut bot_right_cnt = 0usize;
    for &(bx1, bx2) in &pillar_blocks {
        let mut b_sum = 0.0f32;
        let mut b_sat_sum = 0.0f32;
        let mut b_cnt = 0usize;
        let mut b_max = 0.0f32;
        let mut b_min = 255.0f32;
        for y in 238..=250 {
            for x in bx1..=bx2 {
                let p = grid.get_pixel(x, y);
                let l = px_luma(p);
                let sat = px_sat(p);
                if l <= 18.0 && sat <= 18 {
                    bot_black_pixels += 1;
                }
                bot_total_pixels += 1;
                b_sum += l;
                b_sat_sum += sat as f32;
                b_cnt += 1;
                if l > b_max {
                    b_max = l;
                }
                if l < b_min {
                    b_min = l;
                }
                if x <= 44 {
                    bot_left_sum += l;
                    bot_left_cnt += 1;
                } else if x >= 76 {
                    bot_right_sum += l;
                    bot_right_cnt += 1;
                }
            }
        }
        let b_avg = if b_cnt > 0 { b_sum / b_cnt as f32 } else { 100.0 };
        let b_sat_avg = if b_cnt > 0 { b_sat_sum / b_cnt as f32 } else { 0.0 };
        if (b_avg <= 18.0 && b_max <= 32.0)
            || (b_avg <= 34.0 && b_max <= 44.0 && (b_max - b_min) <= 20.0 && b_sat_avg <= 25.0)
        {
            bot_pure_black_blocks += 1;
        }
    }
    let bot_pillar_black_ratio = if bot_total_pixels > 0 {
        bot_black_pixels as f32 / bot_total_pixels as f32
    } else {
        0.0
    };
    let bot_pillar_is_black =
        has_clean_black_bottom_corridors || (bot_pure_black_blocks >= 4 && bot_pillar_black_ratio >= 0.74);
    let bot_pillar_avg_luma = ((if bot_left_cnt > 0 { bot_left_sum / bot_left_cnt as f32 } else { 100.0 })
        + (if bot_right_cnt > 0 { bot_right_sum / bot_right_cnt as f32 } else { 100.0 }))
        / 2.0;

    // Peak-contrast Instagram Reels Dark Chrome (mirror.js:945-984)
    let mut reels_header_min_luma = 255.0f32;
    let mut reels_header_max_luma = 0.0f32;
    let mut reels_header_bright_pixels = 0usize;
    for y in (12..=22).step_by(2) {
        for x in (12..=72).step_by(2) {
            let p = grid.get_pixel(x, y);
            let l = px_luma(p);
            if l < reels_header_min_luma {
                reels_header_min_luma = l;
            }
            if l > reels_header_max_luma {
                reels_header_max_luma = l;
            }
            if l >= 60.0 && px_sat(p) <= 35 {
                reels_header_bright_pixels += 1;
            }
        }
    }

    let mut bot_nav_dark_bg_min_luma = 255.0f32;
    let mut bot_nav_icon_max_luma = 0.0f32;
    let mut bot_nav_bright_icon_pixels = 0usize;
    for y in (233..=246).step_by(2) {
        for x in (14..=106).step_by(2) {
            let p = grid.get_pixel(x, y);
            let l = px_luma(p);
            if l < bot_nav_dark_bg_min_luma {
                bot_nav_dark_bg_min_luma = l;
            }
            if l > bot_nav_icon_max_luma {
                bot_nav_icon_max_luma = l;
            }
            if l >= 52.0 && px_sat(p) <= 35 && x <= 90 {
                bot_nav_bright_icon_pixels += 1;
            }
        }
    }

    let mut has_instagram_reels_dark_chrome = !(top_pillar_is_black
        && (has_clean_black_bottom_corridors || bot_pillar_is_black))
        && island_avg_luma < 25.0
        && bot_nav_dark_bg_min_luma <= 28.0
        && bot_nav_icon_max_luma >= 55.0
        && (bot_nav_icon_max_luma - bot_nav_dark_bg_min_luma) >= 32.0
        && bot_nav_bright_icon_pixels >= 3
        && ((left_clock_max_luma >= (bg_luma_left + 22.0)
            && (left_clock_max_luma - left_clock_min_luma) >= 24.0)
            || (reels_header_max_luma >= 65.0
                && (reels_header_max_luma - reels_header_min_luma) >= 35.0
                && reels_header_bright_pixels >= 3));

    // Instagram / Threads Dark Mode Chrome: 5 bottom icon clusters + header white glyphs (mirror.js:986-1027)
    let mut bot_nav_sum = 0.0f32;
    let mut bot_nav_sat_sum = 0.0f32;
    let mut bot_nav_pixel_cnt = 0usize;
    for y in (232..=255).step_by(2) {
        for x in (6..=114).step_by(4) {
            let p = grid.get_pixel(x, y);
            bot_nav_sum += px_luma(p);
            bot_nav_sat_sum += px_sat(p) as f32;
            bot_nav_pixel_cnt += 1;
        }
    }
    let bot_nav_avg_luma = if bot_nav_pixel_cnt > 0 {
        bot_nav_sum / bot_nav_pixel_cnt as f32
    } else {
        100.0
    };
    let bot_nav_avg_sat = if bot_nav_pixel_cnt > 0 {
        bot_nav_sat_sum / bot_nav_pixel_cnt as f32
    } else {
        100.0
    };

    let mut bot_nav_icon_clusters = 0usize;
    for bx in [16usize, 38, 60, 82, 104] {
        let mut has_icon = false;
        'cluster: for y in 235..=246 {
            for x in (bx - 6)..=(bx + 6) {
                let p = grid.get_pixel(x, y);
                if px_luma(p) >= 120.0 && px_sat(p) <= 28 {
                    has_icon = true;
                    break 'cluster;
                }
            }
        }
        if has_icon {
            bot_nav_icon_clusters += 1;
        }
    }

    let mut header_logo_white_glyphs = 0usize;
    for y in 12..=26 {
        for x in 6..=48 {
            let p = grid.get_pixel(x, y);
            if px_luma(p) >= 120.0 && px_sat(p) <= 28 {
                header_logo_white_glyphs += 1;
            }
        }
    }

    // Bottom Navigation Bar Probes (mirror.js:1030-1085)
    let mut left_tab_sum = 0.0f32;
    let mut left_tab_cnt = 0usize;
    let mut right_tab_sum = 0.0f32;
    let mut right_tab_cnt = 0usize;
    for y in (234..=246).step_by(2) {
        for x in (14..=40).step_by(4) {
            left_tab_sum += px_luma(grid.get_pixel(x, y));
            left_tab_cnt += 1;
        }
        for x in (80..=106).step_by(4) {
            right_tab_sum += px_luma(grid.get_pixel(x, y));
            right_tab_cnt += 1;
        }
    }
    let left_tab_avg_luma = if left_tab_cnt > 0 { left_tab_sum / left_tab_cnt as f32 } else { 100.0 };
    let right_tab_avg_luma = if right_tab_cnt > 0 { right_tab_sum / right_tab_cnt as f32 } else { 100.0 };

    let mut bnl_sum = 0.0f32;
    let mut bnl_sat = 0.0f32;
    let mut bnl_cnt = 0usize;
    let mut bnc_sum = 0.0f32;
    let mut bnc_sat = 0.0f32;
    let mut bnc_cnt = 0usize;
    let mut bnr_sum = 0.0f32;
    let mut bnr_sat = 0.0f32;
    let mut bnr_cnt = 0usize;
    for y in (230..=248).step_by(3) {
        for x in (10..=38).step_by(4) {
            let p = grid.get_pixel(x, y);
            bnl_sum += px_luma(p);
            bnl_sat += px_sat(p) as f32;
            bnl_cnt += 1;
        }
        for x in (46..=74).step_by(4) {
            let p = grid.get_pixel(x, y);
            bnc_sum += px_luma(p);
            bnc_sat += px_sat(p) as f32;
            bnc_cnt += 1;
        }
        for x in (82..=110).step_by(4) {
            let p = grid.get_pixel(x, y);
            bnr_sum += px_luma(p);
            bnr_sat += px_sat(p) as f32;
            bnr_cnt += 1;
        }
    }
    let bot_nav_left_avg_luma = if bnl_cnt > 0 { bnl_sum / bnl_cnt as f32 } else { 0.0 };
    let bot_nav_left_avg_sat = if bnl_cnt > 0 { bnl_sat / bnl_cnt as f32 } else { 100.0 };
    let bot_nav_center_avg_luma = if bnc_cnt > 0 { bnc_sum / bnc_cnt as f32 } else { 0.0 };
    let bot_nav_center_avg_sat = if bnc_cnt > 0 { bnc_sat / bnc_cnt as f32 } else { 100.0 };
    let bot_nav_right_avg_luma = if bnr_cnt > 0 { bnr_sum / bnr_cnt as f32 } else { 0.0 };
    let bot_nav_right_avg_sat = if bnr_cnt > 0 { bnr_sat / bnr_cnt as f32 } else { 100.0 };

    let has_bottom_white_nav_bar = bot_nav_left_avg_luma >= 165.0
        && bot_nav_center_avg_luma >= 165.0
        && bot_nav_right_avg_luma >= 165.0
        && bot_nav_left_avg_sat <= 32.0
        && bot_nav_center_avg_sat <= 32.0
        && bot_nav_right_avg_sat <= 32.0;

    // Active Video Body & Studio Presenter Probes (mirror.js:1087-1193)
    let mut cv_sum = 0.0f32;
    let mut cv_cnt = 0usize;
    let mut center_video_max_luma = 0.0f32;
    let mut center_video_min_luma = 255.0f32;
    for y in (35..=220).step_by(8) {
        for x in (20..=100).step_by(8) {
            let l = px_luma(grid.get_pixel(x, y));
            cv_sum += l;
            cv_cnt += 1;
            if l > center_video_max_luma {
                center_video_max_luma = l;
            }
            if l < center_video_min_luma {
                center_video_min_luma = l;
            }
        }
    }
    let active_video_avg_luma = if cv_cnt > 0 { cv_sum / cv_cnt as f32 } else { center_video_avg_luma };

    let zone_top_active = check_video_zone_active(grid, 22, 88);
    let zone_mid_active = check_video_zone_active(grid, 89, 162);
    let zone_bot_active = check_video_zone_active(grid, 163, 230);

    let mut lh_bright_pix = 0usize;
    let mut lh_sum = 0.0f32;
    let mut lh_cnt = 0usize;
    let mut lh_max = 0.0f32;
    let mut lh_min = 255.0f32;
    for y in (135..=236).step_by(3) {
        for x in (12..=85).step_by(3) {
            let l = px_luma(grid.get_pixel(x, y));
            lh_sum += l;
            lh_cnt += 1;
            if l >= 32.0 {
                lh_bright_pix += 1;
            }
            if l > lh_max {
                lh_max = l;
            }
            if l < lh_min {
                lh_min = l;
            }
        }
    }
    let lower_half_avg = if lh_cnt > 0 { lh_sum / lh_cnt as f32 } else { 0.0 };
    let lower_half_video_active =
        lh_bright_pix >= 10 && ((lh_max - lh_min) >= 25.0 || lower_half_avg >= 28.0);

    let mut zone_mid_bright_pixels = 0usize;
    for y in (72..=185).step_by(3) {
        for x in (12..=108).step_by(3) {
            if px_luma(grid.get_pixel(x, y)) >= 36.0 {
                zone_mid_bright_pixels += 1;
            }
        }
    }

    let mut mlf_sum = 0.0f32;
    let mut mlf_cnt = 0usize;
    let mut mlf_black = 0usize;
    let mut mrf_sum = 0.0f32;
    let mut mrf_cnt = 0usize;
    let mut mrf_black = 0usize;
    for y in (75..=180).step_by(4) {
        for x in (4..=30).step_by(4) {
            let l = px_luma(grid.get_pixel(x, y));
            mlf_sum += l;
            mlf_cnt += 1;
            if l <= 18.0 {
                mlf_black += 1;
            }
        }
        for x in (90..=116).step_by(4) {
            let l = px_luma(grid.get_pixel(x, y));
            mrf_sum += l;
            mrf_cnt += 1;
            if l <= 18.0 {
                mrf_black += 1;
            }
        }
    }
    let mid_left_flank_avg_luma = if mlf_cnt > 0 { mlf_sum / mlf_cnt as f32 } else { 0.0 };
    let mid_left_flank_black_ratio = if mlf_cnt > 0 { mlf_black as f32 / mlf_cnt as f32 } else { 0.0 };
    let mid_right_flank_avg_luma = if mrf_cnt > 0 { mrf_sum / mrf_cnt as f32 } else { 0.0 };
    let mid_right_flank_black_ratio = if mrf_cnt > 0 { mrf_black as f32 / mrf_cnt as f32 } else { 0.0 };

    let mut mid_center_warm_wood_or_skin_pixels = 0usize;
    for y in (75..=180).step_by(4) {
        for x in (20..=100).step_by(4) {
            let p = grid.get_pixel(x, y);
            let l = px_luma(p);
            let sat = px_sat(p);
            if sat >= 20
                && p.g >= 35
                && p.r as i32 > (p.b as i32 + 12)
                && p.g >= p.b
                && l >= 35.0
            {
                mid_center_warm_wood_or_skin_pixels += 1;
            }
        }
    }

    let is_center_studio_video_active = zone_mid_bright_pixels >= 20
        && mid_right_flank_avg_luma >= 22.0
        && mid_center_warm_wood_or_skin_pixels >= 12;
    let has_active_fullscreen_video_body = (zone_top_active && zone_bot_active)
        || is_center_studio_video_active
        || (lower_half_video_active && (has_preliminary_side_bar || is_currently_landscape))
        || (zone_mid_active && (zone_top_active || zone_bot_active) && mid_center_warm_wood_or_skin_pixels >= 14);

    // YouTube Portrait Player Layout Detection (mirror.js:1194-1271)
    let mut top_player_sum = 0.0f32;
    let mut top_player_cnt = 0usize;
    let mut top_player_vivid_pixels = 0usize;
    for y in (16..=80).step_by(4) {
        for x in (8..=112).step_by(4) {
            let p = grid.get_pixel(x, y);
            let l = px_luma(p);
            top_player_sum += l;
            top_player_cnt += 1;
            if px_sat(p) >= 35 && l >= 45.0 {
                top_player_vivid_pixels += 1;
            }
        }
    }
    let top_player_avg_luma = if top_player_cnt > 0 { top_player_sum / top_player_cnt as f32 } else { 0.0 };

    let mut div_l_sum = 0.0f32;
    let mut div_l_cnt = 0usize;
    let mut div_l_dark = 0usize;
    let mut div_r_sum = 0.0f32;
    let mut div_r_cnt = 0usize;
    let mut div_r_dark = 0usize;
    for y in (88..=118).step_by(3) {
        for x in (8..=54).step_by(3) {
            let l = px_luma(grid.get_pixel(x, y));
            div_l_sum += l;
            div_l_cnt += 1;
            if l <= 34.0 {
                div_l_dark += 1;
            }
        }
        for x in (58..=108).step_by(3) {
            let l = px_luma(grid.get_pixel(x, y));
            div_r_sum += l;
            div_r_cnt += 1;
            if l <= 34.0 {
                div_r_dark += 1;
            }
        }
    }
    let divider_left_avg_luma = if div_l_cnt > 0 { div_l_sum / div_l_cnt as f32 } else { 100.0 };
    let divider_left_dark_ratio = if div_l_cnt > 0 { div_l_dark as f32 / div_l_cnt as f32 } else { 0.0 };
    let divider_right_avg_luma = if div_r_cnt > 0 { div_r_sum / div_r_cnt as f32 } else { 100.0 };
    let divider_right_dark_ratio = if div_r_cnt > 0 { div_r_dark as f32 / div_r_cnt as f32 } else { 0.0 };

    let mut ltt_dark = 0usize;
    let mut ltt_sum = 0.0f32;
    let mut ltt_sat = 0.0f32;
    let mut ltt_cnt = 0usize;
    for y in (94..=230).step_by(4) {
        for x in (8..=85).step_by(4) {
            let p = grid.get_pixel(x, y);
            let l = px_luma(p);
            let sat = px_sat(p);
            ltt_sum += l;
            ltt_sat += sat as f32;
            ltt_cnt += 1;
            if l <= 38.0 && sat <= 18 {
                ltt_dark += 1;
            }
        }
    }
    let lower_two_thirds_avg_luma = if ltt_cnt > 0 { ltt_sum / ltt_cnt as f32 } else { 100.0 };
    let lower_two_thirds_avg_sat = if ltt_cnt > 0 { ltt_sat / ltt_cnt as f32 } else { 100.0 };
    let lower_two_thirds_dark_ratio = if ltt_cnt > 0 { ltt_dark as f32 / ltt_cnt as f32 } else { 0.0 };

    let mut mid_right_warm_skin_pixels = 0usize;
    for y in (85..=175).step_by(4) {
        for x in (45..=105).step_by(4) {
            let p = grid.get_pixel(x, y);
            if px_sat(p) >= 20
                && p.g >= 35
                && p.r as i32 > (p.b as i32 + 12)
                && p.g >= p.b
                && px_luma(p) >= 35.0
            {
                mid_right_warm_skin_pixels += 1;
            }
        }
    }

    let is_youtube_portrait_player_layout = !has_preliminary_side_bar
        && !is_currently_landscape
        && !(top_pillar_is_black && bot_pillar_is_black && has_active_fullscreen_video_body)
        && avg_flank_sat <= 35.0
        && vivid_flank_pixel_count <= 8
        && (top_player_avg_luma >= 38.0 || top_player_vivid_pixels >= 18)
        && lower_two_thirds_dark_ratio >= 0.60
        && lower_two_thirds_avg_sat <= 18.0
        && mid_right_warm_skin_pixels <= 12
        && (top_player_avg_luma - lower_two_thirds_avg_luma) >= 20.0
        && ((divider_left_avg_luma <= 34.0 && divider_right_avg_luma <= 34.0)
            || (divider_left_dark_ratio >= 0.40 && divider_right_dark_ratio >= 0.40)
            || (divider_right_avg_luma - divider_left_avg_luma).abs() <= 35.0);

    // App Launch Splash Screen Detector (mirror.js:1273-1307)
    let mut us_sum = 0.0f32;
    let mut us_cnt = 0usize;
    let mut us_bright = 0usize;
    for y in (18..=70).step_by(4) {
        for x in (6..=85).step_by(4) {
            let l = px_luma(grid.get_pixel(x, y));
            us_sum += l;
            us_cnt += 1;
            if l > 38.0 {
                us_bright += 1;
            }
        }
    }
    let upper_splash_avg_luma = if us_cnt > 0 { us_sum / us_cnt as f32 } else { 0.0 };

    let mut ls_sum = 0.0f32;
    let mut ls_cnt = 0usize;
    let mut ls_bright = 0usize;
    for y in (182..=232).step_by(4) {
        for x in (6..=85).step_by(4) {
            let l = px_luma(grid.get_pixel(x, y));
            ls_sum += l;
            ls_cnt += 1;
            if l > 38.0 {
                ls_bright += 1;
            }
        }
    }
    let lower_splash_avg_luma = if ls_cnt > 0 { ls_sum / ls_cnt as f32 } else { 0.0 };

    let is_app_splash_screen = !has_preliminary_side_bar
        && !is_currently_landscape
        && upper_splash_avg_luma <= 14.0
        && lower_splash_avg_luma <= 14.0
        && us_bright <= 3
        && ls_bright <= 3
        && mid_left_flank_avg_luma <= 14.0
        && mid_left_flank_black_ratio >= 0.85
        && mid_right_flank_black_ratio >= 0.72
        && mid_center_warm_wood_or_skin_pixels <= 6;

    // Pure Black Wings y=12..46 + Comment Sheet Detector (mirror.js:1309-1421)
    let mut lw_sum = 0.0f32;
    let mut lw_cnt = 0usize;
    let mut lw_max = 0.0f32;
    let mut lw_black = 0usize;
    let mut rw_sum = 0.0f32;
    let mut rw_cnt = 0usize;
    let mut rw_max = 0.0f32;
    let mut rw_black = 0usize;
    let mut center_thumb_12_46_max = 0.0f32;
    for y in (12..=46).step_by(2) {
        for x in (2..=32).step_by(2) {
            let l = px_luma(grid.get_pixel(x, y));
            lw_sum += l;
            lw_cnt += 1;
            if l <= 14.0 {
                lw_black += 1;
            }
            if l > lw_max {
                lw_max = l;
            }
        }
        for x in (88..=118).step_by(2) {
            let l = px_luma(grid.get_pixel(x, y));
            rw_sum += l;
            rw_cnt += 1;
            if l <= 14.0 {
                rw_black += 1;
            }
            if l > rw_max {
                rw_max = l;
            }
        }
        for x in (37..=83).step_by(2) {
            let l = px_luma(grid.get_pixel(x, y));
            if l > center_thumb_12_46_max {
                center_thumb_12_46_max = l;
            }
        }
    }
    let has_top_black_wings_12_46 = (if lw_cnt > 0 { lw_sum / lw_cnt as f32 } else { 100.0 }) <= 10.0
        && (if rw_cnt > 0 { rw_sum / rw_cnt as f32 } else { 100.0 }) <= 10.0
        && lw_max <= 24.0
        && rw_max <= 24.0
        && (if lw_cnt > 0 { lw_black as f32 / lw_cnt as f32 } else { 0.0 }) >= 0.90
        && (if rw_cnt > 0 { rw_black as f32 / rw_cnt as f32 } else { 0.0 }) >= 0.90;

    let mut header_48_78_max = 0.0f32;
    let mut header_48_78_bright = 0usize;
    for y in (48..=78).step_by(2) {
        for x in (6..=114).step_by(2) {
            let p = grid.get_pixel(x, y);
            let l = px_luma(p);
            if l > header_48_78_max {
                header_48_78_max = l;
            }
            if l >= 55.0 && px_sat(p) <= 30 {
                header_48_78_bright += 1;
            }
        }
    }

    let mut dark_sheet_bg_cnt = 0usize;
    let mut comment_text_px = 0usize;
    let mut sheet_cnt = 0usize;
    for y in (74..=230).step_by(5) {
        for x in (8..=85).step_by(5) {
            let p = grid.get_pixel(x, y);
            let l = px_luma(p);
            let sat = px_sat(p);
            sheet_cnt += 1;
            if l <= 40.0 && sat <= 18 {
                dark_sheet_bg_cnt += 1;
            }
            if l >= 65.0 && sat <= 28 {
                comment_text_px += 1;
            }
        }
    }
    let dark_sheet_bg_ratio = if sheet_cnt > 0 { dark_sheet_bg_cnt as f32 / sheet_cnt as f32 } else { 0.0 };

    let mut rh_sum = 0.0f32;
    let mut rh_cnt = 0usize;
    for y in (80..=225).step_by(5) {
        for x in (96..=114).step_by(3) {
            rh_sum += px_luma(grid.get_pixel(x, y));
            rh_cnt += 1;
        }
    }
    let right_heart_col_avg_luma = if rh_cnt > 0 { rh_sum / rh_cnt as f32 } else { 0.0 };

    let mut video_center_warm_pixels = 0usize;
    for y in (74..=225).step_by(4) {
        for x in (22..=88).step_by(4) {
            let p = grid.get_pixel(x, y);
            if px_sat(p) >= 25 && p.r as i32 > (p.b as i32 + 15) && px_luma(p) >= 40.0 {
                video_center_warm_pixels += 1;
            }
        }
    }

    let mut ca_dark = 0usize;
    let mut ca_warm = 0usize;
    let mut ca_tot = 0usize;
    for y in (90..=225).step_by(3) {
        for x in (12..=100).step_by(3) {
            let p = grid.get_pixel(x, y);
            let l = px_luma(p);
            let sat = px_sat(p);
            ca_tot += 1;
            if l <= 28.0 && sat <= 20 {
                ca_dark += 1;
            }
            if l >= 45.0 || (sat >= 25 && p.r > p.b) {
                ca_warm += 1;
            }
        }
    }
    let comment_sheet_dark_ratio = if ca_tot > 0 { ca_dark as f32 / ca_tot as f32 } else { 0.0 };
    let comment_area_warm_ratio = if ca_tot > 0 { ca_warm as f32 / ca_tot as f32 } else { 0.0 };

    let is_portrait_comment_sheet_screen = !has_preliminary_side_bar
        && comment_sheet_dark_ratio >= 0.65
        && comment_area_warm_ratio <= 0.30
        && has_top_black_wings_12_46
        && (center_thumb_12_46_max >= 16.0 || header_48_78_max >= 55.0)
        && header_48_78_bright >= 4
        && dark_sheet_bg_ratio >= 0.48
        && comment_text_px >= 6
        && right_heart_col_avg_luma <= 38.0
        && video_center_warm_pixels <= 24;

    // YouTube Portrait Watch Layout & Lower White Card (mirror.js:1423-1481)
    let mut lc_sum = 0.0f32;
    let mut lc_sat = 0.0f32;
    let mut lc_cnt = 0usize;
    let mut lc_white = 0usize;
    for y in (138..=215).step_by(4) {
        for x in (10..=110).step_by(4) {
            let p = grid.get_pixel(x, y);
            let l = px_luma(p);
            let sat = px_sat(p);
            lc_sum += l;
            lc_sat += sat as f32;
            lc_cnt += 1;
            if l >= 170.0 && sat <= 32 {
                lc_white += 1;
            }
        }
    }
    let lower_card_avg_luma = if lc_cnt > 0 { lc_sum / lc_cnt as f32 } else { 0.0 };
    let lower_card_avg_sat = if lc_cnt > 0 { lc_sat / lc_cnt as f32 } else { 100.0 };
    let lower_card_white_ratio = if lc_cnt > 0 { lc_white as f32 / lc_cnt as f32 } else { 0.0 };

    let mut mdb_sum = 0.0f32;
    let mut mdb_cnt = 0usize;
    let mut mdb_dark = 0usize;
    for y in (84..=135).step_by(4) {
        for x in (8..=85).step_by(4) {
            let p = grid.get_pixel(x, y);
            let l = px_luma(p);
            mdb_sum += l;
            mdb_cnt += 1;
            if l <= 38.0 && px_sat(p) <= 18 {
                mdb_dark += 1;
            }
        }
    }
    let mid_divider_band_avg_luma = if mdb_cnt > 0 { mdb_sum / mdb_cnt as f32 } else { 100.0 };
    let mid_divider_band_dark_ratio = if mdb_cnt > 0 { mdb_dark as f32 / mdb_cnt as f32 } else { 0.0 };

    let has_yt_4layer_portrait_watch_layout = !has_preliminary_side_bar
        && top_player_avg_luma >= 38.0
        && mid_divider_band_avg_luma <= 38.0
        && mid_divider_band_dark_ratio >= 0.65
        && lower_card_avg_luma >= 125.0
        && (lower_card_white_ratio >= 0.45 || lower_card_avg_sat <= 32.0);

    let has_youtube_dark_lower_white_card = !has_preliminary_side_bar
        && !is_currently_landscape
        && top_player_avg_luma >= 38.0
        && !(top_pillar_is_black
            && (has_clean_black_bottom_corridors || bot_pillar_is_black)
            && top_player_avg_luma < 35.0)
        && (has_yt_4layer_portrait_watch_layout
            || (bg_luma_left < 50.0
                && bg_luma_right < 50.0
                && mid_divider_band_avg_luma <= 38.0
                && mid_divider_band_dark_ratio >= 0.65
                && lower_card_avg_luma >= 125.0
                && lower_card_avg_sat <= 32.0));

    // 6-Block Pillarbox + Dual Corridors YouTube Fullscreen (mirror.js:1483-1500)
    let has_dual_black_pillarbox = top_pillar_is_black
        && bot_pillar_is_black
        && has_active_fullscreen_video_body
        && !has_notch_colored_clock_pill
        && !has_true_portrait_status_bar
        && !is_portrait_comment_sheet_screen
        && !is_youtube_portrait_player_layout
        && !is_app_splash_screen
        && !has_youtube_dark_lower_white_card;

    let has_active_center_video = (active_video_avg_luma >= 18.0
        && (active_video_avg_luma - top_pillar_avg_luma) >= 12.0
        && (active_video_avg_luma - bot_pillar_avg_luma) >= 12.0)
        || (center_video_max_luma - center_video_min_luma) >= 55.0
        || has_active_fullscreen_video_body;

    // TikTok Center [+] Button (mirror.js:1502-1520)
    let mut center_plus_max_luma = 0.0f32;
    let mut center_plus_bright_count = 0usize;
    for y in (233..=245).step_by(2) {
        for x in (52..=68).step_by(2) {
            let l = px_luma(grid.get_pixel(x, y));
            if l > center_plus_max_luma {
                center_plus_max_luma = l;
            }
            if l >= 165.0 && (l - left_tab_avg_luma) >= 70.0 {
                center_plus_bright_count += 1;
            }
        }
    }
    let mut has_tiktok_center_plus_button = !has_dual_black_pillarbox
        && !(top_pillar_is_black && (has_clean_black_bottom_corridors || bot_pillar_is_black))
        && top_bright_pixels_2_20 >= 5
        && left_tab_avg_luma <= 36.0
        && right_tab_avg_luma <= 36.0
        && center_plus_max_luma >= 180.0
        && center_plus_bright_count >= 4;

    // Google Translate Dark Mode (mirror.js:1522-1574)
    let mut translate_bg_cnt = 0usize;
    let mut mb_sum = 0.0f32;
    let mut mb_sat = 0.0f32;
    let mut mb_cnt = 0usize;
    let mut mb_warm = 0usize;
    for y in (46..=152).step_by(6) {
        for x in (8..=85).step_by(6) {
            let p = grid.get_pixel(x, y);
            let l = px_luma(p);
            let sat = px_sat(p);
            mb_sum += l;
            mb_sat += sat as f32;
            mb_cnt += 1;
            if (14.0..=48.0).contains(&l) && sat <= 20 {
                translate_bg_cnt += 1;
            }
            if sat >= 28 && p.r as i32 > (p.b as i32 + 15) {
                mb_warm += 1;
            }
        }
    }
    let translate_input_bg_ratio = if mb_cnt > 0 { translate_bg_cnt as f32 / mb_cnt as f32 } else { 0.0 };
    let mid_box_avg_luma = if mb_cnt > 0 { mb_sum / mb_cnt as f32 } else { 100.0 };
    let mid_box_avg_sat = if mb_cnt > 0 { mb_sat / mb_cnt as f32 } else { 100.0 };

    let mut mic_blue_pixels = 0usize;
    let mut total_blue_btn_pixels = 0usize;
    for y in (188..=244).step_by(2) {
        for x in (12..=108).step_by(2) {
            let p = grid.get_pixel(x, y);
            if p.b >= 110 && p.g >= 65 && (p.b as i32 - p.r as i32) >= 18 && px_luma(p) >= 50.0 {
                total_blue_btn_pixels += 1;
                if (42..=78).contains(&x) {
                    mic_blue_pixels += 1;
                }
            }
        }
    }

    let mut translate_header_bright_pixels = 0usize;
    for y in (11..=42).step_by(3) {
        for x in (6..=112).step_by(4) {
            let p = grid.get_pixel(x, y);
            if px_luma(p) >= 70.0 && px_sat(p) <= 30 {
                translate_header_bright_pixels += 1;
            }
        }
    }

    let is_google_translate_dark = !has_dual_black_pillarbox
        && top_bright_pixels_2_20 >= 4
        && translate_input_bg_ratio >= 0.65
        && (14.0..=46.0).contains(&mid_box_avg_luma)
        && mid_box_avg_sat <= 18.0
        && mb_warm <= 4
        && (mic_blue_pixels >= 5 || total_blue_btn_pixels >= 10 || translate_header_bright_pixels >= 6);

    let mut has_dark_portrait_app_chrome = is_google_translate_dark || is_portrait_comment_sheet_screen;

    // 4. Portrait Bottom Home Bar (mirror.js:1580-1661)
    let mut max_bottom_run = 0usize;
    for y in 248..=256 {
        let mut cur_run = 0usize;
        for x in 40..=80 {
            let bar_luma = px_luma(grid.get_pixel(x, y));
            let bg_luma = px_luma(grid.get_pixel(x, y - 5));
            if bar_luma > 140.0 && (bar_luma - bg_luma) > 25.0 {
                cur_run += 1;
                if cur_run > max_bottom_run {
                    max_bottom_run = cur_run;
                }
            } else {
                cur_run = 0;
            }
        }
    }

    let mut has_dimmed_dark_home_bar = false;
    let mut ac_sum = 0.0f32;
    let mut ac_cnt = 0usize;
    for ay in 238..=241 {
        for ax in (52..=68).step_by(2) {
            ac_sum += px_luma(grid.get_pixel(ax, ay));
            ac_cnt += 1;
        }
    }
    let above_center_luma = if ac_cnt > 0 { ac_sum / ac_cnt as f32 } else { 0.0 };
    for y in 246..=255 {
        let avg_row_band = |x1: usize, x2: usize, step: usize| -> f32 {
            let mut s = 0.0f32;
            let mut c = 0usize;
            for x in (x1..=x2).step_by(step) {
                s += px_luma(grid.get_pixel(x, y));
                c += 1;
            }
            if c > 0 { s / c as f32 } else { 0.0 }
        };
        let row_left_luma = avg_row_band(8, 28, 4);
        let row_right_luma = avg_row_band(92, 112, 4);
        let seg_left_luma = avg_row_band(40, 50, 2);
        let seg_mid_luma = avg_row_band(54, 66, 2);
        let seg_right_luma = avg_row_band(70, 80, 2);
        if row_left_luma <= 28.0
            && row_right_luma <= 28.0
            && seg_left_luma >= 45.0
            && seg_mid_luma >= 45.0
            && seg_right_luma >= 45.0
            && (seg_left_luma - seg_mid_luma).abs() <= 25.0
            && (seg_right_luma - seg_mid_luma).abs() <= 25.0
            && (seg_mid_luma - row_left_luma) >= 20.0
            && (seg_mid_luma - row_right_luma) >= 20.0
            && (seg_mid_luma - above_center_luma) >= 18.0
        {
            has_dimmed_dark_home_bar = true;
            break;
        }
    }
    if top_pillar_is_black && (has_clean_black_bottom_corridors || bot_pillar_is_black) {
        has_dimmed_dark_home_bar = false;
    }
    let mut has_portrait_bottom_bar = !(top_pillar_is_black
        && (has_clean_black_bottom_corridors || bot_pillar_is_black))
        && (max_bottom_run >= 22 || has_dimmed_dark_home_bar);

    // 5. Landscape Side Home Bar Detection on Left Edge ONLY (mirror.js:1663-1769)
    let mut left_edge_strict = false;
    if !has_ios_home_screen_dock
        && !has_bottom_white_nav_bar
        && !has_tiktok_center_plus_button
        && !has_instagram_reels_dark_chrome
        && !(bot_nav_avg_luma <= 45.0 && bot_nav_avg_sat <= 18.0 && bot_nav_icon_clusters >= 3)
        && !(bg_luma_left > 180.0 && bg_luma_right > 180.0)
    {
        let avg_box = |x1: usize, y1: usize, x2: usize, y2: usize, sx: usize, sy: usize| -> f32 {
            let mut s = 0.0f32;
            let mut c = 0usize;
            for y in (y1..=y2).step_by(sy) {
                for x in (x1..=x2).step_by(sx) {
                    s += px_luma(grid.get_pixel(x, y));
                    c += 1;
                }
            }
            if c > 0 { s / c as f32 } else { 0.0 }
        };
        let inner_col_avg_luma = avg_box(10, 95, 18, 165, 2, 2);
        let top_cap_edge_luma = avg_box(1, 62, 3, 80, 1, 2);
        let bot_cap_edge_luma = avg_box(1, 180, 3, 198, 1, 2);
        let near_inner_col_luma = avg_box(5, 105, 7, 155, 1, 2);

        for x_edge in 1..=4 {
            let mut mid_sum = 0.0f32;
            let mut sat_sum = 0.0f32;
            for y in 95..=165 {
                let p = grid.get_pixel(x_edge, y);
                mid_sum += px_luma(p);
                sat_sum += px_sat(p) as f32;
            }
            let bar_mid_luma = mid_sum / 71.0;
            let bar_avg_sat = sat_sum / 71.0;

            if (bar_mid_luma - inner_col_avg_luma_98_162) < 32.0 {
                continue;
            }
            if (bar_mid_luma - top_cap_edge_luma) < 24.0 {
                continue;
            }
            if (bar_mid_luma - bot_cap_edge_luma) < 24.0 {
                continue;
            }
            if (bar_mid_luma - near_inner_col_luma) < 22.0 {
                continue;
            }

            let top_cap_luma: f32 =
                (68..=78).map(|y| px_luma(grid.get_pixel(x_edge, y))).sum::<f32>() / 11.0;
            let bot_cap_luma: f32 =
                (182..=192).map(|y| px_luma(grid.get_pixel(x_edge, y))).sum::<f32>() / 11.0;

            if (bar_mid_luma - top_cap_luma >= 25.0)
                && (bar_mid_luma - bot_cap_luma >= 25.0)
                && (bar_mid_luma - inner_col_avg_luma >= 25.0)
                && bar_mid_luma >= 118.0
                && bar_avg_sat <= 25.0
            {
                let mut run_len = 0usize;
                let mut prev_l = -1.0f32;
                let mut max_dev = 0.0f32;
                let mut dark_contrast_count = 0usize;
                for y in 88..=172 {
                    let bar_luma = px_luma(grid.get_pixel(x_edge, y));
                    if bar_luma >= 118.0 {
                        if run_len == 0 {
                            run_len = 1;
                            prev_l = bar_luma;
                            max_dev = 0.0;
                            dark_contrast_count = 0;
                        } else {
                            let dev = (bar_luma - prev_l).abs();
                            if dev > max_dev {
                                max_dev = dev;
                            }
                            prev_l = bar_luma;
                            run_len += 1;
                        }
                        let inner_luma = px_luma(grid.get_pixel((x_edge + 6).min(GRID_WIDTH - 1), y));
                        if (bar_luma - inner_luma) >= 22.0 {
                            dark_contrast_count += 1;
                        }
                    } else {
                        if run_len >= 28
                            && max_dev < 10.0
                            && (dark_contrast_count as f32 / run_len as f32) >= 0.70
                        {
                            left_edge_strict = true;
                            break;
                        }
                        run_len = 0;
                        prev_l = -1.0;
                    }
                }
                if run_len >= 28
                    && max_dev < 10.0
                    && (dark_contrast_count as f32 / run_len as f32) >= 0.70
                {
                    left_edge_strict = true;
                    break;
                }
            }
        }
    }
    let mut has_landscape_side_bar = left_edge_strict;

    let has_instagram_dark_mode_chrome = !has_landscape_side_bar
        && !has_youtube_landscape_osd
        && bot_nav_avg_luma <= 45.0
        && bot_nav_avg_sat <= 18.0
        && (has_portrait_bottom_bar || bot_nav_icon_clusters >= 3)
        && (has_true_portrait_status_bar
            || header_logo_white_glyphs >= 8
            || reels_header_bright_pixels >= 6);

    // 6. Folder / Overlay Blur & Light Portrait Chrome & Control Center (mirror.js:1780-1904)
    let mut top_blur_sum = 0.0f32;
    let mut top_blur_sat = 0.0f32;
    let mut top_blur_cnt = 0usize;
    let mut top_blur_outer_vivid = 0usize;
    for y in (14..=48).step_by(4) {
        for x in (8..=112).step_by(4) {
            let p = grid.get_pixel(x, y);
            let l = px_luma(p);
            let sat = px_sat(p);
            top_blur_sum += l;
            top_blur_sat += sat as f32;
            top_blur_cnt += 1;
            if sat >= 45 && l >= 40.0 {
                top_blur_outer_vivid += 1;
            }
        }
    }
    let top_blur_avg_luma = if top_blur_cnt > 0 { top_blur_sum / top_blur_cnt as f32 } else { 0.0 };
    let top_blur_avg_sat = if top_blur_cnt > 0 { top_blur_sat / top_blur_cnt as f32 } else { 100.0 };

    let mut bot_blur_sum = 0.0f32;
    let mut bot_blur_sat = 0.0f32;
    let mut bot_blur_cnt = 0usize;
    let mut bot_blur_outer_vivid = 0usize;
    for y in (198..=238).step_by(4) {
        for x in (8..=112).step_by(4) {
            let p = grid.get_pixel(x, y);
            let l = px_luma(p);
            let sat = px_sat(p);
            bot_blur_sum += l;
            bot_blur_sat += sat as f32;
            bot_blur_cnt += 1;
            if sat >= 45 && l >= 40.0 {
                bot_blur_outer_vivid += 1;
            }
        }
    }
    let bot_blur_avg_luma = if bot_blur_cnt > 0 { bot_blur_sum / bot_blur_cnt as f32 } else { 0.0 };
    let bot_blur_avg_sat = if bot_blur_cnt > 0 { bot_blur_sat / bot_blur_cnt as f32 } else { 100.0 };
    let folder_outer_vivid_pixels = top_blur_outer_vivid + bot_blur_outer_vivid;

    let mut folder_center_min_luma = 255.0f32;
    let mut folder_center_max_luma = 0.0f32;
    for y in (70..=180).step_by(4) {
        for x in (20..=100).step_by(4) {
            let l = px_luma(grid.get_pixel(x, y));
            if l < folder_center_min_luma {
                folder_center_min_luma = l;
            }
            if l > folder_center_max_luma {
                folder_center_max_luma = l;
            }
        }
    }

    let has_top_white_chrome = (bg_luma_left >= 160.0
        && bg_luma_right >= 160.0
        && (bg_luma_left - bg_luma_right).abs() <= 30.0
        && avg_flank_sat <= 35.0)
        || (top_white_pixel_count >= 35 && avg_flank_sat <= 35.0);
    let mut has_light_portrait_app_chrome =
        has_top_white_chrome || has_bottom_white_nav_bar || has_youtube_dark_lower_white_card;

    // Control Center probes
    let mut shifted_min = 255.0f32;
    let mut shifted_max = 0.0f32;
    let mut shifted_edges = 0usize;
    let mut shifted_white = 0usize;
    for y in 18..=31 {
        for x in 58..=109 {
            let p1 = grid.get_pixel(x, y);
            let l1 = px_luma(p1);
            let l2 = px_luma(grid.get_pixel(x + 1, y));
            if l1 < shifted_min {
                shifted_min = l1;
            }
            if l1 > shifted_max {
                shifted_max = l1;
            }
            if (l1 - l2).abs() > 40.0 {
                shifted_edges += 1;
            }
            if l1 >= 175.0 && px_sat(p1) <= 30 {
                shifted_white += 1;
            }
        }
    }
    let has_shifted_status_row = (shifted_max - shifted_min) >= 65.0
        && (shifted_edges >= 5 || shifted_white >= 6)
        && edge_count_right <= 4;

    let mut slider_bright = 0usize;
    let mut slider_dark = 0usize;
    for y in (76..=134).step_by(4) {
        for x in (62..=108).step_by(4) {
            let p = grid.get_pixel(x, y);
            let l = px_luma(p);
            if l >= 175.0 && px_sat(p) <= 25 {
                slider_bright += 1;
            } else if l < 75.0 {
                slider_dark += 1;
            }
        }
    }
    let has_cc_sliders = slider_bright >= 4 && slider_dark >= 8;

    let mut toggle_color_count = 0usize;
    for y in (34..=88).step_by(4) {
        for x in (12..=56).step_by(4) {
            let p = grid.get_pixel(x, y);
            let is_blue = p.b > 170 && (p.b as i32 - p.r as i32) > 65;
            let is_green = p.g > 150 && (p.g as i32 - p.r as i32) > 45;
            if is_blue || is_green {
                toggle_color_count += 1;
            }
        }
    }
    let has_cc_toggles = toggle_color_count >= 2;

    let mut cc_top_sum = 0.0f32;
    let mut cc_top_cnt = 0usize;
    for y in (10..=28).step_by(4) {
        for x in (10..=110).step_by(8) {
            cc_top_sum += px_luma(grid.get_pixel(x, y));
            cc_top_cnt += 1;
        }
    }
    let cc_top_blur_avg = if cc_top_cnt > 0 { cc_top_sum / cc_top_cnt as f32 } else { 0.0 };

    // Golden Geometry: isLandscapeMenuLayout strips & eFootball Formation (mirror.js:1905-1960)
    let mut ls_strip_sum = 0.0f32;
    let mut ls_strip_cnt = 0usize;
    for y in (20..=240).step_by(10) {
        for x in (1..=8).step_by(2) {
            ls_strip_sum += px_luma(grid.get_pixel(x, y));
            ls_strip_cnt += 1;
        }
    }
    let left_edge_strip_avg_luma = if ls_strip_cnt > 0 { ls_strip_sum / ls_strip_cnt as f32 } else { 100.0 };

    let mut rs_strip_sum = 0.0f32;
    let mut rs_strip_cnt = 0usize;
    for y in (20..=240).step_by(10) {
        for x in (108..=118).step_by(2) {
            rs_strip_sum += px_luma(grid.get_pixel(x, y));
            rs_strip_cnt += 1;
        }
    }
    let right_edge_strip_avg_luma = if rs_strip_cnt > 0 { rs_strip_sum / rs_strip_cnt as f32 } else { 100.0 };

    let mut cb_sum = 0.0f32;
    let mut cb_cnt = 0usize;
    for y in (20..=240).step_by(10) {
        for x in (35..=85).step_by(10) {
            cb_sum += px_luma(grid.get_pixel(x, y));
            cb_cnt += 1;
        }
    }
    let center_body_avg_luma = if cb_cnt > 0 { cb_sum / cb_cnt as f32 } else { 0.0 };

    let mut brp_sum = 0.0f32;
    let mut brp_cnt = 0usize;
    for y in (190..=240).step_by(6) {
        for x in (80..=116).step_by(4) {
            brp_sum += px_luma(grid.get_pixel(x, y));
            brp_cnt += 1;
        }
    }
    let bot_right_panel_avg_luma = if brp_cnt > 0 { brp_sum / brp_cnt as f32 } else { 100.0 };

    let mut is_efootball_formation = !has_yt_4layer_portrait_watch_layout
        && !has_youtube_dark_lower_white_card
        && island_avg_luma < 28.0
        && blue_back_btn_count >= 6
        && below_clock_blue_pixels >= 4
        && pitch_green_count >= 75
        && bot_right_panel_avg_luma < 52.0
        && !has_true_portrait_status_bar
        && !has_notch_colored_clock_pill
        && (right_edge_strip_avg_luma <= 38.0 || pitch_green_count >= 400);

    // White portrait top/bottom bar & eFootball Neon Poster (mirror.js:1962-2039)
    let has_white_portrait_top_bar = bg_luma_left >= 170.0
        && bg_luma_right >= 170.0
        && (bg_luma_left - bg_luma_right).abs() <= 22.0
        && avg_flank_sat <= 28.0;
    let has_white_portrait_bottom_bar = !top_pillar_is_black
        && has_bottom_white_nav_bar
        && bot_nav_left_avg_luma >= 170.0
        && bot_nav_center_avg_luma >= 170.0
        && bot_nav_right_avg_luma >= 170.0;

    let mut upper_poster_neon_pixels = 0usize;
    for y in (4..=88).step_by(2) {
        for x in (8..=112).step_by(2) {
            let p = grid.get_pixel(x, y);
            if px_sat(p) >= 60 && px_luma(p) >= 38.0 {
                upper_poster_neon_pixels += 1;
            }
        }
    }
    let mut lower_poster_neon_pixels = 0usize;
    let mut lower_true_pink_purple_pixels = 0usize;
    for y in (150..=246).step_by(2) {
        for x in (8..=112).step_by(2) {
            let p = grid.get_pixel(x, y);
            let l = px_luma(p);
            let sat = px_sat(p);
            if sat >= 60 && l >= 38.0 {
                lower_poster_neon_pixels += 1;
            }
            if sat >= 55 && p.r >= 130 && p.b >= 110 && (p.r as i32 - p.g as i32) >= 35 {
                lower_true_pink_purple_pixels += 1;
            }
        }
    }

    let mut mid_belt_total = 0usize;
    let mut navy_belt_pixels = 0usize;
    for y in (92..=145).step_by(2) {
        for x in (10..=110).step_by(2) {
            let p = grid.get_pixel(x, y);
            mid_belt_total += 1;
            if px_luma(p) <= 60.0 && p.b as i32 > (p.g as i32 + 5) && p.b >= 24 {
                navy_belt_pixels += 1;
            }
        }
    }
    let navy_belt_ratio_pct = if mid_belt_total > 0 {
        ((navy_belt_pixels * 100) / mid_belt_total).min(100) as u8
    } else {
        0
    };

    // Full-screen KONAMI Red Intro ratio (safe non-overlapping addition for eFootball opening frame)
    let mut konami_red_samples = 0usize;
    let mut konami_total_samples = 0usize;
    for y in (20..=220).step_by(2) {
        for x in (10..=110).step_by(2) {
            let p = grid.get_pixel(x, y);
            konami_total_samples += 1;
            if p.r >= 145
                && (p.r as i32 - p.g as i32) >= 70
                && (p.r as i32 - p.b as i32) >= 70
            {
                konami_red_samples += 1;
            }
        }
    }
    let konami_red_ratio_pct = if konami_total_samples > 0 {
        ((konami_red_samples * 100) / konami_total_samples) as u8
    } else {
        0
    };

    let mut is_efootball_neon_poster = !has_yt_4layer_portrait_watch_layout
        && !has_youtube_dark_lower_white_card
        && !has_white_portrait_top_bar
        && !has_white_portrait_bottom_bar
        && !is_portrait_comment_sheet_screen
        && left_flank_avg_sat >= 48.0
        && right_flank_avg_sat >= 45.0
        && avg_flank_sat >= 52.0
        && vivid_flank_pixel_count >= 16
        && top_flank_electric_blue_magenta_pixels >= 12
        && upper_poster_neon_pixels >= 32
        && lower_poster_neon_pixels >= 32
        && lower_true_pink_purple_pixels >= 15
        && navy_belt_ratio_pct >= 28;

    // Portrait Overlay / iOS Folder & Control Center (mirror.js:2041-2083)
    let is_portrait_overlay_or_folder = !has_landscape_side_bar
        && !has_dual_black_pillarbox
        && !is_efootball_neon_poster
        && !is_efootball_formation
        && left_flank_avg_sat <= 30.0
        && top_blur_avg_sat <= 30.0
        && bot_blur_avg_sat <= 30.0
        && top_blur_avg_luma >= 38.0
        && bot_blur_avg_luma >= 38.0
        && (top_blur_avg_luma - bot_blur_avg_luma).abs() <= 26.0
        && folder_outer_vivid_pixels <= 6
        && (folder_center_max_luma - folder_center_min_luma) >= 55.0;

    let has_cc_expanded_platter = bot_pillar_avg_luma > 20.0
        && center_body_avg_luma > 40.0
        && avg_flank_sat < 42.0
        && vivid_flank_pixel_count < 12
        && !has_landscape_side_bar;
    let has_cc_modules = has_cc_sliders || has_cc_toggles || has_cc_expanded_platter;
    let is_control_center_view = !has_landscape_side_bar
        && !has_dual_black_pillarbox
        && avg_flank_sat < 42.0
        && vivid_flank_pixel_count < 12
        && colorful_left_zone_count < 8
        && blue_back_btn_count < 3
        && !is_efootball_formation
        && (has_shifted_status_row
            || has_cc_expanded_platter
            || (has_cc_sliders && has_cc_modules)
            || (island_avg_luma < 25.0 && edge_count_left <= 4 && edge_count_right <= 4 && has_cc_modules)
            || (bot_pillar_avg_luma > 20.0
                && (has_cc_sliders || has_cc_toggles || cc_top_blur_avg > 24.0)));

    let mut has_tiktok_or_portrait_bottom_bar =
        has_portrait_bottom_bar || has_tiktok_center_plus_button || has_instagram_reels_dark_chrome;
    let is_dark_heuristic_entry_blocker = is_google_translate_dark
        || is_portrait_comment_sheet_screen
        || has_instagram_reels_dark_chrome
        || has_tiktok_center_plus_button
        || has_dimmed_dark_home_bar
        || is_app_splash_screen;

    // Definite Portrait App Screen evaluated BEFORE Landscape Signals (mirror.js:2098-2121)
    let mut is_definite_portrait_app_screen = if !is_currently_landscape {
        has_light_portrait_app_chrome
            || has_dark_portrait_app_chrome
            || has_instagram_dark_mode_chrome
            || has_ios_home_screen_dock
            || has_true_portrait_status_bar
            || has_tiktok_or_portrait_bottom_bar
            || is_control_center_view
            || is_portrait_overlay_or_folder
            || is_app_splash_screen
            || is_youtube_portrait_player_layout
            || has_youtube_dark_lower_white_card
            || is_dark_heuristic_entry_blocker
    } else {
        has_light_portrait_app_chrome
            || has_instagram_dark_mode_chrome
            || has_ios_home_screen_dock
            || has_true_portrait_status_bar
            || is_control_center_view
            || is_portrait_overlay_or_folder
    };

    // Strictly portrait-vetoed KONAMI Red Intro (full red Instagram Reels/ads CANNOT trigger!)
    let is_konami_red_intro = konami_red_ratio_pct >= 55
        && !is_definite_portrait_app_screen
        && !has_true_portrait_status_bar
        && !has_top_white_chrome
        && !has_bottom_white_nav_bar
        && !has_instagram_reels_dark_chrome
        && !has_instagram_dark_mode_chrome
        && !has_ios_home_screen_dock
        && !has_portrait_bottom_bar;

    // Landscape Menu Layout (mirror.js:2123-2144: right < 28 && left < 42, AND not OR!)
    let mut is_landscape_menu = !has_yt_4layer_portrait_watch_layout
        && !has_youtube_dark_lower_white_card
        && !has_ios_home_screen_dock
        && !has_true_portrait_status_bar
        && ((right_edge_strip_avg_luma < 28.0
            && left_edge_strip_avg_luma < 42.0
            && center_body_avg_luma > 42.0
            && bg_luma_right < 32.0
            && (colorful_left_zone_count >= 8 || vivid_flank_pixel_count >= 10))
            || is_efootball_formation
            || is_efootball_neon_poster)
        && !is_control_center_view
        && !is_definite_portrait_app_screen
        && !has_true_portrait_status_bar
        && !has_notch_colored_clock_pill
        && (right_edge_strip_avg_luma <= 34.0 || is_efootball_neon_poster || is_efootball_formation);

    // Naked Dynamic Island (mirror.js:2146-2167: dark island + vivid game flanks, vetoed by portrait)
    let has_game_flank_saturation = left_flank_avg_sat >= 38.0
        && (avg_flank_sat >= 48.0 || vivid_flank_pixel_count >= 16)
        && !is_control_center_view
        && !is_definite_portrait_app_screen;

    let mut has_naked_dynamic_island = !has_yt_4layer_portrait_watch_layout
        && !has_youtube_dark_lower_white_card
        && island_avg_luma < 25.0
        && island_max_luma < 45.0
        && !has_true_portrait_status_bar
        && !has_portrait_bottom_bar
        && !is_portrait_overlay_or_folder
        && !is_control_center_view
        && !is_definite_portrait_app_screen
        && has_game_flank_saturation
        && (left_flank_avg_luma > 20.0 || right_flank_avg_luma > 20.0)
        && (overall_avg_luma > 16.0 || max_flank_luma > 25.0);

    // Cancel landscape menu/island when strong portrait overlays confirmed (mirror.js:2170-2173)
    if is_portrait_overlay_or_folder
        || has_instagram_dark_mode_chrome
        || has_yt_4layer_portrait_watch_layout
        || has_youtube_dark_lower_white_card
        || has_ios_home_screen_dock
    {
        is_landscape_menu = false;
        has_naked_dynamic_island = false;
    }

    // Absolute priority of confirmed Neon Poster & Naked Island game (mirror.js:2175-2185)
    if is_efootball_neon_poster || has_naked_dynamic_island {
        has_notch_colored_clock_pill = false;
        has_true_portrait_status_bar = false;
        has_portrait_bottom_bar = false;
        has_tiktok_center_plus_button = false;
        has_instagram_reels_dark_chrome = false;
        is_definite_portrait_app_screen = false;
        is_landscape_menu = true;
    }

    // YouTube Fullscreen Landscape (mirror.js:2188-2198)
    let mut is_youtube_fullscreen = ((has_dual_black_pillarbox && has_active_center_video)
        || has_youtube_landscape_osd)
        && !has_portrait_bottom_bar
        && !has_true_portrait_status_bar
        && !is_portrait_overlay_or_folder
        && !is_control_center_view
        && !is_definite_portrait_app_screen;

    // Sticky Fullscreen Pillarbox Lock (mirror.js:2201-2208)
    let has_sticky_fullscreen_pillarbox_lock = is_currently_landscape
        && top_pillar_is_black
        && bot_pillar_is_black
        && !has_notch_colored_clock_pill
        && !has_true_portrait_status_bar
        && !is_portrait_comment_sheet_screen;

    // Absolute Priority of Dual Black Pillarbox, Sticky Lock, & YouTube Landscape OSD (mirror.js:2211-2221)
    if has_dual_black_pillarbox || has_sticky_fullscreen_pillarbox_lock || has_youtube_landscape_osd {
        has_light_portrait_app_chrome = false;
        has_dark_portrait_app_chrome = false;
        has_true_portrait_status_bar = false;
        has_tiktok_or_portrait_bottom_bar = false;
        has_portrait_bottom_bar = false;
        has_tiktok_center_plus_button = false;
        is_definite_portrait_app_screen = false;
        is_youtube_fullscreen = true;
    }

    // 5 Explicit Passive Exit Proofs (mirror.js:2223-2356)
    // Proof 1: exit_by_light_portrait_chrome (NO center white requirement!)
    let exit_by_light_portrait_chrome = !has_landscape_side_bar
        && !has_youtube_landscape_osd
        && !is_efootball_neon_poster
        && (has_white_portrait_top_bar || has_white_portrait_bottom_bar);

    // Proof 2: exit_by_youtube_portrait_watch_page
    let mut ywl_sum = 0.0f32;
    let mut ywl_cnt = 0usize;
    let mut ywl_dark = 0usize;
    let mut ywr_sum = 0.0f32;
    let mut ywr_cnt = 0usize;
    let mut ywr_dark = 0usize;
    for y in (86..=126).step_by(2) {
        for x in (8..=54).step_by(2) {
            let p = grid.get_pixel(x, y);
            let l = px_luma(p);
            ywl_sum += l;
            ywl_cnt += 1;
            if l <= 38.0 && px_sat(p) <= 20 {
                ywl_dark += 1;
            }
        }
        for x in (58..=108).step_by(2) {
            let p = grid.get_pixel(x, y);
            let l = px_luma(p);
            ywr_sum += l;
            ywr_cnt += 1;
            if l <= 38.0 && px_sat(p) <= 20 {
                ywr_dark += 1;
            }
        }
    }
    let yw_left_avg = if ywl_cnt > 0 { ywl_sum / ywl_cnt as f32 } else { 100.0 };
    let yw_right_avg = if ywr_cnt > 0 { ywr_sum / ywr_cnt as f32 } else { 100.0 };
    let yw_dark_ratio = if (ywl_cnt + ywr_cnt) > 0 {
        (ywl_dark + ywr_dark) as f32 / (ywl_cnt + ywr_cnt) as f32
    } else {
        0.0
    };
    let has_yt_watch_dark_divider = ((yw_left_avg <= 48.0 && yw_right_avg <= 48.0) || yw_dark_ratio >= 0.70)
        && (yw_right_avg - yw_left_avg).abs() <= 22.0
        && yw_dark_ratio >= 0.65;

    let mut yw_lc_sum = 0.0f32;
    let mut yw_lc_cnt = 0usize;
    for y in (135..=235).step_by(4) {
        for x in (12..=108).step_by(4) {
            yw_lc_sum += px_luma(grid.get_pixel(x, y));
            yw_lc_cnt += 1;
        }
    }
    let yt_watch_lower_card_avg = if yw_lc_cnt > 0 { yw_lc_sum / yw_lc_cnt as f32 } else { 0.0 };
    let has_true_portrait_status_bar_or_pill = has_notch_colored_clock_pill
        || (has_true_portrait_status_bar
            && (has_plain_white_clock_status_bar
                || (island_avg_luma <= 22.0 && edge_count_left >= 2 && edge_count_right >= 2)
                || (bg_luma_left < 65.0
                    && bg_luma_right < 65.0
                    && left_clock_max_luma >= 85.0
                    && right_status_max_luma >= 80.0)));
    let exit_by_youtube_portrait_watch_page = !has_landscape_side_bar
        && !has_youtube_landscape_osd
        && !has_naked_dynamic_island
        && !is_efootball_neon_poster
        && !is_efootball_formation
        && !is_landscape_menu
        && has_true_portrait_status_bar_or_pill
        && has_yt_watch_dark_divider
        && yt_watch_lower_card_avg >= 25.0;

    // Proof 3: exit_by_home_screen_portrait
    let mut home_dock_colorful_pixels = 0usize;
    let mut bdz_sum = 0.0f32;
    let mut bdz_cnt = 0usize;
    for y in (175..=248).step_by(2) {
        for x in (12..=108).step_by(2) {
            let p = grid.get_pixel(x, y);
            let l = px_luma(p);
            if px_sat(p) >= 45 && l >= 45.0 {
                home_dock_colorful_pixels += 1;
            }
            if y >= 224 {
                bdz_sum += l;
                bdz_cnt += 1;
            }
        }
    }
    let bot_dock_zone_avg_luma = if bdz_cnt > 0 { bdz_sum / bdz_cnt as f32 } else { 0.0 };
    let exit_by_home_screen_portrait = has_ios_home_screen_dock
        || (!has_landscape_side_bar
            && !has_youtube_landscape_osd
            && !has_naked_dynamic_island
            && !is_efootball_neon_poster
            && !is_efootball_formation
            && !is_landscape_menu
            && (has_notch_colored_clock_pill || has_true_portrait_status_bar)
            && home_dock_colorful_pixels >= 14
            && bot_dock_zone_avg_luma >= 42.0);

    // Proof 4: exit_by_ios_folder_portrait
    let exit_by_ios_folder_portrait = !has_landscape_side_bar
        && !has_youtube_landscape_osd
        && is_portrait_overlay_or_folder
        && !top_pillar_is_black
        && !bot_pillar_is_black;

    // Proof 5: exit_by_instagram_dark_mode
    let exit_by_instagram_dark_mode =
        !has_landscape_side_bar && !has_youtube_landscape_osd && has_instagram_dark_mode_chrome;

    let mut strict_portrait_exit_proof = exit_by_light_portrait_chrome
        || exit_by_youtube_portrait_watch_page
        || exit_by_home_screen_portrait
        || exit_by_ios_folder_portrait
        || exit_by_instagram_dark_mode;

    // Absolute Hard Horizontal Lock for Landscape Game & Menu (mirror.js:2359-2363)
    if is_currently_landscape
        && (has_naked_dynamic_island
            || is_efootball_neon_poster
            || is_efootball_formation
            || is_landscape_menu
            || is_konami_red_intro)
        && !exit_by_ios_folder_portrait
        && !exit_by_home_screen_portrait
        && !exit_by_instagram_dark_mode
    {
        strict_portrait_exit_proof = false;
    }

    // Absolute Portrait Override (mirror.js:2366-2371)
    if is_definite_portrait_app_screen
        && !has_dual_black_pillarbox
        && !has_sticky_fullscreen_pillarbox_lock
        && !has_youtube_landscape_osd
        && (!is_currently_landscape || strict_portrait_exit_proof)
    {
        has_landscape_side_bar = false;
        has_naked_dynamic_island = false;
        is_landscape_menu = false;
        is_youtube_fullscreen = false;
    }

    // Absolute Override when has_ios_home_screen_dock is true (mirror.js:2374-2383)
    if has_ios_home_screen_dock {
        has_landscape_side_bar = false;
        has_naked_dynamic_island = false;
        is_efootball_neon_poster = false;
        is_efootball_formation = false;
        is_landscape_menu = false;
        has_youtube_landscape_osd = false;
        is_youtube_fullscreen = false;
    }

    // Overall Landscape Game / Video determination (mirror.js:2386-2396 + is_konami_red_intro)
    let is_landscape_game_frame = (has_landscape_side_bar
        || has_naked_dynamic_island
        || is_youtube_fullscreen
        || has_youtube_landscape_osd
        || is_landscape_menu
        || is_konami_red_intro
        || (is_currently_landscape && !strict_portrait_exit_proof))
        && if !is_currently_landscape {
            !is_definite_portrait_app_screen
        } else {
            !strict_portrait_exit_proof
        };

    // Can passive exit to portrait (mirror.js:2439-2459)
    let can_passive_exit_to_portrait = if is_currently_landscape {
        if has_naked_dynamic_island
            || is_efootball_neon_poster
            || is_efootball_formation
            || is_landscape_menu
            || is_konami_red_intro
        {
            false
        } else {
            strict_portrait_exit_proof
        }
    } else {
        !has_youtube_landscape_osd
            && !has_dual_black_pillarbox
            && !has_sticky_fullscreen_pillarbox_lock
            && !is_efootball_neon_poster
            && (is_definite_portrait_app_screen
                || has_true_portrait_status_bar
                || ((has_light_portrait_app_chrome
                    || has_dark_portrait_app_chrome
                    || is_portrait_overlay_or_folder)
                    && overall_avg_luma >= 24.0))
    };

    let suggested_orientation = if is_landscape_game_frame {
        OrientationMode::Landscape270
    } else {
        OrientationMode::Portrait0
    };

    let last_reason = if suggested_orientation == OrientationMode::Landscape270 {
        if is_konami_red_intro {
            "konami_red_intro"
        } else if is_efootball_neon_poster {
            "efootball_neon_poster"
        } else if is_efootball_formation {
            "efootball_formation"
        } else if has_landscape_side_bar {
            "landscape_side_bar"
        } else if has_youtube_landscape_osd {
            "youtube_landscape_osd"
        } else if is_youtube_fullscreen {
            "youtube_fullscreen_pillarbox"
        } else if has_naked_dynamic_island {
            "naked_dynamic_island_game"
        } else if is_landscape_menu {
            "landscape_menu_layout"
        } else {
            "hard_horizontal_lock_hold"
        }
    } else if is_currently_landscape && exit_by_light_portrait_chrome {
        "exit_by_light_portrait_chrome"
    } else if is_currently_landscape && exit_by_instagram_dark_mode {
        "exit_by_instagram_dark_mode"
    } else if is_currently_landscape && exit_by_home_screen_portrait {
        "exit_by_home_screen_portrait"
    } else if is_currently_landscape && exit_by_ios_folder_portrait {
        "exit_by_ios_folder_portrait"
    } else if is_currently_landscape && exit_by_youtube_portrait_watch_page {
        "exit_by_youtube_portrait_watch"
    } else if is_app_splash_screen {
        "app_splash_screen_guard"
    } else if is_definite_portrait_app_screen {
        "definite_portrait_app_screen"
    } else {
        "portrait_default"
    };

    FrameAnalysisResult {
        is_landscape_game_frame,
        is_efootball_neon_poster,
        is_efootball_formation,
        is_landscape_menu,
        is_youtube_fullscreen,
        has_youtube_landscape_osd,
        has_notch_colored_clock_pill,
        is_tiktok_comment_sheet: is_portrait_comment_sheet_screen,
        is_tiktok_splash_screen: is_app_splash_screen,
        is_instagram_reels_dark_chrome: has_instagram_reels_dark_chrome,
        is_google_translate_dark,
        is_control_center_view,
        is_ios_folder_view: is_portrait_overlay_or_folder,
        has_ios_home_screen_dock,
        strict_portrait_exit_proof,
        suggested_orientation,
        is_konami_red_intro,
        has_landscape_side_bar,
        has_naked_dynamic_island,
        has_plain_white_clock_status_bar,
        is_calm_flank_status,
        has_true_portrait_status_bar,
        has_portrait_bottom_home_bar: has_portrait_bottom_bar,
        has_portrait_split_app_chrome: has_light_portrait_app_chrome || has_instagram_dark_mode_chrome,
        navy_belt_ratio_pct,
        upper_poster_neon_pixels,
        lower_poster_neon_pixels,
        has_top_white_chrome,
        has_bottom_white_nav_bar,
        has_light_portrait_app_chrome,
        has_instagram_reels_dark_chrome,
        has_instagram_dark_mode_chrome,
        has_tiktok_center_plus_button,
        has_tiktok_or_portrait_bottom_bar,
        is_portrait_comment_sheet_screen,
        is_app_splash_screen,
        is_portrait_overlay_or_folder,
        is_definite_portrait_app_screen,
        top_pillar_is_black,
        bot_pillar_is_black,
        has_dual_black_pillarbox,
        has_sticky_fullscreen_pillarbox_lock,
        exit_by_light_portrait_chrome,
        exit_by_youtube_portrait_watch_page,
        exit_by_home_screen_portrait,
        exit_by_ios_folder_portrait,
        exit_by_instagram_dark_mode,
        can_passive_exit_to_portrait,
        last_reason,
    }
}

pub const HEVC_AUTOROTATE_MANUAL_COOLDOWN_MS: u64 = 30_000;
pub const HEVC_AUTOROTATE_NAV_HOME_GRACE_MS: u64 = 5_000;
pub const HEVC_AUTOROTATE_HOME_REENTRY_BLOCK_MS: u64 = 2_800;

/// Stateful zero-allocation debouncer for CoreDevice HEVC live Auto-Rotate (`mirror.js:2537-2613`).
/// AirPlay never uses this tracker; AirPlay remains strictly manual-only.
#[derive(Debug, Clone)]
pub struct HevcAutoRotateTracker {
    currently_landscape: bool,
    pending_landscape_votes: u8,
    pending_portrait_votes: u8,
    manual_cooldown_until_ms: u64,
    nav_home_recent_until_ms: u64,
    home_exit_grace_until_ms: u64,
    pub last_suggested: OrientationMode,
    pub last_reason: &'static str,
    pub transitions_to_landscape: u64,
    pub transitions_to_portrait: u64,
}

impl Default for HevcAutoRotateTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl HevcAutoRotateTracker {
    pub fn new() -> Self {
        Self {
            currently_landscape: false,
            pending_landscape_votes: 0,
            pending_portrait_votes: 0,
            manual_cooldown_until_ms: 0,
            nav_home_recent_until_ms: 0,
            home_exit_grace_until_ms: 0,
            last_suggested: OrientationMode::Portrait0,
            last_reason: "init",
            transitions_to_landscape: 0,
            transitions_to_portrait: 0,
        }
    }

    /// Resets tracker state cleanly on stream start / stop / session epoch change.
    pub fn reset(&mut self) {
        self.currently_landscape = false;
        self.pending_landscape_votes = 0;
        self.pending_portrait_votes = 0;
        self.manual_cooldown_until_ms = 0;
        self.nav_home_recent_until_ms = 0;
        self.home_exit_grace_until_ms = 0;
        self.last_suggested = OrientationMode::Portrait0;
        self.last_reason = "reset";
    }

    pub fn is_landscape(&self) -> bool {
        self.currently_landscape
    }

    pub fn last_reason(&self) -> &'static str {
        self.last_reason
    }

    pub fn set_currently_landscape(&mut self, landscape: bool) {
        self.currently_landscape = landscape;
        self.pending_landscape_votes = 0;
        self.pending_portrait_votes = 0;
    }

    /// Starts a 30-second manual cooldown so Auto-Rotate never fights user rotate clicks.
    pub fn notify_manual_override(&mut self, now_ms: u64, landscape_now: bool) {
        self.currently_landscape = landscape_now;
        self.pending_landscape_votes = 0;
        self.pending_portrait_votes = 0;
        self.manual_cooldown_until_ms = now_ms.saturating_add(HEVC_AUTOROTATE_MANUAL_COOLDOWN_MS);
        self.last_reason = "manual_override_cooldown";
    }

    /// Opens a 5-second fast-exit window (1 vote), blocks re-entry for 2.8 s, and snaps to Portrait.
    pub fn notify_nav_home_recent(&mut self, now_ms: u64) {
        self.currently_landscape = false;
        self.pending_landscape_votes = 0;
        self.pending_portrait_votes = 0;
        self.nav_home_recent_until_ms = now_ms.saturating_add(HEVC_AUTOROTATE_NAV_HOME_GRACE_MS);
        self.home_exit_grace_until_ms = now_ms.saturating_add(HEVC_AUTOROTATE_HOME_REENTRY_BLOCK_MS);
        self.last_reason = "nav_home_recent";
    }

    /// Evaluates one `ProbeGrid120x260` sample and returns `Some(270)` or `Some(0)` on transition.
    pub fn update_sample(
        &mut self,
        grid: &ProbeGrid120x260,
        raw_w: u32,
        raw_h: u32,
        now_ms: u64,
        is_ipad: bool,
    ) -> Option<u16> {
        if is_ipad || raw_w == 0 || raw_h == 0 || raw_w > raw_h {
            return None;
        }
        if now_ms < self.manual_cooldown_until_ms {
            self.last_reason = "manual_cooldown_active";
            return None;
        }
        if !self.currently_landscape && now_ms < self.home_exit_grace_until_ms {
            self.pending_landscape_votes = 0;
            self.last_reason = "home_exit_grace_block";
            return None;
        }
        let analysis = analyze_frame_grid(grid, self.currently_landscape);
        self.last_suggested = analysis.suggested_orientation;
        self.last_reason = analysis.last_reason;
        if !self.currently_landscape {
            if analysis.is_app_splash_screen {
                self.pending_landscape_votes = 0;
                self.last_reason = "app_splash_screen_blocked";
                return None;
            }
            if analysis.suggested_orientation == OrientationMode::Landscape270 {
                let needed = if analysis.has_landscape_side_bar || analysis.is_konami_red_intro {
                    1
                } else {
                    2
                };
                self.pending_landscape_votes = self.pending_landscape_votes.saturating_add(1);
                self.pending_portrait_votes = 0;
                if self.pending_landscape_votes >= needed {
                    self.currently_landscape = true;
                    self.pending_landscape_votes = 0;
                    self.transitions_to_landscape = self.transitions_to_landscape.saturating_add(1);
                    return Some(270);
                }
            } else {
                self.pending_landscape_votes = 0;
            }
        } else {
            if analysis.has_sticky_fullscreen_pillarbox_lock
                || analysis.has_dual_black_pillarbox
                || analysis.has_youtube_landscape_osd
                || !analysis.can_passive_exit_to_portrait
                || analysis.is_landscape_game_frame
                || analysis.is_youtube_fullscreen
                || analysis.is_landscape_menu
                || analysis.has_landscape_side_bar
            {
                self.pending_portrait_votes = 0;
                return None;
            }
            if analysis.suggested_orientation == OrientationMode::Portrait0 {
                let needed = if now_ms < self.nav_home_recent_until_ms {
                    1
                } else {
                    3
                };
                self.pending_portrait_votes = self.pending_portrait_votes.saturating_add(1);
                self.pending_landscape_votes = 0;
                if self.pending_portrait_votes >= needed {
                    self.currently_landscape = false;
                    self.pending_portrait_votes = 0;
                    self.transitions_to_portrait = self.transitions_to_portrait.saturating_add(1);
                    return Some(0);
                }
            } else {
                self.pending_portrait_votes = 0;
            }
        }
        None
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

fn build_ios_home_dock_4slot_icons(grid: &mut ProbeGrid120x260) {
    // 4 realistic iOS Dock slots separated by 3 quiet wallpaper gaps (mirror.js:156-249)
    // Slot 1 (x=11..27): Phone green; Slot 2 (x=37..53): Safari blue/white;
    // Slot 3 (x=64..80): Messages green; Slot 4 (x=90..106): WhatsApp green.
    grid.fill_rect(12, 224, 26, 241, 35, 205, 75);
    grid.fill_rect(38, 224, 52, 241, 35, 145, 245);
    grid.fill_rect(65, 224, 79, 241, 35, 205, 75);
    grid.fill_rect(91, 224, 105, 241, 35, 205, 75);
}

#[cfg(test)]
fn build_instagram_light_feed_dark_post_grid() -> ProbeGrid120x260 {
    let mut grid = ProbeGrid120x260::new();
    // Clean white Instagram light feed backdrop
    grid.fill_all(248, 248, 250);
    // Dynamic Island pill at x=46..74, y=4..9
    grid.fill_rect(46, 4, 74, 9, 6, 6, 6);
    // Dark clock digits "20.13" at x=8..30, y=3..8
    for x in (8..=30).step_by(3) {
        grid.fill_rect(x, 4, x + 1, 8, 20, 20, 20);
    }
    // Dark signal & battery status glyphs at x=86..112, y=3..8
    for x in (86..=110).step_by(4) {
        grid.fill_rect(x, 4, x + 1, 8, 20, 20, 20);
    }
    // Colorful Stories ring row at y=22..40
    for cx in [16usize, 38, 60, 82, 104] {
        grid.fill_rect(cx - 5, 22, cx + 5, 40, 235, 65, 120);
        grid.fill_rect(cx - 3, 24, cx + 3, 38, 120, 95, 80);
    }
    // Full-bleed dark photo/video post in center touching both edges (y=48..205)
    grid.fill_rect(0, 48, 119, 205, 18, 20, 22);
    grid.fill_rect(35, 80, 85, 170, 75, 72, 68);
    // Caption text rows on white background (y=210..226)
    grid.fill_rect(8, 212, 65, 216, 85, 85, 88);
    grid.fill_rect(8, 220, 48, 224, 120, 120, 125);
    // White bottom nav bar (y=233..259) with 5 dark icons
    for bx in [16usize, 38, 60, 82, 104] {
        grid.fill_rect(bx - 2, 238, bx + 2, 243, 30, 30, 32);
    }
    grid
}

fn build_instagram_reels_dark_mode_grid(full_red_video: bool) -> ProbeGrid120x260 {
    let mut grid = ProbeGrid120x260::new();
    grid.fill_all(8, 8, 10);
    // Dynamic Island at x=46..74, y=4..9
    grid.fill_rect(46, 4, 74, 9, 4, 4, 4);
    // Thin white clock "20.13" (block 0) & battery (block 5) in status bar
    for x in (10..=28).step_by(3) {
        grid.fill_rect(x, 4, x + 1, 7, 235, 235, 235);
    }
    for x in (88..=110).step_by(4) {
        grid.fill_rect(x, 4, x + 1, 7, 235, 235, 235);
    }
    // Thin white "Reels Friends" header text at y=14..19 (realistic sparse glyphs, NOT solid gray block!)
    for x in (14..=66).step_by(4) {
        grid.fill_rect(x, 14, x + 1, 18, 230, 230, 230);
    }
    // Reels video body
    if full_red_video {
        grid.fill_rect(0, 24, 119, 228, 215, 18, 22);
    } else {
        grid.fill_rect(0, 48, 119, 208, 95, 88, 82);
        // Right-edge action buttons (Like, Comment, Share, Remix) on x=112..118
        for ay in [108usize, 128, 148, 170] {
            grid.fill_rect(113, ay, 117, ay + 6, 240, 240, 240);
        }
    }
    // Bottom OLED nav bar (y=233..259) with 5 separated white nav icon clusters
    grid.fill_rect(0, 232, 119, 259, 6, 6, 8);
    for bx in [16usize, 38, 60, 82, 104] {
        grid.fill_rect(bx - 1, 238, bx + 2, 242, 235, 235, 235);
    }
    grid
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
            println!("FAIL Scenario {}: {} (is_land={}, reason={}) -> Got {:?}, Expected {:?}", idx, name, is_land, res.last_reason, res.suggested_orientation, expected);
            false
        }
    };

    // --- Skenario 1..12: eFootball 2027 Games (Expected: Landscape270) ---
    // 1. Intro Merah KONAMI
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(210, 15, 20);
        grid.fill_rect(6, 2, 42, 12, 220, 20, 20);
        grid.fill_rect(78, 2, 114, 12, 220, 20, 20);
        if check(1, "eFootball_Intro_Red_Konami", &grid, false, OrientationMode::Landscape270) { passed += 1; }
    }

    // 2. Intro Hitam Logo 'e'
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(10, 10, 12);
        grid.fill_rect(50, 100, 70, 160, 240, 220, 10);
        if check(2, "eFootball_Intro_Black_eLogo", &grid, true, OrientationMode::Landscape270) { passed += 1; }
    }

    // 3. Intro Biru "THE PITCH IS OURS"
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(15, 25, 95);
        grid.fill_rect(1, 20, 8, 240, 10, 12, 25);
        if check(3, "eFootball_Intro_Blue_ThePitchIsOurs", &grid, true, OrientationMode::Landscape270) { passed += 1; }
    }

    // 4. Poster Neon Lionel Messi & Lamine Yamal (mirror.js:1978-2039)
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(12, 14, 25);
        grid.fill_rect(6, 2, 42, 12, 20, 180, 210);
        grid.fill_rect(78, 2, 114, 12, 210, 30, 190);
        grid.fill_rect(20, 14, 100, 85, 20, 195, 230); // Upper neon aura
        grid.fill_rect(10, 92, 110, 145, 16, 28, 72); // Navy belt (y=92..145)
        grid.fill_rect(20, 152, 100, 235, 220, 40, 145); // Messi pink jersey (y=150..246)
        if check(4, "eFootball_Neon_Poster_Messi_Yamal", &grid, false, OrientationMode::Landscape270) { passed += 1; }
    }

    // 5. Menu Utama / Beranda Brasileirao (mirror.js:1905-1935, 2123-2144)
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(60, 70, 90);
        grid.fill_rect(108, 0, 119, 259, 12, 14, 20); // Right dark navy bar
        grid.fill_rect(80, 0, 119, 14, 12, 14, 20); // bgLumaRight < 32
        grid.fill_rect(1, 20, 8, 240, 15, 18, 25); // Left dark navy strip
        grid.fill_rect(10, 11, 42, 24, 230, 175, 20); // Colorful menu crest
        grid.fill_rect(20, 50, 100, 120, 220, 180, 20);
        if check(5, "eFootball_MainMenu_Brasileirao", &grid, false, OrientationMode::Landscape270) { passed += 1; }
    }

    // 6. Menu Utama J.LEAGUE
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(50, 60, 80);
        grid.fill_rect(108, 0, 119, 259, 14, 16, 22);
        grid.fill_rect(80, 0, 119, 14, 14, 16, 22);
        grid.fill_rect(1, 20, 8, 240, 16, 18, 24);
        grid.fill_rect(10, 11, 42, 24, 225, 165, 25);
        grid.fill_rect(20, 50, 100, 120, 220, 180, 20);
        if check(6, "eFootball_MainMenu_JLeague", &grid, false, OrientationMode::Landscape270) { passed += 1; }
    }

    // 7. Taktik / Formasi 3-2-2-3 (mirror.js:1949-1960)
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(20, 20, 25);
        grid.fill_rect(22, 45, 96, 195, 30, 110, 40);
        grid.fill_rect(4, 11, 24, 26, 20, 105, 215); // Blue "< Kembali" button
        grid.fill_rect(40, 2, 80, 11, 10, 10, 10);
        grid.fill_rect(106, 0, 119, 259, 14, 16, 22); // Right panel dark <= 38
        grid.fill_rect(80, 190, 116, 245, 15, 15, 20);
        if check(7, "eFootball_Formation_3223", &grid, false, OrientationMode::Landscape270) { passed += 1; }
    }

    // 8. Match Gameplay (Wide Green Pitch)
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(20, 20, 25);
        grid.fill_rect(22, 45, 96, 195, 25, 115, 35);
        grid.fill_rect(4, 11, 24, 26, 20, 105, 215);
        grid.fill_rect(40, 2, 80, 11, 10, 10, 10);
        grid.fill_rect(106, 0, 119, 259, 14, 16, 22);
        grid.fill_rect(80, 190, 116, 245, 15, 15, 20);
        if check(8, "eFootball_Match_Gameplay", &grid, false, OrientationMode::Landscape270) { passed += 1; }
    }

    // 9. Match Goal Celebration
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(40, 50, 70);
        grid.fill_rect(1, 20, 8, 240, 10, 12, 18);
        grid.fill_rect(112, 20, 119, 240, 12, 14, 20);
        grid.fill_rect(20, 60, 100, 180, 30, 120, 40);
        if check(9, "eFootball_Match_GoalCelebration", &grid, true, OrientationMode::Landscape270) { passed += 1; }
    }

    // 10. Match Half-Time Menu
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(55, 65, 80);
        grid.fill_rect(1, 20, 8, 240, 12, 15, 20);
        grid.fill_rect(108, 0, 119, 259, 15, 18, 22);
        grid.fill_rect(80, 0, 119, 14, 15, 18, 22);
        grid.fill_rect(10, 11, 42, 24, 210, 170, 25);
        grid.fill_rect(20, 50, 100, 120, 210, 170, 25);
        if check(10, "eFootball_Match_HalfTimeMenu", &grid, false, OrientationMode::Landscape270) { passed += 1; }
    }

    // 11. Penalty Shootout
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(20, 22, 28);
        grid.fill_rect(22, 45, 96, 195, 35, 120, 45);
        grid.fill_rect(4, 11, 24, 26, 25, 110, 210);
        grid.fill_rect(40, 2, 80, 11, 10, 10, 10);
        grid.fill_rect(106, 0, 119, 259, 14, 16, 22);
        grid.fill_rect(80, 190, 116, 245, 15, 15, 20);
        if check(11, "eFootball_Match_PenaltyShootout", &grid, false, OrientationMode::Landscape270) { passed += 1; }
    }

    // 12. Victory Screen
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(14, 16, 24);
        grid.fill_rect(6, 2, 42, 12, 220, 25, 180);
        grid.fill_rect(78, 2, 114, 12, 20, 210, 220);
        grid.fill_rect(20, 14, 100, 85, 20, 210, 220);
        grid.fill_rect(10, 92, 110, 145, 20, 30, 90);
        grid.fill_rect(20, 152, 100, 235, 210, 30, 150);
        if check(12, "eFootball_Victory_Screen", &grid, false, OrientationMode::Landscape270) { passed += 1; }
    }

    // --- Skenario 13..19: YouTube Fullscreen Landscape (Expected: Landscape270) ---
    // 13. YouTube 16:9 Fullscreen (Dual Clean Corridors)
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(100, 110, 120);
        grid.fill_rect(0, 0, 119, 10, 5, 5, 5);
        grid.fill_rect(0, 240, 119, 259, 5, 5, 5);
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

    // 16. YouTube Ambient Mode Glow
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(110, 120, 130);
        grid.fill_rect(0, 0, 119, 10, 25, 26, 30);
        grid.fill_rect(0, 240, 119, 259, 24, 25, 29);
        if check(16, "YouTube_Ambient_Mode_Glow", &grid, false, OrientationMode::Landscape270) { passed += 1; }
    }

    // 17. YouTube Zoom-to-Fill with thin Red Progress Bar OSD (<= 3px wide on single column)
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(85, 90, 95);
        grid.fill_rect(22, 20, 23, 200, 220, 15, 15);
        if check(17, "YouTube_Zoom_To_Fill_OSD", &grid, false, OrientationMode::Landscape270) { passed += 1; }
    }

    // 18. YouTube Landscape with Channel Watermark at x=4..21
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(90, 90, 90);
        grid.fill_rect(0, 0, 119, 10, 5, 5, 5);
        grid.fill_rect(0, 240, 119, 259, 5, 5, 5);
        grid.fill_rect(4, 244, 21, 252, 200, 200, 200);
        if check(18, "YouTube_Landscape_With_Watermark", &grid, false, OrientationMode::Landscape270) { passed += 1; }
    }

    // 19. YouTube Landscape with AssistiveTouch at x=45..74
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(90, 90, 90);
        grid.fill_rect(0, 0, 119, 10, 5, 5, 5);
        grid.fill_rect(0, 240, 119, 259, 5, 5, 5);
        grid.fill_rect(45, 244, 74, 252, 140, 140, 140);
        if check(19, "YouTube_Landscape_With_AssistiveTouch", &grid, false, OrientationMode::Landscape270) { passed += 1; }
    }

    // --- Skenario 20..32: Portrait Guard Screens (Expected: Portrait0) ---
    // 20. iPhone 11 Pro Max Notch Colored Clock Pill (Blue) + battery right + Home Dock exit proof
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(15, 15, 18);
        grid.fill_rect(4, 2, 28, 9, 20, 80, 220);
        grid.fill_rect(10, 4, 22, 7, 240, 240, 240);
        for x in (88..=110).step_by(4) {
            grid.fill_rect(x, 4, x + 1, 8, 220, 220, 220);
        }
        build_ios_home_dock_4slot_icons(&mut grid);
        if check(20, "Portrait_iPhone11ProMax_Notch_ClockPill", &grid, true, OrientationMode::Portrait0) { passed += 1; }
    }

    // 21. iPhone 17 Pro Dynamic Island Clock + Home Dock 4-slot 3-gap
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(20, 22, 26);
        grid.fill_rect(46, 4, 74, 9, 5, 5, 5);
        build_ios_home_dock_4slot_icons(&mut grid);
        if check(21, "Portrait_iPhone17Pro_DynamicIsland_Clock", &grid, true, OrientationMode::Portrait0) { passed += 1; }
    }

    // 22. TikTok OLED Comment Sheet (Entry-blocker per mirror.js:2091 -> is_land=false)
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(10, 10, 12);
        grid.fill_rect(4, 12, 28, 46, 2, 2, 2);
        grid.fill_rect(92, 12, 116, 46, 2, 2, 2);
        grid.fill_rect(45, 12, 75, 46, 80, 80, 85);
        for x in (20..=90).step_by(6) {
            grid.fill_rect(x, 54, x + 2, 62, 180, 180, 185);
        }
        for y in (80..=210).step_by(15) {
            grid.fill_rect(16, y, 70, y + 3, 140, 140, 145);
        }
        grid.fill_rect(10, 242, 110, 256, 45, 45, 50);
        if check(22, "Portrait_TikTok_OLED_CommentSheet", &grid, false, OrientationMode::Portrait0) { passed += 1; }
    }

    // 23. TikTok Splash Screen (Entry-blocker per mirror.js:1296, 2564 -> is_land=false)
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(4, 4, 4);
        grid.fill_rect(42, 115, 78, 145, 230, 230, 230); // Centered logo
        if check(23, "Portrait_TikTok_SplashScreen", &grid, false, OrientationMode::Portrait0) { passed += 1; }
    }

    // 24. TikTok Feed Dark (Entry-blocker with [+] center button per mirror.js:1512 -> is_land=false)
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(10, 10, 12);
        for x in (16..=44).step_by(3) {
            grid.fill_rect(x, 6, x + 1, 14, 210, 210, 210);
        }
        grid.fill_rect(54, 234, 66, 244, 240, 240, 240); // Bright [+] center button
        if check(24, "Portrait_TikTok_Feed_Dark", &grid, false, OrientationMode::Portrait0) { passed += 1; }
    }

    // 25. Instagram Reels 1:1 Square / Dark Mode Realistic (exits even when is_land=true!)
    {
        let grid = build_instagram_reels_dark_mode_grid(false);
        if check(25, "Portrait_Instagram_Reels_1_1_Square", &grid, true, OrientationMode::Portrait0) { passed += 1; }
    }

    // 26. Instagram Dark Mode Feed Realistic (exits even when is_land=true!)
    {
        let mut grid = build_instagram_reels_dark_mode_grid(false);
        // Header "Instagram" white glyph cluster on y=13..22, x=8..44
        for x in (8..=44).step_by(3) {
            grid.fill_rect(x, 14, x + 1, 21, 240, 240, 240);
        }
        if check(26, "Portrait_Instagram_DarkMode_Feed", &grid, true, OrientationMode::Portrait0) { passed += 1; }
    }

    // 27. Instagram Story Viewer (Portrait entry-blocker with white clock/battery & reply outline)
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(15, 15, 18);
        grid.fill_rect(46, 4, 74, 9, 5, 5, 5);
        for x in (10..=28).step_by(3) {
            grid.fill_rect(x, 4, x + 1, 8, 235, 235, 235);
        }
        for x in (88..=110).step_by(4) {
            grid.fill_rect(x, 4, x + 1, 8, 235, 235, 235);
        }
        for x in (10..=50).step_by(3) {
            grid.fill_rect(x, 15, x + 1, 19, 210, 210, 210);
        }
        grid.fill_rect(12, 240, 96, 250, 60, 60, 65);
        if check(27, "Portrait_Instagram_Story_Viewer", &grid, false, OrientationMode::Portrait0) { passed += 1; }
    }

    // 28. Google Translate Dark Mode (Entry-blocker per mirror.js:1561 -> is_land=false)
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(20, 20, 22);
        grid.fill_rect(46, 4, 74, 9, 5, 5, 5); // Dynamic Island
        for x in (10..=28).step_by(3) {
            grid.fill_rect(x, 4, x + 1, 8, 235, 235, 235); // White clock
        }
        for x in (88..=110).step_by(4) {
            grid.fill_rect(x, 4, x + 1, 8, 235, 235, 235); // Battery/signal
        }
        grid.fill_rect(10, 14, 95, 28, 175, 175, 180); // "Translate" header
        grid.fill_rect(6, 44, 114, 155, 28, 30, 34); // Dark slate input card
        grid.fill_rect(45, 205, 75, 238, 40, 140, 230); // Pastel blue mic
        if check(28, "Portrait_GoogleTranslate_DarkMode", &grid, false, OrientationMode::Portrait0) { passed += 1; }
    }

    // 29. iOS Control Center View (Entry-blocker per mirror.js:2067 -> is_land=false)
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(45, 45, 50);
        for x in (14..=52).step_by(8) {
            grid.fill_rect(x, 38, x + 4, 82, 40, 130, 235); // Blue/green toggles
        }
        grid.fill_rect(66, 80, 78, 128, 220, 220, 225); // Bright slider fill
        if check(29, "Portrait_iOS_ControlCenter", &grid, false, OrientationMode::Portrait0) { passed += 1; }
    }

    // 30. iOS Folder View (Exits even when is_land=true!)
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(50, 52, 56);
        grid.fill_rect(15, 14, 105, 48, 52, 54, 58);
        grid.fill_rect(35, 90, 85, 160, 200, 200, 205); // High-contrast folder icons (range >= 55)
        grid.fill_rect(15, 198, 105, 238, 51, 53, 57);
        if check(30, "Portrait_iOS_Folder_View", &grid, true, OrientationMode::Portrait0) { passed += 1; }
    }

    // 31. iOS Home Screen Dock (4 slots + 3 quiet gaps, exits even when is_land=true!)
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(30, 32, 35);
        build_ios_home_dock_4slot_icons(&mut grid);
        if check(31, "Portrait_iOS_HomeScreen_Dock", &grid, true, OrientationMode::Portrait0) { passed += 1; }
    }

    // 32. Apple Notes Light Mode (Exits even when is_land=true via exit_by_light_portrait_chrome!)
    {
        let mut grid = ProbeGrid120x260::new();
        grid.fill_all(245, 245, 240);
        for x in (8..=28).step_by(3) {
            grid.fill_rect(x, 4, x + 1, 8, 25, 25, 25);
        }
        for x in (88..=110).step_by(4) {
            grid.fill_rect(x, 4, x + 1, 8, 25, 25, 25);
        }
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
        assert_eq!(p.sat, 100);
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

    #[test]
    fn test_tahap283_autorotate_efootball_konami_youtube_14_scenarios() {
        // 1. KONAMI Vivid Red Intro: fast 1-vote entry from Portrait -> Landscape 270
        let mut tracker = HevcAutoRotateTracker::new();
        let mut konami = ProbeGrid120x260::new();
        konami.fill_all(210, 15, 20);
        let res_k = analyze_frame_grid(&konami, false);
        assert!(res_k.is_konami_red_intro);
        assert!(!res_k.has_ios_home_screen_dock);
        assert_eq!(res_k.suggested_orientation, OrientationMode::Landscape270);
        assert_eq!(tracker.update_sample(&konami, 1206, 2622, 1_000, false), Some(270));
        assert!(tracker.is_landscape());

        // 2. eFootball Neon Poster: must stay landscape & NOT falsely trigger home dock
        let mut poster = ProbeGrid120x260::new();
        poster.fill_all(12, 16, 36);
        poster.fill_rect(6, 2, 42, 12, 20, 185, 225);
        poster.fill_rect(78, 2, 114, 12, 215, 35, 190);
        poster.fill_rect(10, 20, 110, 88, 18, 220, 240);
        poster.fill_rect(10, 92, 110, 145, 14, 30, 95);
        poster.fill_rect(25, 152, 95, 240, 225, 40, 145);
        let res_p = analyze_frame_grid(&poster, true);
        assert!(!res_p.has_ios_home_screen_dock, "eFootball poster jersey must NOT trigger dock");
        assert!(res_p.navy_belt_ratio_pct >= 28 && res_p.is_efootball_neon_poster);
        assert_eq!(res_p.suggested_orientation, OrientationMode::Landscape270);
        assert_eq!(tracker.update_sample(&poster, 1206, 2622, 2_000, false), None);

        // 3. eFootball Green Pitch + colorful HUD in dock band (must stay landscape!)
        let mut match_frame = ProbeGrid120x260::new();
        match_frame.fill_all(20, 22, 28);
        match_frame.fill_rect(16, 35, 104, 215, 28, 122, 42);
        match_frame.fill_rect(12, 218, 108, 248, 210, 60, 180);
        assert!(!analyze_frame_grid(&match_frame, true).has_ios_home_screen_dock);
        assert_eq!(tracker.update_sample(&match_frame, 1206, 2622, 3_000, false), None);

        // 4. eFootball Dark Loading Screen (hysteresis keeps landscape while already landscape)
        let mut dark_load = ProbeGrid120x260::new();
        dark_load.fill_all(8, 8, 12);
        assert_eq!(analyze_frame_grid(&dark_load, true).suggested_orientation, OrientationMode::Landscape270);

        // 5. Landscape with Vertical Side Home Bar on left edge -> 1-vote fast entry
        let mut t_side = HevcAutoRotateTracker::new();
        let mut side_bar = ProbeGrid120x260::new();
        side_bar.fill_all(22, 24, 30);
        side_bar.fill_rect(2, 95, 3, 165, 235, 235, 235);
        assert!(analyze_frame_grid(&side_bar, false).has_landscape_side_bar);
        assert_eq!(t_side.update_sample(&side_bar, 1206, 2622, 500, false), Some(270));

        // 6. Thin <= 3px Red YouTube Portrait Progress Bar must NOT trigger KONAMI red or rotate
        let mut yt_port = ProbeGrid120x260::new();
        yt_port.fill_all(18, 18, 22);
        yt_port.fill_rect(12, 82, 108, 84, 230, 15, 15);
        yt_port.fill_rect(46, 4, 74, 9, 5, 5, 5);
        for x in (10..=26).step_by(3) {
            yt_port.fill_rect(x, 4, x + 1, 8, 235, 235, 235);
        }
        for x in (90..=110).step_by(4) {
            yt_port.fill_rect(x, 4, x + 1, 8, 235, 235, 235);
        }
        let res_ytp = analyze_frame_grid(&yt_port, false);
        assert!(!res_ytp.is_konami_red_intro);
        assert!(!res_ytp.has_youtube_landscape_osd);
        assert!(res_ytp.has_true_portrait_status_bar);
        assert_eq!(res_ytp.suggested_orientation, OrientationMode::Portrait0);

        // 7. YouTube 16:9 Fullscreen takes 2 votes (from portrait)
        let mut t_yt = HevcAutoRotateTracker::new();
        let mut yt_fs = ProbeGrid120x260::new();
        yt_fs.fill_all(100, 110, 120);
        yt_fs.fill_rect(0, 0, 119, 10, 5, 5, 5);
        yt_fs.fill_rect(0, 240, 119, 259, 5, 5, 5);
        assert_eq!(t_yt.update_sample(&yt_fs, 1206, 2622, 1_000, false), None);
        assert_eq!(t_yt.update_sample(&yt_fs, 1206, 2622, 2_000, false), Some(270));

        // 8. Passive Exit -> iOS Settings Light Chrome takes 3 votes when passive
        let mut settings = ProbeGrid120x260::new();
        settings.fill_all(245, 245, 248);
        assert_eq!(t_yt.update_sample(&settings, 1206, 2622, 3_000, false), None);
        assert_eq!(t_yt.update_sample(&settings, 1206, 2622, 4_000, false), None);
        assert_eq!(t_yt.update_sample(&settings, 1206, 2622, 5_000, false), Some(0));

        // 9. iOS Home Screen 4-slot + 3-gap dock passive exit (3 votes) & Home-Nav 1-vote fast exit
        let mut home = ProbeGrid120x260::new();
        home.fill_all(28, 32, 40);
        build_ios_home_dock_4slot_icons(&mut home);
        assert!(analyze_frame_grid(&home, true).has_ios_home_screen_dock);

        // 10. Manual Override opens 30,000 ms cooldown
        t_yt.notify_manual_override(10_000, false);
        assert_eq!(t_yt.update_sample(&konami, 1206, 2622, 15_000, false), None);
        assert_eq!(t_yt.update_sample(&konami, 1206, 2622, 39_500, false), None);
        assert_eq!(t_yt.update_sample(&konami, 1206, 2622, 40_100, false), Some(270));

        // 11. nav_home_recent opens 5,000 ms fast-exit & 2,800 ms grace and snaps state to Portrait
        t_yt.notify_nav_home_recent(50_000);
        assert!(!t_yt.is_landscape());

        // 12. Native wide frame (raw_w > raw_h) skips synthetic rotation
        let mut t_wide = HevcAutoRotateTracker::new();
        assert_eq!(t_wide.update_sample(&konami, 1920, 888, 1_000, false), None);

        // 13. iPad profile skips auto-rotate
        assert_eq!(t_wide.update_sample(&konami, 1206, 2622, 1_000, true), None);

        // 14. reset() clears all state cleanly
        t_yt.reset();
        assert!(!t_yt.is_landscape());
        assert_eq!(t_yt.last_suggested, OrientationMode::Portrait0);
    }

    #[test]
    fn test_tahap283_autorotate_home_dock_3gap_4slot_never_triggers_on_efootball_jerseys() {
        let mut ef = ProbeGrid120x260::new();
        ef.fill_all(25, 35, 70);
        ef.fill_rect(12, 40, 108, 180, 220, 40, 130);
        ef.fill_rect(12, 218, 108, 248, 220, 40, 130);
        let res_ef = analyze_frame_grid(&ef, true);
        assert!(
            !res_ef.has_ios_home_screen_dock,
            "Continuous colorful eFootball jersey across dock band must NOT trigger Home Dock"
        );
        assert_eq!(res_ef.suggested_orientation, OrientationMode::Landscape270);

        let mut dock = ProbeGrid120x260::new();
        dock.fill_all(32, 36, 44);
        build_ios_home_dock_4slot_icons(&mut dock);
        let res_dock = analyze_frame_grid(&dock, true);
        assert!(res_dock.has_ios_home_screen_dock);
        assert_eq!(res_dock.suggested_orientation, OrientationMode::Portrait0);
    }

    #[test]
    fn test_tahap284_instagram_light_feed_dark_post_stories_never_false_rotates() {
        let ig_light = build_instagram_light_feed_dark_post_grid();
        let res = analyze_frame_grid(&ig_light, false);
        assert!(res.has_top_white_chrome, "Light header must be detected");
        assert!(res.has_bottom_white_nav_bar, "White bottom nav must be detected");
        assert!(res.has_light_portrait_app_chrome);
        assert!(res.is_definite_portrait_app_screen);
        assert!(!res.has_landscape_side_bar, "Full-bleed dark photo + white bg must NOT trigger landscape side bar");
        assert!(!res.is_landscape_menu);
        assert_eq!(res.suggested_orientation, OrientationMode::Portrait0);

        let mut tracker = HevcAutoRotateTracker::new();
        for ts in [1_000u64, 2_000, 3_000] {
            assert_eq!(tracker.update_sample(&ig_light, 1216, 2656, ts, false), None);
            assert!(!tracker.is_landscape());
        }
    }

    #[test]
    fn test_tahap284_instagram_light_feed_exits_landscape_passively_in_3_samples() {
        let ig_light = build_instagram_light_feed_dark_post_grid();
        let res = analyze_frame_grid(&ig_light, true);
        assert!(res.exit_by_light_portrait_chrome);
        assert!(res.strict_portrait_exit_proof);
        assert!(res.can_passive_exit_to_portrait);
        assert_eq!(res.suggested_orientation, OrientationMode::Portrait0);

        let mut tracker = HevcAutoRotateTracker::new();
        tracker.set_currently_landscape(true);
        assert_eq!(tracker.update_sample(&ig_light, 1216, 2656, 1_000, false), None);
        assert_eq!(tracker.update_sample(&ig_light, 1216, 2656, 2_000, false), None);
        assert_eq!(tracker.update_sample(&ig_light, 1216, 2656, 3_000, false), Some(0));
        assert!(!tracker.is_landscape());
    }

    #[test]
    fn test_tahap284_instagram_reels_dark_mode_and_full_red_reel_never_false_rotates() {
        // (a) Dark Reels with thin "Reels Friends", white clock/battery, bright right action icons, & 5 OLED bottom icons
        let reels_dark = build_instagram_reels_dark_mode_grid(false);
        for is_land in [false, true] {
            let res = analyze_frame_grid(&reels_dark, is_land);
            assert!(!res.top_pillar_is_black, "Reels status clock/battery must veto top_pillar_is_black");
            assert!(!res.has_landscape_side_bar, "Right-edge Reels action icons must NOT trigger side bar");
            assert!(res.has_instagram_reels_dark_chrome, "Peak-contrast Reels chrome must match");
            assert!(res.has_instagram_dark_mode_chrome, "5-cluster OLED bottom nav must match");
            if is_land {
                assert!(res.exit_by_instagram_dark_mode);
            }
            assert_eq!(res.suggested_orientation, OrientationMode::Portrait0);
        }

        // (b) Full vivid red video/ad inside Instagram Reels Dark Mode must NOT trigger KONAMI red intro
        let reels_red = build_instagram_reels_dark_mode_grid(true);
        let res_red = analyze_frame_grid(&reels_red, false);
        assert!(res_red.has_instagram_dark_mode_chrome);
        assert!(!res_red.is_konami_red_intro, "Full red video inside Reels must NEVER trigger KONAMI red");
        assert_eq!(res_red.suggested_orientation, OrientationMode::Portrait0);
    }

    #[test]
    fn test_tahap284_efootball_youtube_fullscreen_home_2800ms_grace_and_tiktok_splash_blocker() {
        // 1. eFootball KONAMI red 1-vote entry + Home click blocks re-entry for 2,800 ms
        let mut tr = HevcAutoRotateTracker::new();
        let mut konami = ProbeGrid120x260::new();
        konami.fill_all(210, 15, 20);
        assert_eq!(tr.update_sample(&konami, 1216, 2656, 1_000, false), Some(270));
        tr.notify_nav_home_recent(2_000);
        assert!(!tr.is_landscape());
        // Blocked inside 2,800 ms window (t = 3_000 < 4_800 and 4_700 < 4_800)
        assert_eq!(tr.update_sample(&konami, 1216, 2656, 3_000, false), None);
        assert_eq!(tr.update_sample(&konami, 1216, 2656, 4_700, false), None);
        // Unblocked at 4_900 >= 4_800
        assert_eq!(tr.update_sample(&konami, 1216, 2656, 4_900, false), Some(270));

        // 2. Nav-home 5,000 ms fast-exit (1 vote) when already landscape
        let ig_light = build_instagram_light_feed_dark_post_grid();
        let mut tr_nav = HevcAutoRotateTracker::new();
        tr_nav.notify_nav_home_recent(10_000);
        tr_nav.set_currently_landscape(true);
        assert_eq!(tr_nav.update_sample(&ig_light, 1216, 2656, 14_500, false), Some(0));

        // 3. App splash screen blocks landscape entry even if another edge heuristic would fire
        let mut splash = ProbeGrid120x260::new();
        splash.fill_all(4, 4, 4);
        splash.fill_rect(42, 115, 78, 145, 230, 230, 230);
        let res_sp = analyze_frame_grid(&splash, false);
        assert!(res_sp.is_app_splash_screen);
        let mut tr_sp = HevcAutoRotateTracker::new();
        assert_eq!(tr_sp.update_sample(&splash, 1216, 2656, 1_000, false), None);
        assert_eq!(tr_sp.last_reason(), "app_splash_screen_blocked");
    }

    #[test]
    fn test_tahap284_last_reason_telemetry_non_empty_and_zero_pii() {
        let ig_light = build_instagram_light_feed_dark_post_grid();
        let res_p = analyze_frame_grid(&ig_light, false);
        let res_exit = analyze_frame_grid(&ig_light, true);
        let mut konami = ProbeGrid120x260::new();
        konami.fill_all(210, 15, 20);
        let res_k = analyze_frame_grid(&konami, false);

        for reason in [res_p.last_reason, res_exit.last_reason, res_k.last_reason] {
            assert!(!reason.is_empty());
            assert!(reason.is_ascii());
        }
        assert_eq!(res_p.last_reason, "definite_portrait_app_screen");
        assert_eq!(res_exit.last_reason, "exit_by_light_portrait_chrome");
        assert_eq!(res_k.last_reason, "konami_red_intro");
    }
}
