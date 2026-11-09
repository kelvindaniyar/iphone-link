"""Cable-free connection to the iPhone over the local network.

iOS 17+ announces a RemotePairing service over Bonjour on Wi-Fi. A host that
holds a RemotePairing record for the phone can open the same no-admin
userspace tunnel used for USB, just over the network. The record is created
once over USB, where the existing Trust makes it promptless.

Tahap 41:
- Auto-Enable Wireless Lockdown (EnableWifiConnections = True) when cable is plugged in.
- Local configuration & cache in wifi_state.json.
- Real Network Discovery: usbmux (Network), TCP probe port 62078, ARP MAC lookup.
- Prioritized private LAN adapter binding for mDNS queries.
"""

from __future__ import annotations

import asyncio
import concurrent.futures
import contextlib
import json
import logging
import re
import socket
import struct
import subprocess
import sys
import threading
from datetime import datetime

_WIN_NO_WINDOW_KWARGS: dict = {}
if sys.platform == "win32":
    _si = subprocess.STARTUPINFO()
    _si.dwFlags |= subprocess.STARTF_USESHOWWINDOW
    _si.wShowWindow = 0  # SW_HIDE
    _WIN_NO_WINDOW_KWARGS = {
        "creationflags": getattr(subprocess, "CREATE_NO_WINDOW", 0x08000000),
        "startupinfo": _si,
    }

from pathlib import Path
from typing import Optional

logger = logging.getLogger("handset.wifi")

BONJOUR_TIMEOUT = 2.0
_MDNS_GROUP = "224.0.0.251"
_MDNS_PORT = 5353
_bonjour_patched = False
_AUTO_ENABLED_UDIDS: set[str] = set()
_WIFI_ENABLED_UDIDS: set[str] = set()
_WIFI_ENABLED_UDIDS_PENDING: set[str] = set()
_LAN_FOUND: dict[str, dict] = {}
_LAN_LOCK = threading.Lock()
_LAN_SCANNING = threading.Event()
_LAN_LAST_SCAN = 0.0
LAN_RESCAN_S = 20.0
LAN_FORGET_S = 90.0
_VIRTUAL_ADAPTER_HINTS = ("vethernet", "virtual", "vmware", "hyper-v", "loopback", "wsl", "vpn", "tap")
_REMOTEPAIRING_PORT_CACHE: dict[str, int] = {}
_WIFI_LOCKDOWN_LOCK = threading.Lock()
_WIFI_STATE_MEM_CACHE: tuple[str, float, dict] | None = None
_PAIRED_UDIDS_MEM_CACHE: tuple[float, list[str]] | None = None

from handset import get_app_root; WIFI_STATE_PATH = get_app_root() / "wifi_state.json"


# -----------------------------------------------------------------------------
# Configuration & State Management (wifi_state.json)
# -----------------------------------------------------------------------------

def load_wifi_state() -> dict:
    global _WIFI_STATE_MEM_CACHE
    wifi_path = get_app_root() / "wifi_state.json"
    path_key = str(wifi_path)
    if not wifi_path.is_file():
        _WIFI_STATE_MEM_CACHE = (path_key, -1.0, {"devices": {}})
        return {"devices": {}}
    try:
        mtime = wifi_path.stat().st_mtime
        if (
            _WIFI_STATE_MEM_CACHE is not None
            and _WIFI_STATE_MEM_CACHE[0] == path_key
            and _WIFI_STATE_MEM_CACHE[1] == mtime
        ):
            return _WIFI_STATE_MEM_CACHE[2]
        parsed = json.loads(wifi_path.read_text(encoding="utf-8"))
        if not isinstance(parsed, dict):
            parsed = {"devices": {}}
        _WIFI_STATE_MEM_CACHE = (path_key, mtime, parsed)
        return parsed
    except Exception:
        return {"devices": {}}


def save_wifi_state(state: dict) -> None:
    global _WIFI_STATE_MEM_CACHE
    wifi_path = get_app_root() / "wifi_state.json"
    try:
        wifi_path.write_text(json.dumps(state, indent=2), encoding="utf-8")
        _WIFI_STATE_MEM_CACHE = (str(wifi_path), wifi_path.stat().st_mtime, state)
    except Exception as exc:
        logger.warning("Failed to save wifi_state.json: %s", exc)


def get_device_wifi_state(udid: str) -> dict:
    if not udid:
        return {}
    state = load_wifi_state()
    return state.get("devices", {}).get(udid, {})


def update_device_wifi_state(
    udid: str,
    *,
    enabled: Optional[bool] = None,
    ip: Optional[str] = None,
    mac: Optional[str] = None,
    name: Optional[str] = None,
    last_seen_cable: Optional[bool] = None,
    last_seen_wifi: Optional[bool] = None,
) -> dict:
    if not udid:
        return {}
    state = load_wifi_state()
    devices = state.setdefault("devices", {})
    entry = devices.setdefault(udid, {})
    now_iso = datetime.now().isoformat()

    if enabled is not None:
        entry["wifi_enabled_on_device"] = bool(enabled)
    if ip:
        entry["last_ip"] = ip
    if mac:
        entry["mac_address"] = mac.lower().replace(":", "-")
    if name:
        entry["device_name"] = name
    if last_seen_cable:
        entry["last_seen_cable"] = now_iso
    if last_seen_wifi:
        entry["last_seen_wifi"] = now_iso
    entry["updated_at"] = now_iso

    save_wifi_state(state)
    return entry


# -----------------------------------------------------------------------------
# Network Discovery Helpers (Subnet Parallel Scan, TCP Probe, ARP, Unicast mDNS)
# -----------------------------------------------------------------------------

def _is_private_lan(ip: str) -> bool:
    if not isinstance(ip, str):
        return False
    if ip.startswith(("192.168.", "10.")):
        return True
    if ip.startswith("172."):
        parts = ip.split(".")
        if len(parts) >= 2:
            try:
                second = int(parts[1])
                return 16 <= second <= 31
            except ValueError:
                pass
    return False


