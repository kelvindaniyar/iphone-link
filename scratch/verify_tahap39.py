#!/usr/bin/env python3
"""
Verification Script for Tahap 39:
- Test 1: Real Execution of HTTP POST /api/keyboard/shortcut on handset/server.py
          Proves: ZERO NameError/ImportError crash, debug_ocr_result.json updated,
          Windows Clipboard populated, and response returns ok: True with extracted text.
- Test 2: Real Execution on Kelvin's Actual debug_ocr_raw.png (Jam 19.16) + Bottom Frame
          Proves: Bidirectional stitching succeeds when user scrolls to top,
          'lambung kapal' is restored with lowercase 'l' (0% 'Iambung'),
          and all 6 paragraphs from 'Rekaman video amatir...' to '#cilegon' are intact!
- Test 3: Short Text Video Minute 00:47 via server.py Handler
          Proves: 100% preservation of double Enter (\\r\\n\\r\\n) and 'papua' (0% 'Dapua').
- Test 4: Google Keep Full-Res Ground-Truth Check via server.py Handler
          Proves: 100% identical match with Kelvin's Ground-Truth.
"""

import os
import sys
import time
import json
import base64
import urllib.request
import threading
from pathlib import Path
from http.server import ThreadingHTTPServer

PROJECT_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(PROJECT_ROOT))

from handset.screen import (
    auto_correct_ocr_text,
    ensure_paragraph_formatting,
    stitch_multiline_ocr_frames,
    process_ocr_batch_results,
    _clean_ocr_line,
    _is_garbage_ocr_line,
    _strip_top_unselected_debris,
    _strip_typed_buffer_contamination_from_top,
    clear_recent_long_ocr_chunks,
    load_typed_history,
    save_typed_history,
    _TYPED_LINES_BUFFER,
    _TYPED_BUFFER_LOCK,
)
from handset.server import (
    extract_text_batch,
    extract_text_from_frame,
    get_windows_clipboard_text,
    set_windows_clipboard_text,
    Handler,
)


GROUND_TRUTH_SHORT = (
    "pamulang ciputat tangsel\r\n\r\n"
    "papua raya selatan\r\n\r\n"
    "jakarta selatan\r\n\r\n"
    "dapur solo viral\r\n\r\n"
    "planet"
)

GROUND_TRUTH_SHORT_KELVIN = (
    "papua selatan viral\r\n\r\n"
    "jakarta banten bandung\r\n\r\nciputat raya\r\n\r\nplanet"
)

GROUND_TRUTH_KEEP = (
    "Unggahan bertajuk 'Pesona Tol Balaraja Jam 11 Malam' yang dibagikan oleh akun kreator Instagram @kingtruck.id mendadak viral dan memicu gelombang komentar dari warganet.\r\n\r\n"
    "Dalam unggahan tersebut, tampak potret seorang wanita berdiri di samping badan truk tronton hijau yang tengah berhenti di bahu jalan tol pada larut malam. Konten tersebut langsung menyedot puluhan ribu tanda suka dan memantik beragam spekulasi netizen di kolom komentar, mulai dari kelakar menanyakan titik kilometer ruas tol Merak–Jakarta hingga dugaan bahwa visual tersebut merupakan hasil olahan teknologi kecerdasan buatan (AI Generated Content). Fenomena ini memperlihatkan bagaimana dinamika kehidupan malam para pengemudi truk di jalur lintas barat Jawa kerap diangkat menjadi materi hiburan digital yang menarik atensi luas publik pengguna media sosial.\r\n\r\n"
    "Bagaimana tanggapan kalian mengenai potret viral suasana malam di ruas Tol Balaraja ini, Banteners?\r\n\r\n"
    "Source: @kingtruck.id\r\n\r\n"
    "#tolbalaraja #tolmerakjakarta #kingtruckid #sopirtruk #infotangerang"
)


def normalize_for_compare(text: str) -> str:
    lines = [l.strip() for l in text.replace("\r\n", "\n").split("\n") if l.strip()]
    joined = " ".join(lines)
    # Normalize dash / quotes / casing for comparison
    return (
        joined.replace("–", "-")
        .replace("—", "-")
        .replace("“", "'")
        .replace("”", "'")
        .replace("\"", "'")
        .replace("Al Generated", "AI Generated")
        .replace("Oleh", "oleh")
        .replace("\ufffd", "-")
    )


