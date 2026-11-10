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
                || candidate.starts_with("x.com")
                || candidate.starts_with("twitter.com")
            {
                candidate = format!("https://{}", candidate);
            }
        }

        if is_valid_social_url(&candidate) && !results.contains(&candidate) {
            results.push(candidate);
        }
    }

    results
}

/// Checks whether a URL is a recognized social link platform.
pub fn is_valid_social_url(url: &str) -> bool {
    let lower = url.to_lowercase();
    (lower.contains("instagram.com/p/")
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
        || lower.contains("x.com/")
        || lower.contains("twitter.com/"))
        && !url.contains(' ')
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
}
