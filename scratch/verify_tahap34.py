#!/usr/bin/env python3
"""
Verification Script for Tahap 34:
- Test 1: Apple Notes Popup Balloon Occlusion ("tersebut memaksa para penumpang" vs "tersebut memaksaopng\n1Q0 1O0O")
          and Self-Consistent @Username Completion ("@eril<l<rist" -> "@erikkristian")
- Test 2: Google Keep Anti Cross-App Splicing (KMP Batumandi memory vs Tol Bekacau) & 5 clean paragraph blocks with \r\n\r\n
- Test 3: Short Sentence Enter Spacing Regression ("lionel messi argentina 17 pro\r\n\r\nlamine yamal neymar\r\n\r\nbarcelona")
- Test 4: ocr_extractor.ps1 C# pre-processor code inspection (Max-RB, blue lollipop, multi-cluster bounds)
"""

import sys
import re
from pathlib import Path

PROJECT_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(PROJECT_ROOT))

from handset.screen import (
    auto_correct_ocr_text,
    ensure_paragraph_formatting,
    stitch_multiline_ocr_frames,
    stitch_two_ocr_texts,
    merge_two_overlapping_blocks,
    clear_recent_long_ocr_chunks,
    _RECENT_LONG_OCR_CHUNKS,
    _CHUNKS_LOCK,
    _clean_ocr_line,
    _is_garbage_ocr_line,
    _score_gap_quality,
    _complete_usernames,
)


def test_1_apple_notes_popup_occlusion_and_username():
    print("=" * 70)
    print("TEST 1: Apple Notes Popup Occlusion & Username Completion (Min 02:39 - 02:50)")
    print("=" * 70)

    # Frame Atas: popup balloon covers "tersebut memaksa para penumpang", resulting in "tersebut memaksaopng" and "1Q0 1O0O"
    top_frame = (
        "Momen menegangkan detik-detik proses evakuasi penumpang KMP Batumandi yang kandas di perairan Merak terekam jelas.\r\n"
        "Dalam rekaman video yang diabadikan oleh @erikkristian, tampak seorang ayah menggendong buah hatinya saat menuruni tali evakuasi dengan penuh kehati-hatian menuju kapal nelayan dan tim SAR gabungan.\r\n"
        "tersebut memaksaopng\r\n"
        "1Q0 1O0O\r\n"
        "Situasi darurat ini terjadi di Perairan Merak, Banten pada Rabu malam saat kapal feri tersebut mengalami mati mesin di tengah cuaca buruk."
    )

    # Frame Bawah: scrolled down, clean view of "tersebut memaksa para penumpang" and source line with optical defect "@eril<l<rist"
    bottom_frame = (
        "Dalam rekaman video yang diabadikan oleh @erikkristian, tampak seorang ayah menggendong buah hatinya saat menuruni tali evakuasi dengan penuh kehati-hatian menuju kapal nelayan dan tim SAR gabungan.\r\n"
        "tersebut memaksa para penumpang\r\n"
        "menuruni kapal menggunakan tali evakuasi demi keselamatan jiwa mereka.\r\n"
        "Situasi darurat ini terjadi di Perairan Merak, Banten pada Rabu malam saat kapal feri tersebut mengalami mati mesin di tengah cuaca buruk.\r\n"
        "Bagaimana perasaan kalian jika berada di posisi para penumpang kapal ini, Banteners?\r\n"
        "Source: Threads @eril<l<rist\r\n"
        "#banten #kmpbatumandi #evakuasikapal #pelabuhanmerak"
    )

    # Test individual functions
    assert _is_garbage_ocr_line("1Q0 1O0O"), "Line '1Q0 1O0O' MUST be recognized as garbage!"
    assert _clean_ocr_line("tersebut memaksaopng") == "tersebut memaksa para penumpang", "memaksaopng must be corrected!"

    # Gap scoring
    gap_dirty = ["tersebut memaksaopng", "1Q0 1O0O"]
    gap_clean = ["tersebut memaksa para penumpang"]
    score_dirty = _score_gap_quality(gap_dirty)
    score_clean = _score_gap_quality(gap_clean)
    print(f"Gap dirty score: {score_dirty}, Gap clean score: {score_clean}")
    assert score_clean > score_dirty, f"Clean gap score ({score_clean}) MUST be higher than dirty gap score ({score_dirty})!"

    # Merge frames
    merged_lines = merge_two_overlapping_blocks(top_frame.splitlines(), bottom_frame.splitlines())
    assert merged_lines is not None, "merge_two_overlapping_blocks returned None for valid overlapping frames!"
    merged_text = "\r\n".join(merged_lines)

    assert "1Q0 1O0O" not in merged_text, "Garbage '1Q0 1O0O' must NOT be in merged output!"
    assert "memaksaopng" not in merged_text, "'memaksaopng' must NOT be in merged output!"
    assert "tersebut memaksa para penumpang" in merged_text, "Clean sentence 'tersebut memaksa para penumpang' must be preserved!"

    # Full stitch & paragraph formatting
    clear_recent_long_ocr_chunks()
    stitched = stitch_multiline_ocr_frames([top_frame, bottom_frame])
    print(f"\nStitched Result:\n{stitched}\n")

    assert "tersebut memaksa para penumpang" in stitched, "Clean sentence must be in final output!"
    assert "1Q0 1O0O" not in stitched, "1Q0 1O0O must not be in final output!"
    assert "Source: Threads @erikkristian" in stitched, f"Expected 'Source: Threads @erikkristian', got: {stitched}"
    print("[PASS] Test 1: Apple Notes popup occlusion correctly resolved to clean lines, and username completed to @erikkristian!")


