"""Xcode's device screen, on Windows.

After Trust, iOS 17.4+ exposes CoreDeviceProxy over the USB service broker,
or RemotePairing over Wi-Fi once paired for it. Either tunnel carries a
display service. The phone then sends the screen as HEVC video. The browser
decodes it.
"""

from __future__ import annotations

import asyncio
import threading

from handset.device import developer_mode_enabled
from handset.mux import MuxError, list_devices
from handset.wifi import wifi_paired_udids, wifi_tunnel

DEVELOPER_MODE_MESSAGE = (
    "The picture stays black because Developer Mode is off. On the iPhone open Settings, "
    "Privacy & Security, Developer Mode, turn it on, and let the phone restart. After it boots, "
    "unlock it, tap Turn On, then press Show screen again."
)

VIEW_URL = "http://127.0.0.1:8766/"
_LOCK = threading.Lock()
_STATE = {"phase": "idle", "udid": None, "url": None, "error": None, "note": None, "transport": None}
_CURRENT_TASK: asyncio.Task | None = None
_CURRENT_LOOP: asyncio.AbstractEventLoop | None = None
_ACTIVE_SERVER = None


def screen_status() -> dict:
    with _LOCK:
        return dict(_STATE)


def stop_screen() -> dict:
    """Cleanly stop mirroring, terminate stream server, and reset state."""
    global _CURRENT_TASK, _CURRENT_LOOP, _ACTIVE_SERVER
    with _LOCK:
        if _CURRENT_TASK and _CURRENT_LOOP and _CURRENT_LOOP.is_running():
            try:
                _CURRENT_LOOP.call_soon_threadsafe(_CURRENT_TASK.cancel)
            except Exception:
                pass
        _ACTIVE_SERVER = None
        _STATE.update(phase="idle", udid=None, url=None, error=None, note="Mirroring stopped.")
        return dict(_STATE)


def restart_screen(udid: str | None = None) -> dict:
    """Nudge the device encoder and keep the HTTP stream on 8766 alive."""
    import http.client

    chosen = (udid or "").strip() or _STATE.get("udid") or _only_udid()
    nudged = False
    for path in ("/restart", "/pli"):
        try:
            conn = http.client.HTTPConnection("127.0.0.1", 8766, timeout=20)
            conn.request("POST", path, body=b"", headers={})
            response = conn.getresponse()
            response.read()
            conn.close()
            nudged = True
        except OSError:
            continue
    with _LOCK:
        running = _STATE.get("phase") == "running"
    if running:
        return _set(note="Stream restarted. If the picture is still black, press Restart screen again.")
    if chosen:
        return start_screen(chosen, _STATE.get("transport") or "auto")
    return _set(error="Connect the iPhone first, then start mirroring.")


def start_screen(udid: str | None, transport: str = "auto") -> dict:
    usb = _usb_udids()
    wifi = wifi_paired_udids()
    chosen = (udid or "").strip() or _only_udid(usb, wifi, transport)
    if not chosen:
        if transport == "wifi":
            return _set(phase="idle", udid=None, url=None, error="Set up Wi-Fi once with the cable plugged in.")
        return _set(phase="idle", udid=None, url=None, error="Connect the iPhone first.")

    if transport == "auto":
        transport = "usb" if chosen in usb else "wifi"
    if transport == "usb" and chosen not in usb:
        return _set(phase="error", udid=chosen, url=None, error="The iPhone is not on the cable. Plug it in or choose Wi-Fi.")
    if transport == "wifi" and chosen not in wifi:
        return _set(phase="error", udid=chosen, url=None, error="Set up Wi-Fi once with the cable plugged in.")
    if transport == "usb" and developer_mode_enabled(chosen) is False:
        return _set(phase="error", udid=chosen, url=None, error=DEVELOPER_MODE_MESSAGE)

    with _LOCK:
        if _STATE["phase"] in {"starting", "running"} and _STATE["udid"] == chosen:
            return dict(_STATE)
        note = "Looking for the iPhone on Wi-Fi…" if transport == "wifi" else None
        _STATE.update(phase="starting", udid=chosen, url=None, error=None, note=note, transport=transport)
    threading.Thread(target=_run, args=(chosen, transport), name="handset-screen", daemon=True).start()
    return screen_status()


