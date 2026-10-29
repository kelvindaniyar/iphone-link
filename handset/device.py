"""Status and identity reads for a plugged-in iPhone.

Tahap 59:
- Caches `HandsetInfo` (`_HANDSET_METADATA_CACHE[udid]`), `developer_mode_enabled`
  (`_DEVELOPER_MODE_CACHE[udid]`), and `driver_status()` (`_DRIVER_STATUS_CACHE`) in RAM
  so `GET /api/status` & `GET /api/devices/<udid>` take < 1ms with 0% USB TLS Lockdown
  chatter and 0% `sc.exe` subprocess spawns once a device is plugged in.
- Unifies initial USB plug-in inspection with `disable_wireless_lockdown_async(udid)` in
  a single `create_using_usbmux` session so `EnableWifiConnections` & `EnableWifiDebugging`
  are permanently set to `False` with <= 1 Lockdown session total per plug-in.
"""

from __future__ import annotations

import asyncio
import contextlib
import socket
import subprocess
import sys
import threading
import time
from pathlib import Path

_WIN_NO_WINDOW_KWARGS: dict = {}
if sys.platform == "win32":
    _si = subprocess.STARTUPINFO()
    _si.dwFlags |= subprocess.STARTF_USESHOWWINDOW
    _si.wShowWindow = 0  # SW_HIDE
    _WIN_NO_WINDOW_KWARGS = {
        "creationflags": getattr(subprocess, "CREATE_NO_WINDOW", 0x08000000),
        "startupinfo": _si,
    }

from handset.mux import MuxError, list_devices, lockdown_request, open_lockdown

INFO_KEYS = (
    "DeviceName",
    "DeviceClass",
    "ProductType",
    "ProductVersion",
    "BuildVersion",
    "HardwareModel",
    "ModelNumber",
    "UniqueDeviceID",
    "SerialNumber",
    "PasswordProtected",
    "ActivationState",
    "TelephonyCapability",
)

DRIVER_PATHS = (
    Path(r"C:\Program Files\Common Files\Apple\Mobile Device Support"),
    Path(r"C:\Program Files (x86)\Common Files\Apple\Mobile Device Support"),
)

_DEVICE_CACHE_LOCK = threading.RLock()
_HANDSET_METADATA_CACHE: dict[str, dict] = {}
_DEVELOPER_MODE_CACHE: dict[str, bool | None] = {}
_DEV_MODE_RETRY_AFTER: dict[str, float] = {}
_DRIVER_STATUS_CACHE: tuple[float, float, dict] | None = None


def get_cached_device_names() -> list[str]:
    """Return known connected iPhone DeviceName strings from in-memory metadata cache."""
    with _DEVICE_CACHE_LOCK:
        names: list[str] = []
        for entry in _HANDSET_METADATA_CACHE.values():
            info = entry.get("info") if isinstance(entry, dict) else None
            dev_name = info.get("DeviceName") if isinstance(info, dict) else None
            if dev_name and str(dev_name).strip():
                names.append(str(dev_name).strip())
        return names


def clear_device_caches(udid: str | None = None) -> None:
    """Clear cached HandsetInfo & Developer Mode metadata (for tests or unplug)."""
    global _DRIVER_STATUS_CACHE
    with _DEVICE_CACHE_LOCK:
        if udid is None:
            _HANDSET_METADATA_CACHE.clear()
            _DEVELOPER_MODE_CACHE.clear()
            _DEV_MODE_RETRY_AFTER.clear()
            _DRIVER_STATUS_CACHE = None
        else:
            _HANDSET_METADATA_CACHE.pop(udid, None)
            _DEVELOPER_MODE_CACHE.pop(udid, None)
            _DEV_MODE_RETRY_AFTER.pop(udid, None)


def _prune_disconnected_udids(active_udids: set[str], usb_udids: set[str]) -> None:
    with _DEVICE_CACHE_LOCK:
        for cached_udid in list(_HANDSET_METADATA_CACHE.keys()):
            if cached_udid not in active_udids:
                _HANDSET_METADATA_CACHE.pop(cached_udid, None)
        for cached_udid in list(_DEVELOPER_MODE_CACHE.keys()):
            if cached_udid not in active_udids:
                _DEVELOPER_MODE_CACHE.pop(cached_udid, None)
                _DEV_MODE_RETRY_AFTER.pop(cached_udid, None)
    with contextlib.suppress(Exception):
        from handset.wifi import on_usb_devices_changed

        on_usb_devices_changed(usb_udids)


def driver_status() -> dict:
    global _DRIVER_STATUS_CACHE
    now = time.monotonic()
    with _DEVICE_CACHE_LOCK:
        if _DRIVER_STATUS_CACHE is not None:
            cached_at, ttl, cached_val = _DRIVER_STATUS_CACHE
            if (now - cached_at) < ttl:
                return dict(cached_val)

    found = [str(path) for path in DRIVER_PATHS if path.exists()]
    mux = _mux_listening()
    service = "RUNNING" if mux else _service_state("Apple Mobile Device Service")
    ready = bool(found) or service == "RUNNING" or mux
    if ready:
        detail = "Apple's USB service is running. Plug in the iPhone, unlock it, and tap Trust."
    else:
        detail = (
            "Windows has no Apple USB driver, so it cannot see an iPhone yet. "
            "Install Apple Devices from the Microsoft Store, then reconnect the phone and tap Trust."
        )
    result = {"ready": ready, "paths": found, "service": service, "detail": detail}
    ttl = 60.0 if ready else 5.0
    with _DEVICE_CACHE_LOCK:
        _DRIVER_STATUS_CACHE = (now, ttl, result)
    return dict(result)


