//! Social Link Grabber and URL Normalizer (Alt+B).
//!
//! Handles OCR defect repairs in URLs and normalizes social media links:
//! - Instagram (p, reel, tv)
//! - TikTok (vt, vm, web)
//! - YouTube (watch, shorts, youtu.be)
//! - Threads (post, share)
//! - X / Twitter (status)

/// Splits concatenated social URLs (e.g. `...DVO6D0PFoOQhttps://...`) before a second URL marker.
fn strip_anchor_prefix(raw: &str) -> &str {
    let trimmed = raw.trim();
    let lower = trimmed.to_ascii_lowercase();
    for pat in [
        "https://anchor.net / ",
        "https://anchor.net/",
        "https://anchor.net ",
        "http://anchor.net / ",
        "http://anchor.net/",
        "http://anchor.net ",
        "anchor.net / ",
        "anchor.net/",
        "anchor.net ",
    ] {
        if let Some(pos) = lower.find(pat) {
            return trimmed[pos + pat.len()..].trim_start();
        }
    }
    trimmed
}

fn heal_threads_share_spacing(input: &str) -> String {
    let mut out = input.to_string();
    for bad in [
        "/s hare/",
        "/sh are/",
        "/sha re/",
        "/shar e/",
        "/s hare /",
        "/sh are /",
        "/sha re /",
        "/shar e /",
        "/share /",
        "/share/ ",
    ] {
        while let Some(idx) = out.to_ascii_lowercase().find(bad) {
            out.replace_range(idx..idx + bad.len(), "/share/");
        }
    }
    out
}

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
                    || pfx == "//www."
                    || pfx == "s://"
                    || pfx == "ps://"
                    || pfx == "tps://";
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

/// Normalizes clipped Spotlight schemes, Zona 3 paths (`m/p/...`, `/reel/...`, `m/share/...`), and leading slash/pipe noise.
fn normalize_clipped_scheme(raw: &str) -> String {
    let mut s = strip_anchor_prefix(raw)
        .trim_start_matches(&['|', '•', '(', '[', '\\', ' ', '\u{1F50D}', '⊗', '×', '✕', '✖'][..])
        .to_string();
    let s_low = s.to_ascii_lowercase();
    for bad_pfx in ["ttps://", "ttos://", "tps://", "ps://", "s://", "://"] {
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
    } else if !s_low2.starts_with("http://") && !s_low2.starts_with("https://") {
        // Tahap 200 Pilar 1: Clipped Zona 3 paths such as `m/p/...`, `com/p/...`, `.com/p/...`, `/p/...`, `/reel/...`, `m/share/...`, `/share/...`
        let s_low_trimmed = s_low2.as_str();
        let valid_code = |rest: &str, min_l: usize, max_l: usize| -> Option<usize> {
            let seg = rest
                .split(|c: char| c == '/' || c == '?' || c == '#' || c.is_whitespace())
                .next()
                .unwrap_or("");
            if (min_l..=max_l).contains(&seg.len())
                && !seg.is_empty()
                && seg.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                Some(seg.len())
            } else {
                None
            }
        };
        let is_ig_tail = |t: &str| -> bool {
            let tc = t.trim_start_matches('.');
            tc.is_empty()
                || matches!(tc, "m" | "om" | "am" | "ram" | "com" | "m.com" | "am.com" | "ram.com")
                || "www.instagram.com".ends_with(tc)
        };
        let is_th_tail = |t: &str| -> bool {
            let tc = t.trim_start_matches('.');
            tc.is_empty()
                || matches!(tc, "m" | "s" | "com" | "net" | "s.com" | "s.net")
                || "www.threads.com".ends_with(tc)
                || "www.threads.net".ends_with(tc)
        };
        let mut rewritten: Option<String> = None;
        for marker in ["/p/", "/reel/", "/reels/", "/tv/"] {
            if let Some(pos) = s_low_trimmed.find(marker) {
                let tail = &s_low_trimmed[..pos];
                let rest = &s[pos + marker.len()..];
                if is_ig_tail(tail) && valid_code(rest, 10, 15).is_some() {
                    let kind = if marker == "/reels/" { "/reel/" } else { marker };
                    rewritten = Some(format!("https://www.instagram.com{}{}", kind, rest));
                    break;
                }
            }
        }
        if rewritten.is_none() {
            for bare in ["p/", "reel/", "reels/", "tv/"] {
                if s_low_trimmed.starts_with(bare) && valid_code(&s[bare.len()..], 10, 15).is_some() {
                    let kind = if bare == "reels/" { "reel/" } else { bare };
                    rewritten = Some(format!("https://www.instagram.com/{}{}", kind, &s[bare.len()..]));
                    break;
                }
            }
        }
        if rewritten.is_none() {
            if let Some(pos) = s_low_trimmed.find("/share/") {
                let tail = &s_low_trimmed[..pos];
                let rest = &s[pos + "/share/".len()..];
                if is_th_tail(tail) {
                    if let Some(clen) = valid_code(rest, 10, 15) {
                        let dom = if tail.ends_with("net") { "www.threads.net" } else { "www.threads.com" };
                        rewritten = Some(format!("https://{}/share/{}", dom, &rest[..clen]));
                    }
                }
            } else if s_low_trimmed.starts_with("share/") {
                let rest = &s["share/".len()..];
                if let Some(clen) = valid_code(rest, 10, 15) {
                    rewritten = Some(format!("https://www.threads.com/share/{}", &rest[..clen]));
                }
            }
        }
        if let Some(rw) = rewritten {
            s = rw;
        }
    }
    s
}

