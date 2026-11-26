//! AirPlay receiver advertisement & `GET /info` binary plist builder (`hevc: false` / H.264 `avc1`).

use plist::{Dictionary, Value};

use super::hex_encode;
use super::identity::AirPlayIdentity;

pub const DEFAULT_PORT: u16 = 7100;
pub const MODEL: &str = "AppleTV3,2";
pub const SOURCE_VERSION: &str = "220.68";
pub const PI: &str = "2e388006-13ba-4041-9a67-25dd4a43d536";
/// Advertised H.264 sweet-spot HD+ Retina display dimensions (`2160 x 2160` @ 60 fps):
/// - Portrait (`9:19.5` modern iPhone): `~994 x 2160` (`2.15 MP`, 4x sharper than `498 x 1080`,
///   while cutting ~29% Wi-Fi encode/TCP/WebCodecs pixel load vs `2560 x 2560` for ultra-low latency!)
/// - Landscape (`19.5:9`): `~2160 x 994` (`2.15 MP` crisp 60 fps)
pub const DISPLAY_SIZE: (u32, u32) = (2160, 2160);

/// Apple TV 3 AirPlay feature mask; bit 42 (`0x400` in hi) enables HEVC (`false` -> `0` forces H.264).
pub fn feature_bits(hevc: bool) -> (u32, u32) {
    (0x5A7F_FEE6, if hevc { 0x400 } else { 0 })
}

pub struct ReceiverInfo<'a> {
    pub id: &'a AirPlayIdentity,
    pub name: &'a str,
    pub hevc: bool,
}

