"""Native Frameless Launcher for iPhone Link using pywebview (Edge WebView2 on Windows).

Tahap 58 Architecture (Pure Tahap 53 2-Process Isolation + Win32 Job Object Anti-Zombie):
- Runs Backend Server (`handset.server` + `screen.py` + `audio.py`) in a SEPARATE isolated OS
  child process (`--server-child` for `.exe`, `-m handset --server-child` for `.py`) with
  CREATE_NO_WINDOW — zero shared GIL or COM apartment interference with pywebview WinForms/.NET CLR!
- Binds the isolated server child process (and its PowerShell A2DP audio worker) to a kernel
  Win32 Job Object configured with JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE (0x2000), guaranteeing
  0% zombie background processes even if iPhone Link is force-killed from Task Manager.
- Restores pure Tahap 53 WebView2 arguments (`--enable-features=PlatformHEVCDecoderSupport`),
  keeps per-folder WebView2 storage isolation (Tahap 55), clean box-free Windows 11 System Tray
  (Tahap 57), and one-shot startup (+5s) + minimize-only working set trim (no periodic timer).
"""

from __future__ import annotations

import multiprocessing
import os
import subprocess
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


def _parse_server_cli_args(argv: list[str]) -> tuple[str, int]:
    host = os.environ.get("PANE_HOST", "127.0.0.1")
    port = int(os.environ.get("PANE_PORT", "8765"))
    for idx, arg in enumerate(argv):
        if arg == "--host" and idx + 1 < len(argv):
            host = argv[idx + 1]
        elif arg.startswith("--host="):
            host = arg.split("=", 1)[1]
        elif arg == "--port" and idx + 1 < len(argv):
            try:
                port = int(argv[idx + 1])
            except ValueError:
                pass
        elif arg.startswith("--port="):
            try:
                port = int(arg.split("=", 1)[1])
            except ValueError:
                pass
    return host, port


# 3. EARLY SERVER-CHILD INTERCEPT (BEFORE importing webview / pythonnet / .NET CLR!):
#    Guarantees 100% pure Python backend server process identical to Tahap 53 (`-m handset.server`).
if __name__ == "__main__":
    multiprocessing.freeze_support()
    if "--server-child" in sys.argv or "--server-only" in sys.argv:
        _srv_host, _srv_port = _parse_server_cli_args(sys.argv[1:])
        from handset.server import serve as _serve_backend
        _serve_backend(host=_srv_host, port=_srv_port)
        raise SystemExit(0)

# 4. Pure Tahap 53 WebView2 Hardware HEVC Decoder Acceleration flag
WEBVIEW2_DEFAULT_ARGS = "--enable-features=PlatformHEVCDecoderSupport"
os.environ["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"] = WEBVIEW2_DEFAULT_ARGS

_WIN_NO_WINDOW_KWARGS: dict = {}
if sys.platform == "win32":
    _si = subprocess.STARTUPINFO()
    _si.dwFlags |= subprocess.STARTF_USESHOWWINDOW
    _si.wShowWindow = 0  # SW_HIDE
    _WIN_NO_WINDOW_KWARGS = {
        "creationflags": getattr(subprocess, "CREATE_NO_WINDOW", 0x08000000),
        "startupinfo": _si,
    }

from handset import get_app_root, get_bundle_root

JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE = 0x00002000
JobObjectExtendedLimitInformation = 9

_SERVER_LOCK = threading.Lock()
_SERVER_PROC: subprocess.Popen | None = None
_SERVER_JOB_HANDLE: int | None = None
_SERVER_ENDPOINT: tuple[str, int] = ("127.0.0.1", 8765)
_ACTIVE_TRAY_ICON = None
_TRIM_LOCK = threading.Lock()