def _run(udid: str, transport: str) -> None:
    try:
        asyncio.run(_serve(udid, transport))
        _set(phase="idle", udid=udid, url=None, error="The screen stream stopped.")
    except Exception as error:
        _set(phase="error", udid=udid, url=None, error=_plain(error))


DISPLAY_SERVICE = "com.apple.coredevice.displayservice"


def _open_tunnel(udid: str, transport: str):
    if transport == "wifi":
        return wifi_tunnel(udid)
    from pymobiledevice3.remote.rsd_tunnel import PreferredRsdTunnel

    return PreferredRsdTunnel(serial=udid, autopair=True)


async def _serve(udid: str, transport: str) -> None:
    from pymobiledevice3.remote.core_device.screen_stream import ScreenStreamServer, _NAMED_BUTTONS
    from pymobiledevice3.services.mobile_image_mounter import auto_mount

    # HID Consumer Control (0x0C): 0x00B8 (Eject) to toggle iOS virtual keyboard cleanly
    # (Do NOT use 0x029D AL Keyboard Layout as it acts as Globe key and switches to Emoji)
    _NAMED_BUTTONS["eject"] = (12, 0x00B8, 0.05)
    _NAMED_BUTTONS.pop("keyboard-toggle", None)

    for attempt in range(2):
        async with _open_tunnel(udid, transport) as rsd:
            if DISPLAY_SERVICE in (rsd.peer_info or {}).get("Services", {}):
                await _stream(rsd, udid, ScreenStreamServer)
                return
            if attempt:
                raise RuntimeError(
                    "The developer image mounted, but the iPhone still offers no display service. "
                    "Unplug it, plug it back in, and press Show screen again."
                )
            _set(note="Mounting the developer image on the iPhone. This downloads it the first time.")
            await auto_mount(rsd)


async def _stream(rsd, udid: str, ScreenStreamServer) -> None:
    from pymobiledevice3.remote.core_device.display_service import is_media_in_use_error

    server = ScreenStreamServer(
        rsd,
        bind="127.0.0.1",
        http_port=8766,
        audio_default_on=False,
    )

    async def _eager_video_only() -> None:
        # Windows cannot decode AAC-ELD; skip eager audio so logs stay clean and
        # the display tunnel still comes up for /codec and /stream.bin.
        try:
            await server._ensure_fresh_stream(force=False)
        except Exception as error:
            if is_media_in_use_error(error):
                raise
            import logging

            logging.getLogger(__name__).warning(
                "eager video start failed (will retry on /codec)",
                exc_info=True,
            )

    server._eager_stream_start = _eager_video_only
    global _CURRENT_TASK, _CURRENT_LOOP, _ACTIVE_SERVER
    _CURRENT_LOOP = asyncio.get_running_loop()
    _CURRENT_TASK = asyncio.current_task()
    _ACTIVE_SERVER = server
    try:
        _set(phase="running", udid=udid, url=VIEW_URL, error=None, note=None)
        await server.serve()
    except asyncio.CancelledError:
        pass
    finally:
        _CURRENT_TASK = None
        _ACTIVE_SERVER = None


def send_keys_direct(usages: list[int], timeout: float = 0.5) -> bool:
    """Send keyboard HID usages directly to active server or fallback to loopback HTTP."""
    global _ACTIVE_SERVER, _CURRENT_LOOP
    server = _ACTIVE_SERVER
    loop = _CURRENT_LOOP
    if server is not None and loop is not None and loop.is_running():
        async def _send():
            await server._ensure_keyboard()
            assert server._uhs is not None and server._kb_service_id is not None
            await server._uhs.send_keyboard(server._kb_service_id, usages)
        try:
            future = asyncio.run_coroutine_threadsafe(_send(), loop)
            future.result(timeout=timeout)
            return True
        except Exception:
            pass

    with _LOCK:
        if _STATE.get("phase") != "running":
            return False

    try:
        import http.client, json
        conn = http.client.HTTPConnection("127.0.0.1", 8766, timeout=timeout)
        conn.request("POST", "/key", body=json.dumps({"usages": usages}).encode("utf-8"), headers={"Content-Type": "application/json"})
        resp = conn.getresponse()
        resp.read()
        conn.close()
        return resp.status == 200
    except Exception:
        return False


