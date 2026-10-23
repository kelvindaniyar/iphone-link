"""Local device window. Binds to loopback only."""

from __future__ import annotations

import http.client
import json
import time
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import unquote, urlparse

from handset.device import read_handset, snapshot
from handset.mux import MuxError
from handset.screen import (
    restart_screen,
    screen_status,
    send_button_direct,
    send_keys_direct,
    start_screen,
    stop_screen,
    toggle_onscreen_keyboard,
    trigger_ios_shortcut,
    type_text_to_iphone,
)
from handset.wifi import pair_for_wifi, wifi_paired_udids
from handset.audio import audio_manager

ROOT = Path(__file__).resolve().parent / "web"
SETTINGS_FILE = ROOT.parent.parent / "user_settings.json"
HOST = "127.0.0.1"
PORT = 8765
MIRROR_PREFIX = "/mirror"
STREAM_HOST = "127.0.0.1"
STREAM_PORT = 8766
_HOP_BY_HOP = frozenset(
    {"connection", "keep-alive", "proxy-authenticate", "proxy-authorization", "te", "trailers", "upgrade"}
)
_CLIENT_GONE = (BrokenPipeError, ConnectionAbortedError, ConnectionResetError)

DEFAULT_SETTINGS = {
    "crop": {"top": 0, "bottom": 0, "left": 0, "right": 0},
    "fitMode": "fill",
    "theme": "dark",
    "audioSinkEnabled": False,
    "audioVolume": 100,
    "trackpadSpeed": 100,
    "crossPlatformShortcuts": True,
    "pastePcClipboard": True,
}

def get_windows_clipboard_seq() -> int:
    """Return the Windows clipboard sequence number (increments on every copy/cut)."""
    try:
        import ctypes
        return int(ctypes.windll.user32.GetClipboardSequenceNumber())
    except Exception:
        return 0


_ACTIVE_CLIPBOARD_SOURCE = "pc"  # "pc" or "iphone"
_LAST_WIN_CLIPBOARD_SEQ = get_windows_clipboard_seq()


def get_windows_clipboard_text() -> str | None:
    """Read plain/unicode text from the Windows PC clipboard."""
    try:
        import ctypes
        user32 = ctypes.windll.user32
        kernel32 = ctypes.windll.kernel32
        if not user32.OpenClipboard(0):
            return None
        try:
            CF_UNICODETEXT = 13
            h_clip = user32.GetClipboardData(CF_UNICODETEXT)
            if not h_clip:
                return None
            p_text = kernel32.GlobalLock(h_clip)
            if not p_text:
                return None
            try:
                return ctypes.c_wchar_p(p_text).value
            finally:
                kernel32.GlobalUnlock(h_clip)
        finally:
            user32.CloseClipboard()
    except Exception as e:
        print(f"get_windows_clipboard_text error: {e}", flush=True)
        return None


def set_windows_clipboard_text(text: str) -> bool:
    """Write unicode text to the Windows PC clipboard safely in 64-bit."""
    if not text:
        return False
    try:
        import ctypes
        from ctypes import wintypes

        user32 = ctypes.windll.user32
        kernel32 = ctypes.windll.kernel32

        user32.OpenClipboard.argtypes = [wintypes.HWND]
        user32.OpenClipboard.restype = wintypes.BOOL
        user32.EmptyClipboard.argtypes = []
        user32.EmptyClipboard.restype = wintypes.BOOL
        user32.SetClipboardData.argtypes = [wintypes.UINT, wintypes.HANDLE]
        user32.SetClipboardData.restype = wintypes.HANDLE
        user32.CloseClipboard.argtypes = []
        user32.CloseClipboard.restype = wintypes.BOOL

        kernel32.GlobalAlloc.argtypes = [wintypes.UINT, ctypes.c_size_t]
        kernel32.GlobalAlloc.restype = wintypes.HGLOBAL
        kernel32.GlobalLock.argtypes = [wintypes.HGLOBAL]
        kernel32.GlobalLock.restype = wintypes.LPVOID
        kernel32.GlobalUnlock.argtypes = [wintypes.HGLOBAL]
        kernel32.GlobalUnlock.restype = wintypes.BOOL
        kernel32.RtlMoveMemory.argtypes = [wintypes.LPVOID, wintypes.LPCVOID, ctypes.c_size_t]
        kernel32.RtlMoveMemory.restype = None

        if not user32.OpenClipboard(None):
            return False
        try:
            if not user32.EmptyClipboard():
                return False

            encoded = text.encode("utf-16le") + b"\x00\x00"
            GMEM_MOVEABLE = 0x0002
            CF_UNICODETEXT = 13

            h_mem = kernel32.GlobalAlloc(GMEM_MOVEABLE, len(encoded))
            if not h_mem:
                return False

            p_mem = kernel32.GlobalLock(h_mem)
            if not p_mem:
                return False

            try:
                kernel32.RtlMoveMemory(p_mem, encoded, len(encoded))
            finally:
                kernel32.GlobalUnlock(h_mem)

            if not user32.SetClipboardData(CF_UNICODETEXT, h_mem):
                return False
            return True
        finally:
            user32.CloseClipboard()
    except Exception as e:
        print(f"set_windows_clipboard_text error: {e}", flush=True)
        return False