def _create_kill_on_close_job_object() -> int | None:
    """Create a Win32 Job Object with JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE (0x2000).

    When the parent GUI process closes (even via Task Manager End Task), the Windows NT kernel
    immediately terminates all assigned child & grandchild processes (server + PowerShell audio).
    """
    if sys.platform != "win32":
        return None
    try:
        import ctypes
        from ctypes import wintypes

        class JOBOBJECT_BASIC_LIMIT_INFORMATION(ctypes.Structure):
            _fields_ = [
                ("PerProcessUserTimeLimit", ctypes.c_int64),
                ("PerJobUserTimeLimit", ctypes.c_int64),
                ("LimitFlags", wintypes.DWORD),
                ("MinimumWorkingSetSize", ctypes.c_size_t),
                ("MaximumWorkingSetSize", ctypes.c_size_t),
                ("ActiveProcessLimit", wintypes.DWORD),
                ("Affinity", ctypes.c_size_t),
                ("PriorityClass", wintypes.DWORD),
                ("SchedulingClass", wintypes.DWORD),
            ]

        class IO_COUNTERS(ctypes.Structure):
            _fields_ = [
                ("ReadOperationCount", ctypes.c_uint64),
                ("WriteOperationCount", ctypes.c_uint64),
                ("OtherOperationCount", ctypes.c_uint64),
                ("ReadTransferCount", ctypes.c_uint64),
                ("WriteTransferCount", ctypes.c_uint64),
                ("OtherTransferCount", ctypes.c_uint64),
            ]

        class JOBOBJECT_EXTENDED_LIMIT_INFORMATION(ctypes.Structure):
            _fields_ = [
                ("BasicLimitInformation", JOBOBJECT_BASIC_LIMIT_INFORMATION),
                ("IoInfo", IO_COUNTERS),
                ("ProcessMemoryLimit", ctypes.c_size_t),
                ("JobMemoryLimit", ctypes.c_size_t),
                ("PeakProcessMemoryUsed", ctypes.c_size_t),
                ("PeakJobMemoryUsed", ctypes.c_size_t),
            ]

        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel32.CreateJobObjectW.argtypes = [wintypes.LPVOID, wintypes.LPCWSTR]
        kernel32.CreateJobObjectW.restype = wintypes.HANDLE
        kernel32.SetInformationJobObject.argtypes = [
            wintypes.HANDLE,
            ctypes.c_int,
            wintypes.LPVOID,
            wintypes.DWORD,
        ]
        kernel32.SetInformationJobObject.restype = wintypes.BOOL
        kernel32.CloseHandle.argtypes = [wintypes.HANDLE]
        kernel32.CloseHandle.restype = wintypes.BOOL

        hjob = kernel32.CreateJobObjectW(None, None)
        if not hjob:
            return None

        info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION()
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        ok = bool(
            kernel32.SetInformationJobObject(
                wintypes.HANDLE(hjob),
                JobObjectExtendedLimitInformation,
                ctypes.byref(info),
                ctypes.sizeof(info),
            )
        )
        if not ok:
            kernel32.CloseHandle(wintypes.HANDLE(hjob))
            return None
        return int(hjob)
    except Exception:
        return None


def _assign_process_to_job(job_handle: int | None, proc: subprocess.Popen) -> bool:
    """Bind a spawned subprocess to the Win32 Job Object."""
    if sys.platform != "win32" or not job_handle or proc is None:
        return False
    try:
        import ctypes
        from ctypes import wintypes

        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel32.AssignProcessToJobObject.argtypes = [wintypes.HANDLE, wintypes.HANDLE]
        kernel32.AssignProcessToJobObject.restype = wintypes.BOOL
        return bool(
            kernel32.AssignProcessToJobObject(
                wintypes.HANDLE(job_handle),
                wintypes.HANDLE(int(proc._handle)),
            )
        )
    except Exception:
        return False


def _close_job_handle(job_handle: int | None) -> None:
    if sys.platform != "win32" or not job_handle:
        return
    try:
        import ctypes
        from ctypes import wintypes

        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel32.CloseHandle.argtypes = [wintypes.HANDLE]
        kernel32.CloseHandle.restype = wintypes.BOOL
        kernel32.CloseHandle(wintypes.HANDLE(job_handle))
    except Exception:
        pass


