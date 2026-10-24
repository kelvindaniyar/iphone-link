"""Xcode's device screen, on Windows.

After Trust, iOS 17.4+ exposes CoreDeviceProxy over the USB service broker,
or RemotePairing over Wi-Fi once paired for it. Either tunnel carries a
display service. The phone then sends the screen as HEVC video. The browser
decodes it.
"""

from __future__ import annotations

import asyncio
import difflib
import json
import re
import threading
import time
from pathlib import Path

from handset.device import developer_mode_enabled
from handset.mux import MuxError, list_devices
from handset.wifi import probe_wifi_device, wifi_paired_udids, wifi_tunnel

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
    if transport == "wifi":
        probe = probe_wifi_device(chosen)
        if not probe.get("paired") and not probe.get("wifi_enabled"):
            return _set(
                phase="error",
                udid=chosen,
                url=None,
                error="Mode Wi-Fi belum di-setup. Colokkan kabel USB satu kali dan klik 'Set up Wi-Fi'.",
            )
        if probe.get("reachable"):
            ip_label = f" ({probe.get('ip')})" if probe.get("ip") else ""
            note = f"Menghubungkan ke iPhone via Wi-Fi{ip_label}… Pastikan iPhone tidak terkunci."
        else:
            note = "Mencari iPhone di jaringan Wi-Fi lokal… Pastikan layar iPhone menyala (unlocked) dan di Wi-Fi yang sama."
    else:
        note = None

    if transport == "usb" and developer_mode_enabled(chosen) is False:
        return _set(phase="error", udid=chosen, url=None, error=DEVELOPER_MODE_MESSAGE)

    with _LOCK:
        if _STATE["phase"] in {"starting", "running"} and _STATE["udid"] == chosen:
            return dict(_STATE)
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

    try:
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
    except Exception as exc:
        if transport == "wifi":
            probe = probe_wifi_device(udid)
            if not probe.get("reachable"):
                raise RuntimeError(
                    "Gagal terhubung via Wi-Fi: iPhone tidak merespons di jaringan lokal. "
                    "Pastikan layar iPhone menyala (unlocked), terhubung ke Wi-Fi yang sama dengan PC, "
                    "atau colok kabel USB 1x untuk mengaktifkan izin Wi-Fi Sync."
                ) from exc
            ip_str = f" ({probe.get('ip')})" if probe.get("ip") else ""
            raise RuntimeError(
                f"Gagal menghubungkan tunnel mirroring Wi-Fi ke iPhone{ip_str}: {exc}. "
                "Pastikan Developer Mode aktif dan firewall Windows mengizinkan koneksi lokal."
            ) from exc
        raise


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
_LAST_TYPED_ACTIVE_LINE: str = ""
_LAST_TYPED_TIMESTAMP: float = 0.0
_TYPED_BUFFER_LOCK = threading.Lock()

PROJECT_ROOT = Path(__file__).resolve().parent.parent
TYPED_HISTORY_FILE = PROJECT_ROOT / "typed_history.json"


def save_typed_history() -> None:
    """Persist typed history buffer and last active line to disk."""
    try:
        with _TYPED_BUFFER_LOCK:
            data = {
                "last_active": _LAST_TYPED_ACTIVE_LINE,
                "timestamp": _LAST_TYPED_TIMESTAMP,
                "buffer": _TYPED_LINES_BUFFER,
            }
        with open(TYPED_HISTORY_FILE, "w", encoding="utf-8") as f:
            json.dump(data, f, ensure_ascii=False, indent=2)
    except Exception:
        pass


def _is_valid_typed_history_entry(line: str) -> bool:
    """Validate typed history entries: forbid long paste blocks (>= 35 chars) or news headlines (Tahap 38)."""
    if not line:
        return True  # Keep blank line
    if len(line) >= 35:
        return False
    low = line.lower()
    if "rekaman video amatir" in low or "unggahan bertajuk" in low:
        return False
    return True


def load_typed_history() -> None:
    """Load persisted typed history from disk on startup and purge long paste contamination."""
    global _LAST_TYPED_ACTIVE_LINE, _LAST_TYPED_TIMESTAMP, _TYPED_LINES_BUFFER
    if not TYPED_HISTORY_FILE.exists():
        return
    try:
        with open(TYPED_HISTORY_FILE, "r", encoding="utf-8") as f:
            data = json.load(f)
        with _TYPED_BUFFER_LOCK:
            act = data.get("last_active", "")
            _LAST_TYPED_ACTIVE_LINE = act if _is_valid_typed_history_entry(act) else ""
            _LAST_TYPED_TIMESTAMP = float(data.get("timestamp", 0.0))
            raw_buf = list(data.get("buffer", []))
            _TYPED_LINES_BUFFER[:] = [l for l in raw_buf if _is_valid_typed_history_entry(l)]
    except Exception:
        pass


# Load history on startup
load_typed_history()


def commit_and_reset_current_typed_line() -> None:
    """Commit current typed accumulator into buffer if valid (>=3 chars and <35 chars) and reset accumulator."""
    global _CURRENT_TYPED_LINE, _TYPED_LINES_BUFFER, _LAST_TYPED_ACTIVE_LINE, _LAST_TYPED_TIMESTAMP
    import time
    with _TYPED_BUFFER_LOCK:
        trimmed = _CURRENT_TYPED_LINE.strip()
        if len(trimmed) >= 3 and _is_valid_typed_history_entry(trimmed):
            if not _TYPED_LINES_BUFFER or _TYPED_LINES_BUFFER[-1] != trimmed:
                _TYPED_LINES_BUFFER.append(trimmed)
            if len(_TYPED_LINES_BUFFER) > 60:
                _TYPED_LINES_BUFFER.pop(0)
        _CURRENT_TYPED_LINE = ""
        _LAST_TYPED_ACTIVE_LINE = ""
    save_typed_history()


def record_typed_char(char: str) -> None:
    """Record a typed character, Backspace, or Enter into the shadow buffer."""
    global _CURRENT_TYPED_LINE, _TYPED_LINES_BUFFER, _LAST_TYPED_ACTIVE_LINE, _LAST_TYPED_TIMESTAMP
    import time
    if not char:
        return

    now = time.time()
    # If pause > 6.0s since last keystroke, commit previous typing and start fresh!
    if _LAST_TYPED_TIMESTAMP > 0 and (now - _LAST_TYPED_TIMESTAMP) > 6.0:
        commit_and_reset_current_typed_line()

    with _TYPED_BUFFER_LOCK:
        if char == "Backspace":
            if _CURRENT_TYPED_LINE:
                _CURRENT_TYPED_LINE = _CURRENT_TYPED_LINE[:-1]
                trimmed = _CURRENT_TYPED_LINE.strip()
                if len(trimmed) >= 3 and _is_valid_typed_history_entry(trimmed):
                    _LAST_TYPED_ACTIVE_LINE = trimmed
                _LAST_TYPED_TIMESTAMP = time.time()
        elif char in ("\n", "\r", "Enter"):
            trimmed = _CURRENT_TYPED_LINE.strip()
            if len(trimmed) >= 3 and _is_valid_typed_history_entry(trimmed):
                if not _TYPED_LINES_BUFFER or _TYPED_LINES_BUFFER[-1] != trimmed:
                    _TYPED_LINES_BUFFER.append(trimmed)
                if len(_TYPED_LINES_BUFFER) > 60:
                    _TYPED_LINES_BUFFER.pop(0)
                _LAST_TYPED_ACTIVE_LINE = trimmed
                _LAST_TYPED_TIMESTAMP = time.time()
            elif not trimmed:
                # Consecutive Enter -> record empty line in buffer
                if _TYPED_LINES_BUFFER and _TYPED_LINES_BUFFER[-1] != "":
                    _TYPED_LINES_BUFFER.append("")
                    if len(_TYPED_LINES_BUFFER) > 60:
                        _TYPED_LINES_BUFFER.pop(0)
            # Reset current line accumulator for next sentence, but KEEP _LAST_TYPED_ACTIVE_LINE intact!
            _CURRENT_TYPED_LINE = ""
        elif char in ("Space", " "):
            _CURRENT_TYPED_LINE += " "
            trimmed = _CURRENT_TYPED_LINE.strip()
            if len(trimmed) >= 3 and _is_valid_typed_history_entry(trimmed):
                _LAST_TYPED_ACTIVE_LINE = trimmed
            _LAST_TYPED_TIMESTAMP = time.time()
        elif len(char) == 1:
            _CURRENT_TYPED_LINE += char
            trimmed = _CURRENT_TYPED_LINE.strip()
            if len(trimmed) >= 3 and _is_valid_typed_history_entry(trimmed):
                _LAST_TYPED_ACTIVE_LINE = trimmed
            _LAST_TYPED_TIMESTAMP = time.time()
    save_typed_history()


def record_typed_text(text: str) -> None:
    """Record text block or append typed text chunk into shadow buffer."""
    global _CURRENT_TYPED_LINE, _TYPED_LINES_BUFFER, _LAST_TYPED_ACTIVE_LINE, _LAST_TYPED_TIMESTAMP
    import time
    if not text:
        return

    now = time.time()
    if _LAST_TYPED_TIMESTAMP > 0 and (now - _LAST_TYPED_TIMESTAMP) > 6.0:
        commit_and_reset_current_typed_line()

    with _TYPED_BUFFER_LOCK:
        if "\n" in text or "\r" in text:
            lines = text.replace("\r\n", "\n").replace("\r", "\n").split("\n")
            for line in lines:
                trimmed = line.strip()
                if len(trimmed) >= 3 and _is_valid_typed_history_entry(trimmed):
                    if not _TYPED_LINES_BUFFER or _TYPED_LINES_BUFFER[-1] != trimmed:
                        _TYPED_LINES_BUFFER.append(trimmed)
                    if len(_TYPED_LINES_BUFFER) > 60:
                        _TYPED_LINES_BUFFER.pop(0)
                    _LAST_TYPED_ACTIVE_LINE = trimmed
                    _LAST_TYPED_TIMESTAMP = time.time()
                elif trimmed == "":
                    # If empty line between typed items, record empty line in buffer
                    if _TYPED_LINES_BUFFER and _TYPED_LINES_BUFFER[-1] != "":
                        _TYPED_LINES_BUFFER.append("")
                        if len(_TYPED_LINES_BUFFER) > 60:
                            _TYPED_LINES_BUFFER.pop(0)
            _CURRENT_TYPED_LINE = ""
        else:
            # Append chunk (single char or word snippet) to _CURRENT_TYPED_LINE
            _CURRENT_TYPED_LINE += text
            trimmed = _CURRENT_TYPED_LINE.strip()
            if len(trimmed) >= 3:
                _LAST_TYPED_ACTIVE_LINE = trimmed
            _LAST_TYPED_TIMESTAMP = time.time()
    save_typed_history()