def _local_ipv4() -> list[str]:
    """Return local IPv4 addresses prioritizing real private LAN adapters over APIPA."""
    try:
        import ifaddr
    except ImportError:
        return []
    found: list[str] = []
    for adapter in ifaddr.get_adapters():
        for ip in adapter.ips:
            if isinstance(ip.ip, str) and not ip.ip.startswith("127."):
                found.append(ip.ip)

    lan_ips = [ip for ip in found if _is_private_lan(ip)]
    other_ips = [ip for ip in found if not _is_private_lan(ip) and not ip.startswith("169.254.")]
    apipa_ips = [ip for ip in found if ip.startswith("169.254.")]

    return lan_ips + other_ips + apipa_ips


def find_ip_by_mac(mac: str) -> Optional[str]:
    """Find local IP address matching given MAC address from Windows ARP cache."""
    if not mac:
        return None
    target = mac.lower().replace(":", "-")
    try:
        out = subprocess.check_output(["arp", "-a"], text=True, timeout=1.5, **_WIN_NO_WINDOW_KWARGS)
    except Exception:
        return None
    for line in out.splitlines():
        if target in line.lower():
            match = re.search(r"(\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3})", line)
            if match:
                return match.group(1)
    return None


def probe_tcp_port(ip: str, port: int = 62078, timeout: float = 0.35) -> bool:
    """Fast check whether target IP is actively accepting TCP connections on port 62078 (Lockdown)."""
    if not ip:
        return False
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.settimeout(timeout)
    try:
        sock.connect((ip, port))
        return True
    except OSError:
        return False
    finally:
        sock.close()


def discover_iphone_ip_on_lan(udid: Optional[str] = None, timeout: float = 0.35) -> Optional[str]:
    """Actively discovers the iPhone's LAN IPv4 address on the local /24 subnet.

    Tolerates iOS 17/18+ Private Wi-Fi MAC Address randomization (which renders
    factory MAC lookups obsolete) and multicast mDNS blocking on hidden networks.

    1. First tests last known IP from wifi_state.json (< 0.05s).
    2. Tests private IPs found in Windows ARP cache in parallel (< 0.1s).
    3. Scans all host IPs in the local /24 subnet via parallel TCP port 62078 probe (< 0.4s).
    4. Optionally sends unicast DNS-SD probe to wake up sleeping Wi-Fi radios.
    """
    cached = get_device_wifi_state(udid) if udid else {}
    last_ip = cached.get("last_ip")
    if last_ip and probe_tcp_port(last_ip, 62078, timeout=0.25):
        update_device_wifi_state(udid or "default", ip=last_ip, last_seen_wifi=True)
        return last_ip

    def _check(ip: str) -> Optional[str]:
        try:
            s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            s.settimeout(0.25)
            s.connect((ip, 62078))
            s.close()
            return ip
        except OSError:
            return None

    # 1. ARP cache candidates
    arp_ips: list[str] = []
    try:
        out = subprocess.check_output(["arp", "-a"], text=True, timeout=1.0, **_WIN_NO_WINDOW_KWARGS)
        for line in out.splitlines():
            m = re.search(r"(\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3})", line)
            if m and _is_private_lan(m.group(1)):
                arp_ips.append(m.group(1))
    except Exception:
        pass

    if arp_ips:
        with concurrent.futures.ThreadPoolExecutor(max_workers=min(len(arp_ips), 16)) as pool:
            for ip in pool.map(_check, set(arp_ips)):
                if ip:
                    if udid:
                        update_device_wifi_state(udid, ip=ip, last_seen_wifi=True)
                    return ip

    # 2. Subnet /24 parallel scan (253 candidate IPs in ~0.35s)
    lan_ips = _local_ipv4()
    primary_lan = next((ip for ip in lan_ips if _is_private_lan(ip)), None) or "192.168.100.8"
    prefix = ".".join(primary_lan.split(".")[:3]) + "."
    candidates = [f"{prefix}{i}" for i in range(1, 255) if f"{prefix}{i}" != primary_lan]
    if last_ip and last_ip in candidates:
        candidates.remove(last_ip)
        candidates.insert(0, last_ip)

    with concurrent.futures.ThreadPoolExecutor(max_workers=64) as pool:
        for ip in pool.map(_check, candidates):
            if ip:
                if udid:
                    update_device_wifi_state(udid, ip=ip, last_seen_wifi=True)
                return ip

    # 3. Radio Wake-Up Probe (Direct Unicast DNS-SD query on 5353)
    try:
        sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        sock.bind((primary_lan, 0))
        sock.settimeout(0.15)
        from pymobiledevice3 import bonjour
        query = bonjour.build_query(bonjour.REMOTEPAIRING_SERVICE_NAME, bonjour.QTYPE_PTR, unicast=True)
        for target in set(arp_ips[:10] + ([last_ip] if last_ip else [])):
            with contextlib.suppress(Exception):
                sock.sendto(query, (target, 5353))
        sock.close()
    except Exception:
        pass

    # Quick re-probe on ARP IPs after wake-up packet
    if arp_ips:
        with concurrent.futures.ThreadPoolExecutor(max_workers=min(len(arp_ips), 16)) as pool:
            for ip in pool.map(_check, set(arp_ips)):
                if ip:
                    if udid:
                        update_device_wifi_state(udid, ip=ip, last_seen_wifi=True)
                    return ip

    return None


# -----------------------------------------------------------------------------
# Tahap 132: LAN identification per UDID (parity with ZenLink `zenlink.apple.lan`)
# -----------------------------------------------------------------------------
#
# Windows often never receives mDNS replies, so the device is found by probing
# TCP 62078 (lockdown) on this PC's own private /24 subnets and asking each
# listener for its UniqueDeviceID with this PC's pair record. Unlike the older
# "first host that answers 62078" heuristic this never confuses an iPhone with
# another Apple device on the same network.


def _lan_subnets() -> list:
    import ipaddress

    try:
        import ifaddr
    except ImportError:
        return []
    nets: list = []
    for adapter in ifaddr.get_adapters():
        label = f"{getattr(adapter, 'nice_name', '') or ''} {getattr(adapter, 'name', '') or ''}".lower()
        if any(hint in label for hint in _VIRTUAL_ADAPTER_HINTS):
            continue
        for info in adapter.ips:
            if not isinstance(info.ip, str):
                continue
            try:
                addr = ipaddress.IPv4Address(info.ip)
            except ValueError:
                continue
            if not addr.is_private or addr.is_loopback or addr.is_link_local:
                continue
            net = ipaddress.IPv4Network(f"{info.ip}/{max(int(info.network_prefix), 24)}", strict=False)
            if net not in nets:
                nets.append(net)
    return nets