def send_button_direct(name: str, state: str = "press", timeout: float = 0.5) -> bool:
    """Send named button event directly to active server or fallback to loopback HTTP."""
    global _ACTIVE_SERVER, _CURRENT_LOOP
    server = _ACTIVE_SERVER
    loop = _CURRENT_LOOP
    if server is not None and loop is not None and loop.is_running():
        async def _send():
            from pymobiledevice3.remote.core_device.screen_stream import (
                _NAMED_BUTTONS,
                HID_BUTTON_STATE_DOWN,
                HID_BUTTON_STATE_UP,
            )
            if name not in _NAMED_BUTTONS:
                return
            usage_page, usage_code, hold_seconds = _NAMED_BUTTONS[name]
            await server._ensure_hid()
            assert server._indigo is not None
            if state == "press":
                await server._indigo.send_button(usage_page, usage_code, HID_BUTTON_STATE_DOWN)
                await asyncio.sleep(hold_seconds)
                await server._indigo.send_button(usage_page, usage_code, HID_BUTTON_STATE_UP)
            elif state == "down":
                await server._indigo.send_button(usage_page, usage_code, HID_BUTTON_STATE_DOWN)
            elif state == "up":
                await server._indigo.send_button(usage_page, usage_code, HID_BUTTON_STATE_UP)
        try:
            future = asyncio.run_coroutine_threadsafe(_send(), loop)
            future.result(timeout=timeout)
            return True
        except Exception:
            pass

    with _LOCK:
        if _STATE.get("phase") != "running":
            return False

    try:
        import http.client, json
        conn = http.client.HTTPConnection("127.0.0.1", 8766, timeout=timeout)
        conn.request("POST", "/button", body=json.dumps({"name": name, "state": state}).encode("utf-8"), headers={"Content-Type": "application/json"})
        resp = conn.getresponse()
        resp.read()
        conn.close()
        return resp.status == 200
    except Exception:
        return False


def toggle_onscreen_keyboard() -> bool:
    """Toggle iOS onscreen keyboard cleanly using HID Eject (0x0C, 0x00B8)."""
    return send_button_direct("eject")


SHORTCUT_USAGES = {
    "select-all": 0x04,  # a
    "copy": 0x06,        # c
    "paste-ios": 0x19,   # v
    "cut": 0x1B,         # x
    "undo": 0x1D,        # z
}


def trigger_ios_shortcut(action: str) -> bool:
    """Send clean sequential modifier + key reports to iOS for reliable hotkey execution."""
    import time
    key_usage = SHORTCUT_USAGES.get(action)
    if not key_usage:
        return False
    send_keys_direct([])
    time.sleep(0.01)
    send_keys_direct([0xE3])
    time.sleep(0.03)
    send_keys_direct([0xE3, key_usage])
    time.sleep(0.05)
    send_keys_direct([0xE3])
    time.sleep(0.02)
    send_keys_direct([])
    return True


_CHAR_TO_HID: dict[str, list[int]] = {}


