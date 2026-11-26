//! GitHub OTA Auto-Updater Engine.
//!
//! Provides:
//! - Update channel selection: Stable vs Beta
//! - Semantic version parsing and comparison (v4.0.0 vs v3.0.0)
//! - Zero-dependency SHA-256 verification
//! - Protected config files whitelist (user_settings.json, wifi_state.json, typed_history.json)

use std::time::{SystemTime, UNIX_EPOCH};

/// Canonical public GitHub repository URL (single source of truth for runtime API derivation).
pub const PUBLIC_GITHUB_REPO_URL: &str = "https://github.com/kelvindaniyar/iphone-link";

/// Current desktop application semantic version synchronized with iPhone Link v5.0.0.
pub const CURRENT_VERSION: &str = "5.0.0";

/// Derives the GitHub Releases REST API URL at runtime from `PUBLIC_GITHUB_REPO_URL`.
pub fn github_releases_api_url() -> String {
    let trimmed = PUBLIC_GITHUB_REPO_URL.trim().trim_end_matches('/');
    let slug = trimmed.strip_prefix("https://github.com/").unwrap_or("");
    format!("https://api.github.com/repos/{slug}/releases")
}

/// Strict allowlist for opening external GitHub release / asset URLs via `ShellExecuteW`.
pub fn is_allowed_github_release_url(url: &str) -> bool {
    let trimmed = url.trim();
    if trimmed.is_empty()
        || trimmed.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return false;
    }
    if let Some(rest) = trimmed.strip_prefix(PUBLIC_GITHUB_REPO_URL) {
        return rest == "/releases" || rest.starts_with("/releases/");
    }
    trimmed.starts_with("https://objects.githubusercontent.com/")
}

/// Returns `true` strictly for explicit local loopback HTTP test endpoints.
pub fn is_loopback_http_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    lower.starts_with("http://127.0.0.1:")
        || lower.starts_with("http://localhost:")
        || lower == "http://127.0.0.1"
        || lower == "http://localhost"
}

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

fn extract_beta_ordinal(version_str: &str) -> u32 {
    let lower = version_str.to_ascii_lowercase();
    if let Some((_, pre)) = lower.split_once('-') {
        let digits: String = pre.chars().filter(|c| c.is_ascii_digit()).collect();
        return digits.parse::<u32>().unwrap_or(0);
    }
    0
}

/// Strictly checks whether `latest` is a newer semantic version than `current`.
pub fn is_newer_version(current: &str, latest: &str) -> bool {
    let (Some(curr), Some(cand)) = (Semver::parse(current), Semver::parse(latest)) else {
        return false;
    };
    if cand.is_newer_than(&curr) {
        return true;
    }
    if cand.major == curr.major
        && cand.minor == curr.minor
        && cand.patch == curr.patch
        && cand.is_beta
        && curr.is_beta
    {
        return extract_beta_ordinal(latest) > extract_beta_ordinal(current);
    }
    false
}

fn escape_json_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {}
            _ => out.push(c),
        }
    }
    out
}