def _trim_process_working_set() -> int:
    """One-shot safe working set trim (used ONLY once at startup +5s and when minimized to tray)."""
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
    """Trigger a single non-blocking working set trim after delay_sec (no periodic loop)."""
    def _delayed():
        if delay_sec > 0:
            time.sleep(delay_sec)
        _trim_process_working_set()

    threading.Thread(target=_delayed, name="iphone-link-ram-trim", daemon=True).start()


def _cleanup_stale_zombie_processes(host: str = "127.0.0.1", port: int = 8765) -> None:
    """Clean up any orphaned iPhone Link / handset.server / AudioPlaybackConnection background processes
    left over from an earlier crashed session before starting a fresh GUI + server pair."""
    if sys.platform != "win32":
        return

    # 1. Politely tell any lingering server on `port` to shut down first
    try:
        req = urllib.request.Request(
            f"http://{host}:{port}/api/server/shutdown",
            data=b"{}",
            headers={"Content-Type": "application/json"},
            method="POST",
        )
        with urllib.request.urlopen(req, timeout=0.6):
            pass
    except Exception:
        pass

    # 2. Terminate any orphan iPhone Link.exe, handset server python, or AudioPlaybackConnection PowerShell
    my_pid = os.getpid()
    my_ppid = os.getppid()
    ps_sweep = f"""
$myPid = {my_pid}
$myPpid = {my_ppid}
Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Where-Object {{
    $_.ProcessId -ne $PID -and $_.ProcessId -ne $myPid -and $_.ProcessId -ne $myPpid -and (
        ($_.Name -ieq 'iPhone Link.exe') -or
        ($_.Name -match '^pythonw?(\\.exe)?$' -and $_.CommandLine -match '(handset\\.server|handset\\\\server\\.py|--server-child|--server-only|audio\\.py)') -or
        ($_.Name -match '^powershell(\\.exe)?$' -and $_.CommandLine -match 'AudioPlaybackConnection')
    )
}} | ForEach-Object {{
    try {{ Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }} catch {{}}
}}
"""
    try:
        subprocess.run(
            ["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", ps_sweep],
            capture_output=True,
            text=True,
            timeout=5,
            **_WIN_NO_WINDOW_KWARGS,
        )
    except Exception:
        pass


class WindowApi:
    """JS-accessible API exposed under window.pywebview.api."""

    def __init__(self, host: str = "127.0.0.1", port: int = 8765):
        self._window = None
        self._host = host
        self._port = port
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

    def _api_post(self, path: str, payload: bytes = b"{}", timeout: float = 1.5) -> dict:
        import json

        req = urllib.request.Request(
            f"http://{self._host}:{self._port}{path}",
            data=payload,
            headers={"Content-Type": "application/json"},
            method="POST",
        )
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return json.loads(resp.read().decode("utf-8"))

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
                    schedule_working_set_trim(0.35)
                    return {"ok": True, "hidden": True}
                except Exception:
                    pass
        if self._window:
            try:
                self._window.hide()
            except Exception:
                pass
        schedule_working_set_trim(0.35)
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
            self._api_post("/api/device/stealth", b'{"enabled": true}')
        except Exception:
            pass

    def tray_toggle_audio(self):
        if self._window is not None:
            self._eval_js("if (window.trayToggleAudio) window.trayToggleAudio();")
            return
        try:
            self._api_post("/api/audio/toggle", b"{}")
        except Exception:
            pass

    def tray_clear_copy_cache(self):
        try:
            self._api_post("/api/ocr/clear-chunks", b"{}", timeout=0.8)
        except Exception:
            pass
        self._eval_js("if (window.triggerClearCopyCacheWithFeedback) window.triggerClearCopyCacheWithFeedback();")

    def get_tray_menu_state(self) -> dict[str, bool]:
        """Query live stealth & audio state from isolated server child process when tray menu opens."""
        stealth_on = False
        audio_on = False
        try:
            import json

            req = urllib.request.Request(f"http://{self._host}:{self._port}/api/tray-state")
            with urllib.request.urlopen(req, timeout=0.35) as resp:
                data = json.loads(resp.read().decode("utf-8"))
                stealth_on = bool(data.get("stealth"))
                audio_on = bool(data.get("audio"))
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
        """Wipe in-memory and disk privacy data via isolated server (with local fallback)."""
        try:
            return self._api_post("/api/device/wipe-privacy-data", b"{}", timeout=2.0)
        except Exception:
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


