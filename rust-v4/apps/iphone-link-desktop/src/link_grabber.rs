//! Social Link Grabber and URL Normalizer (Alt+B).
//!
//! Handles OCR defect repairs in URLs and normalizes social media links:
//! - Instagram (p, reel, tv)
//! - TikTok (vt, vm, web)
//! - YouTube (watch, shorts, youtu.be)
//! - Threads (post, share)
//! - X / Twitter (status)

/// Splits concatenated social URLs (e.g. `...DVO6D0PFoOQhttps://...`) before a second URL marker.
fn split_concatenated_url_tokens(token: &str) -> Vec<&str> {
    let lower = token.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let markers = [
        "https://",
        "http://",
        "www.instagram.",
        "www.threads.",
        "www.tiktok.",
        "instagram.com/",
        "threads.com/",
        "threads.net/",
    ];

    let mut cut_points = Vec::new();
    let mut last_cut = 0usize;
    let mut pos = 8usize;
    while pos < bytes.len() {
        let slice = &lower[pos..];
        let mut matched_len = 0usize;
        for m in markers {
            if slice.starts_with(m) {
                let pfx = &lower[last_cut..pos];
                let is_own_host_prefix = pfx == "https://"
                    || pfx == "http://"
                    || pfx == "https://www."
                    || pfx == "http://www."
                    || pfx == "/www."
                    || pfx == "//www.";
                if !is_own_host_prefix {
                    cut_points.push(pos);
                    last_cut = pos;
                    matched_len = m.len();
                    break;
                }
            }
        }
        pos += if matched_len > 0 { matched_len } else { 1 };
    }

    if cut_points.is_empty() {
        return vec![token];
    }

    let mut out = Vec::new();
    let mut start = 0usize;
    for cp in cut_points {
        if cp > start {
            out.push(&token[start..cp]);
        }
        start = cp;
    }
    if start < token.len() {
        out.push(&token[start..]);
    }
    out
}

/// Normalizes clipped Spotlight schemes and leading slash/pipe noise (e.g. `/www.threads.com/share/...`, `tps://www.`).
fn normalize_clipped_scheme(raw: &str) -> String {
    let mut s = raw.trim().trim_start_matches(&['|', '•', '(', '['][..]).to_string();
    let s_low = s.to_ascii_lowercase();
    for bad_pfx in ["ttps://", "ttos://", "tps://", "ps://", "://"] {
        if s_low.starts_with(bad_pfx) {
            s = format!("https://{}", &s[bad_pfx.len()..]);
            break;
        }
    }
    let s_low2 = s.to_ascii_lowercase();
    if s_low2.starts_with("//www.") {
        s = format!("https://{}", &s[2..]);
    } else if s_low2.starts_with("/www.") {
        s = format!("https://{}", &s[1..]);
    } else if s_low2.starts_with("//threads.")
        || s_low2.starts_with("//instagram.")
        || s_low2.starts_with("//tiktok.")
    {
        s = format!("https://www.{}", &s[2..]);
    } else if s_low2.starts_with("/threads.")
        || s_low2.starts_with("/instagram.")
        || s_low2.starts_with("/tiktok.")
    {
        s = format!("https://www.{}", &s[1..]);
    }
    s
}

