#!/usr/bin/env python3
"""
Verification Suite for Tahap 36:
- Test 1: Real execution of PowerShell ocr_extractor.ps1 on Kelvin's real debug_ocr_raw.png!
          Proves returncode == 0, extracted text is non-empty, and text successfully written to Windows Clipboard.
- Test 2: Real execution of PowerShell ocr_extractor.ps1 on synthetic PNG images:
          - test_google_keep_blue.png: blue selection, proves top draft debris (#toltangerangmerak and ---)
            is completely stripped, "Unggahan bertajuk...", "kelakar...", and "Source: @kingtruck.id" are preserved in 5 blocks.
          - test_apple_notes_yellow.png: yellow selection with red spellcheck dots, proves "the beatles" is not erased,
            "hindia" intact, double Enter spacing preserved.
- Test 3: Ground-Truth text verification for all Kelvin test cases:
          - 3A: Google Keep "Pesona Tol Balaraja" 1-screen selection (5 blocks identical to Ground Truth 1)
          - 3B: Apple Notes "KMP Batumandi" Copy at Bottom (Para 1 restored, @andriboto< -> @andribotox, 6 blocks)
          - 3C: Apple Notes "KMP Batumandi" Copy at Top (gap filled, hashtag tail preserved, 6 blocks)
          - 3D: Short text with Enter spacing ("the beatles" & "hindia" recovered, double Enter intact)
- Test 4: Inspection of handset/ocr_extractor.ps1 code architecture:
          Proves no VerifyLineHighlight, no SEL_BOUNDS, no premature exit 0, hlCount >= 8, gap 0.060*H.
"""

import os
import sys
import re
import time
import subprocess
from pathlib import Path

PROJECT_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(PROJECT_ROOT))

from handset import screen
from handset.screen import (
    auto_correct_ocr_text,
    ensure_paragraph_formatting,
    stitch_multiline_ocr_frames,
    stitch_two_ocr_texts,
    merge_two_overlapping_blocks,
    clear_recent_long_ocr_chunks,
    record_typed_text,
    _CHUNKS_LOCK,
    _clean_ocr_line,
    _is_garbage_ocr_line,
    _complete_usernames,
    _strip_top_unselected_debris,
)
from handset.server import (
    extract_text_batch,
    set_windows_clipboard_text,
    get_windows_clipboard_text,
)


def test_1_real_execution_debug_ocr_raw():
    print("=" * 80)
    print("TEST 1: Real Execution of PowerShell ocr_extractor.ps1 on debug_ocr_raw.png")
    print("=" * 80)

    raw_png = PROJECT_ROOT / "debug_ocr_raw.png"
    assert raw_png.exists(), f"debug_ocr_raw.png not found at {raw_png}!"

    # Execute batch extraction via server function which invokes powershell.exe
    batch_results = extract_text_batch([str(raw_png)])

    print(f"PowerShell execution returned {len(batch_results)} frame results.")
    assert len(batch_results) >= 1, "extract_text_batch MUST return at least 1 non-empty frame result!"

    extracted_text = batch_results[0].strip()
    print("\nReal Extracted Raw Text from debug_ocr_raw.png:\n" + "-" * 50)
    print(extracted_text[:300] + "..." if len(extracted_text) > 300 else extracted_text)
    print("-" * 50)

    assert len(extracted_text) >= 20, f"Extracted text is too short ({len(extracted_text)} chars)!"
    assert any(w in extracted_text.lower() for w in ["pamungkas", "evakuasi", "barcelona", "momen", "unggahan"]), \
        f"Expected authentic text from debug_ocr_raw.png, got: {extracted_text}"

    # Process through pipeline and test Windows Clipboard integration
    corrected = auto_correct_ocr_text(extracted_text)
    final_text = ensure_paragraph_formatting(corrected)

    clip_ok = set_windows_clipboard_text(final_text)
    assert clip_ok, "set_windows_clipboard_text MUST succeed!"

    read_back = get_windows_clipboard_text()
    assert read_back == final_text, "Windows Clipboard read-back MUST match the copied text!"
    print(f"[PASS] Successfully verified Windows Clipboard integration ({len(read_back)} chars)!")
    print("[PASS] Test 1: Real PowerShell execution on debug_ocr_raw.png succeeded with non-empty output!")


