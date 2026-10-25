"""Status and identity reads for a plugged-in iPhone."""

from __future__ import annotations

import asyncio
import socket
import subprocess
import sys
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


def driver_status() -> dict:
    found = [str(path) for path in DRIVER_PATHS if path.exists()]
    service = _service_state("Apple Mobile Device Service")
    mux = _mux_listening()
    ready = bool(found) or service == "RUNNING" or mux
    if ready:
        detail = "Apple's USB service is running. Plug in the iPhone, unlock it, and tap Trust."
    else:
        detail = (
            "Windows has no Apple USB driver, so it cannot see an iPhone yet. "
            "Install Apple Devices from the Microsoft Store, then reconnect the phone and tap Trust."
        )
    return {"ready": ready, "paths": found, "service": service, "detail": detail}


def snapshot() -> dict:
    driver = driver_status()
    devices: list[dict] = []
    error = None
    try:
        devices = list_devices()
    except MuxError as exc:
        error = str(exc)
    developer_mode = developer_mode_enabled(devices[0]["udid"]) if len(devices) == 1 else None
    return {
        "library": {"ready": True, "version": "usbmux"},
        "driver": driver,
        "devices": devices,
        "developerMode": developer_mode,
        "error": error,
    }


def read_handset(udid: str) -> dict:
    match = next((device for device in list_devices() if device["udid"] == udid), None)
    if match is None:
        raise MuxError("That iPhone is not connected.")
    with open_lockdown(int(match["deviceId"])) as sock:
        kind = lockdown_request(sock, "QueryType")
        if kind.get("Type") != "com.apple.mobile.lockdown":
            raise MuxError("The phone did not answer as a lockdown service.")
        result = lockdown_request(sock, "GetValue")
    if result.get("Error"):
        return {"udid": udid, "paired": False, "info": {}, "error": _lockdown_error(str(result["Error"]))}
    values = result.get("Value") if isinstance(result.get("Value"), dict) else {}
    info = {key: values.get(key) for key in INFO_KEYS if key in values}
    return {"udid": udid, "paired": True, "info": info}


def _lockdown_error(code: str) -> str:
    messages = {
        "InvalidHostID": "This PC is not paired with the iPhone. Unlock the phone and tap Trust.",
        "PairingDialogResponsePending": "Tap Trust on the iPhone.",
        "UserDeniedPairing": "Trust was declined on the iPhone.",
        "PasswordProtected": "Unlock the iPhone and try again.",
    }
    return messages.get(code, code)


def developer_mode_enabled(udid: str) -> bool | None:
    async def check() -> bool:
        from pymobiledevice3.lockdown import create_using_usbmux

        lockdown = await create_using_usbmux(serial=udid, autopair=False)
        try:
            return bool(await lockdown.get_developer_mode_status())
        finally:
            await lockdown.close()

    try:
        return asyncio.run(check())
    except Exception:
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