def _init_char_map() -> None:
    global _CHAR_TO_HID
    if _CHAR_TO_HID:
        return
    # Letters a-z (0x04 - 0x1D), A-Z (Shift 0xE1 + 0x04 - 0x1D)
    for i in range(26):
        low = chr(ord('a') + i)
        up = chr(ord('A') + i)
        code = 0x04 + i
        _CHAR_TO_HID[low] = [code]
        _CHAR_TO_HID[up] = [0xE1, code]
    # Digits 1-9 (0x1E - 0x26), 0 (0x27)
    for i in range(1, 10):
        _CHAR_TO_HID[str(i)] = [0x1D + i]
    _CHAR_TO_HID['0'] = [0x27]
    # Shift digits
    shift_digits = {
        '!': 0x1E, '@': 0x1F, '#': 0x20, '$': 0x21, '%': 0x22,
        '^': 0x23, '&': 0x24, '*': 0x25, '(': 0x26, ')': 0x27,
    }
    for ch, code in shift_digits.items():
        _CHAR_TO_HID[ch] = [0xE1, code]
    # Whitespace and symbols
    symbols = {
        ' ': [0x2C], '\n': [0x28], '\t': [0x2B],
        '-': [0x2D], '_': [0xE1, 0x2D],
        '=': [0x2E], '+': [0xE1, 0x2E],
        '[': [0x2F], '{': [0xE1, 0x2F],
        ']': [0x30], '}': [0xE1, 0x30],
        '\\': [0x31], '|': [0xE1, 0x31],
        ';': [0x33], ':': [0xE1, 0x33],
        "'": [0x34], '"': [0xE1, 0x34],
        '`': [0x35], '~': [0xE1, 0x35],
        ',': [0x36], '<': [0xE1, 0x36],
        '.': [0x37], '>': [0xE1, 0x37],
        '/': [0x38], '?': [0xE1, 0x38],
    }
    _CHAR_TO_HID.update(symbols)


_init_char_map()



_TYPED_LINES_BUFFER: list[str] = []
_CURRENT_TYPED_LINE: str = ""
_TYPED_BUFFER_LOCK = threading.Lock()


def record_typed_char(char: str) -> None:
    """Record a typed character, Backspace, or Enter into the shadow buffer."""
    global _CURRENT_TYPED_LINE, _TYPED_LINES_BUFFER
    with _TYPED_BUFFER_LOCK:
        if char == "Backspace":
            _CURRENT_TYPED_LINE = _CURRENT_TYPED_LINE[:-1]
        elif char in ("\n", "\r", "Enter"):
            trimmed = _CURRENT_TYPED_LINE.strip()
            if trimmed:
                if not _TYPED_LINES_BUFFER or _TYPED_LINES_BUFFER[-1] != trimmed:
                    _TYPED_LINES_BUFFER.append(trimmed)
                if len(_TYPED_LINES_BUFFER) > 60:
                    _TYPED_LINES_BUFFER.pop(0)
            _CURRENT_TYPED_LINE = ""
        elif len(char) == 1:
            _CURRENT_TYPED_LINE += char


def record_typed_text(text: str) -> None:
    """Record a block of typed/pasted text into the shadow buffer."""
    global _CURRENT_TYPED_LINE, _TYPED_LINES_BUFFER
    if not text:
        return
    with _TYPED_BUFFER_LOCK:
        lines = text.replace("\r\n", "\n").replace("\r", "\n").split("\n")
        for line in lines:
            trimmed = line.strip()
            if trimmed:
                if not _TYPED_LINES_BUFFER or _TYPED_LINES_BUFFER[-1] != trimmed:
                    _TYPED_LINES_BUFFER.append(trimmed)
                if len(_TYPED_LINES_BUFFER) > 60:
                    _TYPED_LINES_BUFFER.pop(0)
        _CURRENT_TYPED_LINE = ""


def get_typed_candidates() -> list[str]:
    """Return all candidate lines from the typed shadow buffer."""
    with _TYPED_BUFFER_LOCK:
        cands = list(_TYPED_LINES_BUFFER)
        if _CURRENT_TYPED_LINE.strip():
            cands.append(_CURRENT_TYPED_LINE.strip())
        return cands


