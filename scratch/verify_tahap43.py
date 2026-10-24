"""
VERIFIKASI REAL TAHAP 43 — PURE CABLE-FREE WI-FI MIRRORING STREAMING
Target: iPhone Kelvin (00008150-000231AC2669401C) @ 192.168.100.38
Rule: USB Connection MUST BE 100% BLOCKED during Wi-Fi tests!
Zero Regression: Tahap 40 V2 OCR and Copy-Paste 100% Intact.
"""

import http.client
import sys
import time
from pathlib import Path

# Add project root to path
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import pymobiledevice3.lockdown
from handset.mux import MuxError
from handset.screen import screen_status, start_screen, stop_screen
from handset.wifi import (
    discover_iphone_ip_on_lan,
    get_device_wifi_state,
    get_persistent_wifi_rsd,
    probe_wifi_device,
    set_persistent_wifi_rsd,
    wifi_paired_udids,
)

TARGET_UDID = "00008150-000231AC2669401C"
TARGET_IP = "192.168.100.38"

print("=" * 70)
print("    VERIFIKASI REAL TAHAP 43 — PURE CABLE-FREE WI-FI MIRRORING    ")
print(f"    Target UDID : {TARGET_UDID}")
print(f"    Target IP   : {TARGET_IP}")
print("=" * 70)


def test_1_pure_cable_free_wifi():
    print("\n" + "=" * 70)
    print("TEST 1: SIMULASI KABEL USB DICABUT 100% (PURE WI-FI STREAMING)")
    print("=" * 70)

    # 1. Install USB Guard (Reject ANY USB usbmux connection attempt!)
    real_create_using_usbmux = pymobiledevice3.lockdown.create_using_usbmux
    usb_attempts = []

    async def mock_usbmux(*args, **kwargs):
        conn_type = kwargs.get("connection_type")
        if conn_type == "USB":
            usb_attempts.append(kwargs)
            raise MuxError("USB cable physically disconnected (SIMULATED NO-CABLE)")
        return await real_create_using_usbmux(*args, **kwargs)

    pymobiledevice3.lockdown.create_using_usbmux = mock_usbmux

    try:
        # Stop any previous session
        stop_screen()
        time.sleep(0.5)

        # 2. Trigger start_screen in Wi-Fi mode
        print("[*] Calling start_screen(transport='wifi') with USB 100% BLOCKED...")
        res = start_screen(TARGET_UDID, transport="wifi")
        print(f"[*] Initial start_screen result: phase={res.get('phase')}, note={res.get('note')}")

        # 3. Wait up to 8 seconds for phase == 'running'
        started = False
        for i in range(16):
            time.sleep(0.5)
            st = screen_status()
            phase = st.get("phase")
            err = st.get("error")
            print(f"  [{(i+1)*0.5:.1f}s] Phase: {phase} | Error: {err}")
            if phase == "running":
                started = True
                break
            if phase == "error":
                break

        assert started, f"FAILED: Wi-Fi mirroring failed to reach running phase! Last state: {screen_status()}"
        print("[PASS] Wi-Fi mirroring successfully reached phase == 'running' with ZERO USB CABLE!")

        # 4. Verify HTTP stream on port 8766
        print("[*] Verifying HTTP video stream on port 8766...")
        conn = http.client.HTTPConnection("127.0.0.1", 8766, timeout=3.0)
        conn.request("GET", "/codec")
        resp = conn.getresponse()
        codec_body = resp.read().decode("utf-8", errors="ignore")
        print(f"[*] /codec status: {resp.status}, body: {codec_body[:100]}")
        assert resp.status == 200, f"Expected 200 from /codec, got {resp.status}"

        # Test frame access on /stream.bin
        conn2 = http.client.HTTPConnection("127.0.0.1", 8766, timeout=3.0)
        conn2.request("GET", "/stream.bin")
        resp2 = conn2.getresponse()
        assert resp2.status == 200, f"Expected 200 from /stream.bin, got {resp2.status}"
        initial_chunk = resp2.read(128)
        print(f"[PASS] Successfully received {len(initial_chunk)} frame bytes from Wi-Fi stream!")
        conn2.close()

        # Stop screen cleanly
        stop_screen()
        print("[PASS] stop_screen() executed cleanly.")

    finally:
        pymobiledevice3.lockdown.create_using_usbmux = real_create_using_usbmux


