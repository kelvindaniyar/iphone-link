"""Native Frameless Launcher for Pane using pywebview (Edge WebView2 on Windows)."""

import sys
import time
import urllib.request
import webview


class WindowApi:
    """JS-accessible API exposed under window.pywebview.api."""

    def __init__(self):
        self._window = None

    def set_window(self, window):
        self._window = window

    def minimize(self):
        if self._window:
            self._window.minimize()

    def close(self):
        if self._window:
            self._window.destroy()

    def toggle_fullscreen(self):
        if self._window:
            self._window.toggle_fullscreen()

    def set_drawer(self, is_open: bool):
        if self._window:
            # Side drawer: expand window to 740px or collapse back to 380px
            target_width = 740 if is_open else 380
            h = max(self._window.height or 840, 840)
            self._window.resize(target_width, h)

    def is_pywebview(self):
        return True


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

    window = webview.create_window(
        title="iPhone Mirroring",
        url="http://127.0.0.1:8765/?v=20261023_fix",
        js_api=api,
        width=380,
        height=840,
        frameless=True,
        easy_drag=True,
        min_size=(360, 600),
        background_color="#000000",
    )
    api.set_window(window)

    # Jalankan webview dengan Edge Chromium (WebView2) di Windows
    webview.start(debug=False)


if __name__ == "__main__":
    main()
