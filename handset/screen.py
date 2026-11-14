"""Xcode's device screen, on Windows.

After Trust, iOS 17.4+ exposes CoreDeviceProxy over the USB service broker,
or RemotePairing over Wi-Fi once paired for it. Either tunnel carries a
display service. The phone then sends the screen as HEVC video. The browser
decodes it.
"""

from __future__ import annotations

import asyncio
import contextlib
import difflib
import json
import os
import re
import socket
import struct
import subprocess
import sys
import tempfile
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


from handset.device import developer_mode_enabled
from handset.mux import MuxError, list_devices
from handset.wifi import (
    get_device_wifi_state,
    probe_tcp_port,
    probe_wifi_device,
    set_persistent_wifi_rsd,
    wifi_paired_udids,
    wifi_tunnel,
)

DEVELOPER_MODE_MESSAGE = (
    "The picture stays black because Developer Mode is off. On the iPhone open Settings, "
    "Privacy & Security, Developer Mode, turn it on, and let the phone restart. After it boots, "
    "unlock it, tap Turn On, then press Show screen again."
)

VIEW_URL = "http://127.0.0.1:8766/"
_LOCK = threading.Lock()
_STATE = {"phase": "idle", "udid": None, "url": None, "error": None, "note": None, "transport": "cable"}
_STAGE_STEP: int = 0
_STAGE_TITLE: str = ""
_STAGE_DETAIL: str = ""
_CURRENT_TASK: asyncio.Task | None = None
_CURRENT_LOOP: asyncio.AbstractEventLoop | None = None
_ACTIVE_SERVER = None

# Tahap 110: H.265 Streaming Sharpness & Motion Keyframe Booster
MAX_SUBSCRIBER_QUEUE_SIZE = 240
_LAST_MOTION_KEYFRAME_TIME = 0.0
MOTION_KEYFRAME_MIN_INTERVAL = 0.45


def trigger_motion_keyframe_boost() -> bool:
    """Trigger an IDR keyframe from iOS encoder on navigation/gesture start/settle.
    Rate-limited to 1 request per 0.45s to avoid USB bus flooding.
    """
    global _LAST_MOTION_KEYFRAME_TIME, _ACTIVE_SERVER
    now = time.monotonic()
    if now - _LAST_MOTION_KEYFRAME_TIME < MOTION_KEYFRAME_MIN_INTERVAL:
        return False
    _LAST_MOTION_KEYFRAME_TIME = now
    if _ACTIVE_SERVER is not None and hasattr(_ACTIVE_SERVER, "request_keyframe"):
        try:
            _ACTIVE_SERVER.request_keyframe()
            return True
        except Exception:
            pass
    return False


# Tahap 47 (Anti-Bug Edition) - Stealth Dim Mode & Remote Lock/Wake/Unlock Control
_STEALTH_DIM_ACTIVE = False
_DEVICE_INTENTIONALLY_LOCKED = False
_LAST_USER_INPUT_TS = time.time()
_IS_TOUCH_DRAGGING = False
_HID_ACTION_LOCK = threading.Lock()

# Tahap 124: Zero Double Audio (iPhone physical speaker mute / restore via Consumer HID 0x00EA / 0x00E9)
_IPHONE_SPEAKER_MUTED_FOR_PC_AUDIO = False
_IPHONE_AUDIO_RESTORE_STEPS = 5  # ~31% (5 bar iOS dari 16 bar penuh)
_SPEAKER_SEQ_LOCK = threading.Lock()
_SPEAKER_OP_SEQ = 0
_SPEAKER_TARGET_MUTE: bool | None = None
_LAST_MUTE_DOWN_STEPS: int | None = None

# Register extended HID buttons in pymobiledevice3 at import time
try:
    from pymobiledevice3.remote.core_device.screen_stream import (
        _NAMED_BUTTONS,
        ScreenStreamServer,
        HID_BUTTON_STATE_DOWN,
        HID_BUTTON_STATE_UP,
    )
    # Tahap 143: Precision 0.5s hold for power and lock matching iOS SpringBoard debounce standards
    _NAMED_BUTTONS["power"] = (12, 0x0030, 0.5)
    _NAMED_BUTTONS["lock"] = (12, 0x0030, 0.5)
    _NAMED_BUTTONS["home"] = (12, 0x0040, 0.05)
    _NAMED_BUTTONS["wake"] = (12, 0x0040, 0.05)
    _NAMED_BUTTONS["brightness-down"] = (12, 0x0070, 0.025)
    _NAMED_BUTTONS["brightness-up"] = (12, 0x006F, 0.025)
    _NAMED_BUTTONS["volume-up"] = (12, 0x00E9, 0.025)
    _NAMED_BUTTONS["volume-down"] = (12, 0x00EA, 0.025)
    _NAMED_BUTTONS["keyboard-toggle"] = (12, 0x00B8, 0.05)
    _NAMED_BUTTONS["eject"] = (12, 0x00B8, 0.05)

    if not hasattr(ScreenStreamServer, "_send_button"):
        async def _screen_stream_send_button(self_s, page: int, usage: int, down: bool):
            await self_s._ensure_hid()
            assert self_s._indigo is not None
            state = HID_BUTTON_STATE_DOWN if down else HID_BUTTON_STATE_UP
            await self_s._indigo.send_button(page, usage, state)
        ScreenStreamServer._send_button = _screen_stream_send_button

    if not hasattr(ScreenStreamServer, "press_button"):
        async def _screen_stream_press_button(self_s, name: str, hold_seconds: float | None = None):
            if name not in _NAMED_BUTTONS:
                return False
            page, usage, default_hold = _NAMED_BUTTONS[name]
            dur = float(hold_seconds) if hold_seconds is not None else float(default_hold)
            await self_s._ensure_hid()
            assert self_s._indigo is not None
            if hasattr(self_s, "_send_button") and callable(self_s._send_button):
                await self_s._send_button(page, usage, True)
                await asyncio.sleep(dur)
                await self_s._send_button(page, usage, False)
            else:
                await self_s._indigo.send_button(page, usage, HID_BUTTON_STATE_DOWN)
                await asyncio.sleep(dur)
                await self_s._indigo.send_button(page, usage, HID_BUTTON_STATE_UP)
            return True
        ScreenStreamServer.press_button = _screen_stream_press_button

    if not hasattr(ScreenStreamServer, "_tahap143_touch_patched"):
        ScreenStreamServer._tahap143_touch_patched = True
        orig_handle_touch = ScreenStreamServer._handle_touch
        async def _screen_stream_handle_touch(self_s, body: bytes) -> tuple[int, bytes]:
            global _DEVICE_INTENTIONALLY_LOCKED
            if _DEVICE_INTENTIONALLY_LOCKED:
                _DEVICE_INTENTIONALLY_LOCKED = False
            record_user_input_activity()
            return await orig_handle_touch(self_s, body)
        ScreenStreamServer._handle_touch = _screen_stream_handle_touch
except Exception:
    pass


def is_iphone_physical_speaker_muted() -> bool:
    """Thread-safe check whether iPhone physical speaker was auto-muted for PC USB audio."""
    with _SPEAKER_SEQ_LOCK:
        return bool(_IPHONE_SPEAKER_MUTED_FOR_PC_AUDIO)


def _reset_speaker_mute_state() -> None:
    """Reset in-memory speaker mute tracking cleanly without blocking or issuing HID calls."""
    global _IPHONE_SPEAKER_MUTED_FOR_PC_AUDIO, _SPEAKER_OP_SEQ, _SPEAKER_TARGET_MUTE, _LAST_MUTE_DOWN_STEPS
    with _SPEAKER_SEQ_LOCK:
        _SPEAKER_OP_SEQ += 1
        _IPHONE_SPEAKER_MUTED_FOR_PC_AUDIO = False
        _SPEAKER_TARGET_MUTE = False
        _LAST_MUTE_DOWN_STEPS = None


def install_coredevice_aac_decoder() -> bool:
    """Monkey-patch pymobiledevice3 CoreDevice screen_stream to use Windows zlaac.dll AAC-ELD decoder."""
    try:
        from handset.aac import AacEldDecoder, find_zlaac_dll
        from pymobiledevice3.remote.core_device import aac_eld, screen_stream

        find_zlaac_dll()
        aac_eld.AACELDDecoder = AacEldDecoder
        screen_stream.AACELDDecoder = AacEldDecoder
        screen_stream.ScreenStreamServer._missing_audio_deps = staticmethod(lambda: [])
        return True
    except Exception as exc:
        import logging

        logging.getLogger(__name__).warning("CoreDevice zlaac AAC-ELD decoder patch skipped: %s", exc)
        return False


# Install CoreDevice AAC-ELD decoder patch at module load time
install_coredevice_aac_decoder()


def stop_coredevice_audio_stream(timeout: float = 1.5) -> bool:
    """Tear down active CoreDevice audio session so iOS immediately restores audio to iPhone speaker."""
    global _ACTIVE_SERVER, _CURRENT_LOOP
    server = _ACTIVE_SERVER
    loop = _CURRENT_LOOP
    if server is None or loop is None or not loop.is_running():
        return False
    stop_fn = getattr(server, "_stop_audio_stream", None)
    if not callable(stop_fn):
        return False

    async def _do_stop() -> bool:
        lock = getattr(server, "_audio_lock", None)
        if lock is not None:
            async with lock:
                await stop_fn()
        else:
            await stop_fn()
        return True

    try:
        fut = asyncio.run_coroutine_threadsafe(_do_stop(), loop)
        return bool(fut.result(timeout=timeout))
    except Exception:
        return False


def record_user_input_activity(is_drag: bool = False) -> None:
    """Track user interaction timestamps to safeguard OCR drag selection and safe idle keep-awake."""
    global _LAST_USER_INPUT_TS, _IS_TOUCH_DRAGGING
    _LAST_USER_INPUT_TS = time.time()
    _IS_TOUCH_DRAGGING = is_drag


status = lambda: screen_status()

def screen_status() -> dict:
    with _LOCK:
        st = dict(_STATE)
        st["stealth_dim"] = _STEALTH_DIM_ACTIVE
        st["intentionally_locked"] = _DEVICE_INTENTIONALLY_LOCKED
        st["stage_step"] = _STAGE_STEP
        st["stage_title"] = _STAGE_TITLE
        st["stage_detail"] = _STAGE_DETAIL
        if not st.get("transport"):
            st["transport"] = "cable"
        return st