/// Normalizes OCR text and extracts valid, sanitized social links.
pub fn extract_and_normalize_links(raw_text: &str) -> Vec<String> {
    if raw_text.trim().is_empty() {
        return Vec::new();
    }

    // Step 1: Strip stray iOS selection handle pipes and dots (|• ... •|) and Spotlight clear icons
    let mut cleaned = raw_text.to_string();
    cleaned = cleaned.replace("|•", "");
    cleaned = cleaned.replace("•|", "");
    cleaned = cleaned.replace("|", "");
    cleaned = cleaned.replace("\u{2014} Ask Siri", "");
    cleaned = cleaned.replace("- Ask Siri", "");
    cleaned = cleaned.replace("Ask Siri", "");
    cleaned = cleaned.replace("⊗", " ");

    // Step 2: Fix optical defects in protocols and domains
    cleaned = cleaned.replace("http5://", "https://");
    cleaned = cleaned.replace("http5:", "https://");
    cleaned = cleaned.replace("http://", "https://");
    cleaned = cleaned.replace("youtubc.com", "youtube.com");
    cleaned = cleaned.replace("y0utube.com", "youtube.com");
    cleaned = cleaned.replace("instagrarn.com", "instagram.com");
    cleaned = cleaned.replace("tlktok.com", "tiktok.com");
    cleaned = cleaned.replace("tikt0k.com", "tiktok.com");

    // Fix OCR spaces inside domain or protocol
    cleaned = cleaned.replace("https:// ", "https://");
    cleaned = cleaned.replace("http:// ", "http://");
    cleaned = cleaned.replace("www. ", "www.");
    cleaned = cleaned.replace(". com", ".com");
    cleaned = cleaned.replace(". net", ".net");
    cleaned = cleaned.replace(". be", ".be");

    let mut results = Vec::new();
    let words: Vec<&str> = cleaned.split_whitespace().collect();

    for w in words {
        for piece in split_concatenated_url_tokens(w.trim()) {
            let mut t = piece.trim();
            // Strip trailing punctuation (preserving trailing '_' and '-' on 11-char shortcodes)
            t = t.trim_end_matches(&['.', ',', ';', '!', ']', ')', '}', '>', ' ', '⊗', '✕', '✖', '©', '®']);

            let mut candidate = normalize_clipped_scheme(t);
            let low_c = candidate.to_ascii_lowercase();
            if !low_c.starts_with("http://") && !low_c.starts_with("https://") {
                if low_c.starts_with("www.") {
                    candidate = format!("https://{}", candidate);
                } else if low_c.starts_with("instagram.com")
                    || low_c.starts_with("threads.net")
                    || low_c.starts_with("threads.com")
                {
                    candidate = format!("https://www.{}", candidate);
                } else if low_c.starts_with("tiktok.com")
                    || low_c.starts_with("vt.tiktok.com")
                    || low_c.starts_with("vm.tiktok.com")
                    || low_c.starts_with("youtube.com")
                    || low_c.starts_with("youtu.be")
                    || low_c.starts_with("x.com")
                    || low_c.starts_with("twitter.com")
                {
                    candidate = format!("https://{}", candidate);
                }
            }

            // Strip Threads tracking query ?xmt=... on post/share links (keep clean canonical URL)
            let lower_cand = candidate.to_ascii_lowercase();
            if (lower_cand.contains("threads.com/") || lower_cand.contains("threads.net/"))
                && (lower_cand.contains("/post/") || lower_cand.contains("/share/"))
            {
                if let Some(q_idx) = candidate.find('?') {
                    let query = &lower_cand[q_idx..];
                    if query.contains("xmt=") || query.contains("slof=") || query.contains("asc=") {
                        candidate.truncate(q_idx);
                    }
                }
            }

            if let Some(validated) = clean_and_validate_extracted_url(&candidate, None) {
                if !results.contains(&validated) {
                    results.push(validated);
                }
            }
        }
    }

    results
}

