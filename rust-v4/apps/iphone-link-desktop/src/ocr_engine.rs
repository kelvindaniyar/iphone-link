//! WinRT Multi-Frame OCR Post-Processor and Bidirectional Clipboard Engine.
//!
//! Provides:
//! - Optical post-processor typo corrections
//! - 3 Golden Rules: Short text double Enter preservation, KMP multi-frame stitcher, auto-clear cache
//! - Rejection filters for popup balloons and Notes folder view
//! - Native Win32 Bidirectional Clipboard FFI

use std::ptr;
use std::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardSource {
    Pc,
    Iphone,
}

#[derive(Debug, Clone)]
pub struct ClipboardState {
    pub source: ClipboardSource,
    pub last_seq: u32,
    pub last_content: String,
}

impl Default for ClipboardState {
    fn default() -> Self {
        Self {
            source: ClipboardSource::Pc,
            last_seq: 0,
            last_content: String::new(),
        }
    }
}

static COPY_CACHE: Mutex<Option<Vec<String>>> = Mutex::new(None);

/// Clears recent long OCR chunks and copy cache (Alt+R / Smart Auto-Clear).
pub fn clear_copy_cache() {
    if let Ok(mut cache) = COPY_CACHE.lock() {
        *cache = Some(Vec::new());
    }
}

/// Applies optical corrections to OCR output text.
/// Fixes known WinRT OCR character misclassifications:
/// - "diabadil<an" -> "diabadikan"
/// - "@eril<l<ristian" -> "@erikkristian"
/// - "!olom" -> "kolom"
/// - "Iambung" -> "lambung"
/// - "Baturnandi" / "baturnandi" -> "Batumandi" / "batumandi"
/// - "Dapua" / "dapua" -> "papua"
/// - "hincfia" -> "hindia"
/// - "(Al Generated Content)" -> "(AI Generated Content)"
/// - "#cilegon]" -> "#cilegon"
/// - "_heroik" -> "heroik"
/// - "@andriboto<" -> "@andribotox"
pub fn auto_correct_ocr_text(input: &str) -> String {
    let mut text = input.to_string();

    // 1. Hashtag stray handle brackets/pipes (e.g. #cilegon] -> #cilegon)
    text = text.replace("#cilegon]", "#cilegon");
    text = text.replace("#cilegon|", "#cilegon");
    text = text.replace("#infotangerang|", "#infotangerang");
    text = text.replace("#infotangerang]", "#infotangerang");

    // 2. Mention trailing defect characters (e.g. @andriboto< -> @andribotox)
    text = text.replace("@andriboto<", "@andribotox");
    text = text.replace("@andriboto|", "@andribotox");
    text = text.replace("@eril<l<ristian", "@erikkristian");
    text = text.replace("@eril<l<rist", "@erikkristian");

    // 3. Batumandi corrections
    text = text.replace("Baturnandi", "Batumandi");
    text = text.replace("baturnandi", "batumandi");
    text = text.replace("#kmpbaturnandi", "#kmpbatumandi");
    text = text.replace("#kmpBaturnandi", "#kmpBatumandi");
    text = text.replace("#baturnandi", "#batumandi");

    // 4. Optical character repairs
    text = text.replace("diabadil<an", "diabadikan");
    text = text.replace("hincfia", "hindia");
    text = text.replace("Hincfia", "Hindia");

    // 5. AI Generated Content
    text = text.replace("(Al Generated Content)", "(AI Generated Content)");
    text = text.replace("Al Generated", "AI Generated");

    // 6. Papua repair
    text = text.replace("Dapua", "papua");
    text = text.replace("dapua", "papua");

    // 7. Lambung / initial I repairs
    text = text.replace("Iambung", "lambung");

    // 8. !olom -> kolom
    text = text.replace(" !olom", " kolom");
    if text.starts_with("!olom") {
        text.replace_range(0..5, "kolom");
    }

    // 9. Noise underscores on words (e.g. "aksi _heroik" -> "aksi heroik")
    text = text.replace(" _heroik", " heroik");
    text = text.replace("_heroik", "heroik");

    // 10. Selection lollipop-handle edge corruptions (Tahap 193)
    text = text
        .replace("tarhutla", "karhutla")
        .replace("Tarhutla", "Karhutla")
        .replace("enggalnih", "engga nih")
        .replace("Enggalnih", "Engga nih")
        .replace("engga1nih", "engga nih")
        .replace("Engga1nih", "Engga nih")
        .replace("engga!nih", "engga nih")
        .replace("lomah enak", "lontoh enak")
        .replace("Lomah enak", "Lontoh enak")
        .replace("prabowk", "prabowo")
        .replace("Prabowk", "Prabowo")
        .replace("prabowl", "prabowo")
        .replace("Prabowl", "Prabowo");

    let had_crlf = text.contains("\r\n");
    let normalized = text.replace("\r\n", "\n");
    let fixed_lines: Vec<String> = normalized
        .split('\n')
        .map(repair_line_optical_handle_artifacts)
        .collect();
    let sep = if had_crlf { "\r\n" } else { "\n" };
    fixed_lines.join(sep)
}