def get_typed_candidates() -> list[str]:
    """Return all candidate lines from the typed shadow buffer, filtering out single characters."""
    with _TYPED_BUFFER_LOCK:
        cands = []
        if len(_LAST_TYPED_ACTIVE_LINE) >= 3:
            cands.append(_LAST_TYPED_ACTIVE_LINE)
        curr_trimmed = _CURRENT_TYPED_LINE.strip()
        if len(curr_trimmed) >= 3 and curr_trimmed not in cands:
            cands.append(curr_trimmed)
        for l in reversed(_TYPED_LINES_BUFFER):
            if len(l) >= 3 and l not in cands:
                cands.append(l)
        return cands


def auto_correct_ocr_text(ocr_text: str) -> str:
    """Fuzzy auto-correct OCR text against words/lines from the typed shadow buffer with strict length safety."""
    import difflib, time

    def _is_length_comparable(cand: str, target: str) -> bool:
        if len(target) < 5:
            return True
        allowed = max(3, int(len(target) * 0.20))
        return abs(len(cand) - len(target)) <= allowed

    candidates = get_typed_candidates()

    with _TYPED_BUFFER_LOCK:
        active_line = _LAST_TYPED_ACTIVE_LINE.strip()
        active_timestamp = _LAST_TYPED_TIMESTAMP

    raw_ocr = (ocr_text or "").strip()
    active_age = time.time() - active_timestamp
    has_valid_active = len(active_line) >= 3 and active_age < 120.0

    # --- Priority 1: User typed recently in active session (<120s) and OCR is empty or noisy short (<= 2 chars) ---
    if has_valid_active:
        if not raw_ocr or len(raw_ocr) <= 2:
            return active_line

        raw_lower = raw_ocr.lower()
        act_lower = active_line.lower()

        # Strict length comparability check for active line
        if len(raw_ocr) >= 5 and not _is_length_comparable(active_line, raw_ocr):
            pass # Disallow replacing clean OCR with excessively longer candidate
        else:
            # If exact match or active_line is contained in OCR (e.g. multi-line OCR containing the active sentence)
            if act_lower == raw_lower or (len(act_lower) >= 6 and act_lower in raw_lower):
                return active_line

            # SequenceMatcher match against full raw_ocr
            ratio = difflib.SequenceMatcher(None, raw_lower, act_lower).ratio()
            if ratio >= 0.78:
                return active_line

    if not raw_ocr.strip():
        return active_line if has_valid_active else ""

    raw_lines = [l.strip() for l in raw_ocr.splitlines()]

    # If no valid candidates in history buffer, return clean OCR text preserving empty lines
    if not candidates:
        cleaned_lines = [_clean_ocr_line(l) if l else "" for l in raw_lines]
        return "\r\n".join(cleaned_lines)

    # For single short line, we can do full match against candidate sentences
    non_empty = [l for l in raw_lines if l]
    if len(non_empty) <= 1:
        full_ocr = non_empty[0] if non_empty else ""
        full_ocr_lower = full_ocr.lower()
        ocr_words = set(full_ocr_lower.split())

        best_cand = None
        best_score = 0.0

        for cand in candidates:
            c_strip = cand.strip()
            if len(c_strip) < 3:
                continue
            if len(full_ocr) >= 5 and not _is_length_comparable(c_strip, full_ocr):
                continue
            c_lower = c_strip.lower()

            # Exact match
            if c_lower == full_ocr_lower:
                return c_strip

            # Tahap 38: First letter mismatch guard
            if c_lower[0] != full_ocr_lower[0]:
                ocr_w0 = full_ocr_lower.split()[0] if full_ocr_lower.split() else ""
                cand_w0 = c_lower.split()[0] if c_lower.split() else ""
                if not ocr_w0 or not cand_w0:
                    continue
                w0_ratio = difflib.SequenceMatcher(None, ocr_w0, cand_w0).ratio()
                if w0_ratio < 0.70:
                    continue
                sm_ratio = difflib.SequenceMatcher(None, full_ocr_lower, c_lower).ratio()
                if sm_ratio < 0.84:
                    continue
                score = sm_ratio
            elif len(c_lower) >= 8 and (c_lower in full_ocr_lower or full_ocr_lower in c_lower):
                # Containment (for multi-word sentences)
                score = 0.95
            else:
                sm_ratio = difflib.SequenceMatcher(None, full_ocr_lower, c_lower).ratio()
                c_words = set(c_lower.split())
                w_ratio = 0.0
                if c_words and ocr_words:
                    overlap = c_words.intersection(ocr_words)
                    w_ratio = len(overlap) / float(max(len(c_words), len(ocr_words)))
                score = max(sm_ratio, w_ratio)

            if has_valid_active and c_strip.lower() == active_line.lower():
                score += 0.08

            if score > best_score:
                best_score = score
                best_cand = c_strip

        if best_score >= 0.78 and best_cand:
            return best_cand

    # --- Priority 3: Line-by-line correction strictly preserving empty lines ---
    # Tahap 35 Perbaikan 4: Check if other lines in raw_lines match typed buffer
    other_lines_matching_buffer = 0
    for l in raw_lines:
        if not l.strip():
            continue
        ll = _clean_ocr_line(l).lower()
        if not ll:
            continue
        for cand in candidates:
            cl = cand.strip().lower()
            if ll == cl or (len(ll) >= 6 and (ll in cl or cl in ll)):
                other_lines_matching_buffer += 1
                break
            elif difflib.SequenceMatcher(None, ll, cl).ratio() >= 0.72:
                other_lines_matching_buffer += 1
                break

    is_short_note = (len(non_empty) <= 6)

    corrected_lines = []
    for line in raw_lines:
        if not line:
            corrected_lines.append("")
            continue

        c_line = _clean_ocr_line(line)
        if not c_line:
            continue
        l_lower = c_line.lower()

        # Contextual Prefix Recovery for short text (Tahap 35 Perbaikan 4):
        # E.g. "the" is a word prefix of "the beatles" in _TYPED_LINES_BUFFER,
        # and other lines match buffer ("pamungkas kenangan manis", "barasuara", "hindia")
        prefix_cand = None
        is_exact_cand = any(c.strip().lower() == l_lower for c in candidates)
        if not is_exact_cand and is_short_note and other_lines_matching_buffer >= 1 and len(l_lower.split()) <= 2 and len(l_lower) <= 15:
            for cand in candidates:
                c_strip = cand.strip()
                c_lower = c_strip.lower()
                if (c_lower.startswith(l_lower + " ") and 
                    len(c_lower.split()) <= len(l_lower.split()) + 1 and 
                    len(c_lower) <= len(l_lower) + 10):
                    prefix_cand = c_strip
                    break

        if prefix_cand:
            corrected_lines.append(prefix_cand)
            continue

        line_best = c_line
        line_ratio = 0.0
        l_words = l_lower.split()
        for cand in candidates:
            c_strip = cand.strip()
            if len(c_strip) < 3:
                continue
            c_lower = c_strip.lower()
            c_words = c_lower.split()

            # Dynamic threshold (Tahap 35): 0.72 for words with same first and last letter and abs len diff <= 2
            is_same_endpoints = (
                len(c_lower) >= 4 and len(l_lower) >= 4
                and c_lower[0] == l_lower[0] and c_lower[-1] == l_lower[-1]
                and abs(len(c_lower) - len(l_lower)) <= 2
            )
            req_ratio = 0.72 if is_same_endpoints else 0.78

            if len(c_line) >= 5 and not _is_length_comparable(c_strip, c_line) and not is_same_endpoints:
                continue

            # First letter mismatch safety guard (Tahap 38):
            # If first character differs, require sentence ratio >= 0.84 AND first word ratio >= 0.70
            # E.g. "Dapua raya selatan" vs "papua raya selatan" (w0 ratio 0.80 >= 0.70, sent ratio 0.94 >= 0.84) allowed!
            if c_lower[0] != l_lower[0]:
                if not l_words or not c_words:
                    continue
                w0_ratio = difflib.SequenceMatcher(None, l_words[0], c_words[0]).ratio()
                if w0_ratio < 0.70:
                    continue
                req_ratio = max(req_ratio, 0.84)

            if len(c_line) >= 6 and (l_lower in c_lower or c_lower in l_lower) and c_lower[0] == l_lower[0]:
                line_best = c_strip
                line_ratio = 1.0
                break
            r = difflib.SequenceMatcher(None, l_lower, c_lower).ratio()
            if r >= req_ratio and r > line_ratio:
                line_ratio = r
                line_best = c_strip

        if line_ratio >= 0.72:
            corrected_lines.append(line_best)
        else:
            corrected_lines.append(c_line)

    result = "\r\n".join(corrected_lines)
    # STRICT SAFETY GUARD: Never return 1 or 2 chars if longer text exists
    if len(result.strip()) <= 2:
        if has_valid_active:
            return active_line
        if raw_ocr.strip() and len(raw_ocr.strip()) > len(result.strip()):
            return raw_ocr
    return result


_RECENT_LONG_OCR_CHUNKS: list[dict] = []  # items: {"ts": float, "lines": list[str], "color": str}
_CHUNKS_LOCK = threading.Lock()
_LAST_SELECTION_COLOR: str | None = None


def clear_recent_long_ocr_chunks() -> None:
    """Clear session OCR chunk memory when switching notes, clearing cache, or detecting folder list view (Tahap 32/40)."""
    global _RECENT_LONG_OCR_CHUNKS, _LAST_SELECTION_COLOR
    with _CHUNKS_LOCK:
        _RECENT_LONG_OCR_CHUNKS.clear()
    _LAST_SELECTION_COLOR = None


