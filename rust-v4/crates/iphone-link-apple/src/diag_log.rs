//! Zero-PII In-Memory Diagnostic Ring Buffer & Sanitizer (Tahap 293).
//!
//! Stores up to 120 recent timestamped diagnostic milestones in memory and provides
//! strict sanitization of UDIDs, private LAN IPs, MAC addresses, Windows usernames,
//! and PIN/DPAPI secrets before exporting or copying logs.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

const MAX_RING_ENTRIES: usize = 120;

static BOOT_INSTANT: OnceLock<Instant> = OnceLock::new();
static RING_BUFFER: OnceLock<Mutex<VecDeque<String>>> = OnceLock::new();

fn boot_instant() -> Instant {
    *BOOT_INSTANT.get_or_init(Instant::now)
}

fn ring_buffer() -> &'static Mutex<VecDeque<String>> {
    RING_BUFFER.get_or_init(|| Mutex::new(VecDeque::with_capacity(MAX_RING_ENTRIES)))
}

fn short_fnv1a_tag(s: &str) -> String {
    let mut hash: u32 = 0x811c_9dc5;
    for b in s.as_bytes() {
        hash ^= u32::from(*b);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    format!("UDID#{:04x}", hash & 0xFFFF)
}

fn is_hex_or_udid_char(b: u8) -> bool {
    b.is_ascii_hexdigit() || b == b'-'
}

fn redact_udids(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < bytes.len() {
        // Match modern 24-hex + '-' UDID (`00008xxx-00xxxxxxxxxxxxxxxx`, len 25) or 40-hex legacy UDID
        if bytes[i].is_ascii_hexdigit() {
            let mut j = i;
            while j < bytes.len() && is_hex_or_udid_char(bytes[j]) {
                j += 1;
            }
            let token = &input[i..j];
            let hex_only_len = token.bytes().filter(|b| b.is_ascii_hexdigit()).count();
            let looks_modern_udid = token.len() == 25
                && token.as_bytes().get(8) == Some(&b'-')
                && token[..8].bytes().all(|b| b.is_ascii_hexdigit())
                && token[9..].bytes().all(|b| b.is_ascii_hexdigit());
            let looks_legacy_udid = hex_only_len == 40 && !token.contains('-');
            let looks_dash_udid = token.len() >= 20
                && token.contains('-')
                && token.to_ascii_lowercase().starts_with("00008")
                && hex_only_len >= 18;
            if looks_modern_udid || looks_legacy_udid || looks_dash_udid {
                out.push_str(&short_fnv1a_tag(token));
                i = j;
                continue;
            }
        }
        // Safe char boundary copy
        if let Some(ch) = input[i..].chars().next() {
            out.push(ch);
            i += ch.len_utf8();
        } else {
            break;
        }
    }
    out
}

fn is_rfc1918_v4(octets: [u8; 4]) -> bool {
    octets[0] == 10
        || (octets[0] == 172 && (16..=31).contains(&octets[1]))
        || (octets[0] == 192 && octets[1] == 168)
}

fn redact_private_ipv4_and_mac(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < bytes.len() {
        // Check MAC address: XX:XX:XX:XX:XX:XX or XX-XX-XX-XX-XX-XX (17 chars)
        if i + 17 <= bytes.len() {
            let slice = &bytes[i..i + 17];
            let sep = slice[2];
            if (sep == b':' || sep == b'-')
                && [2, 5, 8, 11, 14].iter().all(|&p| slice[p] == sep)
                && [0, 1, 3, 4, 6, 7, 9, 10, 12, 13, 15, 16]
                    .iter()
                    .all(|&p| slice[p].is_ascii_hexdigit())
            {
                let prev_ok = i == 0 || !bytes[i - 1].is_ascii_hexdigit();
                let next_ok = i + 17 == bytes.len() || !bytes[i + 17].is_ascii_hexdigit();
                if prev_ok && next_ok {
                    out.push_str("<MAC>");
                    i += 17;
                    continue;
                }
            }
        }

        // Check IPv4 candidate
        if bytes[i].is_ascii_digit() {
            let prev_ok = i == 0 || !(bytes[i - 1].is_ascii_digit() || bytes[i - 1] == b'.');
            if prev_ok {
                let mut j = i;
                while j < bytes.len() && (bytes[j].is_ascii_digit() || bytes[j] == b'.') {
                    j += 1;
                }
                let cand = &input[i..j];
                if let Ok(ip) = cand.parse::<std::net::Ipv4Addr>() {
                    if is_rfc1918_v4(ip.octets()) {
                        out.push_str("<LAN-IP>");
                        i = j;
                        continue;
                    }
                }
            }
        }

        // Check IPv6 link-local `fe80:...`
        if i + 5 <= bytes.len() && input[i..i + 5].eq_ignore_ascii_case("fe80:") {
            let mut j = i + 5;
            while j < bytes.len()
                && (bytes[j].is_ascii_hexdigit() || bytes[j] == b':' || bytes[j] == b'%')
            {
                j += 1;
            }
            out.push_str("<LINK-LOCAL-IP>");
            i = j;
            continue;
        }

        if let Some(ch) = input[i..].chars().next() {
            out.push(ch);
            i += ch.len_utf8();
        } else {
            break;
        }
    }
    out
}

fn redact_user_paths(input: &str) -> String {
    let mut current = input.to_string();
    for prefix in [r"C:\Users\", "C:/Users/", r"c:\users\", "c:/users/"] {
        while let Some(pos) = current.find(prefix) {
            let after_pos = pos + prefix.len();
            let rest = &current[after_pos..];
            let end_rel = rest
                .find(['\\', '/', '"', '\'', ' ', '\t', '\r', '\n'])
                .unwrap_or(rest.len());
            let mut next = String::with_capacity(current.len());
            next.push_str(&current[..pos]);
            next.push_str("<USERPROFILE>");
            next.push_str(&rest[end_rel..]);
            current = next;
        }
    }
    current
}

fn redact_secrets(input: &str) -> String {
    let mut s = input.to_string();
    // Redact dpapi:<base64>
    while let Some(pos) = s.to_ascii_lowercase().find("dpapi:") {
        let rest = &s[pos + 6..];
        let end_rel = rest
            .find(['"', '\'', ' ', '\t', '\r', '\n', ',', '}'])
            .unwrap_or(rest.len());
        let mut next = String::with_capacity(s.len());
        next.push_str(&s[..pos]);
        next.push_str("<PIN-REDACTED>");
        next.push_str(&rest[end_rel..]);
        s = next;
    }
    // Redact JSON/log unlockPin variants (`"unlockPin":"..."`, `unlockPin":"..."`, `unlockPin=...`)
    for key in [
        r#""unlockPin":""#,
        r#""unlockPin": ""#,
        r#"unlockPin":""#,
        r#"unlockPin": ""#,
        r#""unlock_pin":""#,
        r#"unlock_pin":""#,
    ] {
        let mut search_from = 0;
        while let Some(rel_pos) = s[search_from..].find(key) {
            let val_start = search_from + rel_pos + key.len();
            if let Some(end_q) = s[val_start..].find('"') {
                let val_slice = &s[val_start..val_start + end_q];
                if val_slice == "<PIN-REDACTED>" {
                    search_from = val_start + end_q + 1;
                    continue;
                }
                let mut next = String::with_capacity(s.len());
                next.push_str(&s[..val_start]);
                next.push_str("<PIN-REDACTED>");
                next.push_str(&s[val_start + end_q..]);
                s = next;
                search_from = val_start + "<PIN-REDACTED>".len();
            } else {
                break;
            }
        }
    }
    for eq_key in ["unlockPin=", "unlock_pin="] {
        let mut search_from = 0;
        while let Some(rel_pos) = s[search_from..].find(eq_key) {
            let val_start = search_from + rel_pos + eq_key.len();
            let rest = &s[val_start..];
            let end_rel = rest
                .find(['"', '\'', ' ', '\t', '\r', '\n', ',', '}'])
                .unwrap_or(rest.len());
            if &rest[..end_rel] == "<PIN-REDACTED>" {
                search_from = val_start + end_rel;
                continue;
            }
            let mut next = String::with_capacity(s.len());
            next.push_str(&s[..val_start]);
            next.push_str("<PIN-REDACTED>");
            next.push_str(&rest[end_rel..]);
            s = next;
            search_from = val_start + "<PIN-REDACTED>".len();
        }
    }
    s
}

/// Sanitizes an arbitrary diagnostic string so zero private UDIDs, private LAN IPs,
/// MAC addresses, Windows usernames, or PIN/DPAPI secrets remain.
pub fn sanitize_pii_for_diagnostic_log(input: &str) -> String {
    let s1 = redact_secrets(input);
    let s2 = redact_user_paths(&s1);
    let s3 = redact_udids(&s2);
    let s4 = redact_private_ipv4_and_mac(&s3);
    s4.replace('\u{2014}', "-").replace('\u{2013}', "-")
}

/// Records a sanitized milestone entry into the 120-entry diagnostic ring buffer.
pub fn record(category: &str, msg: &str) {
    let elapsed_ms = boot_instant().elapsed().as_millis();
    let cat = category.trim();
    let clean = sanitize_pii_for_diagnostic_log(msg.trim());
    let line = format!("[+{elapsed_ms}ms] [{cat}] {clean}");
    if let Ok(mut g) = ring_buffer().lock() {
        if g.len() >= MAX_RING_ENTRIES {
            g.pop_front();
        }
        g.push_back(line);
    }
}

/// Returns a chronological snapshot of the sanitized diagnostic log entries.
pub fn snapshot() -> Vec<String> {
    ring_buffer()
        .lock()
        .map(|g| g.iter().cloned().collect())
        .unwrap_or_default()
}

/// Clears the ring buffer for isolated unit tests.
pub fn clear_for_test() {
    if let Ok(mut g) = ring_buffer().lock() {
        g.clear();
    }
}
