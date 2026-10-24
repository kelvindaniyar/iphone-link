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
import threading
from datetime import datetime
from pathlib import Path
from typing import Optional

logger = logging.getLogger("handset.wifi")

BONJOUR_TIMEOUT = 2.0
_MDNS_GROUP = "224.0.0.251"
_MDNS_PORT = 5353
_bonjour_patched = False
_AUTO_ENABLED_UDIDS: set[str] = set()

WIFI_STATE_PATH = Path(__file__).resolve().parent.parent / "wifi_state.json"


# -----------------------------------------------------------------------------
# Configuration & State Management (wifi_state.json)
# -----------------------------------------------------------------------------

def load_wifi_state() -> dict:
    if not WIFI_STATE_PATH.is_file():
        return {"devices": {}}
    try:
        return json.loads(WIFI_STATE_PATH.read_text(encoding="utf-8"))
    except Exception:
        return {"devices": {}}


def save_wifi_state(state: dict) -> None:
    try:
        WIFI_STATE_PATH.write_text(json.dumps(state, indent=2), encoding="utf-8")
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
        out = subprocess.check_output(["arp", "-a"], text=True, timeout=1.5)
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
        out = subprocess.check_output(["arp", "-a"], text=True, timeout=1.0)
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
            if sock.family != socket.AF_INET:
                continue
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
        return sent

    bonjour._bind_ipv4 = bind_ipv4
    bonjour._send_query_all = send_query_all
    _bonjour_patched = True


# -----------------------------------------------------------------------------
# Auto-Enable Wireless Lockdown (Pilar 2)
# -----------------------------------------------------------------------------

async def auto_enable_wireless_lockdown(udid: str) -> dict:
    """Enable Wireless Lockdown on the device over trusted USB link and record state."""
    from pymobiledevice3.lockdown import create_using_usbmux

    lockdown = await create_using_usbmux(serial=udid, connection_type="USB", autopair=True)
    try:
        try:
            enabled = await lockdown.get_enable_wifi_connections()
        except Exception:
            enabled = False

        if not enabled:
            try:
                await lockdown.set_enable_wifi_connections(True)
                enabled = await lockdown.get_enable_wifi_connections()
            except Exception as e:
                logger.warning("Failed to set enable_wifi_connections: %s", e)

        mac = getattr(lockdown, "wifi_mac_address", None)
        device_name = getattr(lockdown, "display_name", None) or lockdown.short_info.get("DeviceName")

        # Discover real LAN IP immediately across local subnet
        ip = discover_iphone_ip_on_lan(udid)
        update_device_wifi_state(
            udid,
            enabled=enabled,
            mac=mac,
            ip=ip,
            name=device_name,
            last_seen_cable=True,
        )
        return {"ok": True, "enabled": enabled, "mac": mac, "ip": ip, "name": device_name}
    finally:
        await lockdown.close()


def trigger_auto_enable_wireless_lockdown(udid: str) -> None:
    """Non-blocking trigger to verify and enable Wireless Lockdown in the background."""
    if not udid or udid in _AUTO_ENABLED_UDIDS:
        return

    _AUTO_ENABLED_UDIDS.add(udid)

    def _worker():
        try:
            asyncio.run(auto_enable_wireless_lockdown(udid))
        except Exception as exc:
            logger.debug("Background auto_enable_wireless_lockdown failed for %s: %s", udid, exc)

    threading.Thread(target=_worker, name=f"handset-wifi-enable-{udid[:8]}", daemon=True).start()


# -----------------------------------------------------------------------------
# Wi-Fi Pairing & Reachability Probing
# -----------------------------------------------------------------------------

def wifi_paired_udids() -> list[str]:
    try:
        from pymobiledevice3.pair_records import iter_remote_paired_identifiers

        return sorted(set(iter_remote_paired_identifiers()))
    except Exception:
        return []