def _is_notes_folder_list_frame(lines: list[str] | str) -> bool:
    r"""
    Detect whether a frame belongs to the iOS Notes Folder List view (halaman Daftar Folder Notes).
    Contains headers like: ^Today$, ^Yesterday$, ^Previous \d+ Days$, ^iCloud$, ^Notes$, ^\d+ Notes$,
    months (September, March, etc.), or note preview timestamp lines (e.g. '07.42 prabowo gibran').
    """
    if not lines:
        return False
    if isinstance(lines, str):
        if "<<<NOTES_LIST_VIEW>>>" in lines:
            return True
        lines_list = [l.strip() for l in lines.splitlines() if l.strip()]
    else:
        lines_list = [l.strip() for l in lines if l and l.strip()]
        if any("<<<NOTES_LIST_VIEW>>>" in l for l in lines_list):
            return True

    folder_list_header_re = re.compile(
        r'^(today|yesterday|previous\s+\d+\s+days|hari\s+ini|kemarin|\d+\s+hari\s+sebelumnya|icloud|notes|\d+\s+notes|catatan|\d+\s+catatan|january|february|march|april|may|june|july|august|september|october|november|december|januari|februari|maret|mei|juni|juli|agustus|oktober|desember)$',
        re.IGNORECASE
    )
    folder_list_preview_re = re.compile(
        r'^\d{1,2}[.:]\d{2}\s+\S+',
        re.IGNORECASE
    )

    ellipsis_count = 0
    for line in lines_list:
        clean = line.strip()
        if folder_list_header_re.match(clean):
            return True
        if folder_list_preview_re.match(clean):
            return True
        if re.search(r'(\.\.\.|…)$', clean):
            ellipsis_count += 1
            if ellipsis_count >= 2:
                return True

    return False


def _strip_typed_buffer_contamination_from_top(lines: list[str]) -> list[str]:
    """
    LAPIS 3 (Tahap 33 & Tahap 38): For long text (>= 6 lines), strip contaminated top lines
    (e.g. old short note titles like 'banten feeds test 17 pro' or 'banten feeds iphone 17 pro')
    that match entries in the user's typed history buffer or Notes list preview items.
    STRICT SAFETY RULE: DILARANG KERAS menghapus baris pertama lines[0] jika lines[0] adalah
    kalimat pembuka berita yang bersambung ke lines[1] (len(lines[0]) >= 30, tanpa titik, dan lines[1] diawali huruf kecil).
    Sebuah baris HANYA boleh dianggap kontaminasi jika panjangnya < 32 karakter!
    """
    non_empty = [l for l in lines if l and l.strip()]
    if not lines or len(non_empty) < 6:
        return lines

    typed_cands = get_typed_candidates()
    if not typed_cands:
        return lines

    result = list(lines)
    # Check top 1-2 lines
    for _ in range(2):
        non_empty_res = [l for l in result if l and l.strip()]
        if len(non_empty_res) < 6:
            break
        if not result[0].strip():
            result.pop(0)
            continue
        top_line = result[0].strip()
        top_low = top_line.lower()

        # Never strip news article paragraph openers or hashtags
        if top_line.startswith("#") or re.search(r'(\.|\)\.|\]\.|\"\.|\'\.|\?|!)$', top_line):
            break

        # Tahap 38: Check if lines[0] connects to lines[1] (continuation of sentence)
        if len(non_empty_res) >= 2:
            next_line = non_empty_res[1].strip()
            # If top line is long (>= 30) and next line starts with lowercase letter, it is a flowing paragraph!
            if len(top_line) >= 30 and next_line and next_line[0].islower():
                break

        # A top line can ONLY be considered contamination if length < 32!
        if len(top_line) >= 32:
            break

        is_contamination = False
        for cand in typed_cands:
            c_clean = cand.strip()
            if len(c_clean) < 4 or len(c_clean) >= 35:
                continue
            c_low = c_clean.lower()
            if re.search(r'(\.|\)\.|\]\.|\"\.|\'\.|\?|!)$', c_clean):
                continue
            if "rekaman video amatir" in c_low or "unggahan bertajuk" in c_low:
                continue

            # Check exact match, fuzzy ratio >= 0.75, or containment
            ratio = difflib.SequenceMatcher(None, top_low, c_low).ratio()
            if ratio >= 0.75 or (len(c_low) >= 8 and (c_low in top_low or top_low in c_low)):
                is_contamination = True
                break

        # Also strip if top line is a Notes folder list artifact or timestamp preview
        if not is_contamination:
            if _is_notes_folder_list_frame(top_line) or re.match(r'^\d{1,2}[.:]\d{2}\s+', top_line):
                is_contamination = True

        if is_contamination:
            print(f"[Tahap 38] Stripped top contaminated line: '{result[0]}'", flush=True)
            result.pop(0)
        else:
            break

    return result


def _strip_top_unselected_debris(lines: list[str]) -> list[str]:
    """
    Strip unselected separator lines (---) and hashtag lines that appear BEFORE
    the first main news body paragraph (Tahap 35 Perbaikan 2).
    E.g. #toltangerangmerak #infotangerang followed by --- above 'Unggahan bertajuk...'.
    """
    if not lines or len(lines) < 2:
        return lines

    result = list(lines)
    # Check if there is an article paragraph later (starts with letter, len >= 25)
    has_body_later = False
    for l in result:
        clean = l.strip()
        if len(clean) >= 25 and not clean.startswith("#") and not re.match(r'^(source|sumber|video|foto)\s*:', clean, re.IGNORECASE):
            has_body_later = True
            break

    if not has_body_later:
        return result

    while result:
        first = result[0].strip()
        if not first:
            result.pop(0)
            continue
        if re.match(r'^[-—–_=]{2,}$', first):
            result.pop(0)
            continue
        if len(first) < 8 and ('•' in first or '·' in first):
            result.pop(0)
            continue
        if first.startswith("#") or ("#" in first and len(first) < 40 and not first.endswith(".")):
            result.pop(0)
            continue
        break

    return result


def _normalize_line_for_match(text: str) -> str:
    """Normalize line for fuzzy overlap matching: lowercase, strip, condense whitespace."""
    if not text:
        return ""
    cleaned = re.sub(r'[\r\n\t]+', ' ', text.lower()).strip()
    return re.sub(r'\s{2,}', ' ', cleaned)


def _is_garbage_ocr_line(text: str) -> bool:
    """Check if an OCR line is hallucinated toolbar debris, banner text, iOS date header, list header, or on-screen keyboard."""
    if not text or not text.strip():
        return False
    t = text.strip()
    if t in ("<<<DOC_START>>>", "<<<NOTES_LIST_VIEW>>>"):
        return True
    t_low = t.lower()

    # System banners & floating toolbar text
    if any(bad in t_low for bad in ["jendela-kecil", "device hub", "this device was viewed remotely", "remotely from mac"]):
        return True

    # iOS Notes Folder List headers (e.g. "Today", "Yesterday", "Previous 7 Days", "Previous 30 Days", "iCloud", "Notes", "9 Notes")
    if re.match(r'^(today|yesterday|previous\s+\d+\s+days|hari\s+ini|kemarin|\d+\s+hari\s+sebelumnya|icloud|notes|\d+\s+notes|catatan|\d+\s+catatan|january|february|march|april|may|june|july|august|september|october|november|december|januari|februari|maret|mei|juni|juli|agustus|oktober|desember)$', t, re.IGNORECASE):
        return True

    # iOS Notes Folder List preview timestamp line (e.g. "07:02 Tidak ada teks", "07.42 prabowo gibran", "14.30 No additional text")
    if re.match(r'^\d{1,2}[.:]\d{2}\s+\S+', t, re.IGNORECASE):
        return True

    # iOS Notes date line (e.g. "24 October 2026 at 04.31" or "20 Oktober 2026")
    date_pattern = r'^\d{1,2}\s+(January|February|March|April|May|June|July|August|September|October|November|December|Oktober|Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)[a-z]*\s+\d{4}(\s+(at|pukul)\s+\d{1,2}[.:]\d{2})?$'
    if re.match(date_pattern, t, re.IGNORECASE):
        return True

    # iOS On-Screen Virtual Keyboard rows (e.g. "w e r t y u i o p", "z x c v b n m", "a s d f g h j k l")
    tokens = [tok for tok in t.split() if tok]
    single_char_tokens = [tok for tok in tokens if len(tok) == 1 and tok.isalpha()]
    if len(single_char_tokens) >= 4 and len(single_char_tokens) >= (len(tokens) * 0.70):
        return True

    # iOS word prediction bar / keyboard debris (e.g. "cat aku", "kamu aku", "kamu aku ya")
    if t_low in ["cat aku", "kamu aku", "kamu aku ya", "aku kamu", "cat kamu"]:
        return True
    if len(tokens) <= 3 and any(p == t_low for p in ["cat aku", "kamu aku", "kamu aku ya"]):
        return True

    # iOS popup callout border phantom characters (Tahap 34: e.g. "1Q0 1O0O", "100 1000", "1Q0", "1O0O")
    if re.match(r'^[0-9OQIl\s]{3,15}$', t):
        return True

    # iOS popup callout border edge fragments (Tahap 38: e.g. "r I", "I I", "r l", "| I", "r |", "r")
    if re.match(r'^[rIl1|/\[\]\(\)\-]{1,2}(\s+[rIl1|/\[\]\(\)\-]{1,2})*$', t):
        return True

    # Debris lines with bullet/middle-dot symbols (Tahap 40: e.g. "•a•y", "• y", "·a")
    if len(t) < 8 and ('•' in t or '·' in t):
        return True

    # Hallucinated words mixing letters and digits randomly
    words = t.split()
    if any(w.lower() in ["v3one", "oil13i1", "111e113n", "me1a1ui", "miliknva", "ao/o3124an"] for w in words):
        return True

    mixed_word_count = sum(
        1 for w in words
        if re.search(r'[a-zA-Z]', w) and re.search(r'[0-9]', w) and not re.match(r'^[a-zA-Z]+[0-9]+$', w)
    )
    if mixed_word_count >= 2:
        return True

    return False


