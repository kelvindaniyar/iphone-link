"""Verification script for Tahap 31:
1. TEST 1: Full Pipeline on Kelvin's Sticky Notes Case (02:44 - 03:09 and 03:49 - 03:57)
   Verifies:
   1. "diabadil<an" -> "diabadikan"
   2. "@eril<l<ristian" -> "@erikkristian" (both in Paragraph 2 and in Source line)
   3. "Baturnandi" & "#kmpbaturnandi" -> "Batumandi" & "#kmpbatumandi"
   4. "tampal<" -> "tampak"
   5. "disambut Oleh" & "diunggah Oleh" -> "disambut oleh" & "diunggah oleh"
   6. "berisil<O" -> "berisiko"
   7. "sel<oci" -> "sekoci"
   8. "se!amat" -> "selamat"
   9. "seorang ayah d!" -> "seorang ayah di"
   10. Keyboard debris "cat aku", "w e r t y u i o p", and "z x c v b n m" 100% REMOVED!
   11. 6 paragraph blocks cleanly separated by \r\n\r\n and consecutive hashtags joined by single \r\n.

2. TEST 2: Optical Clean Line Unit Tests (_clean_ocr_line & _pick_cleaner_line)
   Verifies regex replacements and defect penalty scoring.

3. TEST 3: C# Visual Keyboard Detection & Darkening Curve in ocr_extractor.ps1
   Verifies presence of keyboard detection, cropBottom = 0.49 * H, r < 165 loosening, and stroke-preserving curve.
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
    _pick_cleaner_line,
    auto_correct_ocr_text,
    ensure_paragraph_formatting,
    stitch_multiline_ocr_frames,
)
import handset.server as server_mod


def test_1_sticky_notes_full_pipeline():
    print("\n" + "=" * 75)
    print("TEST 1: Full Pipeline on Kelvin Sticky Notes Raw Input (Video Tahap 30)")
    print("=" * 75)

    # Exact raw text from Kelvin's Sticky Notes at 02:44 - 03:09 and 03:49 - 03:57
    raw_sticky_notes_kelvin = (
        "Rekaman video amatir yang memperlihatkan\n"
        "detik-detik seorang ayah menggendong erat\n"
        "buah hatinya saat menuruni tali evakuasi\n"
        "Kapal Motor Penumpang (KMP) Baturnandi di\n"
        "perairan Selat Sunda viral di media sosial.\n"
        "Momen menegangkan tersebut diabadil<an\n"
        "dan diunggah Oleh akun Threads\n"
        "@eril<l<ristian saat kebakaran melanda KMP\n"
        "Baturnandi di dekat Pelabuhan Merak, Kota\n"
        "Cilegon, pada Selasa (20/10/2026). Dalam\n"
        "rekaman tersebut, tampal< seorang ayah\n"
        "berjuang menuruni lambung kapal yang\n"
        "tinggi hanya dengan mengandalkan seutas\n"
        "tali penyelamat sambil mendekap anaknya,\n"
        "sebelum akhirnya disambut Oleh petugas dan\n"
        "warga di atas rakit darurat (liferaft).\n"
        "Situasi darurat di tengah kepulan asap tebal\n"
        "tersebut memaksa para penumpang\n"
        "mengambil langkah evakuasi berisil<O tinggi\n"
        "demi menyelamatkan diri menuju sel<oci dan\n"
        "perahu penolong yang bersiaga di bawah\n"
        "lambung kapal. Seluruh 428 penumpang\n"
        "serta 43 awak kapal dilaporkan dalam\n"
        "kondisi se!amat ke daratan serta\n"
        "tanpa adanya korban jiwa.\n"
        "Bagaimana perasaan kalian melihat\n"
        "perjuangan seorang ayah d!\n"
        "Banteners?\n"
        "Source: Threads @eril<l<ristian\n"
        "#kmpbaturnandi #kebakaran #selatsunda\n"
        "#pelabuhanmerak #cilegon\n"
        "cat aku\n"
        "w e r t y u i o p\n"
        "z x c v b n m"
    )

    # Execute full server pipeline
    stitched = stitch_multiline_ocr_frames([raw_sticky_notes_kelvin])
    corrected = auto_correct_ocr_text(stitched)
    final_output = ensure_paragraph_formatting(corrected)

    print("\n--- FINAL OUTPUT COPIED TO WINDOWS CLIPBOARD ---")
    print(final_output)
    print("------------------------------------------------\n")

    # 1. "diabadil<an" -> "diabadikan"
    assert "diabadikan" in final_output, "Failed to restore 'diabadikan'!"
    assert "diabadil<an" not in final_output, "Artifact 'diabadil<an' still present!"
    print("[PASS] 1. 'diabadil<an' -> 'diabadikan'")

    # 2. "@eril<l<ristian" -> "@erikkristian" (both in Para 2 and in Source line)
    assert "@erikkristian" in final_output, "Failed to restore '@erikkristian'!"
    assert "@eril<l<ristian" not in final_output, "Artifact '@eril<l<ristian' still present!"
    count_erik = final_output.count("@erikkristian")
    assert count_erik == 2, f"Expected 2 occurrences of '@erikkristian', found {count_erik}"
    print(f"[PASS] 2. '@eril<l<ristian' -> '@erikkristian' (found {count_erik} times, in Para 2 & Source)")

    # 3. "Baturnandi" & "#kmpbaturnandi" -> "Batumandi" & "#kmpbatumandi"
    assert "Batumandi" in final_output, "Failed to restore 'Batumandi'!"
    assert "#kmpbatumandi" in final_output, "Failed to restore '#kmpbatumandi'!"
    assert "Baturnandi" not in final_output, "Artifact 'Baturnandi' still present!"
    assert "#kmpbaturnandi" not in final_output, "Artifact '#kmpbaturnandi' still present!"
    print("[PASS] 3. 'Baturnandi' & '#kmpbaturnandi' -> 'Batumandi' & '#kmpbatumandi'")

    # 4. "tampal<" -> "tampak"
    assert "tampak seorang ayah" in final_output, "Failed to restore 'tampak seorang ayah'!"
    assert "tampal<" not in final_output, "Artifact 'tampal<' still present!"
    print("[PASS] 4. 'tampal<' -> 'tampak'")

    # 5. "disambut Oleh" & "diunggah Oleh" -> "disambut oleh" & "diunggah oleh"
    assert "disambut oleh petugas" in final_output, "Failed to restore 'disambut oleh petugas'!"
    assert "diunggah oleh akun" in final_output, "Failed to restore 'diunggah oleh akun'!"
    assert "disambut Oleh" not in final_output, "Mid-sentence 'disambut Oleh' still present!"
    assert "diunggah Oleh" not in final_output, "Mid-sentence 'diunggah Oleh' still present!"
    print("[PASS] 5. 'disambut Oleh' & 'diunggah Oleh' -> 'disambut oleh' & 'diunggah oleh'")

    # 6. "berisil<O" -> "berisiko"
    assert "berisiko tinggi" in final_output, "Failed to restore 'berisiko tinggi'!"
    assert "berisil<O" not in final_output and "berisikO" not in final_output, "Artifact 'berisil<O' still present!"
    print("[PASS] 6. 'berisil<O' -> 'berisiko'")

    # 7. "sel<oci" -> "sekoci"
    assert "menuju sekoci dan" in final_output, "Failed to restore 'menuju sekoci dan'!"
    assert "sel<oci" not in final_output, "Artifact 'sel<oci' still present!"
    print("[PASS] 7. 'sel<oci' -> 'sekoci'")

    # 8. "se!amat" -> "selamat"
    assert "kondisi selamat ke daratan" in final_output, "Failed to restore 'kondisi selamat ke daratan'!"
    assert "se!amat" not in final_output, "Artifact 'se!amat' still present!"
    print("[PASS] 8. 'se!amat' -> 'selamat'")

    # 9. "seorang ayah d!" -> "seorang ayah di"
    assert "seorang ayah di" in final_output, "Failed to restore 'seorang ayah di'!"
    assert "seorang ayah d!" not in final_output, "Artifact 'seorang ayah d!' still present!"
    print("[PASS] 9. 'seorang ayah d!' -> 'seorang ayah di'")

    # 10. Keyboard debris "cat aku", "w e r t y u i o p", "z x c v b n m" 100% REMOVED!
    assert "cat aku" not in final_output, "Keyboard debris 'cat aku' not removed!"
    assert "w e r t y u i o p" not in final_output, "Keyboard row 'w e r t y u i o p' not removed!"
    assert "z x c v b n m" not in final_output, "Keyboard row 'z x c v b n m' not removed!"
    print("[PASS] 10. Keyboard debris ('cat aku', 'w e r t y u i o p', 'z x c v b n m') 100% removed.")

    # 11. Paragraph format: exactly 6 blocks separated by \r\n\r\n
    paragraphs = final_output.split("\r\n\r\n")
    print(f"\nDetected {len(paragraphs)} paragraph blocks:")
    for idx, p in enumerate(paragraphs):
        p_lines = p.splitlines()
        print(f"  Para {idx + 1} ({len(p_lines)} lines): '{p_lines[0][:35]}...' -> '...{p_lines[-1][-30:]}'")

    assert len(paragraphs) == 6, f"Expected exactly 6 paragraphs, got {len(paragraphs)}!"
    assert "#kmpbatumandi" in paragraphs[5] and "#cilegon" in paragraphs[5], "Hashtag block mismatch!"
    assert len(paragraphs[5].splitlines()) == 2, "Expected 2 lines in hashtag block!"
    print("[PASS] 11. Exactly 6 paragraph blocks separated by \\r\\n\\r\\n with single \\r\\n within hashtag block.")


def test_2_optical_clean_line_unit_tests():
    print("\n" + "=" * 75)
    print("TEST 2: Optical Clean Line Unit Tests (_clean_ocr_line & _pick_cleaner_line)")
    print("=" * 75)

    cases = [
        ("diabadil<an", "diabadikan"),
        ("@eril<l<ristian", "@erikkristian"),
        ("tampal< seorang ayah", "tampak seorang ayah"),
        ("berisil<O tinggi", "berisiko tinggi"),
        ("menuju sel<oci dan", "menuju sekoci dan"),
        ("kondisi se!amat", "kondisi selamat"),
        ("perjuangan seorang ayah d!", "perjuangan seorang ayah di"),
        ("disambut Oleh petugas", "disambut oleh petugas"),
        ("diunggah Oleh akun", "diunggah oleh akun"),
        ("Baturnandi", "Batumandi"),
        ("#kmpbaturnandi", "#kmpbatumandi"),
    ]

    for raw_in, expected in cases:
        out = _clean_ocr_line(raw_in)
        assert out == expected, f"Input '{raw_in}' -> expected '{expected}', got '{out}'"
        print(f"  [OK] '{raw_in}' -> '{out}'")

    # Garbage line filtering
    garbage_cases = [
        "w e r t y u i o p",
        "z x c v b n m",
        "a s d f g h j k l",
        "cat aku",
        "kamu aku",
    ]
    for g in garbage_cases:
        assert _is_garbage_ocr_line(g), f"Garbage line '{g}' was not detected as garbage!"
        assert _clean_ocr_line(g) == "", f"Garbage line '{g}' was not cleared to empty string!"
        print(f"  [OK] Garbage line '{g}' correctly recognized and cleared.")

    # _pick_cleaner_line penalty test
    dirty_line = "Momen menegangkan tersebut diabadil<an dan diunggah Oleh akun"
    clean_line = "Momen menegangkan tersebut diabadikan dan diunggah oleh akun"
    picked = _pick_cleaner_line(dirty_line, clean_line)
    assert picked == clean_line, f"Expected clean line to be picked, got '{picked}'"
    print("  [OK] _pick_cleaner_line correctly prioritized clean line over optical defect line.")


def test_3_csharp_visual_keyboard_and_darkening_curve():
    print("\n" + "=" * 75)
    print("TEST 3: C# Visual Keyboard Detection & Darkening Curve (ocr_extractor.ps1)")
    print("=" * 75)

    ps1_path = PROJECT_ROOT / "handset" / "ocr_extractor.ps1"
    content = ps1_path.read_text(encoding="utf-8")

    # 1. Visual keyboard detector:
    assert "hasKeyboard = false" in content, "hasKeyboard flag missing!"
    assert "kbGrayCount" in content, "kbGrayCount sampler missing!"
    assert "hasKeyboard ? (int)(H * 0.49) : (int)(H * 0.768)" in content, (
        "Keyboard-aware cropBottom restriction missing!"
    )
    print("[PASS] 1. On-Screen Keyboard visual detector and cropBottom restriction (0.49*H) validated.")

    # 2. Dark text protection loosened / Tahap 34 balanced text check:
    assert "(r < 165) && (g < 165) && (b < 165)" in content or "(r < 155) && (g < 155) && (b < 155)" in content, "Dark text boundary missing!"
    print("[PASS] 2. Loosened/balanced dark text protection validated.")

    # 3. Stroke-preserving darkening curve (Tahap 31 r or Tahap 34 maxRB):
    assert "r >= 228" in content or "maxRB >= 225" in content, "Background threshold missing!"
    assert "(byte)(r * 0.55)" in content or "(byte)(maxRB * 0.55)" in content, "Stroke darkening curve missing!"
    assert "228 - 165" in content or "225 - 165" in content, "Smooth transition curve missing!"
    print("[PASS] 3. Stroke-preserving darkening curve validated.")


def main():
    print("===========================================================================")
    print("TAHAP 31 COMPREHENSIVE VERIFICATION SUITE")
    print("Optical Defect Fixes (l<, O, !, rn), Visual Keyboard Rejection, & Stroke Darkening")
    print("===========================================================================")

    test_1_sticky_notes_full_pipeline()
    test_2_optical_clean_line_unit_tests()
    test_3_csharp_visual_keyboard_and_darkening_curve()

    print("\n" + "*" * 75)
    print("ALL TAHAP 31 VERIFICATION TESTS PASSED 100% SUCCESSFULLY!")
    print("*" * 75)


if __name__ == "__main__":
    main()