def extract_text_from_frame(frame_data_url: str) -> str | None:
    """Extract highlighted/selected text from iPhone screen frame using Windows 11 WinRT OCR."""
    if not frame_data_url or "," not in frame_data_url:
        return None
    try:
        import base64, tempfile, subprocess, os
        header, b64data = frame_data_url.split(",", 1)
        raw_bytes = base64.b64decode(b64data)
        if len(raw_bytes) < 100:
            return None

        with tempfile.NamedTemporaryFile(suffix=".jpg", delete=False) as f:
            tmp_path = f.name
            f.write(raw_bytes)

        script_path = str(Path(__file__).resolve().parent / "ocr_extractor.ps1")
        try:
            res = subprocess.run(
                ["powershell.exe", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", script_path, tmp_path],
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                timeout=4.0,
            )
            text = (res.stdout or "").strip()
            return text if text else None
        finally:
            try:
                os.remove(tmp_path)
            except Exception:
                pass
    except Exception as e:
        print(f"extract_text_from_frame error: {e}", flush=True)
        return None


def paste_to_iphone(text: str | None = None, force_pc: bool = False, paste_pc_client_setting: bool | None = None) -> dict:
    """Smart paste: Direct HID Typing Injection for Windows PC text or native Cmd+V for internal iOS copy."""
    global _ACTIVE_CLIPBOARD_SOURCE, _LAST_WIN_CLIPBOARD_SEQ
    settings = load_user_settings()
    paste_pc_enabled = settings.get("pastePcClipboard", True) if paste_pc_client_setting is None else bool(paste_pc_client_setting)

    # Rule 1: Explicit PC paste request (e.g. from "Paste PC Text Now" button)
    if force_pc:
        pc_text = text if text is not None else get_windows_clipboard_text()
        if pc_text:
            _ACTIVE_CLIPBOARD_SOURCE = "pc"
            _LAST_WIN_CLIPBOARD_SEQ = get_windows_clipboard_seq()
            typed = type_text_to_iphone(pc_text)
            return {"ok": True, "method": "typed", "length": typed}
        return {"ok": False, "error": "Clipboard is empty"}

    # Rule 2: If toggle "Ctrl+V Pastes PC Clipboard" is OFF -> MUST execute native iOS Cmd+V and NEVER type PC text
    if not paste_pc_enabled:
        ok = trigger_ios_shortcut("paste-ios")
        return {"ok": ok, "method": "ios-shortcut", "reason": "toggle-off"}

    # Rule 3: Check sequence number to see if user copied something NEW in Windows
    current_seq = get_windows_clipboard_seq()
    if current_seq != _LAST_WIN_CLIPBOARD_SEQ:
        _ACTIVE_CLIPBOARD_SOURCE = "pc"
        _LAST_WIN_CLIPBOARD_SEQ = current_seq

    # Rule 4: If active source is iPhone (user just copied/cut in iPhone and did not copy new PC text)
    if _ACTIVE_CLIPBOARD_SOURCE == "iphone":
        ok = trigger_ios_shortcut("paste-ios")
        return {"ok": ok, "method": "ios-shortcut", "reason": "iphone-source"}

    # Rule 5: Active source is PC -> type PC clipboard text
    pc_text = text if text is not None else get_windows_clipboard_text()
    if pc_text:
        typed = type_text_to_iphone(pc_text)
        return {"ok": True, "method": "typed", "length": typed}

    # Fallback to iOS paste if PC clipboard is empty
    ok = trigger_ios_shortcut("paste-ios")
    return {"ok": ok, "method": "ios-shortcut", "reason": "pc-empty-fallback"}


def load_user_settings() -> dict:
    if SETTINGS_FILE.is_file():
        try:
            data = json.loads(SETTINGS_FILE.read_text(encoding="utf-8"))
            if isinstance(data, dict):
                merged = {**DEFAULT_SETTINGS, **data}
                if isinstance(data.get("crop"), dict):
                    merged["crop"] = {**DEFAULT_SETTINGS["crop"], **data["crop"]}
                return merged
        except Exception as e:
            print(f"load_user_settings error: {e}", flush=True)
    return dict(DEFAULT_SETTINGS)


def save_user_settings(new_settings: dict) -> dict:
    current = load_user_settings()
    if not isinstance(new_settings, dict):
        return current
    for k, v in new_settings.items():
        if k == "crop" and isinstance(v, dict):
            current["crop"] = {
                "top": max(0, min(120, int(v.get("top", 0) or 0))),
                "bottom": max(0, min(120, int(v.get("bottom", 0) or 0))),
                "left": max(0, min(120, int(v.get("left", 0) or 0))),
                "right": max(0, min(120, int(v.get("right", 0) or 0))),
            }
        elif k == "trackpadSpeed":
            try:
                current["trackpadSpeed"] = max(30, min(200, int(v or 100)))
            except Exception:
                current["trackpadSpeed"] = 100
        elif k in ("crossPlatformShortcuts", "pastePcClipboard"):
            current[k] = bool(v)
        elif k in current:
            current[k] = v
    try:
        SETTINGS_FILE.write_text(json.dumps(current, indent=2), encoding="utf-8")
    except Exception as e:
        print(f"save_user_settings error: {e}", flush=True)
    return current


class Handler(BaseHTTPRequestHandler):
    def end_headers(self) -> None:
        self.send_header("Cache-Control", "no-store, no-cache, must-revalidate, max-age=0")
        self.send_header("Pragma", "no-cache")
        self.send_header("Expires", "0")
        super().end_headers()

    def do_GET(self) -> None:
        path = urlparse(self.path).path
        if path == MIRROR_PREFIX or path.startswith(f"{MIRROR_PREFIX}/"):
            self._proxy_mirror("GET")
            return
        if path == "/api/status":
            payload = snapshot()
            payload["screen"] = screen_status()
            payload["wifi"] = {"paired": wifi_paired_udids()}
            payload["audio"] = audio_manager.status()
            self._json(payload)
            return
        if path == "/api/settings":
            self._json(load_user_settings())
            return
        if path in ("/api/audio", "/api/audio/status"):
            self._json(audio_manager.status())
            return
        if path == "/api/screen":
            self._json(screen_status())
            return
        if path in ("/api/screen/stop", "/mirror/stop"):
            self._json(stop_screen())
            return
        if path == "/debug-metrics":
            log_path = ROOT.parent.parent / "debug_metrics.json"
            if log_path.is_file():
                try:
                    self._json(json.loads(log_path.read_text(encoding="utf-8")))
                    return
                except Exception:
                    pass
            self._json({"history": []})
            return
        if path.startswith("/api/devices/"):
            udid = unquote(path.removeprefix("/api/devices/")).strip("/")
            if not udid or "/" in udid:
                self._json({"error": "Missing device id."}, status=400)
                return
            self._device(udid)
            return
        self._file(path)

    def do_POST(self) -> None:
        path = urlparse(self.path).path
        if path == "/debug-metrics":
            length = int(self.headers.get("Content-Length") or 0)
            raw = self.rfile.read(length) if length else b"{}"
            try:
                payload = json.loads(raw.decode("utf-8") or "{}")
            except Exception:
                payload = {}
            self._record_debug_metrics(payload)
            self._json({"ok": True})
            return
        if path == MIRROR_PREFIX or path.startswith(f"{MIRROR_PREFIX}/"):
            self._proxy_mirror("POST")
            return
        length = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(length) if length else b"{}"
        try:
            payload = json.loads(raw.decode("utf-8") or "{}")
        except json.JSONDecodeError:
            payload = {}
        if not isinstance(payload, dict):
            payload = {}

        if path == "/api/settings":
            saved = save_user_settings(payload)
            self._json({"ok": True, "settings": saved})
            return
        if path == "/api/audio/toggle":
            enabled = payload.get("enabled") if "enabled" in payload else None
            st = audio_manager.toggle(enabled)
            save_user_settings({"audioSinkEnabled": st.get("enabled", False)})
            self._json(st)
            return
        if path == "/api/audio/volume":
            vol = payload.get("volume", 100)
            st = audio_manager.set_volume(vol)
            save_user_settings({"audioVolume": st.get("volume", 100)})
            self._json(st)
            return
        if path == "/api/audio/pair":
            self._json(audio_manager.open_bluetooth_settings())
            return
        if path == "/api/keyboard/shortcut":
            action = str(payload.get("action", ""))
            frame_data_url = payload.get("frameDataUrl")
            wait_result = bool(payload.get("waitResult", False))

            # Trigger native iOS hotkey
            ok = trigger_ios_shortcut(action)

            global _ACTIVE_CLIPBOARD_SOURCE, _LAST_WIN_CLIPBOARD_SEQ
            if action in ("copy", "cut"):
                _ACTIVE_CLIPBOARD_SOURCE = "iphone"
                _LAST_WIN_CLIPBOARD_SEQ = get_windows_clipboard_seq()

                extracted_text = None

                def _do_extract(frame_url: str):
                    global _ACTIVE_CLIPBOARD_SOURCE, _LAST_WIN_CLIPBOARD_SEQ
                    try:
                        text = extract_text_from_frame(frame_url)
                        if text:
                            if set_windows_clipboard_text(text):
                                _LAST_WIN_CLIPBOARD_SEQ = get_windows_clipboard_seq()
                                _ACTIVE_CLIPBOARD_SOURCE = "iphone"
                                print(f"Copied iPhone selection to Windows PC clipboard ({len(text)} chars)", flush=True)
                        return text
                    except Exception as err:
                        print(f"Async extract error: {err}", flush=True)
                        return None

                if frame_data_url:
                    if wait_result:
                        extracted_text = _do_extract(frame_data_url)
                    else:
                        import threading
                        threading.Thread(target=_do_extract, args=(frame_data_url,), daemon=True).start()

                self._json({
                    "ok": ok,
                    "action": action,
                    "text": extracted_text,
                    "length": len(extracted_text) if extracted_text else 0,
                })
                return

            self._json({"ok": ok, "action": action})
            return
        if path == "/api/keyboard/paste":
            self._json(
                paste_to_iphone(
                    payload.get("text"),
                    force_pc=bool(payload.get("force", False)),
                    paste_pc_client_setting=payload.get("pastePcClipboard"),
                )
            )
            return
        if path == "/api/keyboard/toggle-onscreen":
            ok = toggle_onscreen_keyboard()
            self._json({"ok": ok})
            return

        udid = payload.get("udid") if isinstance(payload.get("udid"), str) else None
        if path == "/api/screen/restart":
            self._json(restart_screen(udid))
            return
        if path in ("/api/screen/stop", "/mirror/stop"):
            self._json(stop_screen())
            return
        if path == "/api/wifi/pair":
            if not udid:
                self._json({"ok": False, "error": "Plug in the iPhone first."}, status=400)
                return
            self._json(pair_for_wifi(udid))
            return
        if path != "/api/screen":
            self._json({"error": "Unknown action."}, status=404)
            return
        transport = payload.get("transport")
        if transport not in {"auto", "usb", "wifi"}:
            transport = "auto"
        self._json(start_screen(udid, transport))

    def log_message(self, fmt: str, *args) -> None:
        return

    def _record_debug_metrics(self, payload: dict) -> None:
        payload["received_at"] = time.strftime("%Y-%m-%d %H:%M:%S")
        source = payload.get("source", "unknown")
        print(f"\n==================== [DEBUG-METRICS: {source}] ====================", flush=True)
        print(json.dumps(payload, indent=2), flush=True)
        print("==================================================================\n", flush=True)
        
        log_path = ROOT.parent.parent / "debug_metrics.json"
        data = {}
        if log_path.is_file():
            try:
                data = json.loads(log_path.read_text(encoding="utf-8"))
            except Exception:
                data = {}
        if not isinstance(data, dict):
            data = {}
        
        data[f"latest_{source}"] = payload
        if source == "mirror.js" and "frameCount" in payload:
            data["latest_mirror_frame"] = payload
        elif source == "mirror.js" and "event" in payload:
            data[f"latest_mirror_{payload['event']}"] = payload
        data["last_updated"] = payload["received_at"]
        history = data.get("history", [])
        if not isinstance(history, list):
            history = []
        history.append(payload)
        if len(history) > 60:
            history = history[-60:]
        data["history"] = history
        
        try:
            log_path.write_text(json.dumps(data, indent=2), encoding="utf-8")
        except Exception as e:
            print(f"Failed to write debug_metrics.json: {e}", flush=True)

    def _device(self, udid: str) -> None:
        try:
            self._json(read_handset(udid))
        except MuxError as error:
            self._json({"udid": udid, "paired": False, "error": str(error)}, status=409)

    def _json(self, payload: dict, status: int = 200) -> None:
        body = json.dumps(payload).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _mirror_upstream_path(self) -> str:
        path = unquote(urlparse(self.path).path)
        upstream = path.removeprefix(MIRROR_PREFIX) or "/"
        if not upstream.startswith("/"):
            upstream = f"/{upstream}"
        query = urlparse(self.path).query
        return upstream + (f"?{query}" if query else "")

    def _write_client(self, data: bytes) -> bool:
        try:
            self.wfile.write(data)
            self.wfile.flush()
        except _CLIENT_GONE:
            return False
        return True

    def _proxy_mirror_stream(self, method: str, target: str) -> None:
        """Relay the live body de-chunked; this server speaks HTTP/1.0, so the
        response is delimited by closing the connection."""
        conn = http.client.HTTPConnection(STREAM_HOST, STREAM_PORT, timeout=600)
        started = False
        self.close_connection = True
        try:
            conn.request(method, target, headers={"Connection": "close"})
            response = conn.getresponse()
            self.send_response(response.status)
            for key, value in response.getheaders():
                if key.lower() in _HOP_BY_HOP or key.lower() in {"transfer-encoding", "content-length"}:
                    continue
                self.send_header(key, value)
            self.end_headers()
            started = True
            while True:
                chunk = response.read1(65536)
                if not chunk:
                    break
                if not self._write_client(chunk):
                    return
        except (OSError, http.client.HTTPException) as error:
            if not started:
                self._json({"error": f"Screen stream is not running ({error})."}, status=503)
        finally:
            conn.close()

    def _proxy_mirror(self, method: str) -> None:
        target = self._mirror_upstream_path()
        upstream_path = target.split("?", 1)[0]

        # Alias without .bin extension to prevent IDM / download managers from intercepting stream
        if upstream_path in ("/live", "/live/"):
            query = urlparse(self.path).query
            upstream_target = "/stream.bin" + (f"?{query}" if query else "")
            self._proxy_mirror_stream(method, upstream_target)
            return

        is_live_stream = method == "GET" and upstream_path.endswith(".bin")
        if is_live_stream:
            self._proxy_mirror_stream(method, target)
            return

        length = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(length) if length and method in {"POST", "PUT"} else None

        headers: dict[str, str] = {}
        for name in ("Content-Type", "Accept"):
            value = self.headers.get(name)
            if value:
                headers[name] = value

        conn = http.client.HTTPConnection(STREAM_HOST, STREAM_PORT, timeout=120)
        started = False
        try:
            conn.request(method, target, body=body, headers=headers)
            response = conn.getresponse()
            payload = response.read()
            self.send_response(response.status)
            started = True
            for key, value in response.getheaders():
                if key.lower() in _HOP_BY_HOP:
                    continue
                self.send_header(key, value)
            self.end_headers()
            if payload:
                self._write_client(payload)
        except OSError as error:
            if not started:
                self._json({"error": f"Screen stream is not running ({error})."}, status=503)
        finally:
            conn.close()

    def _file(self, path: str) -> None:
        relative = "index.html" if path in {"", "/"} else path.lstrip("/")
        root = ROOT.resolve()
        target = (root / relative).resolve()
        try:
            target.relative_to(root)
        except ValueError:
            self._json({"error": "Not found."}, status=404)
            return
        if not target.is_file():
            self._json({"error": "Not found."}, status=404)
            return
        kind = {
            ".html": "text/html; charset=utf-8",
            ".css": "text/css; charset=utf-8",
            ".js": "text/javascript; charset=utf-8",
            ".svg": "image/svg+xml",
        }.get(target.suffix, "application/octet-stream")
        body = target.read_bytes()
        self.send_response(200)
        self.send_header("Content-Type", kind)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


class SingleServer(ThreadingHTTPServer):
    # On Windows, address reuse lets a second copy bind the same port and steal requests.
    allow_reuse_address = False


def main() -> None:
    settings = load_user_settings()
    if settings.get("audioSinkEnabled"):
        try:
            audio_manager.enable()
        except Exception:
            pass
    server = SingleServer((HOST, PORT), Handler)
    print(f"Handset listening on http://{HOST}:{PORT}", flush=True)
    try:
        server.serve_forever()
    finally:
        audio_manager.shutdown()


if __name__ == "__main__":
    main()