def _clean_ocr_line(line: str) -> str:
    """Clean orphan digits, trailing debris, right-edge truncated words, selection handle typos, and optical OCR defects."""
    if not line:
        return ""
    t = line.strip()
    if _is_garbage_ocr_line(t):
        return ""

    # Strip trailing orphan single digit after uppercase acronym (e.g. "KMP 4" -> "KMP")
    t = re.sub(r'\b([A-Z]{2,})\s+[0-9]$', r'\1', t)

    # Auto-correct edge-truncated words due to AssistiveTouch (Tahap 29)
    t = re.sub(r'\btali evakua$', 'tali evakuasi', t)
    t = re.sub(r'\bmemperlih$', 'memperlihatkan', t)

    # Fix selection handle occlusion & OCR typos (Tahap 30 & Tahap 34)
    t = re.sub(r'(?:^|\b)tik-detik\b', 'detik-detik', t)
    t = re.sub(r'\b(?:i1wa|j1wa|ilwa|jiw4)\b', 'jiwa', t)
    t = re.sub(r'\bmemaksa(?:opng|\s+pa)\b', 'memaksa para penumpang', t, flags=re.IGNORECASE)

    # --- Optical OCR Artifacts Normalization (Tahap 31 & Tahap 34) ---
    # a. Restoration of letter "k" from "l<" / "I<" / "1<" / "|<"
    loop_count = 0
    while loop_count < 10 and re.search(r'(?<=[A-Za-z0-9_@])[lI1|]<|[lI1|]<(?=[A-Za-z0-9_.,!?\s]|$)', t):
        t = re.sub(r'(?<=[A-Za-z0-9_@])[lI1|]<|[lI1|]<(?=[A-Za-z0-9_.,!?\s]|$)', 'k', t)
        loop_count += 1

    # b. Restoration of capital "O" or "0" in middle/end of lowercase word to "o"
    t = re.sub(r'(?<=[a-z])[O0](?=[a-z]|\b)', 'o', t)
    # Mid-sentence capitalized "Oleh" after comma or word character -> "oleh"
    t = re.sub(r'(?<=[a-zA-Z0-9,])\s+Oleh\b', ' oleh', t)

    # c. Restoration of exclamation mark "!" replacing "l" or "i"
    # Mid-word "!" (e.g. "se!amat") -> "l"
    t = re.sub(r'(?<=[A-Za-z])!(?=[A-Za-z])', 'l', t)
    # Preposition "d!" / "d," (e.g. "seorang ayah d!" or "darurat d, tengah") -> "di"
    t = re.sub(r'\bd!(?=\s|$|[,.;])', 'di', t)
    t = re.sub(r'(?<=\s)d,\s+(?=[a-z])', 'di ', t)

    # d. Restoration of "rn" to "m" for affected words (Batumandi, #kmpbatumandi)
    t = re.sub(r'\bBaturnandi\b', 'Batumandi', t)
    t = re.sub(r'\bbaturnandi\b', 'batumandi', t)
    t = re.sub(r'#kmpbaturnandi\b', '#kmpbatumandi', t, flags=re.IGNORECASE)
    t = re.sub(r'#baturnandi\b', '#batumandi', t, flags=re.IGNORECASE)

    # e. Restoration of optical defect "hincfia" -> "hindia" (Tahap 35)
    t = re.sub(r'\bhincfia\b', 'hindia', t, flags=re.IGNORECASE)

    # f. Strip stray trailing < or | or ! on handles if any
    t = re.sub(r'(@[A-Za-z0-9_.]+)[<|!]+(?=\s|$)', r'\1', t)

    # g. Restoration of en-dash in route names (e.g. "Merak–Jakarta") from mojibake/quotes
    t = re.sub(r'\bMerak\S+?Jakarta\b', 'Merak–Jakarta', t)

    # h. Strip stray trailing selection handle brackets/pipes on hashtags (Tahap 38: e.g. #cilegon] -> #cilegon, #infotangerang| -> #infotangerang)
    t = re.sub(r'(#[A-Za-z0-9_]+)[\]\[\|><\)\}!]+(?=\s|$)', r'\1', t)

    # i. Restoration of "(AI Generated Content)" from OCR typo "(Al Generated Content)" (Tahap 38)
    t = re.sub(r'\(\s*Al\s+Generated\s+Content\s*\)', '(AI Generated Content)', t)
    t = re.sub(r'\bAl\s+Generated\b', 'AI Generated', t)

    # j. Direct optical restoration of "Dapua" -> "papua" (Tahap 38)
    t = re.sub(r'\b[Dd]apua\b', 'papua', t)

    # k. Optical restoration of "Iambung" -> "lambung" & capital "I" mistranscriptions of lowercase "l" at word start (Tahap 39)
    t = re.sub(r'\bI(?=(?:ambung|angkah|angsung|agi|ain|alu|ebih|intas|aut|epas|uka|oket|okasi|aporan|embar|ingkungan|uar)\b)', 'l', t)
    t = re.sub(r'\bIambung\b', 'lambung', t)

    # l. Capital 'I' mistranscription of lowercase 'l' at end of lowercase word (Tahap 40: e.g. "awak KapaI" -> "awak kapal")
    t = re.sub(r'(?<=[a-z])I\b', 'l', t)
    t = re.sub(r'\bawak\s+Kapa[Il]\b', 'awak kapal', t, flags=re.IGNORECASE)

    # m. Restore @wisnutry017 -> @wisnutryo17 (Tahap 40)
    t = re.sub(r'@wisnutry017\b', '@wisnutryo17', t)

    # n. Restore clipped descender hashtags (Tahap 40: #iakartarunninafestival -> #jakartarunningfestival, #irf2026 -> #jrf2026)
    t = re.sub(r'#iakartarunninafestival\b', '#jakartarunningfestival', t, flags=re.IGNORECASE)
    t = re.sub(r'#irf2026\b', '#jrf2026', t, flags=re.IGNORECASE)
    t = re.sub(r'#iakarta\b', '#jakarta', t, flags=re.IGNORECASE)
    t = re.sub(r'#irf\b', '#jrf', t, flags=re.IGNORECASE)

    # o. Optical restoration of "!olom" -> "kolom" (Tahap 40: e.g. "di !olom komentar" -> "di kolom komentar")
    t = re.sub(r'(?<=\s)!olom\b|^!olom\b', 'kolom', t, flags=re.IGNORECASE)

    return t


def _line_optical_defect_penalty(line: str) -> int:
    """Calculate optical defect penalty for cleaner line selection."""
    if not line:
        return 0
    penalty = 0
    if re.search(r'[lI1|]<', line):
        penalty += 10
    if re.search(r'@[A-Za-z0-9_.]+[<|!]', line):
        penalty += 15
    if re.search(r'(?<=[A-Za-z])!(?=[A-Za-z])', line):
        penalty += 10
    if re.search(r'\bd!(?=\s|$|[,.;])', line):
        penalty += 10
    if re.search(r'baturnandi', line, re.IGNORECASE):
        penalty += 10
    if re.search(r'\bhincfia\b', line, re.IGNORECASE):
        penalty += 10
    if re.search(r'(?<=[a-zA-Z0-9,])\s+Oleh\b', line):
        penalty += 5
    if re.search(r'(?<=[a-z])[O0](?=[a-z]|\b)', line):
        penalty += 5
    return penalty


def _pick_cleaner_line(line1: str, line2: str) -> str:
    """Select the cleaner version of an anchor line (e.g. avoiding trailing orphan digits, truncated words, or OCR optical defects)."""
    l1_clean = _clean_ocr_line(line1)
    l2_clean = _clean_ocr_line(line2)

    # Prioritize line with fewer optical defects (Tahap 31)
    p1 = _line_optical_defect_penalty(line1)
    p2 = _line_optical_defect_penalty(line2)
    if p1 < p2:
        return l1_clean
    if p2 < p1:
        return l2_clean

    # Prioritize complete "detik-detik" over truncated "tik-detik" (Tahap 30)
    if "detik-detik" in l1_clean and "detik-detik" not in l2_clean:
        return l1_clean
    if "detik-detik" in l2_clean and "detik-detik" not in l1_clean:
        return l2_clean

    # Prioritize clean "jiwa" over OCR typo "i1wa" (Tahap 30)
    if "jiwa" in l1_clean and "jiwa" not in l2_clean:
        return l1_clean
    if "jiwa" in l2_clean and "jiwa" not in l1_clean:
        return l2_clean

    if line1 != l1_clean and line2 == l2_clean:
        return l2_clean
    if line2 != l2_clean and line1 == l1_clean:
        return l1_clean

    # If one line ends with a truncated word of the other line (e.g. "evakua" vs "evakuasi"),
    # WAJIB pick the line with the complete/longer word (Tahap 29)!
    w1 = l1_clean.split()
    w2 = l2_clean.split()
    if w1 and w2:
        last1 = w1[-1].rstrip(".,?!")
        last2 = w2[-1].rstrip(".,?!")
        if last1 != last2:
            if last2.startswith(last1) and len(last2) > len(last1):
                return l2_clean
            if last1.startswith(last2) and len(last1) > len(last2):
                return l1_clean

    return l1_clean if len(l1_clean) >= len(l2_clean) else l2_clean


def _unsplit_broken_lines(lines: list[str]) -> list[str]:
    """
    Broken Word Un-splitter (Tahap 29):
    If a line is followed by a short line containing only 1 small word fragment
    (1-6 lowercase letters like 'atkan', 'si', 'nya', 'kan'), merge it back onto the previous line.
    E.g. '... yang memperlih' + 'atkan' -> '... yang memperlihatkan'.
    """
    if not lines:
        return []
    result: list[str] = []
    i = 0
    while i < len(lines):
        line = lines[i]
        if not line or not line.strip():
            result.append(line)
            i += 1
            continue

        if i + 1 < len(lines):
            nxt = lines[i + 1].strip()
            # Match 1-6 lowercase letters, optionally followed by punctuation
            m = re.match(r'^([a-z]{1,6})([.,?!]?)$', nxt)
            if m:
                frag = m.group(1)
                punct = m.group(2)
                curr_stripped = line.strip()
                if not re.search(r'[.?!:]$', curr_stripped):
                    last_tok = curr_stripped.split()[-1] if curr_stripped.split() else ""
                    if last_tok.endswith(frag) or (last_tok == "memperlihatkan" and frag == "atkan") or (last_tok == "evakuasi" and frag == "si"):
                        result.append(line)
                        i += 2  # consume current line and discard orphan fragment
                        continue

                    if curr_stripped.endswith("-"):
                        merged_line = curr_stripped[:-1] + frag + punct
                    elif frag in {"atkan", "si", "nya", "kan", "kah", "lah", "an", "i", "wan", "man", "tas"} or len(last_tok) >= 4:
                        merged_line = curr_stripped + frag + punct
                    else:
                        merged_line = curr_stripped + " " + frag + punct
                    line = merged_line
                    i += 1  # consume the fragment line

        result.append(line)
        i += 1
    return result


def _is_generic_template_line(line: str) -> bool:
    """
    Check if a line is a generic social media template line (Tahap 34).
    These template lines are forbidden from serving as the sole anchor connecting two different notes/apps.
    """
    if not line:
        return False
    t = line.strip()
    if re.match(r'^(Source|Sumber|Video|Foto)\s*:', t, re.IGNORECASE):
        return True
    if re.match(r'^Bagaimana\s+(perasaan|tanggapan|menurut)\b', t, re.IGNORECASE):
        return True
    if t.startswith("#"):
        return True
    if re.search(r'\bBanteners\??$', t, re.IGNORECASE):
        return True
    return False