def _is_server_responding(url: str, timeout: float = 2.0) -> bool:
    try:
        req = urllib.request.Request(url, headers={"User-Agent": "iPhoneLink-Launcher"})
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return resp.status == 200
    except Exception:
        return False


def wait_for_server(url: str = "http://127.0.0.1:8765/api/tray-state", timeout: float = 10.0) -> bool:
    start = time.time()
    while time.time() - start < timeout:
        if _is_server_responding(url, timeout=5.0):
            return True
        time.sleep(0.12)
    return False


def _build_server_child_command(host: str, port: int) -> list[str]:
    """Build hidden child-process command line for isolated backend server (Tahap 58)."""
    if getattr(sys, "frozen", False):
        return [sys.executable, "--server-child", "--host", host, "--port", str(port)]
    return [sys.executable, "-m", "handset", "--server-child", "--host", host, "--port", str(port)]


def _ensure_backend_server_running(
    host: str = "127.0.0.1",
    port: int = 8765,
    timeout: float = 10.0,
) -> bool:
    """Launch the backend HTTP server in an isolated child OS process bound to a Win32 Job Object."""
    global _SERVER_PROC, _SERVER_JOB_HANDLE, _SERVER_ENDPOINT
    ready_url = f"http://{host}:{port}/api/tray-state"

    with _SERVER_LOCK:
        if _SERVER_PROC is not None and _SERVER_PROC.poll() is None and _is_server_responding(ready_url, timeout=1.0):
            return True

        if _is_server_responding(ready_url, timeout=0.6):
            return True

        if _SERVER_JOB_HANDLE is None:
            _SERVER_JOB_HANDLE = _create_kill_on_close_job_object()

        cmd = _build_server_child_command(host, port)
        child_env = dict(os.environ)
        child_env["PANE_HOST"] = host
        child_env["PANE_PORT"] = str(port)

        proc = subprocess.Popen(
            cmd,
            cwd=str(get_app_root()),
            env=child_env,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            **_WIN_NO_WINDOW_KWARGS,
        )
        _assign_process_to_job(_SERVER_JOB_HANDLE, proc)
        _SERVER_PROC = proc
        _SERVER_ENDPOINT = (host, port)

    return wait_for_server(url=ready_url, timeout=timeout)


def shutdown_backend_server() -> None:
    """Cleanly stop System Tray icon, shut down isolated child server + audio worker, and close Job Object."""
    global _SERVER_PROC, _SERVER_JOB_HANDLE, _ACTIVE_TRAY_ICON

    tray = _ACTIVE_TRAY_ICON
    _ACTIVE_TRAY_ICON = None
    if tray is not None:
        try:
            tray.stop()
        except Exception:
            pass

    with _SERVER_LOCK:
        proc = _SERVER_PROC
        job_handle = _SERVER_JOB_HANDLE
        host, port = _SERVER_ENDPOINT
        _SERVER_PROC = None
        _SERVER_JOB_HANDLE = None

    if proc is not None and proc.poll() is None:
        try:
            req = urllib.request.Request(
                f"http://{host}:{port}/api/server/shutdown",
                data=b"{}",
                headers={"Content-Type": "application/json"},
                method="POST",
            )
            with urllib.request.urlopen(req, timeout=1.2):
                pass
        except Exception:
            pass

        try:
            proc.wait(timeout=1.5)
        except Exception:
            try:
                proc.terminate()
                proc.wait(timeout=1.0)
            except Exception:
                pass

    # Closing the Job Object handle instructs the Windows kernel to immediately kill any remaining child/audio processes!
    if job_handle is not None:
        _close_job_handle(job_handle)