def _lan_pair_records(udid: Optional[str] = None) -> list[tuple[str, dict]]:
    """Trusted lockdown pair records of this PC (known UDID first, else every record on disk)."""
    import os
    import plistlib

    records: list[tuple[str, dict]] = []
    if udid:
        with contextlib.suppress(Exception):
            from pymobiledevice3.pair_records import create_pairing_records_cache_folder, get_preferred_pair_record

            rec = get_preferred_pair_record(udid, create_pairing_records_cache_folder())
            if rec:
                records.append((udid, rec))
    if not records:
        folder = Path(os.environ.get("ALLUSERSPROFILE", "C:\\ProgramData")) / "Apple" / "Lockdown"
        with contextlib.suppress(Exception):
            for plist_path in folder.glob("*.plist"):
                if plist_path.stem == "SystemConfiguration":
                    continue
                with contextlib.suppress(Exception):
                    with plist_path.open("rb") as handle:
                        records.append((plist_path.stem, plistlib.load(handle)))
    return records


async def _port_open_async(ip: str, port: int, timeout: float) -> bool:
    try:
        _reader, writer = await asyncio.wait_for(asyncio.open_connection(ip, port), timeout)
    except (OSError, asyncio.TimeoutError):
        return False
    writer.close()
    with contextlib.suppress(Exception):
        await writer.wait_closed()
    return True


async def _lan_identify(ip: str, records: list[tuple[str, dict]]) -> Optional[dict]:
    from pymobiledevice3.lockdown import create_using_tcp

    for ident, rec in records:
        lockdown = None
        try:
            lockdown = await asyncio.wait_for(
                create_using_tcp(ip, identifier=ident, autopair=False, pair_record=rec), timeout=6
            )
            values = getattr(lockdown, "all_values", None)
            if not isinstance(values, dict) or not values:
                values = await lockdown.get_value()
            if not isinstance(values, dict):
                continue
            found = str(values.get("UniqueDeviceID") or "")
            if found:
                return {
                    "udid": found,
                    "ip": ip,
                    "name": str(values.get("DeviceName") or ""),
                    "productType": str(values.get("ProductType") or ""),
                    "version": str(values.get("ProductVersion") or ""),
                }
        except Exception:
            continue
        finally:
            if lockdown is not None:
                with contextlib.suppress(Exception):
                    await lockdown.close()
    return None


async def _lan_scan(udid: Optional[str] = None) -> dict[str, dict]:
    import time as _time

    hosts = [str(h) for net in _lan_subnets() for h in net.hosts()][:1022]
    if not hosts:
        return {}
    sem = asyncio.Semaphore(128)

    async def probe(ip: str) -> Optional[str]:
        async with sem:
            return ip if await _port_open_async(ip, 62078, 0.6) else None

    open_ips = [ip for ip in await asyncio.gather(*(probe(h) for h in hosts)) if ip]
    if not open_ips:
        return {}
    records = _lan_pair_records(udid)
    if not records:
        return {}
    ident_sem = asyncio.Semaphore(6)

    async def identify(ip: str) -> Optional[dict]:
        async with ident_sem:
            return await _lan_identify(ip, records)

    found: dict[str, dict] = {}
    for info in await asyncio.gather(*(identify(ip) for ip in open_ips)):
        if info:
            found[info["udid"]] = info
    now = _time.monotonic()
    with _LAN_LOCK:
        for found_udid, info in found.items():
            _LAN_FOUND[found_udid] = {**info, "seen": now}
    for found_udid, info in found.items():
        with contextlib.suppress(Exception):
            update_device_wifi_state(found_udid, ip=info["ip"], name=info.get("name") or None, last_seen_wifi=True)
    return found


def _run_lan_scan(udid: Optional[str] = None) -> None:
    global _LAN_LAST_SCAN
    import time as _time

    try:
        asyncio.run(_lan_scan(udid))
    except Exception as exc:  # noqa: BLE001
        logger.warning("lan scan failed: %s", exc)
    finally:
        _LAN_LAST_SCAN = _time.monotonic()
        _LAN_SCANNING.clear()


def lan_host_for(udid: str, *, rescan: bool = False) -> Optional[str]:
    """LAN IPv4 of the device with this UDID (identified via lockdown, cached 90 s)."""
    import time as _time

    if not udid:
        return None
    with _LAN_LOCK:
        entry = dict(_LAN_FOUND.get(udid) or {})
    if entry and not rescan and (_time.monotonic() - entry.get("seen", 0.0)) < LAN_FORGET_S:
        return entry.get("ip")
    if entry and entry.get("ip") and probe_tcp_port(entry["ip"], 62078, timeout=0.4):
        with _LAN_LOCK:
            if udid in _LAN_FOUND:
                _LAN_FOUND[udid]["seen"] = _time.monotonic()
        return entry["ip"]
    if not _LAN_SCANNING.is_set():
        _LAN_SCANNING.set()
        _run_lan_scan(udid)
    with _LAN_LOCK:
        entry = dict(_LAN_FOUND.get(udid) or {})
    return entry.get("ip")


def lan_scan_async(udid: Optional[str] = None, *, force: bool = False) -> None:
    """Refresh the LAN cache in the background (at most every ``LAN_RESCAN_S`` seconds)."""
    import time as _time

    if _LAN_SCANNING.is_set():
        return
    if not force and (_time.monotonic() - _LAN_LAST_SCAN) < LAN_RESCAN_S:
        return
    _LAN_SCANNING.set()
    threading.Thread(target=_run_lan_scan, args=(udid,), name="lan-scan", daemon=True).start()


def lan_recently_seen(udid: str, max_age: float = LAN_FORGET_S) -> Optional[str]:
    """Cached LAN IP if the device was identified within ``max_age`` seconds (no network I/O)."""
    import time as _time

    with _LAN_LOCK:
        entry = _LAN_FOUND.get(udid)
        if entry and (_time.monotonic() - entry.get("seen", 0.0)) < max_age:
            return entry.get("ip")
    return None