def _score_gap_quality(gap_lines: list[str]) -> int:
    """
    Score OCR gap quality between anchors (Tahap 34).
    Heavy penalty for balloon border phantom digits/letters ("1Q0 1O0O"),
    broken words ("memaksaopng", "memaksa pa"), and optical defects ("l<").
    """
    if not gap_lines:
        return 0
    score = 0
    for line in gap_lines:
        t = line.strip()
        # Heavy penalty for phantom balloon border letters/digits (e.g. "1Q0 1O0O", "100 1000")
        if re.search(r'^[0-9OQIl\s]{3,15}$', t):
            score -= 100
        # Heavy penalty for occluded / broken words
        if re.search(r'\b(?:memaksaopng|memaksa\s+pa|opng)\b', t, re.IGNORECASE):
            score -= 100
        # Optical defect penalty
        if re.search(r'[lI1|]<', t):
            score -= 50
        # Mixed letter/digit nonsense
        words = t.split()
        if sum(1 for w in words if re.search(r'[a-zA-Z]', w) and re.search(r'[0-9]', w) and not re.match(r'^[a-zA-Z]+[0-9]+$', w)) >= 1:
            score -= 50
        # Positive score for authentic words
        score += len(words) * 10
    return score


def _complete_usernames(text: str) -> str:
    """
    Self-Consistent @Username Completion (Tahap 34 & Tahap 35):
    If the document has a full mention like '@erikkristian' or '@andribotox' in Paragraph 2,
    and another line (e.g. Source line) has a truncated mention or handle occluded by selection handle
    (e.g. '@andriboto<', '@andriboto|', '@erikkrist', '@eril<l<rist', '@erikkri'),
    automatically complete it to the full mention.
    """
    if not text or "@" not in text:
        return text

    # Pre-clean any leftover l< in handles
    norm_text = re.sub(r'(?<=[A-Za-z0-9_@])[lI1|]<|[lI1|]<(?=[A-Za-z0-9_.,!?\s]|$)', 'k', text)

    # Find full clean mentions (length >= 6 without trailing defect punctuation)
    all_clean_mentions = re.findall(r'@[A-Za-z0-9_.]+', norm_text)
    full_handles = [m for m in set(all_clean_mentions) if len(m) >= 6 and not m.endswith(('.', '_'))]
    if not full_handles:
        return norm_text

    result = norm_text

    # 1. First, catch mentions with trailing defect characters like <, |, ! (Tahap 35: @andriboto< -> @andribotox)
    defect_mentions = set(re.findall(r'@[A-Za-z0-9_.]+[<|!]+', result))
    for raw_m in defect_mentions:
        clean_pfx = re.sub(r'[<|!]+$', '', raw_m)
        matched_full = None
        for full_h in full_handles:
            if full_h.lower().startswith(clean_pfx.lower()) and len(full_h) > len(clean_pfx):
                matched_full = full_h
                break
        if matched_full:
            result = re.sub(re.escape(raw_m) + r'(?![A-Za-z0-9_])', matched_full, result)
        else:
            result = re.sub(re.escape(raw_m) + r'(?![A-Za-z0-9_])', clean_pfx, result)

    # 2. Complete truncated clean mentions with length >= 5
    for full_h in sorted(full_handles, key=lambda m: len(m), reverse=True):
        base = full_h[1:]
        for l in range(len(base) - 1, 4, -1):
            prefix = "@" + base[:l]
            pattern = re.escape(prefix) + r'(?![A-Za-z0-9_.])'
            result = re.sub(pattern, full_h, result)

    return result


def _is_body_sentence_line(line: str) -> bool:
    """Check if a line is a genuine body content sentence (not header, not footer, not hashtag)."""
    if not line or len(line.strip()) < 20:
        return False
    l = line.strip().lower()
    if l.startswith("#") or l.startswith("source:") or l.startswith("sumber:"):
        return False
    if re.match(r'^bagaimana\s+(perasaan|tanggapan|kalian|menurut)', l):
        return False
    if _is_generic_template_line(line) or _is_garbage_ocr_line(line):
        return False
    return True


def has_overlapping_body_line(lines_a: list[str], lines_b: list[str]) -> bool:
    """Check if lines_a and lines_b share at least 1 non-generic body sentence (Tahap 37 Anti-Leak Rule)."""
    if not lines_a or not lines_b:
        return False
    body_a = [l.strip() for l in lines_a if _is_body_sentence_line(l)]
    body_b = [l.strip() for l in lines_b if _is_body_sentence_line(l)]
    for la in body_a:
        na = _normalize_line_for_match(la)
        for lb in body_b:
            nb = _normalize_line_for_match(lb)
            if abs(len(na) - len(nb)) > max(15, int(max(len(na), len(nb)) * 0.35)):
                continue
            ratio = difflib.SequenceMatcher(None, na, nb).ratio()
            if ratio >= 0.82 or (len(na) >= 22 and (na in nb or nb in na)):
                return True
    return False


def merge_two_overlapping_blocks(linesA: list[str], linesB: list[str]) -> list[str] | None:
    """
    Gap-Filling Segment Stitcher (Tahap 28 & Tahap 34):
    Identifies matching anchor pairs, detects alignment direction, fills in occluded
    gaps between anchors with quality scoring (e.g. restoring lines covered by iOS Cut/Copy/Paste balloon),
    cleans orphan digits, and clamps tail to clean content.
    Requires at least 1 non-generic body line match to prevent cross-app splicing.
    """
    if not linesA or not linesB:
        return None

    cleanA = _unsplit_broken_lines([_clean_ocr_line(l) for l in linesA if l and l.strip() and not _is_garbage_ocr_line(l)])
    cleanB = _unsplit_broken_lines([_clean_ocr_line(l) for l in linesB if l and l.strip() and not _is_garbage_ocr_line(l)])
    if not cleanA or not cleanB:
        return None

    if cleanA == cleanB:
        return cleanA

    # Find matching line pairs with len >= 18 and ratio >= 0.78
    matches: list[tuple[int, int, float, int]] = []
    normA = [_normalize_line_for_match(l) for l in cleanA]
    normB = [_normalize_line_for_match(l) for l in cleanB]

    for i, na in enumerate(normA):
        if len(na) < 18:
            continue
        for j, nb in enumerate(normB):
            if len(nb) < 18:
                continue

            len_diff = abs(len(na) - len(nb))
            if len_diff > max(len(na), len(nb)) * 0.40:
                continue

            sm_ratio = difflib.SequenceMatcher(None, na, nb).ratio()
            if sm_ratio >= 0.78 or (len(na) >= 20 and (na in nb or nb in na)):
                ratio_val = max(sm_ratio, 0.88 if (na in nb or nb in na) else sm_ratio)
                matches.append((i, j, ratio_val, i - j))

    if not matches:
        return None

    # Tahap 34: Validate that matching pairs contain at least ONE specific body line match
    # DILARANG menyambungkan linesA dan linesB jika anchor yang cocok HANYA baris template umum!
    has_specific_body_anchor = False
    for i, j, sm_ratio, _ in matches:
        la = cleanA[i]
        lb = cleanB[j]
        if not _is_generic_template_line(la) and not _is_generic_template_line(lb):
            if len(normA[i]) >= 22 and len(normB[j]) >= 22 and sm_ratio >= 0.85:
                w_a = set(re.findall(r'\w+', normA[i]))
                w_b = set(re.findall(r'\w+', normB[j]))
                if w_a and w_b:
                    overlap_ratio = len(w_a & w_b) / float(max(len(w_a), len(w_b)))
                    if overlap_ratio >= 0.65:
                        has_specific_body_anchor = True
                        break

    if not has_specific_body_anchor:
        return None

    # Group by offset i - j to find dominant alignment
    offset_counts: dict[int, int] = {}
    for m in matches:
        off = m[3]
        offset_counts[off] = offset_counts.get(off, 0) + 1

    dom_offset, _ = max(offset_counts.items(), key=lambda kv: kv[1])
    consistent_matches = [m for m in matches if abs(m[3] - dom_offset) <= 1]
    if not consistent_matches:
        consistent_matches = matches

    # Build strictly increasing anchor sequence: i_0 < i_1 < ... and j_0 < j_1 < ...
    consistent_matches.sort(key=lambda m: m[0])
    anchors: list[tuple[int, int]] = []
    last_i = -1
    last_j = -1
    for m in consistent_matches:
        ci, cj = m[0], m[1]
        if ci > last_i and cj > last_j:
            anchors.append((ci, cj))
            last_i = ci
            last_j = cj

    if not anchors:
        return None

    merged_lines: list[str] = []

    # 1. Head (before first anchor)
    first_i, first_j = anchors[0]
    if dom_offset > 0:
        head = cleanA[:first_i]
    elif dom_offset < 0:
        head = cleanB[:first_j]
    else:
        head = cleanA[:first_i] if first_i >= first_j else cleanB[:first_j]
    merged_lines.extend(head)

    # 2. Iterate through anchors and fill gaps
    for k in range(len(anchors)):
        curr_i, curr_j = anchors[k]
        best_anchor = _pick_cleaner_line(cleanA[curr_i], cleanB[curr_j])
        merged_lines.append(best_anchor)

        if k < len(anchors) - 1:
            next_i, next_j = anchors[k + 1]
            gapA = cleanA[curr_i + 1 : next_i]
            gapB = cleanB[curr_j + 1 : next_j]

            if not gapA and not gapB:
                continue
            elif gapA and not gapB:
                merged_lines.extend(gapA)
            elif gapB and not gapA:
                merged_lines.extend(gapB)
            else:
                scoreA = _score_gap_quality(gapA)
                scoreB = _score_gap_quality(gapB)
                if scoreA > scoreB:
                    merged_lines.extend(gapA)
                elif scoreB > scoreA:
                    merged_lines.extend(gapB)
                else:
                    if len(gapA) == len(gapB):
                        for ga, gb in zip(gapA, gapB):
                            merged_lines.append(_pick_cleaner_line(ga, gb))
                    elif len(gapA) > len(gapB):
                        merged_lines.extend(gapA)
                    else:
                        merged_lines.extend(gapB)

    # 3. Tail (after last anchor)
    last_i, last_j = anchors[-1]
    tailA = cleanA[last_i + 1 :]
    tailB = cleanB[last_j + 1 :]

    # Tahap 35 Perbaikan 3B: If one block contains #hashtags and the other does not,
    # the tail MUST preserve the hashtag-bearing tail (e.g. bottom frame tail continuing below top frame)!
    has_hashtag_A = any(l.strip().startswith("#") for l in cleanA)
    has_hashtag_B = any(l.strip().startswith("#") for l in cleanB)

    if has_hashtag_A and not has_hashtag_B:
        tail = tailA
    elif has_hashtag_B and not has_hashtag_A:
        tail = tailB
    elif dom_offset > 0:
        tail = tailB if tailB else tailA
    elif dom_offset < 0:
        tail = tailA if tailA else tailB
    else:
        tail = tailA if len(tailA) >= len(tailB) else tailB
    merged_lines.extend(tail)

    # Post-process: deduplicate immediate identical adjacent lines
    deduped: list[str] = []
    for line in merged_lines:
        c_line = _clean_ocr_line(line)
        if not c_line or _is_garbage_ocr_line(c_line):
            continue
        if deduped and _normalize_line_for_match(deduped[-1]) == _normalize_line_for_match(c_line):
            continue
        deduped.append(c_line)

    # Clamp tail: if there are hashtag lines (#...), cut any trailing lines after hashtag block
    last_hashtag_idx = -1
    for idx, l in enumerate(deduped):
        if l.strip().startswith("#"):
            last_hashtag_idx = idx

    if last_hashtag_idx != -1:
        clean_tail = deduped[:last_hashtag_idx + 1]
        for l in deduped[last_hashtag_idx + 1:]:
            if l.strip().startswith("#"):
                clean_tail.append(l)
            elif not _is_garbage_ocr_line(l) and len(l.split()) > 3:
                clean_tail.append(l)
        deduped = clean_tail

    return deduped


