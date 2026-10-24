"""Verification script for Tahap 32:
1. TEST 1: Notes Folder List Frame Rejection & Top-of-Note Lock (DOC_START)
   - Simulates batch frames where Frame 0 is Notes List View ("Today\nbanten feeds iphone 17 pro\nRekaman..."),
     Frame 1 is Top-of-Note with date header ("24 October 2026 at 07.02\nRekaman..."),
     and Frame 2 is Bottom-of-Note.
   - Verifies "Today" and "banten feeds iphone 17 pro" are 100% eliminated,
     and output starts precisely at "Rekaman video amatir yang memperlihatkan".
   - Verifies session chunk isolation via clear_recent_long_ocr_chunks().

2. TEST 2: Strict Paragraph Formatter on Fake Double-Enter OCR Input (Video Minute 03:16)
   - Simulates 30+ line caption where EVERY SINGLE LINE is separated by \r\n\r\n (fake blank lines).
   - Verifies all lines within Para 1 (5 lines), Para 2 (11 lines), Para 3 (9 lines), Para 4,
     and Hashtags (2 lines) are joined by single \r\n (tight paragraphs).
   - Verifies exactly 5 \r\n\r\n paragraph separators yielding exactly 6 blocks.

3. TEST 3: Notes Folder List Header and Preview Detection Unit Tests (_is_notes_folder_list_frame)
   - Verifies detection of Today, Yesterday, Previous X Days, iCloud, Notes, X Notes,
     and preview timestamp lines (e.g. "07:02 Tidak ada teks").
   - Verifies normal text sentences containing "today" or "notes" are NOT falsely classified.

4. TEST 4: Inspection of ocr_extractor.ps1 for 0.85*avgH gap and Top-of-Note Lock.
"""

import re
import sys
from pathlib import Path

if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")

PROJECT_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(PROJECT_ROOT))

from handset.screen import (
    _clean_ocr_line,
    _is_garbage_ocr_line,
    _is_notes_folder_list_frame,
    auto_correct_ocr_text,
    clear_recent_long_ocr_chunks,
    ensure_paragraph_formatting,
    stitch_multiline_ocr_frames,
)
import handset.server as server_mod