def _patch_bonjour() -> None:
    """Browse mDNS on prioritized IPv4 adapters, ensuring LAN interface receives replies."""
    global _bonjour_patched
    if _bonjour_patched:
        return
    from pymobiledevice3 import bonjour

    original_bind = bonjour._bind_ipv4
    original_send = bonjour._send_query_all

    async def bind_ipv4(*args, **kwargs):
        transport, sock = await original_bind(*args, **kwargs)
        group = socket.inet_aton(_MDNS_GROUP)
        for ip in _local_ipv4():
            with contextlib.suppress(OSError):
                sock.setsockopt(
                    socket.IPPROTO_IP,
                    socket.IP_ADD_MEMBERSHIP,
                    struct.pack("=4s4s", group, socket.inet_aton(ip)),
                )
        return transport, sock

    async def send_query_all(transports, pkt, *args, **kwargs):
        sent = await original_send(transports, pkt, *args, **kwargs)
        for _, sock in transports:
            if sock.family == socket.AF_INET:
                for ip in _local_ipv4():
                    with contextlib.suppress(OSError):
                        sock.setsockopt(socket.IPPROTO_IP, socket.IP_MULTICAST_IF, socket.inet_aton(ip))
                        sock.sendto(pkt, (_MDNS_GROUP, _MDNS_PORT))
                        sent += 1
                # DIRECT UNICAST INJECTION: send query directly to known device IPs (bypasses Hidden SSID multicast filters)
                try:
                    for dev_state in get_all_device_wifi_states().values():
                        dev_ip = dev_state.get("last_ip")
                        if dev_ip and _is_private_lan(dev_ip):
                            with contextlib.suppress(OSError):
                                sock.sendto(pkt, (dev_ip, _MDNS_PORT))
                                sent += 1
                except Exception:
                    pass
            elif sock.family == socket.AF_INET6:
                # Include IPv6 Multicast queries across active wireless adapters (iOS 27 mDNS)
                with contextlib.suppress(Exception):
                    for ifindex, ifname in socket.if_nameindex():
                        if "wireless" in ifname or ifindex == 4:
                            sock.sendto(pkt, (bonjour.MDNS_MCAST_V6, bonjour.MDNS_PORT, 0, ifindex))
                            sent += 1
        return sent

    bonjour._bind_ipv4 = bind_ipv4
    bonjour._send_query_all = send_query_all
    _bonjour_patched = True


# -----------------------------------------------------------------------------
# Wi-Fi Connections (Tahap 132, parity with ZenLink "Apple Driver - Wi-Fi")
# -----------------------------------------------------------------------------
#
# Tahap 59 used to switch Wi-Fi connections off on every plug-in (to keep
# Bluetooth audio clean). That made CoreDevice Wi-Fi mirroring impossible, so it
# is gone: iPhone Link never switches Wi-Fi connections off. Whenever the phone
# is on the cable (or "Set up Wi-Fi" is pressed) the flag is turned ON.

async def enable_wifi_connections_async(udid: str, lockdown=None) -> dict:
    """Turn ON EnableWifiConnections over the trusted USB link and record the Wi-Fi state."""
    from pymobiledevice3.lockdown import create_using_usbmux

    owns_lockdown = lockdown is None
    if owns_lockdown:
        lockdown = await create_using_usbmux(serial=udid, connection_type="USB", autopair=True)
    try:
        enabled = False
        with contextlib.suppress(Exception):
            enabled = bool(await lockdown.get_enable_wifi_connections())
        if not enabled:
            try:
                await lockdown.set_enable_wifi_connections(True)
                enabled = bool(await lockdown.get_enable_wifi_connections())
            except Exception as exc:  # noqa: BLE001
                logger.warning("Failed to set enable_wifi_connections: %s", exc)

        mac = getattr(lockdown, "wifi_mac_address", None)
        device_name = None
        with contextlib.suppress(Exception):
            device_name = getattr(lockdown, "display_name", None) or (lockdown.short_info or {}).get("DeviceName")
        cached_ip = get_device_wifi_state(udid).get("last_ip")
        update_device_wifi_state(
            udid, enabled=enabled, mac=mac, ip=cached_ip, name=device_name, last_seen_cable=True
        )
        if enabled:
            with _WIFI_LOCKDOWN_LOCK:
                _WIFI_ENABLED_UDIDS.add(udid)
            lan_scan_async(udid, force=True)
        return {"ok": True, "enabled": enabled, "mac": mac, "ip": cached_ip, "name": device_name}
    finally:
        if owns_lockdown and lockdown is not None:
            with contextlib.suppress(Exception):
                await lockdown.close()


async def auto_enable_wireless_lockdown(udid: str) -> dict:
    """Enable Wireless Lockdown on the device over trusted USB link and record state."""
    return await enable_wifi_connections_async(udid)


def ensure_wifi_enabled_on_usb(udid: str, *, blocking: bool = False) -> dict:
    """Turn EnableWifiConnections ON once per plugged-in USB UDID."""
    if not udid:
        return {"ok": False, "reason": "missing_udid"}

    with _WIFI_LOCKDOWN_LOCK:
        if udid in _WIFI_ENABLED_UDIDS and not blocking:
            return {"ok": True, "udid": udid, "cached": True, "EnableWifiConnections": True}
        if udid in _WIFI_ENABLED_UDIDS_PENDING and not blocking:
            return {"ok": True, "udid": udid, "scheduled": True}
        _WIFI_ENABLED_UDIDS_PENDING.add(udid)

    def _work() -> dict:
        try:
            return asyncio.run(enable_wifi_connections_async(udid))
        except Exception as exc:  # noqa: BLE001
            logger.debug("ensure_wifi_enabled_on_usb failed for %s: %s", udid, exc)
            return {"ok": False, "udid": udid, "error": str(exc)}
        finally:
            with _WIFI_LOCKDOWN_LOCK:
                _WIFI_ENABLED_UDIDS_PENDING.discard(udid)

    if blocking:
        return _work()
    threading.Thread(target=_work, name=f"handset-wifi-on-{udid[:8]}", daemon=True).start()
    return {"ok": True, "udid": udid, "scheduled": True}


def trigger_auto_enable_wireless_lockdown(udid: str) -> None:
    """Background-enable Wi-Fi connections for a freshly plugged device (once per plug-in)."""
    ensure_wifi_enabled_on_usb(udid)