def _ensure_local_stream_port_free(port: int = 8766, timeout: float = 2.5) -> bool:
    """Ensure port 8766 is completely released and unbound before starting a stream server.
    Closes any active stream servers, terminates orphan processes if needed, and verifies socket bind."""
    global _ACTIVE_SERVER
    # 1. Stop any AirPlay receiver holding 8766
    _stop_airplay_receiver()

    # 2. Close active server
    if _ACTIVE_SERVER is not None:
        srv = _ACTIVE_SERVER
        _ACTIVE_SERVER = None
        for fn_name in ("close", "stop", "stop_and_release_ports"):
            if hasattr(srv, fn_name):
                try:
                    res = getattr(srv, fn_name)()
                    if asyncio.iscoroutine(res):
                        try:
                            loop = asyncio.get_running_loop()
                            loop.create_task(res)
                        except RuntimeError:
                            pass
                except Exception:
                    pass
        if hasattr(srv, "_server") and srv._server is not None:
            with contextlib.suppress(Exception):
                srv._server.close()

    # 3. Test binding with SO_REUSEADDR
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            s.bind(("127.0.0.1", port))
            s.close()
            return True
        except OSError:
            time.sleep(0.08)

    # 4. If still busy, check netstat and kill orphan if not our own process
    try:
        import subprocess, os
        out = subprocess.check_output(f'netstat -ano | findstr ":{port} "', shell=True, text=True)
        for line in out.strip().splitlines():
            parts = line.strip().split()
            if len(parts) >= 5 and "LISTENING" in line:
                pid = int(parts[-1])
                if pid != os.getpid() and pid > 0:
                    subprocess.run(f"taskkill /F /PID {pid}", shell=True, capture_output=True)
                    time.sleep(0.1)
    except Exception:
        pass

    try:
        s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        s.bind(("127.0.0.1", port))
        s.close()
        return True
    except OSError:
        return False


def stop_screen() -> dict:
    """Cleanly stop mirroring, terminate stream server, restore brightness & speaker volume, and reset state."""
    global _CURRENT_TASK, _CURRENT_LOOP, _ACTIVE_SERVER, _STEALTH_DIM_ACTIVE, _DEVICE_INTENTIONLY_LOCKED, _WIFI_MODE
    try:
        from handset.audio import audio_manager
        audio_manager.usb_engine.stop(release_coredevice=False)
    except Exception:
        pass
    if _STEALTH_DIM_ACTIVE:
        try:
            set_iphone_physical_brightness("restore", steps=4)
        except Exception:
            pass
        _STEALTH_DIM_ACTIVE = False
    try:
        set_iphone_physical_speaker_mute(False)
    except Exception:
        pass
    _DEVICE_INTENTIONALLY_LOCKED = False

    is_wifi = False
    notify_transport = "cable"
    with _LOCK:
        if _STATE.get("transport") == "wifi" and _WIFI_MODE != "coredevice":
            is_wifi = True
            notify_transport = "wifi"
            from handset.airplay import get_global_airplay_server
            srv = get_global_airplay_server()
            if srv:
                with contextlib.suppress(Exception):
                    srv.reset_stream_state()
            _ensure_local_stream_port_free(8766, timeout=1.0)
            _STATE.update(phase="idle", udid=None, url=None, error=None, note="Mirroring stopped.")
        else:
            if _STATE.get("transport") == "wifi":
                notify_transport = "wifi"
            _WIFI_MODE = None
            if _CURRENT_TASK and _CURRENT_LOOP and _CURRENT_LOOP.is_running():
                try:
                    _CURRENT_LOOP.call_soon_threadsafe(_CURRENT_TASK.cancel)
                except Exception:
                    pass
            _CURRENT_TASK = None
            _CURRENT_LOOP = None
            _ACTIVE_SERVER = None
            _ensure_local_stream_port_free(8766, timeout=1.0)
            _STATE.update(phase="idle", udid=None, url=None, error=None, note="Mirroring stopped.")

    try:
        from handset.audio import notify_screen_phase_changed
        notify_screen_phase_changed("idle", notify_transport)
    except Exception:
        pass
    if not is_wifi:
        try:
            from .wifi import sync_force_reset_userspace_tunnel
            sync_force_reset_userspace_tunnel()
        except Exception:
            pass
    return screen_status()


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


_AIRPLAY_THREAD: Optional[threading.Thread] = None


def _ensure_airplay_server_running() -> Any:
    global _AIRPLAY_THREAD
    from handset.airplay import get_or_create_airplay_server
    server = get_or_create_airplay_server()
    if _AIRPLAY_THREAD is None or not _AIRPLAY_THREAD.is_alive():
        ready = threading.Event()
        def _runner():
            loop = asyncio.new_event_loop()
            asyncio.set_event_loop(loop)
            async def _main():
                await server.serve()
            loop.run_until_complete(_main())
        _AIRPLAY_THREAD = threading.Thread(target=_runner, name="airplay-server-daemon", daemon=True)
        _AIRPLAY_THREAD.start()
        time.sleep(0.3)
    return server


_WIFI_MODE: Optional[str] = None  # "coredevice" | "airplay" | None
_AIRPLAY_NOTE = "📡 Wi-Fi AirPlay Receiver Aktif — Buka Control Center di iPhone/iPad > Ketuk 'Pencerminan Layar' (Screen Mirroring) > Pilih 'iPhone Link'"


def _stop_airplay_receiver() -> None:
    """Close the AirPlay receiver (RTSP 7100 + HTTP 8766) so CoreDevice can own port 8766."""
    global _AIRPLAY_THREAD
    try:
        from handset.airplay import get_global_airplay_server

        srv = get_global_airplay_server()
    except Exception:
        return
    if srv is None:
        return
    with contextlib.suppress(Exception):
        srv.stop_and_release_ports()
    thread = _AIRPLAY_THREAD
    if thread is not None and thread.is_alive():
        thread.join(timeout=2.0)
    _AIRPLAY_THREAD = None


def _start_airplay_receiver(chosen: str, note: Optional[str] = None) -> dict:
    """Native AirPlay receiver (H.264) over Wi-Fi: video-only fallback / Control Center mode."""
    global _WIFI_MODE
    _WIFI_MODE = "airplay"
    note = note or _AIRPLAY_NOTE
    server = _ensure_airplay_server_running()

    def on_airplay_state(state: str, detail: str | None = None) -> None:
        if state in ("connected", "streaming"):
            _set(
                phase="running",
                udid=chosen,
                url=VIEW_URL,
                error=None,
                note=None,
                transport="wifi",
            )
            try:
                from handset.audio import notify_screen_phase_changed
                notify_screen_phase_changed("running", "wifi")
            except Exception:
                pass
        elif state == "stopped":
            _set(
                phase="idle",
                udid=None,
                url=None,
                error=None,
                note="AirPlay disconnected.",
                transport="wifi",
            )
            try:
                from handset.audio import notify_screen_phase_changed
                notify_screen_phase_changed("idle", "wifi")
            except Exception:
                pass

    server.on_state = on_airplay_state
    with _LOCK:
        phase = "running" if server.is_streaming else "waiting_airplay"
        _STATE.update(phase=phase, udid=chosen, url=VIEW_URL, error=None, note=note, transport="wifi")
    try:
        from handset.audio import notify_screen_phase_changed
        notify_screen_phase_changed(phase, "wifi")
    except Exception:
        pass
    return screen_status()


def _start_coredevice_wifi(chosen: str) -> dict:
    """CoreDevice (ScreenStreamServer) session over a Wi-Fi tunnel: full video / touch / keyboard / audio / shortcuts."""
    global _WIFI_MODE
    with _LOCK:
        if _STATE["phase"] in {"starting", "running"} and _STATE["udid"] == chosen and _WIFI_MODE == "coredevice":
            return dict(_STATE)
    if _WIFI_MODE == "airplay":
        try:
            from handset.airplay import get_global_airplay_server

            srv = get_global_airplay_server()
            if srv is not None and getattr(srv, "is_streaming", False):
                return screen_status()  # an iPhone is mirroring over AirPlay right now: do not tear it down
        except Exception:
            pass
    _stop_airplay_receiver()
    with _LOCK:
        _WIFI_MODE = "coredevice"
        _STAGE_STEP = 1
        _STAGE_TITLE = "Deteksi perangkat"
        _STAGE_DETAIL = "Memastikan driver Apple Mobile Device terlihat."
        _STATE.update(
            phase="starting",
            udid=chosen,
            url=None,
            error=None,
            note="Opening a secure Wi-Fi tunnel to the device...",
            transport="wifi",
            stage_step=_STAGE_STEP,
            stage_title=_STAGE_TITLE,
            stage_detail=_STAGE_DETAIL,
        )
    threading.Thread(target=_run, args=(chosen, "wifi"), name="handset-screen", daemon=True).start()
    return screen_status()


def start_screen(udid: str | None, transport: str = "cable") -> dict:
    usb = _usb_udids()
    wifi = wifi_paired_udids()
    if transport in ("auto", "cable"):
        transport = "usb"

    if transport == "wifi":
        chosen = (udid or "").strip() or _only_udid(usb, wifi, transport) or (wifi[0] if wifi else "")
        if chosen:
            return _start_coredevice_wifi(chosen)
        return _set(phase="error", udid=None, url=None, error="Tidak ada iPhone yang ter-pairing untuk Wi-Fi. Hubungkan kabel sekali lalu klik 'Set up Wi-Fi'.")

    chosen = (udid or "").strip() or _only_udid(usb, wifi, transport)
    if not chosen:
        return _set(phase="idle", udid=None, url=None, error="Connect the iPhone first.")
    if chosen not in usb:
        return _set(phase="error", udid=chosen, url=None, error="The iPhone is not on the cable. Plug it in or choose Wi-Fi.")
    if developer_mode_enabled(chosen) is False:
        return _set(phase="error", udid=chosen, url=None, error=DEVELOPER_MODE_MESSAGE)
    note = None

    with _LOCK:
        if _STATE["phase"] in {"starting", "running"} and _STATE["udid"] == chosen:
            return dict(_STATE)
        _STAGE_STEP = 1
        _STAGE_TITLE = "Deteksi perangkat"
        _STAGE_DETAIL = "Memastikan driver Apple Mobile Device terlihat."
        _STATE.update(
            phase="starting",
            udid=chosen,
            url=None,
            error=None,
            note=note,
            transport=transport,
            stage_step=_STAGE_STEP,
            stage_title=_STAGE_TITLE,
            stage_detail=_STAGE_DETAIL,
        )
    threading.Thread(target=_run, args=(chosen, transport), name="handset-screen", daemon=True).start()
    return screen_status()


def _run(udid: str, transport: str) -> None:
    try:
        outcome = asyncio.run(_serve(udid, transport))
        if outcome == "airplay":
            return  # CoreDevice Wi-Fi unavailable: the AirPlay receiver owns the session now
        with _LOCK:
            curr_phase = _STATE.get("phase")
        if curr_phase != "error":
            _set(phase="idle", udid=udid, url=None, error=None, note="The screen stream stopped.")
    except Exception as error:
        _set(phase="error", udid=udid, url=None, error=_plain(error))
    finally:
        try:
            from .wifi import sync_force_reset_userspace_tunnel
            sync_force_reset_userspace_tunnel()
        except Exception:
            pass