def test_1_notes_folder_list_rejection_and_top_of_note_lock():
    print("\n" + "=" * 75)
    print("TEST 1: Notes Folder List Frame Rejection & Top-of-Note Lock (DOC_START)")
    print("=" * 75)

    clear_recent_long_ocr_chunks()

    # Frame 0: Contaminated frame recorded while Kelvin was on Notes Folder List view (02:11 - 02:26)
    frame_0_list_view = (
        "Today\n"
        "banten feeds iphone 17 pro\n"
        "Rekaman video amatir yang memperlihatkan"
    )

    # Frame 1: Top of Note view with iOS date header (02:26)
    frame_1_top_of_note = (
        "24 October 2026 at 07.02\n"
        "Rekaman video amatir yang memperlihatkan\n"
        "detik-detik seorang ayah menggendong erat\n"
        "buah hatinya saat menuruni tali evakuasi\n"
        "Kapal Motor Penumpang (KMP) Batumandi di\n"
        "perairan Selat Sunda viral di media sosial.\n"
        "Momen menegangkan tersebut diabadikan\n"
        "dan diunggah oleh akun Threads\n"
        "@erikkristian saat kebakaran melanda KMP\n"
        "Batumandi di dekat Pelabuhan Merak, Kota\n"
        "Cilegon, pada Selasa (20/10/2026). Dalam\n"
        "rekaman tersebut, tampak seorang ayah\n"
        "berjuang menuruni lambung kapal yang\n"
        "tinggi hanya dengan mengandalkan seutas\n"
        "tali penyelamat sambil mendekap anaknya,\n"
        "sebelum akhirnya disambut oleh petugas dan\n"
        "warga di atas rakit darurat (liferaft)."
    )

    # Frame 2: Bottom of Note view scrolled down
    frame_2_bottom_of_note = (
        "Situasi darurat di tengah kepulan asap tebal\n"
        "tersebut memaksa para penumpang\n"
        "mengambil langkah evakuasi berisiko tinggi\n"
        "demi menyelamatkan diri menuju sekoci dan\n"
        "perahu penolong yang bersiaga di bawah\n"
        "lambung kapal. Seluruh 428 penumpang\n"
        "serta 43 awak kapal dilaporkan dalam\n"
        "kondisi selamat ke daratan serta\n"
        "tanpa adanya korban jiwa.\n"
        "Bagaimana perasaan kalian melihat\n"
        "perjuangan seorang ayah di\n"
        "atas tali evakuasi ini,\n"
        "Banteners?\n"
        "Source: Threads @erikkristian\n"
        "#kmpbatumandi #kebakaran #selatsunda\n"
        "#pelabuhanmerak #selatsunda #cilegon"
    )

    # 1. Verify Frame 0 is recognized as Notes folder list view
    assert _is_notes_folder_list_frame(frame_0_list_view), "Frame 0 should be detected as Notes folder list frame!"
    assert not _is_notes_folder_list_frame(frame_1_top_of_note), "Frame 1 should NOT be detected as folder list frame!"
    assert not _is_notes_folder_list_frame(frame_2_bottom_of_note), "Frame 2 should NOT be detected as folder list frame!"
    print("[PASS] 1. Frame classification: Frame 0 is correctly identified as Notes list view.")

    # 2. Stitch batch containing [Frame 0, Frame 1, Frame 2]
    batch_frames = [frame_0_list_view, frame_1_top_of_note, frame_2_bottom_of_note]
    stitched = stitch_multiline_ocr_frames(batch_frames)
    corrected = auto_correct_ocr_text(stitched)
    final_output = ensure_paragraph_formatting(corrected)

    print("\n--- FINAL OUTPUT RESULT ---")
    print(final_output[:250] + "...\n[middle truncated]\n..." + final_output[-180:])
    print("---------------------------\n")

    # 3. Assertions for Bug 1
    assert "Today" not in final_output, "Contamination 'Today' still present in final output!"
    assert "banten feeds" not in final_output.lower(), "Contamination 'banten feeds iphone 17 pro' still present!"
    assert not final_output.startswith("Today"), "Output must not start with 'Today'!"
    assert final_output.startswith("Rekaman video amatir yang memperlihatkan"), (
        f"Output must start exactly with 'Rekaman video amatir yang memperlihatkan'! Found start: '{final_output[:50]}'"
    )
    print("[PASS] 2. Contamination 'Today' and 'banten feeds iphone 17 pro' 100% eliminated.")
    print("[PASS] 3. Output starts precisely at DOC_START ('Rekaman video amatir yang memperlihatkan').")

    # 4. Memory isolation: test subsequent copy after folder list visit
    # Simulate user visiting folder list again
    clear_recent_long_ocr_chunks()
    stitched_again = stitch_multiline_ocr_frames([frame_1_top_of_note, frame_2_bottom_of_note])
    final_again = ensure_paragraph_formatting(auto_correct_ocr_text(stitched_again))
    assert "banten feeds" not in final_again.lower()
    assert final_again.startswith("Rekaman video amatir yang memperlihatkan")
    print("[PASS] 4. Memory isolation verified: clear_recent_long_ocr_chunks ensures no cross-note memory leakage.")


