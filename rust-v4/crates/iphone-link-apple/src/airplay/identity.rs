//! Persistent neutral AirPlay receiver identity (`Ed25519` 32-byte seed + locally-administered MAC).
//!
//! Stored in `%LOCALAPPDATA%\iPhoneLink\airplay_identity.json` (or `IPHONE_LINK_AIRPLAY_IDENTITY_PATH`
//! during isolated tests) so iOS recognizes a single stable `"iPhone Link"` receiver across sessions.

use std::path::{Path, PathBuf};

use crate::rp_wifi_tls::{ed25519_public_from_seed, ed25519_sign};
use crate::win_cng::fill_random;

use super::{hex_decode, hex_encode};

pub const ENV_AIRPLAY_IDENTITY_PATH: &str = "IPHONE_LINK_AIRPLAY_IDENTITY_PATH";
pub const DEFAULT_RECEIVER_NAME: &str = "iPhone Link";

#[derive(Debug, Clone)]
pub struct AirPlayIdentity {
    pub seed: [u8; 32],
    pub public_key: [u8; 32],
    /// Colon-separated uppercase locally administered unicast MAC (`"XX:XX:XX:XX:XX:XX"`).
    pub mac: String,
}

impl AirPlayIdentity {
    pub fn from_seed_and_mac(seed: [u8; 32], mac: String) -> Self {
        let public_key = ed25519_public_from_seed(&seed);
        Self {
            seed,
            public_key,
            mac,
        }
    }

    pub fn sign(&self, message: &[u8]) -> [u8; 64] {
        ed25519_sign(&self.seed, message)
    }

    pub fn mac_compact(&self) -> String {
        self.mac.replace(':', "")
    }

    pub fn default_path() -> Option<PathBuf> {
        if let Some(custom) = std::env::var_os(ENV_AIRPLAY_IDENTITY_PATH) {
            let s = custom.to_string_lossy();
            if !s.trim().is_empty() {
                return Some(PathBuf::from(s.trim()));
            }
        }
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            return Some(
                PathBuf::from(local)
                    .join("iPhoneLink")
                    .join("airplay_identity.json"),
            );
        }
        None
    }

    pub fn load_default() -> Self {
        match Self::default_path() {
            Some(path) => Self::load_or_create(&path),
            None => Self::generate_ephemeral(),
        }
    }

    pub fn generate_ephemeral() -> Self {
        let mut seed = [0u8; 32];
        let mut mac_raw = [0u8; 6];
        let _ = fill_random(&mut seed);
        let _ = fill_random(&mut mac_raw);
        mac_raw[0] = (mac_raw[0] | 0x02) & 0xFE; // locally administered unicast MAC
        let mac = mac_raw
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(":");
        Self::from_seed_and_mac(seed, mac)
    }

    pub fn load_or_create(path: &Path) -> Self {
        if let Ok(raw) = std::fs::read_to_string(path) {
            if let Ok(val) = serde_json::from_str(&raw) {
                let seed_opt = val
                    .get("seed")
                    .and_then(|v| v.as_str())
                    .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
                    .and_then(|s| <[u8; 32]>::try_from(hex_decode(s)).ok());
                let mac_opt = val
                    .get("mac")
                    .and_then(|v| v.as_str())
                    .filter(|m| m.len() == 17 && m.is_ascii())
                    .map(str::to_string);
                if let (Some(seed), Some(mac)) = (seed_opt, mac_opt) {
                    return Self::from_seed_and_mac(seed, mac);
                }
            }
        }

        let created = Self::generate_ephemeral();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let body = format!(
            r#"{{"seed":"{}","mac":"{}"}}"#,
            hex_encode(&created.seed),
            created.mac
        );
        let _ = std::fs::write(path, body);
        created
    }
}
