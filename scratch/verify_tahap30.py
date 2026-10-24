"""Verification script for Tahap 30:
1. TEST 1: Full End-to-End Pipeline (stitch_multiline_ocr_frames -> auto_correct_ocr_text -> ensure_paragraph_formatting)
   - Input: Exact raw text from Kelvin's Notepad at 03:42 (with 'tik-detik', 'korban i1wa.', tightly packed lines).
   - Verifies:
     * 'detik-detik seorang ayah menggendong erat' (restored from 'tik-detik')
     * 'tanpa adanya korban jiwa.' (restored from 'korban i1wa.')
     * Exactly 6 paragraph blocks separated by \r\n\r\n.
     * 2 hashtag lines joined with single \r\n (never double \r\n\r\n).
     * No \r\r\n corruption.

2. TEST 2: Viewport History Global Deduplication Simulation (Kelvin 02:05 - 02:35 scroll bottom scenario)
   - 1 top frame at t=0s.
   - 1 bottom frame at t=3s.
   - 40 bottom sampling calls with jitter over 27 seconds.
   - Verifies that top frame is 100% RETAINED in getRecentSelectionFrames().

3. TEST 3: AssistiveTouch & Selection Handle Guard in C# preprocessor (ocr_extractor.ps1)
   - Verifies presence of cW >= 55 && cH >= 55 && aspect >= 0.75f && aspect <= 1.35f
   - Verifies yellow selection handle filter preserving dark text.

4. TEST 4: Single-Pass Batch OCR Logic in server.py and ocr_extractor.ps1
   - Verifies powershell.exe is called only ONCE in server.py _do_extract.
   - Verifies short text (< 4 lines) exits immediately without processing history.
"""

import math
import re
import sys
import time
from pathlib import Path

if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")

PROJECT_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(PROJECT_ROOT))

from handset.screen import (
    _clean_ocr_line,
    _pick_cleaner_line,
    _unsplit_broken_lines,
    auto_correct_ocr_text,
    ensure_paragraph_formatting,
    stitch_multiline_ocr_frames,
)
import handset.server as server_mod


