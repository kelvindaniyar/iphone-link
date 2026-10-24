"""Native Frameless Launcher for Pane using pywebview (Edge WebView2 on Windows)."""

import os
import sys
import threading
import time
import urllib.request
from pathlib import Path

# WebView2 arguments: pastikan dukungan hardware HEVC decoder aktif
os.environ["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"] = (
    "--enable-features=PlatformHEVCDecoderSupport"
)

import webview


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


def apply_dwm_effects(title="iPhone Link"):
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
            ico_candidates = [
                Path(__file__).resolve().parent.parent / "assets" / "iphone_link.ico",
                Path(__file__).resolve().parent.parent / "packaging" / "pane.ico",
            ]
            for ico_p in ico_candidates:
                if ico_p.exists():
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
                    break
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


def wait_for_server(url="http://127.0.0.1:8765/api/status", timeout=10):
    start = time.time()
    while time.time() - start < timeout:
        try:
            req = urllib.request.Request(url, headers={"User-Agent": "iPhoneLink-Launcher"})
            with urllib.request.urlopen(req, timeout=1.0) as resp:
                if resp.status == 200:
                    return True
        except Exception:
            time.sleep(0.3)
    return False


def main():
    api = WindowApi()
    
    # Set explicit AppUserModelID agar Taskbar Windows 11 menampilkan Logo iPhone Link
    if sys.platform == "win32":
        try:
            import ctypes
            ctypes.windll.shell32.SetCurrentProcessExplicitAppUserModelID("Kelvin.iPhoneLink.Desktop.1")
        except Exception:
            pass

    # Tunggu server aktif jika baru diluncurkan
    wait_for_server()

    # Jadwalkan penerapan efek DWM sudut membulat dan icon native saat HWND window siap
    threading.Thread(target=apply_dwm_effects, args=("iPhone Link",), daemon=True).start()

    window = webview.create_window(
        title="iPhone Link",
        url=f"http://127.0.0.1:8765/?t={int(time.time())}",
        js_api=api,
        width=380,
        height=840,
        frameless=True,
        easy_drag=False,
        min_size=(360, 450),
        background_color="#0f172a",
    )
    api.set_window(window)

    # Jalankan webview dengan Edge Chromium (WebView2) di Windows
    storage_path = str(Path.home() / ".pane_webview_storage")
    os.makedirs(storage_path, exist_ok=True)
    webview.start(
        debug=False,
        private_mode=False,
        storage_path=storage_path,
    )


if __name__ == "__main__":
    main()
