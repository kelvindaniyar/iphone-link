//! Windows Media Foundation H.265 / HEVC hardware decoder configuration and macroblock padding auto-trim.

/// DXGI format NV12 constant (0x3231564E / 103).
pub const DXGI_FORMAT_NV12: u32 = 103;

/// Low-latency decode flag for Media Foundation transforms.
pub const MF_LOW_LATENCY: bool = true;

/// Macroblock padding trim parameters for Apple CoreDevice H.265 streams.
/// Default Apple padding: top = 0, bottom = 58, left = 0, right = 14.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct H265Trim {
    pub top: u32,
    pub bottom: u32,
    pub left: u32,
    pub right: u32,
}

impl Default for H265Trim {
    fn default() -> Self {
        Self {
            top: 0,
            bottom: 58,
            left: 0,
            right: 14,
        }
    }
}

impl H265Trim {
    pub const ZERO: Self = Self {
        top: 0,
        bottom: 0,
        left: 0,
        right: 0,
    };

    /// Calculate clean visible dimensions after stripping macroblock padding.
    pub fn trim_dimensions(&self, raw_width: u32, raw_height: u32) -> (u32, u32) {
        let clean_w = raw_width.saturating_sub(self.left + self.right);
        let clean_h = raw_height.saturating_sub(self.top + self.bottom);
        (clean_w, clean_h)
    }
}

/// Media Foundation decoder configuration profile.
#[derive(Debug, Clone)]
pub struct MfDecoderConfig {
    pub low_latency: bool,
    pub surface_format: u32,
    pub trim: H265Trim,
}

impl Default for MfDecoderConfig {
    fn default() -> Self {
        Self {
            low_latency: MF_LOW_LATENCY,
            surface_format: DXGI_FORMAT_NV12,
            trim: H265Trim::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trim_dimensions() {
        let trim = H265Trim::default();
        // iPhone standard raw stream 1216x2656 trimmed by 14px width and 58px height -> 1202x2598
        let (clean_w, clean_h) = trim.trim_dimensions(1216, 2656);
        assert_eq!(clean_w, 1202);
        assert_eq!(clean_h, 2598);

        let (zero_w, zero_h) = H265Trim::ZERO.trim_dimensions(1216, 2656);
        assert_eq!(zero_w, 1216);
        assert_eq!(zero_h, 2656);
    }

    #[test]
    fn test_decoder_config() {
        let cfg = MfDecoderConfig::default();
        assert!(cfg.low_latency);
        assert_eq!(cfg.surface_format, DXGI_FORMAT_NV12);
        assert_eq!(cfg.trim.bottom, 58);
        assert_eq!(cfg.trim.right, 14);
    }
}