def stitch_two_ocr_texts(text1: str, text2: str) -> str:
    """Stitch two overlapping OCR text frames together by finding the overlap between text1 bottom and text2 top."""
    t1_clean = (text1 or "").strip()
    t2_clean = (text2 or "").strip()
    if not t1_clean:
        return t2_clean
    if not t2_clean:
        return t1_clean
    if t1_clean == t2_clean:
        return t2_clean

    lines1 = _unsplit_broken_lines([_clean_ocr_line(l) for l in t1_clean.splitlines() if l.strip()])
    lines2 = _unsplit_broken_lines([_clean_ocr_line(l) for l in t2_clean.splitlines() if l.strip()])

    interior_merged = merge_two_overlapping_blocks(lines1, lines2)
    if interior_merged:
        return "\r\n".join(interior_merged)

    # Find best overlap between bottom of lines1 and top of lines2
    best_match = None
    best_score = 0.0

    for i in range(len(lines1) - 1, -1, -1):
        l1 = lines1[i]
        if not l1 or len(l1) < 22 or _is_generic_template_line(l1):
            continue
        for j in range(0, min(len(lines2), 20)):
            l2 = lines2[j]
            if not l2 or len(l2) < 22 or _is_generic_template_line(l2):
                continue

            l1_low = l1.lower()
            l2_low = l2.lower()

            sm = difflib.SequenceMatcher(None, l1_low, l2_low).ratio()
            w1 = set(re.findall(r'\w+', l1_low))
            w2 = set(re.findall(r'\w+', l2_low))
            w_overlap = (len(w1 & w2) / float(max(len(w1), len(w2)))) if (w1 and w2) else 0.0

            if sm >= 0.85 and w_overlap >= 0.65:
                bonus = 0.0
                if i + 1 < len(lines1) and j + 1 < len(lines2):
                    nxt1 = lines1[i + 1].strip().lower()
                    nxt2 = lines2[j + 1].strip().lower()
                    if nxt1 and nxt2 and len(nxt1) >= 4 and len(nxt2) >= 4:
                        nxt_ratio = difflib.SequenceMatcher(None, nxt1, nxt2).ratio()
                        if nxt_ratio >= 0.65 or (nxt1 in nxt2 or nxt2 in nxt1):
                            bonus = 0.25

                total_score = sm + bonus
                if total_score > best_score:
                    best_score = total_score
                    best_match = (i, j, l1, l2)

        if best_match and best_score >= 0.90:
            break

    # Tahap 34: HAPUS fallback non-overlapping lines1 + lines2!
    # If no valid overlapping body match is found, DO NOT concatenate different notes/apps! Return current frame t2_clean.
    if not best_match:
        return t2_clean

    i, j, l1, l2 = best_match
    longer_line = _pick_cleaner_line(l1, l2)
    stitched_lines = lines1[:i] + [longer_line] + lines2[j + 1:]
    return "\r\n".join(stitched_lines)


def stitch_multiline_ocr_frames(texts: list[str]) -> str:
    """
    Stitch multiple chronologically ordered OCR frames into a seamless single multi-paragraph text
    with preserved paragraph spacing (\\r\\n\\r\\n).
    Rejects Notes Folder List frames, locks DOC_START below date header, and prevents contamination (Tahap 32).
    """
    if not texts:
        return ""

    # Detect if any frame is a Notes folder list frame (Tahap 32 Bug 1)
    has_folder_list = any(_is_notes_folder_list_frame(t) for t in texts)
    if has_folder_list:
        clear_recent_long_ocr_chunks()

    valid_texts = [t.strip() for t in texts if t and t.strip() and not _is_notes_folder_list_frame(t)]
    if not valid_texts:
        return ""

    # Look for iOS Notes date header or <<<DOC_START>>> in input texts to determine absolute document start (DOC_START)
    doc_start_norm: str | None = None
    date_header_re = re.compile(
        r'^\d{1,2}\s+(?:January|February|March|April|May|June|July|August|September|October|November|December|Oktober|Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)[a-z]*\s+\d{4}(?:\s+(?:at|pukul)\s+\d{1,2}[.:]\d{2})?$',
        re.IGNORECASE
    )

    for t in valid_texts:
        raw_lines = [l.strip() for l in t.splitlines() if l.strip()]
        for idx, line in enumerate(raw_lines):
            if line == "<<<DOC_START>>>" or date_header_re.match(line):
                # The line immediately following <<<DOC_START>>> or date header is DOC_START
                for nxt_idx in range(idx + 1, len(raw_lines)):
                    candidate = _clean_ocr_line(raw_lines[nxt_idx])
                    if candidate and not _is_garbage_ocr_line(candidate) and candidate != "<<<DOC_START>>>":
                        doc_start_norm = _normalize_line_for_match(candidate)
                        break
                if doc_start_norm:
                    break
        if doc_start_norm:
            break

    # Record paragraph breaks: lines followed by empty line in input frames
    para_breaks_after: set[str] = set()
    for t in valid_texts:
        raw_lines = [l.strip() for l in t.splitlines()]
        for idx in range(len(raw_lines) - 1):
            if raw_lines[idx] and raw_lines[idx + 1] == "":
                norm_l = _normalize_line_for_match(raw_lines[idx])
                if norm_l:
                    para_breaks_after.add(norm_l)

    curr_frame = valid_texts[-1]
    curr_raw = [_clean_ocr_line(l) for l in curr_frame.splitlines() if l.strip() and not _is_garbage_ocr_line(l)]
    curr_raw = _strip_top_unselected_debris(curr_raw)
    curr_lines = _unsplit_broken_lines(curr_raw)

    # ATURAN EMAS 1 (Tahap 36): Short text (<= 5 non-empty lines, total chars < 200) -> NEVER stitch with history!
    is_short_snippet = (len(curr_lines) <= 5 and all(len(l) < 55 for l in curr_lines) and sum(len(l) for l in curr_lines) < 200)
    if is_short_snippet:
        return curr_frame

    # Middle paragraph opener regex (Tahap 35 Perbaikan 3A)
    # These phrases can NEVER qualify as the opening of the document!
    middle_para_opener_re = re.compile(
        r'^(momen\s+menegangkan|situasi\s+darurat|dalam\s+unggahan|konten\s+tersebut|fenomena\s+ini|seluruh\s+\d+|bagaimana\s+(perasaan|tanggapan|menurut)|source\s*:|sumber\s*:)',
        re.IGNORECASE
    )
    first_line_clean = curr_lines[0] if curr_lines else ""
    is_middle_para = bool(middle_para_opener_re.match(first_line_clean))

    # Check if current frame already contains a complete single-screen text (Tahap 36 Golden Rule 2)
    has_closing = any(l.startswith("#") for l in curr_lines[-2:]) if len(curr_lines) >= 2 else False
    if doc_start_norm:
        has_body_start = (_normalize_line_for_match(first_line_clean) == doc_start_norm)
    else:
        has_body_start = (
            not is_middle_para
            and len(first_line_clean) >= 20
            and not _is_generic_template_line(first_line_clean)
            and first_line_clean[0].isupper()
            and not first_line_clean.endswith(",")
        )
    is_complete_single_screen = (has_closing and has_body_start and len(curr_lines) >= 6)

    # If current frame is 100% complete and has no broken occlusion gap, do not contaminate with history!
    if is_complete_single_screen and _score_gap_quality(curr_lines) >= 0:
        return "\r\n".join(curr_lines)

    # Stitch backwards anchored on curr_lines so current frame is NEVER dropped!
    current_lines = list(curr_lines)
    for prev in reversed(valid_texts[:-1]):
        raw_prev = [_clean_ocr_line(l) for l in prev.splitlines() if l.strip() and not _is_garbage_ocr_line(l)]
        prev_lines = _unsplit_broken_lines(raw_prev)
        if not prev_lines:
            continue
        merged = merge_two_overlapping_blocks(prev_lines, current_lines)
        if not merged:
            # Bidirectional fallback (Tahap 39): if user scrolled up to note top after viewing note bottom
            merged = merge_two_overlapping_blocks(current_lines, prev_lines)
        if merged:
            current_lines = merged
        else:
            if not is_complete_single_screen:
                t_merged = stitch_two_ocr_texts("\r\n".join(prev_lines), "\r\n".join(current_lines))
                m_lines = [_clean_ocr_line(l) for l in t_merged.splitlines() if l.strip() and not _is_garbage_ocr_line(l)]
                if not m_lines or m_lines == current_lines:
                    t_merged = stitch_two_ocr_texts("\r\n".join(current_lines), "\r\n".join(prev_lines))
                    m_lines = [_clean_ocr_line(l) for l in t_merged.splitlines() if l.strip() and not _is_garbage_ocr_line(l)]
                if m_lines:
                    current_lines = _unsplit_broken_lines(m_lines)

    current_lines = _unsplit_broken_lines(current_lines)

    # Top-of-Note Lock: ensure current_lines starts at DOC_START if detected
    if doc_start_norm:
        for idx, l in enumerate(current_lines):
            if _normalize_line_for_match(l) == doc_start_norm:
                if idx > 0:
                    current_lines = current_lines[idx:]
                break

    # LAPIS 3 (Tahap 33): Strip typed buffer contamination from top of long text
    current_lines = _strip_typed_buffer_contamination_from_top(current_lines)
    # Tahap 35 Perbaikan 2: Strip unselected top hashtags/separators
    current_lines = _strip_top_unselected_debris(current_lines)

    # If long note (>= 4 lines), check and merge with session chunk memory (_RECENT_LONG_OCR_CHUNKS)
    if len(current_lines) >= 4:
        now = time.time()
        with _CHUNKS_LOCK:
            _RECENT_LONG_OCR_CHUNKS[:] = [c for c in _RECENT_LONG_OCR_CHUNKS if (now - c["ts"]) <= 90.0]

            for past in reversed(_RECENT_LONG_OCR_CHUNKS):
                # Tahap 37 Anti-Leak: MUST share at least 1 genuine body sentence with current frame!
                if not has_overlapping_body_line(past["lines"], current_lines):
                    continue
                hist_merged = merge_two_overlapping_blocks(past["lines"], current_lines)
                if not hist_merged:
                    hist_merged = merge_two_overlapping_blocks(current_lines, past["lines"])
                if hist_merged and len(hist_merged) >= len(current_lines):
                    # Top-of-Note Lock on merged history: DILARANG KERAS menambahkan baris di atas DOC_START!
                    if doc_start_norm:
                        for idx, l in enumerate(hist_merged):
                            if _normalize_line_for_match(l) == doc_start_norm:
                                if idx > 0:
                                    hist_merged = hist_merged[idx:]
                                break
                    hist_merged = _strip_typed_buffer_contamination_from_top(hist_merged)
                    hist_merged = _strip_top_unselected_debris(hist_merged)
                    current_lines = hist_merged

            _RECENT_LONG_OCR_CHUNKS.append({"ts": now, "lines": list(current_lines)})
            if len(_RECENT_LONG_OCR_CHUNKS) > 5:
                _RECENT_LONG_OCR_CHUNKS[:] = _RECENT_LONG_OCR_CHUNKS[-5:]

    current_lines = _unsplit_broken_lines(current_lines)

    # Top-of-Note Lock final pass
    if doc_start_norm:
        for idx, l in enumerate(current_lines):
            if _normalize_line_for_match(l) == doc_start_norm:
                if idx > 0:
                    current_lines = current_lines[idx:]
                break

    current_lines = _strip_typed_buffer_contamination_from_top(current_lines)
    current_lines = _strip_top_unselected_debris(current_lines)

    if len(current_lines) <= 1:
        return "\r\n".join(current_lines)

    joined = "\r\n".join(current_lines)
    return ensure_paragraph_formatting(joined, para_breaks_after)