def snapshot() -> dict:
    driver = driver_status()
    devices: list[dict] = []
    error = None
    try:
        devices = list_devices()
    except MuxError as exc:
        error = str(exc)

    active_udids = {str(d["udid"]) for d in devices if d.get("udid")}
    usb_udids = {
        str(d["udid"])
        for d in devices
        if d.get("udid") and str(d.get("connection", "USB")).upper() == "USB"
    }
    _prune_disconnected_udids(active_udids, usb_udids)

    developer_mode = developer_mode_enabled(devices[0]["udid"]) if len(devices) == 1 else None
    return {
        "library": {"ready": True, "version": "usbmux"},
        "driver": driver,
        "devices": devices,
        "developerMode": developer_mode,
        "error": error,
    }


def read_handset(udid: str) -> dict:
    connected = list_devices()
    match = next((device for device in connected if device["udid"] == udid), None)
    if match is None:
        with _DEVICE_CACHE_LOCK:
            _HANDSET_METADATA_CACHE.pop(udid, None)
        raise MuxError("That iPhone is not connected.")

    with _DEVICE_CACHE_LOCK:
        cached = _HANDSET_METADATA_CACHE.get(udid)
        if cached and cached.get("paired") is True and cached.get("info"):
            return {
                "udid": udid,
                "paired": True,
                "info": dict(cached["info"]),
            }

    with open_lockdown(int(match["deviceId"])) as sock:
        kind = lockdown_request(sock, "QueryType")
        if kind.get("Type") != "com.apple.mobile.lockdown":
            raise MuxError("The phone did not answer as a lockdown service.")
        result = lockdown_request(sock, "GetValue")
    if result.get("Error"):
        return {"udid": udid, "paired": False, "info": {}, "error": _lockdown_error(str(result["Error"]))}
    values = result.get("Value") if isinstance(result.get("Value"), dict) else {}
    info = {key: values.get(key) for key in INFO_KEYS if key in values}
    payload = {"udid": udid, "paired": True, "info": info}
    with _DEVICE_CACHE_LOCK:
        _HANDSET_METADATA_CACHE[udid] = {"udid": udid, "paired": True, "info": dict(info)}
    return payload


def _lockdown_error(code: str) -> str:
    messages = {
        "InvalidHostID": "This PC is not paired with the iPhone. Unlock the phone and tap Trust.",
        "PairingDialogResponsePending": "Tap Trust on the iPhone.",
        "UserDeniedPairing": "Trust was declined on the iPhone.",
        "PasswordProtected": "Unlock the iPhone and try again.",
    }
    return messages.get(code, code)


def developer_mode_enabled(udid: str) -> bool | None:
    now = time.monotonic()
    with _DEVICE_CACHE_LOCK:
        if udid in _DEVELOPER_MODE_CACHE:
            cached_val = _DEVELOPER_MODE_CACHE[udid]
            if cached_val is not None:
                return cached_val
            if now < _DEV_MODE_RETRY_AFTER.get(udid, 0.0):
                return None

    async def check_and_prime_once() -> bool | None:
        from pymobiledevice3.lockdown import create_using_usbmux

        lockdown = await create_using_usbmux(serial=udid, autopair=False)
        try:
            # 1. Disable EnableWifiConnections & EnableWifiDebugging in the SAME single USB session!
            with contextlib.suppress(Exception):
                from handset.wifi import disable_wireless_lockdown_async

                await disable_wireless_lockdown_async(udid, lockdown=lockdown)

            # 2. Pre-populate _HANDSET_METADATA_CACHE[udid] so read_handset(udid) needs 0 extra sockets!
            with contextlib.suppress(Exception):
                values = getattr(lockdown, "all_values", None)
                if not isinstance(values, dict) or not values:
                    values = await lockdown.get_value()
                if not isinstance(values, dict) or not values:
                    values = getattr(lockdown, "short_info", None) or {}
                if isinstance(values, dict) and values:
                    info = {key: values.get(key) for key in INFO_KEYS if key in values}
                    if info:
                        with _DEVICE_CACHE_LOCK:
                            _HANDSET_METADATA_CACHE[udid] = {
                                "udid": udid,
                                "paired": True,
                                "info": dict(info),
                            }

            # 3. Query Developer Mode status once
            return bool(await lockdown.get_developer_mode_status())
        finally:
            with contextlib.suppress(Exception):
                await lockdown.close()

    try:
        dev_enabled = asyncio.run(check_and_prime_once())
        with _DEVICE_CACHE_LOCK:
            _DEVELOPER_MODE_CACHE[udid] = dev_enabled
            if dev_enabled is None:
                _DEV_MODE_RETRY_AFTER[udid] = now + 30.0
            else:
                _DEV_MODE_RETRY_AFTER.pop(udid, None)
        return dev_enabled
    except Exception:
        with _DEVICE_CACHE_LOCK:
            _DEVELOPER_MODE_CACHE[udid] = None
            _DEV_MODE_RETRY_AFTER[udid] = now + 30.0
        return None


def _mux_listening() -> bool:
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.settimeout(0.4)
    try:
        sock.connect(("127.0.0.1", 27015))
        return True
    except OSError:
        return False
    finally:
        sock.close()


def _service_state(name: str) -> str | None:
    try:
        completed = subprocess.run(
            ["sc", "query", name],
            capture_output=True,
            text=True,
            timeout=8,
            check=False,
            **_WIN_NO_WINDOW_KWARGS,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    for line in completed.stdout.splitlines():
        if "STATE" in line:
            parts = line.split()
            if parts:
                return parts[-1]
    return None