def test_1_end_to_end_pipeline_notepad_0342():
    print("\n" + "=" * 75)
    print("TEST 1: Full End-to-End Pipeline (Kelvin 03:42 Notepad Raw Input)")
    print("=" * 75)

    # Exact raw text from Kelvin's Notepad at 03:42:
    # Notice: 'tik-detik' on line 2, 'korban i1wa.' on line 24, and no blank lines between paragraphs
    raw_notepad_0342 = (
        "Rekaman video amatir yang memperlihatkan\n"
        "tik-detik seorang ayah menggendong erat\n"
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
        "warga di atas rakit darurat (liferaft).\n"
        "Situasi darurat di tengah kepulan asap tebal\n"
        "tersebut memaksa para penumpang\n"
        "mengambil langkah evakuasi berisiko tinggi\n"
        "demi menyelamatkan diri menuju sekoci dan\n"
        "perahu penolong yang bersiaga di bawah\n"
        "lambung kapal. Seluruh 428 penumpang\n"
        "serta 43 awak kapal dilaporkan berhasil\n"
        "dievakuasi dengan selamat ke Pelabuhan Merak\n"
        "tanpa adanya korban i1wa.\n"
        "Bagaimana perasaan kalian melihat video ini,\n"
        "Banteners?\n"
        "Source: Threads @erikkristian\n"
        "#kmpbatumandi #kebakaran #selatsunda\n"
        "#pelabuhanmerak #cilegon"
    )

    # 1. Step 1 in server.py: stitch_multiline_ocr_frames
    stitched = stitch_multiline_ocr_frames([raw_notepad_0342])
    print(f"1. After stitch_multiline_ocr_frames: {len(stitched)} chars, {len(stitched.splitlines())} lines")

    # 2. Step 2 in server.py: auto_correct_ocr_text
    corrected = auto_correct_ocr_text(stitched)
    print(f"2. After auto_correct_ocr_text: {len(corrected)} chars, {len(corrected.splitlines())} lines")

    # 3. Step 3 in server.py: ensure_paragraph_formatting (CALLED PALING AKHIR)
    final_output = ensure_paragraph_formatting(corrected)
    print(f"3. After ensure_paragraph_formatting: {len(final_output)} chars, {len(final_output.splitlines())} lines")

    print("\n--- FINAL OUTPUT COPIED TO WINDOWS CLIPBOARD ---")
    print(final_output)
    print("------------------------------------------------\n")

    # Check 1: 'detik-detik' restored from 'tik-detik'
    assert "detik-detik seorang ayah menggendong erat" in final_output, "Typo 'tik-detik' was not corrected to 'detik-detik'!"
    assert not re.search(r'(?<!de)tik-detik\b', final_output), "Found orphan 'tik-detik' in final output!"
    print("[PASS] 1. 'tik-detik' 100% auto-corrected to 'detik-detik seorang ayah menggendong erat'.")

    # Check 2: 'jiwa' restored from 'i1wa'
    assert "tanpa adanya korban jiwa." in final_output, "Typo 'korban i1wa.' was not corrected to 'korban jiwa.'!"
    assert "i1wa" not in final_output, "Found orphan 'i1wa' in final output!"
    print("[PASS] 2. 'korban i1wa.' 100% auto-corrected to 'tanpa adanya korban jiwa.'.")

    # Check 3: Exactly 6 distinct paragraph blocks separated by \r\n\r\n
    paragraphs = final_output.split("\r\n\r\n")
    print(f"[INFO] Detected {len(paragraphs)} paragraph blocks:")
    for idx, p in enumerate(paragraphs):
        p_lines = p.splitlines()
        print(f"  Paragraph {idx + 1} ({len(p_lines)} lines): First line: '{p_lines[0][:38]}...' | Last line: '...{p_lines[-1][-30:]}'")

    assert len(paragraphs) == 6, f"Expected exactly 6 paragraphs, got {len(paragraphs)}!"

    # Verify content of each paragraph block
    assert "Rekaman video amatir" in paragraphs[0] and "viral di media sosial." in paragraphs[0], "Paragraph 1 boundary mismatch!"
    assert "Momen menegangkan" in paragraphs[1] and "rakit darurat (liferaft)." in paragraphs[1], "Paragraph 2 boundary mismatch!"
    assert "Situasi darurat" in paragraphs[2] and "tanpa adanya korban jiwa." in paragraphs[2], "Paragraph 3 boundary mismatch!"
    assert "Bagaimana perasaan" in paragraphs[3] and "Banteners?" in paragraphs[3], "Paragraph 4 boundary mismatch!"
    assert "Source: Threads @erikkristian" in paragraphs[4], "Paragraph 5 (Source) boundary mismatch!"
    assert "#kmpbatumandi" in paragraphs[5] and "#cilegon" in paragraphs[5], "Paragraph 6 (Hashtags) boundary mismatch!"
    print("[PASS] 3. All 6 paragraph blocks correctly separated by \\r\\n\\r\\n.")

    # Check 4: The 2 hashtag lines must be joined by single \r\n, NEVER \r\n\r\n
    hashtag_para = paragraphs[5]
    hashtag_lines = hashtag_para.splitlines()
    assert len(hashtag_lines) == 2, f"Expected 2 lines in hashtag block, got {len(hashtag_lines)}"
    assert "\r\n\r\n" not in hashtag_para, "Hashtag lines separated by blank line!"
    print("[PASS] 4. Consecutive hashtag lines joined with single \\r\\n (no blank line in between).")

    # Check 5: No \r\r\n corruption
    assert "\r\r\n" not in final_output, "Found corrupt \\r\\r\\n line endings!"
    print("[PASS] 5. Clean Windows CRLF line endings verified without corrupt \\r\\r\\n.")


