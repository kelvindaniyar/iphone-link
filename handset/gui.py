"""Native Frameless Launcher for Pane using pywebview (Edge WebView2 on Windows)."""

import os
import sys
import threading
import time
import urllib.request

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

    def set_window(self, window):
        self._window = window

    def _get_hwnd(self):
        try:
            import ctypes
            return ctypes.windll.user32.FindWindowW(None, "iPhone Mirroring")
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

    def sync_device_aspect(self, display_width: int, drawer_open: bool = False):
        """Sesuaikan lebar jendela jika diminta secara manual."""
        if not self._window:
            return
        try:
            dw = max(380, min(800, int(display_width)))
            self._current_display_width = dw
            self._drawer_open = bool(drawer_open)
            target_w = dw + (360 if self._drawer_open else 0)
            h = max(self._window.height or 840, 840)
            self._window.resize(target_w, h)
        except Exception as e:
            print("sync_device_aspect error:", e)

    def set_drawer(self, is_open: bool):
        if self._window:
            self._drawer_open = bool(is_open)
            target_width = 740 if self._drawer_open else 380
            h = max(self._window.height or 840, 840)
            self._window.resize(target_width, h)

    def set_theme(self, theme: str):
        """Update native Windows 11 DWM dark/light mode attribute."""
        is_dark = theme != "light"
        try:
            import ctypes
            from ctypes import wintypes
            hwnd = ctypes.windll.user32.FindWindowW(None, "iPhone Mirroring")
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


def apply_dwm_effects(title="iPhone Mirroring"):
    """Terapkan efek DWM Windows 11 (sudut membulat + dark mode) secara aman pada HWND."""
    try:
        import ctypes
        from ctypes import wintypes
        time.sleep(0.7)
        hwnd = ctypes.windll.user32.FindWindowW(None, title)
        if not hwnd:
            return
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
            req = urllib.request.Request(url, headers={"User-Agent": "Pane-Launcher"})
            with urllib.request.urlopen(req, timeout=1.0) as resp:
                if resp.status == 200:
                    return True
        except Exception:
            time.sleep(0.3)
    return False


def main():
    api = WindowApi()
    
    # Tunggu server aktif jika baru diluncurkan
    wait_for_server()

    # Jadwalkan penerapan efek DWM sudut membulat saat HWND window siap
    threading.Thread(target=apply_dwm_effects, args=("iPhone Mirroring",), daemon=True).start()

    window = webview.create_window(
        title="iPhone Mirroring",
        url=f"http://127.0.0.1:8765/?t={int(time.time())}",
        js_api=api,
        width=380,
        height=840,
        frameless=True,
        easy_drag=False,
        min_size=(380, 700),
        background_color="#0f172a",
    )
    api.set_window(window)

    # Jalankan webview dengan Edge Chromium (WebView2) di Windows
    webview.start(debug=False)


if __name__ == "__main__":
    main()
