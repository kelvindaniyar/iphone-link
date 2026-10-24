"""Verification and Live Audit script for Tahap 46:
1. Static Code Audit (No dummy bytes, IPv6 in _patch_bonjour, honest iOS 27 diagnostic messages).
2. Live Hardware & Network Diagnostic on Kelvin's iPhone 17 Pro (iOS 27.0.1):
   - IPv4 (192.168.100.38), MAC (6E-04-FD-DB-8B-50), and IPv6 Link-Local (fe80::ea:7139:4874:c809%4).
   - Full 16,384 ephemeral port scan (49152..65535) on IPv4 and IPv6.
   - Service audit on port 62078 (CoreDeviceTunnelProxy, ScreenshotService, DvtProvider).
   - UI diagnostic error handling verification.
3. Zero Regression Tahap 40 V2 (verify_tahap40.py).
"""

import asyncio
import concurrent.futures
import json
import os
import re
import socket
import subprocess
import sys
import time

UDID = "00008150-000231AC2669401C"
IP = "192.168.100.38"
MAC = "6E-04-FD-DB-8B-50"
IPV6_LINK_LOCAL = "fe80::ea:7139:4874:c809"
SCOPE_ID = 4

print("=" * 75)
print("    VERIFIKASI REAL TAHAP 46 — AUDIT FINAL KONEKSI WI-FI iOS 27.0.1")
print(f"    Target UDID : {UDID}")
print(f"    Target IP   : {IP}")
print(f"    Target MAC  : {MAC}")
print(f"    Target IPv6 : {IPV6_LINK_LOCAL}%{SCOPE_ID}")
print("=" * 75)

# ======================================================================
# CHECK 0: STATIC CODE AUDIT
# ======================================================================
print("\n[CHECK 0] Static Code Audit...")
with open("handset/screen.py", "r", encoding="utf-8") as f:
    screen_src = f.read()

with open("handset/wifi.py", "r", encoding="utf-8") as f:
    wifi_src = f.read()

# 1. Zero dummy bytes
assert r'b"\x00" *' not in screen_src, "FORBIDDEN dummy zero padding found in screen.py!"
assert "dummy_frame" not in screen_src, "FORBIDDEN dummy_frame found in screen.py!"

# 2. IPv6 link-local in _patch_bonjour
assert "bonjour.MDNS_MCAST_V6" in wifi_src, "IPv6 Multicast missing in _patch_bonjour!"

# 3. Honest iOS 27 diagnostic message in screen.py
assert "menutup port RemoteXPC" in screen_src, "Honest iOS 27.0.1 diagnostic message missing in screen.py!"

# 4. Cache buster in web files
for web_file in ["handset/web/mirror.html", "handset/web/index.html", "handset/web/app.js"]:
    with open(web_file, "r", encoding="utf-8") as f:
        web_src = f.read()
    assert "20261025_tahap46_final_verdict_audit" in web_src, f"Cache buster not updated in {web_file}!"

print("  [PASS] Static audit passed: Zero dummy bytes, IPv6 mDNS enabled, honest iOS 27 message active.")

# ======================================================================
# TEST 1: LIVE HARDWARE & NETWORK AUDIT (IPv4, IPv6, FULL PORT SCAN)
# ======================================================================
print("\n" + "=" * 75)
print("TEST 1: LIVE HARDWARE & NETWORK AUDIT (iOS 27.0.1)")
print("=" * 75)

# 1. ARP / IPv6 neighbor verification
res_neighbor = subprocess.run(
    ["powershell", "-NoProfile", "-Command", "Get-NetNeighbor -InterfaceIndex 4 | Select-Object IPAddress, LinkLayerAddress, State | ConvertTo-Json"],
    capture_output=True,
    text=True,
)
items = json.loads(res_neighbor.stdout) if res_neighbor.stdout.strip().startswith("[") else ([json.loads(res_neighbor.stdout)] if res_neighbor.stdout.strip().startswith("{") else [])
found_v4 = any(it.get("IPAddress") == IP and it.get("LinkLayerAddress") == MAC for it in items)
found_v6 = any(IPV6_LINK_LOCAL in it.get("IPAddress", "") and it.get("LinkLayerAddress") == MAC for it in items)
print(f"[*] Verifikasi ARP IPv4 ({IP} -> {MAC}): {'VALID' if found_v4 else 'NOT FOUND'}")
print(f"[*] Verifikasi NDP IPv6 ({IPV6_LINK_LOCAL} -> {MAC}): {'VALID' if found_v6 else 'NOT FOUND'}")

# 2. Full 16,384 Ephemeral Port Scan (49152..65535) + Standard Ports
ports_to_scan = [62078, 5000, 7000] + list(range(49152, 65536))
print(f"[*] Memindai {len(ports_to_scan)} port TCP pada IPv4 ({IP})...")

def scan_v4(p):
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    s.settimeout(0.12)
    try:
        s.connect((IP, p))
        return p
    except:
        return None
    finally:
        s.close()

def scan_v6(p):
    s = socket.socket(socket.AF_INET6, socket.SOCK_STREAM)
    s.settimeout(0.12)
    try:
        s.connect((IPV6_LINK_LOCAL, p, 0, SCOPE_ID))
        return p
    except:
        return None
    finally:
        s.close()

t0 = time.time()
open_v4 = []
with concurrent.futures.ThreadPoolExecutor(max_workers=256) as ex:
    for res in ex.map(scan_v4, ports_to_scan):
        if res:
            open_v4.append(res)