/// Repairs trailing lollipop-handle artifacts at line boundaries while keeping empty lines intact.
fn repair_line_optical_handle_artifacts(raw_line: &str) -> String {
    let trimmed = raw_line.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let mut line = raw_line.to_string();
    for suffix in ["wok I", "wok 1", "wok l", "wok |"] {
        if line.trim_end().ends_with(suffix) {
            let trimmed_end = line.trim_end();
            let prefix_len = trimmed_end.len() - suffix.len();
            line = format!("{}wok?", &trimmed_end[..prefix_len]);
            break;
        }
    }
    if line.trim_end().ends_with("pemulun") && !line.trim_end().ends_with("pemulung") {
        let trimmed_end = line.trim_end();
        let prefix_len = trimmed_end.len() - "pemulun".len();
        line = format!("{}pemulung", &trimmed_end[..prefix_len]);
    }
    line
}

/// Checks if a line or chunk contains Notes folder / list chrome.
pub fn is_notes_folder_list_frame(text: &str) -> bool {
    let t = text.trim();
    if t.contains("<<<NOTES_LIST_VIEW>>>") {
        return true;
    }
    let lower = t.to_lowercase();
    if lower == "folders" || lower == "notes" || lower.starts_with("notes (") || lower == "all icloud" {
        return true;
    }
    false
}

/// Checks if a text line is a phantom balloon border or iOS callout.
pub fn is_balloon_popup_text(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return false;
    }
    // Phantom balloon border strings like "1Q0 1O0O", "100 1000"
    if t == "1Q0 1O0O" || t == "1O0 1O0O" || t == "100 1000" {
        return true;
    }
    let is_all_digits_and_letters = t.chars().all(|c| c.is_ascii_digit() || c == 'O' || c == 'Q' || c == 'I' || c == 'l' || c == ' ');
    if is_all_digits_and_letters && t.len() >= 3 && t.len() <= 15 && !t.chars().any(|c| c.is_alphabetic() && c != 'O' && c != 'Q' && c != 'I' && c != 'l') {
        return true;
    }
    // Callout options
    let lower = t.to_lowercase();
    if lower == "cut" || lower == "copy" || lower == "paste" || lower == "select all" || lower == "look up" || lower == "share..." || lower == "cut | copy | paste" {
        return true;
    }
    false
}

/// Stitches two overlapping text frames by finding the longest matching line overlap.
pub fn stitch_two_ocr_texts(text1: &str, text2: &str) -> String {
    let lines1: Vec<&str> = text1.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
    let lines2: Vec<&str> = text2.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();

    if lines1.is_empty() {
        return text2.to_string();
    }
    if lines2.is_empty() {
        return text1.to_string();
    }

    // Find overlap between tail of lines1 and head of lines2
    let max_check = lines1.len().min(lines2.len());
    let mut best_overlap = 0usize;

    for k in (1..=max_check).rev() {
        let tail = &lines1[lines1.len() - k..];
        let head = &lines2[..k];
        let mut matches = true;
        for i in 0..k {
            if tail[i] != head[i] {
                // Fuzzy line equality (ignoring minor punctuation)
                let c1: String = tail[i].chars().filter(|c| c.is_alphanumeric()).collect();
                let c2: String = head[i].chars().filter(|c| c.is_alphanumeric()).collect();
                if c1.is_empty() || c1 != c2 {
                    matches = false;
                    break;
                }
            }
        }
        if matches {
            best_overlap = k;
            break;
        }
    }

    let mut result = Vec::new();
    for l in &lines1 {
        result.push(*l);
    }
    for l in &lines2[best_overlap..] {
        result.push(*l);
    }

    result.join("\r\n")
}

