"""Native Frameless Launcher for iPhone Link using pywebview (Edge WebView2 on Windows).

Tahap 54:
- Runs the backend HTTP server directly in-process via a background daemon thread
  (_ensure_backend_server_running), eliminating any separate console/zombie server process.
- Supports both normal Python execution (`python handset/gui.py` / `python -m handset`)
  and standalone frozen PyInstaller executable (`iPhone Link.exe`).
"""

from __future__ import annotations

import os
import sys
import threading
import time
import urllib.request
from pathlib import Path

# 1. Ensure safe stdout/stderr handles when running windowless (pythonw.exe or frozen .exe)
if sys.stdout is None:
    sys.stdout = open(os.devnull, "w", encoding="utf-8")
if sys.stderr is None:
    sys.stderr = open(os.devnull, "w", encoding="utf-8")

# 2. Ensure project root is in sys.path when invoked directly as `python handset/gui.py`
if not getattr(sys, "frozen", False):
    _REPO_ROOT = Path(__file__).resolve().parent.parent
    if str(_REPO_ROOT) not in sys.path:
        sys.path.insert(0, str(_REPO_ROOT))

# 3. WebView2 arguments: pastikan dukungan hardware HEVC decoder aktif
os.environ["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"] = (
    "--enable-features=PlatformHEVCDecoderSupport"
)

from handset import get_app_root, get_bundle_root

_ACTIVE_HTTP_SERVER = None
_SERVER_THREAD: threading.Thread | None = None
_SERVER_LOCK = threading.Lock()


class WindowApi:
    """JS-accessible API exposed under window.pywebview.api."""

    def __init__(self):
        self._window = None
        self._current_display_width = 380
        self._drawer_open = False
        self._is_landscape = False

    def set_window(self, window):
        self._window = window

    def _get_hwnd(self):
        try:
            import ctypes
            h = ctypes.windll.user32.FindWindowW(None, "iPhone Link")
            if not h:
                h = ctypes.windll.user32.FindWindowW(None, "iPhone Mirroring")
            return h
        except Exception:
            return None

    def start_window_drag(self) -> None:
        """Start native Windows window dragging without pywebview global hook."""
        if self._window is None or sys.platform != "win32":
            return
        try:
            hwnd = self._get_hwnd()
            if hwnd:
                import ctypes
                from ctypes import wintypes
                ctypes.windll.user32.ReleaseCapture()
                ctypes.windll.user32.SendMessageW(wintypes.HWND(hwnd), 0x00A1, 2, 0)  # WM_NCLBUTTONDOWN, HTCAPTION
        except Exception:
            pass

    def minimize(self):
        if self._window:
            self._window.minimize()

    def close(self):
        if self._window:
            self._window.destroy()

    def toggle_fullscreen(self):
        if self._window:
            self._window.toggle_fullscreen()

    def set_orientation(self, landscape: bool, drawer_open: bool = False):
        """Ubah ukuran jendela ke dimensi Landscape / Horizontal atau Portrait."""
        if not self._window:
            return
        try:
            self._is_landscape = bool(landscape)
            self._drawer_open = bool(drawer_open)
            if self._is_landscape:
                target_w = 1200 if self._drawer_open else 840
                target_h = 720 if self._drawer_open else 490
            else:
                target_w = 740 if self._drawer_open else 380
                target_h = 840
            self._window.resize(target_w, target_h)
        except Exception as e:
            print("set_orientation error:", e)

    def sync_device_aspect(self, display_width: int, drawer_open: bool = False):
        """Sesuaikan lebar jendela jika diminta secara manual."""
        if not self._window:
            return
        try:
            self._drawer_open = bool(drawer_open)
            if self._is_landscape:
                target_w = 1200 if self._drawer_open else 840
                target_h = 720 if self._drawer_open else 490
            else:
                dw = max(380, min(800, int(display_width)))
                self._current_display_width = dw
                target_w = dw + (360 if self._drawer_open else 0)
                target_h = max(self._window.height or 840, 840)
            self._window.resize(target_w, target_h)
        except Exception as e:
            print("sync_device_aspect error:", e)

    def set_drawer(self, is_open: bool):
        if self._window:
            self._drawer_open = bool(is_open)
            if self._is_landscape:
                target_w = 1200 if self._drawer_open else 840
                target_h = 720 if self._drawer_open else 490
            else:
                target_w = 740 if self._drawer_open else 380
                target_h = 840
            self._window.resize(target_w, target_h)

    def set_theme(self, theme: str):
        """Update native Windows 11 DWM dark/light mode attribute."""
        is_dark = theme != "light"
        try:
            import ctypes
            from ctypes import wintypes
            hwnd = self._get_hwnd()
            if hwnd:
                ctypes.windll.dwmapi.DwmSetWindowAttribute(
                    wintypes.HWND(hwnd),
                    20,  # DWMWA_USE_IMMERSIVE_DARK_MODE
                    ctypes.byref(ctypes.c_int(1 if is_dark else 0)),
                    ctypes.sizeof(ctypes.c_int),
                )
        except Exception:
            pass

    def is_pywebview(self):
        return True

    def wipe_privacy_data(self):
        """Wipe in-memory and disk privacy data (Tahap 52)."""
        from handset.screen import wipe_all_privacy_data
        return wipe_all_privacy_data()