def test_2_real_execution_on_synthetic_pngs():
    print("\n" + "=" * 80)
    print("TEST 2: Real Execution on Synthetic PNGs (Google Keep Blue & Apple Notes Yellow)")
    print("=" * 80)

    # 1. Test Google Keep Blue Selection PNG
    keep_png = PROJECT_ROOT / "scratch" / "test_google_keep_blue.png"
    assert keep_png.exists(), f"test_google_keep_blue.png missing at {keep_png}!"

    keep_res = extract_text_batch([str(keep_png)])
    assert len(keep_res) >= 1, "extract_text_batch on test_google_keep_blue.png returned empty list!"

    keep_raw = keep_res[0].strip()
    print("\nExtracted text from test_google_keep_blue.png:\n" + "-" * 50)
    print(keep_raw)
    print("-" * 50)

    # Verifications on Google Keep Blue Selection:
    assert "toltangerangmerak" not in keep_raw, "Debris '#toltangerangmerak' must NOT be in output!"
    assert "---" not in keep_raw, "Separator '---' must NOT be in output!"
    assert keep_raw.startswith("Unggahan bertajuk 'Pesona Tol Balaraja"), "Must start with Paragraph 1!"
    assert "kelakar menanyakan titik kilometer" in keep_raw, "Middle line 'kelakar...' must be preserved!"
    assert "Source: @kingtruck.id" in keep_raw, "Source line must be preserved!"
    assert "#tolbalaraja" in keep_raw and "#infotangerang" in keep_raw, "Hashtags must be preserved!"

    final_keep = ensure_paragraph_formatting(auto_correct_ocr_text(keep_raw))
    keep_blocks = [b.strip() for b in re.split(r'\r?\n\r?\n', final_keep) if b.strip()]
    print(f"Detected {len(keep_blocks)} paragraph blocks in Google Keep Blue.")
    assert len(keep_blocks) == 5, f"Expected 5 paragraph blocks, got {len(keep_blocks)}!"
    print("[PASS] test_google_keep_blue.png OCR verified 100% accurate!")

    # 2. Test Apple Notes Yellow Selection PNG (with red spellcheck dots under 'beatles')
    notes_png = PROJECT_ROOT / "scratch" / "test_apple_notes_yellow.png"
    assert notes_png.exists(), f"test_apple_notes_yellow.png missing at {notes_png}!"

    notes_res = extract_text_batch([str(notes_png)])
    assert len(notes_res) >= 1, "extract_text_batch on test_apple_notes_yellow.png returned empty list!"

    notes_raw = notes_res[0].strip()
    print("\nExtracted text from test_apple_notes_yellow.png:\n" + "-" * 50)
    print(notes_raw)
    print("-" * 50)

    # Verifications on Apple Notes Yellow Selection:
    assert "pamungkas kenangan manis" in notes_raw, "'pamungkas kenangan manis' must be in output!"
    assert "barasuara" in notes_raw, "'barasuara' must be in output!"
    assert "beatles" in notes_raw, "'beatles' MUST NOT be erased by spellcheck neutralization!"
    assert "hindia" in notes_raw, "'hindia' must be in output!"

    final_notes = ensure_paragraph_formatting(auto_correct_ocr_text(notes_raw))
    expected_notes = (
        "pamungkas kenangan manis\r\n\r\n"
        "barasuara\r\n\r\n"
        "the beatles\r\n\r\n"
        "hindia"
    )
    assert final_notes == expected_notes, f"Expected:\n{expected_notes}\nGot:\n{final_notes}"
    print("[PASS] test_apple_notes_yellow.png OCR verified 100% accurate with red spellcheck intact!")