/// Validates and canonicalizes an extracted social media URL (Tahap 194..196).
/// - Strictly rejects Instagram shortcodes containing '.' or having length < 11 or > 15.
/// - Preserves 11th-char `_` and `-` on Instagram and Threads `/share/` shortcodes.
/// - Splits concatenated duplicate URLs (`...DVO6D0PFoOQhttps://...`).
/// - Normalizes '/reels/' to '/reel/'.
/// - Strips extra query parameters (&ig_mid=..., &utm_source=...) and invalid/short ?igsh= tokens.
pub fn clean_and_validate_extracted_url(raw_url: &str, stem_ratio: Option<f32>) -> Option<String> {
    let trimmed = raw_url
        .trim()
        .trim_end_matches(&['.', ',', ';', '!', ']', ')', '}', '>', ' ', '⊗', '✕', '✖', '©', '®']);
    if trimmed.is_empty() || trimmed.contains(' ') {
        return None;
    }

    let pieces = split_concatenated_url_tokens(trimmed);
    if pieces.len() > 1 {
        for p in pieces.iter().rev() {
            if let Some(ok_url) = clean_and_validate_extracted_url(p, stem_ratio) {
                return Some(ok_url);
            }
        }
        return None;
    }

    let mut candidate = normalize_clipped_scheme(trimmed);
    let low_start = candidate.to_ascii_lowercase();
    if low_start.starts_with("http://") {
        candidate = format!("https://{}", &candidate[7..]);
    } else if !low_start.starts_with("https://") {
        if low_start.starts_with("threads.com/") || low_start.starts_with("threads.net/") || low_start.starts_with("instagram.com/") {
            candidate = format!("https://www.{}", candidate);
        } else {
            candidate = format!("https://{}", candidate);
        }
    } else if low_start.starts_with("https://threads.com/")
        || low_start.starts_with("https://threads.net/")
        || low_start.starts_with("https://instagram.com/")
    {
        candidate = format!("https://www.{}", &candidate[8..]);
    }

    // Normalize /reels/ -> /reel/
    let lower_init = candidate.to_ascii_lowercase();
    if let Some(pos) = lower_init.find("instagram.com/reels/") {
        let split_at = pos + "instagram.com".len();
        candidate.replace_range(split_at..split_at + "/reels/".len(), "/reel/");
    }

    let lower = candidate.to_ascii_lowercase();
    for (marker, kind) in [
        ("instagram.com/p/", "p"),
        ("instagram.com/reel/", "reel"),
        ("instagram.com/tv/", "tv"),
    ] {
        if let Some(idx) = lower.find(marker) {
            let after_marker = &candidate[idx + marker.len()..];
            let shortcode = after_marker
                .split(|c| c == '/' || c == '?' || c == '#')
                .next()
                .unwrap_or("");
            if shortcode.len() < 11
                || shortcode.len() > 15
                || !shortcode
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                return None;
            }

            let fixed_code = verify_ambiguous_shortcode(shortcode, stem_ratio);

            if let Some(igsh_pos) = lower.find("igsh=") {
                let after_igsh = &candidate[igsh_pos + "igsh=".len()..];
                let raw_token = after_igsh
                    .split(&['&', '#'][..])
                    .next()
                    .unwrap_or("")
                    .trim_end_matches(|c: char| {
                        !c.is_ascii_alphanumeric() && c != '_' && c != '-' && c != '='
                    });
                let core_token = raw_token.trim_end_matches('=');
                if core_token.len() >= 12
                    && !raw_token.is_empty()
                    && raw_token
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '=')
                {
                    let norm_token = match core_token.len() % 4 {
                        2 => format!("{}==", core_token),
                        3 => format!("{}=", core_token),
                        _ => raw_token.to_string(),
                    };
                    return Some(format!(
                        "https://www.instagram.com/{}/{}/?igsh={}",
                        kind, fixed_code, norm_token
                    ));
                }
            }

            return Some(format!(
                "https://www.instagram.com/{}/{}/",
                kind, fixed_code
            ));
        }
    }

    for marker in ["threads.com/share/", "threads.net/share/"] {
        if let Some(idx) = lower.find(marker) {
            let after = &candidate[idx + marker.len()..];
            let code = after
                .split(|c| c == '/' || c == '?' || c == '#')
                .next()
                .unwrap_or("")
                .trim_end_matches(&['⊗', '✕', '✖', '©', '®'][..]);
            if (10..=15).contains(&code.len())
                && code
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                let host = if marker.starts_with("threads.net") {
                    "www.threads.net"
                } else {
                    "www.threads.com"
                };
                return Some(format!("https://{}/share/{}", host, code));
            }
            return None;
        }
    }

    if is_valid_social_url(&candidate) {
        Some(candidate)
    } else {
        None
    }
}

fn verify_ambiguous_shortcode(shortcode: &str, _stem_ratio: Option<f32>) -> String {
    shortcode.to_string()
}

/// Checks whether a URL is a recognized social link platform.
pub fn is_valid_social_url(url: &str) -> bool {
    let lower = url.to_lowercase();
    let is_known = lower.contains("instagram.com/p/")
        || lower.contains("instagram.com/reel/")
        || lower.contains("instagram.com/tv/")
        || lower.contains("tiktok.com/")
        || lower.contains("vt.tiktok.com/")
        || lower.contains("vm.tiktok.com/")
        || lower.contains("youtube.com/watch")
        || lower.contains("youtube.com/shorts")
        || lower.contains("youtu.be/")
        || lower.contains("threads.net/@")
        || lower.contains("threads.net/t/")
        || lower.contains("threads.net/share/")
        || lower.contains("threads.com/@")
        || lower.contains("threads.com/t/")
        || lower.contains("threads.com/share/")
        || lower.contains("x.com/")
        || lower.contains("twitter.com/");

    if !is_known || url.contains(' ') {
        return false;
    }

    for marker in ["instagram.com/p/", "instagram.com/reel/", "instagram.com/tv/"] {
        if let Some(idx) = lower.find(marker) {
            let seg = url[idx + marker.len()..]
                .split(|c| c == '/' || c == '?' || c == '#')
                .next()
                .unwrap_or("");
            if seg.len() < 11
                || seg.len() > 15
                || !seg
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                return false;
            }
        }
    }
    true
}