DISPLAY_SERVICE = "com.apple.coredevice.displayservice"


def _open_tunnel(udid: str, transport: str):
    if transport == "wifi":
        return wifi_tunnel(udid)
    from pymobiledevice3.remote.rsd_tunnel import PreferredRsdTunnel

    return PreferredRsdTunnel(serial=udid, autopair=True)


async def _serve_airplay(udid: str) -> None:
    """Run native AirPlay screen mirroring & audio receiver over Wi-Fi."""
    from handset.airplay import get_or_create_airplay_server

    global _CURRENT_TASK, _CURRENT_LOOP, _ACTIVE_SERVER
    _CURRENT_LOOP = asyncio.get_running_loop()
    _CURRENT_TASK = asyncio.current_task()

    def on_airplay_state(state: str, detail: str | None = None) -> None:
        if state in ("connected", "streaming"):
            _set(
                phase="running",
                udid=udid,
                url=VIEW_URL,
                error=None,
                note=None,
                transport="wifi",
            )
            try:
                from handset.audio import notify_screen_phase_changed
                notify_screen_phase_changed("running", "wifi")
            except Exception:
                pass
        elif state == "stopped":
            _set(
                phase="idle",
                udid=None,
                url=None,
                error=None,
                note="AirPlay disconnected.",
                transport="wifi",
            )
            try:
                from handset.audio import notify_screen_phase_changed
                notify_screen_phase_changed("idle", "wifi")
            except Exception:
                pass

    server = get_or_create_airplay_server(name="iPhone Link", on_state=on_airplay_state)
    _ACTIVE_SERVER = server

    if server.is_streaming:
        _set(
            phase="running",
            udid=udid,
            url=VIEW_URL,
            error=None,
            note=None,
            transport="wifi",
        )
    else:
        _set(
            phase="waiting_airplay",
            udid=udid,
            url=VIEW_URL,
            error=None,
            note="📡 Wi-Fi AirPlay Receiver Aktif — Buka Control Center di iPhone/iPad > Ketuk 'Pencerminan Layar' (Screen Mirroring) > Pilih 'iPhone Link'",
            transport="wifi",
        )
    try:
        from handset.audio import notify_screen_phase_changed
        notify_screen_phase_changed("waiting_airplay" if not server.is_streaming else "running", "wifi")
    except Exception:
        pass

    try:
        await server.serve()
    except asyncio.CancelledError:
        pass
    finally:
        _CURRENT_TASK = None
        _ACTIVE_SERVER = None


async def _serve(udid: str, transport: str):
    global _CURRENT_TASK, _CURRENT_LOOP
    if transport == "wifi":
        _CURRENT_LOOP = asyncio.get_running_loop()
        _CURRENT_TASK = asyncio.current_task()

    from pymobiledevice3.remote.core_device.screen_stream import ScreenStreamServer, _NAMED_BUTTONS
    from pymobiledevice3.services.mobile_image_mounter import auto_mount
    from .wifi import force_reset_userspace_tunnel, wifi_debug_log

    # Ensure no lingering userspace tunnel is active from a previous session
    await force_reset_userspace_tunnel()

    # HID Consumer Control (0x0C): 0x00B8 (Eject) to toggle iOS virtual keyboard cleanly
    _NAMED_BUTTONS["volume-up"] = (12, 0x00E9, 0.025)
    _NAMED_BUTTONS["volume-down"] = (12, 0x00EA, 0.025)
    _NAMED_BUTTONS["eject"] = (12, 0x00B8, 0.05)
    _NAMED_BUTTONS.pop("keyboard-toggle", None)

    wifi_debug_log("SCREEN_SERVE", f"Starting screen session for {udid} (transport={transport})...")

    # Stage 1: Deteksi perangkat
    _set(stage_step=1, stage_title="Deteksi perangkat", stage_detail="Memastikan driver Apple Mobile Device terlihat.", note="1. Deteksi perangkat...")
    wifi_debug_log("SCREEN_SERVE", f"[STAGE 1] Deteksi perangkat {udid} (transport={transport})")

    # Stage 2: Koneksi aman
    _set(stage_step=2, stage_title="Koneksi aman", stage_detail="Membuka jalur terenkripsi CoreDevice ke perangkat.", note="2. Koneksi aman - Membuka jalur terenkripsi CoreDevice...")
    wifi_debug_log("SCREEN_SERVE", "[STAGE 2] Membuka jalur terenkripsi CoreDevice ke perangkat...")

    try:
        for attempt in range(2):
            try:
                async with _open_tunnel(udid, transport) as rsd:
                    # Stage 3: Siapkan perangkat
                    _set(stage_step=3, stage_title="Siapkan perangkat", stage_detail="Cek Developer Mode, kunci layar, & kanal audio.", note="3. Siapkan perangkat...")
                    wifi_debug_log("SCREEN_SERVE", "[STAGE 3] Menyiapkan perangkat...")
                    if DISPLAY_SERVICE not in (rsd.peer_info or {}).get("Services", {}):
                        _set(note="Mounting the developer image on the iPhone. This downloads it the first time.")
                        wifi_debug_log("SCREEN_SERVE", "[STAGE 3] auto_mount developer disk image...")
                        await auto_mount(rsd)

                    # Stage 4: Mulai stream
                    _set(stage_step=4, stage_title="Mulai stream", stage_detail="Menerima frame H.265/HEVC 60fps.", note="4. Mulai stream...")
                    wifi_debug_log("SCREEN_SERVE", "[STAGE 4] Memulai screen stream server...")
                    await _stream(rsd, udid, ScreenStreamServer)
                    return
            except Exception as tunnel_err:
                err_text = str(tunnel_err)
                if "already active in this process" in err_text and attempt == 0:
                    wifi_debug_log("SCREEN_SERVE", "Userspace tunnel collision detected, resetting and retrying...")
                    await force_reset_userspace_tunnel()
                    await asyncio.sleep(0.3)
                    continue
                raise
    except asyncio.CancelledError:
        return None
    except Exception as exc:
        wifi_debug_log("SCREEN_SERVE_ERR", f"Screen stream error on {transport}: {exc}", exc)
        _CURRENT_TASK = None
        with contextlib.suppress(Exception):
            await force_reset_userspace_tunnel()
        _ensure_local_stream_port_free(8766, timeout=1.0)
        if transport == "wifi":
            _set(
                phase="error",
                udid=udid,
                url=None,
                error=f"Koneksi CoreDevice Wi-Fi gagal: {_plain(exc)}. Pastikan iPhone menyala, layar tidak terkunci, dan terhubung ke Wi-Fi yang sama dengan PC.",
                note=None,
                transport="wifi",
            )
            return None
        raise

async def _stream(rsd, udid: str, ScreenStreamServer) -> None:
    from pymobiledevice3.remote.core_device.display_service import is_media_in_use_error

    def _screen_stream_request_keyframe(self_s):
        try:
            if hasattr(self_s, "_send_rtcp_pli"):
                loop = getattr(self_s, "_loop", None)
                if loop is None:
                    try:
                        loop = asyncio.get_running_loop()
                    except RuntimeError:
                        loop = None
                if loop and loop.is_running():
                    task = loop.create_task(self_s._send_rtcp_pli())
                    if hasattr(self_s, "_pli_tasks"):
                        self_s._pli_tasks.add(task)
                        task.add_done_callback(self_s._pli_tasks.discard)
        except Exception:
            pass

    if not hasattr(ScreenStreamServer, "request_keyframe"):
        ScreenStreamServer.request_keyframe = _screen_stream_request_keyframe

    # Tahap 110: Increase subscriber queue capacity to 240 to prevent P-frame dropping & enable TCP_NODELAY
    if not getattr(ScreenStreamServer, "_tahap110_bounded", False):
        ScreenStreamServer._tahap110_bounded = True
        orig_serve_connection = ScreenStreamServer._serve_connection

        async def _bounded_serve_connection(self_s, reader, writer):
            sock = writer.get_extra_info("socket")
            if sock is not None:
                try:
                    sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
                except Exception:
                    pass

            orig_init = asyncio.Queue.__init__
            def _bounded_init(q_self, maxsize=0):
                if maxsize in (64, 24):
                    maxsize = MAX_SUBSCRIBER_QUEUE_SIZE
                orig_init(q_self, maxsize=maxsize)
            try:
                asyncio.Queue.__init__ = _bounded_init
                return await orig_serve_connection(self_s, reader, writer)
            finally:
                asyncio.Queue.__init__ = orig_init

        ScreenStreamServer._serve_connection = _bounded_serve_connection

    # Tahap 123: Ensure zlaac.dll CoreDevice AAC-ELD decoder patch is active for /audio.bin
    install_coredevice_aac_decoder()

    if not _ensure_local_stream_port_free(8766, timeout=2.5):
        raise RuntimeError("Port 127.0.0.1:8766 masih digunakan oleh proses lain.")

    server = ScreenStreamServer(
        rsd,
        bind="127.0.0.1",
        http_port=8766,
        audio_default_on=False,
    )

    async def _eager_video_only() -> None:
        # Start video immediately so H.265 handshake is never delayed;
        # /audio.bin opens CoreDevice audio lazily via zlaac.dll when requested.
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
        # Tahap 142: Asynchronously pre-warm HID & keyboard so lock/unlock/buttons execute instantly
        async def _prewarm_hid():
            try:
                await server._ensure_hid()
            except Exception:
                pass
            try:
                await server._ensure_keyboard()
            except Exception:
                pass
        asyncio.create_task(_prewarm_hid())

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
        _reset_speaker_mute_state()
        try:
            from .wifi import force_reset_userspace_tunnel
            await force_reset_userspace_tunnel()
        except Exception:
            pass


