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


def load_typed_history() -> None:
    """Load persisted typed history from disk on startup."""
    global _LAST_TYPED_ACTIVE_LINE, _LAST_TYPED_TIMESTAMP, _TYPED_LINES_BUFFER
    if not TYPED_HISTORY_FILE.exists():
        return
    try:
        with open(TYPED_HISTORY_FILE, "r", encoding="utf-8") as f:
            data = json.load(f)
        with _TYPED_BUFFER_LOCK:
            _LAST_TYPED_ACTIVE_LINE = data.get("last_active", "")
            _LAST_TYPED_TIMESTAMP = float(data.get("timestamp", 0.0))
            _TYPED_LINES_BUFFER = list(data.get("buffer", []))
    except Exception:
        pass


# Load history on startup
load_typed_history()


def commit_and_reset_current_typed_line() -> None:
    """Commit current typed accumulator into buffer if valid (>=3 chars) and reset accumulator."""
    global _CURRENT_TYPED_LINE, _TYPED_LINES_BUFFER, _LAST_TYPED_ACTIVE_LINE, _LAST_TYPED_TIMESTAMP
    import time
    with _TYPED_BUFFER_LOCK:
        trimmed = _CURRENT_TYPED_LINE.strip()
        if len(trimmed) >= 3:
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
                if len(trimmed) >= 3:
                    _LAST_TYPED_ACTIVE_LINE = trimmed
                _LAST_TYPED_TIMESTAMP = time.time()
        elif char in ("\n", "\r", "Enter"):
            trimmed = _CURRENT_TYPED_LINE.strip()
            if len(trimmed) >= 3:
                if not _TYPED_LINES_BUFFER or _TYPED_LINES_BUFFER[-1] != trimmed:
                    _TYPED_LINES_BUFFER.append(trimmed)
                if len(_TYPED_LINES_BUFFER) > 60:
                    _TYPED_LINES_BUFFER.pop(0)
                _LAST_TYPED_ACTIVE_LINE = trimmed
                _LAST_TYPED_TIMESTAMP = time.time()
            # Reset current line accumulator for next sentence, but KEEP _LAST_TYPED_ACTIVE_LINE intact!
            _CURRENT_TYPED_LINE = ""
        elif char in ("Space", " "):
            _CURRENT_TYPED_LINE += " "
            trimmed = _CURRENT_TYPED_LINE.strip()
            if len(trimmed) >= 3:
                _LAST_TYPED_ACTIVE_LINE = trimmed
            _LAST_TYPED_TIMESTAMP = time.time()
        elif len(char) == 1:
            _CURRENT_TYPED_LINE += char
            trimmed = _CURRENT_TYPED_LINE.strip()
            if len(trimmed) >= 3:
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
                if len(trimmed) >= 3:
                    if not _TYPED_LINES_BUFFER or _TYPED_LINES_BUFFER[-1] != trimmed:
                        _TYPED_LINES_BUFFER.append(trimmed)
                    if len(_TYPED_LINES_BUFFER) > 60:
                        _TYPED_LINES_BUFFER.pop(0)
                    _LAST_TYPED_ACTIVE_LINE = trimmed
                    _LAST_TYPED_TIMESTAMP = time.time()
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
            if ratio >= 0.55:
                return active_line

    if not raw_ocr:
        return active_line if has_valid_active else ""

    ocr_lines = [l.strip() for l in raw_ocr.splitlines() if len(l.strip()) >= 3]
    if not ocr_lines:
        ocr_lines = [l.strip() for l in raw_ocr.splitlines() if l.strip()]

    # If no valid candidates in history buffer, return clean OCR text directly
    if not candidates:
        result = "\r\n".join(ocr_lines)
        return result

    # --- Priority 2: Match OCR against historical candidates in typed_history / buffer ---
    full_ocr = " ".join(ocr_lines).strip()
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

        # Containment (for multi-word sentences)
        if len(c_lower) >= 8 and (c_lower in full_ocr_lower or full_ocr_lower in c_lower):
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

    if best_score >= 0.55 and best_cand:
        return best_cand

    # --- Priority 3: Line-by-line correction ---
    corrected_lines = []
    for line in ocr_lines:
        l_lower = line.lower()
        line_best = line
        line_ratio = 0.0
        for cand in candidates:
            c_strip = cand.strip()
            if len(c_strip) < 3:
                continue
            if len(line) >= 5 and not _is_length_comparable(c_strip, line):
                continue
            c_lower = c_strip.lower()
            if len(line) >= 6 and (l_lower in c_lower or c_lower in l_lower):
                line_best = c_strip
                line_ratio = 1.0
                break
            r = difflib.SequenceMatcher(None, l_lower, c_lower).ratio()
            if r > line_ratio:
                line_ratio = r
                line_best = c_strip
        if line_ratio >= 0.55:
            corrected_lines.append(line_best)
        else:
            corrected_lines.append(line)

    result = "\r\n".join(corrected_lines)
    # STRICT SAFETY GUARD: Never return 1 or 2 chars if longer text exists
    if len(result) <= 2:
        if has_valid_active:
            return active_line
        if raw_ocr and len(raw_ocr) > len(result):
            return raw_ocr
    return result


_RECENT_LONG_OCR_CHUNKS: list[dict] = []  # items: {"ts": float, "lines": list[str]}
_CHUNKS_LOCK = threading.Lock()


def _normalize_line_for_match(text: str) -> str:
    """Normalize line for fuzzy overlap matching: lowercase, strip, condense whitespace."""
    if not text:
        return ""
    cleaned = re.sub(r'[\r\n\t]+', ' ', text.lower()).strip()
    return re.sub(r'\s{2,}', ' ', cleaned)