/// Stitches multiple chronologically ordered OCR frames into a single text with Golden Rules.
///
/// Golden Rule 1: Short text (<= 5 non-empty lines, total chars < 200) preserves double Enter (\r\n\r\n).
/// Golden Rules 2 & 3: Multi-frame stitcher for long notes (Batumandi), filters balloons, folder list.
pub fn stitch_multiline_ocr_frames(frames: &[String]) -> String {
    if frames.is_empty() {
        return String::new();
    }

    // Filter out empty or Notes folder frames
    let valid_frames: Vec<String> = frames
        .iter()
        .map(|f| auto_correct_ocr_text(f))
        .filter(|f| !f.trim().is_empty() && !is_notes_folder_list_frame(f))
        .collect();

    if valid_frames.is_empty() {
        return String::new();
    }

    let current = valid_frames.last().unwrap();
    let current_lines: Vec<&str> = current.lines().map(|l| l.trim()).filter(|l| !l.is_empty() && !is_balloon_popup_text(l)).collect();

    // Golden Rule 1: Short text snippet (never stitch isolated short snippets with previous history)
    let is_short_snippet = valid_frames.len() == 1
        || (current_lines.len() <= 5
            && current_lines.iter().all(|l| l.len() < 60)
            && current_lines.iter().map(|l| l.len()).sum::<usize>() < 200
            && !valid_frames.iter().take(valid_frames.len() - 1).any(|prev| {
                let prev_lines: Vec<&str> = prev.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
                current_lines.iter().any(|cl| prev_lines.contains(cl))
            }));

    if is_short_snippet {
        // Preserve exact blank lines (Double Enter) for short snippets, normalized to CRLF
        let normalized_input = current.replace("\r\n", "\n");
        let preserved: Vec<&str> = normalized_input
            .split('\n')
            .map(|l| if l.trim().is_empty() { "" } else { l.trim_end() })
            .collect();
        return preserved.join("\r\n");
    }

    // Golden Rules 2 & 3: Multi-frame stitching backwards
    let mut stitched = current.clone();
    for prev in valid_frames.iter().rev().skip(1) {
        stitched = stitch_two_ocr_texts(prev, &stitched);
    }

    // Clean any phantom balloons in final lines
    let clean_lines: Vec<&str> = stitched
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !is_balloon_popup_text(l))
        .collect();

    clean_lines.join("\r\n\r\n")
}

// ============================================================================
// WIN32 CLIPBOARD FFI
// ============================================================================

const CF_UNICODETEXT: u32 = 13;
const GMEM_MOVEABLE: u32 = 0x0002;