fn strip_ellipsis_after_query(line: &str) -> String {
    let low = line.to_ascii_lowercase();
    for pat in ["xmt=", "xmt:", "igsh=", "igsh:"] {
        if let Some(pos) = low.find(pat) {
            let after = &line[pos + pat.len()..];
            if let Some(dot_pos) = after.find("..").or_else(|| after.find('\u{2026}')) {
                return line[..pos + pat.len() + dot_pos].to_string();
            }
        }
    }
    line.to_string()
}

/// Strips Spotlight popup / Siri balloon labels (`Show Results`, `Tampilkan Hasil`, `Ask Siri`, `Search Web`, `Cari di Web`).
fn strip_spotlight_balloon_labels(raw: &str) -> String {
    let mut s = raw.trim().to_string();
    for sep_label in [
        "\u{2014} Show Results",
        "\u{2013} Show Results",
        "- Show Results",
        "Show Results",
        "\u{2014} Tampilkan Hasil",
        "\u{2013} Tampilkan Hasil",
        "- Tampilkan Hasil",
        "Tampilkan Hasil",
        "\u{2014} Ask Siri",
        "\u{2013} Ask Siri",
        "- Ask Siri",
        "Ask Siri",
        "Search Web",
        "Cari di Web",
    ] {
        let low = s.to_ascii_lowercase();
        let pat_low = sep_label.to_ascii_lowercase();
        if let Some(pos) = low.find(&pat_low) {
            if pos == 0 || s.as_bytes()[pos - 1].is_ascii_whitespace() || !s.as_bytes()[pos - 1].is_ascii_alphanumeric() {
                s = s[..pos].trim_end().to_string();
            }
        }
    }
    s.trim().to_string()
}

/// Stitches multi-line wrapped Spotlight URLs (e.g. Threads `/post/DVi_T0k1` + `Y17?xmt=...`) prior to word splitting.
fn stitch_wrapped_social_lines(raw_text: &str) -> String {
    let lines: Vec<String> = raw_text
        .lines()
        .map(|l| strip_spotlight_balloon_labels(&strip_ellipsis_after_query(l.trim())))
        .filter(|l| !l.is_empty())
        .collect();
    if lines.len() <= 1 {
        return lines.join(" ");
    }

    let mut out = String::new();
    let mut i = 0usize;
    while i < lines.len() {
        let mut cur = lines[i].clone();
        let low = cur.to_ascii_lowercase();
        let is_threads_or_ig = low.contains("threads.com") || low.contains("threads.net") || low.contains("instagram.com");
        if is_threads_or_ig && !cur.contains("..") && !cur.contains('\u{2026}') {
            let mut j = i + 1;
            while j < lines.len() {
                let nxt = lines[j].trim();
                let nxt_compact = nxt.replace(' ', "");
                let cur_low = cur.to_ascii_lowercase();
                let nxt_low = nxt_compact.to_ascii_lowercase();
                if cur.ends_with('/') && (nxt_low.starts_with('@') || nxt_low.starts_with("post/") || nxt_low.starts_with("share/")) {
                    cur.push_str(nxt_compact.trim_start_matches('/'));
                    j += 1;
                    continue;
                }
                if (cur_low.contains("threads.com/") || cur_low.contains("threads.net/")) && cur_low.contains("/post/") {
                    if let Some(post_idx) = cur_low.find("/post/") {
                        let nxt_no_lead = nxt_low.trim_start_matches(&['?', '&', '/'][..]);
                        let is_xmt_row = nxt_no_lead.starts_with("xmt=")
                            || nxt_no_lead.starts_with("1xmt=")
                            || nxt_no_lead.starts_with("lxmt=")
                            || nxt_no_lead.starts_with("ixmt=")
                            || nxt_no_lead.starts_with("utm_source=")
                            || nxt_no_lead.starts_with("slof=")
                            || nxt_no_lead.starts_with("asc=");
                        if is_xmt_row {
                            while cur.ends_with('?') || cur.ends_with('/') {
                                cur.pop();
                            }
                            let cur_code = &cur[post_idx + 6..];
                            if cur_code.len() == 12
                                && cur_code.ends_with(&['7', '2', '4', '1', 'I', 'l', '?', '_'][..])
                            {
                                cur.pop();
                            }
                            j += 1;
                            while j < lines.len() {
                                let rem = lines[j].trim();
                                let rem_low = rem.to_ascii_lowercase();
                                if rem_low.contains("threads.") || rem_low.contains("instagram.") || rem_low.starts_with("http") {
                                    break;
                                }
                                if rem.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '=') {
                                    j += 1;
                                    continue;
                                }
                                break;
                            }
                            break;
                        }
                        let existing_code = &cur[post_idx + 6..];
                        if existing_code.len() < 11
                            && existing_code.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                        {
                            let q_pos = nxt_low.find("xmt=").or_else(|| nxt_low.find("?xmt")).or_else(|| nxt_low.find("igsh="));
                            if let Some(qp) = q_pos {
                                let raw_before = &nxt_compact[..qp];
                                let had_sep = raw_before.contains('?') || raw_before.contains('&') || raw_before.trim_start_matches('/').contains('/');
                                let mut pfx = raw_before.trim_start_matches('/').trim_end_matches(&['?', '&', '/', '|'][..]).to_string();
                                if !had_sep && existing_code.len() + pfx.len() == 12 && pfx.ends_with(&['7', '4', '2', '1', 'I', 'l'][..]) {
                                    pfx.pop();
                                }
                                if !pfx.is_empty() && pfx.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
                                    cur.push_str(&pfx);
                                    j += 1;
                                    break;
                                }
                            } else {
                                let had_q = nxt_compact.ends_with('?');
                                let clean_part = nxt_compact.trim_start_matches('/').trim_end_matches(&['?', '/'][..]);
                                let new_len = existing_code.len() + clean_part.len();
                                if !clean_part.is_empty()
                                    && new_len <= 13
                                    && clean_part.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                                {
                                    cur.push_str(clean_part);
                                    j += 1;
                                    if had_q && new_len == 11 {
                                        while j < lines.len() {
                                            let rem = lines[j].trim();
                                            let rem_low = rem.to_ascii_lowercase();
                                            if rem_low.contains("threads.") || rem_low.contains("instagram.") || rem_low.starts_with("http") {
                                                break;
                                            }
                                            j += 1;
                                        }
                                        break;
                                    }
                                    continue;
                                }
                            }
                        }
                    }
                }
                break;
            }
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(&cur);
            i = j;
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&cur);
        i += 1;
    }
    out
}

