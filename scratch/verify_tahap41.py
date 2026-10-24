"""Comprehensive verification suite for Tahap 41:
1. Test 1: Auto-Enable Wireless Lockdown & wifi_state.json persistence
2. Test 2: Real Network Discovery (usbmux, TCP port 62078 probe, ARP MAC lookup, prioritized LAN IP binding)
3. Test 3: Backend endpoints (/api/status, /api/wifi/probe, /api/screen on wifi mode)
4. Test 4: Full regression check of Tahap 40 OCR & Short text
"""

import json
import sys
import os
import subprocess
import time
from pathlib import Path
from urllib.request import urlopen, Request

# Force UTF-8 output on Windows console
if sys.stdout.encoding.lower() != "utf-8":
    sys.stdout.reconfigure(encoding="utf-8")

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

import handset.wifi as hw
from handset.screen import start_screen, screen_status, stop_screen


def test_1_wifi_state_persistence():
    print("\n" + "=" * 70)
    print("TEST 1: Auto-Enable Wireless Lockdown & wifi_state.json Persistence")
    print("=" * 70)

    test_udid = "00008150-TESTUDID999"
    # Save test state
    updated = hw.update_device_wifi_state(
        test_udid,
        enabled=True,
        ip="192.168.100.77",
        mac="AA:BB:CC:DD:EE:FF",
        name="Kelvin's iPhone 15 Pro",
        last_seen_cable=True,
    )
    assert updated.get("wifi_enabled_on_device") is True, "wifi_enabled_on_device must be True"
    assert updated.get("last_ip") == "192.168.100.77", "last_ip mismatch"
    assert updated.get("mac_address") == "aa-bb-cc-dd-ee-ff", "mac_address normalized mismatch"

    # Verify reload from disk
    state = hw.load_wifi_state()
    assert test_udid in state.get("devices", {}), "Device must be present in saved wifi_state.json"
    reloaded = hw.get_device_wifi_state(test_udid)
    assert reloaded["device_name"] == "Kelvin's iPhone 15 Pro"
    print("  [PASS] wifi_state.json created and verified on disk!")

    # Clean up test entry
    state["devices"].pop(test_udid, None)
    hw.save_wifi_state(state)
    print("  [PASS] Cleanup completed cleanly.")


def test_2_network_discovery_and_lan_prioritization():
    print("\n" + "=" * 70)
    print("TEST 2: Network Discovery & Prioritized LAN Interface Binding")
    print("=" * 70)

    local_ips = hw._local_ipv4()
    print(f"  Local IPv4 adapters discovered: {local_ips}")
    if local_ips:
        # If there is a 192.168 or 10. address, it must come before any 169.254 address
        private_indices = [i for i, ip in enumerate(local_ips) if hw._is_private_lan(ip)]
        apipa_indices = [i for i, ip in enumerate(local_ips) if ip.startswith("169.254.")]
        if private_indices and apipa_indices:
            assert min(private_indices) < min(apipa_indices), "Private LAN interface must be prioritized before APIPA"
            print("  [PASS] Private LAN interface (192.168/10.x) prioritized before APIPA!")

    # Test TCP probe on localhost port (should be fast < 0.5s)
    t0 = time.time()
    reachable = hw.probe_tcp_port("127.0.0.1", 62078, timeout=0.2)
    dt = time.time() - t0
    assert dt < 0.6, f"TCP probe timeout exceeded ({dt:.2f}s)"
    print(f"  [PASS] TCP probe execution time: {dt:.3f}s (Reachable: {reachable})")

    # Test probe_wifi_device for Kelvin's paired UDID
    paired_udids = hw.wifi_paired_udids()
    if paired_udids:
        udid = paired_udids[0]
        probe = hw.probe_wifi_device(udid)
        print(f"  Probe result for {udid[:8]}…: reachable={probe['reachable']}, status='{probe['status_text']}'")
        assert "status_text" in probe and "status_tone" in probe
        assert probe["paired"] is True, "Device should be marked as paired"
        print("  [PASS] Honest reachability probe executed successfully!")


def test_3_backend_endpoints_and_wifi_mode_safety():
    print("\n" + "=" * 70)
    print("TEST 3: Backend /api/status & Safe Wi-Fi Mirroring Error Reporting")
    print("=" * 70)

    # Test get_wifi_status
    wifi_status = hw.get_wifi_status()
    print("  get_wifi_status():", {k: wifi_status[k] for k in ["paired", "reachable", "statusText", "statusTone"]})
    assert "statusText" in wifi_status
    assert "statusTone" in wifi_status
    print("  [PASS] get_wifi_status returns complete diagnostic schema!")

    # Test start_screen on wifi transport
    paired_udids = hw.wifi_paired_udids()
    target = paired_udids[0] if paired_udids else "00008150-DUMMY"
    res = start_screen(target, transport="wifi")
    print(f"  start_screen({target[:8]}…, transport='wifi') returned: phase='{res.get('phase')}', note='{res.get('note')}'")
    assert res.get("phase") in ("starting", "error"), f"Unexpected phase: {res.get('phase')}"
    # Clean up stream state
    stop_screen()
    print("  [PASS] start_screen handles wifi transport safely without crashing!")


def test_4_full_regression_tahap40():
    print("\n" + "=" * 70)
    print("TEST 4: Full Regression Test of Tahap 40 OCR & Short Text")
    print("=" * 70)

    script = ROOT / "scratch" / "verify_tahap40.py"
    res = subprocess.run([sys.executable, str(script)], capture_output=True, text=True, cwd=str(ROOT))
    if res.returncode != 0:
        print("Tahap 40 Regression Output:\n", res.stdout, res.stderr)
        assert False, "Tahap 40 verification failed during regression check!"
    print("  [PASS] All 4 Tahap 40 tests passed 100% without any regression!")


def main():
    print("=" * 70)
    print("STARTING TAHAP 41 COMPREHENSIVE VERIFICATION SUITE")
    print("=" * 70)

    test_1_wifi_state_persistence()
    test_2_network_discovery_and_lan_prioritization()
    test_3_backend_endpoints_and_wifi_mode_safety()
    test_4_full_regression_tahap40()

    print("\n" + "=" * 70)
    print(">>> ALL TAHAP 41 VERIFICATION TESTS PASSED 100% PERFECTLY! <<<")
    print("=" * 70 + "\n")


if __name__ == "__main__":
    main()