def test_2_strict_paragraph_formatting_on_fake_double_enter_ocr():
    print("\n" + "=" * 75)
    print("TEST 2: Strict Paragraph Formatter on Fake Double-Enter OCR Input (Video Minute 03:16)")
    print("=" * 75)

    # In video minute 03:16, low OCR gap threshold (0.32) caused \r\n\r\n to be inserted
    # between ALMOST EVERY SINGLE LINE of the caption!
    raw_caption_lines = [
        # Para 1 (5 lines)
        "Rekaman video amatir yang memperlihatkan",
        "detik-detik seorang ayah menggendong erat",
        "buah hatinya saat menuruni tali evakuasi",
        "Kapal Motor Penumpang (KMP) Batumandi di",
        "perairan Selat Sunda viral di media sosial.",
        # Para 2 (11 lines)
        "Momen menegangkan tersebut diabadikan",
        "dan diunggah oleh akun Threads",
        "@erikkristian saat kebakaran melanda KMP",
        "Batumandi di dekat Pelabuhan Merak, Kota",
        "Cilegon, pada Selasa (20/10/2026). Dalam",
        "rekaman tersebut, tampak seorang ayah",
        "berjuang menuruni lambung kapal yang",
        "tinggi hanya dengan mengandalkan seutas",
        "tali penyelamat sambil mendekap anaknya,",
        "sebelum akhirnya disambut oleh petugas dan",
        "warga di atas rakit darurat (liferaft).",
        # Para 3 (9 lines)
        "Situasi darurat di tengah kepulan asap tebal",
        "tersebut memaksa para penumpang",
        "mengambil langkah evakuasi berisiko tinggi",
        "demi menyelamatkan diri menuju sekoci dan",
        "perahu penolong yang bersiaga di bawah",
        "lambung kapal. Seluruh 428 penumpang",
        "serta 43 awak kapal dilaporkan dalam",
        "kondisi selamat ke daratan serta",
        "tanpa adanya korban jiwa.",
        # Para 4 (4 lines)
        "Bagaimana perasaan kalian melihat",
        "perjuangan seorang ayah di",
        "atas tali evakuasi ini,",
        "Banteners?",
        # Source (1 line)
        "Source: Threads @erikkristian",
        # Hashtags (2 lines)
        "#kmpbatumandi #kebakaran #selatsunda",
        "#pelabuhanmerak #selatsunda #cilegon",
    ]

    # Malicious simulated input: EVERY SINGLE LINE has \r\n\r\n after it!
    fake_double_enter_ocr_text = "\r\n\r\n".join(raw_caption_lines)

    formatted = ensure_paragraph_formatting(fake_double_enter_ocr_text)

    paragraphs = formatted.split("\r\n\r\n")
    print(f"Total paragraph blocks produced: {len(paragraphs)}")
    for idx, p in enumerate(paragraphs):
        p_lines = p.splitlines()
        print(f"  Block {idx + 1} ({len(p_lines)} lines): '{p_lines[0][:35]}...' -> '...{p_lines[-1][-30:]}'")

    # Verify exactly 6 blocks separated by 5 double newlines
    assert len(paragraphs) == 6, f"Expected exactly 6 paragraph blocks, got {len(paragraphs)}!"

    # Verify lines within each block are joined by single \r\n
    assert len(paragraphs[0].splitlines()) == 5, f"Expected 5 lines in Para 1, got {len(paragraphs[0].splitlines())}"
    assert len(paragraphs[1].splitlines()) == 11, f"Expected 11 lines in Para 2, got {len(paragraphs[1].splitlines())}"
    assert len(paragraphs[2].splitlines()) == 9, f"Expected 9 lines in Para 3, got {len(paragraphs[2].splitlines())}"
    assert len(paragraphs[3].splitlines()) == 4, f"Expected 4 lines in Para 4, got {len(paragraphs[3].splitlines())}"
    assert len(paragraphs[4].splitlines()) == 1, f"Expected 1 line in Source, got {len(paragraphs[4].splitlines())}"
    assert len(paragraphs[5].splitlines()) == 2, f"Expected 2 lines in Hashtags, got {len(paragraphs[5].splitlines())}"

    # Verify no fake double newlines inside Para 1
    assert "memperlihatkan\r\n\r\ndetik-detik" not in formatted, "Fake double enter between lines 1 and 2 of Para 1!"
    assert "memperlihatkan\r\ndetik-detik" in formatted, "Para 1 lines 1-2 must be single \\r\\n!"

    # Verify no fake double newlines inside Para 2
    assert "menuruni lambung kapal yang\r\n\r\ntinggi hanya" not in formatted, "Fake double enter inside Para 2!"
    assert "menuruni lambung kapal yang\r\ntinggi hanya" in formatted, "Para 2 lines must be single \\r\\n!"

    # Verify hashtags are single newline
    assert "#kmpbatumandi #kebakaran #selatsunda\r\n#pelabuhanmerak" in formatted, "Hashtag lines must be single \\r\\n!"
    assert "#kmpbatumandi #kebakaran #selatsunda\r\n\r\n#pelabuhanmerak" not in formatted, "Hashtag lines must NOT have double \\r\\n!"

    print("[PASS] 1. All lines inside Para 1, 2, 3, 4 and Hashtags use single \\r\\n (tight spacing).")
    print("[PASS] 2. Exactly 5 \\r\\n\\r\\n paragraph breaks yielding exactly 6 blocks identical to iPhone Notes.")