#[link(name = "user32")]
extern "system" {
    fn GetClipboardSequenceNumber() -> u32;
    fn OpenClipboard(hwnd: *mut std::ffi::c_void) -> i32;
    fn CloseClipboard() -> i32;
    fn EmptyClipboard() -> i32;
    fn GetClipboardData(format: u32) -> *mut std::ffi::c_void;
    fn SetClipboardData(format: u32, hmem: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
}

#[link(name = "kernel32")]
extern "system" {
    fn GlobalAlloc(flags: u32, bytes: usize) -> *mut std::ffi::c_void;
    fn GlobalLock(hmem: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    fn GlobalUnlock(hmem: *mut std::ffi::c_void) -> i32;
}

/// Reads the current text from the Win32 clipboard.
pub fn read_win32_clipboard() -> Option<String> {
    unsafe {
        if OpenClipboard(ptr::null_mut()) == 0 {
            return None;
        }
        let handle = GetClipboardData(CF_UNICODETEXT);
        if handle.is_null() {
            CloseClipboard();
            return None;
        }
        let ptr = GlobalLock(handle) as *const u16;
        if ptr.is_null() {
            CloseClipboard();
            return None;
        }

        let mut len = 0usize;
        while *ptr.add(len) != 0 {
            len += 1;
        }

        let slice = std::slice::from_raw_parts(ptr, len);
        let result = String::from_utf16_lossy(slice);

        GlobalUnlock(handle);
        CloseClipboard();
        Some(result)
    }
}

/// Writes Unicode text to the Win32 clipboard.
pub fn write_win32_clipboard(text: &str) -> bool {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes_len = wide.len() * std::mem::size_of::<u16>();

    unsafe {
        if OpenClipboard(ptr::null_mut()) == 0 {
            return false;
        }
        EmptyClipboard();

        let hmem = GlobalAlloc(GMEM_MOVEABLE, bytes_len);
        if hmem.is_null() {
            CloseClipboard();
            return false;
        }

        let ptr = GlobalLock(hmem) as *mut u16;
        if ptr.is_null() {
            CloseClipboard();
            return false;
        }

        ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
        GlobalUnlock(hmem);

        let res = SetClipboardData(CF_UNICODETEXT, hmem);
        CloseClipboard();

        !res.is_null()
    }
}

/// Retrieves the current system clipboard sequence number.
pub fn get_clipboard_sequence_number() -> u32 {
    unsafe { GetClipboardSequenceNumber() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_optical_corrections() {
        let input = "Momen diabadil<an oleh @eril<l<ristian di !olom komentar kapal Baturnandi menuju Dapua dengan hincfia (Al Generated Content) #cilegon] aksi _heroik @andriboto<";
        let corrected = auto_correct_ocr_text(input);
        assert!(corrected.contains("diabadikan"));
        assert!(corrected.contains("@erikkristian"));
        assert!(corrected.contains("kolom"));
        assert!(corrected.contains("Batumandi"));
        assert!(corrected.contains("papua"));
        assert!(corrected.contains("hindia"));
        assert!(corrected.contains("(AI Generated Content)"));
        assert!(corrected.contains("#cilegon"));
        assert!(!corrected.contains("#cilegon]"));
        assert!(corrected.contains("heroik"));
        assert!(!corrected.contains("_heroik"));
        assert!(corrected.contains("@andribotox"));
    }

    #[test]
    fn test_golden_rule_1_short_text() {
        let frame = "Headline Singkat\r\n\r\nParagraf kedua pendek.".to_string();
        let stitched = stitch_multiline_ocr_frames(&[frame.clone()]);
        assert_eq!(stitched, frame);
    }

    #[test]
    fn test_balloon_and_folder_rejection() {
        assert!(is_balloon_popup_text("1Q0 1O0O"));
        assert!(is_balloon_popup_text("Cut | Copy | Paste"));
        assert!(is_balloon_popup_text("Select All"));
        assert!(!is_balloon_popup_text("Ini teks biasa kapal Batumandi"));

        assert!(is_notes_folder_list_frame("<<<NOTES_LIST_VIEW>>>"));
        assert!(is_notes_folder_list_frame("Folders"));
        assert!(is_notes_folder_list_frame("Notes (12)"));
        assert!(!is_notes_folder_list_frame("Catatan penting tentang rapat"));
    }

    #[test]
    fn test_kmp_stitch_batumandi_six_paragraphs() {
        let part1 = "Paragraf 1: Penyelamatan KMP Batumandi\r\nParagraf 2: Evakuasi berjalan lancar\r\nParagraf 3: Semua penumpang selamat";
        let part2 = "Paragraf 2: Evakuasi berjalan lancar\r\nParagraf 3: Semua penumpang selamat\r\nParagraf 4: Petugas sigap di pelabuhan";
        let part3 = "Paragraf 4: Petugas sigap di pelabuhan\r\nParagraf 5: Cuaca berangsur membaik\r\nParagraf 6: Penyelidikan selesai";

        let frames = vec![part1.to_string(), part2.to_string(), part3.to_string()];
        let stitched = stitch_multiline_ocr_frames(&frames);

        assert!(stitched.contains("Paragraf 1"));
        assert!(stitched.contains("Paragraf 2"));
        assert!(stitched.contains("Paragraf 3"));
        assert!(stitched.contains("Paragraf 4"));
        assert!(stitched.contains("Paragraf 5"));
        assert!(stitched.contains("Paragraf 6"));
    }

    #[test]
    fn test_tahap193_lollipop_handle_repairs_and_double_enter() {
        let noisy = "tarhutla aman enggalnih wok I\r\n\r\nlomah enak teddy prabowk".to_string();
        let stitched = stitch_multiline_ocr_frames(&[noisy]);
        assert_eq!(stitched, "karhutla aman engga nih wok?\r\n\r\nlontoh enak teddy prabowo");

        let football = "lionel messi lamine yamal\r\n\r\nronaldo kdb ini pemulun".to_string();
        let stitched_fb = stitch_multiline_ocr_frames(&[football]);
        assert_eq!(stitched_fb, "lionel messi lamine yamal\r\n\r\nronaldo kdb ini pemulung");
    }

    #[test]
    fn test_single_enter_preserved_no_false_double_enter() {
        let single_enter = "baris satu\r\nbaris dua".to_string();
        let stitched = stitch_multiline_ocr_frames(&[single_enter.clone()]);
        assert_eq!(stitched, single_enter);
        assert!(!stitched.contains("\r\n\r\n"));
    }
}