def test_2_google_keep_anti_cross_app_splicing():
    print("\n" + "=" * 70)
    print("TEST 2: Google Keep Anti Cross-App Splicing & Paragraph Formatting (Min 02:58 - 03:39)")
    print("=" * 70)

    # 1. Populate chunk memory with Apple Notes KMP Batumandi text
    clear_recent_long_ocr_chunks()
    import time
    with _CHUNKS_LOCK:
        _RECENT_LONG_OCR_CHUNKS.append({
            "ts": time.time(),
            "lines": [
                "Momen menegangkan detik-detik proses evakuasi penumpang KMP Batumandi yang kandas di perairan Merak terekam jelas.",
                "Dalam rekaman video yang diabadikan oleh @erikkristian, tampak seorang ayah menggendong buah hatinya saat menuruni tali evakuasi dengan penuh kehati-hatian menuju kapal nelayan dan tim SAR gabungan.",
                "tersebut memaksa para penumpang",
                "menuruni kapal menggunakan tali evakuasi demi keselamatan jiwa mereka.",
                "Situasi darurat ini terjadi di Perairan Merak, Banten pada Rabu malam saat kapal feri tersebut mengalami mati mesin di tengah cuaca buruk.",
                "Bagaimana perasaan kalian jika berada di posisi para penumpang kapal ini, Banteners?",
                "Source: Threads @erikkristian",
                "#banten #kmpbatumandi #evakuasikapal #pelabuhanmerak"
            ]
        })

    # 2. Google Keep Tol Bekacau text (simulating OCR output with blank lines between paragraphs)
    keep_frame = (
        "Unggahan bertajuk \"Fenomena 'Tol Berbayar Jadi Pasar Malam' di Jalur Bekasi\" mendadak viral di berbagai platform media sosial setelah memperlihatkan kondisi tidak biasa di ruas tol tersebut.\r\n"
        "\r\n"
        "Rekaman singkat memperlihatkan deretan pedagang kaki lima dan warga yang asyik berkumpul di tepi jalur tol yang seharusnya steril dari aktivitas warga sekitar demi keamanan berlalu lintas.\r\n"
        "\r\n"
        "Bagaimana tanggapan kalian mengenai potret viral suasana malam di ruas Tol Bekacau ini, Banteners?\r\n"
        "\r\n"
        "Source: @kangmas.id\r\n"
        "\r\n"
        "#tolbekacau #fenomenatol #truklintasjawa #viralbekasi #infotangerang"
    )

    # Test stitch_multiline_ocr_frames with previous Apple Notes frame in history + Keep frame
    hist_apple_frame = (
        "Momen menegangkan detik-detik proses evakuasi penumpang KMP Batumandi yang kandas di perairan Merak terekam jelas.\r\n"
        "Bagaimana perasaan kalian jika berada di posisi para penumpang kapal ini, Banteners?\r\n"
        "Source: Threads @erikkristian\r\n"
        "#banten #kmpbatumandi"
    )

    stitched_keep = stitch_multiline_ocr_frames([hist_apple_frame, keep_frame])
    print(f"\nGoogle Keep Stitched Result:\n{stitched_keep}\n")

    # Assertions:
    # 1. Zero cross-app splicing from KMP Batumandi
    assert "KMP Batumandi" not in stitched_keep, "CRITICAL ERROR: KMP Batumandi from Apple Notes was spliced into Google Keep!"
    assert "Perairan Merak" not in stitched_keep, "CRITICAL ERROR: Apple Notes text found in Google Keep!"
    assert "evakuasi" not in stitched_keep, "CRITICAL ERROR: Apple Notes evakuasi found in Google Keep!"

    # 2. Paragraph 1 must be present
    assert stitched_keep.startswith("Unggahan bertajuk"), f"Paragraph 1 missing! Starts with: {stitched_keep[:60]}"

    # 3. Exactly 5 clean blocks separated by \r\n\r\n
    blocks = [b.strip() for b in re.split(r'\r?\n\r?\n', stitched_keep) if b.strip()]
    print(f"Detected {len(blocks)} blocks:")
    for b_idx, b in enumerate(blocks):
        print(f" Block {b_idx + 1}: {repr(b[:40])}...")

    assert len(blocks) == 5, f"Expected exactly 5 paragraph blocks, got {len(blocks)}!"
    assert blocks[0].startswith("Unggahan bertajuk")
    assert blocks[1].startswith("Rekaman singkat")
    assert blocks[2].startswith("Bagaimana tanggapan")
    assert blocks[3].startswith("Source: @kangmas.id")
    assert blocks[4].startswith("#tolbekacau")

    # 4. Check hashtags
    assert "#truklintasjawa" in stitched_keep, "Hashtag #truklintasjawa must be intact!"
    assert "#infotangerang" in stitched_keep, "Hashtag #infotangerang must be intact!"

    print("[PASS] Test 2: Google Keep 100% free of Apple Notes contamination, and formatted with 5 clean paragraph blocks!")


