//! CoreDevice DisplayService packet parser and H.265 / AAC-ELD stream demuxer.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HevcNalType {
    Vps,        // 32
    Sps,        // 33
    Pps,        // 34
    IdrKeyframe,// 19 or 20
    Slice,      // 0..9
    Other(u8),
}

impl HevcNalType {
    pub fn from_header(first_byte: u8) -> Self {
        let nal_unit_type = (first_byte >> 1) & 0x3F;
        match nal_unit_type {
            32 => Self::Vps,
            33 => Self::Sps,
            34 => Self::Pps,
            19 | 20 => Self::IdrKeyframe,
            0..=9 => Self::Slice,
            other => Self::Other(other),
        }
    }

    pub fn is_keyframe(&self) -> bool {
        matches!(self, Self::IdrKeyframe)
    }

    pub fn is_parameter_set(&self) -> bool {
        matches!(self, Self::Vps | Self::Sps | Self::Pps)
    }
}

/// Parsed H.265 NAL unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HevcNalUnit<'a> {
    pub nal_type: HevcNalType,
    pub payload: &'a [u8],
}

/// Extract HEVC NAL units from an Annex B byte stream (0x00000001 or 0x000001 start codes).
pub fn parse_annex_b_nalus(data: &[u8]) -> Vec<HevcNalUnit<'_>> {
    let mut nalus = Vec::new();
    let mut start_indices = Vec::new();

    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i] == 0x00 && data[i + 1] == 0x00 {
            if data[i + 2] == 0x01 {
                start_indices.push((i, i + 3));
                i += 3;
                continue;
            } else if i + 4 <= data.len() && data[i + 2] == 0x00 && data[i + 3] == 0x01 {
                start_indices.push((i, i + 4));
                i += 4;
                continue;
            }
        }
        i += 1;
    }

    for idx in 0..start_indices.len() {
        let (_, nal_data_start) = start_indices[idx];
        let nal_end = if idx + 1 < start_indices.len() {
            start_indices[idx + 1].0
        } else {
            data.len()
        };

        if nal_data_start < nal_end {
            let nal_bytes = &data[nal_data_start..nal_end];
            let nal_type = HevcNalType::from_header(nal_bytes[0]);
            nalus.push(HevcNalUnit {
                nal_type,
                payload: nal_bytes,
            });
        }
    }

    nalus
}

/// AAC-ELD audio packet demuxer for CoreDevice display service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AacEldPacket {
    pub cookie: Vec<u8>,
    pub access_unit: Vec<u8>,
    pub sample_rate: u32,
    pub channels: u16,
}

impl AacEldPacket {
    pub fn new(raw_cookie: &[u8], access_unit: &[u8]) -> Self {
        let mut cookie = if raw_cookie.is_empty() {
            vec![0xF8, 0xE8, 0x50, 0x00]
        } else {
            raw_cookie.to_vec()
        };
        // Mandatory frameLengthFlag bit 0x10 patch on byte 2
        if cookie.len() >= 3 {
            cookie[2] |= 0x10;
        }

        Self {
            cookie,
            access_unit: access_unit.to_vec(),
            sample_rate: 48000,
            channels: 2,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hevc_nal_type_parsing() {
        // VPS: 32 << 1 = 64 (0x40)
        assert_eq!(HevcNalType::from_header(0x40), HevcNalType::Vps);
        // SPS: 33 << 1 = 66 (0x42)
        assert_eq!(HevcNalType::from_header(0x42), HevcNalType::Sps);
        // PPS: 34 << 1 = 68 (0x44)
        assert_eq!(HevcNalType::from_header(0x44), HevcNalType::Pps);
        // IDR: 19 << 1 = 38 (0x26)
        assert_eq!(HevcNalType::from_header(0x26), HevcNalType::IdrKeyframe);
        // Slice: 1 << 1 = 2 (0x02)
        assert_eq!(HevcNalType::from_header(0x02), HevcNalType::Slice);
    }

    #[test]
    fn test_parse_annex_b() {
        let stream = [
            0x00, 0x00, 0x00, 0x01, 0x40, 0x01, 0x0C, // VPS
            0x00, 0x00, 0x00, 0x01, 0x42, 0x01, 0x01, // SPS
            0x00, 0x00, 0x01, 0x44, 0x08,             // PPS
            0x00, 0x00, 0x00, 0x01, 0x26, 0x01,       // IDR Keyframe
        ];
        let nalus = parse_annex_b_nalus(&stream);
        assert_eq!(nalus.len(), 4);
        assert_eq!(nalus[0].nal_type, HevcNalType::Vps);
        assert_eq!(nalus[1].nal_type, HevcNalType::Sps);
        assert_eq!(nalus[2].nal_type, HevcNalType::Pps);
        assert_eq!(nalus[3].nal_type, HevcNalType::IdrKeyframe);
        assert!(nalus[3].nal_type.is_keyframe());
    }

    #[test]
    fn test_aac_eld_packet_patch() {
        let raw_cookie = [0xF8, 0xE6, 0x40, 0x00];
        let packet = AacEldPacket::new(&raw_cookie, &[0xDE, 0xAD, 0xBE, 0xEF]);
        assert_eq!(packet.cookie[2], 0x50); // 0x40 | 0x10 = 0x50
        assert_eq!(packet.sample_rate, 48000);
        assert_eq!(packet.channels, 2);
    }
}