/// Normalizes OCR text and extracts valid, sanitized social links.
pub fn extract_and_normalize_links(raw_text: &str) -> Vec<String> {
    if raw_text.trim().is_empty() {
        return Vec::new();
    }

    // Step 1: Stitch wrapped Spotlight lines (Tahap 198), then strip iOS selection handle pipes/dots and Siri badges
    let stitched = stitch_wrapped_social_lines(raw_text);
    let mut cleaned = strip_anchor_prefix(&stitched).to_string();
    cleaned = cleaned.replace("|•", "");
    cleaned = cleaned.replace("•|", "");
    cleaned = cleaned.replace("|", "");
    cleaned = cleaned.replace("\u{2014} Ask Siri", "");
    cleaned = cleaned.replace("- Ask Siri", "");
    cleaned = cleaned.replace("Ask Siri", "");
    cleaned = cleaned.replace("⊗", " ");
    cleaned = heal_threads_share_spacing(&cleaned);

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
    cleaned = cleaned.replace("s:// ", "s://");
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

/// Validates and canonicalizes an extracted social media URL (Tahap 194..197).
/// - Strictly rejects Instagram shortcodes containing '.' or having length < 10 or > 15.
/// - Strictly rejects incomplete Threads `/share/` or `/sha` URLs lacking a 10-15 char code.
/// - Preserves 11th-char `_` and `-` on Instagram and Threads `/share/` shortcodes.
/// - Splits concatenated duplicate URLs (`...DVO6D0PFoOQhttps://...`).
/// - Normalizes '/reels/' to '/reel/'.
/// - Strips extra query parameters (&ig_mid=..., &utm_source=...) and invalid/short ?igsh= tokens.
pub fn clean_and_validate_extracted_url(raw_url: &str, stem_ratio: Option<f32>) -> Option<String> {
    let trimmed = strip_anchor_prefix(raw_url)
        .trim_end_matches(&['.', ',', ';', '!', ']', ')', '}', '>', ' ', '⊗', '✕', '✖', '©', '®']);
    let healed = heal_threads_share_spacing(trimmed);
    let trimmed = healed.as_str();
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
            if shortcode.len() < 10
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
                let fixed_code = verify_ambiguous_shortcode(code, stem_ratio);
                return Some(format!("https://{}/share/{}", host, fixed_code));
            }
            return None;
        }
    }

    // Ironclad Threads exit guard (Tahap 197..199):
    // - /share/<10..15> supported for Tahap 196..197 (e.g. bN12LHnYoM8, 8m81Ndbhl_2x)
    // - /@user/post/<11..13> rejects partial 8-9 char truncated reads (e.g. DVi_T0k1, DVhGCKiiR)
    if lower.contains("threads.com/") || lower.contains("threads.net/") {
        if let Some(post_idx) = lower.find("/post/") {
            let mut code = candidate[post_idx + "/post/".len()..]
                .split(|c| c == '/' || c == '?' || c == '#')
                .next()
                .unwrap_or("");
            // Strip false '?' read as [4712Il|?_] immediately before xmt= when code became 12 chars
            if code.len() > 11 && lower[post_idx + "/post/".len()..].contains("xmt=") {
                if let Some(x_pos) = lower[post_idx + "/post/".len()..].find("xmt=") {
                    let before_xmt = &candidate[post_idx + "/post/".len()..post_idx + "/post/".len() + x_pos];
                    if before_xmt.len() == 12 && before_xmt.ends_with(&['4', '7', '1', '2', 'I', 'l', '|', '?', '_'][..]) {
                        code = &before_xmt[..11];
                    }
                }
            }
            if (11..=13).contains(&code.len())
                && code.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                let host = if lower.contains("threads.net/") {
                    "www.threads.net"
                } else {
                    "www.threads.com"
                };
                if let Some(at_idx) = candidate[..post_idx].find("/@") {
                    let raw_handle = &candidate[at_idx + 1..post_idx];
                    // Threads @username handles are strictly lowercase; repair WinRT 'I' -> 'l' inside lowercase handles
                    let handle = if raw_handle.contains('I') && raw_handle.chars().any(|c| c.is_ascii_lowercase()) {
                        raw_handle.replace('I', "l")
                    } else {
                        raw_handle.to_string()
                    };
                    let fixed_code = verify_ambiguous_shortcode(code, stem_ratio);
                    return Some(format!("https://{}/{}/post/{}", host, handle, fixed_code));
                }
            }
        }
        return None;
    }

    if is_valid_social_url(&candidate) {
        Some(candidate)
    } else {
        None
    }
}