def on_usb_devices_changed(active_usb_udids: set[str]) -> None:
    """Forget per-plug state for any UDID that has been physically unplugged."""
    with _WIFI_LOCKDOWN_LOCK:
        for dead in _WIFI_ENABLED_UDIDS - set(active_usb_udids):
            _WIFI_ENABLED_UDIDS.discard(dead)


# -----------------------------------------------------------------------------
# Wi-Fi Pairing & Reachability Probing
# -----------------------------------------------------------------------------

def wifi_paired_udids() -> list[str]:
    global _PAIRED_UDIDS_MEM_CACHE
    import time as _time

    now = _time.monotonic()
    if _PAIRED_UDIDS_MEM_CACHE is not None and (now - _PAIRED_UDIDS_MEM_CACHE[0]) < 15.0:
        return list(_PAIRED_UDIDS_MEM_CACHE[1])
    try:
        from pymobiledevice3.pair_records import iter_remote_paired_identifiers

        paired = sorted(set(iter_remote_paired_identifiers()))
        _PAIRED_UDIDS_MEM_CACHE = (now, paired)
        return list(paired)
    except Exception:
        return []


def pair_for_wifi(udid: str) -> dict:
    """Write the RemotePairing record and enable Wireless Lockdown over the trusted USB link."""
    global _PAIRED_UDIDS_MEM_CACHE
    with _WIFI_LOCKDOWN_LOCK:
        _WIFI_ENABLED_UDIDS.discard(udid)

    async def pair() -> None:
        from pymobiledevice3.exceptions import RemotePairingCompletedError
        from pymobiledevice3.lockdown import create_using_usbmux
        from pymobiledevice3.remote.tunnel_service import RemotePairingLockdownService

        lockdown = await create_using_usbmux(serial=udid, connection_type="USB", autopair=True)
        try:
            # 1. Enable wireless lockdown key
            try:
                await lockdown.set_enable_wifi_connections(True)
            except Exception as e:
                logger.warning("Warning: could not set enable_wifi_connections during pair: %s", e)

            mac = getattr(lockdown, "wifi_mac_address", None)
            device_name = getattr(lockdown, "display_name", None) or lockdown.short_info.get("DeviceName")

            # 2. RemotePairing service registration
            service = await RemotePairingLockdownService.create(lockdown)
            try:
                await service.connect(autopair=True)
            except RemotePairingCompletedError:
                pass
            finally:
                await service.close()

            # 3. Discover real LAN IP (identified by UDID) and record state
            ip = await asyncio.to_thread(lan_host_for, udid, rescan=True) or discover_iphone_ip_on_lan(udid)
            update_device_wifi_state(udid, enabled=True, mac=mac, ip=ip, name=device_name, last_seen_cable=True)
        finally:
            await lockdown.close()

    try:
        asyncio.run(pair())
    except Exception as error:
        return {"ok": False, "error": f"Wi-Fi setup needs the cable once: {error}"}

    _PAIRED_UDIDS_MEM_CACHE = None
    paired_list = wifi_paired_udids()
    is_paired = udid in paired_list
    return {"ok": is_paired, "paired": paired_list, "wifi_enabled": True}


def probe_wifi_device(udid: str) -> dict:
    """Probe whether the device is truly reachable over the local network (Wi-Fi).

    Checks:
    1. usbmuxd attached devices with ConnectionType == 'Network'
    2. TCP probe to port 62078 at last known IP in wifi_state.json
    3. Active LAN Subnet Discovery (Pilar 1 - tolerates randomized MAC & hidden network)
    4. ARP cache lookup via stored Wi-Fi MAC address
    """
    paired = udid in wifi_paired_udids()
    cached = get_device_wifi_state(udid)
    enabled = cached.get("wifi_enabled_on_device", False)

    # 1. Check usbmux for Network connection
    try:
        from handset.mux import list_devices

        for d in list_devices():
            if d.get("udid") == udid and str(d.get("connection", "")).lower() in ("network", "wifi"):
                update_device_wifi_state(udid, last_seen_wifi=True)
                return {
                    "udid": udid,
                    "reachable": True,
                    "ip": cached.get("last_ip"),
                    "method": "usbmux_network",
                    "wifi_enabled": True,
                    "paired": paired,
                    "status_text": f"✓ Wi‑Fi Ready (Connected on LAN: {cached.get('last_ip') or 'Network'})",
                    "status_tone": "ok",
                    "message": "iPhone terdeteksi aktif di jaringan Wi-Fi via Apple Service. Siap mirroring.",
                }
    except Exception:
        pass

    # 2. Check TCP probe to last known IP
    last_ip = cached.get("last_ip")
    if last_ip and probe_tcp_port(last_ip, 62078, timeout=0.25):
        update_device_wifi_state(udid, last_seen_wifi=True)
        return {
            "udid": udid,
            "reachable": True,
            "ip": last_ip,
            "method": "tcp_probe",
            "wifi_enabled": True,
            "paired": paired,
            "status_text": f"✓ Wi‑Fi Ready (Connected on LAN: {last_ip})",
            "status_tone": "ok",
            "message": f"iPhone terdeteksi aktif di IP {last_ip}. Siap mirroring tanpa kabel.",
        }

    # 3. Active LAN Subnet Discovery (Pilar 1 - tolerates randomized MAC & hidden network)
    discovered_ip = discover_iphone_ip_on_lan(udid, timeout=0.35)
    if discovered_ip and probe_tcp_port(discovered_ip, 62078, timeout=0.25):
        update_device_wifi_state(udid, ip=discovered_ip, last_seen_wifi=True)
        return {
            "udid": udid,
            "reachable": True,
            "ip": discovered_ip,
            "method": "subnet_scan",
            "wifi_enabled": True,
            "paired": paired,
            "status_text": f"✓ Wi‑Fi Ready (Connected on LAN: {discovered_ip})",
            "status_tone": "ok",
            "message": f"iPhone terdeteksi aktif di IP {discovered_ip}. Siap mirroring tanpa kabel.",
        }

    # 4. Fallback ARP cache lookup by MAC address
    mac = cached.get("mac_address")
    if mac:
        found_ip = find_ip_by_mac(mac)
        if found_ip and probe_tcp_port(found_ip, 62078, timeout=0.25):
            update_device_wifi_state(udid, ip=found_ip, last_seen_wifi=True)
            return {
                "udid": udid,
                "reachable": True,
                "ip": found_ip,
                "method": "arp_probe",
                "wifi_enabled": True,
                "paired": paired,
                "status_text": f"✓ Wi‑Fi Ready (Connected on LAN: {found_ip})",
                "status_tone": "ok",
                "message": f"iPhone terdeteksi aktif di IP {found_ip}. Siap mirroring tanpa kabel.",
            }

    # If unreachable, return honest diagnosis with last known IP note
    target_ip_note = f" (IP terakhir: {last_ip})" if last_ip else ""
    if paired and enabled:
        return {
            "udid": udid,
            "reachable": False,
            "ip": last_ip,
            "method": "none",
            "wifi_enabled": True,
            "paired": True,
            "status_text": "⚠️ Wi‑Fi Paired (Pastikan iPhone menyala & di Wi‑Fi yang sama)",
            "status_tone": "warn",
            "message": (
                f"Profil pairing tersimpan{target_ip_note}, tetapi iPhone belum merespons di jaringan lokal. "
                "Pastikan layar iPhone menyala (unlocked) dan terhubung ke Wi-Fi yang sama dengan PC."
            ),
        }
    if paired and not enabled:
        return {
            "udid": udid,
            "reachable": False,
            "ip": last_ip,
            "method": "none",
            "wifi_enabled": False,
            "paired": True,
            "status_text": "🔌 Colok kabel USB 1x untuk aktifkan mode Wi‑Fi",
            "status_tone": "bad",
            "message": "Izin Wireless Lockdown belum aktif pada iPhone. Colok kabel USB 1x untuk mengaktifkannya otomatis.",
        }

    return {
        "udid": udid,
        "reachable": False,
        "ip": None,
        "method": "none",
        "wifi_enabled": False,
        "paired": False,
        "status_text": "🔌 Colok kabel USB 1x untuk aktifkan mode Wi‑Fi",
        "status_tone": "bad",
        "message": "iPhone belum pernah di-setup untuk Wi-Fi. Colokkan kabel USB dan klik 'Set up Wi-Fi'.",
    }