def pair_for_wifi(udid: str) -> dict:
    """Write the RemotePairing record and enable Wireless Lockdown over the trusted USB link."""

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

            # 3. Discover real LAN IP and record state
            ip = discover_iphone_ip_on_lan(udid)
            update_device_wifi_state(udid, enabled=True, mac=mac, ip=ip, name=device_name, last_seen_cable=True)
        finally:
            await lockdown.close()

    try:
        asyncio.run(pair())
    except Exception as error:
        return {"ok": False, "error": f"Wi-Fi setup needs the cable once: {error}"}

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


def get_wifi_status(selected_udid: Optional[str] = None) -> dict:
    """Return consolidated Wi-Fi status for /api/status endpoint."""
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


async def resolve_remote_pairing_service(serial: Optional[str], ip: str, timeout: float = 1.5):
    """Directly resolve and connect to RemotePairing service on target IP without multicast Bonjour."""
    from pymobiledevice3 import bonjour
    from pymobiledevice3.remote import tunnel_service

    discovered_port: Optional[int] = None
    loop = asyncio.get_running_loop()

    # 1. Ephemeral UDP socket to send direct Unicast DNS-SD query on (ip, 5353)
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
    except Exception as exc:
        logger.debug("Unicast DNS-SD resolution failed on %s: %s", ip, exc)

    if discovered_port:
        try:
            logger.info("Found RemotePairing port %d on %s via Unicast DNS-SD", discovered_port, ip)
            svc = await tunnel_service.create_core_device_tunnel_service_using_remotepairing(
                serial or "", ip, discovered_port, autopair=False
            )
            return svc, None
        except Exception as exc:
            logger.debug("Failed connecting to discovered RemotePairing port %d on %s: %s", discovered_port, ip, exc)

    # 2. Candidate port probes
    candidate_ports = [49152, 49153, 49154, 49155, 50000, 52341]
    for p in candidate_ports:
        try:
            s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            s.settimeout(0.15)
            s.connect((ip, p))
            s.close()
            svc = await tunnel_service.create_core_device_tunnel_service_using_remotepairing(
                serial or "", ip, p, autopair=False
            )
            logger.info("Connected to RemotePairing on port %d on %s", p, ip)
            return svc, None
        except Exception:
            continue

    return None, None


async def _wifi_provider(serial: Optional[str], autopair: bool, remotepairing_fallback: bool = True):
    from pymobiledevice3.lockdown import create_using_tcp, create_using_usbmux
    from pymobiledevice3.pair_records import create_pairing_records_cache_folder, get_preferred_pair_record
    from pymobiledevice3.remote import tunnel_service

    _patch_bonjour()

    # Ensure we know the target device's active LAN IP
    cached_info = get_device_wifi_state(serial) if serial else {}
    ip = discover_iphone_ip_on_lan(serial) or cached_info.get("last_ip")

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
    """An RSD tunnel to ``udid`` over Wi-Fi, with persistent daemon reuse."""
    from pymobiledevice3.remote import userspace_tunnel

    class WifiRsdTunnel(userspace_tunnel.UserspaceRsdTunnel):
        def __init__(self, serial: Optional[str] = None, autopair: bool = False) -> None:
            super().__init__(serial=serial, autopair=autopair)
            self._using_persistent = False

        async def aopen(self):
            persistent = get_persistent_wifi_rsd(self.serial)
            if persistent is not None:
                logger.info("Reusing Persistent Wi-Fi RSD Tunnel for %s", self.serial)
                self.rsd = persistent
                self._using_persistent = True
                return persistent

            rsd = await self._aopen_locked()
            set_persistent_wifi_rsd(self.serial, rsd)
            self._using_persistent = True
            return rsd

        async def aclose(self):
            # Keep persistent tunnel alive across user stop/start toggles!
            if self._using_persistent:
                return
            await super().aclose()

        async def _aopen_locked(self):
            original = userspace_tunnel._create_no_root_tunnel_provider
            userspace_tunnel._create_no_root_tunnel_provider = _wifi_provider
            try:
                return await super()._aopen_locked()
            finally:
                userspace_tunnel._create_no_root_tunnel_provider = original

    return WifiRsdTunnel(serial=udid, autopair=False)

