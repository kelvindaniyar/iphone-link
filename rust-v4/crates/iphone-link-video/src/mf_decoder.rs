//! Windows Media Foundation H.265 hardware decoder prober, macroblock trimmer,
//! smart downscaler, and inverse touch matrix for portrait and landscape rotations.

use std::ffi::c_void;

pub const MF_VERSION: u32 = 0x0002_0070;

#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryW(lpLibFileName: *const u16) -> *mut c_void;
    fn GetProcAddress(hModule: *mut c_void, lpProcName: *const u8) -> *const c_void;
    fn FreeLibrary(hLibModule: *mut c_void) -> i32;
}

type FnMFStartup = unsafe extern "system" fn(u32, u32) -> i32;
type FnMFShutdown = unsafe extern "system" fn() -> i32;

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Probe real Windows Media Foundation subsystem (mfplat.dll) startup and shutdown.
pub fn probe_media_foundation() -> Result<bool, String> {
    let mfplat_wide = to_wide("mfplat.dll");
    let h_mfplat = unsafe { LoadLibraryW(mfplat_wide.as_ptr()) };
    if h_mfplat.is_null() {
        return Err("Could not load mfplat.dll (Media Feature Pack missing)".to_string());
    }

    unsafe {
        let p_startup = GetProcAddress(h_mfplat, b"MFStartup\0".as_ptr());
        let p_shutdown = GetProcAddress(h_mfplat, b"MFShutdown\0".as_ptr());

        if p_startup.is_null() || p_shutdown.is_null() {
            FreeLibrary(h_mfplat);
            return Err("Failed to resolve MFStartup or MFShutdown".to_string());
        }

        let startup: FnMFStartup = std::mem::transmute(p_startup);
        let shutdown: FnMFShutdown = std::mem::transmute(p_shutdown);

        let hr = startup(MF_VERSION, 0);
        let ok = hr == 0;
        let _ = shutdown();
        FreeLibrary(h_mfplat);

        if ok {
            Ok(true)
        } else {
            Err(format!("MFStartup failed with HRESULT 0x{:08X}", hr))
        }
    }
}

/// 4-Side Edge Crop parameters (0..24 px per side).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EdgeCrop {
    pub top: u32,
    pub bottom: u32,
    pub left: u32,
    pub right: u32,
}

/// Downscale factor setting for performance optimization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownscaleFactor {
    Native1x,     // 1:1 original resolution (1216x2656)
    Scale75,      // 0.75x high-performance downscale (912x1992)
}

/// Screen rotation angle for touch coordinate inversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RotationAngle {
    Deg0,   // Portrait
    Deg90,  // Landscape Left
    Deg180, // Inverted Portrait
    Deg270, // Landscape Right (eFootball 2027 Asymmetric Lock)
}

/// Inverse Touch Matrix: converts PC viewport normalized coordinates (0..65535)
/// into physical iOS device touch coordinates based on active rotation.
pub fn inverse_touch_transform(pc_x: u16, pc_y: u16, rotation: RotationAngle) -> (u16, u16) {
    match rotation {
        RotationAngle::Deg0 => (pc_x, pc_y),
        RotationAngle::Deg90 => (pc_y, 65535 - pc_x),
        RotationAngle::Deg180 => (65535 - pc_x, 65535 - pc_y),
        RotationAngle::Deg270 => (65535 - pc_y, pc_x),
    }
}

/// HEVC Hardware Video Processing Pipeline configuration.
#[derive(Debug, Clone)]
pub struct HevcPipelineConfig {
    pub raw_width: u32,
    pub raw_height: u32,
    pub macroblock_trim: (u32, u32, u32, u32), // (top, bottom, left, right)
    pub edge_crop: EdgeCrop,
    pub downscale: DownscaleFactor,
    pub rotation: RotationAngle,
}

impl Default for HevcPipelineConfig {
    fn default() -> Self {
        Self {
            raw_width: 1216,
            raw_height: 2656,
            macroblock_trim: (0, 58, 0, 14), // Default Apple H.265 trim
            edge_crop: EdgeCrop::default(),
            downscale: DownscaleFactor::Native1x,
            rotation: RotationAngle::Deg0,
        }
    }
}

impl HevcPipelineConfig {
    /// Calculate final presentation dimensions after macroblock trim, edge crop, and downscaling.
    pub fn calculate_output_dimensions(&self) -> (u32, u32) {
        let (top, bottom, left, right) = self.macroblock_trim;
        let mut w = self.raw_width.saturating_sub(left + right);
        let mut h = self.raw_height.saturating_sub(top + bottom);

        // Apply edge crop
        w = w.saturating_sub(self.edge_crop.left + self.edge_crop.right);
        h = h.saturating_sub(self.edge_crop.top + self.edge_crop.bottom);

        // Apply downscaling
        match self.downscale {
            DownscaleFactor::Native1x => (w, h),
            DownscaleFactor::Scale75 => ((w * 3) / 4, (h * 3) / 4),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_media_foundation_probe() {
        let res = probe_media_foundation();
        assert!(res.is_ok(), "MF probe failed: {:?}", res.err());
        assert!(res.unwrap());
    }

    #[test]
    fn test_output_dimension_calculations() {
        let cfg_native = HevcPipelineConfig::default();
        let (w1, h1) = cfg_native.calculate_output_dimensions();
        assert_eq!(w1, 1202);
        assert_eq!(h1, 2598);

        let mut cfg_downscale = HevcPipelineConfig::default();
        cfg_downscale.downscale = DownscaleFactor::Scale75;
        let (w2, h2) = cfg_downscale.calculate_output_dimensions();
        assert_eq!(w2, (1202 * 3) / 4); // 901
        assert_eq!(h2, (2598 * 3) / 4); // 1948
    }

    #[test]
    fn test_inverse_touch_matrix_rotations() {
        // Test Deg0 (Portrait)
        assert_eq!(inverse_touch_transform(1000, 2000, RotationAngle::Deg0), (1000, 2000));

        // Test Deg270 (eFootball 2027 Landscape Lock)
        // pc_x = 1000, pc_y = 2000 -> ios_x = 65535 - 2000 = 63535, ios_y = 1000
        let (ios_x, ios_y) = inverse_touch_transform(1000, 2000, RotationAngle::Deg270);
        assert_eq!(ios_x, 63535);
        assert_eq!(ios_y, 1000);

        // Test Deg90 (Landscape Left)
        // pc_x = 1000, pc_y = 2000 -> ios_x = 2000, ios_y = 65535 - 1000 = 64535
        let (ios90_x, ios90_y) = inverse_touch_transform(1000, 2000, RotationAngle::Deg90);
        assert_eq!(ios90_x, 2000);
        assert_eq!(ios90_y, 64535);

        // Test Deg180 (Inverted Portrait)
        let (ios180_x, ios180_y) = inverse_touch_transform(1000, 2000, RotationAngle::Deg180);
        assert_eq!(ios180_x, 65535 - 1000);
        assert_eq!(ios180_y, 65535 - 2000);
    }
}