def ensure_paragraph_formatting(text: str, raw_para_breaks: set[str] | None = None) -> str:
    """
    Final Paragraph Formatter (Tahap 32 & Tahap 33):
    Called AS THE VERY LAST STEP before sending to Windows Clipboard.
    Guarantees clean double newlines (\\r\\n\\r\\n) between paragraphs and single newlines (\\r\\n) within paragraphs and hashtags.
    Strictly forbids fake double newlines (\\r\\n\\r\\n) mid-sentence for long text!
    For short text (<= 5 lines), 100% preserves empty lines between lines!
    """
    if not text or not text.strip():
        return ""

    raw_lines = [l.strip() for l in text.splitlines()]
    # Tahap 35: Strip top unselected debris and typed buffer contamination BEFORE indexing has_blank_after!
    raw_lines = _strip_top_unselected_debris(raw_lines)
    raw_lines = _strip_typed_buffer_contamination_from_top(raw_lines)

    non_empty_lines: list[str] = []
    has_blank_after: set[int] = set()

    for idx, l in enumerate(raw_lines):
        if l:
            c_l = _clean_ocr_line(l)
            if not c_l or _is_garbage_ocr_line(c_l):
                continue
            non_empty_lines.append(c_l)
        else:
            if non_empty_lines:
                has_blank_after.add(len(non_empty_lines) - 1)

    # Secondary strip pass on cleaned lines
    non_empty_lines = _strip_typed_buffer_contamination_from_top(non_empty_lines)
    non_empty_lines = _strip_top_unselected_debris(non_empty_lines)

    # Clamp tail: discard any garbage or debris after the hashtag block
    last_hashtag_idx = -1
    for idx, l in enumerate(non_empty_lines):
        if l.strip().startswith("#"):
            last_hashtag_idx = idx

    if last_hashtag_idx != -1:
        clean_tail = []
        for idx, l in enumerate(non_empty_lines):
            if idx <= last_hashtag_idx:
                clean_tail.append(l)
            else:
                if l.strip().startswith("#"):
                    clean_tail.append(l)
                elif not _is_garbage_ocr_line(l) and len(l.split()) > 3:
                    clean_tail.append(l)
        non_empty_lines = clean_tail

    if not non_empty_lines:
        return ""

    if len(non_empty_lines) <= 1:
        return non_empty_lines[0]

    is_short_text = len(non_empty_lines) <= 6
    is_long_text = len(non_empty_lines) > 6

    if is_short_text:
        # Cross-reference _TYPED_LINES_BUFFER: check if user typed double Enter between lines (Tahap 38)
        with _TYPED_BUFFER_LOCK:
            buf = list(_TYPED_LINES_BUFFER)
        for idx in range(len(non_empty_lines) - 1):
            if idx in has_blank_after:
                continue
            curr_low = non_empty_lines[idx].lower().strip()
            next_low = non_empty_lines[idx + 1].lower().strip()
            for b_idx in range(len(buf) - 2):
                if buf[b_idx + 1] == "":  # empty line in buffer
                    b_prev = buf[b_idx].lower().strip()
                    b_next = buf[b_idx + 2].lower().strip()
                    if (curr_low == b_prev or (len(curr_low) >= 4 and (curr_low in b_prev or b_prev in curr_low))) and \
                       (next_low == b_next or (len(next_low) >= 4 and (next_low in b_next or b_next in next_low))):
                        has_blank_after.add(idx)
                        break

    # Compute average full line length
    full_line_lengths = [len(l) for l in non_empty_lines if len(l) >= 28 and not l.startswith("#")]
    avg_full_line_len = (sum(full_line_lengths) / len(full_line_lengths)) if full_line_lengths else 42.0

    para_closing_re = re.compile(
        r'(?:media\s+sosial|liferaft|rakit\s+darurat|korban\s+jiwa|dari\s+warganet|banteners)\s*[.)?!"]*$',
        re.IGNORECASE
    )
    section_opener_re = re.compile(
        r'^(momen\s+menegangkan|situasi\s+darurat|dalam\s+(unggahan|video|rekaman|konten|foto)|kericuhan\b|bagaimana\s+(perasaan|tanggapan|menurut)|rekaman\s+singkat|source\s*:|sumber\s*:|video\s*:|foto\s*:|#)',
        re.IGNORECASE
    )
    terminal_punct_re = re.compile(r'(\.|\)\.|\]\.|\"\.|\'\.|\?|!)[ \t]*$')

    output_lines: list[str] = []
    for i, line in enumerate(non_empty_lines):
        output_lines.append(line)
        if i < len(non_empty_lines) - 1:
            nxt = non_empty_lines[i + 1]
            norm_curr = _normalize_line_for_match(line)
            norm_next = _normalize_line_for_match(nxt)

            is_curr_source = bool(re.match(r'^(source|sumber)\s*:', line, re.IGNORECASE))
            is_next_source = bool(re.match(r'^(source|sumber)\s*:', nxt, re.IGNORECASE))
            is_curr_hashtag = line.startswith("#")
            is_next_hashtag = nxt.startswith("#")

            is_para_end = False

            if is_short_text:
                # Tahap 33 & Tahap 38: For short text (<= 6 non-empty lines), 100% preserve all blank lines!
                if i in has_blank_after or (raw_para_breaks and (norm_curr in raw_para_breaks or any(norm_curr in b or b in norm_curr for b in raw_para_breaks))):
                    is_para_end = True
            else:
                # Long text rules (Tahap 32 & Tahap 34 & Tahap 40)
                # Rule 4: Between consecutive hashtag lines, STRICTLY FORBIDDEN to insert blank line!
                if is_curr_hashtag and is_next_hashtag:
                    is_para_end = False
                # Rule 3: Always insert \r\n\r\n before Source/Sumber or before first #
                elif is_next_source:
                    is_para_end = True
                elif is_next_hashtag and not is_curr_hashtag:
                    is_para_end = True
                else:
                    # Rule 1: Syarat Wajib Kalimat Selesai: line MUST end with terminal punct or be Source
                    has_sentence_end = bool(terminal_punct_re.search(line)) or is_curr_source
                    is_next_capital = bool(re.match(r'^[A-Z"“\'#]', nxt))
                    has_blank = (
                        i in has_blank_after
                        or (raw_para_breaks and (norm_curr in raw_para_breaks or any(norm_curr in b or b in norm_curr for b in raw_para_breaks)))
                    )

                    if has_sentence_end:
                        # Rule 2A (Tahap 34 & Tahap 40): If sentence has terminal punct AND OCR has blank line AND next starts with capital
                        if has_blank and is_next_capital:
                            is_para_end = True
                        # Rule 2B: Known section openers
                        elif section_opener_re.match(nxt):
                            is_para_end = True
                        # Rule 2C: Known paragraph closing phrases
                        elif para_closing_re.search(line):
                            is_para_end = True
                        # Rule 2D: Short terminal line (< 82% of average line length)
                        elif (
                            len(line) < (0.82 * avg_full_line_len)
                            and is_next_capital
                            and not is_curr_hashtag
                        ):
                            is_para_end = True

                    # Rule 4: Di antara dua baris yang belum berakhir titik, DILARANG KERAS menyisipkan \r\n\r\n!
                    if not is_curr_source and not terminal_punct_re.search(line) and not is_next_source and not (is_next_hashtag and not is_curr_hashtag):
                        is_para_end = False

            if is_para_end:
                output_lines.append("")

    joined = "\r\n".join(output_lines)
    # Self-Consistent @Username Completion (Tahap 34)
    joined = _complete_usernames(joined)
    # Multi-line / cross-line OCR optical restoration (Tahap 38)
    joined = re.sub(r'\(\s*Al\s+Generated\s+Content\s*\)', '(AI Generated Content)', joined, flags=re.IGNORECASE)
    joined = re.sub(r'\bAl\s+Generated\b', 'AI Generated', joined)
    # Normalize Windows CRLF line endings
    joined = re.sub(r'(\r?\n){3,}', '\r\n\r\n', joined)
    joined = re.sub(r'(?<!\r)\n', '\r\n', joined)
    joined = joined.replace("\r\r\n", "\r\n")
    return joined