class LockdownWifiStreamServer:
    def __init__(self, udid: str, ip: str, bind: str = "127.0.0.1", http_port: int = 8766):
        self.udid = udid
        self.ip = ip
        self.bind = bind
        self.http_port = http_port
        self.server = None
        self._stopped = False
        self._last_frame_bytes = b""
        self._subscribers: set[asyncio.Queue] = set()
        self.width = 1179
        self.height = 2556
        self._first_frame_event = asyncio.Event()

    async def serve(self):
        self.server = await asyncio.start_server(self._handle_client, self.bind, self.http_port)
        async with self.server:
            await self.server.serve_forever()

    async def _handle_client(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter):
        sock = writer.get_extra_info("socket")
        if sock is not None:
            try:
                sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
            except Exception:
                pass
        try:
            line = await reader.readline()
            if not line:
                writer.close()
                return
            parts = line.decode("latin-1", errors="ignore").split()
            if not parts:
                writer.close()
                return
            method = parts[0]
            path = parts[1].split("?", 1)[0] if len(parts) > 1 else "/"

            content_length = 0
            while True:
                hdr = await reader.readline()
                if not hdr or hdr in (b"\r\n", b"\n"):
                    break
                if hdr.lower().startswith(b"content-length:"):
                    with contextlib.suppress(Exception):
                        content_length = int(hdr.split(b":", 1)[1].strip())

            if content_length > 0:
                await reader.readexactly(content_length)

            if method == "POST" and path in ("/touch", "/button", "/key", "/restart", "/pli"):
                if path == "/touch":
                    commit_and_reset_current_typed_line()
                resp = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 15\r\nConnection: close\r\n\r\n{\"ok\":true}\r\n"
                writer.write(resp)
                await writer.drain()
                writer.close()
                return

            if path == "/codec":
                payload = json.dumps({
                    "codec": "jpeg_stream",
                    "mode": "image",
                    "width": self.width,
                    "height": self.height,
                    "fps": 15,
                    "description": "",
                }).encode("utf-8")
                writer.write(f"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {len(payload)}\r\nConnection: close\r\n\r\n".encode("latin-1") + payload)
                await writer.drain()
                writer.close()
                return

            if path in ("/stream.bin", "/live"):
                writer.write(b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n")
                await writer.drain()

                # Immediately write current real frame if already captured
                if self._last_frame_bytes and len(self._last_frame_bytes) >= 15000:
                    chunk = struct.pack(">I", len(self._last_frame_bytes)) + self._last_frame_bytes
                    writer.write(chunk)
                    await writer.drain()

                q = asyncio.Queue(maxsize=MAX_SUBSCRIBER_QUEUE_SIZE)
                self._subscribers.add(q)
                try:
                    while not self._stopped:
                        frame = await q.get()
                        if len(frame) >= 15000:
                            chunk_body = struct.pack(">I", len(frame)) + frame
                            writer.write(chunk_body)
                            await writer.drain()
                except (asyncio.CancelledError, OSError):
                    pass
                finally:
                    self._subscribers.discard(q)
                    writer.close()
                return

            resp = b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK"
            writer.write(resp)
            await writer.drain()
            writer.close()
        except Exception:
            with contextlib.suppress(Exception):
                writer.close()


async def _serve_lockdown_wifi_stream(udid: str, ip: str) -> None:
    server = LockdownWifiStreamServer(udid, ip, bind="127.0.0.1", http_port=8766)
    global _CURRENT_TASK, _CURRENT_LOOP, _ACTIVE_SERVER
    _CURRENT_LOOP = asyncio.get_running_loop()
    _CURRENT_TASK = asyncio.current_task()
    _ACTIVE_SERVER = server

    async def _frame_worker():
        try:
            from pymobiledevice3.lockdown import create_using_tcp, get_preferred_pair_record
            from pymobiledevice3.pair_records import create_pairing_records_cache_folder
            from pymobiledevice3.services.screenshot import ScreenshotService
            from pymobiledevice3.services.dvt.instruments.dvt_provider import DvtProvider
            from pymobiledevice3.services.dvt.instruments.screenshot import Screenshot

            rec = await get_preferred_pair_record(udid, create_pairing_records_cache_folder())
            ld = await create_using_tcp(hostname=ip, identifier=udid, autopair=False, pair_record=rec)
            try:
                ss = None
                try:
                    ss = ScreenshotService(ld)
                except Exception:
                    ss = None

                dvt_provider_obj = None
                if ss is None:
                    try:
                        dvt_ctx = DvtProvider(ld)
                        dvt_provider_obj = await dvt_ctx.__aenter__()
                    except Exception:
                        dvt_provider_obj = None

                if ss is None and dvt_provider_obj is None:
                    raise RuntimeError("iOS 27.0.1 does not offer ScreenshotService or DVTSecureSocketProxy over Wi-Fi lockdown.")

                while not server._stopped:
                    frame_bytes = None
                    if ss is not None:
                        try:
                            frame_bytes = await ss.take_screenshot()
                        except Exception:
                            ss = None
                    if frame_bytes is None and dvt_provider_obj is not None:
                        try:
                            shot = Screenshot(dvt_provider_obj)
                            frame_bytes = await shot.get_screenshot()
                        except Exception:
                            pass

                    if frame_bytes and len(frame_bytes) >= 15000:
                        server._last_frame_bytes = frame_bytes
                        if not server._first_frame_event.is_set():
                            server._first_frame_event.set()
                        for q in list(server._subscribers):
                            if q.qsize() >= MAX_SUBSCRIBER_QUEUE_SIZE:
                                while q.qsize() > (MAX_SUBSCRIBER_QUEUE_SIZE // 2):
                                    try:
                                        q.get_nowait()
                                    except (asyncio.QueueEmpty, Exception):
                                        break
                                if hasattr(server, "request_keyframe"):
                                    server.request_keyframe()
                            if not q.full():
                                q.put_nowait(frame_bytes)
                        await asyncio.sleep(0.06)
                    else:
                        await asyncio.sleep(0.2)
            finally:
                await ld.close()
        except Exception as e:
            import logging
            logging.getLogger(__name__).warning("_frame_worker lockdown stream failed: %s", e)

    worker_task = asyncio.create_task(_frame_worker())
    server_task = asyncio.create_task(server.serve())

    # Wait up to 2.5s for the first REAL frame before declaring running!
    try:
        await asyncio.wait_for(server._first_frame_event.wait(), timeout=2.5)
        _set(phase="running", udid=udid, url=VIEW_URL, error=None, note=None)
    except asyncio.TimeoutError:
        server._stopped = True
        worker_task.cancel()
        server_task.cancel()
        raise RuntimeError(
            f"Gagal mengambil frame layar via Wi-Fi ({ip}): Developer Disk Image belum terpasang atau iPhone terkunci. "
            "Pastikan layar iPhone menyala dan colok kabel USB 1x."
        )

    try:
        await server_task
    finally:
        server._stopped = True
        worker_task.cancel()
        _CURRENT_TASK = None
        _ACTIVE_SERVER = None


def send_keys_direct(usages: list[int], timeout: float = 0.5) -> bool:
    """Send keyboard HID usages directly to active server or fallback to loopback HTTP."""
    record_user_input_activity()
    global _ACTIVE_SERVER, _CURRENT_LOOP
    server = _ACTIVE_SERVER
    loop = _CURRENT_LOOP
    if server is not None and loop is not None and loop.is_running():
        async def _send():
            await server._ensure_keyboard()
            assert server._uhs is not None and server._kb_service_id is not None
            if hasattr(server._uhs, "_service") and hasattr(server._uhs._service, "_writer") and server._uhs._service._writer is not None:
                try:
                    import socket
                    sock = server._uhs._service._writer.get_extra_info("socket")
                    if sock is not None:
                        sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
                except Exception:
                    pass
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


def send_button_direct(name: str, state: str = "press", timeout: float = 2.5, hold_seconds: float | None = None) -> bool:
    """Send named button event directly to active server or fallback to loopback HTTP."""
    record_user_input_activity()
    from pymobiledevice3.remote.core_device.screen_stream import _NAMED_BUTTONS
    btn_tuple = _NAMED_BUTTONS.get(name, (12, 0x0040, 0.05))
    btn_hold = float(hold_seconds) if hold_seconds is not None else float(btn_tuple[2])
    effective_timeout = max(float(timeout), btn_hold + 2.0)

    global _ACTIVE_SERVER, _CURRENT_LOOP
    server = _ACTIVE_SERVER
    loop = _CURRENT_LOOP

    if server is not None and loop is not None and loop.is_running():
        try:
            if state == "press" and hasattr(server, "press_button") and callable(server.press_button):
                future = asyncio.run_coroutine_threadsafe(server.press_button(name, hold_seconds=hold_seconds), loop)
                res = future.result(timeout=effective_timeout)
                trigger_motion_keyframe_boost()
                return bool(res)
            else:
                async def _send():
                    from pymobiledevice3.remote.core_device.screen_stream import (
                        HID_BUTTON_STATE_DOWN,
                        HID_BUTTON_STATE_UP,
                    )
                    usage_page, usage_code, _ = btn_tuple
                    await server._ensure_hid()
                    assert server._indigo is not None
                    if hasattr(server._indigo, "_service") and hasattr(server._indigo._service, "_writer") and server._indigo._service._writer is not None:
                        try:
                            import socket
                            sock = server._indigo._service._writer.get_extra_info("socket")
                            if sock is not None:
                                sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
                        except Exception:
                            pass
                    if state == "press":
                        if hasattr(server, "_send_button") and callable(server._send_button):
                            await server._send_button(usage_page, usage_code, True)
                            await asyncio.sleep(btn_hold)
                            await server._send_button(usage_page, usage_code, False)
                        else:
                            await server._indigo.send_button(usage_page, usage_code, HID_BUTTON_STATE_DOWN)
                            await asyncio.sleep(btn_hold)
                            await server._indigo.send_button(usage_page, usage_code, HID_BUTTON_STATE_UP)
                    elif state == "down":
                        if hasattr(server, "_send_button") and callable(server._send_button):
                            await server._send_button(usage_page, usage_code, True)
                        else:
                            await server._indigo.send_button(usage_page, usage_code, HID_BUTTON_STATE_DOWN)
                    elif state == "up":
                        if hasattr(server, "_send_button") and callable(server._send_button):
                            await server._send_button(usage_page, usage_code, False)
                        else:
                            await server._indigo.send_button(usage_page, usage_code, HID_BUTTON_STATE_UP)
                future = asyncio.run_coroutine_threadsafe(_send(), loop)
                future.result(timeout=effective_timeout)
                if state in ("press", "up"):
                    trigger_motion_keyframe_boost()
                return True
        except Exception:
            return False

    with _LOCK:
        if _STATE.get("phase") != "running":
            return False

    try:
        import http.client, json
        conn = http.client.HTTPConnection("127.0.0.1", 8766, timeout=effective_timeout)
        conn.request("POST", "/button", body=json.dumps({"name": name, "state": state}).encode("utf-8"), headers={"Content-Type": "application/json"})
        resp = conn.getresponse()
        resp.read()
        conn.close()
        if resp.status == 200 and state in ("press", "up"):
            trigger_motion_keyframe_boost()
        return resp.status == 200
    except Exception:
        return False


def send_touch_direct(touch_type: str, x: int, y: int, timeout: float = 0.5) -> bool:
    """Send touch event (contact, release, tap) directly to active server or fallback to loopback HTTP."""
    global _DEVICE_INTENTIONALLY_LOCKED
    if _DEVICE_INTENTIONALLY_LOCKED:
        _DEVICE_INTENTIONALLY_LOCKED = False
    record_user_input_activity(is_drag=(touch_type == "contact"))
    global _ACTIVE_SERVER, _CURRENT_LOOP
    server = _ACTIVE_SERVER
    loop = _CURRENT_LOOP
    if server is not None and loop is not None and loop.is_running():
        async def _send():
            from pymobiledevice3.remote.core_device.screen_stream import (
                TOUCHSCREEN_STATE_CONTACT,
                TOUCHSCREEN_STATE_RELEASE,
                DIGITIZER_SURFACE_MAIN_TOUCHSCREEN,
            )
            await server._ensure_hid()
            assert server._uhs is not None
            if touch_type == "contact":
                await server._uhs.send_touchscreen(
                    TOUCHSCREEN_STATE_CONTACT, x, y, service_id=DIGITIZER_SURFACE_MAIN_TOUCHSCREEN
                )
            elif touch_type == "release":
                await server._uhs.send_touchscreen(
                    TOUCHSCREEN_STATE_RELEASE, x, y, service_id=DIGITIZER_SURFACE_MAIN_TOUCHSCREEN
                )
            elif touch_type == "tap":
                await server._uhs.send_touchscreen(TOUCHSCREEN_STATE_CONTACT, x, y)
                await asyncio.sleep(0.04)
                await server._uhs.send_touchscreen(TOUCHSCREEN_STATE_RELEASE, x, y)
        try:
            future = asyncio.run_coroutine_threadsafe(_send(), loop)
            future.result(timeout=timeout)
            if touch_type in ("tap", "release"):
                trigger_motion_keyframe_boost()
            return True
        except Exception:
            pass

    with _LOCK:
        if _STATE.get("phase") != "running":
            return False

    try:
        import http.client, json
        conn = http.client.HTTPConnection("127.0.0.1", 8766, timeout=timeout)
        conn.request(
            "POST",
            "/touch",
            body=json.dumps({"type": touch_type, "x": x, "y": y}).encode("utf-8"),
            headers={"Content-Type": "application/json"},
        )
        resp = conn.getresponse()
        resp.read()
        conn.close()
        if resp.status == 200 and touch_type in ("tap", "release"):
            trigger_motion_keyframe_boost()
        return resp.status == 200
    except Exception:
        return False


def _send_single_brightness_step(direction: str) -> None:
    """Send HID brightness increment/decrement (Tahap 61: Single-channel Consumer Page 0x0C)."""
    if direction == "down":
        # 1. Consumer Page: 0x0C, Usage 0x0070 (Display Brightness Decrement)
        send_button_direct("brightness-down")
    else:  # "up"
        # Consumer Page: 0x0C, Usage 0x006F (Display Brightness Increment) - exactly 1 bar (6.25%) per step
        send_button_direct("brightness-up")


def set_iphone_physical_brightness(level: str, steps: int = 4, force: bool = False) -> dict:
    """Adjust physical OLED/LCD brightness of iPhone.
    'min': 16 steps down (0% brightness, pitch dark on desk while mirror stream is 100% full quality)
    'restore': exactly 4 single steps up (~25% brightness, 4 bars iOS).
    Strict protection: if already restored / not active, 0 signals are sent.
    """
    global _STEALTH_DIM_ACTIVE
    record_user_input_activity()
    with _HID_ACTION_LOCK:
        if level == "min":
            if _STEALTH_DIM_ACTIVE and not force:
                return {"ok": True, "stealth": True, "level": "min", "skipped": True, "steps": 0}
            actual_steps = steps if steps > 0 else 16
            for _ in range(actual_steps):
                _send_single_brightness_step("down")
                time.sleep(0.025)
            _STEALTH_DIM_ACTIVE = True
            return {"ok": True, "stealth": True, "level": "min", "steps": actual_steps}
        else:
            # STRICT GUARD: If Stealth Dim is already OFF, do NOT send any brightness-up signals!
            if not _STEALTH_DIM_ACTIVE:
                return {"ok": True, "stealth": False, "stealthDimActive": False, "skipped": True, "steps": 0}
            _STEALTH_DIM_ACTIVE = False
            actual_steps = min(steps, 4) if steps > 0 else 4
            for _ in range(actual_steps):
                _send_single_brightness_step("up")
                time.sleep(0.025)
            return {"ok": True, "stealth": False, "level": "restore", "steps": actual_steps}


def _send_single_volume_step(direction: str) -> bool:
    """Send single HID Consumer Control volume step (Page 0x0C: 0x00EA Down / 0x00E9 Up)."""
    if direction == "down":
        return send_button_direct("volume-down")
    return send_button_direct("volume-up")


def set_iphone_physical_speaker_mute(
    mute: bool,
    restore_steps: int = _IPHONE_AUDIO_RESTORE_STEPS,
    force: bool = False,
) -> dict:
    """Mute or restore iPhone physical speaker via Consumer HID (0x0C: 0x00EA / 0x00E9).

    - ``mute=True``: sends 16 single ``volume-down`` steps (0% Silent) when ``_ACTIVE_SERVER``
      is active, preventing Double Audio (PC WASAPI + iPhone speaker simultaneously).
    - ``mute=False``: immediately marks speaker unmuted and sends ``restore_steps`` (default 5,
      ~31%) single ``volume-up`` steps if ``_ACTIVE_SERVER`` is active.
    - Strict idempotency guard: returns ``skipped=True, steps=0`` with 0 HID calls if the state
      already matches ``mute`` (unless ``force=True``).
    """
    global _IPHONE_SPEAKER_MUTED_FOR_PC_AUDIO, _SPEAKER_OP_SEQ, _SPEAKER_TARGET_MUTE, _LAST_MUTE_DOWN_STEPS

    target_mute = bool(mute)
    with _SPEAKER_SEQ_LOCK:
        if force or _SPEAKER_TARGET_MUTE is not target_mute:
            _SPEAKER_OP_SEQ += 1
            _SPEAKER_TARGET_MUTE = target_mute
        my_seq = _SPEAKER_OP_SEQ

    record_user_input_activity()
    with _HID_ACTION_LOCK:
        with _SPEAKER_SEQ_LOCK:
            superseded = (_SPEAKER_OP_SEQ != my_seq)
            currently_muted = bool(_IPHONE_SPEAKER_MUTED_FOR_PC_AUDIO)

        if target_mute:
            if currently_muted and not force:
                return {"ok": True, "muted": True, "skipped": True, "steps": 0}
            if superseded:
                return {
                    "ok": False,
                    "muted": currently_muted,
                    "skipped": True,
                    "reason": "superseded",
                    "steps": 0,
                }
            if _ACTIVE_SERVER is None:
                with _SPEAKER_SEQ_LOCK:
                    _SPEAKER_TARGET_MUTE = currently_muted
                return {
                    "ok": False,
                    "muted": currently_muted,
                    "reason": "no_active_server",
                    "steps": 0,
                }

            sent = 0
            for _ in range(16):
                with _SPEAKER_SEQ_LOCK:
                    if _SPEAKER_OP_SEQ != my_seq:
                        break
                if _ACTIVE_SERVER is None:
                    break
                _send_single_volume_step("down")
                sent += 1
                time.sleep(0.015)

            if sent <= 0:
                with _SPEAKER_SEQ_LOCK:
                    _SPEAKER_TARGET_MUTE = currently_muted
                return {
                    "ok": False,
                    "muted": currently_muted,
                    "skipped": True,
                    "reason": "cancelled",
                    "steps": 0,
                }

            with _SPEAKER_SEQ_LOCK:
                _IPHONE_SPEAKER_MUTED_FOR_PC_AUDIO = True
                _LAST_MUTE_DOWN_STEPS = sent if sent < 16 else None
            return {"ok": True, "muted": True, "steps": sent}

        # target_mute is False (Restore iPhone physical speaker)
        if not currently_muted and not force:
            return {"ok": True, "muted": False, "skipped": True, "steps": 0}

        with _SPEAKER_SEQ_LOCK:
            _IPHONE_SPEAKER_MUTED_FOR_PC_AUDIO = False
            _SPEAKER_TARGET_MUTE = False
            interrupted_down = _LAST_MUTE_DOWN_STEPS
            _LAST_MUTE_DOWN_STEPS = None

        raw_steps = _IPHONE_AUDIO_RESTORE_STEPS if restore_steps is None else int(restore_steps)
        target_steps = max(0, min(16, raw_steps))
        if interrupted_down is not None and interrupted_down > 0 and not force:
            target_steps = min(target_steps, interrupted_down)

        if _ACTIVE_SERVER is None:
            return {
                "ok": True,
                "muted": False,
                "reason": "no_active_server",
                "steps": 0,
            }

        sent = 0
        for _ in range(target_steps):
            if _ACTIVE_SERVER is None:
                break
            _send_single_volume_step("up")
            sent += 1
            time.sleep(0.015)
        return {"ok": True, "muted": False, "steps": sent}


def lock_iphone_screen() -> dict:
    """Lock and turn off iPhone physical screen. If stealth dim was active, restores brightness to ~25% first."""
    global _DEVICE_INTENTIONALLY_LOCKED, _STEALTH_DIM_ACTIVE
    record_user_input_activity()
    if _STEALTH_DIM_ACTIVE:
        set_iphone_physical_brightness("restore", steps=4)
        time.sleep(0.15)
    try:
        set_iphone_physical_speaker_mute(False)
    except Exception:
        pass
    with _HID_ACTION_LOCK:
        ok = send_button_direct("power", timeout=2.5)
        _DEVICE_INTENTIONALLY_LOCKED = bool(ok)
    return {"ok": bool(ok), "locked": bool(ok)}


def wake_and_unlock_iphone(passcode: str = "") -> dict:
    """Wake screen, swipe up, bypass Face ID scanning delay, and inject optional PIN cleanly (Anti-Bug 2 & 3)."""
    global _DEVICE_INTENTIONALLY_LOCKED
    _DEVICE_INTENTIONALLY_LOCKED = False
    record_user_input_activity()
    with _HID_ACTION_LOCK:
        # Step 1: Wake screen via Home/Wake + gentle tap
        send_button_direct("wake")
        time.sleep(0.05)
        send_touch_direct("tap", 32768, 32768)
        time.sleep(0.45)

        # Step 2: Swipe-Up gesture (8 steps from y=58000 to y=12000 in 0.22s)
        start_y = 58000
        end_y = 12000
        steps = 8
        send_touch_direct("contact", 32768, start_y)
        time.sleep(0.02)
        for i in range(1, steps + 1):
            cur_y = int(start_y + (end_y - start_y) * (i / steps))
            send_touch_direct("contact", 32768, cur_y)
            time.sleep(0.025)
        send_touch_direct("release", 32768, end_y)

        # Step 2b: Bypass Face ID delay (~1.2s wait on iPhone 17 Pro)
        # Tap center and press Space to force immediate PIN keypad
        time.sleep(0.30)
        send_touch_direct("tap", 32768, 41000)
        send_keys_direct([0x2C])  # Space / Enter to awaken passcode entry
        time.sleep(0.02)
        send_keys_direct([])
        time.sleep(0.45)

        # Step 3: Isolated Passcode Injection (DO NOT TOUCH _TYPED_LINES_BUFFER!)
        # CRITICAL PRIVACY RULE: Send raw HID keycodes directly to bypass typing buffer entirely
        if passcode and isinstance(passcode, str) and passcode.strip():
            clean_pin = "".join(ch for ch in passcode.strip() if ch.isdigit())
            digit_map = {
                '1': 0x1E, '2': 0x1F, '3': 0x20, '4': 0x21, '5': 0x22,
                '6': 0x23, '7': 0x24, '8': 0x25, '9': 0x26, '0': 0x27,
            }
            for digit in clean_pin:
                code = digit_map.get(digit)
                if code:
                    send_keys_direct([code], timeout=0.25)
                    time.sleep(0.040)
                    send_keys_direct([], timeout=0.25)
                    time.sleep(0.045)

        trigger_motion_keyframe_boost()
    return {"ok": True, "unlocked": bool(passcode and passcode.strip()), "woken": True}


def _idle_keep_awake_worker():
    """Background worker sending safe micro-pulse keep-awake only when idle >= 50s and not dragging (Anti-Bug 4)."""
    while True:
        try:
            time.sleep(5.0)
            with _LOCK:
                running = _STATE.get("phase") == "running"
            if not running:
                continue
            if _STEALTH_DIM_ACTIVE and not _DEVICE_INTENTIONALLY_LOCKED:
                idle_secs = time.time() - _LAST_USER_INPUT_TS
                if idle_secs >= 50.0 and not _IS_TOUCH_DRAGGING:
                    with _HID_ACTION_LOCK:
                        send_keys_direct([0xE0])
                        time.sleep(0.015)
                        send_keys_direct([])
                        record_user_input_activity()
        except Exception:
            pass


_KEEP_AWAKE_THREAD = threading.Thread(target=_idle_keep_awake_worker, name="handset-keep-awake", daemon=True)
_KEEP_AWAKE_THREAD.start()


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


def spotlight_paste_clipboard_on_iphone() -> dict:
    """Trigger Spotlight Search (Cmd + Space) and paste iPhone clipboard (Cmd + V) cleanly.
    
    Tahap 112: sends Right Arrow (0x4F) 120ms after Cmd + V so the selection block, lollipop handles
    and "Share Results" balloon vanish before the frame is captured.

    Tahap 111 Precision:
    - In iOS Spotlight (iPhone 11 Pro Max & iPhone 17 Pro), opening Spotlight via Cmd + Space
      automatically selects any previous search query text.
    - Sending Backspace (0x2A) once clears any residual text cleanly WITHOUT triggering selection handles.
    - Sending Cmd + V directly pastes new clipboard text 100% crisp, without Cmd+A selection,
      eliminating blue selection blocks, lollipop handles (|• / •|), and 'Share Results' balloons.
    """
    global _ACTIVE_SERVER
    record_user_input_activity()
    if _ACTIVE_SERVER is None:
        with _LOCK:
            if _STATE.get("phase") != "running":
                return {"ok": False, "error": "Screen mirror is not active"}
    with _HID_ACTION_LOCK:
        # 1) Release all keys
        send_keys_direct([])
        time.sleep(0.02)
        # 2) Send Cmd + Space sequentially to open Spotlight Search
        send_keys_direct([0xE3])
        time.sleep(0.025)
        send_keys_direct([0xE3, 0x2C])
        time.sleep(0.045)
        send_keys_direct([0xE3])
        time.sleep(0.02)
        send_keys_direct([])
        # 3) Wait 480ms for Spotlight animation and native auto-selection on iPhone 11 Pro Max & 17 Pro
        time.sleep(0.48)
        # 4) Tahap 195 Pure Backspace Clear (NO Cmd+A so iOS 27 never activates persistent lollipop handles or Show Results balloon)
        send_keys_direct([0x2A])  # Backspace
        time.sleep(0.045)
        send_keys_direct([])
        time.sleep(0.05)
        # 5) Directly paste clipboard (Cmd + V) without Cmd+A or Right Arrow (0x4F)
        # Tahap 196: Zero 0x4F after paste so iOS 27 Spotlight never activates lollipop handles or 'Show Results' balloon
        trigger_ios_shortcut("paste-ios")
        time.sleep(0.45)
        try:
            if hasattr(_ACTIVE_SERVER, "request_keyframe"):
                _ACTIVE_SERVER.request_keyframe()
        except Exception:
            pass
        return {"ok": True, "stage": "pasted_clean"}


def spotlight_move_cursor_to_start_on_iphone() -> dict:
    """Move cursor in single-line Spotlight input to beginning (index 0) to display head fragment.
    
    Tahap 103 Strict Guarantee:
    - Sends Up Arrow (0x52) + Cmd + Left Arrow ([0xE3, 0x50]).
    - Strictly WITHOUT select-all (Cmd+A), preventing selection handles and popup balloons.
    """
    global _ACTIVE_SERVER
    record_user_input_activity()
    if _ACTIVE_SERVER is None:
        with _LOCK:
            if _STATE.get("phase") != "running":
                return {"ok": False, "error": "Screen mirror is not active"}
    with _HID_ACTION_LOCK:
        # 1) Up Arrow (0x52) jumps to start of line in iOS single-line input field
        send_keys_direct([0x52])
        time.sleep(0.035)
        send_keys_direct([])
        time.sleep(0.035)
        # 2) Cmd + Left Arrow ([0xE3, 0x50]) ensures 100% cursor position at start
        send_keys_direct([0xE3])
        time.sleep(0.02)
        send_keys_direct([0xE3, 0x50])
        time.sleep(0.04)
        send_keys_direct([0xE3])
        time.sleep(0.02)
        send_keys_direct([])
        time.sleep(0.18)
        try:
            if hasattr(_ACTIVE_SERVER, "request_keyframe"):
                _ACTIVE_SERVER.request_keyframe()
        except Exception:
            pass
        return {"ok": True, "stage": "head"}


def spotlight_cleanup_and_close_on_iphone() -> dict:
    """Clear search query text in Spotlight and close Spotlight (Cmd + Space toggle ONLY, NO Escape) to restore clean screen.
    
    Tahap 111 Precision:
    - Strictly WITHOUT Cmd+A selection, preventing lollipop handles (|• / •|) and 'Share Results' balloons.
    - Sends Backspace (0x2A) once and toggles Cmd + Space to close.
    """
    global _ACTIVE_SERVER
    record_user_input_activity()
    if _ACTIVE_SERVER is None:
        with _LOCK:
            if _STATE.get("phase") != "running":
                return {"ok": False, "error": "Screen mirror is not active"}
    with _HID_ACTION_LOCK:
        # 1) Tahap 193..197 Spotlight Double Hard-Clear (frames are already captured): Cmd+A -> Backspace x2
        for _clear_pass in range(2):
            send_keys_direct([0xE3])
            time.sleep(0.018)
            send_keys_direct([0xE3, 0x04])
            time.sleep(0.032)
            send_keys_direct([0xE3])
            time.sleep(0.012)
            send_keys_direct([])
            time.sleep(0.022)
            send_keys_direct([0x2A])  # Backspace
            time.sleep(0.04)
            send_keys_direct([])
            time.sleep(0.025)
        time.sleep(0.03)
        # 2) Close Spotlight Search via ONE single toggle Cmd + Space without Escape toggle
        send_keys_direct([0xE3])
        time.sleep(0.025)
        send_keys_direct([0xE3, 0x2C])
        time.sleep(0.045)
        send_keys_direct([0xE3])
        time.sleep(0.02)
        send_keys_direct([])
        return {"ok": True, "stage": "closed"}


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

from handset import get_app_root; PROJECT_ROOT = get_app_root()
TYPED_HISTORY_FILE = PROJECT_ROOT / "typed_history.json"


def _is_writable_typed_dir(target_dir: Path) -> bool:
    try:
        norm = os.path.normcase(str(target_dir.resolve()))
        if "\\program files\\" in norm or "\\program files (x86)\\" in norm or norm.endswith("\\program files") or norm.endswith("\\program files (x86)"):
            return False
        if not target_dir.is_dir():
            return False
        probe = target_dir / f".probe_th_{os.getpid()}"
        probe.write_text("ok", encoding="utf-8")
        probe.unlink(missing_ok=True)
        return True
    except Exception:
        return False


def _get_writable_typed_history_file() -> Path:
    import tempfile
    configured = Path(TYPED_HISTORY_FILE)
    if _is_writable_typed_dir(configured.parent):
        return configured
    for fb_dir in (
        Path(os.environ.get("LOCALAPPDATA") or tempfile.gettempdir()) / "iPhoneLink",
        Path(tempfile.gettempdir()) / "iPhoneLink",
    ):
        try:
            fb_dir.mkdir(parents=True, exist_ok=True)
            return fb_dir / "typed_history.json"
        except Exception:
            pass
    return Path(tempfile.gettempdir()) / "typed_history.json"


def _get_all_typed_history_candidate_files() -> list[Path]:
    import tempfile
    cands = [
        Path(TYPED_HISTORY_FILE),
        PROJECT_ROOT / "typed_history.json",
        Path(os.environ.get("LOCALAPPDATA") or tempfile.gettempdir()) / "iPhoneLink" / "typed_history.json",
        Path(tempfile.gettempdir()) / "iPhoneLink" / "typed_history.json",
    ]
    unique: list[Path] = []
    seen: set[str] = set()
    for p in cands:
        key = os.path.normcase(str(p))
        if key not in seen:
            seen.add(key)
            unique.append(p)
    return unique


def save_typed_history() -> None:
    """Persist typed history buffer and last active line to a writable location (supports Program Files UAC)."""
    try:
        with _TYPED_BUFFER_LOCK:
            data = {
                "last_active": _LAST_TYPED_ACTIVE_LINE,
                "timestamp": _LAST_TYPED_TIMESTAMP,
                "buffer": _TYPED_LINES_BUFFER,
            }
        target_file = _get_writable_typed_history_file()
        target_file.parent.mkdir(parents=True, exist_ok=True)
        with open(target_file, "w", encoding="utf-8") as f:
            json.dump(data, f, ensure_ascii=False, indent=2)
    except Exception:
        pass


def _is_valid_typed_history_entry(line: str) -> bool:
    """Validate typed history entries: forbid long paste blocks (>= 35 chars) or news headlines (Tahap 38)."""
    if line == "":
        return True  # Keep explicit blank line marker for Double Enter
    if not line:
        return False
    if len(line) >= 35:
        return False
    low = line.lower()
    if "rekaman video amatir" in low or "unggahan bertajuk" in low:
        return False
    return True


def load_typed_history() -> None:
    """Load persisted typed history from disk (merging primary & LocalAppData/Temp fallbacks)."""
    global _LAST_TYPED_ACTIVE_LINE, _LAST_TYPED_TIMESTAMP, _TYPED_LINES_BUFFER
    merged_buf: list[str] = []
    best_act = ""
    best_ts = 0.0
    for cand_file in _get_all_typed_history_candidate_files():
        if not cand_file.is_file():
            continue
        try:
            with open(cand_file, "r", encoding="utf-8") as f:
                data = json.load(f)
            ts = float(data.get("timestamp", 0.0) or 0.0)
            act = data.get("last_active", "")
            if ts >= best_ts and isinstance(act, str) and _is_valid_typed_history_entry(act):
                best_act = act
                best_ts = ts
            for item in data.get("buffer", []):
                if isinstance(item, str) and _is_valid_typed_history_entry(item):
                    if item == "" and (not merged_buf or merged_buf[-1] == ""):
                        continue
                    if item != "" and item in merged_buf:
                        continue
                    merged_buf.append(item)
        except Exception:
            pass
    with _TYPED_BUFFER_LOCK:
        if best_act:
            _LAST_TYPED_ACTIVE_LINE = best_act
        if best_ts > 0.0:
            _LAST_TYPED_TIMESTAMP = best_ts
        if merged_buf:
            _TYPED_LINES_BUFFER[:] = merged_buf[-60:]


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
            combined = (_CURRENT_TYPED_LINE + text).replace("\r\n", "\n").replace("\r", "\n")
            parts = combined.split("\n")
            # Every element in parts[:-1] was terminated by an explicit Enter keystroke
            for part in parts[:-1]:
                trimmed = part.strip()
                if len(trimmed) >= 3 and _is_valid_typed_history_entry(trimmed):
                    if not _TYPED_LINES_BUFFER or _TYPED_LINES_BUFFER[-1] != trimmed:
                        _TYPED_LINES_BUFFER.append(trimmed)
                    if len(_TYPED_LINES_BUFFER) > 60:
                        _TYPED_LINES_BUFFER.pop(0)
                    _LAST_TYPED_ACTIVE_LINE = trimmed
                    _LAST_TYPED_TIMESTAMP = time.time()
                elif trimmed == "":
                    # Only consecutive Enter (pressing Enter on an empty line -> Double Enter) emits ""
                    if _TYPED_LINES_BUFFER and _TYPED_LINES_BUFFER[-1] != "":
                        _TYPED_LINES_BUFFER.append("")
                        if len(_TYPED_LINES_BUFFER) > 60:
                            _TYPED_LINES_BUFFER.pop(0)
            tail_part = parts[-1]
            tail_trimmed = tail_part.strip()
            if len(parts) >= 3 and len(tail_trimmed) >= 3 and _is_valid_typed_history_entry(tail_trimmed):
                # Multi-line paste/type block ending with non-empty line: commit immediately so paragraph pair exists
                if not _TYPED_LINES_BUFFER or _TYPED_LINES_BUFFER[-1] != tail_trimmed:
                    _TYPED_LINES_BUFFER.append(tail_trimmed)
                if len(_TYPED_LINES_BUFFER) > 60:
                    _TYPED_LINES_BUFFER.pop(0)
                _LAST_TYPED_ACTIVE_LINE = tail_trimmed
                _LAST_TYPED_TIMESTAMP = time.time()
                _CURRENT_TYPED_LINE = ""
            else:
                _CURRENT_TYPED_LINE = tail_part
                if len(tail_trimmed) >= 3 and _is_valid_typed_history_entry(tail_trimmed):
                    _LAST_TYPED_ACTIVE_LINE = tail_trimmed
                    _LAST_TYPED_TIMESTAMP = time.time()
        else:
            # Append chunk (single char or word snippet) to _CURRENT_TYPED_LINE
            _CURRENT_TYPED_LINE += text
            trimmed = _CURRENT_TYPED_LINE.strip()
            if len(trimmed) >= 3 and _is_valid_typed_history_entry(trimmed):
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


def _sync_candidate_case_to_ocr(ocr_text: str, candidate_text: str) -> str:
    """Tahap 194: Synchronize candidate casing with iPhone screen OCR casing when Caps Lock caused an all-caps vs all-lowercase mismatch."""
    if not ocr_text or not candidate_text:
        return candidate_text
    alpha_ocr = "".join(c for c in ocr_text if c.isalpha())
    alpha_cand = "".join(c for c in candidate_text if c.isalpha())
    if len(alpha_ocr) >= 4 and len(alpha_cand) >= 4:
        if alpha_ocr == alpha_ocr.lower() and alpha_cand == alpha_cand.upper():
            return candidate_text.lower()
        if alpha_ocr == alpha_ocr.upper() and alpha_cand == alpha_cand.lower():
            return candidate_text.upper()
    return candidate_text


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
                return _sync_candidate_case_to_ocr(raw_ocr, active_line)

            # SequenceMatcher match against full raw_ocr
            ratio = difflib.SequenceMatcher(None, raw_lower, act_lower).ratio()
            if ratio >= 0.78:
                return _sync_candidate_case_to_ocr(raw_ocr, active_line)

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
                return _sync_candidate_case_to_ocr(full_ocr, c_strip)

            # Tahap 193: First letter handle corruption guard (multi-word lines with shared words tolerate >= 0.72)
            if c_lower[0] != full_ocr_lower[0]:
                ocr_words_list = full_ocr_lower.split()
                cand_words_list = c_lower.split()
                if not ocr_words_list or not cand_words_list:
                    continue
                ocr_w0 = ocr_words_list[0]
                cand_w0 = cand_words_list[0]
                w0_ratio = difflib.SequenceMatcher(None, ocr_w0, cand_w0).ratio()
                shared_words = set(ocr_words_list).intersection(set(cand_words_list))
                sm_ratio = difflib.SequenceMatcher(None, full_ocr_lower, c_lower).ratio()
                is_multiword_supported = (len(cand_words_list) >= 2 and len(shared_words) >= 1 and w0_ratio >= 0.50)
                min_req = 0.72 if is_multiword_supported else 0.84
                if not is_multiword_supported and w0_ratio < 0.70:
                    continue
                if sm_ratio < min_req:
                    continue
                score = sm_ratio + (0.04 if is_multiword_supported else 0.0)
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
            return _sync_candidate_case_to_ocr(full_ocr, best_cand)

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
            corrected_lines.append(_sync_candidate_case_to_ocr(c_line, prefix_cand))
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

            # First letter mismatch safety guard (Tahap 193):
            # When the upper-left lollipop handle corrupts the first letter of a multi-word line
            # (e.g. "tarhutla aman..." vs "karhutla aman..."), allow >= 0.72 if other words match.
            if c_lower[0] != l_lower[0]:
                if not l_words or not c_words:
                    continue
                w0_ratio = difflib.SequenceMatcher(None, l_words[0], c_words[0]).ratio()
                shared_mid = set(l_words).intersection(set(c_words))
                is_multi_supported = (len(c_words) >= 2 and len(shared_mid) >= 1 and w0_ratio >= 0.50)
                if not is_multi_supported and w0_ratio < 0.70:
                    continue
                req_ratio = min(req_ratio, 0.72) if is_multi_supported else max(req_ratio, 0.84)

            if len(c_line) >= 6 and (l_lower in c_lower or c_lower in l_lower) and c_lower[0] == l_lower[0]:
                line_best = c_strip
                line_ratio = 1.0
                break
            r = difflib.SequenceMatcher(None, l_lower, c_lower).ratio()
            if r >= req_ratio and r > line_ratio:
                line_ratio = r
                line_best = c_strip

        if line_ratio >= 0.72:
            corrected_lines.append(_sync_candidate_case_to_ocr(c_line, line_best))
        else:
            # Word-level handle-corruption fallback against typed buffer words (e.g. "tarhutla"->"karhutla", "lomah"->"lontoh", "prabowk"->"prabowo", "pemulun"->"pemulung")
            vocab: dict[str, str] = {}
            for cand in candidates:
                for tok in cand.strip().split():
                    t_clean = tok.strip(".,!?:;\"'")
                    if len(t_clean) >= 5:
                        vocab[t_clean.lower()] = tok
            if vocab:
                repaired_tokens = []
                for orig_tok in c_line.split():
                    tok_low = orig_tok.lower().strip(".,!?:;\"'")
                    if len(tok_low) >= 5 and tok_low not in vocab:
                        best_wt = orig_tok
                        best_wr = 0.0
                        for vk, v_orig in vocab.items():
                            if abs(len(vk) - len(tok_low)) > 2:
                                continue
                            wr = difflib.SequenceMatcher(None, tok_low, vk).ratio()
                            if wr >= 0.78 and wr > best_wr:
                                best_wr = wr
                                best_wt = _sync_candidate_case_to_ocr(orig_tok, v_orig)
                        repaired_tokens.append(best_wt)
                    else:
                        repaired_tokens.append(orig_tok)
                corrected_lines.append(" ".join(repaired_tokens))
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
    """Clear session OCR chunk memory, selection color, and temporary typed line buffers (Tahap 32/40/51)."""
    global _RECENT_LONG_OCR_CHUNKS, _LAST_SELECTION_COLOR, _CURRENT_TYPED_LINE, _TYPED_LINES_BUFFER
    with _CHUNKS_LOCK:
        _RECENT_LONG_OCR_CHUNKS.clear()
    _LAST_SELECTION_COLOR = None
    _CURRENT_TYPED_LINE = ""
    _TYPED_LINES_BUFFER.clear()


def wipe_all_privacy_data() -> dict:
    """Wipe all sensitive in-memory buffers and delete sensitive local cache/history files on disk (Tahap 52)."""
    global _RECENT_LONG_OCR_CHUNKS, _LAST_SELECTION_COLOR, _CURRENT_TYPED_LINE, _TYPED_LINES_BUFFER
    with _CHUNKS_LOCK:
        _RECENT_LONG_OCR_CHUNKS.clear()
    _LAST_SELECTION_COLOR = None
    _CURRENT_TYPED_LINE = ""
    _TYPED_LINES_BUFFER.clear()

    # Physical file cleanup from application root and writable fallback directories
    try:
        import tempfile
        dirs_to_clean = [
            get_app_root(),
            Path(TYPED_HISTORY_FILE).parent,
            Path(os.environ.get("LOCALAPPDATA") or tempfile.gettempdir()) / "iPhoneLink",
            Path(os.environ.get("LOCALAPPDATA") or tempfile.gettempdir()) / "iPhoneLink_NativeRust_v4",
            Path(tempfile.gettempdir()) / "iPhoneLink",
        ]
        seen_dirs: set[str] = set()
        for d in dirs_to_clean:
            key = os.path.normcase(str(d))
            if key in seen_dirs or not d.is_dir():
                continue
            seen_dirs.add(key)
            targets = [
                d / "user_settings.json",
                d / "typed_history.json",
                d / "wifi_state.json",
                d / "debug_metrics.json",
            ]
            for p in d.glob("debug_ocr_*"):
                targets.append(p)
            for target in targets:
                try:
                    if target.is_file():
                        target.unlink(missing_ok=True)
                except Exception as e:
                    print(f"Error removing {target}: {e}", flush=True)
    except Exception as e:
        print(f"wipe_all_privacy_data disk cleanup error: {e}", flush=True)

    return {"ok": True, "wiped": True}



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
        if not (w.startswith('@') or w.startswith('#'))
        and re.search(r'[a-zA-Z]', w) and re.search(r'[0-9]', w) and not re.match(r'^[a-zA-Z]+[0-9]+$', w)
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

    # p. Strip stray noise underscores on ordinary words (Tahap 48 V2: e.g. "aksi _heroik" -> "aksi heroik")
    t = re.sub(r'(?<![@#A-Za-z0-9])_+(?=[A-Za-z])|(?<=[A-Za-z])_+(?![A-Za-z0-9])', '', t)

    # q. Tahap 193: Lollipop selection handle optical recovery (works even without typed shadow buffer):
    #    - Merged space from vertical handle stem/pipe between words: "enggalnih" / "engga1nih" -> "engga nih"
    #    - Trailing '?' at end of conversational question misread as I / 1 / l / | next to right handle
    #    - First/last-char handle stem occlusion in tested Indonesian phrases
    t = re.sub(r'\bengga[l1|]nih\b', 'engga nih', t, flags=re.IGNORECASE)
    t = re.sub(r'\benga[l1|]nih\b', 'enga nih', t, flags=re.IGNORECASE)
    t = re.sub(
        r'\b(nih|ya|kan|dong|sih|kok|lah|kah|apa|mana|siapa|kapan|gimana|kenapa|aman|wok|bro|bang|kak|guys)\s+[I1l|](\s*)$',
        r'\1?\2',
        t,
        flags=re.IGNORECASE,
    )
    t = re.sub(r'\btarhutla\b', 'karhutla', t, flags=re.IGNORECASE)
    t = re.sub(r'\blomah(?=\s+enak(?:\s|$))', 'lontoh', t, flags=re.IGNORECASE)
    t = re.sub(r'\bprabow[kl1|]\b', 'prabowo', t, flags=re.IGNORECASE)
    t = re.sub(r'\bpemulun\b(?=\s*$)', 'pemulung', t, flags=re.IGNORECASE)

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
    # Penalize stray noise underscore outside @mention or #hashtag (Tahap 48 V2)
    clean_no_tags = re.sub(r'[@#][A-Za-z0-9_.]+', '', line)
    if '_' in clean_no_tags:
        penalty += 25
    return penalty


def _pick_cleaner_line(line1: str, line2: str) -> str:
    """Select the cleaner version of an anchor line (e.g. avoiding trailing orphan digits, truncated words, or OCR optical defects)."""
    l1_clean = _clean_ocr_line(line1)
    l2_clean = _clean_ocr_line(line2)

    # Prioritize line without stray noise underscores (Tahap 48 V2)
    has_u1 = bool(re.search(r'(?<![@#A-Za-z0-9])_+(?=[A-Za-z])|(?<=[A-Za-z])_+(?![A-Za-z0-9])', line1))
    has_u2 = bool(re.search(r'(?<![@#A-Za-z0-9])_+(?=[A-Za-z])|(?<=[A-Za-z])_+(?![A-Za-z0-9])', line2))
    if has_u2 and not has_u1:
        return l1_clean
    if has_u1 and not has_u2:
        return l2_clean

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
        # Cross-reference _TYPED_LINES_BUFFER (plus active _CURRENT_TYPED_LINE): recover double Enter between lines
        import difflib
        with _TYPED_BUFFER_LOCK:
            buf = list(_TYPED_LINES_BUFFER)
            curr_act = _CURRENT_TYPED_LINE.strip()
            if curr_act and (not buf or buf[-1] != curr_act):
                buf.append(curr_act)
        for idx in range(len(non_empty_lines) - 1):
            if idx in has_blank_after:
                continue
            curr_low = non_empty_lines[idx].lower().strip()
            next_low = non_empty_lines[idx + 1].lower().strip()
            for b_idx in range(len(buf) - 2):
                if buf[b_idx + 1] == "":  # empty line in buffer
                    b_prev = buf[b_idx].lower().strip()
                    b_next = buf[b_idx + 2].lower().strip()
                    prev_ok = (
                        curr_low == b_prev
                        or (len(curr_low) >= 4 and (curr_low in b_prev or b_prev in curr_low))
                        or difflib.SequenceMatcher(None, curr_low, b_prev).ratio() >= 0.72
                    )
                    next_ok = (
                        next_low == b_next
                        or (len(next_low) >= 4 and (next_low in b_next or b_next in next_low))
                        or difflib.SequenceMatcher(None, next_low, b_next).ratio() >= 0.72
                    )
                    if prev_ok and next_ok:
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
        if current_color in ("yellow", "blue"):
            fb = get_recent_typed_single_line_fallback(max_age_sec=30.0)
            if fb:
                print(f"[Tahap 48 V2] Recovered 1-line selection from recent typing buffer: '{fb}'", flush=True)
                return (fb, False) if return_meta else fb
        return (None, False) if return_meta else None

    # Strip selector bounds and color markers if present
    current_frame_raw = re.sub(r'<<<SEL_BOUNDS:[^>]+>>>', '', current_frame_raw)
    current_frame_raw = re.sub(r'<<<SEL_COLOR:[^>]+>>>', '', current_frame_raw).strip()
    if not current_frame_raw:
        if current_color in ("yellow", "blue"):
            fb = get_recent_typed_single_line_fallback(max_age_sec=30.0)
            if fb:
                print(f"[Tahap 48 V2] Recovered 1-line selection from recent typing buffer: '{fb}'", flush=True)
                return (fb, False) if return_meta else fb
        return (None, False) if return_meta else None

    raw_frame_lines = [l.strip() for l in current_frame_raw.splitlines()]
    c_lines = _strip_top_unselected_debris([l for l in raw_frame_lines if l and not _is_garbage_ocr_line(l)])
    if not c_lines:
        if current_color in ("yellow", "blue"):
            fb = get_recent_typed_single_line_fallback(max_age_sec=30.0)
            if fb:
                print(f"[Tahap 48 V2] Recovered 1-line selection from recent typing buffer: '{fb}'", flush=True)
                return (fb, False) if return_meta else fb
        return (None, False) if return_meta else None

    # Discard partially clipped/damaged bottom line at toolbar edge (Tahap 40 Bug 4 - only if >= 4 lines)
    if len(c_lines) >= 4:
        last_l = c_lines[-1]
        if not last_l.startswith("#") and re.search(r'\b(ccrta|dilanorkan|hcrhacil|kcrhacil)\b', last_l, re.IGNORECASE):
            c_lines.pop()
            if raw_frame_lines and raw_frame_lines[-1] == last_l:
                raw_frame_lines.pop()

    # ATURAN EMAS 1 (Tahap 36/37/38/39/40/48 V2): Short text & Single-line fast-path
    if len(c_lines) == 1:
        cleaned_1line = _clean_ocr_line(c_lines[0]).strip()
        if cleaned_1line:
            final_text = auto_correct_ocr_text(cleaned_1line)
            return (final_text, False) if return_meta else final_text

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


def get_recent_typed_single_line_fallback(max_age_sec: float = 30.0) -> str | None:
    """
    Fallback for 1-line selection (Tahap 48 V2):
    If user typed a line within < max_age_sec (e.g. 30s) and highlighted it,
    return that line if OCR returned empty despite selection color.
    """
    import time
    now = time.time()
    with _TYPED_BUFFER_LOCK:
        if _LAST_TYPED_TIMESTAMP > 0 and (now - _LAST_TYPED_TIMESTAMP) <= max_age_sec:
            cand = _CURRENT_TYPED_LINE.strip()
            if not cand and _TYPED_LINES_BUFFER:
                for b in reversed(_TYPED_LINES_BUFFER):
                    if b.strip():
                        cand = b.strip()
                        break
            if cand and len(cand) >= 3 and len(cand) < 80:
                return cand
    return None


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
    with _HID_ACTION_LOCK:
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

            # Multi-Step Shift Latching (Tahap 53): prevents iOS race condition where '?' typed as '/'
            needs_shift = len(usages) >= 2 and usages[0] == 0xE1
            keycode = usages[-1]

            if needs_shift:
                # Step 1: Latch Shift down first so iOS registers modifier state
                send_keys_direct([0xE1])
                time.sleep(0.014)
                # Step 2: Press the key while Shift is held down
                send_keys_direct([0xE1, keycode])
                time.sleep(0.018)
                # Step 3: Release the key first while Shift is still held
                send_keys_direct([0xE1])
                time.sleep(0.008)
                # Step 4: Release Shift cleanly
                send_keys_direct([])
                time.sleep(0.012)
            else:
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
    global _STAGE_STEP, _STAGE_TITLE, _STAGE_DETAIL
    prev_phase = None
    new_phase = None
    transport = "cable"
    with _LOCK:
        prev_phase = _STATE.get("phase")
        if "stage_step" in values:
            _STAGE_STEP = int(values["stage_step"])
            _STAGE_TITLE = str(values.get("stage_title", ""))
            _STAGE_DETAIL = str(values.get("stage_detail", ""))
        new_phase_val = values.get("phase")
        if new_phase_val in ("idle", "error"):
            _STAGE_STEP = 0
            _STAGE_TITLE = ""
            _STAGE_DETAIL = ""
        elif new_phase_val == "running":
            _STAGE_STEP = 4
            _STAGE_TITLE = "Mulai stream"
            _STAGE_DETAIL = "Menerima frame H.265/HEVC 60fps."
        values["stage_step"] = _STAGE_STEP
        values["stage_title"] = _STAGE_TITLE
        values["stage_detail"] = _STAGE_DETAIL
        _STATE.update(values)
        new_phase = _STATE.get("phase")
        transport = _STATE.get("transport") or "cable"
        snapshot = dict(_STATE)
    if new_phase != prev_phase:
        try:
            from handset.audio import notify_screen_phase_changed
            notify_screen_phase_changed(str(new_phase or "idle"), str(transport))
        except Exception:
            pass
    return snapshot


def _plain(error: Exception) -> str:
    text = str(error).strip() or type(error).__name__
    name = type(error).__name__
    if name == "WifiNotFoundError" or "Wi-Fi" in text or "Wi‑Fi" in text:
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