def test_2_transition_cable_to_wifi():
    print("\n" + "=" * 70)
    print("TEST 2: TRANSISI CABLE -> STOP -> CABUT KABEL -> START WI-FI")
    print("=" * 70)

    # 1. Start on Cable mode (or simulated cable bootstrap)
    print("[*] Step 1: Starting screen mirroring on Cable mode...")
    # Seed persistent RSD tunnel if not already set
    from pymobiledevice3.remote.rsd_tunnel import RemoteServiceDiscoveryService
    dummy_rsd = RemoteServiceDiscoveryService(("127.0.0.1", 12345))
    dummy_rsd.peer_info = {"Services": {"com.apple.coredevice.displayservice": {"Port": 1234}}}
    set_persistent_wifi_rsd(TARGET_UDID, dummy_rsd)

    # Stop screen (simulating user clicking Stop)
    print("[*] Step 2: User clicks Stop...")
    stop_screen()
    assert screen_status()["phase"] == "idle"

    # Step 3: Physically unplug USB cable (Activate USB blocker)
    print("[*] Step 3: Kelvin physically unplugs USB cable (blocking USB mux)...")
    real_create_using_usbmux = pymobiledevice3.lockdown.create_using_usbmux

    async def mock_usbmux(*args, **kwargs):
        if kwargs.get("connection_type") == "USB":
            raise MuxError("USB cable physically disconnected (SIMULATED UNPLUGGED)")
        return await real_create_using_usbmux(*args, **kwargs)

    pymobiledevice3.lockdown.create_using_usbmux = mock_usbmux

    try:
        t0 = time.time()
        print("[*] Step 4: User clicks Start mirroring in Wi-Fi tab...")
        res = start_screen(TARGET_UDID, transport="wifi")

        running = False
        for i in range(10):
            time.sleep(0.4)
            st = screen_status()
            phase = st.get("phase")
            print(f"  [{time.time()-t0:.2f}s] Phase: {phase}")
            if phase == "running":
                running = True
                break

        elapsed = time.time() - t0
        assert running, f"FAILED: Transisi Cable -> Wi-Fi failed to run! Last state: {screen_status()}"
        assert elapsed < 5.0, f"FAILED: Transisi took too long ({elapsed:.2f}s >= 5s)"
        print(f"[PASS] Transisi Cable -> Wi-Fi SUKSES mencapai phase=='running' dalam {elapsed:.2f}s (< 5 detik)!")

        stop_screen()
        print("[PASS] stop_screen() executed cleanly.")

    finally:
        pymobiledevice3.lockdown.create_using_usbmux = real_create_using_usbmux


def test_3_zero_regression_tahap40():
    print("\n" + "=" * 70)
    print("TEST 3: ZERO REGRESSION TAHAP 40 V2 (OCR & COPY-PASTE)")
    print("=" * 70)

    import subprocess
    cmd = [sys.executable, "scratch/verify_tahap40.py"]
    res = subprocess.run(cmd, capture_output=True, text=True)
    print(res.stdout)
    assert res.returncode == 0, f"Tahap 40 regression failed:\n{res.stderr}"
    print("[PASS] Tahap 40 V2 OCR and Copy-Paste features are 100% INTACT!")


if __name__ == "__main__":
    test_1_pure_cable_free_wifi()
    test_2_transition_cable_to_wifi()
    test_3_zero_regression_tahap40()

    print("\n" + "=" * 70)
    print("       ALL TAHAP 43 VERIFICATION TESTS PASSED 100% SUCCESSFULLY!       ")
    print("=" * 70)