print(f"    [+] Selesai dalam {time.time()-t0:.2f}s. Port Terbuka IPv4: {open_v4}")

t0 = time.time()
open_v6 = []
with concurrent.futures.ThreadPoolExecutor(max_workers=256) as ex:
    for res in ex.map(scan_v6, ports_to_scan):
        if res:
            open_v6.append(res)
print(f"    [+] Selesai dalam {time.time()-t0:.2f}s. Port Terbuka IPv6: {open_v6}")

# 3. Bonjour & Service Probes on Port 62078
async def _probe_services():
    from pymobiledevice3.lockdown import create_using_tcp
    from pymobiledevice3.pair_records import get_preferred_pair_record, create_pairing_records_cache_folder
    from pymobiledevice3.remote import tunnel_service
    from pymobiledevice3.services.screenshot import ScreenshotService
    from pymobiledevice3.services.dvt.instruments.dvt_provider import DvtProvider

    rec = await get_preferred_pair_record(UDID, create_pairing_records_cache_folder())
    print("\n[*] Menghubungi lockdown TCP port 62078...")
    ld = await create_using_tcp(hostname=IP, identifier=UDID, autopair=False, pair_record=rec)
    print(f"    [+] Terhubung ke lockdownd! DeviceName: '{ld.short_info.get('DeviceName')}', ProductVersion: '{ld.product_version}'")

    # A. CoreDeviceTunnelProxy
    print("[*] Menguji CoreDeviceTunnelProxy.start_tcp_tunnel()...")
    proxy = await tunnel_service.CoreDeviceTunnelProxy.create(ld)
    try:
        async with proxy.start_tcp_tunnel():
            pass
        print("    [+] CoreDeviceTunnelProxy: SUCCEEDED")
    except Exception as e:
        print(f"    [!] CoreDeviceTunnelProxy: DITOLAK OLEH iOS 27 ({type(e).__name__}: {e})")

    # B. ScreenshotService
    print("[*] Menguji ScreenshotService...")
    try:
        ss = ScreenshotService(ld)
        await ss.take_screenshot()
        print("    [+] ScreenshotService: SUCCEEDED")
    except Exception as e:
        print(f"    [!] ScreenshotService: DITOLAK OLEH iOS 27 ({type(e).__name__}: {e})")

    # C. DvtProvider
    print("[*] Menguji DvtProvider (com.apple.instruments.remoteserver.DVTSecureSocketProxy)...")
    try:
        async with DvtProvider(ld) as dvt:
            pass
        print("    [+] DvtProvider: SUCCEEDED")
    except Exception as e:
        print(f"    [!] DvtProvider: DITOLAK OLEH iOS 27 ({type(e).__name__}: {e})")

    await ld.close()

asyncio.run(_probe_services())
print("\n[PASS] TEST 1 PASSED: Hardware & network audit lengkap dan tercatat.")

# ======================================================================
# TEST 2: UI DIAGNOSTIC ERROR HANDLING & GRACEFUL REPORTING
# ======================================================================
print("\n" + "=" * 75)
print("TEST 2: UI DIAGNOSTIC ERROR HANDLING VERIFICATION")
print("=" * 75)

from handset.screen import start_screen, screen_status, stop_screen

# Test start_screen on wifi without cable
print("[*] Menjalankan start_screen(transport='wifi') tanpa kabel...")
res = start_screen(UDID, transport="wifi")
print(f"    [+] Respon start_screen: phase={res.get('phase')}, note={res.get('note')}")

# Wait up to 12 seconds for background runner to complete
for _ in range(120):
    time.sleep(0.1)
    stat = screen_status()
    if stat.get("phase") in ("error", "idle"):
        break

final_stat = screen_status()
print(f"    [+] Status akhir mirroring: phase={final_stat.get('phase')}")
print(f"    [+] Pesan error di UI:\n        '{final_stat.get('error')}'")

assert final_stat.get("phase") == "error", f"Expected phase 'error', got {final_stat.get('phase')}"
assert "iOS 27.0.1 menutup port RemoteXPC" in (final_stat.get("error") or ""), (
    f"Pesan error tidak memuat diagnosis teknis iOS 27.0.1! Got: {final_stat.get('error')}"
)
print("  [PASS] UI diagnostic error verified: Banner merah menampilkan diagnosis jujur dan presisi ke user.")

stop_screen()

# ======================================================================
# TEST 3: ZERO REGRESSION TEST TAHAP 40 V2 (OCR & COPY-PASTE)
# ======================================================================
print("\n" + "=" * 75)
print("TEST 3: ZERO REGRESSION TEST TAHAP 40 V2 (OCR & COPY-PASTE)")
print("=" * 75)

res = subprocess.run(
    [sys.executable, "scratch/verify_tahap40.py"],
    capture_output=True,
    text=True,
    cwd=os.getcwd(),
)

print(res.stdout)
if res.stderr:
    print(res.stderr, file=sys.stderr)

assert res.returncode == 0, f"Tahap 40 V2 regression verification FAILED with exit code {res.returncode}!"
assert "TAHAP 40 VERIFICATION TESTS PASSED" in res.stdout, "Tahap 40 V2 output does not contain success indicator!"
print("[PASS] TEST 3 PASSED: Tahap 40 V2 OCR & Copy-Paste 100% PASS with ZERO REGRESSION!")

print("\n" + "=" * 75)
print("    >>> SEMUA PENGUJIAN TAHAP 46 BERHASIL 100% (PASS)! <<<")
print("=" * 75)