def merge_two_overlapping_blocks(linesA: list[str], linesB: list[str]) -> list[str] | None:
    """
    Bidirectional Interior Anchor Alignment:
    Finds consistent overlapping lines (>= 18 chars, ratio >= 0.78), detects whether
    linesA is top (offset > 0) or linesB is top (offset < 0), and splits at the
    interior middle anchor (matches[len // 2]).
    Corrupted top lines (clipped scroll edge) and corrupted bottom lines
    (e.g. Notes floating toolbar 'aO/O3124an') are cleanly excluded.
    """
    if not linesA or not linesB:
        return None

    cleanA = [l.strip() for l in linesA if l and l.strip()]
    cleanB = [l.strip() for l in linesB if l and l.strip()]
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

    # Group by offset i - j to find dominant alignment
    offset_counts: dict[int, int] = {}
    for m in matches:
        off = m[3]
        offset_counts[off] = offset_counts.get(off, 0) + 1

    dom_offset, _ = max(offset_counts.items(), key=lambda kv: kv[1])
    consistent_matches = [m for m in matches if abs(m[3] - dom_offset) <= 1]
    if not consistent_matches:
        consistent_matches = matches

    consistent_matches.sort(key=lambda m: m[0])

    # Check if one block is a complete subset of the other
    first_i, first_j = consistent_matches[0][0], consistent_matches[0][1]
    last_i, last_j = consistent_matches[-1][0], consistent_matches[-1][1]
    remA = len(cleanA) - 1 - last_i
    remB = len(cleanB) - 1 - last_j

    if first_j >= first_i and remB >= remA:
        return cleanB
    if first_i >= first_j and remA >= remB:
        return cleanA

    # Interior middle anchor
    mid_idx = len(consistent_matches) // 2
    split_i, split_j, _, _ = consistent_matches[mid_idx]

    if dom_offset > 0:
        # linesA is TOP, linesB is BOTTOM
        merged = cleanA[:split_i] + cleanB[split_j:]
        return merged
    elif dom_offset < 0:
        # linesB is TOP, linesA is BOTTOM (e.g. user scrolled upwards)
        merged = cleanB[:split_j] + cleanA[split_i:]
        return merged
    else:
        # dom_offset == 0: aligned at start; take longer version
        return cleanA if len(cleanA) >= len(cleanB) else cleanB


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

    lines1 = [l.strip() for l in t1_clean.splitlines() if l.strip()]
    lines2 = [l.strip() for l in t2_clean.splitlines() if l.strip()]

    # Try interior anchor alignment first
    interior_merged = merge_two_overlapping_blocks(lines1, lines2)
    if interior_merged:
        return "\r\n".join(interior_merged)

    # Find best overlap between bottom of lines1 and top of lines2
    best_match = None
    best_score = 0.0

    for i in range(len(lines1) - 1, -1, -1):
        l1 = lines1[i]
        if not l1 or len(l1) < 4:
            continue
        for j in range(0, min(len(lines2), 20)):
            l2 = lines2[j]
            if not l2 or len(l2) < 4:
                continue

            l1_low = l1.lower()
            l2_low = l2.lower()

            sm = difflib.SequenceMatcher(None, l1_low, l2_low).ratio()
            if len(l1_low) >= 8 and (l1_low in l2_low or l2_low in l1_low):
                sm = max(sm, 0.85)

            w1 = set(l1_low.split())
            w2 = set(l2_low.split())
            if len(w1) >= 3 and len(w2) >= 3:
                w_overlap = len(w1 & w2) / float(max(len(w1), len(w2)))
                if w_overlap >= 0.70:
                    sm = max(sm, w_overlap)

            if sm >= 0.72:
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

    if not best_match:
        return t2_clean

    i, j, l1, l2 = best_match
    longer_line = l1 if len(l1) >= len(l2) else l2
    stitched_lines = lines1[:i] + [longer_line] + lines2[j + 1:]
    return "\r\n".join(stitched_lines)


def stitch_multiline_ocr_frames(texts: list[str]) -> str:
    """Stitch multiple chronologically ordered OCR frames into a seamless single multi-paragraph text."""
    valid_texts = [t.strip() for t in texts if t and t.strip()]
    if not valid_texts:
        return ""

    current_lines = [l.strip() for l in valid_texts[0].splitlines() if l.strip()]

    for nxt in valid_texts[1:]:
        nxt_lines = [l.strip() for l in nxt.splitlines() if l.strip()]
        if not nxt_lines:
            continue
        merged = merge_two_overlapping_blocks(current_lines, nxt_lines)
        if merged:
            current_lines = merged
        else:
            t_merged = stitch_two_ocr_texts("\r\n".join(current_lines), "\r\n".join(nxt_lines))
            current_lines = [l.strip() for l in t_merged.splitlines() if l.strip()]

    # If long note (>= 4 lines), check and merge with session chunk memory (_RECENT_LONG_OCR_CHUNKS)
    if len(current_lines) >= 4:
        now = time.time()
        with _CHUNKS_LOCK:
            global _RECENT_LONG_OCR_CHUNKS
            _RECENT_LONG_OCR_CHUNKS = [c for c in _RECENT_LONG_OCR_CHUNKS if (now - c["ts"]) <= 90.0]

            for past in reversed(_RECENT_LONG_OCR_CHUNKS):
                hist_merged = merge_two_overlapping_blocks(current_lines, past["lines"])
                if hist_merged and len(hist_merged) >= len(current_lines):
                    current_lines = hist_merged

            _RECENT_LONG_OCR_CHUNKS.append({"ts": now, "lines": list(current_lines)})
            if len(_RECENT_LONG_OCR_CHUNKS) > 5:
                _RECENT_LONG_OCR_CHUNKS = _RECENT_LONG_OCR_CHUNKS[-5:]

    return "\r\n".join(current_lines)


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