def get_wifi_status(selected_udid: Optional[str] = None, *, probe_network: bool = False) -> dict:
    """Return consolidated Wi-Fi status.

    Tahap 59: When `probe_network=False` (default on `GET /api/status`), NEVER execute
    network probing, ARP lookups (`arp -a`), TCP port 62078 scans, or UDP mDNS packets!
    Active LAN scanning ONLY runs when `probe_network=True` (explicit Wi-Fi Scan / Probe).
    """
    paired_list = wifi_paired_udids()
    target = selected_udid or (paired_list[0] if paired_list else None)

    if not target:
        return {
            "paired": [],
            "enabled": False,
            "reachable": False,
            "ip": None,
            "statusText": "Wi‑Fi not set up",
            "statusTone": "",
            "note": "Plug in the cable once to set up Wi‑Fi.",
        }

    if not probe_network:
        cached = get_device_wifi_state(target)
        last_ip = cached.get("last_ip")
        is_paired = target in paired_list
        wifi_enabled = bool(cached.get("wifi_enabled_on_device", False))
        if is_paired:
            # Tahap 132: refresh the LAN cache in the background (never blocks this poll).
            lan_scan_async(target)
            live_ip = lan_recently_seen(target)
            ip_note = f" (IP: {live_ip or last_ip})" if (live_ip or last_ip) else ""
            if wifi_enabled and live_ip:
                return {
                    "paired": paired_list,
                    "enabled": True,
                    "reachable": True,
                    "ip": live_ip,
                    "method": "lan-identified",
                    "statusText": "✓ Wi‑Fi siap (Terhubung di LAN)",
                    "statusTone": "ok",
                    "note": f"Mirroring CoreDevice via Wi‑Fi tanpa kabel siap{ip_note}.",
                    "udid": target,
                }
            return {
                "paired": paired_list,
                "enabled": wifi_enabled,
                "reachable": False,
                "ip": last_ip,
                "method": "cached",
                "statusText": "✓ Wi‑Fi siap" if wifi_enabled else "📶 Colok kabel sekali untuk aktifkan Wi‑Fi",
                "statusTone": "ok" if wifi_enabled else "warn",
                "note": f"Wi‑Fi CoreDevice dipasangkan. Pastikan iPhone menyala di jaringan yang sama{ip_note}.",
                "udid": target,
            }
        return {
            "paired": paired_list,
            "enabled": False,
            "reachable": False,
            "ip": last_ip,
            "method": "cached",
            "statusText": "🔌 Colok kabel USB 1x untuk aktifkan mode Wi‑Fi",
            "statusTone": "bad",
            "note": "iPhone belum pernah di-setup untuk Wi-Fi. Colokkan kabel USB dan klik 'Set up Wi-Fi'.",
            "udid": target,
        }

    probe = probe_wifi_device(target)
    return {
        "paired": paired_list,
        "enabled": probe.get("wifi_enabled", False),
        "reachable": probe.get("reachable", False),
        "ip": probe.get("ip"),
        "method": probe.get("method"),
        "statusText": probe.get("status_text"),
        "statusTone": probe.get("status_tone"),
        "note": probe.get("message"),
        "udid": target,
    }


# -----------------------------------------------------------------------------
# Multi-Stage Wi-Fi Streaming Tunnel Provider (Pilar 3)
# -----------------------------------------------------------------------------

class WifiNotFoundError(RuntimeError):
    pass


_PERSISTENT_WIFI_RSD = None
_PERSISTENT_WIFI_UDID: Optional[str] = None


def get_persistent_wifi_rsd(udid: Optional[str] = None):
    """Retrieve existing active Wi-Fi RSD tunnel if healthy and matching UDID."""
    global _PERSISTENT_WIFI_RSD, _PERSISTENT_WIFI_UDID
    if _PERSISTENT_WIFI_RSD is not None and (not udid or _PERSISTENT_WIFI_UDID == udid):
        try:
            if hasattr(_PERSISTENT_WIFI_RSD, "peer_info") and _PERSISTENT_WIFI_RSD.peer_info:
                return _PERSISTENT_WIFI_RSD
        except Exception:
            _PERSISTENT_WIFI_RSD = None
            _PERSISTENT_WIFI_UDID = None
    return None