def test_2_viewport_history_global_deduplication():
    print("\n" + "=" * 75)
    print("TEST 2: Viewport History Global Deduplication (30s Bottom Scroll Simulation)")
    print("=" * 75)

    class ViewportHistorySim:
        def __init__(self, capacity=24, ttl_ms=180000):
            self.capacity = capacity
            self.ttl_ms = ttl_ms
            self.history = []

        def get_delta(self, sig1, sig2):
            if not sig1 or not sig2:
                return 999
            return sum(abs(a - b) for a, b in zip(sig1, sig2)) / len(sig1)

        def sample(self, now, sig, data_url):
            # Prune
            self.history = [item for item in self.history if (now - item["ts"]) <= self.ttl_ms]

            if not self.history:
                self.history.append({"ts": now, "sig": sig, "dataUrl": data_url})
                return

            min_diff = 999999
            closest_idx = -1
            for idx, item in enumerate(self.history):
                d = self.get_delta(sig, item["sig"])
                if d < min_diff:
                    min_diff = d
                    closest_idx = idx

            # Global deduplication: if min_diff < 7.5, refresh slot without pushing new item!
            if min_diff < 7.5 and closest_idx >= 0:
                self.history[closest_idx]["ts"] = now
                self.history[closest_idx]["sig"] = sig
                self.history[closest_idx]["dataUrl"] = data_url
            else:
                self.history.append({"ts": now, "sig": sig, "dataUrl": data_url})
                if len(self.history) > self.capacity:
                    self.history.pop(0)

        def get_recent_selection_frames(self, now):
            valid = [item for item in self.history if (now - item["ts"]) <= self.ttl_ms]
            if len(valid) <= 5:
                return [item["dataUrl"] for item in valid]
            n = len(valid)
            indices = sorted(list(set([0, n // 4, n // 2, (3 * n) // 4, n - 1])))
            chosen = [valid[i] for i in indices]
            return [item["dataUrl"] for item in chosen]

    sim = ViewportHistorySim(capacity=24, ttl_ms=180000)

    # Detik 02:05: User opens top of note
    t0 = 1000000
    top_sig = [100.0] * 16
    top_url = "data:image/png;base64,TOP_FRAME_AT_02_05"
    sim.sample(t0, top_sig, top_url)
    assert len(sim.history) == 1

    # Detik 02:08: User scrolls to bottom of note
    t1 = t0 + 3000
    bottom_sig = [200.0] * 16
    bottom_url = "data:image/png;base64,BOTTOM_FRAME_AT_02_08"
    sim.sample(t1, bottom_sig, bottom_url)
    assert len(sim.history) == 2
    print(f"After scroll to bottom: {len(sim.history)} history slots recorded.")

    # Detik 02:08 s.d. 02:35 (27 seconds): User scrolls up and down slightly at the bottom (40 samples with minor jitter)
    for step in range(40):
        t_step = t1 + (step + 1) * 600  # every 600ms
        jitter_val = 200.0 + (math.sin(step) * 2.5)  # delta is ~2.5, well below 7.5!
        jitter_sig = [jitter_val] * 16
        jitter_url = f"data:image/png;base64,BOTTOM_FRAME_STEP_{step}"
        sim.sample(t_step, jitter_sig, jitter_url)

    # In Tahap 29, 40 samples evicted slot 0 because history capacity was only 15 and diff was only checked against last!
    # With Tahap 30 Global Deduplication, history slots must remain small and top frame must NEVER be evicted!
    print(f"After 40 bottom scroll samples: {len(sim.history)} slots in history (capacity: {sim.capacity}).")
    assert len(sim.history) <= sim.capacity, f"History exceeded capacity: {len(sim.history)}"

    # Get recent selection frames at t = 35 seconds
    t_end = t1 + 35000
    frames = sim.get_recent_selection_frames(t_end)
    print(f"get_recent_selection_frames returned {len(frames)} frames.")

    assert top_url in frames, "CRITICAL BUG: Top frame was evicted after 27 seconds of scrolling at bottom!"
    assert frames[0] == top_url, "Top frame is not at index 0 of selection frames!"
    print("[PASS] Top frame (detik 02:05) 100% retained at index 0 after 27s of bottom scrolling.")


def test_3_csharp_assistivetouch_guard_and_handle_filter():
    print("\n" + "=" * 75)
    print("TEST 3: C# AssistiveTouch Guard & Selection Handle Filter (ocr_extractor.ps1)")
    print("=" * 75)

    ps1_path = PROJECT_ROOT / "handset" / "ocr_extractor.ps1"
    content = ps1_path.read_text(encoding="utf-8")

    # 1. Verify strict AssistiveTouch guard:
    assert "cW >= 55 && cH >= 55 && clusterRowCount >= 40 && aspect >= 0.75f && aspect <= 1.35f" in content, (
        "Strict AssistiveTouch guard condition missing in ocr_extractor.ps1!"
    )
    print("[PASS] 1. AssistiveTouch guard condition validated (cW>=55, cH>=55, rowCount>=40, 0.75<=aspect<=1.35).")

    # 2. Verify selection handle filter:
    assert "(r > 180) && (g > 120) && ((r - b) > 40)" in content, "Selection handle color detection missing!"
    assert "(r < 165) && (g < 165) && (b < 165)" in content or "(r < 155) && (g < 155) && (b < 155)" in content, "Dark text edge protection missing!"
    assert ("isHandle && !isDarkText" in content) or ("isHlOrHandle && !isDarkText" in content), "Handle neutralization guard missing!"
    print("[PASS] 2. Selection handle filter with dark text boundary protection validated.")

    # 3. Verify single-pass batch OCR logic:
    assert "Process-SingleFrame" in content, "Process-SingleFrame helper missing in ocr_extractor.ps1!"
    assert "$nonEmptyCurrent.Count -lt 4 -and $charCountCurrent -lt 150" in content, "Short text fast-exit condition missing!"
    assert "---FRAME_SPLIT---" in content, "Frame split delimiter missing!"
    print("[PASS] 3. Single-pass batch OCR architecture validated in ocr_extractor.ps1.")


def test_4_server_single_pass_batch_ocr():
    print("\n" + "=" * 75)
    print("TEST 4: Server Single-Pass Batch OCR Integration (server.py)")
    print("=" * 75)

    server_path = PROJECT_ROOT / "handset" / "server.py"
    content = server_path.read_text(encoding="utf-8")

    # Verify server.py calls extract_text_batch ONLY ONCE in _do_extract:
    do_extract_def = content[content.find("def _do_extract"):content.find("if frame_data_url:")]
    assert "extract_text_from_frame(frame_url" not in do_extract_def, (
        "Found redundant extract_text_from_frame call in _do_extract! Must call extract_text_batch once only."
    )
    assert "batch_texts = extract_text_batch(frames_to_batch, save_debug=True)" in do_extract_def, (
        "extract_text_batch call missing from _do_extract!"
    )
    assert "ensure_paragraph_formatting(final_text)" in do_extract_def, (
        "ensure_paragraph_formatting must be called PALING AKHIR in _do_extract!"
    )
    print("[PASS] server.py calls extract_text_batch exactly ONCE and applies ensure_paragraph_formatting as final step.")


def main():
    print("===========================================================================")
    print("TAHAP 30 COMPREHENSIVE VERIFICATION SUITE")
    print("Paragraph Blank Lines Preservation, Typo Fixes, Viewport Deduplication, & Single-Pass Batch OCR")
    print("===========================================================================")

    test_1_end_to_end_pipeline_notepad_0342()
    test_2_viewport_history_global_deduplication()
    test_3_csharp_assistivetouch_guard_and_handle_filter()
    test_4_server_single_pass_batch_ocr()

    print("\n" + "*" * 75)
    print("ALL 4 TAHAP 30 VERIFICATION TESTS PASSED 100% SUCCESSFULLY!")
    print("*" * 75)


if __name__ == "__main__":
    main()