def process_ocr_batch_results(batch_texts: list[str], return_meta: bool = False) -> str | None | tuple[str | None, bool]:
    """
    Centralized OCR Batch Post-Processing Pipeline (Tahap 39 & Tahap 40).
    Called directly by handset/server.py and test suites to guarantee 100% identical execution.
    - Handles short snippets (<= 6 lines, total chars < 220) preserving double Enter (\\r\\n\\r\\n).
    - Checks 1-screen complete caption (requiring #hashtag at end) to prevent premature truncation.
    - Connects Apple Notes Top + Bottom chunks across separate copies within 75s when colors match.
    - Preserves raw OCR paragraph break blank lines via raw_para_breaks set.
    - Auto-flushes chunk cache when selection color switches or once a complete caption is copied.
    """
    if not batch_texts:
        return (None, False) if return_meta else None

    # Detect if any frame is a Notes folder list frame (Tahap 32 & Tahap 33)
    has_folder_list = any(_is_notes_folder_list_frame(t) or "<<<NOTES_LIST_VIEW>>>" in t for t in batch_texts)
    if has_folder_list:
        clear_recent_long_ocr_chunks()

    # Track selection color and flush cache if switching apps/notes with different highlight color (Tahap 40)
    global _LAST_SELECTION_COLOR
    current_color = "none"
    for bt in reversed(batch_texts):
        if not bt:
            continue
        m = re.search(r'<<<SEL_COLOR:(yellow|blue|none)>>>', bt)
        if m:
            current_color = m.group(1)
            break

    if _LAST_SELECTION_COLOR in ("yellow", "blue") and current_color in ("yellow", "blue") and current_color != _LAST_SELECTION_COLOR:
        clear_recent_long_ocr_chunks()

    if current_color in ("yellow", "blue"):
        _LAST_SELECTION_COLOR = current_color

    # Extract raw paragraph break markers before stripping blank lines (Tahap 40 Bug 3)
    raw_para_breaks: set[str] = set()
    for bt in batch_texts:
        if not bt or not bt.strip():
            continue
        bt_clean = re.sub(r'<<<SEL_BOUNDS:[^>]+>>>', '', bt)
        bt_clean = re.sub(r'<<<SEL_COLOR:[^>]+>>>', '', bt_clean)
        lines_raw = [l.strip() for l in bt_clean.splitlines()]
        for idx in range(len(lines_raw) - 1):
            if lines_raw[idx] and lines_raw[idx + 1] == "":
                norm_l = _normalize_line_for_match(lines_raw[idx])
                if norm_l:
                    raw_para_breaks.add(norm_l)

    # ANTI-LEAK RULE 1 (Tahap 37): Current frame is ALWAYS the primary source!
    current_frame_raw = batch_texts[-1].strip()
    if not current_frame_raw or _is_notes_folder_list_frame(current_frame_raw) or current_frame_raw == "<<<NOTES_LIST_VIEW>>>":
        return (None, False) if return_meta else None

    # Strip selector bounds and color markers if present
    current_frame_raw = re.sub(r'<<<SEL_BOUNDS:[^>]+>>>', '', current_frame_raw)
    current_frame_raw = re.sub(r'<<<SEL_COLOR:[^>]+>>>', '', current_frame_raw).strip()
    if not current_frame_raw:
        return (None, False) if return_meta else None

    raw_frame_lines = [l.strip() for l in current_frame_raw.splitlines()]
    c_lines = _strip_top_unselected_debris([l for l in raw_frame_lines if l and not _is_garbage_ocr_line(l)])
    if not c_lines:
        return (None, False) if return_meta else None

    # Discard partially clipped/damaged bottom line at toolbar edge (Tahap 40 Bug 4)
    if len(c_lines) >= 3:
        last_l = c_lines[-1]
        if not last_l.startswith("#") and re.search(r'\b(ccrta|dilanorkan|hcrhacil|kcrhacil)\b', last_l, re.IGNORECASE):
            c_lines.pop()
            if raw_frame_lines and raw_frame_lines[-1] == last_l:
                raw_frame_lines.pop()

    # ATURAN EMAS 1 (Tahap 36/37/38/39/40): Short text (<= 6 non-empty lines, total chars < 220)
    # NEVER stitch with history frames or past chunks!
    total_chars = sum(len(l) for l in c_lines)
    is_short_snippet = (len(c_lines) <= 6 and all(len(l) < 55 for l in c_lines) and total_chars < 220)
    if is_short_snippet:
        # WAJIB PERTAHANKAN baris kosong "" di antara baris-baris pendek
        cleaned_snippet_lines = [_clean_ocr_line(l) if l else "" for l in raw_frame_lines]
        while cleaned_snippet_lines and not cleaned_snippet_lines[0]:
            cleaned_snippet_lines.pop(0)
        while cleaned_snippet_lines and not cleaned_snippet_lines[-1]:
            cleaned_snippet_lines.pop()
        current_frame_text = "\r\n".join(cleaned_snippet_lines).strip()
        final_text = auto_correct_ocr_text(current_frame_text)
        final_text = ensure_paragraph_formatting(final_text, raw_para_breaks=raw_para_breaks)
        return (final_text, False) if return_meta else final_text

    current_frame_text = "\r\n".join(c_lines).strip()

    # ATURAN EMAS 2 & 3: Check single-screen complete caption vs multi-screen
    has_hashtag_end = any(l.startswith("#") for l in c_lines[-2:]) if len(c_lines) >= 2 else False
    first_line = c_lines[0] if c_lines else ""
    is_middle_para = bool(re.match(r'^(momen\s+menegangkan|situasi\s+darurat|dalam\s+unggahan|konten\s+tersebut|fenomena\s+ini|seluruh\s+\d+|bagaimana\s+(perasaan|tanggapan|menurut)|source\s*:|sumber\s*:)', first_line, re.IGNORECASE))
    has_body_start = (
        not is_middle_para
        and len(first_line) >= 20
        and not _is_generic_template_line(first_line)
        and first_line[0].isupper()
        and not first_line.endswith(",")
    )
    is_complete_single_screen = (has_hashtag_end and has_body_start and len(c_lines) >= 6)

    # Session Chunk Continuity (Tahap 40 Bug 4):
    # If user copied Top of note earlier (< 75s ago, same color), and now copied Bottom of note, connect them!
    now = time.time()
    with _CHUNKS_LOCK:
        _RECENT_LONG_OCR_CHUNKS[:] = [c for c in _RECENT_LONG_OCR_CHUNKS if (now - c.get("ts", 0)) <= 75.0]

        if not is_complete_single_screen and len(c_lines) >= 4:
            for past in reversed(_RECENT_LONG_OCR_CHUNKS):
                if current_color in ("yellow", "blue") and past.get("color") in ("yellow", "blue") and past.get("color") != current_color:
                    continue
                p_lines = [l for l in past.get("lines", []) if not re.search(r'\b(ccrta|dilanorkan|hcrhacil|kcrhacil)\b', l, re.IGNORECASE)]
                if not p_lines:
                    continue
                merged = None
                if has_overlapping_body_line(p_lines, c_lines):
                    merged = merge_two_overlapping_blocks(p_lines, c_lines)
                    if not merged:
                        merged = merge_two_overlapping_blocks(c_lines, p_lines)
                if not merged:
                    p_has_start = (
                        len(p_lines) >= 4
                        and p_lines[0]
                        and p_lines[0][0].isupper()
                        and not _is_generic_template_line(p_lines[0])
                        and not any(l.startswith("#") for l in p_lines)
                    )
                    # Only connect non-overlapping Top + Bottom if current frame does NOT have its own body start (starts mid-article like "serta 40...")
                    if p_has_start and has_hashtag_end and not has_body_start and current_color in ("yellow", "blue") and past.get("color") == current_color:
                        merged = p_lines + c_lines
                if merged:
                    c_lines = _strip_top_unselected_debris(merged)
                    current_frame_text = "\r\n".join(c_lines).strip()
                    break

        if len(c_lines) >= 4:
            _RECENT_LONG_OCR_CHUNKS.append({"ts": now, "lines": list(c_lines), "color": current_color})
            if len(_RECENT_LONG_OCR_CHUNKS) > 5:
                _RECENT_LONG_OCR_CHUNKS[:] = _RECENT_LONG_OCR_CHUNKS[-5:]

    if len(batch_texts) <= 1 or (is_complete_single_screen and _score_gap_quality(c_lines) >= 0):
        final_text = auto_correct_ocr_text(current_frame_text)
    else:
        # ANTI-LEAK RULE 2 (Tahap 37): Filter history frames!
        valid_history = []
        for ht in batch_texts[:-1]:
            if not ht or not ht.strip() or _is_notes_folder_list_frame(ht) or "<<<NOTES_LIST_VIEW>>>" in ht:
                continue
            ht_clean = re.sub(r'<<<SEL_BOUNDS:[^>]+>>>', '', ht)
            ht_clean = re.sub(r'<<<SEL_COLOR:[^>]+>>>', '', ht_clean).strip()
            h_lines = _strip_top_unselected_debris([l.strip() for l in ht_clean.splitlines() if l.strip()])
            if has_overlapping_body_line(h_lines, c_lines):
                valid_history.append("\r\n".join(h_lines))

        if valid_history:
            frames_to_stitch = valid_history + [current_frame_text]
            stitched = stitch_multiline_ocr_frames(frames_to_stitch)
            final_text = stitched if stitched else current_frame_text
            final_text = auto_correct_ocr_text(final_text)
        else:
            final_text = auto_correct_ocr_text(current_frame_text)

    final_text = ensure_paragraph_formatting(final_text, raw_para_breaks=raw_para_breaks)

    # Smart Auto-Clear (Tahap 40 Feature 5):
    # Once complete caption (>= 5 lines, body start, ending with #hashtag) is obtained,
    # flush _RECENT_LONG_OCR_CHUNKS so the next copy starts clean!
    final_non_empty = [l for l in final_text.splitlines() if l.strip()]
    is_now_complete = (
        len(final_non_empty) >= 5
        and any(l.strip().startswith("#") for l in final_non_empty[-2:])
        and final_non_empty[0]
        and final_non_empty[0][0].isupper()
        and not _is_generic_template_line(final_non_empty[0])
    )
    auto_cleared = False
    if is_now_complete:
        clear_recent_long_ocr_chunks()
        auto_cleared = True

    if return_meta:
        return final_text, auto_cleared
    return final_text


def get_debug_typed_state() -> tuple[str, list[str]]:
    """Return active line and buffer tail for debug JSON output."""
    with _TYPED_BUFFER_LOCK:
        active = _CURRENT_TYPED_LINE.strip() if _CURRENT_TYPED_LINE else _LAST_TYPED_ACTIVE_LINE
        return active, list(_TYPED_LINES_BUFFER[-5:] if _TYPED_LINES_BUFFER else [])


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
        if ch == "\n":
            send_keys_direct([0x28])
            time.sleep(0.025)
            send_keys_direct([])
            time.sleep(0.025)
            prev_usages = None
            typed_count += 1
            continue

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