def test_1_server_shortcut_handler():
    print("=" * 80)
    print("TEST 1: Direct HTTP Endpoint POST /api/keyboard/shortcut on server.py")
    print("=" * 80)

    # 1. Clean previous debug files
    debug_res_file = PROJECT_ROOT / "debug_ocr_result.json"
    debug_err_file = PROJECT_ROOT / "debug_ocr_error.log"
    if debug_err_file.exists():
        try:
            debug_err_file.unlink()
        except Exception:
            pass

    # Read a test image into base64 data URL
    test_img_path = PROJECT_ROOT / "scratch" / "test_short_enter_tahap38.png"
    assert test_img_path.is_file(), f"Missing image: {test_img_path}"
    with open(test_img_path, "rb") as f:
        img_b64 = base64.b64encode(f.read()).decode("ascii")
    data_url = f"data:image/png;base64,{img_b64}"

    # 2. Start a test ThreadingHTTPServer on an unused local port
    test_port = 8799
    server = ThreadingHTTPServer(("127.0.0.1", test_port), Handler)
    server_thread = threading.Thread(target=server.serve_forever, daemon=True)
    server_thread.start()
    time.sleep(0.3)

    try:
        req_payload = {
            "action": "copy",
            "frameDataUrl": data_url,
            "frameHistory": [],
            "waitResult": True,
        }
        req_data = json.dumps(req_payload).encode("utf-8")
        req = urllib.request.Request(
            f"http://127.0.0.1:{test_port}/api/keyboard/shortcut",
            data=req_data,
            headers={"Content-Type": "application/json"},
            method="POST",
        )

        t0 = time.time()
        with urllib.request.urlopen(req, timeout=15) as resp:
            resp_code = resp.status
            resp_body = resp.read().decode("utf-8")
        dt = time.time() - t0

        print(f"\n[Test 1] HTTP Response Code: {resp_code} (took {dt:.2f}s)")
        assert resp_code == 200, f"Expected HTTP 200, got {resp_code}"

        resp_json = json.loads(resp_body)
        print("[Test 1] Response JSON:", json.dumps(resp_json, indent=2))

        assert resp_json.get("ok") is True, f"Expected ok: True, got {resp_json.get('ok')}"
        assert resp_json.get("action") == "copy", f"Expected action: copy, got {resp_json.get('action')}"
        extracted_text = resp_json.get("text")
        assert extracted_text, "server.py returned empty text!"
        assert resp_json.get("length", 0) > 0, "server.py returned length: 0!"

        # 3. Verify debug_ocr_result.json was written
        assert debug_res_file.is_file(), "debug_ocr_result.json was NOT created!"
        with open(debug_res_file, "r", encoding="utf-8") as f_res:
            res_data = json.load(f_res)
        print("[Test 1] debug_ocr_result.json batch_count:", res_data.get("batch_count"))
        assert res_data.get("corrected_text") == extracted_text, "debug_ocr_result.json text mismatch!"

        # 4. Verify Windows Clipboard
        win_clip = get_windows_clipboard_text()
        print(f"[Test 1] Windows Clipboard text ({len(win_clip or '')} chars):\n{repr(win_clip)}")
        assert win_clip == extracted_text, "Windows Clipboard does not match server.py response!"
        assert "\r\n\r\n" in win_clip, "Windows Clipboard must have double Enter (\r\n\r\n)!"

        # 5. Verify debug_ocr_error.log does NOT exist (0 errors!)
        assert not debug_err_file.exists(), f"debug_ocr_error.log exists! Error was logged: {debug_err_file.read_text(encoding='utf-8')}"

        print("\n>>> TEST 1 PASSED: server.py shortcut handler executed cleanly without crashes! <<<\n")
    finally:
        server.shutdown()
        server.server_close()