def set_persistent_wifi_rsd(udid: str, rsd) -> None:
    """Register persistent Wi-Fi RSD tunnel so it survives user stop/start and cable disconnection."""
    global _PERSISTENT_WIFI_RSD, _PERSISTENT_WIFI_UDID
    _PERSISTENT_WIFI_RSD = rsd
    _PERSISTENT_WIFI_UDID = udid


async def force_reset_userspace_tunnel() -> None:
    """Explicitly closes any active userspace tunnel and forces module globals reset."""
    global _PERSISTENT_WIFI_RSD, _PERSISTENT_WIFI_UDID
    _PERSISTENT_WIFI_RSD = None
    _PERSISTENT_WIFI_UDID = None
    try:
        from pymobiledevice3.remote import userspace_tunnel, tunnel_service
        active = getattr(userspace_tunnel, "_active_tunnel", None)
        if active is not None:
            try:
                if hasattr(active, "aclose"):
                    await asyncio.wait_for(active.aclose(), timeout=1.5)
            except Exception as e:
                logger.debug("userspace tunnel aclose during reset failed: %s", e)
        userspace_tunnel._active_tunnel = None
        userspace_tunnel.USERSPACE_ACTIVE = False
        tunnel_service.USE_USERSPACE_TUNNEL = False

        # Release any stuck lifecycle lock on current event loop
        try:
            loop = asyncio.get_running_loop()
            locks = getattr(userspace_tunnel, "_lifecycle_locks", {})
            lock = locks.get(loop)
            if lock and lock.locked():
                with contextlib.suppress(Exception):
                    lock.release()
        except RuntimeError:
            pass
    except Exception as exc:
        logger.debug("force_reset_userspace_tunnel exception: %s", exc)


def sync_force_reset_userspace_tunnel() -> None:
    """Synchronously force resets userspace tunnel globals to prevent blocking subsequent starts."""
    global _PERSISTENT_WIFI_RSD, _PERSISTENT_WIFI_UDID
    _PERSISTENT_WIFI_RSD = None
    _PERSISTENT_WIFI_UDID = None
    try:
        from pymobiledevice3.remote import userspace_tunnel, tunnel_service
        active = getattr(userspace_tunnel, "_active_tunnel", None)
        if active is not None:
            watcher = getattr(active, "_transport_watcher", None)
            if watcher is not None and hasattr(watcher, "cancel"):
                with contextlib.suppress(Exception):
                    watcher.cancel()
            with contextlib.suppress(Exception):
                active.rsd = None
            with contextlib.suppress(Exception):
                active.tun = None
        userspace_tunnel._active_tunnel = None
        userspace_tunnel.USERSPACE_ACTIVE = False
        tunnel_service.USE_USERSPACE_TUNNEL = False
    except Exception as exc:
        logger.debug("sync_force_reset_userspace_tunnel exception: %s", exc)


def _mdns_remotepairing_ports(host: str, timeout: float = 2.5) -> list[int]:
    """RemotePairing ports `host` advertises (zeroconf receives Bonjour on Windows where pmd3's browser may not)."""
    import time as _time

    ports: list[int] = []
    try:
        from zeroconf import ServiceBrowser, Zeroconf
    except Exception:
        return ports

    names: list[str] = []
    service_type = "_remotepairing._tcp.local."

    class Listener:
        def add_service(self, _zc, _type, name):  # noqa: ANN001
            names.append(name)

        def update_service(self, *_args):  # noqa: ANN002
            return None

        def remove_service(self, *_args):  # noqa: ANN002
            return None

    zc = None
    try:
        zc = Zeroconf()
        ServiceBrowser(zc, service_type, Listener())
        _time.sleep(timeout)
        for name in list(names):
            info = zc.get_service_info(service_type, name, timeout=1500)
            if info and host in info.parsed_addresses() and info.port:
                ports.append(int(info.port))
    except Exception as exc:  # noqa: BLE001
        logger.debug("zeroconf RemotePairing browse failed: %s", exc)
    finally:
        if zc is not None:
            with contextlib.suppress(Exception):
                zc.close()
    return ports


async def _open_ports(host: str, first: int = 49152, last: int = 65535, timeout: float = 12.0) -> list[int]:
    """The RemotePairing port is dynamic: find the device's listening ports (ZenLink range scan)."""
    sem = asyncio.Semaphore(1500)

    async def probe(port: int) -> Optional[int]:
        async with sem:
            return port if await _port_open_async(host, port, 0.8) else None

    tasks = [asyncio.ensure_future(probe(p)) for p in range(first, last + 1)]
    done, pending = await asyncio.wait(tasks, timeout=timeout)
    for task in pending:
        task.cancel()
    return sorted(t.result() for t in done if not t.cancelled() and t.exception() is None and t.result())


async def _try_remotepairing(serial: Optional[str], ip: str, port: int, timeout: float = 10.0):
    from pymobiledevice3.remote import tunnel_service

    try:
        svc = await asyncio.wait_for(
            tunnel_service.create_core_device_tunnel_service_using_remotepairing(serial or "", ip, port, autopair=False),
            timeout=timeout,
        )
    except Exception as exc:  # noqa: BLE001
        logger.debug("wifi: %s:%s is not RemotePairing (%r)", ip, port, exc)
        _REMOTEPAIRING_PORT_CACHE.pop(ip, None)
        return None
    _REMOTEPAIRING_PORT_CACHE[ip] = port
    logger.info("wifi: RemotePairing at %s:%s", ip, port)
    return svc