def _resolve_icon_path() -> Path | None:
    """Locate iphone_link.ico across normal and PyInstaller frozen layouts."""
    candidates = [
        get_app_root() / "assets" / "iphone_link.ico",
        get_bundle_root() / "assets" / "iphone_link.ico",
        Path(__file__).resolve().parent.parent / "assets" / "iphone_link.ico",
        get_app_root() / "packaging" / "pane.ico",
        get_bundle_root() / "packaging" / "pane.ico",
    ]
    for cand in candidates:
        if cand.is_file():
            return cand
    return None


def apply_dwm_effects(title: str = "iPhone Link") -> None:
    """Terapkan efek DWM Windows 11 (sudut membulat + dark mode) dan icon native pada HWND."""
    try:
        import ctypes
        from ctypes import wintypes
        time.sleep(0.7)
        hwnd = ctypes.windll.user32.FindWindowW(None, title)
        if not hwnd:
            hwnd = ctypes.windll.user32.FindWindowW(None, "iPhone Mirroring")
        if not hwnd:
            return

        # 0. Set Native Windows Window Icon (.ico)
        try:
            ico_p = _resolve_icon_path()
            if ico_p is not None:
                IMAGE_ICON = 1
                LR_LOADFROMFILE = 0x0010
                LR_DEFAULTSIZE = 0x0040
                WM_SETICON = 0x0080
                ICON_SMALL = 0
                ICON_BIG = 1
                hicon = ctypes.windll.user32.LoadImageW(
                    None, str(ico_p), IMAGE_ICON, 0, 0, LR_LOADFROMFILE | LR_DEFAULTSIZE
                )
                if hicon:
                    ctypes.windll.user32.SendMessageW(wintypes.HWND(hwnd), WM_SETICON, ICON_SMALL, hicon)
                    ctypes.windll.user32.SendMessageW(wintypes.HWND(hwnd), WM_SETICON, ICON_BIG, hicon)
        except Exception:
            pass

        dwmapi = getattr(ctypes.windll, "dwmapi", None)
        if not dwmapi:
            return
        # 1. Sudut membulat Windows 11 (DWMWA_WINDOW_CORNER_PREFERENCE = 33, DWMWCP_ROUND = 2)
        dwmapi.DwmSetWindowAttribute(
            wintypes.HWND(hwnd),
            33,
            ctypes.byref(ctypes.c_int(2)),
            ctypes.sizeof(ctypes.c_int),
        )
        # 2. Immersive Dark Mode default (DWMWA_USE_IMMERSIVE_DARK_MODE = 20)
        dwmapi.DwmSetWindowAttribute(
            wintypes.HWND(hwnd),
            20,
            ctypes.byref(ctypes.c_int(1)),
            ctypes.sizeof(ctypes.c_int),
        )
    except Exception:
        pass


def _is_server_responding(url: str, timeout: float = 4.0) -> bool:
    try:
        req = urllib.request.Request(url, headers={"User-Agent": "iPhoneLink-Launcher"})
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return resp.status == 200
    except Exception:
        return False


def wait_for_server(url: str = "http://127.0.0.1:8765/api/status", timeout: float = 8.0) -> bool:
    start = time.time()
    while time.time() - start < timeout:
        if _is_server_responding(url, timeout=4.0):
            return True
        time.sleep(0.15)
    return False


def _ensure_backend_server_running(
    host: str = "127.0.0.1",
    port: int = 8765,
    timeout: float = 8.0,
) -> bool:
    """Ensure the backend HTTP server is running in-process inside a daemon thread."""
    global _ACTIVE_HTTP_SERVER, _SERVER_THREAD
    status_url = f"http://{host}:{port}/api/status"
    if _is_server_responding(status_url, timeout=4.0):
        return True

    with _SERVER_LOCK:
        if _is_server_responding(status_url, timeout=4.0):
            return True
        from handset.server import create_http_server
        try:
            srv = create_http_server(host=host, port=port)
            _ACTIVE_HTTP_SERVER = srv
            t = threading.Thread(
                target=srv.serve_forever,
                name=f"iphone-link-server-{port}",
                daemon=True,
            )
            t.start()
            _SERVER_THREAD = t
        except OSError:
            pass

    return wait_for_server(url=status_url, timeout=timeout)