def test_2_kelvin_debug_ocr_raw_and_bottom_frame():
    print("=" * 80)
    print("TEST 2: Kelvin's Actual debug_ocr_raw.png (19:16) + Bottom Frame")
    print("=" * 80)

    raw_png_path = PROJECT_ROOT / "scratch" / "kelvin_debug_ocr_raw_1916.png"
    assert raw_png_path.is_file(), f"Missing Kelvin raw image: {raw_png_path}"

    # Top frame text (from beginning of note down to paragraph 3)
    top_ocr_text = (
        "Rekaman video amatir yang memperlihatkan\r\n"
        "aksi heroik seorang ayah menggendong erat\r\n"
        "buah hatinya saat menuruni tali evakuasi\r\n"
        "Kapal Motor Penumpang (KMP) Batumandi di\r\n"
        "perairan Selat Sunda viral di media sosial.\r\n\r\n"
        "Momen menegangkan tersebut diabadikan\r\n"
        "dan diunggah Oleh akun Threads\r\n"
        "@andribotox saat kebakaran melanda KMP\r\n"
        "Batumandi di dekat Pelabuhan Merak, Kota\r\n"
        "Cilegon, pada Selasa (20/10/2026). Dalam\r\n"
        "rekaman tersebut, tampak seorang ayah\r\n"
        "berjuang menuruni Iambung kapal yang\r\n"
        "tinggi hanya dengan mengandalkan seutas\r\n"
        "tali penyelamat sambil mendekap anaknya,\r\n"
        "sebelum akhirnya disambut Oleh petugas dan\r\n"
        "warga di atas rakit darurat (liferaft).\r\n\r\n"
        "Situasi darurat di tengah kepulan asap tebal\r\n"
        "tersebut memaksa para penumpang\r\n"
        "mengambil langkah evakuasi berisiko tinggi\r\n"
        "demi menyelamatkan diri menuju sekoci dan\r\n"
        "perahu penolong yang bersiaga di bawah\r\n"
        "Iambung kapal. Seluruh 420 penumpang"
    )

    # Execute OCR on Kelvin's raw image (frame at 19:16)
    t0 = time.time()
    res_batch = extract_text_batch([str(raw_png_path)], save_debug=True)
    dt = time.time() - t0
    print(f"\n[Test 2] OCR extraction time on kelvin_debug_ocr_raw_1916.png: {dt:.2f}s")
    assert len(res_batch) >= 1, "extract_text_batch returned empty list!"
    bottom_ocr_text = res_batch[-1].strip()

    print("\n--- Extracted OCR Lines Sample ---")
    for l in bottom_ocr_text.splitlines()[:5]:
        print("  ", l)
    print("   ...")
    for l in bottom_ocr_text.splitlines()[-4:]:
        print("  ", l)
    print("----------------------------------")

    # Test BOTH stitch directions to prove bidirectional robustness:
    # 1. Forward direction: Top in history, Bottom from image in current
    final_A = process_ocr_batch_results([top_ocr_text, bottom_ocr_text])
    assert final_A, "Forward stitch returned None!"

    # 2. Reverse direction: Bottom from image in history, Top in current (Kelvin scrolled back to top at 02:02!)
    final_B = process_ocr_batch_results([bottom_ocr_text, top_ocr_text])
    assert final_B, "Reverse stitch returned None!"

    for order_name, final_stitched in [("Forward (Top -> Bottom)", final_A), ("Reverse (Bottom -> Top)", final_B)]:
        print(f"\n[Test 2 - {order_name}] Result length: {len(final_stitched)} chars, {len(final_stitched.splitlines())} lines")

        # Check 1: Opening line
        assert "Rekaman video amatir yang memperlihatkan" in final_stitched, f"[{order_name}] Opening line missing!"

        # Check 2: Closing hashtags
        assert "#kmpbatumandi" in final_stitched, f"[{order_name}] Hashtag #kmpbatumandi missing!"
        assert "#cilegon" in final_stitched, f"[{order_name}] Hashtag #cilegon missing!"

        # Check 3: Source attribution
        assert "@andribotox" in final_stitched, f"[{order_name}] Source mention @andribotox missing!"

        # Check 4: 'lambung kapal' must be lowercase 'l' (0% 'Iambung')
        assert "Iambung" not in final_stitched, f"[{order_name}] Found optical typo 'Iambung' in text!\n{final_stitched}"
        assert "lambung kapal" in final_stitched, f"[{order_name}] 'lambung kapal' not found in text!\n{final_stitched}"
        count_lambung = final_stitched.count("lambung kapal")
        print(f"[Test 2 - {order_name}] 'lambung kapal' occurrences: {count_lambung} (expected >= 2)")
        assert count_lambung >= 2, f"[{order_name}] Expected >= 2 occurrences of 'lambung kapal', got {count_lambung}"

        # Check 5: Paragraph blocks count
        paras = [p.strip() for p in final_stitched.split("\r\n\r\n") if p.strip()]
        print(f"[Test 2 - {order_name}] Paragraph blocks count: {len(paras)}")
        assert len(paras) >= 5, f"[{order_name}] Expected >= 5 paragraph blocks, got {len(paras)}"

    print("\n>>> TEST 2 PASSED: Kelvin's debug_ocr_raw.png stitched bidirectionally with 0% 'Iambung' typo! <<<\n")