async def resolve_remote_pairing_service(serial: Optional[str], ip: str, timeout: float = 1.5):
    """Find and open the RemotePairing service on `ip` without relying on multicast Bonjour.

    Order (parity with ZenLink): cached port -> unicast DNS-SD -> zeroconf -> well-known ports -> full
    ephemeral range scan 49152-65535.
    """
    from pymobiledevice3 import bonjour

    tried: set[int] = set()

    async def attempt(port: Optional[int]):
        if not port or port in tried:
            return None
        tried.add(port)
        return await _try_remotepairing(serial, ip, port)

    # 0. Port that worked last time
    svc = await attempt(_REMOTEPAIRING_PORT_CACHE.get(ip))
    if svc is not None:
        return svc, None

    # 1. Ephemeral UDP socket to send direct Unicast DNS-SD query on (ip, 5353)
    discovered_port: Optional[int] = None
    loop = asyncio.get_running_loop()
    try:
        sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        sock.setblocking(False)
        sock.bind(("0.0.0.0", 0))
        query = bonjour.build_query(bonjour.REMOTEPAIRING_SERVICE_NAME, bonjour.QTYPE_PTR, unicast=True)
        await loop.sock_sendto(sock, query, (ip, 5353))
        end = loop.time() + min(timeout, 1.0)
        while loop.time() < end:
            try:
                data = await asyncio.wait_for(loop.sock_recv(sock, 4096), timeout=max(0.05, end - loop.time()))
                for rr in bonjour.parse_mdns_message(data):
                    if rr.get("type") == bonjour.QTYPE_SRV and rr.get("port"):
                        discovered_port = int(rr["port"])
                        break
                if discovered_port:
                    break
            except (asyncio.TimeoutError, OSError):
                break
        sock.close()
    except Exception as exc:  # noqa: BLE001
        logger.debug("Unicast DNS-SD resolution failed on %s: %s", ip, exc)
    svc = await attempt(discovered_port)
    if svc is not None:
        return svc, None

    # 2. zeroconf browse (worker thread)
    for port in await asyncio.to_thread(_mdns_remotepairing_ports, ip, 2.5):
        svc = await attempt(port)
        if svc is not None:
            return svc, None

    # 3. Well-known ports (including the iOS 27 active port 58767)
    for port in (49152, 58767, 49153, 49154, 49155, 50000, 52341):
        if await _port_open_async(ip, port, 0.15):
            svc = await attempt(port)
            if svc is not None:
                return svc, None

    # 4. Full ephemeral range scan
    for port in await _open_ports(ip):
        svc = await attempt(port)
        if svc is not None:
            return svc, None

    return None, None


async def _wifi_provider(serial: Optional[str], autopair: bool, remotepairing_fallback: bool = True):
    from pymobiledevice3.lockdown import create_using_tcp, create_using_usbmux
    from pymobiledevice3.pair_records import create_pairing_records_cache_folder, get_preferred_pair_record
    from pymobiledevice3.remote import tunnel_service

    _patch_bonjour()

    # Ensure we know the target device's active LAN IP (identified by UDID, like ZenLink lan.py)
    cached_info = get_device_wifi_state(serial) if serial else {}
    ip = await asyncio.to_thread(lan_host_for, serial) if serial else None
    if not ip:
        ip = cached_info.get("last_ip") or await asyncio.to_thread(discover_iphone_ip_on_lan, serial)

    # STAGE 1: Direct Unicast DNS-SD & Ephemeral Scan on target IP (0% multicast dependency!)
    if ip:
        svc, ld = await resolve_remote_pairing_service(serial, ip, timeout=1.5)
        if svc is not None:
            logger.info("Wi-Fi Stage 1 SUCCESS: Direct Unicast RemotePairing on %s", ip)
            return svc, ld

    # STAGE 2: RemotePairing over Bonjour / mDNS (with unicast injection from _patch_bonjour)
    try:
        services = await tunnel_service.get_remote_pairing_tunnel_services(
            udid=serial, bonjour_timeout=2.0
        )
        if services:
            for extra in services[1:]:
                await extra.close()
            logger.info("Wi-Fi Stage 2 SUCCESS: Connected via Bonjour RemotePairing for %s", serial)
            return services[0], None
    except Exception as e:
        logger.debug("Wi-Fi Stage 2 Bonjour failed: %s", e)

    # STAGE 3: CoreDeviceTunnelProxy over usbmux Network connection
    try:
        ld_net = await create_using_usbmux(serial=serial, connection_type="Network", autopair=True)
        proxy = await tunnel_service.CoreDeviceTunnelProxy.create(ld_net)
        logger.info("Wi-Fi Stage 3 SUCCESS: CoreDeviceTunnelProxy over usbmux Network")
        return proxy, ld_net
    except Exception as e:
        logger.debug("Wi-Fi Stage 3 usbmux Network failed: %s", e)

    # STAGE 4: USB-Assisted Bootstrap / Hybrid Fallback if cable is still plugged in
    try:
        ld_usb = await create_using_usbmux(serial=serial, connection_type="USB", autopair=True)
        with contextlib.suppress(Exception):
            await ld_usb.set_enable_wifi_connections(True)
            await ld_usb.set_value(True, "com.apple.mobile.wireless_lockdown", "EnableWifiDebugging")
        proxy = await tunnel_service.CoreDeviceTunnelProxy.create(ld_usb)
        setattr(proxy, "_is_usb_fallback", True)
        logger.info("Wi-Fi Stage 4 SUCCESS: Bootstrapped via USB-Assisted Hybrid Link")
        return proxy, ld_usb
    except Exception as e:
        logger.debug("Wi-Fi Stage 4 USB bootstrap failed: %s", e)

    target_info = f" (IP terakhir: {ip})" if ip else ""
    raise WifiNotFoundError(
        f"Gagal terhubung via Wi-Fi{target_info}: iPhone tidak merespons di jaringan lokal. "
        "Pastikan layar iPhone menyala (unlocked), terhubung ke Wi-Fi yang sama dengan PC, "
        "atau colok kabel USB 1x untuk mengaktifkan izin Wi-Fi Sync."
    )


def wifi_tunnel(udid: str):
    """An RSD tunnel to ``udid`` over Wi-Fi with clean lifecycle reset."""
    from pymobiledevice3.remote import userspace_tunnel

    class WifiRsdTunnel(userspace_tunnel.UserspaceRsdTunnel):
        def __init__(self, serial: Optional[str] = None, autopair: bool = False) -> None:
            super().__init__(serial=serial, autopair=autopair)

        async def aopen(self):
            if getattr(userspace_tunnel, "_active_tunnel", None) is not None:
                await force_reset_userspace_tunnel()
                await asyncio.sleep(0.2)
            return await self._aopen_locked()

        async def aclose(self):
            try:
                await super().aclose()
            finally:
                await force_reset_userspace_tunnel()

        async def _aopen_locked(self):
            original = userspace_tunnel._create_no_root_tunnel_provider
            userspace_tunnel._create_no_root_tunnel_provider = _wifi_provider
            try:
                return await super()._aopen_locked()
            finally:
                userspace_tunnel._create_no_root_tunnel_provider = original

    return WifiRsdTunnel(serial=udid, autopair=False)