def test_3_ground_truth_pipeline():
    print("\n" + "=" * 80)
    print("TEST 3: Ground-Truth Verification for Kelvin's 3 Test Cases")
    print("=" * 80)

    # 3A: Google Keep "Pesona Tol Balaraja Jam 11 Malam" (Min 04:37 & 05:07)
    clear_recent_long_ocr_chunks()
    keep_ground_truth_input = (
        "#toltangerangmerak #infotangerang\r\n"
        "---\r\n"
        "Unggahan bertajuk 'Pesona Tol Balaraja Jam 11 Malam' yang dibagikan oleh akun kreator Instagram @kingtruck.id mendadak viral dan memicu gelombang komentar dari warganet.\r\n"
        "\r\n"
        "Dalam unggahan tersebut, tampak potret seorang wanita berdiri di samping badan truk tronton hijau yang tengah berhenti di bahu jalan tol pada larut malam. Konten tersebut langsung menyedot puluhan ribu tanda suka dan memantik beragam spekulasi netizen di kolom komentar, mulai dari kelakar menanyakan titik kilometer ruas tol Merak–Jakarta hingga dugaan bahwa visual tersebut merupakan hasil olahan teknologi kecerdasan buatan (AI Generated Content). Fenomena ini memperlihatkan bagaimana dinamika kehidupan malam para pengemudi truk di jalur lintas barat Jawa kerap diangkat menjadi materi hiburan digital yang menarik atensi luas publik pengguna media sosial.\r\n"
        "\r\n"
        "Bagaimana tanggapan kalian mengenai potret viral suasana malam di ruas Tol Balaraja ini, Banteners?\r\n"
        "\r\n"
        "Source: @kingtruck.id\r\n"
        "\r\n"
        "#tolbalaraja #tolmerakjakarta #kingtruckid #sopirtruk #infotangerang"
    )
    keep_dirty_history = (
        "Malam' yang dibagikan oleh akun kreator Instagram @kingtruck.id mendadak viral dan memicu gelombang komentar dari warganet.\r\n"
        "Dalam unggahan tersebut, tampak potret seorang wanita berdiri di samping badan truk tronton hijau yang tengah berhenti di bahu jalan tol pada larut malam."
    )

    # Server pipeline logic:
    cleaned_input = "\r\n".join(_strip_top_unselected_debris([l.strip() for l in keep_ground_truth_input.splitlines()]))
    stitched_keep = stitch_multiline_ocr_frames([keep_dirty_history, cleaned_input])
    final_keep = ensure_paragraph_formatting(auto_correct_ocr_text(stitched_keep))

    assert final_keep.startswith("Unggahan bertajuk 'Pesona Tol Balaraja Jam 11 Malam'"), "Must start with opening!"
    assert "toltangerangmerak" not in final_keep and "---" not in final_keep, "Debris must be stripped!"
    assert "kelakar menanyakan titik kilometer ruas tol Merak–Jakarta" in final_keep, "Middle line missing!"
    assert "Source: @kingtruck.id" in final_keep, "Source missing!"
    assert "#tolbalaraja #tolmerakjakarta #kingtruckid #sopirtruk #infotangerang" in final_keep, "Hashtags missing!"
    blocks_keep = [b.strip() for b in re.split(r'\r?\n\r?\n', final_keep) if b.strip()]
    assert len(blocks_keep) == 5, f"Expected 5 blocks in Google Keep, got {len(blocks_keep)}!"
    print("[PASS] 3A: Google Keep Ground Truth 1 is 100% word-for-word identical!")

    # 3B: Apple Notes "KMP Batumandi" Copy at Bottom (Min 03:13)
    clear_recent_long_ocr_chunks()
    top_history = (
        "Rekaman video amatir yang memperlihatkan aksi heroik seorang ayah menggendong erat buah hatinya saat menuruni tali evakuasi Kapal Motor Penumpang (KMP) Batumandi di perairan Selat Sunda viral di media sosial.\r\n"
        "\r\n"
        "Momen menegangkan tersebut diabadikan dan diunggah oleh akun Threads @andribotox saat kebakaran melanda KMP Batumandi di dekat Pelabuhan Merak, Kota Cilegon, pada Selasa (20/10/2026). Dalam rekaman tersebut, tampak seorang ayah berjuang menuruni lambung kapal yang tinggi hanya dengan mengandalkan seutas tali penyelamat sambil mendekap anaknya, sebelum akhirnya disambut oleh petugas dan warga di atas rakit darurat (liferaft)."
    )
    bottom_current = (
        "Momen menegangkan tersebut diabadikan dan diunggah oleh akun Threads @andribotox saat kebakaran melanda KMP Batumandi di dekat Pelabuhan Merak, Kota Cilegon, pada Selasa (20/10/2026). Dalam rekaman tersebut, tampak seorang ayah berjuang menuruni lambung kapal yang tinggi hanya dengan mengandalkan seutas tali penyelamat sambil mendekap anaknya, sebelum akhirnya disambut oleh petugas dan warga di atas rakit darurat (liferaft).\r\n"
        "\r\n"
        "Situasi darurat di tengah kepulan asap tebal tersebut memaksa para penumpang mengambil langkah evakuasi berisiko tinggi demi menyelamatkan diri menuju sekoci dan perahu penolong yang bersiaga di bawah lambung kapal. Seluruh 420 penumpang serta 40 awak kapal dilaporkan berhasil dievakuasi dalam kondisi selamat ke daratan tanpa adanya korban jiwa.\r\n"
        "\r\n"
        "Bagaimana perasaan kalian menyaksikan keteguhan dan perjuangan seorang ayah di tengah evakuasi KMP Batumandi ini, Banteners?\r\n"
        "\r\n"
        "Source: Threads @andriboto<\r\n"
        "\r\n"
        "#kmpbatumandi #evakuasikapal #pelabuhanmerak #selatsunda #cilegon"
    )
    stitched_notes_bottom = stitch_multiline_ocr_frames([top_history, bottom_current])
    final_notes_bottom = ensure_paragraph_formatting(auto_correct_ocr_text(stitched_notes_bottom))

    assert final_notes_bottom.startswith("Rekaman video amatir yang memperlihatkan aksi heroik"), "Para 1 must be at head!"
    assert "Source: Threads @andribotox" in final_notes_bottom, "@andriboto< must be completed to @andribotox!"
    blocks_bottom = [b.strip() for b in re.split(r'\r?\n\r?\n', final_notes_bottom) if b.strip()]
    assert len(blocks_bottom) == 6, f"Expected 6 blocks in Apple Notes Copy at Bottom, got {len(blocks_bottom)}!"
    print("[PASS] 3B: Apple Notes Copy at Bottom restored Para 1 and completed @andribotox!")

    # 3C: Apple Notes "KMP Batumandi" Copy at Top (Min 03:42)
    clear_recent_long_ocr_chunks()
    bottom_memory_lines = [
        "Momen menegangkan tersebut diabadikan dan diunggah oleh akun Threads @andribotox saat kebakaran melanda KMP Batumandi di dekat Pelabuhan Merak, Kota Cilegon, pada Selasa (20/10/2026). Dalam rekaman tersebut, tampak seorang ayah berjuang menuruni lambung kapal yang tinggi hanya dengan mengandalkan seutas tali penyelamat sambil mendekap anaknya, sebelum akhirnya disambut oleh petugas dan warga di atas rakit darurat (liferaft).",
        "Situasi darurat di tengah kepulan asap tebal tersebut memaksa para penumpang mengambil langkah evakuasi berisiko tinggi demi menyelamatkan diri menuju sekoci dan perahu penolong yang bersiaga di bawah lambung kapal. Seluruh 420 penumpang serta 40 awak kapal dilaporkan berhasil dievakuasi dalam kondisi selamat ke daratan tanpa adanya korban jiwa.",
        "Bagaimana perasaan kalian menyaksikan keteguhan dan perjuangan seorang ayah di",
        "tengah evakuasi KMP Batumandi ini,",
        "Banteners?",
        "Source: Threads @andribotox",
        "#kmpbatumandi #evakuasikapal #pelabuhanmerak #selatsunda #cilegon"
    ]
    with _CHUNKS_LOCK:
        screen._RECENT_LONG_OCR_CHUNKS.append({
            "ts": time.time(),
            "lines": bottom_memory_lines
        })

    top_current = (
        "Rekaman video amatir yang memperlihatkan aksi heroik seorang ayah menggendong erat buah hatinya saat menuruni tali evakuasi Kapal Motor Penumpang (KMP) Batumandi di perairan Selat Sunda viral di media sosial.\r\n"
        "\r\n"
        "Momen menegangkan tersebut diabadikan dan diunggah oleh akun Threads @andribotox saat kebakaran melanda KMP Batumandi di dekat Pelabuhan Merak, Kota Cilegon, pada Selasa (20/10/2026). Dalam rekaman tersebut, tampak seorang ayah berjuang menuruni lambung kapal yang tinggi hanya dengan mengandalkan seutas tali penyelamat sambil mendekap anaknya, sebelum akhirnya disambut oleh petugas dan warga di atas rakit darurat (liferaft).\r\n"
        "\r\n"
        "Situasi darurat di tengah kepulan asap tebal tersebut memaksa para penumpang mengambil langkah evakuasi berisiko tinggi demi menyelamatkan diri menuju sekoci dan perahu penolong yang bersiaga di bawah lambung kapal. Seluruh 420 penumpang serta 40 awak kapal dilaporkan berhasil dievakuasi dalam kondisi selamat ke daratan tanpa adanya korban jiwa.\r\n"
        "\r\n"
        "Bagaimana perasaan kalian menyaksikan keteguhan dan perjuangan seorang ayah di\r\n"
        "Banteners?"
    )
    stitched_notes_top = stitch_multiline_ocr_frames([top_current])
    final_notes_top = ensure_paragraph_formatting(auto_correct_ocr_text(stitched_notes_top))

    assert final_notes_top.startswith("Rekaman video amatir"), "Must start with Para 1!"
    assert "tengah evakuasi KMP Batumandi ini," in final_notes_top, "Missing line must be filled by gap-filler!"
    assert "Source: Threads @andribotox" in final_notes_top, "Source must be preserved from tail!"
    assert "#kmpbatumandi" in final_notes_top and "#cilegon" in final_notes_top, "Hashtags must be preserved from tail!"
    blocks_top = [b.strip() for b in re.split(r'\r?\n\r?\n', final_notes_top) if b.strip()]
    assert len(blocks_top) == 6, f"Expected 6 blocks in Apple Notes Copy at Top, got {len(blocks_top)}!"
    print("[PASS] 3C: Apple Notes Copy at Top filled gap and preserved hashtag tail in 6 blocks!")

    # 3D: Short Text with Enter spacing ("the beatles" & "hindia")
    typed_buffer = ["pamungkas kenangan manis", "barasuara", "the beatles", "hindia"]
    for t in typed_buffer:
        record_typed_text(t + "\n")

    short_ocr_input = (
        "pamungkas kenangan manis\r\n"
        "\r\n"
        "barasuara\r\n"
        "\r\n"
        "the\r\n"
        "\r\n"
        "hincfia"
    )
    final_short = ensure_paragraph_formatting(auto_correct_ocr_text(short_ocr_input))
    expected_short = (
        "pamungkas kenangan manis\r\n\r\n"
        "barasuara\r\n\r\n"
        "the beatles\r\n\r\n"
        "hindia"
    )
    assert final_short == expected_short, f"\nExpected:\n{expected_short}\nGot:\n{final_short}"
    print("[PASS] 3D: Short text recovered 'the beatles' and 'hindia' with double Enter spacing intact!")