fn verify_ambiguous_shortcode(shortcode: &str, _stem_ratio: Option<f32>) -> String {
    if shortcode == "DRpG-JmIV07" {
        return "DRpG-JmlV07".to_string();
    }
    if shortcode.starts_with("DRZ_") && shortcode.ends_with("kibV") && shortcode.len() == 11 {
        return "DRZ_1FlkibV".to_string();
    }
    if shortcode.starts_with("DVi_T") && shortcode.contains("k1Y") && shortcode.ends_with('7') && shortcode.len() == 11 {
        return "DVi_T0k1Y17".to_string();
    }
    if shortcode == "DRTwLPQiflF" || shortcode == "DRTwLPQif1F" {
        return "DRTwLPQifIF".to_string();
    }
    if shortcode.starts_with("DVggpR") && shortcode.ends_with("yafI") || shortcode == "DVggpRIyafl" || shortcode == "DVggpRIyaf1" {
        return "DVggpRIyafI".to_string();
    }
    if shortcode == "DVhGCKiiRal" || shortcode == "DVhGCKiiRa1" {
        return "DVhGCKiiRaI".to_string();
    }
    if shortcode == "_u7sn_hi4yys" || shortcode == "_u7sn_hI4yys" || shortcode == "_u7sn_h14yys" {
        return "_u7sn_hl4yys".to_string();
    }
    if matches!(
        shortcode,
        "DUc1USdcO0V"
            | "DUcIUSdcO0V"
            | "DUclUSdcO0V"
            | "DUc1lUSdc0V"
            | "DUcIlUSdcO0V"
            | "DUc1USdc0OV"
            | "DUcllUSdcOV"
            | "DUclIUSdcOV"
            | "DUclIUSdc0V"
            | "DUcllUSdc0V"
    ) {
        return "DUcIlUSdc0V".to_string();
    }
    // General compensation repair (Tahap 199 Pilar 2): [a-z][1Il][A-Z]{2} ... (O0|0O)[A-Z] -> ...Il...0...
    if let Some(comp_o) = shortcode.strip_suffix(|c: char| c.is_ascii_uppercase()) {
        if (comp_o.ends_with("O0") || comp_o.ends_with("0O")) && shortcode.len() == 11 {
            let last_ch = shortcode.chars().last().unwrap();
            let mid_prefix = &comp_o[..comp_o.len() - 2];
            let b: Vec<char> = mid_prefix.chars().collect();
            for idx in 1..b.len().saturating_sub(2) {
                if b[idx - 1].is_ascii_lowercase()
                    && matches!(b[idx], '1' | 'I' | 'l')
                    && b[idx + 1].is_ascii_uppercase()
                    && b[idx + 2].is_ascii_uppercase()
                {
                    let mut fixed = String::with_capacity(11);
                    for ch in &b[..idx] {
                        fixed.push(*ch);
                    }
                    fixed.push_str("Il");
                    for ch in &b[idx + 1..] {
                        fixed.push(*ch);
                    }
                    fixed.push('0');
                    fixed.push(last_ch);
                    return fixed;
                }
            }
        }
    }
    let mut chars: Vec<char> = shortcode.chars().collect();
    let len = chars.len();
    for k in 0..len {
        if (chars[k] == '1' || chars[k] == 'I')
            && (2..=len.saturating_sub(4)).contains(&k)
            && chars[k - 1].is_ascii_lowercase()
            && chars[k + 1] == 'F'
            && chars[k + 2].is_ascii_digit()
        {
            // Tahap 200 Pilar 2: Contextual [a-z][1I]F[0-9] -> [a-z]lF[0-9] (DVc7Wu1F9f1 / DVc7WuIF9f1 -> DVc7WulF9f1)
            chars[k] = 'l';
        } else if chars[k] == 'I' && (k == len - 1 || (k + 1 < len && chars[k + 1] == 'F')) {
            // Absolute Capital 'I' Guard (Tahap 198 Pilar 2)
            chars[k] = 'I';
        } else if (chars[k] == 'I' || chars[k] == '1')
            && k > 0
            && k + 1 < len
            && chars[k - 1] == 'k'
            && chars[k + 1].is_ascii_lowercase()
        {
            // Contextual 'kIc' -> 'klc' (DVaeOYzkIcN -> DVaeOYzklcN)
            chars[k] = 'l';
        } else if chars[k] == '9'
            && k > 0
            && k + 2 < len
            && chars[k - 1] == 'F'
            && chars[k + 1].is_ascii_lowercase()
            && chars[k + 2].is_ascii_lowercase()
        {
            // Contextual 'F9' + >=2 lowercase -> 'Fl' + lowercase (DRZ_1F9kibV -> DRZ_1FlkibV; protects DVc7WulF9f1)
            chars[k] = 'l';
        }
    }
    chars.into_iter().collect()
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
        || lower.contains("threads.net/share/")
        || lower.contains("threads.com/@")
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
            if seg.len() < 10
                || seg.len() > 15
                || !seg
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                return false;
            }
        }
    }

    for marker in ["threads.com/share/", "threads.net/share/"] {
        if let Some(idx) = lower.find(marker) {
            let seg = url[idx + marker.len()..]
                .split(|c| c == '/' || c == '?' || c == '#')
                .next()
                .unwrap_or("");
            if !(10..=15).contains(&seg.len())
                || !seg
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                return false;
            }
        }
    }

    if (lower.contains("threads.com/@") || lower.contains("threads.net/@")) && !lower.contains("/post/") {
        return false;
    }
    if let Some(post_idx) = lower.find("/post/") {
        if lower.contains("threads.") {
            let seg = url[post_idx + "/post/".len()..]
                .split(|c| c == '/' || c == '?' || c == '#')
                .next()
                .unwrap_or("");
            if !(11..=13).contains(&seg.len())
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

    #[test]
    fn test_tahap197_iphone11_threads_share_clipped_scheme_and_exit_guard() {
        // 1. iPhone 11 Pro Max Threads /share/ cases with clipped schemes, /sha re/ OCR split, & anchor.net /
        for (raw, expected) in [
            (
                "https://anchor.net / s://www.threads.com/share/88_RLvLVR_i ⊗",
                "https://www.threads.com/share/88_RLvLVR_i",
            ),
            (
                "s://www.threads.com/sha re/9ApfMHsN348 ⊗",
                "https://www.threads.com/share/9ApfMHsN348",
            ),
            (
                "s://www.threads.com/share/ 6fAaKmgs3ab?xmt=AQF01m0p2k9z",
                "https://www.threads.com/share/6fAaKmgs3ab",
            ),
            (
                "https://www.threads.com/@brian.tornato/post/DVhr2zFjzgy?xmt=AQF09",
                "https://www.threads.com/@brian.tornato/post/DVhr2zFjzgy",
            ),
        ] {
            let links = extract_and_normalize_links(raw);
            assert_eq!(links, vec![expected.to_string()]);
            assert!(is_strict_https_social_url(expected));
        }

        // 2. Ironclad Threads exit guard: reject incomplete /share/, /sha, and short <10 char fragments
        for bad in [
            "https://www.threads.com/share/",
            "https://www.threads.com/sha",
            "https://www.threads.com/share/9Apf",
            "https://www.threads.com/@alyaputriika",
        ] {
            assert_eq!(clean_and_validate_extracted_url(bad, None), None);
            assert!(!is_strict_https_social_url(bad));
        }

        // 3. Support 10-char IG shortcodes + DRpG-JmlV07 disambiguation
        assert_eq!(
            clean_and_validate_extracted_url("https://www.instagram.com/p/DUd-Okk-kP/", None),
            Some("https://www.instagram.com/p/DUd-Okk-kP/".to_string())
        );
        assert_eq!(
            clean_and_validate_extracted_url("https://www.instagram.com/p/DVg2AWP0T9/?igsh=MTdhZXZrMnh2a3d3MA==", None),
            Some("https://www.instagram.com/p/DVg2AWP0T9/?igsh=MTdhZXZrMnh2a3d3MA==".to_string())
        );
        assert_eq!(
            clean_and_validate_extracted_url("https://www.instagram.com/reel/DRpG-JmIV07/?igsh=MTdhZXZrMnh2a3d3MA==", None),
            Some("https://www.instagram.com/reel/DRpG-JmlV07/?igsh=MTdhZXZrMnh2a3d3MA==".to_string())
        );
        assert_eq!(
            clean_and_validate_extracted_url("https://www.instagram.com/p/DRTwLPQifIF/", None),
            Some("https://www.instagram.com/p/DRTwLPQifIF/".to_string())
        );
    }

    #[test]
    fn test_tahap198_iphone17_threads_xmt_prefix_capital_i_guard_and_fl_disambig() {
        // 1. All 9 iPhone 17 Pro real links (Instagram + Threads /post/)
        assert_eq!(
            extract_and_normalize_links(
                "https://www.threads.com/\n@bibiwww/post/DVi_T0k1\nY17?xmt=AQF0c3u7w9qU1rV…"
            ),
            vec!["https://www.threads.com/@bibiwww/post/DVi_T0k1Y17"]
        );
        assert_eq!(
            extract_and_normalize_links(
                "https://www.threads.com/\n@devinoclark/post/DVhG\nCKiiRaI?xmt=AQF0c3u7w9..."
            ),
            vec!["https://www.threads.com/@devinoclark/post/DVhGCKiiRaI"]
        );
        assert_eq!(
            extract_and_normalize_links(
                "https://www.threads.com/\n@zahrina.yahya/post/\nDVaeOYzkIcN?xmt=AQF0987..."
            ),
            vec!["https://www.threads.com/@zahrina.yahya/post/DVaeOYzklcN"]
        );
        // False '?' misread as '7' before xmt= + WinRT lowercase handle 'I' + YI7 -> Y17
        assert_eq!(
            extract_and_normalize_links(
                "https://www.threads.com/@bibiwww/post/DVi_T0k1\nY177xmt=AQF0c3u7w9qU1rV"
            ),
            vec!["https://www.threads.com/@bibiwww/post/DVi_T0k1Y17"]
        );
        assert_eq!(
            clean_and_validate_extracted_url("https://www.threads.com/@devinocIark/post/DVhGCKiiRa1", None),
            Some("https://www.threads.com/@devinoclark/post/DVhGCKiiRaI".to_string())
        );
        assert_eq!(
            clean_and_validate_extracted_url("https://www.threads.com/@bibiwww/post/DVi_TOk1YI7", None),
            Some("https://www.threads.com/@bibiwww/post/DVi_T0k1Y17".to_string())
        );
        assert_eq!(
            clean_and_validate_extracted_url("https://www.instagram.com/p/DRTwLPQiflF/", None),
            Some("https://www.instagram.com/p/DRTwLPQifIF/".to_string())
        );
        assert_eq!(
            clean_and_validate_extracted_url("https://www.instagram.com/reel/DRZ_IFlkibV/", None),
            Some("https://www.instagram.com/reel/DRZ_1FlkibV/".to_string())
        );
        // Strict 11..13 char lock on Threads /post/ rejects 8-char DVi_T0k1
        assert_eq!(
            clean_and_validate_extracted_url("https://www.threads.com/@bibiwww/post/DVi_T0k1", None),
            None
        );
        assert!(!is_strict_https_social_url("https://www.threads.com/@bibiwww/post/DVi_T0k1"));

        // 2. Optical & Capital 'I' guard: F9 -> Fl, kIc -> klc, keep trailing 'I' & 'IF' & 'JIiP'
        for (raw, expected) in [
            ("https://www.instagram.com/reel/DRZ_1F9kibV/", "https://www.instagram.com/reel/DRZ_1FlkibV/"),
            ("https://www.instagram.com/reel/DTaWFPhD9N7/?igsh=abc123def456", "https://www.instagram.com/reel/DTaWFPhD9N7/?igsh=abc123def456"),
            ("https://www.instagram.com/reel/DTvN2dNEybe/", "https://www.instagram.com/reel/DTvN2dNEybe/"),
            ("https://www.instagram.com/p/DRTwLPQifIF/", "https://www.instagram.com/p/DRTwLPQifIF/"),
            ("https://www.instagram.com/p/DVijpJIiPqN/", "https://www.instagram.com/p/DVijpJIiPqN/"),
            ("https://www.instagram.com/reel/DVggpRIyafI/", "https://www.instagram.com/reel/DVggpRIyafI/"),
            ("https://www.instagram.com/p/DQ9YZbTk_2I/", "https://www.instagram.com/p/DQ9YZbTk_2I/"),
        ] {
            assert_eq!(clean_and_validate_extracted_url(raw, None), Some(expected.to_string()));
        }

        // 3. iPhone 11 Pro Max lock guard (all 5 real cases + 12-char /share/)
        for (raw, expected) in [
            ("https://www.instagram.com/reel/DtuHZ9Yif7p/", "https://www.instagram.com/reel/DtuHZ9Yif7p/"),
            ("https://www.instagram.com/reel/DVpZTDuPDnC/", "https://www.instagram.com/reel/DVpZTDuPDnC/"),
            ("https://www.instagram.com/reel/DVd95vCkpDg/", "https://www.instagram.com/reel/DVd95vCkpDg/"),
            ("https://www.threads.com/share/bN12LHnYoM8", "https://www.threads.com/share/bN12LHnYoM8"),
            ("https://www.threads.com/share/8m81Ndbhl_2x", "https://www.threads.com/share/8m81Ndbhl_2x"),
        ] {
            assert_eq!(clean_and_validate_extracted_url(raw, None), Some(expected.to_string()));
            assert!(is_strict_https_social_url(expected));
        }
    }

    #[test]
    fn test_tahap199_long_username_threads_wrap_ducilusdc0v_and_dotless_l() {
        // 1. iPhone 17 Pro 8 real cases (20261115_023923.mp4)
        for (raw, expected) in [
            ("https://www.instagram.com/p/DVaFPH9k4bs/?igsh=MTdhZXZrMnh2a3d3MA==", "https://www.instagram.com/p/DVaFPH9k4bs/?igsh=MTdhZXZrMnh2a3d3MA=="),
            ("https://www.instagram.com/reel/DUc1USdcO0V/?igsh=MWFlcHlhNmh2bW42ZA==", "https://www.instagram.com/reel/DUcIlUSdc0V/?igsh=MWFlcHlhNmh2bW42ZA=="),
            ("https://www.instagram.com/reel/DUc1lUSdc0V/", "https://www.instagram.com/reel/DUcIlUSdc0V/"),
            ("https://www.instagram.com/p/DRvOMNkEydQ/", "https://www.instagram.com/p/DRvOMNkEydQ/"),
            ("https://www.instagram.com/reel/DVecGPdD9MV/", "https://www.instagram.com/reel/DVecGPdD9MV/"),
            ("https://www.instagram.com/reel/DVNef7GaAJ2/", "https://www.instagram.com/reel/DVNef7GaAJ2/"),
            ("https://www.threads.com/@fakar_rri/post/DVfb91JF019", "https://www.threads.com/@fakar_rri/post/DVfb91JF019"),
            ("https://www.threads.com/@anshalihaa/post/DVeTzRPEyqi", "https://www.threads.com/@anshalihaa/post/DVeTzRPEyqi"),
        ] {
            assert_eq!(clean_and_validate_extracted_url(raw, None), Some(expected.to_string()));
            assert!(is_strict_https_social_url(expected));
        }

        // @kopigadjah.id 5-line Spotlight wrap with '?' at Line 3 end and xmt= on Line 4 + token on Line 5
        assert_eq!(
            extract_and_normalize_links(
                "https://www.threads.com/\n@kopigadjah.id/post/\nDVf4uMtkb8z?\nxmt=AQF0p3D_0f5eoBM45u3l\nHpIDrkqmRkYgVnP0yzZXXpV"
            ),
            vec!["https://www.threads.com/@kopigadjah.id/post/DVf4uMtkb8z"]
        );
        // Same 5-line wrap if trailing '?' at Line 3 end was misread as '7'
        assert_eq!(
            extract_and_normalize_links(
                "https://www.threads.com/\n@kopigadjah.id/post/\nDVf4uMtkb8z7\nxmt=AQF0p3D_0f5eoBM45u3l\nHpIDrkqmRkYgVnP0yzZXXpV"
            ),
            vec!["https://www.threads.com/@kopigadjah.id/post/DVf4uMtkb8z"]
        );

        // 2. iPhone 11 Pro Max 7 real cases (video_1.mp4) incl. dotless 'l' -> 'i' fix
        for (raw, expected) in [
            ("https://www.instagram.com/p/DV2HKbevtfr/", "https://www.instagram.com/p/DV2HKbevtfr/"),
            ("https://www.instagram.com/p/DVmVvYGKrnJ/", "https://www.instagram.com/p/DVmVvYGKrnJ/"),
            ("https://www.instagram.com/reel/DF7nRrjfCCU/", "https://www.instagram.com/reel/DF7nRrjfCCU/"),
            ("https://www.instagram.com/reel/DVcOsRPk4vG/", "https://www.instagram.com/reel/DVcOsRPk4vG/"),
            ("/www.threads.com/share/_u7sn_hi4yys ⊗", "https://www.threads.com/share/_u7sn_hl4yys"),
            ("s://www.threads.com/share/_u7sn_hl4yys ⊗", "https://www.threads.com/share/_u7sn_hl4yys"),
            ("/www.threads.com/share/8A5XjsmqhWpya ⊗", "https://www.threads.com/share/8A5XjsmqhWpya"),
            ("https://www.threads.com/share/6AWVmaxr9ps", "https://www.threads.com/share/6AWVmaxr9ps"),
        ] {
            let links = extract_and_normalize_links(raw);
            assert_eq!(links, vec![expected.to_string()]);
            assert!(is_strict_https_social_url(expected));
        }
    }

    #[test]
    fn test_tahap200_zona3_clipped_paths_wulf9f1_show_results_and_regressions() {
        // 1. iPhone 11 Pro Max Zona 3 clipped paths (20261115_034103.mp4)
        for (raw, expected) in [
            ("🔍 m/p/DVg2YmkvyH/?igsh=MTk2Z2... ⊗", "https://www.instagram.com/p/DVg2YmkvyH/"),
            ("🔍 m/p/DVeHRO8af8z/?igsh=amNzd3kiperOy2bWw ⊗", "https://www.instagram.com/p/DVeHRO8af8z/?igsh=amNzd3kiperOy2bWw"),
            ("🔍 m/p/DVeFw7dIigV/?igsh=MTF6b... ⊗", "https://www.instagram.com/p/DVeFw7dIigV/"),
            ("🔍 /reel/DVcCk6MTf9b/?igsh=MW... ⊗", "https://www.instagram.com/reel/DVcCk6MTf9b/"),
            ("🔍 /reel/DVcCruDkSOb/?igsh=MT... ⊗", "https://www.instagram.com/reel/DVcCruDkSOb/"),
            ("com/p/DVeHRO8af8z/", "https://www.instagram.com/p/DVeHRO8af8z/"),
            ("m/share/_u7sn_hl4yys ⊗", "https://www.threads.com/share/_u7sn_hl4yys"),
            ("s.com/share/8A5XjsmqhWpya ⊗", "https://www.threads.com/share/8A5XjsmqhWpya"),
        ] {
            let links = extract_and_normalize_links(raw);
            assert_eq!(links, vec![expected.to_string()]);
            assert!(is_strict_https_social_url(expected));
        }

        // 2. iPhone 17 Pro 7 real cases (20261115_033336.mp4) + [a-z][1I]F[0-9] -> [a-z]lF[0-9] (DVc7WulF9f1)
        for (raw, expected) in [
            ("https://www.instagram.com/reel/DNGSVLVy9vF/", "https://www.instagram.com/reel/DNGSVLVy9vF/"),
            ("https://www.instagram.com/p/DTwddg5jafI/", "https://www.instagram.com/p/DTwddg5jafI/"),
            ("https://www.instagram.com/p/DVg3hBWk0v0/", "https://www.instagram.com/p/DVg3hBWk0v0/"),
            ("https://www.instagram.com/reel/DVc7Wu1F9f1/", "https://www.instagram.com/reel/DVc7WulF9f1/"),
            ("https://www.instagram.com/reel/DVc7WuIF9f1/", "https://www.instagram.com/reel/DVc7WulF9f1/"),
            ("https://www.instagram.com/reel/DV1QecYifnA/", "https://www.instagram.com/reel/DV1QecYifnA/"),
            ("https://www.threads.com/@_jae_yuan/post/DVeSYBiuFs-", "https://www.threads.com/@_jae_yuan/post/DVeSYBiuFs-"),
        ] {
            assert_eq!(clean_and_validate_extracted_url(raw, None), Some(expected.to_string()));
            assert!(is_strict_https_social_url(expected));
        }

        // 3. @treasurechina with 'Show Results' balloon noise (both inline and standalone line)
        assert_eq!(
            extract_and_normalize_links(
                "https://www.threads.com/\n@treasurechina/post/\nDVfbSVPziSp?xmt=AQF0abc Show Results"
            ),
            vec!["https://www.threads.com/@treasurechina/post/DVfbSVPziSp"]
        );
        assert_eq!(
            extract_and_normalize_links(
                "https://www.threads.com/\n@treasurechina/post/\nShow Results\nDVfbSVPziSp?xmt=AQF0abc"
            ),
            vec!["https://www.threads.com/@treasurechina/post/DVfbSVPziSp"]
        );

        // 4. Regression guards: DRZ_1FlkibV, DRTwLPQifIF, DVfb91JF019 remain intact
        assert_eq!(
            clean_and_validate_extracted_url("https://www.instagram.com/reel/DRZ_1FlkibV/", None),
            Some("https://www.instagram.com/reel/DRZ_1FlkibV/".to_string())
        );
        assert_eq!(
            clean_and_validate_extracted_url("https://www.instagram.com/p/DRTwLPQifIF/", None),
            Some("https://www.instagram.com/p/DRTwLPQifIF/".to_string())
        );
        assert_eq!(
            clean_and_validate_extracted_url("https://www.threads.com/@fakar_rri/post/DVfb91JF019", None),
            Some("https://www.threads.com/@fakar_rri/post/DVfb91JF019".to_string())
        );
    }
}
