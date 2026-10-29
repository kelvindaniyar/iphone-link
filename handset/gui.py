"""Native Frameless Launcher for iPhone Link using pywebview (Edge WebView2 on Windows).

Tahap 54–56:
- Runs the backend HTTP server directly in-process via a background daemon thread
  (_ensure_backend_server_running), eliminating any separate console/zombie server process.
- Supports both normal Python execution (`python handset/gui.py` / `python -m handset`)
  and standalone frozen PyInstaller executable (`iPhone Link.exe`).
- Tahap 55: Per-folder WebView2 storage isolation (.pane_webview_storage).
- Tahap 56:
  * Disables Windows 11 EcoQoS Power Throttling + sets AboveNormal/High audio priority.
  * Memory-optimized WebView2 flags + safe Win32 Working Set trimmer (_trim_process_working_set).
  * Native Windows 11 System Tray icon (handset.tray.SystemTrayIcon) with Left-Click Show/Hide
    and Right-Click Quick Actions menu + guaranteed NIM_DELETE cleanup.
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

# 3. Tahap 56: Memory-optimized WebView2 arguments while keeping Hardware HEVC GPU acceleration
WEBVIEW2_OPTIMIZED_ARGS = (
    "--enable-features=PlatformHEVCDecoderSupport "
    "--disable-background-networking "
    "--disable-component-update "
    "--disable-domain-reliability "
    "--disable-sync "
    "--disable-features=Translate,OptimizationHints,MediaRouter,CalculateNativeWinOcclusion "
    "--js-flags=--max-old-space-size=96"
)
os.environ["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"] = WEBVIEW2_OPTIMIZED_ARGS

from handset import get_app_root, get_bundle_root

_ACTIVE_HTTP_SERVER = None
_SERVER_THREAD: threading.Thread | None = None
_SERVER_LOCK = threading.Lock()
_ACTIVE_TRAY_ICON = None
_TRIM_LOCK = threading.Lock()
_TRIMMER_STARTED = False


def _trim_process_working_set() -> int:
    """Safely trim unused working set pages for Python + child WebView2 processes via Win32 API.

    Uses CreateToolhelp32Snapshot + psapi.EmptyWorkingSet (0 subprocesses, < 2ms overhead,
    100% safe for active streams because hot pages remain resident).
    """
    if sys.platform != "win32":
        return 0
    if not _TRIM_LOCK.acquire(blocking=False):
        return 0
    trimmed_count = 0
    try:
        import ctypes
        from ctypes import wintypes

        TH32CS_SNAPPROCESS = 0x00000002
        PROCESS_QUERY_LIMITED_INFORMATION = 0x1000
        PROCESS_SET_QUOTA = 0x0100

        class PROCESSENTRY32W(ctypes.Structure):
            _fields_ = [
                ("dwSize", wintypes.DWORD),
                ("cntUsage", wintypes.DWORD),
                ("th32ProcessID", wintypes.DWORD),
                ("th32DefaultHeapID", ctypes.c_size_t),
                ("th32ModuleID", wintypes.DWORD),
                ("cntThreads", wintypes.DWORD),
                ("th32ParentProcessID", wintypes.DWORD),
                ("pcPriClassBase", wintypes.LONG),
                ("dwFlags", wintypes.DWORD),
                ("szExeFile", wintypes.WCHAR * 260),
            ]

        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        psapi = ctypes.WinDLL("psapi", use_last_error=True)

        kernel32.CreateToolhelp32Snapshot.argtypes = [wintypes.DWORD, wintypes.DWORD]
        kernel32.CreateToolhelp32Snapshot.restype = wintypes.HANDLE
        kernel32.Process32FirstW.argtypes = [wintypes.HANDLE, ctypes.POINTER(PROCESSENTRY32W)]
        kernel32.Process32FirstW.restype = wintypes.BOOL
        kernel32.Process32NextW.argtypes = [wintypes.HANDLE, ctypes.POINTER(PROCESSENTRY32W)]
        kernel32.Process32NextW.restype = wintypes.BOOL
        kernel32.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
        kernel32.OpenProcess.restype = wintypes.HANDLE
        kernel32.SetProcessWorkingSetSize.argtypes = [wintypes.HANDLE, ctypes.c_size_t, ctypes.c_size_t]
        kernel32.SetProcessWorkingSetSize.restype = wintypes.BOOL
        kernel32.CloseHandle.argtypes = [wintypes.HANDLE]
        kernel32.CloseHandle.restype = wintypes.BOOL
        psapi.EmptyWorkingSet.argtypes = [wintypes.HANDLE]
        psapi.EmptyWorkingSet.restype = wintypes.BOOL

        root_pid = os.getpid()
        parent_map: dict[int, list[int]] = {}

        snap = kernel32.CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
        invalid_handle = wintypes.HANDLE(-1).value
        if snap and snap != invalid_handle:
            try:
                pe = PROCESSENTRY32W()
                pe.dwSize = ctypes.sizeof(PROCESSENTRY32W)
                if kernel32.Process32FirstW(snap, ctypes.byref(pe)):
                    while True:
                        parent_map.setdefault(int(pe.th32ParentProcessID), []).append(int(pe.th32ProcessID))
                        if not kernel32.Process32NextW(snap, ctypes.byref(pe)):
                            break
            finally:
                kernel32.CloseHandle(snap)

        # Collect current process + all descendant processes (WebView2 Browser/Renderer/GPU/Utility)
        target_pids: list[int] = [root_pid]
        queue = [root_pid]
        seen = {root_pid}
        while queue:
            cur = queue.pop(0)
            for child in parent_map.get(cur, ()):
                if child not in seen:
                    seen.add(child)
                    target_pids.append(child)
                    queue.append(child)

        all_minus_one = ctypes.c_size_t(-1).value
        for pid in target_pids:
            hproc = kernel32.OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SET_QUOTA, False, pid)
            if hproc:
                try:
                    ok = bool(psapi.EmptyWorkingSet(hproc))
                    if not ok:
                        ok = bool(kernel32.SetProcessWorkingSetSize(hproc, all_minus_one, all_minus_one))
                    if ok:
                        trimmed_count += 1
                finally:
                    kernel32.CloseHandle(hproc)
    except Exception:
        pass
    finally:
        _TRIM_LOCK.release()
    return trimmed_count


def schedule_working_set_trim(delay_sec: float = 0.25) -> None:
    """Trigger a non-blocking working set trim after delay_sec."""
    def _delayed():
        if delay_sec > 0:
            time.sleep(delay_sec)
        _trim_process_working_set()

    threading.Thread(target=_delayed, name="iphone-link-ram-trim", daemon=True).start()


def _start_periodic_memory_trimmer(interval_sec: float = 45.0) -> None:
    """Start single lightweight daemon thread: trims once at +5s after startup, then every 45s."""
    global _TRIMMER_STARTED
    if _TRIMMER_STARTED or sys.platform != "win32":
        return
    _TRIMMER_STARTED = True

    def _loop():
        time.sleep(5.0)
        _trim_process_working_set()
        while True:
            time.sleep(interval_sec)
            _trim_process_working_set()

    threading.Thread(target=_loop, name="iphone-link-periodic-trimmer", daemon=True).start()


class WindowApi:
    """JS-accessible API exposed under window.pywebview.api."""

    def __init__(self):
        self._window = None
        self._cached_hwnd: int | None = None
        self._current_display_width = 380
        self._drawer_open = False
        self._is_landscape = False
        self._hidden_to_tray = False

    def set_window(self, window):
        self._window = window

    def _get_hwnd(self):
        try:
            import ctypes
            from ctypes import wintypes

            if self._cached_hwnd and ctypes.windll.user32.IsWindow(wintypes.HWND(self._cached_hwnd)):
                return self._cached_hwnd
            h = ctypes.windll.user32.FindWindowW(None, "iPhone Link")
            if not h:
                h = ctypes.windll.user32.FindWindowW(None, "iPhone Mirroring")
            if h:
                self._cached_hwnd = h
            return h
        except Exception:
            return None

    def _eval_js(self, script: str) -> None:
        if self._window is None:
            return
        try:
            self._window.evaluate_js(script)
        except Exception:
            pass

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
            schedule_working_set_trim(0.35)

    def minimize_to_tray(self):
        """Hide window to Windows 11 System Tray while keeping mirroring & audio alive in background."""
        self._hidden_to_tray = True
        if sys.platform == "win32":
            hwnd = self._get_hwnd()
            if hwnd:
                try:
                    import ctypes
                    from ctypes import wintypes
                    ctypes.windll.user32.ShowWindow(wintypes.HWND(hwnd), 0)  # SW_HIDE
                    schedule_working_set_trim(0.3)
                    return {"ok": True, "hidden": True}
                except Exception:
                    pass
        if self._window:
            try:
                self._window.hide()
            except Exception:
                pass
        schedule_working_set_trim(0.3)
        return {"ok": True, "hidden": True}

    def restore_from_tray(self):
        """Show/restore iPhone Link window from System Tray or minimized state and bring to front."""
        self._hidden_to_tray = False
        if sys.platform == "win32":
            hwnd = self._get_hwnd()
            if hwnd:
                try:
                    import ctypes
                    from ctypes import wintypes
                    user32 = ctypes.windll.user32
                    wh = wintypes.HWND(hwnd)
                    if not user32.IsWindowVisible(wh):
                        user32.ShowWindow(wh, 5)  # SW_SHOW
                    if user32.IsIconic(wh):
                        user32.ShowWindow(wh, 9)  # SW_RESTORE
                    user32.BringWindowToTop(wh)
                    user32.SetForegroundWindow(wh)
                except Exception:
                    pass
        if self._window:
            try:
                self._window.show()
                self._window.restore()
            except Exception:
                pass
            self._eval_js("window.dispatchEvent(new Event('resize'));")
        return {"ok": True, "hidden": False}

    def toggle_tray_window(self):
        """Left-click System Tray handler: restore/focus if hidden or minimized, else hide to tray."""
        if sys.platform == "win32":
            hwnd = self._get_hwnd()
            if hwnd:
                try:
                    import ctypes
                    from ctypes import wintypes
                    user32 = ctypes.windll.user32
                    wh = wintypes.HWND(hwnd)
                    if self._hidden_to_tray or not user32.IsWindowVisible(wh) or user32.IsIconic(wh):
                        return self.restore_from_tray()
                    if user32.GetForegroundWindow() != hwnd:
                        return self.restore_from_tray()
                    return self.minimize_to_tray()
                except Exception:
                    pass
        if self._hidden_to_tray:
            return self.restore_from_tray()
        return self.minimize_to_tray()

    def tray_toggle_landscape(self):
        if self._hidden_to_tray:
            self.restore_from_tray()
        self._eval_js("if (window.toggleLandscapeMode) window.toggleLandscapeMode();")

    def tray_toggle_stealth(self):
        if self._window is not None:
            self._eval_js("if (window.triggerToggleStealthDim) window.triggerToggleStealthDim();")
            return
        try:
            from handset.screen import _STEALTH_DIM_ACTIVE, set_iphone_physical_brightness
            set_iphone_physical_brightness("restore" if _STEALTH_DIM_ACTIVE else "min", steps=4 if _STEALTH_DIM_ACTIVE else 16)
        except Exception:
            pass

    def tray_toggle_audio(self):
        if self._window is not None:
            self._eval_js("if (window.trayToggleAudio) window.trayToggleAudio();")
            return
        try:
            from handset.audio import audio_manager
            audio_manager.toggle()
        except Exception:
            pass

    def tray_clear_copy_cache(self):
        try:
            from handset.screen import clear_recent_long_ocr_chunks
            clear_recent_long_ocr_chunks()
        except Exception:
            pass
        self._eval_js("if (window.triggerClearCopyCacheWithFeedback) window.triggerClearCopyCacheWithFeedback();")

    def get_tray_menu_state(self) -> dict[str, bool]:
        stealth_on = False
        audio_on = False
        try:
            from handset.screen import _STEALTH_DIM_ACTIVE
            stealth_on = bool(_STEALTH_DIM_ACTIVE)
        except Exception:
            pass
        try:
            from handset.audio import audio_manager
            audio_on = audio_manager.is_active()
        except Exception:
            pass
        return {
            "landscape": bool(self._is_landscape),
            "stealth": stealth_on,
            "audio": audio_on,
        }

    def close(self):
        global _ACTIVE_TRAY_ICON
        tray = _ACTIVE_TRAY_ICON
        _ACTIVE_TRAY_ICON = None
        if tray is not None:
            try:
                tray.stop()
            except Exception:
                pass
        if self._window:
            try:
                self._window.destroy()
            except Exception:
                pass

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


def apply_dwm_effects(title: str = "iPhone Link", api: WindowApi | None = None) -> None:
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
        if api is not None:
            api._cached_hwnd = hwnd

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
    """Cleanly stop System Tray icon, restore Stealth brightness if active, and stop server/audio."""
    global _ACTIVE_HTTP_SERVER, _SERVER_THREAD, _ACTIVE_TRAY_ICON
    tray = _ACTIVE_TRAY_ICON
    _ACTIVE_TRAY_ICON = None
    if tray is not None:
        try:
            tray.stop()
        except Exception:
            pass
    try:
        from handset import screen as _scr
        if getattr(_scr, "_STEALTH_DIM_ACTIVE", False):
            _scr.set_iphone_physical_brightness("restore", steps=4)
        _scr.stop_screen()
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
    """If an iPhone Link window is already open (or hidden to System Tray), restore & bring to front."""
    if sys.platform != "win32":
        return False
    try:
        import ctypes
        from ctypes import wintypes

        hwnd = ctypes.windll.user32.FindWindowW(None, "iPhone Link")
        if hwnd and _is_server_responding(f"http://{host}:{port}/api/status", timeout=0.6):
            wh = wintypes.HWND(hwnd)
            if not ctypes.windll.user32.IsWindowVisible(wh):
                ctypes.windll.user32.ShowWindow(wh, 5)  # SW_SHOW
            if ctypes.windll.user32.IsIconic(wh):
                ctypes.windll.user32.ShowWindow(wh, 9)  # SW_RESTORE
            ctypes.windll.user32.SetForegroundWindow(wh)
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
    from handset.tray import SystemTrayIcon  # noqa: F401

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


def _resolve_webview_storage_path() -> str:
    """Return a per-folder isolated WebView2 storage path so fresh extracts never inherit old PINs."""
    local_dir = get_app_root() / ".pane_webview_storage"
    try:
        local_dir.mkdir(parents=True, exist_ok=True)
        return str(local_dir)
    except OSError:
        import hashlib
        folder_id = hashlib.sha256(
            os.path.normcase(str(get_app_root().resolve())).encode("utf-8")
        ).hexdigest()[:12]
        fallback = Path.home() / ".pane_webview_storage_instances" / folder_id
        fallback.mkdir(parents=True, exist_ok=True)
        return str(fallback)


def main() -> None:
    global _ACTIVE_TRAY_ICON
    if "--smoke-test" in sys.argv:
        raise SystemExit(_run_smoke_test())

    import webview
    from handset.audio import disable_process_power_throttling, set_audio_process_priority
    from handset.server import HOST as DEFAULT_HOST, PORT as DEFAULT_PORT
    from handset.tray import SystemTrayIcon

    host = DEFAULT_HOST
    port = int(os.environ.get("PANE_PORT", DEFAULT_PORT))

    if _focus_existing_window_if_running(host, port):
        return

    # Tahap 56: Disable Windows 11 EcoQoS throttling & elevate baseline process priority
    disable_process_power_throttling()
    set_audio_process_priority(high_for_audio=False)

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
    threading.Thread(target=apply_dwm_effects, args=("iPhone Link", api), daemon=True).start()

    # Tahap 56: Start Native Windows 11 System Tray Icon
    tray = SystemTrayIcon(
        icon_path=_resolve_icon_path(),
        on_restore=api.restore_from_tray,
        on_hide=api.minimize_to_tray,
        on_toggle_window=api.toggle_tray_window,
        on_toggle_landscape=api.tray_toggle_landscape,
        on_toggle_stealth=api.tray_toggle_stealth,
        on_toggle_audio=api.tray_toggle_audio,
        on_clear_cache=api.tray_clear_copy_cache,
        on_quit=api.close,
        state_provider=api.get_tray_menu_state,
    )
    tray.start(wait_ready=1.5)
    _ACTIVE_TRAY_ICON = tray

    # Tahap 56: Start safe periodic working set trimmer (+5s & every 45s)
    _start_periodic_memory_trimmer(interval_sec=45.0)

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

    storage_path = _resolve_webview_storage_path()
    try:
        webview.start(
            debug=False,
            private_mode=False,
            storage_path=storage_path,
        )
    finally:
        if _ACTIVE_TRAY_ICON is not None:
            try:
                _ACTIVE_TRAY_ICON.stop()
            except Exception:
                pass
            _ACTIVE_TRAY_ICON = None
        shutdown_backend_server()


if __name__ == "__main__":
    main()