def test_3_notes_folder_list_detection_unit_tests():
    print("\n" + "=" * 75)
    print("TEST 3: Notes Folder List Detection Unit Tests (_is_notes_folder_list_frame)")
    print("=" * 75)

    positive_cases = [
        ["Today", "banten feeds", "preview text"],
        ["Yesterday", "my note title"],
        ["Previous 7 Days", "older note"],
        ["Previous 30 Days", "last month note"],
        ["iCloud", "All iCloud notes"],
        ["Notes", "5 Notes"],
        ["12 Notes", "List of notes"],
        ["07:02 Tidak ada teks", "Note body"],
        ["14.30 No additional text", "Note body"],
        "Today\nbanten feeds iphone 17 pro\nRekaman...",
        "Yesterday\nCatatan belanja",
        "Catatan\n10 Catatan",
    ]

    for case in positive_cases:
        assert _is_notes_folder_list_frame(case), f"Failed to detect folder list for case: {case}"

    print(f"[PASS] 1. Correctly detected all {len(positive_cases)} Notes folder list header & preview cases.")

    negative_cases = [
        ["Rekaman video amatir yang memperlihatkan", "detik-detik seorang ayah"],
        ["Today was a great day for boating in Sunda Strait.", "The weather was clear."],
        ["Notes on project implementation:", "- Feature 1", "- Feature 2"],
        "Momen menegangkan tersebut diabadikan\ndan diunggah oleh akun Threads",
        "Situasi darurat di tengah kepulan asap tebal",
    ]

    for case in negative_cases:
        assert not _is_notes_folder_list_frame(case), f"False positive folder list detection for: {case}"

    print(f"[PASS] 2. Correctly rejected all {len(negative_cases)} normal text cases (no false positives).")


def test_4_ocr_extractor_script_inspection():
    print("\n" + "=" * 75)
    print("TEST 4: Inspection of handset/ocr_extractor.ps1 for 0.85*avgH gap & Top-of-Note Lock")
    print("=" * 75)

    ps1_path = PROJECT_ROOT / "handset" / "ocr_extractor.ps1"
    content = ps1_path.read_text(encoding="utf-8")

    assert "$gap -gt ($avgH * 0.85)" in content, "Threshold ($avgH * 0.85) not found in ocr_extractor.ps1!"
    assert "$avgH * 0.32" not in content, "Old low threshold 0.32 still present in ocr_extractor.ps1!"
    print("[PASS] 1. Line gap threshold successfully raised to ($avgH * 0.85).")

    assert "$dateMinY" in content, "Top-of-Note Lock variable $dateMinY not found in ocr_extractor.ps1!"
    assert "$parsedLines = @($parsedLines | Where-Object { $_.MinY -gt $dateMinY })" in content, (
        "Top-of-Note Lock line filter not found in ocr_extractor.ps1!"
    )
    print("[PASS] 2. Top-of-Note Lock discarding lines above date header found in ocr_extractor.ps1.")

    assert "$notesListHeaderRegex" in content, "$notesListHeaderRegex not found in ocr_extractor.ps1!"
    print("[PASS] 3. Notes list header regex filter present in ocr_extractor.ps1.")


if __name__ == "__main__":
    test_1_notes_folder_list_rejection_and_top_of_note_lock()
    test_2_strict_paragraph_formatting_on_fake_double_enter_ocr()
    test_3_notes_folder_list_detection_unit_tests()
    test_4_ocr_extractor_script_inspection()
    print("\n" + "=" * 75)
    print("ALL TAHAP 32 VERIFICATION TESTS PASSED 100%!")
    print("=" * 75)