def shutdown_backend_server() -> None:
    """Cleanly stop in-process backend server, screen tunnel, and audio manager."""
    global _ACTIVE_HTTP_SERVER, _SERVER_THREAD
    try:
        from handset.screen import stop_screen
        stop_screen()
    except Exception:
        pass
    try:
        from handset.audio import audio_manager
        audio_manager.shutdown()
    except Exception:
        pass
    with _SERVER_LOCK:
        srv = _ACTIVE_HTTP_SERVER
        _ACTIVE_HTTP_SERVER = None
        _SERVER_THREAD = None
    if srv is not None:
        try:
            srv.shutdown()
            srv.server_close()
        except Exception:
            pass


def _focus_existing_window_if_running(host: str, port: int) -> bool:
    """If an iPhone Link window is already open and backend is live, bring it to front."""
    if sys.platform != "win32":
        return False
    try:
        import ctypes
        hwnd = ctypes.windll.user32.FindWindowW(None, "iPhone Link")
        if hwnd and _is_server_responding(f"http://{host}:{port}/api/status", timeout=0.4):
            if ctypes.windll.user32.IsIconic(hwnd):
                ctypes.windll.user32.ShowWindow(hwnd, 9)  # SW_RESTORE
            ctypes.windll.user32.SetForegroundWindow(hwnd)
            return True
    except Exception:
        pass
    return False


def _run_smoke_test() -> int:
    """Headless self-test for verifying standalone EXE & portable builds."""
    import json
    import socket
    import webview  # noqa: F401
    import pymobiledevice3  # noqa: F401
    import pymobiledevice3.remote.core_device.screen_stream  # noqa: F401
    import lzfse  # noqa: F401
    import lzss  # noqa: F401
    from handset.server import _resolve_ocr_script_path, _resolve_web_root

    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(("127.0.0.1", 0))
        free_port = s.getsockname()[1]

    try:
        ok = _ensure_backend_server_running(host="127.0.0.1", port=free_port, timeout=8.0)
        if not ok:
            return 2
        req = urllib.request.Request(f"http://127.0.0.1:{free_port}/api/status")
        with urllib.request.urlopen(req, timeout=5.0) as resp:
            data = json.loads(resp.read().decode("utf-8"))
            if "screen" not in data:
                return 3
        web_root = _resolve_web_root()
        ocr_script = Path(_resolve_ocr_script_path())
        icon_path = _resolve_icon_path()
        if not (web_root / "index.html").is_file():
            return 4
        if not ocr_script.is_file():
            return 5
        if icon_path is None or not icon_path.is_file():
            return 6
        return 0
    finally:
        shutdown_backend_server()


def main() -> None:
    if "--smoke-test" in sys.argv:
        raise SystemExit(_run_smoke_test())

    import webview
    from handset.server import HOST as DEFAULT_HOST, PORT as DEFAULT_PORT

    host = DEFAULT_HOST
    port = int(os.environ.get("PANE_PORT", DEFAULT_PORT))

    if _focus_existing_window_if_running(host, port):
        return

    api = WindowApi()

    # Set explicit AppUserModelID agar Taskbar Windows 11 menampilkan Logo iPhone Link
    if sys.platform == "win32":
        try:
            import ctypes
            ctypes.windll.shell32.SetCurrentProcessExplicitAppUserModelID("Kelvin.iPhoneLink.Desktop.1")
        except Exception:
            pass

    # Nyalakan server backend secara in-process (daemon thread) jika belum berjalan
    _ensure_backend_server_running(host=host, port=port, timeout=5.0)

    # Jadwalkan penerapan efek DWM sudut membulat dan icon native saat HWND window siap
    threading.Thread(target=apply_dwm_effects, args=("iPhone Link",), daemon=True).start()

    window = webview.create_window(
        title="iPhone Link",
        url=f"http://{host}:{port}/?t={int(time.time())}",
        js_api=api,
        width=380,
        height=840,
        frameless=True,
        easy_drag=False,
        min_size=(368, 450),
        background_color="#0f172a",
    )
    api.set_window(window)

    storage_path = str(Path.home() / ".pane_webview_storage")
    os.makedirs(storage_path, exist_ok=True)
    try:
        webview.start(
            debug=False,
            private_mode=False,
            storage_path=storage_path,
        )
    finally:
        shutdown_backend_server()


if __name__ == "__main__":
    main()