fn extract_json_string(body: &str, key: &str) -> Option<String> {
    let p1 = format!(r#""{}":""#, key);
    let p2 = format!(r#""{}": ""#, key);
    let rest = if let Some(idx) = body.find(&p1) {
        &body[idx + p1.len()..]
    } else if let Some(idx) = body.find(&p2) {
        &body[idx + p2.len()..]
    } else {
        return None;
    };
    let mut out = String::new();
    let mut escaped = false;
    for ch in rest.chars() {
        if escaped {
            match ch {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                _ => out.push(ch),
            }
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            return Some(out);
        } else {
            out.push(ch);
        }
    }
    None
}

fn extract_json_bool(body: &str, key: &str) -> Option<bool> {
    let p1 = format!(r#""{}":"#, key);
    let p2 = format!(r#""{}": "#, key);
    let rest = if let Some(idx) = body.find(&p1) {
        body[idx + p1.len()..].trim_start()
    } else if let Some(idx) = body.find(&p2) {
        body[idx + p2.len()..].trim_start()
    } else {
        return None;
    };
    if rest.starts_with("true") {
        Some(true)
    } else if rest.starts_with("false") {
        Some(false)
    } else {
        None
    }
}

fn extract_json_u8(body: &str, key: &str) -> Option<u8> {
    let p1 = format!(r#""{}":"#, key);
    let p2 = format!(r#""{}": "#, key);
    let rest = if let Some(idx) = body.find(&p1) {
        body[idx + p1.len()..].trim_start()
    } else if let Some(idx) = body.find(&p2) {
        body[idx + p2.len()..].trim_start()
    } else {
        return None;
    };
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse::<u16>().ok().map(|v| v.min(100) as u8)
}

/// Cached non-blocking OTA updater state held by NativeServer so `/api/update/status` responds < 1ms.
#[derive(Debug, Clone)]
pub struct UpdateStateCache {
    pub current_version: String,
    pub latest_version: String,
    pub update_available: bool,
    pub status: String,
    pub channel: String,
    pub download_url: String,
    pub release_url: String,
    pub release_notes: String,
    pub message: String,
    pub progress: u8,
    pub offline_fallback: bool,
    pub last_checked_ms: u64,
}

impl Default for UpdateStateCache {
    fn default() -> Self {
        Self {
            current_version: CURRENT_VERSION.to_string(),
            latest_version: CURRENT_VERSION.to_string(),
            update_available: false,
            status: "up_to_date".to_string(),
            channel: "stable".to_string(),
            download_url: String::new(),
            release_url: String::new(),
            release_notes: String::new(),
            message: "iPhone Link sudah versi terbaru.".to_string(),
            progress: 0,
            offline_fallback: false,
            last_checked_ms: 0,
        }
    }
}

impl UpdateStateCache {
    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    pub fn is_active_transfer(&self) -> bool {
        matches!(self.status.as_str(), "downloading" | "extracting")
    }

    pub fn set_offline_fallback(&mut self, channel: &str) {
        self.current_version = CURRENT_VERSION.to_string();
        self.latest_version = CURRENT_VERSION.to_string();
        self.update_available = false;
        self.status = "up_to_date".to_string();
        self.channel = if channel.trim().is_empty() {
            "stable".to_string()
        } else {
            channel.trim().to_string()
        };
        self.download_url.clear();
        self.release_url.clear();
        self.release_notes.clear();
        self.message = "iPhone Link sudah versi terbaru.".to_string();
        self.progress = 0;
        self.offline_fallback = true;
        self.last_checked_ms = Self::now_ms();
    }

    /// Normalize & validate Python worker JSON against strict `CURRENT_VERSION` Semver policy.
    pub fn apply_worker_payload(&mut self, body: &str, channel_fallback: &str) {
        let ch = extract_json_string(body, "channel")
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| channel_fallback.to_string());
        let raw_latest = extract_json_string(body, "latestVersion")
            .or_else(|| extract_json_string(body, "latest_version"))
            .unwrap_or_else(|| CURRENT_VERSION.to_string());
        let raw_status = extract_json_string(body, "status").unwrap_or_else(|| "up_to_date".to_string());
        let progress = extract_json_u8(body, "progress").unwrap_or(self.progress);
        let offline = extract_json_bool(body, "offlineFallback").unwrap_or(false);

        self.current_version = CURRENT_VERSION.to_string();
        self.channel = ch;
        self.progress = progress;
        self.offline_fallback = offline;
        self.last_checked_ms = Self::now_ms();

        // Active transfer / ready / failed statuses preserve progress, otherwise enforce Semver gate
        if matches!(raw_status.as_str(), "downloading" | "extracting" | "ready_to_restart" | "failed") {
            self.status = raw_status;
            if let Some(m) = extract_json_string(body, "message") {
                self.message = m;
            }
            if !raw_latest.is_empty() {
                self.latest_version = raw_latest;
            }
            return;
        }

        if is_newer_version(CURRENT_VERSION, &raw_latest) {
            self.latest_version = raw_latest.trim().trim_start_matches(['v', 'V']).to_string();
            self.update_available = true;
            self.status = "update_available".to_string();
            self.download_url = extract_json_string(body, "downloadUrl").unwrap_or_default();
            self.release_url = extract_json_string(body, "releaseUrl").unwrap_or_default();
            self.release_notes = extract_json_string(body, "releaseNotes").unwrap_or_default();
            self.message = extract_json_string(body, "message")
                .unwrap_or_else(|| format!("Tersedia pembaruan: iPhone Link v{}", self.latest_version));
        } else {
            self.latest_version = CURRENT_VERSION.to_string();
            self.update_available = false;
            self.status = "up_to_date".to_string();
            self.download_url.clear();
            self.release_url.clear();
            self.release_notes.clear();
            self.message = "iPhone Link sudah versi terbaru.".to_string();
        }
    }

    pub fn to_json(&self, auto_check_update: bool) -> String {
        let protected_json: Vec<String> = PROTECTED_CONFIG_FILES
            .iter()
            .map(|f| format!(r#""{}""#, escape_json_value(f)))
            .collect();
        format!(
            concat!(
                r#"{{"ok":true,"status":"{}","currentVersion":"{}","current_version":"{}","#,
                r#""latestVersion":"{}","latest_version":"{}","updateAvailable":{},"update_available":{},"#,
                r#""channel":"{}","autoCheckUpdate":{},"downloadUrl":"{}","releaseUrl":"{}","releaseNotes":"{}","#,
                r#""message":"{}","progress":{},"offlineFallback":{},"protectedFiles":[{}]}}"#
            ),
            escape_json_value(&self.status),
            escape_json_value(&self.current_version),
            escape_json_value(&self.current_version),
            escape_json_value(&self.latest_version),
            escape_json_value(&self.latest_version),
            self.update_available,
            self.update_available,
            escape_json_value(&self.channel),
            auto_check_update,
            escape_json_value(&self.download_url),
            escape_json_value(&self.release_url),
            escape_json_value(&self.release_notes),
            escape_json_value(&self.message),
            self.progress,
            self.offline_fallback,
            protected_json.join(","),
        )
    }
}

fn env_truthy_updater(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(|v| {
            let t = v.trim();
            t == "1" || t.eq_ignore_ascii_case("true") || t.eq_ignore_ascii_case("yes")
        })
        .unwrap_or(false)
}

#[cfg(windows)]
mod updater_winhttp {
    use std::ffi::c_void;
    use std::ptr;

    type Hinternet = *mut c_void;
    type Bool = i32;

    const WINHTTP_ACCESS_TYPE_DEFAULT_PROXY: u32 = 0;
    const WINHTTP_ACCESS_TYPE_NO_PROXY: u32 = 1;
    const WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY: u32 = 4;
    const WINHTTP_FLAG_SECURE: u32 = 0x0080_0000;
    const WINHTTP_ADDREQ_FLAG_ADD: u32 = 0x2000_0000;
    const WINHTTP_ADDREQ_FLAG_REPLACE: u32 = 0x8000_0000;
    const WINHTTP_QUERY_STATUS_CODE: u32 = 19;
    const WINHTTP_QUERY_FLAG_NUMBER: u32 = 0x2000_0000;

    #[link(name = "winhttp")]
    extern "system" {
        fn WinHttpOpen(
            psz_agent_w: *const u16,
            dw_access_type: u32,
            psz_proxy_w: *const u16,
            psz_proxy_bypass_w: *const u16,
            dw_flags: u32,
        ) -> Hinternet;
        fn WinHttpSetTimeouts(
            h_internet: Hinternet,
            n_resolve_timeout: i32,
            n_connect_timeout: i32,
            n_send_timeout: i32,
            n_receive_timeout: i32,
        ) -> Bool;
        fn WinHttpConnect(
            h_session: Hinternet,
            pswz_server_name: *const u16,
            n_server_port: u16,
            dw_reserved: u32,
        ) -> Hinternet;
        fn WinHttpOpenRequest(
            h_connect: Hinternet,
            pwsz_verb: *const u16,
            pwsz_object_name: *const u16,
            pwsz_version: *const u16,
            pwsz_referrer: *const u16,
            ppwsz_accept_types: *const *const u16,
            dw_flags: u32,
        ) -> Hinternet;
        fn WinHttpAddRequestHeaders(
            h_request: Hinternet,
            lpsz_headers: *const u16,
            dw_headers_length: u32,
            dw_modifiers: u32,
        ) -> Bool;
        fn WinHttpSendRequest(
            h_request: Hinternet,
            lpsz_headers: *const u16,
            dw_headers_length: u32,
            lp_optional: *mut c_void,
            dw_optional_length: u32,
            dw_total_length: u32,
            dw_context: usize,
        ) -> Bool;
        fn WinHttpReceiveResponse(h_request: Hinternet, lp_reserved: *mut c_void) -> Bool;
        fn WinHttpQueryHeaders(
            h_request: Hinternet,
            dw_info_level: u32,
            pwsz_name: *const u16,
            lp_buffer: *mut c_void,
            lpdw_buffer_length: *mut u32,
            lpdw_index: *mut u32,
        ) -> Bool;
        fn WinHttpQueryDataAvailable(
            h_request: Hinternet,
            lpdw_number_of_bytes_available: *mut u32,
        ) -> Bool;
        fn WinHttpReadData(
            h_request: Hinternet,
            lp_buffer: *mut c_void,
            dw_number_of_bytes_to_read: u32,
            lpdw_number_of_bytes_read: *mut u32,
        ) -> Bool;
        fn WinHttpCloseHandle(h_internet: Hinternet) -> Bool;
    }

    struct HandleGuard(Hinternet);
    impl HandleGuard {
        fn new(h: Hinternet) -> Option<Self> {
            if h.is_null() { None } else { Some(Self(h)) }
        }
        fn raw(&self) -> Hinternet {
            self.0
        }
    }
    impl Drop for HandleGuard {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    let _ = WinHttpCloseHandle(self.0);
                }
            }
        }
    }

    fn to_wide_nul(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn parse_url(url: &str) -> Result<(bool, String, u16, String), String> {
        let trimmed = url.trim();
        let (https, rest) = if let Some(r) = trimmed.strip_prefix("https://") {
            (true, r)
        } else if let Some(r) = trimmed.strip_prefix("http://") {
            (false, r)
        } else {
            return Err("unsupported URL scheme".to_string());
        };
        let (host_port, path_and_query) = match rest.find('/') {
            Some(idx) => (&rest[..idx], rest[idx..].to_string()),
            None => (rest, "/".to_string()),
        };
        if host_port.is_empty() {
            return Err("missing host".to_string());
        }
        let (host, port) = if let Some((h, p)) = host_port.rsplit_once(':') {
            let parsed_p = p.parse::<u16>().map_err(|_| "invalid port".to_string())?;
            (h.to_string(), parsed_p)
        } else {
            (host_port.to_string(), if https { 443 } else { 80 })
        };
        Ok((https, host, port, path_and_query))
    }

    pub fn get_github_json(url: &str, timeout_ms: u32) -> Result<(u32, String), String> {
        let (https, host, port, path_and_query) = parse_url(url)?;
        let is_loopback = host.eq_ignore_ascii_case("127.0.0.1")
            || host.eq_ignore_ascii_case("localhost")
            || host == "::1";
        let access = if is_loopback {
            WINHTTP_ACCESS_TYPE_NO_PROXY
        } else {
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY
        };
        let ua = format!("iPhone-Link/{}", super::CURRENT_VERSION);
        let ua_w = to_wide_nul(&ua);
        let session = HandleGuard::new(unsafe {
            WinHttpOpen(ua_w.as_ptr(), access, ptr::null(), ptr::null(), 0)
        })
        .or_else(|| {
            HandleGuard::new(unsafe {
                WinHttpOpen(
                    ua_w.as_ptr(),
                    WINHTTP_ACCESS_TYPE_DEFAULT_PROXY,
                    ptr::null(),
                    ptr::null(),
                    0,
                )
            })
        })
        .ok_or_else(|| "WinHttpOpen failed".to_string())?;

        let t_ms = timeout_ms.clamp(1_000, 30_000) as i32;
        unsafe {
            let _ = WinHttpSetTimeouts(session.raw(), t_ms, t_ms, t_ms, t_ms);
        }

        let host_w = to_wide_nul(&host);
        let connect = HandleGuard::new(unsafe {
            WinHttpConnect(session.raw(), host_w.as_ptr(), port, 0)
        })
        .ok_or_else(|| "WinHttpConnect failed".to_string())?;

        let verb_w = to_wide_nul("GET");
        let path_w = to_wide_nul(&path_and_query);
        let flags = if https { WINHTTP_FLAG_SECURE } else { 0 };
        let req = HandleGuard::new(unsafe {
            WinHttpOpenRequest(
                connect.raw(),
                verb_w.as_ptr(),
                path_w.as_ptr(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                flags,
            )
        })
        .ok_or_else(|| "WinHttpOpenRequest failed".to_string())?;

        let hdr_w = to_wide_nul("Accept: application/vnd.github+json\r\nCache-Control: no-cache\r\n");
        let hdr_len = (hdr_w.len().saturating_sub(1)) as u32;
        unsafe {
            let _ = WinHttpAddRequestHeaders(
                req.raw(),
                hdr_w.as_ptr(),
                hdr_len,
                WINHTTP_ADDREQ_FLAG_ADD | WINHTTP_ADDREQ_FLAG_REPLACE,
            );
            if WinHttpSendRequest(req.raw(), ptr::null(), 0, ptr::null_mut(), 0, 0, 0) == 0 {
                return Err("WinHttpSendRequest failed".to_string());
            }
            if WinHttpReceiveResponse(req.raw(), ptr::null_mut()) == 0 {
                return Err("WinHttpReceiveResponse failed".to_string());
            }
        }

        let mut status: u32 = 0;
        let mut status_len: u32 = std::mem::size_of::<u32>() as u32;
        unsafe {
            let _ = WinHttpQueryHeaders(
                req.raw(),
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                ptr::null(),
                (&mut status as *mut u32).cast::<c_void>(),
                &mut status_len,
                ptr::null_mut(),
            );
        }

        let mut body = Vec::with_capacity(8192);
        let mut buf = vec![0u8; 32 * 1024];
        loop {
            let mut avail: u32 = 0;
            if unsafe { WinHttpQueryDataAvailable(req.raw(), &mut avail) } == 0 {
                return Err("WinHttpQueryDataAvailable failed".to_string());
            }
            if avail == 0 {
                break;
            }
            let want = (avail as usize).min(buf.len()) as u32;
            let mut got: u32 = 0;
            if unsafe {
                WinHttpReadData(
                    req.raw(),
                    buf.as_mut_ptr().cast::<c_void>(),
                    want,
                    &mut got,
                )
            } == 0
            {
                return Err("WinHttpReadData failed".to_string());
            }
            if got == 0 {
                break;
            }
            body.extend_from_slice(&buf[..got as usize]);
            if body.len() > 4 * 1024 * 1024 {
                return Err("GitHub response exceeded 4 MB cap".to_string());
            }
        }

        Ok((status, String::from_utf8_lossy(&body).to_string()))
    }
}

/// Parses GitHub Releases JSON (array or single release object) using SemVer, channel filter
/// (`stable` vs `beta`), draft exclusion, and Windows asset matching (`Setup.exe` / `Win64-Native-Rust-*.zip`).
pub fn parse_github_releases_json(raw_json: &str, channel: &str) -> Result<UpdateStateCache, String> {
    let active_ch = if channel.trim().eq_ignore_ascii_case("beta") {
        "beta"
    } else {
        "stable"
    };
    let val: serde_json::Value =
        serde_json::from_str(raw_json).map_err(|e| format!("invalid GitHub JSON: {e}"))?;
    let releases: Vec<&serde_json::Value> = match val {
        serde_json::Value::Array(ref arr) => arr.iter().collect(),
        serde_json::Value::Object(_) => vec![&val],
        _ => return Err("unexpected GitHub JSON root type".to_string()),
    };

    let mut best_ver: Option<(Semver, String, &serde_json::Value)> = None;
    for rel in releases {
        let Some(obj) = rel.as_object() else {
            continue;
        };
        if obj.get("draft").and_then(|v| v.as_bool()).unwrap_or(false) {
            continue;
        }
        let is_pre = obj.get("prerelease").and_then(|v| v.as_bool()).unwrap_or(false);
        let tag_raw = obj
            .get("tag_name")
            .or_else(|| obj.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if tag_raw.is_empty() {
            continue;
        }
        let Some(sv) = Semver::parse(&tag_raw) else {
            continue;
        };
        if active_ch == "stable" && (is_pre || sv.is_beta) {
            continue;
        }
        let replace = match &best_ver {
            None => true,
            Some((cur_sv, cur_tag, _)) => {
                is_newer_version(cur_tag, &tag_raw) || (sv > *cur_sv && cur_tag != &tag_raw)
            }
        };
        if replace {
            best_ver = Some((sv, tag_raw, rel));
        }
    }

    let mut out = UpdateStateCache::default();
    out.channel = active_ch.to_string();
    out.offline_fallback = false;
    out.last_checked_ms = UpdateStateCache::now_ms();

    let Some((_, best_tag, best_rel)) = best_ver else {
        return Ok(out);
    };

    if !is_newer_version(CURRENT_VERSION, &best_tag) {
        return Ok(out);
    }

    let clean_ver = best_tag
        .trim()
        .trim_start_matches(['v', 'V'])
        .to_string();
    let release_url = best_rel
        .get("html_url")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    let mut setup_url = String::new();
    let mut win_zip_url = String::new();
    let mut any_zip_url = String::new();
    let mut first_asset_url = String::new();
    if let Some(assets) = best_rel.get("assets").and_then(|v| v.as_array()) {
        for a in assets {
            let name = a.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let dl = a
                .get("browser_download_url")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if dl.is_empty() {
                continue;
            }
            let nl = name.to_ascii_lowercase();
            if first_asset_url.is_empty() {
                first_asset_url = dl.to_string();
            }
            if setup_url.is_empty() && nl.ends_with(".exe") && nl.contains("setup") {
                setup_url = dl.to_string();
            } else if nl.ends_with(".zip") {
                if any_zip_url.is_empty() {
                    any_zip_url = dl.to_string();
                }
                if win_zip_url.is_empty()
                    && (nl.contains("win") || nl.contains("native-rust") || nl.contains("standalone"))
                {
                    win_zip_url = dl.to_string();
                }
            }
        }
    }

    let installed_in_program_files = std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(|s| s.to_ascii_lowercase().contains("program files")))
        .unwrap_or(false);
    let chosen_dl = if installed_in_program_files && !setup_url.is_empty() {
        setup_url
    } else if !win_zip_url.is_empty() {
        win_zip_url
    } else if !setup_url.is_empty() {
        setup_url
    } else if !any_zip_url.is_empty() {
        any_zip_url
    } else if !first_asset_url.is_empty() {
        first_asset_url
    } else {
        release_url.clone()
    };

    let raw_notes = best_rel
        .get("body")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    let notes = if raw_notes.chars().count() > 400 {
        let mut s: String = raw_notes.chars().take(397).collect();
        s.push_str("...");
        s
    } else {
        raw_notes.to_string()
    };

    out.latest_version = clean_ver.clone();
    out.update_available = true;
    out.status = "update_available".to_string();
    out.download_url = chosen_dl;
    out.release_url = if release_url.is_empty() {
        format!("{PUBLIC_GITHUB_REPO_URL}/releases")
    } else {
        release_url
    };
    out.release_notes = notes;
    out.message = format!("Tersedia pembaruan: iPhone Link v{clean_ver}");
    Ok(out)
}

/// Checks GitHub Releases using native Win32 WinHTTP (`winhttp.dll`), respecting
/// `IPHONE_LINK_TEST_LOOPBACK_ONLY=1` so tests never touch external networks.
pub fn check_github_updates_native(channel: &str, api_url_override: Option<&str>) -> UpdateStateCache {
    let active_ch = if channel.trim().eq_ignore_ascii_case("beta") {
        "beta"
    } else {
        "stable"
    };
    let loopback_only = cfg!(test) || env_truthy_updater("IPHONE_LINK_TEST_LOOPBACK_ONLY");
    let override_clean = api_url_override.map(str::trim).filter(|s| !s.is_empty());

    let target_url = match override_clean {
        Some(u) => {
            if loopback_only && !is_loopback_http_url(u) {
                let mut fb = UpdateStateCache::default();
                fb.set_offline_fallback(active_ch);
                return fb;
            }
            u.to_string()
        }
        None => {
            if loopback_only {
                let mut fb = UpdateStateCache::default();
                fb.set_offline_fallback(active_ch);
                return fb;
            }
            github_releases_api_url()
        }
    };

    #[cfg(windows)]
    {
        match updater_winhttp::get_github_json(&target_url, 6_000) {
            Ok((200, body)) => match parse_github_releases_json(&body, active_ch) {
                Ok(parsed) => parsed,
                Err(_) => {
                    let mut fb = UpdateStateCache::default();
                    fb.set_offline_fallback(active_ch);
                    fb
                }
            },
            _ => {
                let mut fb = UpdateStateCache::default();
                fb.set_offline_fallback(active_ch);
                fb
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = target_url;
        let mut fb = UpdateStateCache::default();
        fb.set_offline_fallback(active_ch);
        fb
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_semver_version_comparison_v4() {
        assert!(!is_newer_version("5.0.0", "2.0.0"));
        assert!(!is_newer_version("5.0.0", "3.0.0"));
        assert!(!is_newer_version("5.0.0", "v4.0.0"));
        assert!(!is_newer_version("5.0.0", "5.0.0"));
        assert!(!is_newer_version("5.0.0", "v5.0.0"));
        assert!(!is_newer_version("5.0.0", "5.0.0-beta.1"));
        assert!(is_newer_version("5.0.0", "5.0.1"));
        assert!(is_newer_version("5.0.0", "v5.1.0-beta.1"));
        assert!(is_newer_version("5.1.0-beta.1", "5.1.0-beta.2"));
        assert!(is_newer_version("5.1.0-beta.2", "5.1.0"));
    }

    #[test]
    fn test_update_state_cache_rejects_downgrade_or_stale_v3() {
        let mut cache = UpdateStateCache::default();
        let stale_payload = r#"{"ok":true,"status":"update_available","latestVersion":"4.0.0","updateAvailable":true,"channel":"stable"}"#;
        cache.apply_worker_payload(stale_payload, "stable");
        assert!(!cache.update_available);
        assert_eq!(cache.current_version, "5.0.0");
        assert_eq!(cache.latest_version, "5.0.0");
        assert_eq!(cache.status, "up_to_date");

        let newer_payload = r#"{"ok":true,"status":"update_available","latestVersion":"5.0.1","updateAvailable":true,"downloadUrl":"https://example.com/v5.0.1.zip","channel":"stable"}"#;
        cache.apply_worker_payload(newer_payload, "stable");
        assert!(cache.update_available);
        assert_eq!(cache.latest_version, "5.0.1");
        assert_eq!(cache.status, "update_available");
    }

    #[test]
    fn test_semver_parsing_and_comparison() {
        let v5 = Semver::parse("v5.0.0").unwrap();
        let v4 = Semver::parse("v4.0.0").unwrap();
        let v5_beta = Semver::parse("v5.0.0-beta.1").unwrap();

        assert!(v5.is_newer_than(&v4));
        assert!(v5.is_newer_than(&v5_beta));
        assert!(!v4.is_newer_than(&v5));
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

    #[test]
    fn test_tahap285_updater_github_loopback_v5_and_v4_contract_parity() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::{Arc, Mutex};

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().unwrap().port();
        let captured_headers = Arc::new(Mutex::new(Vec::<String>::new()));
        let hdr_srv = captured_headers.clone();

        let rel_base = format!("{PUBLIC_GITHUB_REPO_URL}/releases");
        let obj_base = "https://objects.githubusercontent.com/github-production-release-asset";
        let releases_json = format!(
            concat!(
                r#"[{{"tag_name":"v5.3.0","draft":true,"prerelease":false,"html_url":"{rel}/tag/v5.3.0","body":"Draft","assets":[{{"name":"iPhone-Link-Win64-Native-Rust-v5.3.0.zip","browser_download_url":"{rel}/download/v5.3.0/iPhone-Link-Win64-Native-Rust-v5.3.0.zip"}}]}},"#,
                r#"{{"tag_name":"v5.2.0-beta.1","draft":false,"prerelease":true,"html_url":"{rel}/tag/v5.2.0-beta.1","body":"Beta channel release","assets":[{{"name":"iPhone-Link-v5.2.0-beta.1-Setup.exe","browser_download_url":"{rel}/download/v5.2.0-beta.1/iPhone-Link-v5.2.0-beta.1-Setup.exe"}},{{"name":"iPhone-Link-Win64-Native-Rust-v5.2.0-beta.1.zip","browser_download_url":"{rel}/download/v5.2.0-beta.1/iPhone-Link-Win64-Native-Rust-v5.2.0-beta.1.zip"}}]}},"#,
                r#"{{"tag_name":"v5.1.0","draft":false,"prerelease":false,"html_url":"{rel}/tag/v5.1.0","body":"Stable v5.1.0 release","assets":[{{"name":"iPhone-Link-v5.1.0-Setup.exe","browser_download_url":"{rel}/download/v5.1.0/iPhone-Link-v5.1.0-Setup.exe"}},{{"name":"iPhone-Link-Win64-Native-Rust-v5.1.0.zip","browser_download_url":"{rel}/download/v5.1.0/iPhone-Link-Win64-Native-Rust-v5.1.0.zip"}}]}},"#,
                r#"{{"tag_name":"v5.0.0","draft":false,"prerelease":false,"html_url":"{rel}/tag/v5.0.0","body":"Baseline v5.0.0","assets":[{{"name":"iPhone-Link-Win64-Native-Rust-v5.0.0.zip","browser_download_url":"{rel}/download/v5.0.0/iPhone-Link-Win64-Native-Rust-v5.0.0.zip"}}]}}]"#
            ),
            rel = rel_base
        );

        let srv_body = releases_json.clone();
        let srv = std::thread::spawn(move || {
            for _ in 0..2 {
                let (mut sock, _) = listener.accept().expect("accept");
                let mut req_buf = [0u8; 4096];
                let n = sock.read(&mut req_buf).unwrap_or(0);
                let req_str = String::from_utf8_lossy(&req_buf[..n]).to_string();
                hdr_srv.lock().unwrap().push(req_str);
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    srv_body.len(),
                    srv_body
                );
                let _ = sock.write_all(resp.as_bytes());
                let _ = sock.flush();
            }
        });

        let api_url = format!("http://127.0.0.1:{port}/repos/releases");
        let stable_res = check_github_updates_native("stable", Some(&api_url));
        assert!(stable_res.update_available, "stable channel MUST detect v5.1.0");
        assert_eq!(stable_res.latest_version, "5.1.0");
        assert_eq!(stable_res.status, "update_available");
        assert!(!stable_res.offline_fallback);
        assert!(stable_res.download_url.contains("Win64-Native-Rust-v5.1.0.zip") || stable_res.download_url.contains("v5.1.0-Setup.exe"));
        assert!(is_allowed_github_release_url(&stable_res.release_url));

        let beta_res = check_github_updates_native("beta", Some(&api_url));
        assert!(beta_res.update_available, "beta channel MUST detect v5.2.0-beta.1");
        assert_eq!(beta_res.latest_version, "5.2.0-beta.1");
        assert!(!beta_res.offline_fallback);

        let _ = srv.join();
        let reqs = captured_headers.lock().unwrap();
        assert_eq!(reqs.len(), 2);
        for req in reqs.iter() {
            let low = req.to_ascii_lowercase();
            assert!(low.contains("user-agent: iphone-link/5.0.0"), "missing User-Agent header: {req}");
            assert!(low.contains("accept: application/vnd.github+json"), "missing GitHub Accept header: {req}");
        }

        // Loopback guard MUST skip external github.com calls cleanly when override is None
        let offline_guard = check_github_updates_native("stable", None);
        assert!(!offline_guard.update_available);
        assert!(offline_guard.offline_fallback);

        // Allowlist URL security & v4.0.0 ZIP asset naming parity
        assert!(is_allowed_github_release_url(&format!("{rel_base}/tag/v5.0.0")));
        assert!(is_allowed_github_release_url(&format!("{obj_base}/12345")));
        assert!(!is_allowed_github_release_url("http://github.com/kelvindaniyar/iphone-link/releases"));
        assert!(!is_allowed_github_release_url("https://evil.example/releases/v5.1.0"));
        assert!(!is_allowed_github_release_url("file:///C:/Windows/System32/calc.exe"));
        assert!(github_releases_api_url().starts_with("https://api.github.com/repos/"));
        assert!(github_releases_api_url().ends_with("/releases"));
    }
}