def test_3_short_text_enter_spacing_regression():
    print("\n" + "=" * 70)
    print("TEST 3: Short Sentence Enter Spacing Regression")
    print("=" * 70)

    short_text = "lionel messi argentina 17 pro\r\n\r\nlamine yamal neymar\r\n\r\nbarcelona"
    corrected = auto_correct_ocr_text(short_text)
    final = ensure_paragraph_formatting(corrected)

    print(f"Short text result:\n{final}\n")
    blocks = [b.strip() for b in re.split(r'\r?\n\r?\n', final) if b.strip()]
    expected_blocks = ["lionel messi argentina 17 pro", "lamine yamal neymar", "barcelona"]

    assert blocks == expected_blocks, f"Expected {expected_blocks}, got {blocks}"
    assert "\r\n\r\n" in final, "Double Enter lost!"
    print("[PASS] Test 3: Short sentence with double Enter remains 100% stable!")


def test_4_ocr_extractor_code_inspection():
    print("\n" + "=" * 70)
    print("TEST 4: ocr_extractor.ps1 Pre-Processor Code Inspection")
    print("=" * 70)

    ps1_path = PROJECT_ROOT / "handset" / "ocr_extractor.ps1"
    content = ps1_path.read_text(encoding="utf-8")

    # Check Max-RB channel
    assert "int maxRB = Math.Max((int)r, (int)b);" in content, "Max-RB calculation missing in ocr_extractor.ps1!"
    print("[PASS] 1. Max-RB channel pixel pre-processing is present.")

    # Check Blue selection detection
    assert "bool isBlueLollipop = (b > 165) && ((b - r) > 45);" in content, "Blue lollipop detection missing!"
    assert "bool isBlueBg = (b > 205) && (g > 190) && ((b - r) >= 8);" in content, "Blue background detection missing!"
    print("[PASS] 2. Google Keep / iOS blue lollipop & background detection is present.")

    # Check selection bounds pass-through
    assert "int selMinY = 0, int selMaxY = 0" in content, "selMinY/selMaxY parameters missing in VerifyLineHighlight!"
    assert "oy >= (selMinY - 10) && (oy + oh) <= (selMaxY + 20)" in content, "Selection bounds guard missing in VerifyLineHighlight!"
    print("[PASS] 3. Multi-cluster selection bounds protection in VerifyLineHighlight is present.")


if __name__ == "__main__":
    test_1_apple_notes_popup_occlusion_and_username()
    test_2_google_keep_anti_cross_app_splicing()
    test_3_short_text_enter_spacing_regression()
    test_4_ocr_extractor_code_inspection()
    print("\n" + "=" * 70)
    print("ALL TAHAP 34 VERIFICATION TESTS PASSED SUCCESSFULLY! (100% GREEN)")
    print("=" * 70)