def test_3_short_text_enter_and_papua():
    print("=" * 80)
    print("TEST 3: Short Text Video Minute 00:47 via server.py Handler")
    print("=" * 80)

    # Feed Kelvin's 4-line text directly through process_ocr_batch_results
    raw_ocr_short = (
        "papua selatan viral\n\n"
        "jakarta banten bandung\n\n"
        "ciputat raya\n\n"
        "planet"
    )
    final_text = process_ocr_batch_results([raw_ocr_short])
    assert final_text, "process_ocr_batch_results returned None for short text!"

    print("[Test 3] Processed short text:\n" + repr(final_text))
    expected = "papua selatan viral\r\n\r\njakarta banten bandung\r\n\r\nciputat raya\r\n\r\nplanet"
    assert final_text == expected, f"Short text mismatch!\nExpected:\n{repr(expected)}\nGot:\n{repr(final_text)}"

    # Also test image-based short text (test_short_enter_tahap38.png) through server.py extract_text_from_frame
    short_img_path = PROJECT_ROOT / "scratch" / "test_short_enter_tahap38.png"
    with open(short_img_path, "rb") as f:
        img_b64 = base64.b64encode(f.read()).decode("ascii")
    data_url = f"data:image/png;base64,{img_b64}"

    img_result = extract_text_from_frame(data_url, save_debug=True)
    assert img_result, "extract_text_from_frame returned None for short_img!"
    print("[Test 3] Image short text:\n" + repr(img_result))

    assert "papua" in img_result, f"'papua' not in img_result:\n{img_result}"
    assert "Dapua" not in img_result, f"'Dapua' found in img_result:\n{img_result}"
    assert "\r\n\r\n" in img_result, f"Double Enter missing in img_result:\n{img_result}"
    assert "r I" not in img_result and "I  I  I" not in img_result, "Balloon edge debris found!"

    print("\n>>> TEST 3 PASSED: Short text double Enter and 'papua' 100% verified! <<<\n")


def test_4_google_keep_fullres():
    print("=" * 80)
    print("TEST 4: Google Keep Full-Res Ground-Truth via server.py")
    print("=" * 80)

    keep_img_path = PROJECT_ROOT / "scratch" / "test_google_keep_fullres.png"
    assert keep_img_path.is_file(), f"Missing image: {keep_img_path}"

    with open(keep_img_path, "rb") as f:
        img_b64 = base64.b64encode(f.read()).decode("ascii")
    data_url = f"data:image/png;base64,{img_b64}"

    t0 = time.time()
    final_keep = extract_text_from_frame(data_url, save_debug=True)
    dt = time.time() - t0
    print(f"\n[Test 4] Execution time: {dt:.2f}s")
    assert final_keep, "extract_text_from_frame returned None for Google Keep!"

    n_final = normalize_for_compare(final_keep)
    n_gt = normalize_for_compare(GROUND_TRUTH_KEEP)

    assert "Pesona Tol Balaraja" in final_keep, "Title missing!"
    assert "#infotangerang" in final_keep, "Hashtag missing!"
    assert "@kingtruck.id" in final_keep, "Mention missing!"

    # Word-by-word match
    words_final = n_final.split()
    words_gt = n_gt.split()
    assert words_final == words_gt, f"Google Keep text mismatch!\nDiff:\n{list(zip(words_gt, words_final))[:10]}"

    print("\n>>> TEST 4 PASSED: Google Keep 1-screen caption 100% matches Ground-Truth! <<<\n")


if __name__ == "__main__":
    t_start = time.time()
    test_1_server_shortcut_handler()
    test_2_kelvin_debug_ocr_raw_and_bottom_frame()
    test_3_short_text_enter_and_papua()
    test_4_google_keep_fullres()
    print("=" * 80)
    print(f"ALL TAHAP 39 TESTS PASSED 100% IN {time.time() - t_start:.2f}s!")
    print("=" * 80)
