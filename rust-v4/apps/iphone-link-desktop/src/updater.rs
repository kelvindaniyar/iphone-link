//! GitHub OTA Auto-Updater Engine.
//!
//! Provides:
//! - Update channel selection: Stable vs Beta
//! - Semantic version parsing and comparison (v4.0.0 vs v3.0.0)
//! - Zero-dependency SHA-256 verification
//! - Protected config files whitelist (user_settings.json, wifi_state.json, typed_history.json)

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateChannel {
    Stable,
    Beta,
}

impl Default for UpdateChannel {
    fn default() -> Self {
        UpdateChannel::Stable
    }
}

/// Semantic Versioning representation (Major.Minor.Patch).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Semver {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
    pub is_beta: bool,
}

impl Semver {
    pub fn parse(s: &str) -> Option<Self> {
        let clean = s.trim().trim_start_matches('v').trim_start_matches('V');
        let parts: Vec<&str> = clean.split('-').collect();
        let version_core = parts[0];
        let is_beta = parts.len() > 1 && parts[1].to_lowercase().contains("beta");

        let nums: Vec<&str> = version_core.split('.').collect();
        if nums.len() < 2 {
            return None;
        }

        let major = nums[0].parse::<u32>().ok()?;
        let minor = nums[1].parse::<u32>().ok()?;
        let patch = if nums.len() >= 3 {
            nums[2].parse::<u32>().ok().unwrap_or(0)
        } else {
            0
        };

        Some(Semver {
            major,
            minor,
            patch,
            is_beta,
        })
    }

    /// Checks if this version is newer than the other version.
    pub fn is_newer_than(&self, other: &Semver) -> bool {
        if self.major != other.major {
            return self.major > other.major;
        }
        if self.minor != other.minor {
            return self.minor > other.minor;
        }
        if self.patch != other.patch {
            return self.patch > other.patch;
        }
        // If versions are equal, non-beta is newer than beta
        if !self.is_beta && other.is_beta {
            return true;
        }
        false
    }
}

/// List of critical local user configuration files that must NEVER be overwritten during update.
pub const PROTECTED_CONFIG_FILES: &[&str] = &[
    "user_settings.json",
    "wifi_state.json",
    "typed_history.json",
];

/// Verifies whether a given filename is protected from OTA update overwrite.
pub fn is_protected_config_file(filename: &str) -> bool {
    let base = std::path::Path::new(filename)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or(filename);

    PROTECTED_CONFIG_FILES.contains(&base)
}

// ============================================================================
// ZERO-DEPENDENCY PURE RUST SHA-256 IMPLEMENTATION
// ============================================================================

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// Computes SHA-256 hash of data bytes and returns lower-case hexadecimal string.
pub fn compute_sha256(data: &[u8]) -> String {
    let mut h0: u32 = 0x6a09e667;
    let mut h1: u32 = 0xbb67ae85;
    let mut h2: u32 = 0x3c6ef372;
    let mut h3: u32 = 0xa54ff53a;
    let mut h4: u32 = 0x510e527f;
    let mut h5: u32 = 0x9b05688c;
    let mut h6: u32 = 0x1f83d9ab;
    let mut h7: u32 = 0x5be0cd19;

    let bit_len = (data.len() as u64) * 8;
    let mut padded = data.to_vec();
    padded.push(0x80);
    while (padded.len() + 8) % 64 != 0 {
        padded.push(0x00);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in padded.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }

        let mut a = h0;
        let mut b = h1;
        let mut c = h2;
        let mut d = h3;
        let mut e = h4;
        let mut f = h5;
        let mut g = h6;
        let mut h = h7;

        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        h0 = h0.wrapping_add(a);
        h1 = h1.wrapping_add(b);
        h2 = h2.wrapping_add(c);
        h3 = h3.wrapping_add(d);
        h4 = h4.wrapping_add(e);
        h5 = h5.wrapping_add(f);
        h6 = h6.wrapping_add(g);
        h7 = h7.wrapping_add(h);
    }

    format!(
        "{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}",
        h0, h1, h2, h3, h4, h5, h6, h7
    )
}

/// Verifies whether data matches the expected SHA-256 hexadecimal string.
pub fn verify_sha256(data: &[u8], expected_hex: &str) -> bool {
    let hash = compute_sha256(data);
    hash.eq_ignore_ascii_case(expected_hex.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_semver_parsing_and_comparison() {
        let v4 = Semver::parse("v4.0.0").unwrap();
        let v3 = Semver::parse("v3.0.0").unwrap();
        let v4_beta = Semver::parse("v4.0.0-beta.1").unwrap();

        assert!(v4.is_newer_than(&v3));
        assert!(v4.is_newer_than(&v4_beta));
        assert!(!v3.is_newer_than(&v4));
    }

    #[test]
    fn test_sha256_hash_computation() {
        // Known NIST vector for "" (empty string)
        let empty_hash = compute_sha256(b"");
        assert_eq!(
            empty_hash,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );

        // Known NIST vector for "abc"
        let abc_hash = compute_sha256(b"abc");
        assert_eq!(
            abc_hash,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(verify_sha256(b"abc", "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"));
    }

    #[test]
    fn test_protected_config_files() {
        assert!(is_protected_config_file("user_settings.json"));
        assert!(is_protected_config_file("C:/AppData/user_settings.json"));
        assert!(is_protected_config_file("wifi_state.json"));
        assert!(is_protected_config_file("typed_history.json"));
        assert!(!is_protected_config_file("iphone-link-v4.exe"));
        assert!(!is_protected_config_file("zlaac.dll"));
    }
}