impl ReceiverInfo<'_> {
    pub fn features_str(&self) -> String {
        let (lo, hi) = feature_bits(self.hevc);
        format!("0x{lo:X},0x{hi:X}")
    }

    pub fn txt_airplay(&self) -> Vec<(&'static str, String)> {
        vec![
            ("deviceid", self.id.mac.clone()),
            ("features", self.features_str()),
            ("flags", "0x4".to_string()),
            ("model", MODEL.to_string()),
            ("pk", hex_encode(&self.id.public_key)),
            ("pi", PI.to_string()),
            ("srcvers", SOURCE_VERSION.to_string()),
            ("vv", "2".to_string()),
        ]
    }

    pub fn txt_raop(&self) -> Vec<(&'static str, String)> {
        vec![
            ("ch", "2".to_string()),
            ("cn", "0,1,2,3".to_string()),
            ("da", "true".to_string()),
            ("et", "0,3,5".to_string()),
            ("vv", "2".to_string()),
            ("ft", self.features_str()),
            ("am", MODEL.to_string()),
            ("md", "0,1,2".to_string()),
            ("rhd", "5.6.0.0".to_string()),
            ("pw", "false".to_string()),
            ("sf", "0x4".to_string()),
            ("sr", "44100".to_string()),
            ("ss", "16".to_string()),
            ("sv", "false".to_string()),
            ("tp", "UDP".to_string()),
            ("txtvers", "1".to_string()),
            ("vs", SOURCE_VERSION.to_string()),
            ("vn", "65537".to_string()),
            ("pk", hex_encode(&self.id.public_key)),
        ]
    }

    pub fn encode_txt_wire(txt: &[(&str, String)]) -> Vec<u8> {
        let mut out = Vec::new();
        for (k, v) in txt {
            let item = format!("{k}={v}");
            let bytes = item.as_bytes();
            if bytes.len() <= 255 {
                out.push(bytes.len() as u8);
                out.extend_from_slice(bytes);
            }
        }
        out
    }

    /// Builds the `GET /info` response in Apple `bplist00` format.
    pub fn build_info_bplist(&self, content_type: Option<&str>, body: &[u8]) -> Vec<u8> {
        let mut res = Dictionary::new();
        let qualifier = content_type
            .filter(|t| t.to_ascii_lowercase().contains("binary-plist"))
            .and_then(|_| plist::from_bytes(body).ok())
            .and_then(|v| {
                v.as_dictionary()?
                    .get("qualifier")?
                    .as_array()?
                    .first()?
                    .as_string()
                    .map(str::to_owned)
            });

        match qualifier.as_deref() {
            Some("txtAirPlay") => {
                res.insert(
                    "txtAirPlay".into(),
                    Value::Data(Self::encode_txt_wire(&self.txt_airplay())),
                );
            }
            Some("txtRAOP") => {
                res.insert(
                    "txtRAOP".into(),
                    Value::Data(Self::encode_txt_wire(&self.txt_raop())),
                );
            }
            _ => {}
        }
        // Tahap 256 Pilar 1 (1:1 zenlink-airplay/src/info.rs:114-116):
        // When iOS sends GET /info with a non-empty Content-Type (qualifier query),
        // reply ONLY with the qualifier dictionary (not the full info blob).
        if content_type.is_some_and(|t| !t.trim().is_empty()) {
            return plist::to_bytes_bplist00(&Value::Dictionary(res)).unwrap_or_default();
        }

        let (lo, hi) = feature_bits(self.hevc);
        let (w, h) = DISPLAY_SIZE;
        let dict_of = |items: Vec<(&str, Value)>| -> Value {
            let mut d = Dictionary::new();
            for (k, v) in items {
                d.insert(k.to_string(), v);
            }
            Value::Dictionary(d)
        };
        let latency = |t: u64| {
            dict_of(vec![
                ("type", Value::from(t)),
                ("audioType", Value::from("default")),
                ("inputLatencyMicros", Value::from(0u64)),
                ("outputLatencyMicros", Value::from(false)),
            ])
        };
        let audio_fmt = |t: u64| {
            dict_of(vec![
                ("type", Value::from(t)),
                ("audioInputFormats", Value::from(0x03FF_FFFCu64)),
                ("audioOutputFormats", Value::from(0x03FF_FFFCu64)),
            ])
        };

        res.insert("deviceID".into(), Value::from(self.id.mac.clone()));
        res.insert("macAddress".into(), Value::from(self.id.mac.clone()));
        res.insert("pk".into(), Value::Data(self.id.public_key.to_vec()));
        res.insert("features".into(), Value::from(((hi as u64) << 32) | (lo as u64)));
        res.insert("hevc".into(), Value::Boolean(self.hevc));
        res.insert("name".into(), Value::from(self.name));
        res.insert("pi".into(), Value::from(PI));
        res.insert("vv".into(), Value::from(2u64));
        res.insert("statusFlags".into(), Value::from(68u64));
        res.insert("keepAliveLowPower".into(), Value::Boolean(true));
        res.insert("sourceVersion".into(), Value::from(SOURCE_VERSION));
        res.insert("keepAliveSendStatsAsBody".into(), Value::Boolean(true));
        res.insert("model".into(), Value::from(MODEL));
        res.insert("initialVolume".into(), Value::Real(0.0));
        res.insert(
            "audioLatencies".into(),
            Value::Array(vec![latency(100), latency(101)]),
        );
        res.insert(
            "audioFormats".into(),
            Value::Array(vec![audio_fmt(100), audio_fmt(101)]),
        );
        res.insert(
            "displays".into(),
            Value::Array(vec![dict_of(vec![
                ("uuid", Value::from("e0ff8a27-6738-3d56-8a16-cc53aacee925")),
                ("widthPhysical", Value::from(0u64)),
                ("heightPhysical", Value::from(0u64)),
                ("width", Value::from(w as u64)),
                ("height", Value::from(h as u64)),
                ("widthPixels", Value::from(w as u64)),
                ("heightPixels", Value::from(h as u64)),
                ("rotation", Value::Boolean(false)),
                ("refreshRate", Value::Real(1.0 / 60.0)),
                ("maxFPS", Value::from(60u64)),
                ("overscanned", Value::Boolean(false)),
                ("features", Value::from(14u64)),
            ])]),
        );

        plist::to_bytes_bplist00(&Value::Dictionary(res)).unwrap_or_default()
    }
}