def auto_correct_ocr_text(ocr_text: str) -> str:
    """Fuzzy auto-correct OCR text against words/lines from the typed shadow buffer."""
    import difflib
    if not ocr_text:
        return ocr_text
    candidates = get_typed_candidates()
    if not candidates:
        return ocr_text

    ocr_lines = [l.strip() for l in ocr_text.splitlines() if l.strip()]
    if not ocr_lines:
        return ocr_text

    # 1. Full text fuzzy matching against candidate lines
    full_ocr = " ".join(ocr_lines)
    best_cand = None
    best_ratio = 0.0
    for cand in candidates:
        r = difflib.SequenceMatcher(None, full_ocr.lower(), cand.lower()).ratio()
        if r > best_ratio:
            best_ratio = r
            best_cand = cand
    if best_ratio >= 0.65 and best_cand:
        return best_cand

    # 2. Line by line fuzzy matching
    corrected_lines = []
    for line in ocr_lines:
        line_cand = line
        line_ratio = 0.0
        for cand in candidates:
            r = difflib.SequenceMatcher(None, line.lower(), cand.lower()).ratio()
            if r > line_ratio:
                line_ratio = r
                line_cand = cand
        if line_ratio >= 0.65:
            corrected_lines.append(line_cand)
        else:
            corrected_lines.append(line)

    return "\r\n".join(corrected_lines)


def type_text_to_iphone(text: str) -> int:
    """Direct HID Typing Injection: converts text into rapid HID key presses with clean twin key release."""
    import time
    if not text:
        return 0
    _init_char_map()
    record_typed_text(text)
    text = (
        text.replace("“", '"')
        .replace("”", '"')
        .replace("‘", "'")
        .replace("’", "'")
        .replace("—", "-")
        .replace("–", "-")
        .replace("\r\n", "\n")
        .replace("\r", "\n")
    )
    # Clear any active modifiers and pause cleanly before starting typing
    send_keys_direct([])
    time.sleep(0.05)
    typed_count = 0
    prev_usages = None
    for ch in text:
        usages = _CHAR_TO_HID.get(ch)
        if usages is None:
            if ch.lower() in _CHAR_TO_HID:
                usages = _CHAR_TO_HID[ch.lower()]
            else:
                continue
        # If identical to previous character (e.g. "ll" or "oo"), ensure a clean release pause
        if usages == prev_usages:
            time.sleep(0.012)
        send_keys_direct(usages)
        time.sleep(0.010)
        send_keys_direct([])
        time.sleep(0.010)
        prev_usages = usages
        typed_count += 1
    send_keys_direct([])
    return typed_count


def _usb_udids() -> list[str]:
    try:
        return [device["udid"] for device in list_devices()]
    except MuxError:
        return []


def _only_udid(usb: list[str] | None = None, wifi: list[str] | None = None, transport: str = "auto") -> str | None:
    usb = _usb_udids() if usb is None else usb
    wifi = wifi_paired_udids() if wifi is None else wifi
    if transport != "wifi" and len(usb) == 1:
        return usb[0]
    if transport != "usb" and len(wifi) == 1:
        return wifi[0]
    return None


def _set(**values) -> dict:
    with _LOCK:
        _STATE.update(values)
        return dict(_STATE)


def _plain(error: Exception) -> str:
    text = str(error).strip() or type(error).__name__
    name = type(error).__name__
    if name == "WifiNotFoundError":
        return text
    if name in {"ConnectionFailedToUsbmuxdError", "MuxError"} or "27015" in text or "usbmux" in text.lower():
        return "Apple Mobile Device Service is not running. Install Apple Devices, reconnect the iPhone, and tap Trust."
    if name == "UserspaceTunnelUnavailableError":
        return "This phone did not open the display tunnel. It needs iOS 17.4 or later, Trust, and Developer Mode."
    if "CoreDeviceProxy" in text or name == "InvalidServiceError":
        return "The phone did not offer its display tunnel. Turn on Developer Mode, unlock the phone, and tap Trust."
    if name == "DeviceNotFoundError":
        return "The iPhone is not on the cable. Plug it in, unlock it, and press Show screen again."
    if name in {"DeveloperModeIsNotEnabledError", "DeveloperModeError"}:
        return DEVELOPER_MODE_MESSAGE
    if "PasswordProtected" in text or "locked" in text.lower():
        return "Unlock the iPhone and keep it unlocked, then press Show screen again."
    if "PairingDialog" in text or "Trust" in text:
        return "Unlock the iPhone and tap Trust, then show the screen again."
    return text