def _focus_existing_window_if_running(host: str, port: int) -> bool:
    """If an iPhone Link window is already open (or hidden to System Tray), restore & bring to front."""
    if sys.platform != "win32":
        return False
    try:
        import ctypes
        from ctypes import wintypes

        hwnd = ctypes.windll.user32.FindWindowW(None, "iPhone Link")
        if hwnd and _is_server_responding(f"http://{host}:{port}/api/tray-state", timeout=0.5):
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
    """Headless self-test for verifying standalone EXE & portable 2-process isolation."""
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

    app_root = get_app_root()
    transient_names = (
        "wifi_state.json",
        "user_settings.json",
        "typed_history.json",
        "debug_metrics.json",
        ".audio_worker.pid",
    )
    pre_existing = {name for name in transient_names if (app_root / name).exists()}

    child_pid = None
    try:
        ok = _ensure_backend_server_running(host="127.0.0.1", port=free_port, timeout=10.0)
        if not ok:
            return 2
        if _SERVER_PROC is None or _SERVER_PROC.pid == os.getpid():
            return 7
        child_pid = _SERVER_PROC.pid

        req_tray = urllib.request.Request(f"http://127.0.0.1:{free_port}/api/tray-state")
        with urllib.request.urlopen(req_tray, timeout=2.0) as resp_tray:
            tray_data = json.loads(resp_tray.read().decode("utf-8"))
            if not tray_data.get("ok"):
                return 8

        req = urllib.request.Request(f"http://127.0.0.1:{free_port}/api/status")
        with urllib.request.urlopen(req, timeout=8.0) as resp:
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
        if child_pid:
            time.sleep(0.1)
        for name in transient_names:
            if name not in pre_existing:
                try:
                    (app_root / name).unlink(missing_ok=True)
                except Exception:
                    pass


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
    if "--server-child" in sys.argv or "--server-only" in sys.argv:
        srv_host, srv_port = _parse_server_cli_args(sys.argv[1:])
        from handset.server import serve as _serve_backend
        _serve_backend(host=srv_host, port=srv_port)
        return

    if "--smoke-test" in sys.argv:
        raise SystemExit(_run_smoke_test())

    import webview
    from handset.tray import SystemTrayIcon

    host = os.environ.get("PANE_HOST", "127.0.0.1")
    port = int(os.environ.get("PANE_PORT", "8765"))

    if _focus_existing_window_if_running(host, port):
        return

    # Step A: Clean up any stale zombie server / audio processes from crashed sessions
    _cleanup_stale_zombie_processes(host=host, port=port)

    api = WindowApi(host=host, port=port)

    # Set explicit AppUserModelID agar Taskbar Windows 11 menampilkan Logo iPhone Link
    if sys.platform == "win32":
        try:
            import ctypes
            ctypes.windll.shell32.SetCurrentProcessExplicitAppUserModelID("Kelvin.iPhoneLink.Desktop.1")
        except Exception:
            pass

    # Step B & C: Launch backend server in an isolated child OS process bound to Win32 Job Object
    _ensure_backend_server_running(host=host, port=port, timeout=10.0)

    # Jadwalkan penerapan efek DWM sudut membulat dan icon native saat HWND window siap
    threading.Thread(target=apply_dwm_effects, args=("iPhone Link", api), daemon=True).start()

    # Start Native Windows 11 System Tray Icon (Clean Box-Free ASCII Menu)
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

    # One-shot startup working set trim (+5.0s after launch only — NO periodic 45s timer!)
    schedule_working_set_trim(5.0)

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
        shutdown_backend_server()

    # Guarantee 0% lingering .NET CLR / pythonnet background threads after GUI closes
    os._exit(0)


if __name__ == "__main__":
    main()