/// Strict HTTP(S) URL guard (Tahap 195):
/// Rejects raw Base64 tokens (e.g. `MWFlcHlhNmh2bW42ZA==`), bare paths, or strings
/// lacking a valid scheme, domain host with >=2-char ASCII TLD, and path segment.
pub fn is_strict_https_social_url(url: &str) -> bool {
    let trimmed = url.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_whitespace) {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    let rest = if let Some(r) = lower.strip_prefix("https://") {
        r
    } else if let Some(r) = lower.strip_prefix("http://") {
        r
    } else {
        return false;
    };
    let Some(slash_idx) = rest.find('/') else {
        return false;
    };
    let host = &rest[..slash_idx];
    let path = &rest[slash_idx..];
    if host.is_empty() || !host.contains('.') || host.contains('=') || path.len() <= 1 {
        return false;
    }
    let labels: Vec<&str> = host.split('.').collect();
    if labels.iter().any(|lbl| lbl.is_empty()) {
        return false;
    }
    let tld = labels.last().copied().unwrap_or("");
    if tld.len() < 2 || !tld.chars().all(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    is_valid_social_url(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_instagram_link_normalization() {
        let raw = "|•https://www.instagram.com/p/DVijpJIiPqN/?igsh=Mm53anAveGk3azdoWGo= •| - Ask Siri";
        let links = extract_and_normalize_links(raw);
        assert_eq!(links.len(), 1);
        assert_eq!(
            links[0],
            "https://www.instagram.com/p/DVijpJIiPqN/?igsh=Mm53anAveGk3azdoWGo="
        );
    }

    #[test]
    fn test_tiktok_and_ocr_typos() {
        let raw = "Tonton video ini di http5://vt.tiktok.com/ZSjX9k2Lm/ sekarang!";
        let links = extract_and_normalize_links(raw);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0], "https://vt.tiktok.com/ZSjX9k2Lm/");
    }

    #[test]
    fn test_youtube_and_threads() {
        let raw = "Check out https://youtubc.com/watch?v=dQw4w9WgXcQ and https://www.threads.net/@user/post/DA123456789";
        let links = extract_and_normalize_links(raw);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0], "https://youtube.com/watch?v=dQw4w9WgXcQ");
        assert_eq!(links[1], "https://www.threads.net/@user/post/DA123456789");
    }

    #[test]
    fn test_threads_com_xmt_strip() {
        let raw = "https://www.threads.com/@lambe.turah/post/DVr46X9E_mX?xmt=AQF01m0p2k9zABC";
        let links = extract_and_normalize_links(raw);
        assert_eq!(links.len(), 1);
        assert_eq!(
            links[0],
            "https://www.threads.com/@lambe.turah/post/DVr46X9E_mX"
        );
    }

    #[test]
    fn test_tahap194_reject_dot_shortcode_and_strip_ig_mid() {
        assert_eq!(
            clean_and_validate_extracted_url(
                "https://www.instagram.com/reel/WH5tM.ghs/?igsh=MTdhZXZrMnh2a3d3MA==",
                None
            ),
            None
        );
        assert_eq!(
            clean_and_validate_extracted_url("https://www.instagram.com/reel/DFX5WHv/", None),
            None
        );
        assert_eq!(
            clean_and_validate_extracted_url(
                "https://www.instagram.com/p/DR3fe54jB7d/?igsh=MTdhZXZrMnh2a3d3MA==&ig_mid=ABC&utm_source=igweb",
                None
            ),
            Some("https://www.instagram.com/p/DR3fe54jB7d/?igsh=MTdhZXZrMnh2a3d3MA==".to_string())
        );
        assert_eq!(
            clean_and_validate_extracted_url(
                "https://www.instagram.com/p/DR3fe54jB7d/?igsh=bad&ig_mid=ABC",
                None
            ),
            Some("https://www.instagram.com/p/DR3fe54jB7d/".to_string())
        );
        assert_eq!(
            clean_and_validate_extracted_url(
                "https://www.instagram.com/reel/DFX5WHvybP1/?igsh=MTdhZXZrMnh2a3d3MA==",
                None
            ),
            Some("https://www.instagram.com/reel/DFX5WHvybP1/?igsh=MTdhZXZrMnh2a3d3MA==".to_string())
        );
    }

    #[test]
    fn test_tahap195_strict_https_guard_and_dual_iphone_links() {
        assert!(is_strict_https_social_url(
            "https://www.instagram.com/reel/DS8B2CD2lJo/?igsh=MWFlcHlhNmh2bW42ZA=="
        ));
        assert!(is_strict_https_social_url(
            "https://www.instagram.com/reel/DSMGL0rkrwy/?igsh=NnN1N3F0NXk5YWZv"
        ));
        assert!(!is_strict_https_social_url("MWFlcHlhNmh2bW42ZA=="));
        assert!(!is_strict_https_social_url("https://MWFlcHlhNmh2bW42ZA=="));
        assert!(!is_strict_https_social_url("instagram.com/reel/DS8B2CD2lJo/"));
        assert!(!is_strict_https_social_url("https://www.instagram.com/"));
        assert_eq!(
            clean_and_validate_extracted_url("MWFlcHlhNmh2bW42ZA==", None),
            None
        );
        assert_eq!(
            extract_and_normalize_links(
                "https://www.instagram.com/reel/DS8B2CD2lJo/?igsh=MWFlcHlhNmh2bW42ZA=="
            ),
            vec!["https://www.instagram.com/reel/DS8B2CD2lJo/?igsh=MWFlcHlhNmh2bW42ZA=="]
        );
        assert_eq!(
            extract_and_normalize_links(
                "https://www.instagram.com/reel/DSMGL0rkrwy/?igsh=NnN1N3F0NXk5YWZv"
            ),
            vec!["https://www.instagram.com/reel/DSMGL0rkrwy/?igsh=NnN1N3F0NXk5YWZv"]
        );
    }

    #[test]
    fn test_tahap196_trailing_underscore_dash_threads_share_and_duplicate_split() {
        // 1. Protect 11th-char '_' on Instagram shortcodes (iPhone 17 Pro real cases)
        assert_eq!(
            clean_and_validate_extracted_url("https://www.instagram.com/p/DVhBOk2E2k_/", None),
            Some("https://www.instagram.com/p/DVhBOk2E2k_/".to_string())
        );
        assert_eq!(
            clean_and_validate_extracted_url("https://www.instagram.com/p/DR9TRVj-bN_", None),
            Some("https://www.instagram.com/p/DR9TRVj-bN_/".to_string())
        );

        // 2. Anti-duplicate concatenated URL splitter (iPhone 11 Pro Max real case)
        assert_eq!(
            clean_and_validate_extracted_url(
                "https://www.instagram.com/p/DVO6D0PFoOQhttps://www.instagram.com/p/DVO6D0PFoOQ/",
                None
            ),
            Some("https://www.instagram.com/p/DVO6D0PFoOQ/".to_string())
        );
        assert_eq!(
            extract_and_normalize_links(
                "https://www.instagram.com/p/DVO6D0PFoOQhttps://www.instagram.com/p/DVO6D0PFoOQ/"
            ),
            vec!["https://www.instagram.com/p/DVO6D0PFoOQ/"]
        );

        // 3. Threads /share/<11-char> cases (preserving trailing '-' and clipped Zona 3 prefixes)
        for (raw, expected) in [
            (
                "/www.threads.com/share/bNPs0Yhxlyj ⊗",
                "https://www.threads.com/share/bNPs0Yhxlyj",
            ),
            (
                "/www.threads.com/share/84cTwJst74- ⊗",
                "https://www.threads.com/share/84cTwJst74-",
            ),
            (
                "tps://www.threads.com/share/9BMc-hwVuoA",
                "https://www.threads.com/share/9BMc-hwVuoA",
            ),
            (
                "www.threads.com/share/9BnCw9TTAGj",
                "https://www.threads.com/share/9BnCw9TTAGj",
            ),
        ] {
            let links = extract_and_normalize_links(raw);
            assert_eq!(links, vec![expected.to_string()]);
            assert!(is_strict_https_social_url(expected));
        }
    }
}