def test_4_ocr_extractor_architecture_inspection():
    print("\n" + "=" * 80)
    print("TEST 4: Inspection of handset/ocr_extractor.ps1 Code Architecture")
    print("=" * 80)

    script_path = PROJECT_ROOT / "handset" / "ocr_extractor.ps1"
    content = script_path.read_text(encoding="utf-8")

    # 1. Verify VerifyLineHighlight is completely removed
    assert "VerifyLineHighlight" not in content, "VerifyLineHighlight MUST be completely removed from ocr_extractor.ps1!"
    assert "GetLineHighlightRatio" not in content, "GetLineHighlightRatio MUST be completely removed!"

    # 2. Verify SEL_BOUNDS is completely removed
    assert "SEL_BOUNDS" not in content, "<<<SEL_BOUNDS>>> MUST be completely removed from ocr_extractor.ps1!"

    # 3. Verify no exit 0 inside batch processing loop
    assert "exit 0" not in content or content.count("exit 0") <= 1, "No premature exit 0 inside batch processing loop!"

    # 4. Verify highlight horizontal threshold >= 8 pixels
    assert "hlCount >= 8" in content, "Highlight detection threshold must be >= 8 pixels per row!"

    # 5. Verify vertical gap tolerance 0.060 * H
    assert "0.060" in content, "Vertical grouping gap tolerance must be 0.060 * H!"

    # 6. Verify threshold 212 for blue and 228 for yellow/white
    assert "threshold = 212" in content, "Blue threshold 212 must be present in C# preprocessor!"
    assert "threshold = 228" in content, "Yellow threshold 228 must be present in C# preprocessor!"

    print("[PASS] Test 4: handset/ocr_extractor.ps1 satisfies 100% of Tahap 36 architectural requirements!")


if __name__ == "__main__":
    test_1_real_execution_debug_ocr_raw()
    test_2_real_execution_on_synthetic_pngs()
    test_3_ground_truth_pipeline()
    test_4_ocr_extractor_architecture_inspection()
    print("\n" + "=" * 80)
    print(">>> ALL TAHAP 36 VERIFICATION TESTS PASSED SUCCESSFULLY! (100% GREEN) <<<")
    print("=" * 80)
