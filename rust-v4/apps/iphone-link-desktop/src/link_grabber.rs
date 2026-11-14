//! Social Link Grabber and URL Normalizer (Alt+B).
//!
//! Handles OCR defect repairs in URLs and normalizes social media links:
//! - Instagram (p, reel, tv)
//! - TikTok (vt, vm, web)
//! - YouTube (watch, shorts, youtu.be)
//! - Threads (post, share)
//! - X / Twitter (status)

/// Normalizes OCR text and extracts valid, sanitized social links.
pub fn extract_and_normalize_links(raw_text: &str) -> Vec<String> {
    if raw_text.trim().is_empty() {
        return Vec::new();
    }

    // Step 1: Strip stray iOS selection handle pipes and dots (|• ... •|)
    let mut cleaned = raw_text.to_string();
    cleaned = cleaned.replace("|•", "");
    cleaned = cleaned.replace("•|", "");
    cleaned = cleaned.replace("|", "");
    cleaned = cleaned.replace("\u{2014} Ask Siri", "");
    cleaned = cleaned.replace("- Ask Siri", "");

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
        let mut t = w.trim();
        // Strip trailing punctuation
        t = t.trim_end_matches(&['.', ',', ';', '!', ']', ')', '}', '>', ' ']);

        let mut candidate = t.to_string();
        if !candidate.starts_with("http://") && !candidate.starts_with("https://") {
            if candidate.starts_with("www.")
                || candidate.starts_with("instagram.com")
                || candidate.starts_with("tiktok.com")
                || candidate.starts_with("vt.tiktok.com")
                || candidate.starts_with("vm.tiktok.com")
                || candidate.starts_with("youtube.com")
                || candidate.starts_with("youtu.be")
                || candidate.starts_with("threads.net")
                || candidate.starts_with("threads.com")
                || candidate.starts_with("x.com")
                || candidate.starts_with("twitter.com")
            {
                candidate = format!("https://{}", candidate);
            }
        }

        // Strip Threads tracking query ?xmt=... on post links (keep clean canonical URL)
        let lower_cand = candidate.to_ascii_lowercase();
        if (lower_cand.contains("threads.com/") || lower_cand.contains("threads.net/"))
            && lower_cand.contains("/post/")
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

    results
}

/// Validates and canonicalizes an extracted social media URL (Tahap 194).
/// - Strictly rejects Instagram shortcodes containing '.' or having length < 11 or > 15.
/// - Normalizes '/reels/' to '/reel/'.
/// - Strips extra query parameters (&ig_mid=..., &utm_source=...) and invalid/short ?igsh= tokens.
pub fn clean_and_validate_extracted_url(raw_url: &str, stem_ratio: Option<f32>) -> Option<String> {
    let trimmed = raw_url
        .trim()
        .trim_end_matches(&['.', ',', ';', '!', ']', ')', '}', '>', ' ']);
    if trimmed.is_empty() || trimmed.contains(' ') {
        return None;
    }

    let mut candidate = trimmed.to_string();
    if candidate.starts_with("http://") {
        candidate = format!("https://{}", &candidate[7..]);
    } else if !candidate.starts_with("https://") {
        candidate = format!("https://{}", candidate);
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
        || lower.contains("threads.com/@")
        || lower.contains("threads.com/t/")
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
}
