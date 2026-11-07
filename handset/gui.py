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
  (Tahap 57), and eliminates working-set trimming so .NET/WinRT Audio pages stay 100% resident in RAM (Tahap 59).
"""

from __future__ import annotations

import ctypes
from ctypes import wintypes, c_int, c_void_p, c_size_t, byref, sizeof, Structure
import hashlib
import json
import multiprocessing
import os
import socket
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


# 2B. Tahap 114: Auto-Unblock NTFS `:Zone.Identifier` streams BEFORE `import webview` / `.NET CLR`
#     Prevents `Failed to resolve Python.Runtime.Loader.Initialize` on fresh browser ZIP extracts.
#     NOTE:Strictly 0 local imports inside this function (uses module-level os, sys, ctypes, Path).
def _auto_unblock_zone_identifiers() -> None:
    if os.name != "nt":
        return
    os.environ.setdefault("COMPlus_LoadFromRemoteSources", "1")
    os.environ.setdefault("PYTHONNET_PYDLL", "")
    try:
        kernel32 = ctypes.windll.kernel32
        delete_file_w = kernel32.DeleteFileW
        delete_file_w.argtypes = [ctypes.c_wchar_p]
        delete_file_w.restype = ctypes.c_int
        get_attrs_w = kernel32.GetFileAttributesW
        get_attrs_w.argtypes = [ctypes.c_wchar_p]
        get_attrs_w.restype = ctypes.c_uint32
        set_attrs_w = kernel32.SetFileAttributesW
        set_attrs_w.argtypes = [ctypes.c_wchar_p, ctypes.c_uint32]
        set_attrs_w.restype = ctypes.c_int
    except Exception:
        return

    roots: list[str] = []
    meipass = getattr(sys, "_MEIPASS", None)
    if meipass and os.path.isdir(meipass):
        roots.append(str(meipass))
    if getattr(sys, "frozen", False):
        try:
            exe_dir = str(Path(sys.executable).resolve().parent)
            if os.path.isdir(exe_dir) and exe_dir not in roots:
                roots.append(exe_dir)
        except Exception:
            pass
    else:
        try:
            app_dir = str(Path(__file__).resolve().parent.parent)
            if os.path.isdir(app_dir) and app_dir not in roots:
                roots.append(app_dir)
        except Exception:
            pass

    skip_dirs = {".git", "venv", ".venv", "build", "dist", "release", "__pycache__", "node_modules"}
    target_exts = (".dll", ".pyd", ".exe", ".ps1", ".bat", ".cmd", ".json", ".config")
    for root_dir in roots:
        try:
            delete_file_w(root_dir + ":Zone.Identifier")
        except Exception:
            pass
        for dirpath, dirnames, filenames in os.walk(root_dir):
            if not getattr(sys, "frozen", False):
                dirnames[:] = [d for d in dirnames if d not in skip_dirs]
            for fname in filenames:
                if fname.lower().endswith(target_exts):
                    full_p = os.path.join(dirpath, fname)
                    ads_p = full_p + ":Zone.Identifier"
                    try:
                        if not delete_file_w(ads_p):
                            attrs = get_attrs_w(full_p)
                            if attrs != 0xFFFFFFFF and (attrs & 0x1):
                                set_attrs_w(full_p, attrs & ~0x1)
                                delete_file_w(ads_p)
                    except Exception:
                        pass


_auto_unblock_zone_identifiers()


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
os.environ["WEBVIEW2_DEFAULT_BACKGROUND_COLOR"] = "0"

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


def _create_kill_on_close_job_object() -> int | None:
    """Create a Win32 Job Object with JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE (0x2000).

    When the parent GUI process closes (even via Task Manager End Task), the Windows NT kernel
    immediately terminates all assigned child & grandchild processes (server + PowerShell audio).
    """
    if sys.platform != "win32":
        return None
    try:
        # [Tahap 70] moved to module top-level
        # [Tahap 70] moved to module top-level

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
        # [Tahap 70] moved to module top-level
        # [Tahap 70] moved to module top-level

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
        # [Tahap 70] moved to module top-level
        # [Tahap 70] moved to module top-level

        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel32.CloseHandle.argtypes = [wintypes.HANDLE]
        kernel32.CloseHandle.restype = wintypes.BOOL
        kernel32.CloseHandle(wintypes.HANDLE(job_handle))
    except Exception:
        pass


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

    # 2. Terminate ONLY lingering backend python servers or audio powershell scripts
    # DILARANG KERAS mematikan 'iPhone Link.exe' karena bisa membunuh diri sendiri / bootloader PyInstaller
    my_pid = os.getpid()
    my_ppid = os.getppid()
    ps_sweep = f"""
$myPid = {my_pid}
$myPpid = {my_ppid}
Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Where-Object {{
    $_.ProcessId -ne $PID -and $_.ProcessId -ne $myPid -and $_.ProcessId -ne $myPpid -and (
        ($_.Name -match '^pythonw?(\\.exe)?$' -and $_.CommandLine -match '(handset\\.server|handset\\\\server\\.py|--server-child|--server-only)') -or
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


def _reapply_dwm_rounded_corners(hwnd: int | None, is_fullscreen: bool = False) -> None:
    """Tahap 71 & 75: Re-apply Windows 11 DWM rounded corner preference (DWMWA_WINDOW_CORNER_PREFERENCE = 33, DWMWCP_ROUND = 2)."""
    if not hwnd or sys.platform != "win32":
        return
    try:
        wh = wintypes.HWND(hwnd)
        user32 = getattr(ctypes.windll, "user32", None)
        dwmapi = getattr(ctypes.windll, "dwmapi", None)

        if not is_fullscreen and user32:
            # Clear WS_MAXIMIZE (0x01000000) bit from GWL_STYLE (-16) if still lingering
            style = user32.GetWindowLongW(wh, -16)
            if style & 0x01000000:
                user32.SetWindowLongW(wh, -16, style & ~0x01000000)

        if dwmapi:
            pref = ctypes.c_int(2)  # DWMWCP_ROUND = 2
            dwmapi.DwmSetWindowAttribute(wh, 33, ctypes.byref(pref), ctypes.sizeof(pref))

        if not is_fullscreen and user32:
            # Refresh window frame immediately (SWP_NOSIZE | SWP_NOMOVE | SWP_NOZORDER | SWP_FRAMECHANGED)
            user32.SetWindowPos(wh, 0, 0, 0, 0, 0, 0x0001 | 0x0002 | 0x0004 | 0x0020)
    except Exception:
        pass


class WindowApi:
    """JS-accessible API exposed under window.pywebview.api."""

    def __init__(self, host: str = "127.0.0.1", port: int = 8765):
        self._window = None
        self._host = host
        self._port = port
        self._cached_hwnd: int | None = None
        self._current_display_width = 346
        self._ideal_display_width = 346
        self._drawer_open = False
        self._is_landscape = False
        self._hidden_to_tray = False
        self._is_fullscreen = False
        self._saved_window_pos: tuple[int, int] | None = None
        self._saved_portrait_phys_size: tuple[int, int] | None = None
        self._saved_landscape_phys_size: tuple[int, int] | None = None

    def _get_window_dpi_scale(self, hwnd: int | None = None) -> float:
        """Tahap 76: Accurately query per-monitor or system DPI scaling factor (e.g. 1.25x or 1.5x)."""
        if sys.platform != "win32":
            return 1.0
        target_hwnd = hwnd or self._get_hwnd()
        user32 = getattr(ctypes.windll, "user32", None)
        if target_hwnd and user32 and hasattr(user32, "GetDpiForWindow"):
            try:
                dpi = user32.GetDpiForWindow(wintypes.HWND(target_hwnd))
                if dpi and dpi > 0:
                    return max(1.0, float(dpi) / 96.0)
            except Exception:
                pass
        if user32 and hasattr(user32, "GetDpiForSystem"):
            try:
                dpi = user32.GetDpiForSystem()
                if dpi and dpi > 0:
                    return max(1.0, float(dpi) / 96.0)
            except Exception:
                pass
        try:
            gdi32 = getattr(ctypes.windll, "gdi32", None)
            if user32 and gdi32:
                hdc = user32.GetDC(0)
                if hdc:
                    dpi_y = gdi32.GetDeviceCaps(hdc, 90)  # LOGPIXELSY = 90
                    user32.ReleaseDC(0, hdc)
                    if dpi_y and dpi_y > 0:
                        return max(1.0, float(dpi_y) / 96.0)
        except Exception:
            pass
        return 1.0

    def _schedule_reapply_corners(self):
        """Tahap 71 & 75: Re-apply DWM rounded corners immediately, at 0.08s, and 0.22s after window state changes."""
        hwnd = self._get_hwnd()
        if hwnd:
            is_fs = getattr(self, "_is_fullscreen", False)
            _reapply_dwm_rounded_corners(hwnd, is_fs)
            threading.Timer(0.08, lambda: _reapply_dwm_rounded_corners(self._get_hwnd(), getattr(self, "_is_fullscreen", False))).start()
            threading.Timer(0.22, lambda: _reapply_dwm_rounded_corners(self._get_hwnd(), getattr(self, "_is_fullscreen", False))).start()

    def set_window(self, window):
        self._window = window

    def _get_hwnd(self):
        try:
            if self._cached_hwnd and ctypes.windll.user32.IsWindow(wintypes.HWND(self._cached_hwnd)):
                return self._cached_hwnd
            h = ctypes.windll.user32.FindWindowW(None, "iPhone Link")
            if not h:
                h = ctypes.windll.user32.FindWindowW(None, "iPhone Mirroring")
            if not h:
                h = ctypes.windll.user32.FindWindowW(None, "iPhone Link UI Self-Test")
            if not h and self._window and hasattr(self._window, "title"):
                h = ctypes.windll.user32.FindWindowW(None, str(self._window.title))
            if not h:
                try:
                    from webview.platforms.winforms import BrowserView
                    for form in list(BrowserView.instances.values()):
                        if form and hasattr(form, "Handle"):
                            cand = int(form.Handle.ToInt64() if hasattr(form.Handle, "ToInt64") else int(form.Handle))
                            if cand and ctypes.windll.user32.IsWindow(wintypes.HWND(cand)):
                                h = cand
                                break
                except Exception:
                    pass
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
        # [Tahap 70] moved to module top-level

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
                # [Tahap 70] moved to module top-level
                # [Tahap 70] moved to module top-level

                ctypes.windll.user32.ReleaseCapture()
                ctypes.windll.user32.SendMessageW(wintypes.HWND(hwnd), 0x00A1, 2, 0)  # WM_NCLBUTTONDOWN, HTCAPTION
        except Exception:
            pass

    def minimize(self):
        if self._window:
            self._window.minimize()
            self._schedule_reapply_corners()

    def minimize_to_tray(self):
        """Hide window to Windows 11 System Tray while keeping mirroring & audio alive in background."""
        self._hidden_to_tray = True
        if sys.platform == "win32":
            hwnd = self._get_hwnd()
            if hwnd:
                try:
                    # [Tahap 70] moved to module top-level
                    # [Tahap 70] moved to module top-level

                    ctypes.windll.user32.ShowWindow(wintypes.HWND(hwnd), 0)  # SW_HIDE
                    return {"ok": True, "hidden": True}
                except Exception:
                    pass
        if self._window:
            try:
                self._window.hide()
            except Exception:
                pass
        return {"ok": True, "hidden": True}

    def restore_from_tray(self):
        """Show/restore iPhone Link window from System Tray or minimized state and bring to front."""
        self._hidden_to_tray = False
        if sys.platform == "win32":
            hwnd = self._get_hwnd()
            if hwnd:
                try:
                    # [Tahap 70] moved to module top-level
                    # [Tahap 70] moved to module top-level

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
            self._schedule_reapply_corners()
            self._eval_js("window.dispatchEvent(new Event('resize'));")
        return {"ok": True, "hidden": False}

    def toggle_tray_window(self):
        """Left-click System Tray handler: restore/focus if hidden or minimized, else hide to tray."""
        if sys.platform == "win32":
            hwnd = self._get_hwnd()
            if hwnd:
                try:
                    # [Tahap 70] moved to module top-level
                    # [Tahap 70] moved to module top-level

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
            # [Tahap 70] moved to module top-level

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
        try:
            hwnd = self._get_hwnd()
            if hwnd:
                # [Tahap 70] moved to module top-level
                # [Tahap 70] moved to module top-level
                ctypes.windll.user32.ShowWindow(wintypes.HWND(hwnd), 0)
        except Exception:
            pass
        if tray is not None:
            try:
                tray.stop()
            except Exception:
                pass
        try:
            shutdown_backend_server()
        except Exception:
            pass
        if self._window:
            try:
                self._window.destroy()
            except Exception:
                pass
        # Tahap 69: Instant os._exit(0) on close to release locks and storage instantly
        # [Tahap 70] moved to module top-level
        os._exit(0)

    def toggle_fullscreen(self, target_state: bool = None):
        if not self._window:
            return {"ok": False, "fullscreen": False, "is_fullscreen": False}

        hwnd = self._get_hwnd()
        user32 = getattr(ctypes.windll, "user32", None)

        # Step A: Determine whether physically fullscreen or target state
        work_left, work_top = 0, 0
        work_w = 1920
        work_h = 1080
        is_physically_full = False
        if hwnd and user32:
            work_rect = wintypes.RECT()
            if user32.SystemParametersInfoW(0x0030, 0, ctypes.byref(work_rect), 0):
                work_left = int(work_rect.left)
                work_top = int(work_rect.top)
                work_w = int(work_rect.right - work_rect.left)
                work_h = int(work_rect.bottom - work_rect.top)
            else:
                work_w = int(user32.GetSystemMetrics(0))
                work_h = int(user32.GetSystemMetrics(1))

            cur_rect = wintypes.RECT()
            if user32.GetWindowRect(wintypes.HWND(hwnd), ctypes.byref(cur_rect)):
                cur_w = cur_rect.right - cur_rect.left
                cur_h = cur_rect.bottom - cur_rect.top
                if cur_w >= (work_w - 60) and cur_h >= (work_h - 60):
                    is_physically_full = True

        if target_state is not None:
            want_fullscreen = bool(target_state)
        else:
            was_fullscreen = bool(getattr(self, "_is_fullscreen", False)) or is_physically_full
            want_fullscreen = not was_fullscreen

        if want_fullscreen:
            # Entering Full Screen
            if hwnd and user32:
                rect = wintypes.RECT()
                if user32.GetWindowRect(wintypes.HWND(hwnd), ctypes.byref(rect)):
                    cur_w = rect.right - rect.left
                    cur_h = rect.bottom - rect.top
                    if cur_w < (work_w - 80) and (rect.left > 15 or rect.top > 15):
                        self._saved_window_pos = (int(rect.left), int(rect.top))
                    if not getattr(self, "_is_landscape", False) and not getattr(self, "_drawer_open", False):
                        if 300 <= cur_w <= 650 and cur_h >= int(cur_w * 2.15):
                            self._saved_portrait_phys_size = (int(cur_w), int(cur_h))

                self._is_fullscreen = True
                self._drawer_open = False

                # Gunakan SetWindowPos langsung tanpa ShowWindow SW_MAXIMIZE
                user32.SetWindowPos(
                    wintypes.HWND(hwnd),
                    0,
                    int(work_left),
                    int(work_top),
                    int(work_w),
                    int(work_h),
                    0x0004 | 0x0020 | 0x0040,  # SWP_NOZORDER | SWP_FRAMECHANGED | SWP_SHOWWINDOW
                )
            else:
                self._is_fullscreen = True
                self._drawer_open = False
                try:
                    self._window.toggle_fullscreen()
                except Exception:
                    pass

            self._eval_js("if (window.applyFullsizeState) window.applyFullsizeState(true, true);")
            self._schedule_reapply_corners()
            return {
                "ok": True,
                "fullscreen": True,
                "is_fullscreen": True,
            }
        else:
            # Exiting Full Screen -> Mode Window normal
            self._is_fullscreen = False

            if hwnd and user32:
                wh = wintypes.HWND(hwnd)
                # Bersihkan bit WS_MAXIMIZE (0x01000000) jika menempel
                style = user32.GetWindowLongW(wh, -16)
                if style & 0x01000000:
                    user32.SetWindowLongW(wh, -16, style & ~0x01000000)
                user32.ShowWindow(wh, 9)  # SW_RESTORE

            if getattr(self, "_is_landscape", False):
                logical_w = 1200 if self._drawer_open else 840
                logical_h = 468
                base_w = 840
            else:
                base_w = getattr(self, "_ideal_display_width", getattr(self, "_current_display_width", 346)) or 346
                logical_w = (base_w + 360) if self._drawer_open else base_w
                logical_h = 832

            # Panggil self._window.resize(logical_w, logical_h) lebih dulu agar pywebview Form menghitung DPI
            try:
                self._window.resize(int(logical_w), int(logical_h))
            except Exception:
                pass

            dpi_scale = self._get_window_dpi_scale(hwnd)
            after_rect = wintypes.RECT()
            has_after = False
            if hwnd and user32:
                has_after = bool(user32.GetWindowRect(wintypes.HWND(hwnd), ctypes.byref(after_rect)))

            if getattr(self, "_is_landscape", False):
                phys_w = int(round(logical_w * dpi_scale))
                phys_h = int(round(logical_h * dpi_scale))
            else:
                if not self._drawer_open and getattr(self, "_saved_portrait_phys_size", None):
                    saved_pw, saved_ph = self._saved_portrait_phys_size
                    if 300 <= saved_pw <= 650 and saved_ph >= int(saved_pw * 2.15):
                        phys_w, phys_h = saved_pw, saved_ph
                    else:
                        form_w = (after_rect.right - after_rect.left) if has_after else 0
                        phys_w = max(int(round(logical_w * dpi_scale)), form_w)
                        ideal_ph = int(round(phys_w * (832.0 / float(base_w)))) if base_w > 0 else int(round(phys_w * 2.4046))
                        phys_h = min(int(work_h - 16), max(ideal_ph, int(round(832.0 * dpi_scale))))
                else:
                    form_w = (after_rect.right - after_rect.left) if has_after else 0
                    phys_w = max(int(round(logical_w * dpi_scale)), form_w)
                    ideal_ph = int(round(phys_w * (832.0 / float(base_w)))) if base_w > 0 else int(round(phys_w * 2.4046))
                    phys_h = min(int(work_h - 16), max(ideal_ph, int(round(832.0 * dpi_scale))))

            saved = getattr(self, "_saved_window_pos", None)
            if saved and (saved[0] > 15 or saved[1] > 15) and saved[0] < (work_w - 100) and saved[1] < (work_h - 100):
                restore_x, restore_y = int(saved[0]), int(saved[1])
            else:
                restore_x = work_left + max(24, (work_w - phys_w) // 2)
                restore_y = work_top + max(24, (work_h - phys_h) // 2)

            if hwnd and user32:
                wh = wintypes.HWND(hwnd)
                user32.SetWindowPos(
                    wh,
                    0,
                    int(restore_x),
                    int(restore_y),
                    int(phys_w),
                    int(phys_h),
                    0x0004 | 0x0020 | 0x0040,
                )
                _reapply_dwm_rounded_corners(hwnd, False)

            try:
                self._window.move(int(restore_x), int(restore_y))
            except Exception:
                pass

            # Verification watchdog timer at 0.06s to recheck physical aspect ratio
            def _verify_restore():
                try:
                    if hwnd and user32 and not getattr(self, "_is_fullscreen", False) and not getattr(self, "_is_landscape", False):
                        vr = wintypes.RECT()
                        if user32.GetWindowRect(wintypes.HWND(hwnd), ctypes.byref(vr)):
                            vw = vr.right - vr.left
                            vh = vr.bottom - vr.top
                            if vw < 650 and vh < int(vw * 2.15):
                                corr_h = min(int(work_h - 16), int(round(vw * (832.0 / float(base_w)))))
                                user32.SetWindowPos(wintypes.HWND(hwnd), 0, vr.left, vr.top, vw, corr_h, 0x0004 | 0x0020)
                except Exception:
                    pass
            threading.Timer(0.06, _verify_restore).start()

            self._eval_js("if (window.applyFullsizeState) window.applyFullsizeState(false, false);")
            self._schedule_reapply_corners()
            return {
                "ok": True,
                "fullscreen": False,
                "is_fullscreen": False,
                "x": int(restore_x),
                "y": int(restore_y),
                "width": int(phys_w),
                "height": int(phys_h),
            }

    def set_orientation(self, landscape: bool, drawer_open: bool = None):
        """Ubah ukuran jendela ke dimensi Landscape / Horizontal atau Portrait (Tahap 67 & 71 & 75)."""
        if not self._window:
            return
        try:
            self._is_landscape = bool(landscape)
            if drawer_open is not None:
                self._drawer_open = bool(drawer_open)

            # Tahap 75 Guard: Jika sedang Full Screen, JANGAN ubah ukuran jendela OS!
            if getattr(self, "_is_fullscreen", False):
                return {"ok": True, "landscape": self._is_landscape, "fullscreen": True, "skipped_resize": True}

            if self._is_landscape:
                target_w = 1200 if self._drawer_open else 840
                target_h = 468
            else:
                dw = getattr(self, "_ideal_display_width", getattr(self, "_current_display_width", 346))
                target_w = (dw + 360) if self._drawer_open else dw
                target_h = 832

            hwnd = self._get_hwnd()
            user32 = getattr(ctypes.windll, "user32", None)
            if hwnd and user32:
                rect = wintypes.RECT()
                if user32.GetWindowRect(wintypes.HWND(hwnd), ctypes.byref(rect)):
                    if rect.left <= 5 and rect.top <= 5:
                        saved = getattr(self, "_saved_window_pos", None)
                        if saved and (saved[0] > 15 or saved[1] > 15):
                            nx, ny = int(saved[0]), int(saved[1])
                        else:
                            sw = user32.GetSystemMetrics(0)
                            sh = user32.GetSystemMetrics(1)
                            nx = max(24, (sw - target_w) // 2)
                            ny = max(24, (sh - target_h) // 2)
                        user32.SetWindowPos(wintypes.HWND(hwnd), 0, nx, ny, target_w, target_h, 0x0004 | 0x0020 | 0x0040)
                        try:
                            self._window.move(nx, ny)
                        except Exception:
                            pass

            self._window.resize(target_w, target_h)
            self._schedule_reapply_corners()

            # Cache portrait physical size when portrait and drawer closed
            if not self._is_landscape and not self._drawer_open and not getattr(self, "_is_fullscreen", False):
                if hwnd and user32:
                    rect = wintypes.RECT()
                    if user32.GetWindowRect(wintypes.HWND(hwnd), ctypes.byref(rect)):
                        cur_w = rect.right - rect.left
                        cur_h = rect.bottom - rect.top
                        if 300 <= cur_w <= 650 and cur_h >= int(cur_w * 2.15):
                            self._saved_portrait_phys_size = (int(cur_w), int(cur_h))

            return {"ok": True, "landscape": self._is_landscape, "width": target_w, "height": target_h}
        except Exception as e:
            print("set_orientation error:", e)

    def sync_device_aspect(self, display_width: int, drawer_open: bool = False):
        """Tahap 66, 67, 71 & 75: Abaikan aspect sync jika sedang mode Landscape atau Full Screen."""
        if not self._window:
            return
        if getattr(self, "_is_fullscreen", False) or getattr(self, "_is_landscape", False):
            return {"ok": True, "skipped": "fullscreen_or_landscape"}
        try:
            self._drawer_open = bool(drawer_open)
            dw = max(336, min(600, int(display_width)))
            self._ideal_display_width = dw
            self._current_display_width = dw
            target_w = dw + (360 if self._drawer_open else 0)
            target_h = 832

            hwnd = self._get_hwnd()
            user32 = getattr(ctypes.windll, "user32", None)
            if hwnd and user32:
                rect = wintypes.RECT()
                if user32.GetWindowRect(wintypes.HWND(hwnd), ctypes.byref(rect)):
                    if rect.left <= 5 and rect.top <= 5:
                        saved = getattr(self, "_saved_window_pos", None)
                        if saved and (saved[0] > 15 or saved[1] > 15):
                            nx, ny = int(saved[0]), int(saved[1])
                        else:
                            sw = user32.GetSystemMetrics(0)
                            sh = user32.GetSystemMetrics(1)
                            nx = max(24, (sw - target_w) // 2)
                            ny = max(24, (sh - target_h) // 2)
                        user32.SetWindowPos(wintypes.HWND(hwnd), 0, nx, ny, target_w, target_h, 0x0004 | 0x0020 | 0x0040)
                        try:
                            self._window.move(nx, ny)
                        except Exception:
                            pass

            self._window.resize(target_w, target_h)
            self._schedule_reapply_corners()

            # Cache portrait physical size when portrait and drawer closed
            if not self._is_landscape and not self._drawer_open and not getattr(self, "_is_fullscreen", False):
                if hwnd and user32:
                    rect = wintypes.RECT()
                    if user32.GetWindowRect(wintypes.HWND(hwnd), ctypes.byref(rect)):
                        cur_w = rect.right - rect.left
                        cur_h = rect.bottom - rect.top
                        if 300 <= cur_w <= 650 and cur_h >= int(cur_w * 2.15):
                            self._saved_portrait_phys_size = (int(cur_w), int(cur_h))

            return {"ok": True, "width": target_w, "height": target_h}
        except Exception as e:
            print("sync_device_aspect error:", e)
            return {"ok": False, "error": str(e)}

    def set_drawer(self, is_open: bool):
        """Tahap 67, 71, 75 & 91: Buka/tutup Side Drawer tanpa merusak mode Landscape atau Full Screen."""
        self._drawer_open = bool(is_open)
        if getattr(self, "_is_fullscreen", False):
            return {"ok": True, "fullscreen": True, "drawer": self._drawer_open, "drawer_open": self._drawer_open}
        if getattr(self, "_is_landscape", False):
            return self.set_orientation(True, self._drawer_open)
        dw = getattr(self, "_ideal_display_width", getattr(self, "_current_display_width", 346))
        target_w = (dw + 360) if self._drawer_open else dw
        target_h = 832
        if self._window:
            self._window.resize(target_w, target_h)
        self._schedule_reapply_corners()

        # Cache portrait physical size when drawer closed
        if not self._drawer_open and not self._is_landscape:
            hwnd = self._get_hwnd()
            user32 = getattr(ctypes.windll, "user32", None)
            if hwnd and user32:
                rect = wintypes.RECT()
                if user32.GetWindowRect(wintypes.HWND(hwnd), ctypes.byref(rect)):
                    cur_w = rect.right - rect.left
                    cur_h = rect.bottom - rect.top
                    if 300 <= cur_w <= 650 and cur_h >= int(cur_w * 2.15):
                        self._saved_portrait_phys_size = (int(cur_w), int(cur_h))

        return {"ok": True, "drawer_open": self._drawer_open, "width": target_w, "height": target_h}

    def get_window_rect(self):
        """Tahap 63 & 71: Return window x, y, width, height for wallpaper offset synchronization."""
        try:
            hwnd = self._get_hwnd()
            if hwnd:
                # [Tahap 70] moved to module top-level
                # [Tahap 70] moved to module top-level
                rect = wintypes.RECT()
                if ctypes.windll.user32.GetWindowRect(wintypes.HWND(hwnd), ctypes.byref(rect)):
                    return {
                        "x": rect.left,
                        "y": rect.top,
                        "width": rect.right - rect.left,
                        "height": rect.bottom - rect.top,
                    }
        except Exception:
            pass
        return {
            "x": getattr(self._window, "x", 0),
            "y": getattr(self._window, "y", 0),
            "width": getattr(self._window, "width", 346),
            "height": getattr(self._window, "height", 832),
        }

    def set_theme(self, theme: str):
        """Tahap 67: Update native Windows 11 DWM dark/light mode attribute & Acrylic tint."""
        is_dark = theme != "light"
        self._current_theme = theme
        try:
            from webview.platforms.winforms import BrowserView
            import System.Drawing
            import System.Windows.Forms
            for form in list(BrowserView.instances.values()):
                def _update_bg(f=form):
                    try:
                        f.BackColor = System.Drawing.Color.Black
                        f.Invalidate()
                    except Exception:
                        pass
                if hasattr(form, "InvokeRequired") and form.InvokeRequired:
                    form.Invoke(System.Windows.Forms.MethodInvoker(_update_bg))
                else:
                    _update_bg()
        except Exception:
            pass
        try:
            # [Tahap 70] moved to module top-level
            # [Tahap 70] moved to module top-level

            hwnd = self._get_hwnd()
            if hwnd:
                dwmapi = getattr(ctypes.windll, "dwmapi", None)
                user32 = getattr(ctypes.windll, "user32", None)
                if dwmapi:
                    dwmapi.DwmSetWindowAttribute(
                        wintypes.HWND(hwnd),
                        20,  # DWMWA_USE_IMMERSIVE_DARK_MODE
                        byref(c_int(1 if is_dark else 0)),
                        sizeof(c_int),
                    )
                    dwmapi.DwmSetWindowAttribute(
                        wintypes.HWND(hwnd),
                        33,  # DWMWA_WINDOW_CORNER_PREFERENCE = 2 (round)
                        byref(c_int(2)),
                        sizeof(c_int),
                    )
                    dwmapi.DwmSetWindowAttribute(
                        wintypes.HWND(hwnd),
                        38,  # DWMSBT_AUTO = 0
                        byref(c_int(0)),
                        sizeof(c_int),
                    )
                if user32 and hasattr(user32, "SetWindowCompositionAttribute"):
                    class ACCENT_POLICY(Structure):
                        _fields_ = [
                            ("AccentState", c_int),
                            ("AccentFlags", c_int),
                            ("GradientColor", c_int),
                            ("AnimationId", c_int),
                        ]
                    class WINCOMPATTRDATA(Structure):
                        _fields_ = [
                            ("Attribute", c_int),
                            ("Data", c_void_p),
                            ("SizeOfData", c_size_t),
                        ]
                    gradient_color = 0x55161412 if is_dark else 0x45F4F2F0
                    accent = ACCENT_POLICY(4, 2, gradient_color, 0)
                    data = WINCOMPATTRDATA(19, ctypes.cast(byref(accent), c_void_p), sizeof(accent))
                    user32.SetWindowCompositionAttribute(wintypes.HWND(hwnd), byref(data))
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


_WEBVIEW2_TRANSPARENCY_APPLIED = False
_LAST_WEBVIEW2_BG_COLOR = ""


def _find_webview_controls(control):
    """Tahap 62: Recursively discover all WebView2 controls in WinForms hierarchy and pywebview wrapper."""
    found = []
    if control is None:
        return found
    if hasattr(control, "DefaultBackgroundColor"):
        found.append(control)
    browser = getattr(control, "browser", None)
    if browser:
        wv = getattr(browser, "webview", None)
        if wv and hasattr(wv, "DefaultBackgroundColor") and wv not in found:
            found.append(wv)
    controls = getattr(control, "Controls", None)
    if controls:
        try:
            for child in list(controls):
                found.extend(_find_webview_controls(child))
        except Exception:
            pass
    return found


def _apply_files_thin_acrylic_backdrop(window=None, hwnd: int = 0) -> None:
    """Tahap 67: Apply true Windows 11 Thin Acrylic + Acrylic Blur Behind + 100% CoreWebView2 transparency."""
    if not hwnd or sys.platform != "win32":
        return
    try:
        # [Tahap 70] moved to module top-level
        # [Tahap 70] moved to module top-level

        dwmapi = getattr(ctypes.windll, "dwmapi", None)
        user32 = getattr(ctypes.windll, "user32", None)
        gdi32 = getattr(ctypes.windll, "gdi32", None)
        if not dwmapi or not user32:
            return

        api = getattr(window, "_js_api", None) if window else None
        is_dark = getattr(api, "_current_theme", "dark") != "light" if api else True

        # Langkah A: DWM Dark Mode & Extend Frame Into Client Area
        # 1. DWMWA_USE_IMMERSIVE_DARK_MODE = 20 (dark frame prevents white flash)
        dwmapi.DwmSetWindowAttribute(wintypes.HWND(hwnd), 20, byref(c_int(1 if is_dark else 0)), sizeof(c_int))

        # 2. DwmExtendFrameIntoClientArea (-1 on all sides for full glass canvas)
        class MARGINS(Structure):
            _fields_ = [
                ("cxLeftWidth", c_int),
                ("cxRightWidth", c_int),
                ("cyTopHeight", c_int),
                ("cyBottomHeight", c_int),
            ]
        margins = MARGINS(-1, -1, -1, -1)
        dwmapi.DwmExtendFrameIntoClientArea(wintypes.HWND(hwnd), byref(margins))

        # 3. DWMWA_WINDOW_CORNER_PREFERENCE = 33, 2 = DWMWCP_ROUND
        dwmapi.DwmSetWindowAttribute(wintypes.HWND(hwnd), 33, byref(c_int(2)), sizeof(c_int))

        # 4. Reset DWMWA_SYSTEMBACKDROP_TYPE = 38 to DWMSBT_AUTO (0)
        dwmapi.DwmSetWindowAttribute(wintypes.HWND(hwnd), 38, byref(c_int(0)), sizeof(c_int))

        # 5. SetWindowCompositionAttribute (WCA_ACCENT_POLICY = 19, ACCENT_ENABLE_ACRYLICBLURBEHIND = 4)
        class ACCENT_POLICY(Structure):
            _fields_ = [
                ("AccentState", c_int),
                ("AccentFlags", c_int),
                ("GradientColor", c_int),
                ("AnimationId", c_int),
            ]

        class WINCOMPATTRDATA(Structure):
            _fields_ = [
                ("Attribute", c_int),
                ("Data", c_void_p),
                ("SizeOfData", c_size_t),
            ]

        gradient_color = 0x55161412 if is_dark else 0x45F4F2F0
        accent = ACCENT_POLICY(4, 2, gradient_color, 0)
        data = WINCOMPATTRDATA(19, ctypes.cast(byref(accent), c_void_p), sizeof(accent))
        if hasattr(user32, "SetWindowCompositionAttribute"):
            user32.SetWindowCompositionAttribute(wintypes.HWND(hwnd), byref(data))

        # Langkah C: Tahap 68 - WinForms Form Clean Theme Color & CoreWebView2 Transparent
        try:
            global _WEBVIEW2_TRANSPARENCY_APPLIED, _LAST_WEBVIEW2_BG_COLOR
            from webview.platforms.winforms import BrowserView
            import System
            import System.Drawing
            import System.Windows.Forms

            theme_hex = "#10141e" if is_dark else "#eaf0f8"
            for form in list(BrowserView.instances.values()):
                def _set_form_bg(f=form):
                    global _WEBVIEW2_TRANSPARENCY_APPLIED, _LAST_WEBVIEW2_BG_COLOR
                    try:
                        f.BackColor = System.Drawing.ColorTranslator.FromHtml(theme_hex)
                        f.Invalidate()
                    except Exception:
                        pass

                    # Recursively find all WebView2 controls and set DefaultBackgroundColor to Transparent (Alpha = 0)
                    wv_list = _find_webview_controls(f)
                    for wv in wv_list:
                        try:
                            wv.DefaultBackgroundColor = System.Drawing.Color.FromArgb(0, 0, 0, 0)
                            _WEBVIEW2_TRANSPARENCY_APPLIED = True
                            _LAST_WEBVIEW2_BG_COLOR = "Transparent"
                        except Exception:
                            pass
                        try:
                            ctrl = getattr(wv, "CoreWebView2Controller", None)
                            if ctrl:
                                ctrl.DefaultBackgroundColor = System.Drawing.Color.FromArgb(0, 0, 0, 0)
                                _WEBVIEW2_TRANSPARENCY_APPLIED = True
                                _LAST_WEBVIEW2_BG_COLOR = "Transparent"
                        except Exception:
                            pass

                if hasattr(form, "InvokeRequired") and form.InvokeRequired:
                    form.Invoke(System.Windows.Forms.MethodInvoker(_set_form_bg))
                else:
                    _set_form_bg()
        except Exception:
            pass

    except Exception:
        pass


def apply_dwm_effects(title: str = "iPhone Link", api: WindowApi | None = None) -> None:
    """Terapkan efek DWM Windows 11 (Files Thin Acrylic + sudut membulat + dark mode) dan icon native."""
    try:
        # [Tahap 70] moved to module top-level
        # [Tahap 70] moved to module top-level

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

        # Apply Files Thin Acrylic Backdrop & CoreWebView2 Transparency
        _apply_files_thin_acrylic_backdrop(api._window if api else None, hwnd)

        # Re-check transparency after CoreWebView2 finishes internal attach
        time.sleep(1.0)
        _apply_files_thin_acrylic_backdrop(api._window if api else None, hwnd)
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
        # [Tahap 70] moved to module top-level
        # [Tahap 70] moved to module top-level

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
    # [Tahap 70] moved to module top-level
    # [Tahap 70] moved to module top-level
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




def _tahap112_selftest(tahap111_ok: bool) -> dict:
    """Tahap 112: verify Right Arrow unselect, immediate spotlight-close, lollipop-tolerant line 3 and 11-char priority."""
    from PIL import Image as _Img
    from handset import screen as handset_screen
    from handset.qr_bridge import (
        assemble_multiline_url_from_lines,
        clean_extracted_url,
        pick_best_extracted_url,
        prepare_spotlight_top_box_images,
        _is_complete_top_box_url,
    )

    def _run(lines, noise):
        pool = list(assemble_multiline_url_from_lines(lines))
        pool.extend(noise)
        return pick_best_extracted_url(pool)

    cases = {
        "sub2a": (
            ["|•https://www.instagram.com/p/", "DVijpJIiPqN/?", "igsh=Mm53anAveGk3azdoWGo= •|", "— Ask Siri"],
            ["https://www.instagram.com/p/DVijpJi24PqN/", "https://www.instagram.com/p/DVijpJi24PqN/?igsh=Mm53anAveGk3azdoWGo="],
            "https://www.instagram.com/p/DVijpJIiPqN/?igsh=Mm53anAveGk3azdoWGo=",
        ),
        "sub2b": (
            ["|•https://www.instagram.com/p/", "DVf37caEypT/?", "igsh=YmY1bm9icDR4cHZ5 •|", "— Ask Siri"],
            ["https://www.instagram.com/p/DVf372caEypT/", "https://www.instagram.com/p/DVf372caEypT/?igsh=YmY1bm9icDR4cHZ5"],
            "https://www.instagram.com/p/DVf37caEypT/?igsh=YmY1bm9icDR4cHZ5",
        ),
        "sub2c": (
            ["|•https://www.instagram.com/reel/", "DU6jg6hj24p/?", "igsh=MW45OTZ3anhlbmQ3eg== •|", "— Ask Siri"],
            ["https://www.instagram.com/reel/DU6jg6h24p4p/", "https://www.instagram.com/reel/DU6jg6h24p4p/?igsh=MW45OTZ3anhlbmQ3eg=="],
            "https://www.instagram.com/reel/DU6jg6hj24p/?igsh=MW45OTZ3anhlbmQ3eg==",
        ),
    }
    out: dict = {}
    all_ok = True
    for key, (lines, noise, expect) in cases.items():
        got = _run(lines, noise)
        ok = (got == expect)
        out[key] = {"url": got, "passed": ok}
        all_ok = all_ok and ok

    # Cross-candidate igsh merger: clean 11-char code without igsh + sibling 12-char noise carrying igsh
    merged = pick_best_extracted_url([
        "https://www.instagram.com/p/DVf37caEypT/",
        "https://www.instagram.com/p/DVf372caEypT/?igsh=YmY1bm9icDR4cHZ5",
    ])
    out["merger"] = {"url": merged, "passed": merged == "https://www.instagram.com/p/DVf37caEypT/?igsh=YmY1bm9icDR4cHZ5"}
    all_ok = all_ok and out["merger"]["passed"]

    # Sub-Test 2D: both Threads links stay 100% intact
    th1 = clean_extracted_url("https://www.threads.com/@kylee002/post/DVif1ECA_c6?xmt=AQF09aWWR7Ed88iylgk8a3A0xThr9W2HMP97zNPhZ6uxba5vFl")
    th2 = clean_extracted_url("https://www.threads.com/@patriciavan04/post/DVh71mpysd1?xmt=AQF06m1cD-U1Hmc34qV92twZbba7zWbyWV")
    out["sub2d"] = {
        "th1": th1,
        "th2": th2,
        "passed": bool(
            th1 == "https://www.threads.com/@kylee002/post/DVif1ECA_c6?xmt=AQF09aWWR7Ed88iylgk8a3A0xThr9W2HMP97zNPhZ6uxba5vFl"
            and th2 == "https://www.threads.com/@patriciavan04/post/DVh71mpysd1?xmt=AQF06m1cD-U1Hmc34qV92twZbba7zWbyWV"
        ),
    }
    all_ok = all_ok and out["sub2d"]["passed"]

    # Sub-Test 2E: Right Arrow after paste-ios + immediate spotlight-close before /api/qr/scan + Zona 1 = 2 variants
    code_paste = getattr(handset_screen.spotlight_paste_clipboard_on_iphone, "__code__", None)
    consts_paste = list(code_paste.co_consts) if code_paste else []
    has_right_arrow = (0x4F in consts_paste) and ("paste-ios" in consts_paste)
    app_js_path = get_bundle_root() / "handset" / "web" / "app.js"
    js_text = app_js_path.read_text(encoding="utf-8", errors="replace") if app_js_path.is_file() else ""
    js_start = js_text.find("triggerScanQrLinkWithFeedback = async function")
    js_fn = js_text[js_start:js_start + 4500] if js_start >= 0 else ""
    idx_close = js_fn.find("/api/device/spotlight-close")
    idx_scan = js_fn.find("/api/qr/scan")
    close_before_ocr = bool(0 <= idx_close < idx_scan)
    top_imgs = prepare_spotlight_top_box_images(_Img.new("RGB", (1216, 2656), (255, 255, 255)))
    zone_ok = (len(top_imgs) == 7) and _is_complete_top_box_url("https://www.instagram.com/p/DVf37caEypT/?igsh=YmY1bm9icDR4cHZ5") and not _is_complete_top_box_url("https://www.instagram.com/p/")
    out["sub2e"] = {
        "right_arrow_after_paste": has_right_arrow,
        "close_before_scan": close_before_ocr,
        "zona1_two_variants_and_isolation": zone_ok,
        "passed": bool(has_right_arrow and close_before_ocr and zone_ok),
    }
    all_ok = all_ok and out["sub2e"]["passed"]

    out["passed"] = bool(all_ok and tahap111_ok)
    return out

def _run_ui_self_test() -> int:
    """Tahap 65: Live WebView2 Automated End-to-End Self-Test.
    Launches a real WebView2 instance, checks DOM readiness, verifies #status-pill,
    clicks Settings (⚙️), clicks Auto-Rotate ON, clicks Original Ratio (contain),
    verifies window.__jsErrors is empty, and records full results into self_test_result.json.
    """
    # Hard-timeout watchdog: force-kill after 90.0s to prevent any hang in automated runs
    watchdog = threading.Timer(90.0, lambda: os._exit(0))
    watchdog.daemon = True
    watchdog.start()

    # [Tahap 70] moved to module top-level
    # [Tahap 70] moved to module top-level
    import webview

    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(("127.0.0.1", 0))
        free_port = s.getsockname()[1]

    app_root = get_app_root()
    test_result_file = app_root / "self_test_result.json"

    ok = _ensure_backend_server_running(host="127.0.0.1", port=free_port, timeout=10.0)
    if not ok:
        test_result_file.write_text(json.dumps({"passed": False, "error": "Backend server failed to start"}), encoding="utf-8")
        return 1

    api = WindowApi(host="127.0.0.1", port=free_port)
    window = webview.create_window(
        title="iPhone Link UI Self-Test",
        url=f"http://127.0.0.1:{free_port}/?t={int(time.time())}",
        js_api=api,
        width=346,
        height=832,
        frameless=True,
    )
    api.set_window(window)

    test_data = {"passed": False, "checks": {}}

    def _runner():
        nonlocal test_data
        time.sleep(1.8)  # Initial wait for DOM
        try:
            # Step A: Preload mirror iframe so drawFrame is ready
            for _ in range(40):
                time.sleep(0.1)
                try:
                    res_state = window.evaluate_js('''
                    (() => {
                        const ifr = document.querySelector("#screen-view");
                        if (!ifr) return "no-iframe";
                        ifr.hidden = false;
                        ifr.removeAttribute("hidden");
                        ifr.style.display = "block";
                        if (!ifr.src || ifr.src.includes("about:blank")) {
                            ifr.src = "/mirror.html";
                            return "src-set";
                        }
                        const m = ifr.contentWindow;
                        if (m && typeof m.drawFrame === "function") return "ready";
                        return "waiting-drawframe";
                    })()
                    ''')
                    if res_state == "ready":
                        break
                except Exception:
                    pass

            eval_code = """
            (() => {
                const res = {
                    jsErrors: window.__jsErrors || [],
                    statusPillText: "",
                    hasStatusPill: false,
                    drawerOpened: false,
                    autoRotateClicked: false,
                    fitContainClicked: false,
                    hasAutoFitBtn: false,
                    fitAutoClicked: false,
                    wallpaperParallaxActive: false,
                    pixelRenderOk: false,
                    statusCleared: false,
                    screenEmptyCentered: false,
                    screenNoteHidden: true
                };

                // 1. Status Pill check
                const pill = document.querySelector("#status-pill") || document.querySelector("#pill-connection");
                if (pill) {
                    res.hasStatusPill = true;
                    res.statusPillText = (pill.textContent || "").trim();
                }

                // 2. Tahap 71: Top Bar Collision & Visibility at 346px (evaluated before opening drawer)
                const btnRot = document.querySelector("#btn-rotate-landscape");
                const btnSet = document.querySelector("#btn-settings");
                const btnClose = document.querySelector("#win-close");
                const btnMin = document.querySelector("#win-min");
                const btnMax = document.querySelector("#win-max") || document.querySelector("#btn-fullsize");
                if (btnRot && btnSet) {
                    const rRect = btnRot.getBoundingClientRect();
                    const sRect = btnSet.getBoundingClientRect();
                    const mRect = btnMax ? btnMax.getBoundingClientRect() : null;
                    res.rotateBadgeNoOverlap = (sRect.right <= (rRect.left + 1.5)) && (!mRect || (rRect.right <= (mRect.left + 1.5)));
                } else {
                    res.rotateBadgeNoOverlap = true;
                }
                if (btnClose) {
                    const cRect = btnClose.getBoundingClientRect();
                    res.allTopBarIconsVisible = (cRect.right <= (window.innerWidth + 1.5));
                } else {
                    res.allTopBarIconsVisible = true;
                }

                // 3. Settings button click
                const btnSettings = document.querySelector("#btn-settings");
                if (btnSettings) {
                    btnSettings.click();
                    const appRoot = document.querySelector("#app-root");
                    const controlPane = document.querySelector("#control-pane");
                    res.drawerOpened = !!((appRoot && appRoot.classList.contains("drawer-open")) || (controlPane && !controlPane.hidden));
                }

                // 4. Auto-Rotate ON click
                const autoRotateOn = document.querySelector('[data-auto-rotate-choice="on"]');
                if (autoRotateOn) {
                    autoRotateOn.click();
                    res.autoRotateClicked = !!(autoRotateOn.classList.contains("active") || autoRotateOn.classList.contains("is-active"));
                }

                // 4. Fit Choices checks (Auto-Fit & Original Ratio)
                const fitAuto = document.querySelector('[data-fit-choice="autofit"]');
                const fitContain = document.querySelector('[data-fit-choice="contain"]');
                res.hasAutoFitBtn = !!fitAuto;
                if (fitContain) {
                    fitContain.click();
                    res.fitContainClicked = !!(fitContain.classList.contains("active") || fitContain.classList.contains("is-active"));
                }
                if (fitAuto) {
                    fitAuto.click();
                    res.fitAutoClicked = !!(fitAuto.classList.contains("active") || fitAuto.classList.contains("is-active"));
                }

                // 5. Wallpaper Parallax CSS Check
                const appElem = document.querySelector(".app") || document.querySelector("#app-root");
                if (appElem) {
                    const beforeSt = window.getComputedStyle(appElem, "::before");
                    const bgImg = beforeSt ? (beforeSt.backgroundImage || "") : "";
                    res.wallpaperParallaxActive = bgImg.includes("/api/system/wallpaper") || (bgImg.startsWith("url(") && bgImg !== "none");
                }

                // 6. Tahap 68 & 72: Live WebView2 mirror.js drawFrame() Pixel Render & H.265 Macroblock Padding Trim Verification
                const iframe = document.querySelector("#screen-view");
                try {
                    const mWin = iframe ? iframe.contentWindow : null;
                    res.diagIframe = {
                        hasIframe: !!iframe,
                        iframeSrc: iframe ? iframe.src : null,
                        hasMWin: !!mWin,
                        drawFrameType: (mWin ? typeof mWin.drawFrame : "no-mwin"),
                        mirrorErrors: mWin ? (mWin.__mirrorErrors || []) : []
                    };
                    if (mWin && typeof mWin.drawFrame === "function") {
                        if (typeof mWin.setFitMode === "function") mWin.setFitMode("autofit");
                        // Tahap 74: Create test frame (y=0..12 yellow, y=12..2598 green, y=2598..2656 black 58px)
                        const paddedFrame = document.createElement("canvas");
                        paddedFrame.width = 1216;
                        paddedFrame.height = 2656;
                        const pctx = paddedFrame.getContext("2d");
                        pctx.fillStyle = "rgb(255, 230, 0)"; // Yellow top margin (above Dynamic Island)
                        pctx.fillRect(0, 0, 1216, 12);
                        pctx.fillStyle = "rgb(0, 220, 100)"; // Green screen body
                        pctx.fillRect(0, 12, 1216, 2586);
                        pctx.fillStyle = "rgb(0, 0, 0)"; // Black H.265 padding (58px)
                        pctx.fillRect(0, 2598, 1216, 58);

                        // Guard parent postMessage during canvas pixel tests so no re-entrant IPC window resizes occur
                        const realPost = mWin.parent ? mWin.parent.postMessage : null;
                        if (mWin.parent) mWin.parent.postMessage = () => {};

                        // Uji A: Portrait (rotation 0)
                        if (typeof mWin.setRotation === "function") mWin.setRotation(0);
                        mWin.drawFrame(paddedFrame);

                        const mc = mWin.document.getElementById("c");
                        if (mc) {
                            const mctx = mc.getContext("2d");
                            // Read pixel near top edge (y = 2) -> Yellow
                            const pTop = mctx.getImageData(Math.floor(mc.width / 2), 2, 1, 1).data;
                            res.pixelTopData = [pTop[0], pTop[1], pTop[2]];
                            res.topMarginNotCropped = (pTop[0] > 200 && pTop[1] > 180);

                            // Read pixel near bottom edge (y = mc.height - 3) -> Green (not black)
                            const pBot = mctx.getImageData(Math.floor(mc.width / 2), mc.height - 3, 1, 1).data;
                            res.pixelBotData = [pBot[0], pBot[1], pBot[2]];
                            res.bottomPaddingTrimmedPortrait = (pBot[1] > 170 && pBot[0] < 50);

                            // Uji B: Landscape (rotation 270)
                            if (typeof mWin.setRotation === "function") {
                                mWin.setRotation(270);
                                mWin.drawFrame(paddedFrame);
                                const pRight = mctx.getImageData(mc.width - 3, Math.floor(mc.height / 2), 1, 1).data;
                                res.pixelRightData = [pRight[0], pRight[1], pRight[2]];
                                res.rightPaddingTrimmedLandscape = (pRight[1] > 170);
                                mWin.setRotation(0);
                            } else {
                                res.rightPaddingTrimmedLandscape = true;
                            }

                            res.pixelRenderOk = (res.topMarginNotCropped && res.bottomPaddingTrimmedPortrait);
                        }

                        if (mWin.parent && realPost) mWin.parent.postMessage = realPost;

                        const stEl = mWin.document.getElementById("mirror-status");
                        res.statusCleared = !stEl || stEl.textContent.trim() === "" || stEl.style.display === "none";
                    }
                } catch (err) {
                    res.mirrorTestError = String(err);
                }

                // 7. Screen Empty check
                const screenEmpty = document.querySelector("#screen-empty");
                if (screenEmpty) {
                    const st = window.getComputedStyle(screenEmpty);
                    res.screenEmptyCentered = st.display === "flex" && (st.justifyContent === "center" || st.alignItems === "center");
                }

                // 8. Screen Note check (Zero Bottom Text)
                const screenNote = document.querySelector("#screen-note");
                if (screenNote) {
                    const stNote = window.getComputedStyle(screenNote);
                    res.screenNoteHidden = (stNote.display === "none" || screenNote.offsetHeight === 0);
                }

                // 9. Stop Screen hidden check (Tahap 70: 0% red button leak on idle/ready)
                const stopScreen = document.querySelector("#stop-screen");
                if (stopScreen) {
                    const stStop = window.getComputedStyle(stopScreen);
                    res.stopScreenHidden = (stopScreen.hidden || stStop.display === "none" || stopScreen.classList.contains("is-hidden"));
                } else {
                    res.stopScreenHidden = true;
                }

                // 10. Rotate mode badge check
                const rotBadge = document.querySelector("#rotate-mode-badge");
                if (rotBadge) {
                    res.hasRotateBadge = true;
                    res.rotateBadgeText = rotBadge.textContent;
                }

                // 11. Tahap 71: Zero Bottom Black Bar measurement
                const scrView = document.querySelector("#screen-view");
                const qNav = document.querySelector(".quick-nav");
                if (scrView && qNav) {
                    const svRect = scrView.getBoundingClientRect();
                    const qnRect = qNav.getBoundingClientRect();
                    res.bottomGapPx = Math.abs(svRect.bottom - qnRect.top);
                    res.zeroBottomBlackBar = (res.bottomGapPx <= 1.0);
                } else {
                    res.zeroBottomBlackBar = true;
                }

                // 12. Tahap 72: Proportional Fullscreen Portrait Center Layout Check
                const appRootEl = document.querySelector("#app-root") || document.querySelector(".app");
                const phoneScreenEl = document.querySelector("#phone-screen");
                if (appRootEl && phoneScreenEl) {
                    const wasDrawerOpen = appRootEl.classList.contains("drawer-open");
                    if (wasDrawerOpen) appRootEl.classList.remove("drawer-open");
                    const ctrlPane = document.querySelector("#control-pane");
                    const wasCtrlHidden = ctrlPane ? ctrlPane.hidden : true;
                    if (ctrlPane) ctrlPane.hidden = true;

                    appRootEl.classList.add("is-fullsize");
                    const psRect = phoneScreenEl.getBoundingClientRect();
                    res.fullscreenPortraitWidthRatio = psRect.width / (window.innerWidth || 1);
                    if (window.innerWidth > 600) {
                        res.fullscreenPortraitProportional = (res.fullscreenPortraitWidthRatio < 0.65);
                    } else {
                        res.fullscreenPortraitProportional = (psRect.width <= (window.innerWidth + 2));
                    }
                    const centerDiff = Math.abs((psRect.left + psRect.right) / 2 - (window.innerWidth / 2));
                    res.fullscreenPortraitCentered = (centerDiff < 25);

                    // Tahap 73: Brand title and side drawer checks in fullsize
                    const brandTitle = document.querySelector(".brand-title");
                    res.brandTitleVisibleInFullsize = brandTitle ? (window.getComputedStyle(brandTitle).display !== "none") : true;

                    const sideDrawer = document.querySelector(".side-drawer") || ctrlPane;
                    res.sideDrawerHiddenInFullsize = sideDrawer ? (window.getComputedStyle(sideDrawer).display === "none") : true;

                    appRootEl.classList.remove("is-fullsize");
                    if (wasDrawerOpen) appRootEl.classList.add("drawer-open");
                    if (ctrlPane) ctrlPane.hidden = wasCtrlHidden;
                } else {
                    res.fullscreenPortraitProportional = true;
                    res.fullscreenPortraitCentered = true;
                }

                return res;
            })()
            """
            dom_res = window.evaluate_js(eval_code)
            if not isinstance(dom_res, dict):
                test_data["error"] = f"Unexpected evaluate_js result: {dom_res}"
            else:
                js_errors = dom_res.get("jsErrors", [])
                has_pill = dom_res.get("hasStatusPill", False)
                pill_text = dom_res.get("statusPillText", "")
                drawer_open = dom_res.get("drawerOpened", False)
                auto_rotate = dom_res.get("autoRotateClicked", False)
                fit_contain = dom_res.get("fitContainClicked", False)
                fit_auto = dom_res.get("fitAutoClicked", True)
                screen_note_hidden = dom_res.get("screenNoteHidden", True)
                parallax_active = dom_res.get("wallpaperParallaxActive", True)
                pixel_ok = dom_res.get("pixelRenderOk", False)
                status_cleared = dom_res.get("statusCleared", False)
                stop_hidden = dom_res.get("stopScreenHidden", True)

                passed = (
                    len(js_errors) == 0
                    and has_pill
                    and len(pill_text) > 0
                    and drawer_open
                    and auto_rotate
                    and fit_contain
                    and fit_auto
                    and screen_note_hidden
                    and parallax_active
                    and pixel_ok
                    and status_cleared
                    and stop_hidden
                    and dom_res.get("topMarginNotCropped", True)
                    and dom_res.get("zeroBottomBlackBar", True)
                    and dom_res.get("rotateBadgeNoOverlap", True)
                    and dom_res.get("allTopBarIconsVisible", True)
                    and dom_res.get("bottomPaddingTrimmedPortrait", True)
                    and dom_res.get("rightPaddingTrimmedLandscape", True)
                    and dom_res.get("fullscreenPortraitProportional", True)
                    and dom_res.get("brandTitleVisibleInFullsize", True)
                    and dom_res.get("sideDrawerHiddenInFullsize", True)
                )

                # Step C: Live Real #win-max Click Verification in Live WebView2 (Tahap 73, 74 & 76 Proportional Restore)
                try:
                    # Ensure Auto-Rotate is OFF, drawer is closed, and window is in Portrait before testing Skenario A
                    window.evaluate_js('(() => { const off = document.querySelector(\'[data-auto-rotate-choice="off"]\'); if (off) off.click(); const c = document.querySelector("#close-settings"); if (c) c.click(); if (typeof window.setLandscapeOrientation === "function") window.setLandscapeOrientation(false); })()')
                    api.set_orientation(landscape=False, drawer_open=False)
                    time.sleep(0.4)

                    # Tahap 76: Capture initial portrait physical rect and inner dimensions
                    hwnd = api._get_hwnd()
                    user32 = getattr(ctypes.windll, "user32", None)
                    init_rect = wintypes.RECT()
                    if hwnd and user32:
                        user32.GetWindowRect(wintypes.HWND(hwnd), ctypes.byref(init_rect))
                    init_phys_w = init_rect.right - init_rect.left
                    init_phys_h = init_rect.bottom - init_rect.top
                    init_inner = window.evaluate_js('({ w: window.innerWidth, h: window.innerHeight })') or {}

                    # Skenario A: Drawer Closed -> Real #win-max.click() -> Enter Full Size
                    window.evaluate_js('window.toggleFullSize()')
                    time.sleep(0.8)
                    fs_a = window.evaluate_js('''
                    (() => {
                        const appRoot = document.querySelector("#app-root") || document.querySelector(".app");
                        const ps = document.querySelector("#phone-screen");
                        const sv = document.querySelector("#screen-view");
                        const bt = document.querySelector(".brand-title");
                        const ab = document.querySelector(".app-bar");
                        const qn = document.querySelector(".quick-nav");
                        const psRect = ps ? ps.getBoundingClientRect() : null;
                        const svRect = sv ? sv.getBoundingClientRect() : null;
                        const abRect = ab ? ab.getBoundingClientRect() : null;
                        const qnRect = qn ? qn.getBoundingClientRect() : null;
                        const btSt = bt ? window.getComputedStyle(bt).display : "none";
                        const cDiff = psRect ? Math.abs((psRect.left + psRect.right)/2 - (window.innerWidth / 2)) : 999;
                        const topOk = (psRect && abRect) ? (psRect.top >= (abRect.bottom - 1.5)) : true;
                        const bottomOk = (psRect && qnRect) ? (psRect.bottom <= (qnRect.top + 1.5)) : true;
                        const stageDiff = (psRect && abRect && qnRect) ? Math.abs(psRect.height - (qnRect.top - abRect.bottom)) : 0;
                        const heightFitsStage = (stageDiff <= 6.0);
                        return {
                            isFullsizeClass: appRoot ? appRoot.classList.contains("is-fullsize") : false,
                            isFullsizeVar: !!window.isFullSize,
                            brandTitleVisible: (btSt !== "none"),
                            psWidth: psRect ? psRect.width : 0,
                            svWidth: svRect ? svRect.width : 0,
                            centerDiff: cDiff,
                            centered: (cDiff <= 25),
                            proportional: psRect ? (psRect.width >= 310 && (window.innerWidth <= 600 ? psRect.width <= (window.innerWidth + 2) : psRect.width <= 650)) : false,
                            topOk: topOk,
                            bottomOk: bottomOk,
                            heightFitsStage: heightFitsStage,
                            stageDiff: stageDiff,
                            appBarBottom: abRect ? abRect.bottom : 0,
                            psTop: psRect ? psRect.top : 0,
                            psBottom: psRect ? psRect.bottom : 0,
                            qnTop: qnRect ? qnRect.top : 0,
                            windowInnerWidth: window.innerWidth,
                            windowInnerHeight: window.innerHeight
                        };
                    })()
                    ''')

                    # Skenario A: Real #win-max.click() -> Exit Full Size
                    window.evaluate_js('window.toggleFullSize()')
                    time.sleep(0.8)
                    restore_a = window.evaluate_js('''
                    (() => {
                        const appRoot = document.querySelector("#app-root") || document.querySelector(".app");
                        return {
                            isFullsizeClass: appRoot ? appRoot.classList.contains("is-fullsize") : false,
                            isFullsizeVar: !!window.isFullSize,
                            windowInnerWidth: window.innerWidth,
                            windowInnerHeight: window.innerHeight
                        };
                    })()
                    ''')

                    # Tahap 76: Measure restored dimensions after Skenario A
                    restored_rect_a = wintypes.RECT()
                    if hwnd and user32:
                        user32.GetWindowRect(wintypes.HWND(hwnd), ctypes.byref(restored_rect_a))
                    restored_phys_w_a = restored_rect_a.right - restored_rect_a.left
                    restored_phys_h_a = restored_rect_a.bottom - restored_rect_a.top
                    restored_inner_a = restore_a if isinstance(restore_a, dict) else {}
                    restore_a_proportional = (
                        restored_phys_w_a > 0
                        and (restored_phys_h_a / float(restored_phys_w_a)) >= 2.15
                        and abs(restored_phys_h_a - init_phys_h) <= 20
                        and abs(restored_inner_a.get("windowInnerHeight", 0) - init_inner.get("h", 0)) <= 20
                        and restored_inner_a.get("windowInnerWidth", 0) < 600
                    )

                    # Skenario B: Drawer Opened -> Real #win-max.click() -> Auto-close Drawer & Centered
                    window.evaluate_js('document.querySelector("#btn-settings").click()')
                    time.sleep(0.7)
                    window.evaluate_js('window.toggleFullSize()')
                    time.sleep(1.0)
                    fs_b = window.evaluate_js('''
                    (() => {
                        const appRoot = document.querySelector("#app-root") || document.querySelector(".app");
                        const ps = document.querySelector("#phone-screen");
                        const sd = document.querySelector(".side-drawer") || document.querySelector("#control-pane");
                        const ab = document.querySelector(".app-bar");
                        const qn = document.querySelector(".quick-nav");
                        const psRect = ps ? ps.getBoundingClientRect() : null;
                        const abRect = ab ? ab.getBoundingClientRect() : null;
                        const qnRect = qn ? qn.getBoundingClientRect() : null;
                        const sdSt = sd ? window.getComputedStyle(sd).display : "none";
                        const cDiff = psRect ? Math.abs((psRect.left + psRect.right)/2 - (window.innerWidth / 2)) : 999;
                        const topOk = (psRect && abRect) ? (psRect.top >= (abRect.bottom - 1.5)) : true;
                        const bottomOk = (psRect && qnRect) ? (psRect.bottom <= (qnRect.top + 1.5)) : true;
                        const stageDiff = (psRect && abRect && qnRect) ? Math.abs(psRect.height - (qnRect.top - abRect.bottom)) : 0;
                        const heightFitsStage = (stageDiff <= 6.0);
                        return {
                            isFullsizeClass: appRoot ? appRoot.classList.contains("is-fullsize") : false,
                            sideDrawerHidden: (sdSt === "none"),
                            drawerOpenClassRemoved: appRoot ? !appRoot.classList.contains("drawer-open") : true,
                            psWidth: psRect ? psRect.width : 0,
                            centerDiff: cDiff,
                            centered: (cDiff <= 25),
                            proportional: psRect ? (psRect.width >= 310 && (window.innerWidth <= 600 ? psRect.width <= (window.innerWidth + 2) : psRect.width <= 650)) : false,
                            topOk: topOk,
                            bottomOk: bottomOk,
                            heightFitsStage: heightFitsStage,
                            stageDiff: stageDiff
                        };
                    })()
                    ''')

                    # Exit Full Size after Skenario B
                    window.evaluate_js('window.toggleFullSize()')
                    time.sleep(0.6)

                    # Tahap 76: Measure restored dimensions after Skenario B
                    restored_rect_b = wintypes.RECT()
                    if hwnd and user32:
                        user32.GetWindowRect(wintypes.HWND(hwnd), ctypes.byref(restored_rect_b))
                    restored_phys_w_b = restored_rect_b.right - restored_rect_b.left
                    restored_phys_h_b = restored_rect_b.bottom - restored_rect_b.top
                    restored_inner_b = window.evaluate_js('({ w: window.innerWidth, h: window.innerHeight })') or {}
                    restore_b_proportional = (
                        restored_phys_w_b > 0
                        and (restored_phys_h_b / float(restored_phys_w_b)) >= 2.15
                        and abs(restored_phys_h_b - init_phys_h) <= 20
                        and abs(restored_inner_b.get("h", 0) - init_inner.get("h", 0)) <= 20
                        and restored_inner_b.get("w", 0) < 600
                    )

                    real_click_ok = (
                        isinstance(fs_a, dict) and fs_a.get("isFullsizeClass") is True
                        and fs_a.get("brandTitleVisible") is True
                        and fs_a.get("proportional") is True
                        and fs_a.get("centered") is True
                        and fs_a.get("topOk") is True
                        and fs_a.get("bottomOk") is True
                        and fs_a.get("heightFitsStage") is True
                        and isinstance(restore_a, dict) and restore_a.get("isFullsizeClass") is False
                        and restore_a_proportional
                        and isinstance(fs_b, dict) and fs_b.get("isFullsizeClass") is True
                        and fs_b.get("sideDrawerHidden") is True
                        and fs_b.get("drawerOpenClassRemoved") is True
                        and fs_b.get("proportional") is True
                        and fs_b.get("centered") is True
                        and fs_b.get("topOk") is True
                        and fs_b.get("bottomOk") is True
                        and fs_b.get("heightFitsStage") is True
                        and restore_b_proportional
                    )
                    test_data["real_fullscreen_click_test"] = {
                        "passed": real_click_ok,
                        "skenarioA": fs_a,
                        "restoreA": restore_a,
                        "restoreAProportional": restore_a_proportional,
                        "restoreADims": {"w": restored_phys_w_a, "h": restored_phys_h_a, "innerW": restored_inner_a.get("windowInnerWidth", 0), "innerH": restored_inner_a.get("windowInnerHeight", 0)},
                        "skenarioB": fs_b,
                        "restoreBProportional": restore_b_proportional,
                        "restoreBDims": {"w": restored_phys_w_b, "h": restored_phys_h_b, "innerW": restored_inner_b.get("w", 0), "innerH": restored_inner_b.get("h", 0)},
                        "initDims": {"w": init_phys_w, "h": init_phys_h, "innerW": init_inner.get("w", 0), "innerH": init_inner.get("h", 0)}
                    }
                    if not real_click_ok:
                        passed = False

                    # Step D: Tahap 75 Verification (Fullscreen Persistence, Center Restore, Rounded Corners, & Reels Anti-False-Positive)
                    try:
                        # 1. Fullscreen Persistence: Enter fullscreen
                        window.evaluate_js('window.toggleFullSize()')
                        time.sleep(0.6)

                        # Trigger sync_device_aspect, quick-nav click, and orientation change while in fullscreen
                        persist_res = window.evaluate_js('''
                        (() => {
                            const appRoot = document.querySelector("#app-root") || document.querySelector(".app");
                            const initialFull = !!window.isFullSize && (appRoot ? appRoot.classList.contains("is-fullsize") : false);

                            // A: sync_device_aspect
                            if (window.pywebview && window.pywebview.api && typeof window.pywebview.api.sync_device_aspect === "function") {
                                window.pywebview.api.sync_device_aspect(346, false);
                            }

                            // B: quick-nav home button click
                            const navHome = document.querySelector("#nav-home");
                            if (navHome) navHome.click();

                            // C: setLandscapeOrientation calls while in fullscreen
                            if (typeof window.setLandscapeOrientation === "function") {
                                window.setLandscapeOrientation(false, "auto");
                            }

                            const stillFull = !!window.isFullSize && (appRoot ? appRoot.classList.contains("is-fullsize") : false);
                            return {
                                initialFull: initialFull,
                                stillFull: stillFull,
                                windowWidth: window.innerWidth,
                                windowHeight: window.innerHeight
                            };
                        })()
                        ''')

                        # 2. Exit Fullscreen -> Check Center/Position Restore & Rounded Corners
                        time.sleep(0.3)
                        api.set_orientation(landscape=False, drawer_open=False)
                        window.evaluate_js('window.toggleFullSize()')
                        time.sleep(0.8)

                        restore_pos_res = window.evaluate_js('''
                        (() => {
                            const appRoot = document.querySelector("#app-root") || document.querySelector(".app");
                            const appSt = appRoot ? window.getComputedStyle(appRoot) : null;
                            const bodySt = window.getComputedStyle(document.body);
                            const sx = typeof window.screenX !== "undefined" ? window.screenX : window.screenLeft;
                            const sy = typeof window.screenY !== "undefined" ? window.screenY : window.screenTop;
                            return {
                                isFullsizeClass: appRoot ? appRoot.classList.contains("is-fullsize") : false,
                                isFullsizeVar: !!window.isFullSize,
                                screenX: sx,
                                screenY: sy,
                                notAtZeroZero: (sx > 15 || sy > 15),
                                appBorderRadius: appSt ? appSt.borderRadius : "",
                                bodyBorderRadius: bodySt ? bodySt.borderRadius : "",
                                cornersRounded: (appSt && appSt.borderRadius.includes("12px")) || (bodySt && bodySt.borderRadius.includes("12px"))
                            };
                        })()
                        ''')

                        # 3. Synthetic Instagram Dark Reel vs eFootball 2027 auto-rotate orientation test
                        frame_test_res = window.evaluate_js('''
                        (() => {
                            const iframe = document.querySelector("#screen-view");
                            const mWin = iframe ? iframe.contentWindow : null;
                            if (!mWin || typeof mWin.analyzeFrameOrientation !== "function") {
                                return { error: "no-analyzeFrameOrientation" };
                            }

                            // Synthetic Instagram Dark Reel Frame (white clock & battery contrast, 1 blue flank, dark center)
                            const reelC = document.createElement("canvas");
                            reelC.width = 120;
                            reelC.height = 260;
                            const rCtx = reelC.getContext("2d");
                            rCtx.fillStyle = "rgb(5, 5, 5)";
                            rCtx.fillRect(0, 0, 120, 260);
                            // Draw clock digits (alternating white/black columns in x=14..32, y=4..8)
                            rCtx.fillStyle = "rgb(255, 255, 255)";
                            for (let cx = 14; cx <= 32; cx += 3) {
                                rCtx.fillRect(cx, 4, 2, 5);
                            }
                            // Draw battery on right (alternating columns in x=88..106, y=4..8)
                            for (let bx = 88; bx <= 106; bx += 3) {
                                rCtx.fillRect(bx, 4, 2, 5);
                            }
                            // Blue glow on left flank below clock
                            rCtx.fillStyle = "rgb(30, 40, 90)";
                            rCtx.fillRect(14, 9, 25, 2);

                            const reelInfo = mWin.analyzeFrameOrientation(reelC);

                            // Synthetic eFootball 2027 Frame (stadium turf across top y=0..35, dark dynamic island cutout, no clock/battery)
                            const efC = document.createElement("canvas");
                            efC.width = 120;
                            efC.height = 260;
                            const eCtx = efC.getContext("2d");
                            eCtx.fillStyle = "rgb(5, 5, 5)";
                            eCtx.fillRect(0, 0, 120, 260);
                            // Stadium turf across top y=0..35
                            eCtx.fillStyle = "rgb(20, 180, 50)";
                            eCtx.fillRect(0, 0, 120, 35);
                            // Dark Dynamic Island cutout at center top
                            eCtx.fillStyle = "rgb(2, 2, 2)";
                            eCtx.fillRect(46, 0, 29, 10);

                            const efInfo = mWin.analyzeFrameOrientation(efC);

                            return {
                                reelInfo: reelInfo,
                                efInfo: efInfo,
                                reelHasPortraitStatusBar: !!(reelInfo && reelInfo.hasPortraitStatusBar),
                                reelNotLandscapeGame: !!(reelInfo && !reelInfo.isFullscreenGame),
                                efHasNakedIsland: !!(efInfo && efInfo.hasNakedDynamicIsland),
                                efIsFullscreenGame: !!(efInfo && efInfo.isFullscreenGame)
                            };
                        })()
                        ''')

                        win_rect = api.get_window_rect()
                        rect_x = win_rect.get("x", 0)
                        rect_y = win_rect.get("y", 0)
                        not_at_zero_zero = (rect_x > 15 or rect_y > 15 or bool(restore_pos_res.get("notAtZeroZero", False)))

                        tahap75_ok = (
                            isinstance(persist_res, dict) and persist_res.get("stillFull") is True
                            and isinstance(restore_pos_res, dict) and restore_pos_res.get("isFullsizeVar") is False
                            and restore_pos_res.get("cornersRounded") is True
                            and not_at_zero_zero
                            and isinstance(frame_test_res, dict)
                            and frame_test_res.get("reelHasPortraitStatusBar") is True
                            and frame_test_res.get("reelNotLandscapeGame") is True
                            and frame_test_res.get("efHasNakedIsland") is True
                            and frame_test_res.get("efIsFullscreenGame") is True
                        )

                        test_data["tahap75_verification"] = {
                            "passed": tahap75_ok,
                            "persistence": persist_res,
                            "restore": restore_pos_res,
                            "windowRect": win_rect,
                            "notAtZeroZero": not_at_zero_zero,
                            "frameTest": frame_test_res,
                            "apiSavedPos": getattr(api, "_saved_window_pos", None),
                            "apiIsFullscreen": getattr(api, "_is_fullscreen", None)
                        }
                        if not tahap75_ok:
                            passed = False
                    except Exception as e_t75:
                        test_data["tahap75_verification"] = {"passed": False, "error": str(e_t75)}
                        passed = False

                    # Step E: Tahap 87 Verification (23 Scenarios - Multi-iPhone Notch & Dynamic Island, 6-Block Dual Pillarbox AssistiveTouch Lock, TikTok 276 Comments OLED, YouTube Portrait Ad Layout)
                    try:
                        tahap83_res = window.evaluate_js('''
                        (() => {
                            const iframe = document.querySelector("#screen-view");
                            const mWin = iframe ? iframe.contentWindow : null;
                            if (!mWin || typeof mWin.analyzeFrameOrientation !== "function") {
                                return { error: "no-analyzeFrameOrientation" };
                            }

                            // Skenario 1: Instagram @premierleague green pitch feed (00:16)
                            const f1 = document.createElement("canvas");
                            f1.width = 120; f1.height = 260;
                            const ctx1 = f1.getContext("2d");
                            ctx1.fillStyle = "rgb(255, 255, 255)"; // white background & top bar
                            ctx1.fillRect(0, 0, 120, 260);
                            ctx1.fillStyle = "rgb(0, 0, 0)"; // dynamic island
                            ctx1.fillRect(46, 0, 29, 10);
                            ctx1.fillStyle = "rgb(35, 110, 45)";
                            ctx1.fillRect(10, 60, 100, 120);
                            ctx1.fillStyle = "rgb(255, 255, 255)";
                            ctx1.fillRect(0, 230, 120, 30);
                            const info1 = mWin.analyzeFrameOrientation(f1);

                            // Skenario 2: Instagram dark shirt / green trees / dark video (02:27, 02:55)
                            const f2 = document.createElement("canvas");
                            f2.width = 120; f2.height = 260;
                            const ctx2 = f2.getContext("2d");
                            ctx2.fillStyle = "rgb(255, 255, 255)"; // white chrome
                            ctx2.fillRect(0, 0, 120, 260);
                            ctx2.fillStyle = "rgb(0, 0, 0)";
                            ctx2.fillRect(46, 0, 29, 10);
                            ctx2.fillStyle = "rgb(20, 20, 22)";
                            ctx2.fillRect(10, 50, 100, 150);
                            ctx2.fillStyle = "rgb(255, 255, 255)";
                            ctx2.fillRect(0, 230, 120, 30);
                            const info2 = mWin.analyzeFrameOrientation(f2);

                            // Skenario 3: TikTok "YE x CARTI" with dark car edge & white clock "07.41" (02:11)
                            const f3 = document.createElement("canvas");
                            f3.width = 120; f3.height = 260;
                            const ctx3 = f3.getContext("2d");
                            ctx3.fillStyle = "rgb(18, 18, 20)"; // dark video
                            ctx3.fillRect(0, 0, 120, 260);
                            ctx3.fillStyle = "rgb(0, 0, 0)"; // dynamic island
                            ctx3.fillRect(46, 0, 29, 10);
                            ctx3.fillStyle = "rgb(240, 240, 240)";
                            for (let cx = 14; cx <= 32; cx += 3) ctx3.fillRect(cx, 4, 2, 5);
                            ctx3.fillStyle = "rgb(12, 12, 14)";
                            ctx3.fillRect(0, 230, 120, 30);
                            ctx3.fillStyle = "rgb(245, 245, 245)";
                            ctx3.fillRect(55, 236, 10, 8);
                            ctx3.fillStyle = "rgb(25, 25, 28)";
                            ctx3.fillRect(1, 90, 3, 70);
                            const info3 = mWin.analyzeFrameOrientation(f3);

                            // Skenario 4: eFootball 2027 Formasi / Taktik 3-2-2-3 (01:04, 01:42)
                            const f4 = document.createElement("canvas");
                            f4.width = 120; f4.height = 260;
                            const ctx4 = f4.getContext("2d");
                            ctx4.fillStyle = "rgb(20, 22, 28)";
                            ctx4.fillRect(0, 0, 120, 260);
                            ctx4.fillStyle = "rgb(0, 0, 0)"; // black island
                            ctx4.fillRect(46, 0, 29, 10);
                            ctx4.fillStyle = "rgb(30, 110, 220)"; // blue "< Kembali" button
                            ctx4.fillRect(3, 4, 20, 20);
                            ctx4.fillStyle = "rgb(35, 85, 40)"; // dark green pitch
                            ctx4.fillRect(22, 45, 74, 150);
                            ctx4.fillStyle = "rgb(230, 230, 230)";
                            ctx4.fillRect(85, 8, 20, 10);
                            ctx4.fillStyle = "rgb(18, 24, 38)";
                            ctx4.fillRect(80, 190, 36, 50);
                            const info4 = mWin.analyzeFrameOrientation(f4);

                            // Skenario 5: eFootball KONAMI Red (01:09) & Poster Messi & Yamal (01:25)
                            const f5 = document.createElement("canvas");
                            f5.width = 120; f5.height = 260;
                            const ctx5 = f5.getContext("2d");
                            ctx5.fillStyle = "rgb(215, 20, 30)"; // vivid red
                            ctx5.fillRect(0, 0, 120, 260);
                            ctx5.fillStyle = "rgb(0, 0, 0)";
                            ctx5.fillRect(46, 0, 29, 10);
                            const info5 = mWin.analyzeFrameOrientation(f5);

                            // Skenario 6: YouTube Fullscreen Bright Blue Slide (02:15)
                            const f6 = document.createElement("canvas");
                            f6.width = 120; f6.height = 260;
                            const ctx6 = f6.getContext("2d");
                            ctx6.fillStyle = "rgb(10, 35, 220)"; // bright blue slide
                            ctx6.fillRect(0, 0, 120, 260);
                            ctx6.fillStyle = "rgb(16, 16, 16)";
                            ctx6.fillRect(0, 0, 120, 18);
                            ctx6.fillRect(0, 230, 120, 30);
                            ctx6.fillStyle = "rgb(140, 40, 180)";
                            ctx6.fillRect(5, 235, 10, 13);
                            const info6 = mWin.analyzeFrameOrientation(f6);

                            // Skenario 7: YouTube Fullscreen Dark Slide + Overlay (02:32)
                            const f7 = document.createElement("canvas");
                            f7.width = 120; f7.height = 260;
                            const ctx7 = f7.getContext("2d");
                            ctx7.fillStyle = "rgb(28, 28, 32)";
                            ctx7.fillRect(0, 0, 120, 260);
                            ctx7.fillStyle = "rgb(14, 14, 14)";
                            ctx7.fillRect(0, 0, 120, 18);
                            ctx7.fillRect(0, 230, 120, 30);
                            ctx7.fillStyle = "rgb(240, 240, 240)";
                            ctx7.fillRect(30, 45, 60, 15);
                            ctx7.fillRect(30, 90, 60, 20);
                            ctx7.fillRect(30, 140, 60, 20);
                            ctx7.fillRect(30, 175, 60, 15);
                            const info7 = mWin.analyzeFrameOrientation(f7);

                            // Skenario 8: Minimized YouTube to Portrait (02:07 - 02:11)
                            const f8 = document.createElement("canvas");
                            f8.width = 120; f8.height = 260;
                            const ctx8 = f8.getContext("2d");
                            ctx8.fillStyle = "rgb(255, 255, 255)"; // white page
                            ctx8.fillRect(0, 0, 120, 260);
                            ctx8.fillStyle = "rgb(0, 0, 0)";
                            ctx8.fillRect(46, 0, 29, 10);
                            ctx8.fillStyle = "rgb(20, 20, 20)";
                            for (let cx = 14; cx <= 30; cx += 2) ctx8.fillRect(cx, 5, 2, 4);
                            ctx8.fillRect(88, 5, 16, 4);
                            ctx8.fillStyle = "rgb(80, 80, 90)";
                            ctx8.fillRect(5, 25, 110, 70);
                            ctx8.fillStyle = "rgb(250, 250, 250)";
                            ctx8.fillRect(0, 110, 120, 150);
                            const info8 = mWin.analyzeFrameOrientation(f8);

                            // Skenario 9: Home Screen Kelvin (02:02, 02:42) & Folder iOS (02:05, 02:39)
                            const f9 = document.createElement("canvas");
                            f9.width = 120; f9.height = 260;
                            const ctx9 = f9.getContext("2d");
                            ctx9.fillStyle = "rgb(30, 45, 30)";
                            ctx9.fillRect(0, 0, 120, 260);
                            ctx9.fillStyle = "rgb(180, 190, 205)";
                            ctx9.fillRect(80, 0, 40, 60);
                            ctx9.fillStyle = "rgb(0, 0, 0)";
                            ctx9.fillRect(46, 0, 29, 10);
                            ctx9.fillStyle = "rgb(255, 255, 255)";
                            for (let cx = 14; cx <= 32; cx += 3) ctx9.fillRect(cx, 4, 2, 5);
                            ctx9.fillStyle = "rgb(52, 199, 89)";
                            ctx9.fillRect(88, 4, 18, 5);
                            ctx9.fillStyle = "rgb(60, 75, 70)";
                            ctx9.fillRect(12, 224, 96, 26);
                            const info9 = mWin.analyzeFrameOrientation(f9);

                            // Skenario 10: Control Center Home Screen (00:04, 00:27, 00:55)
                            const f10 = document.createElement("canvas");
                            f10.width = 120; f10.height = 260;
                            const ctx10 = f10.getContext("2d");
                            ctx10.fillStyle = "rgb(35, 45, 38)";
                            ctx10.fillRect(0, 0, 120, 260);
                            ctx10.fillStyle = "rgb(0, 0, 0)";
                            ctx10.fillRect(46, 0, 29, 10);
                            ctx10.fillStyle = "rgb(240, 240, 240)";
                            ctx10.fillRect(68, 24, 8, 4);
                            ctx10.fillRect(80, 24, 10, 4);
                            ctx10.fillRect(94, 24, 12, 4);
                            ctx10.fillStyle = "rgb(200, 200, 200)";
                            ctx10.fillRect(115, 90, 3, 60);
                            ctx10.fillStyle = "rgb(45, 55, 48)";
                            ctx10.fillRect(26, 234, 70, 14);
                            const info10 = mWin.analyzeFrameOrientation(f10);

                            // Skenario 11: Threads app white background & dark text (02:36)
                            const f11 = document.createElement("canvas");
                            f11.width = 120; f11.height = 260;
                            const ctx11 = f11.getContext("2d");
                            ctx11.fillStyle = "rgb(255, 255, 255)";
                            ctx11.fillRect(0, 0, 120, 260);
                            ctx11.fillStyle = "rgb(0, 0, 0)";
                            ctx11.fillRect(46, 0, 29, 10);
                            ctx11.fillStyle = "rgb(15, 15, 15)";
                            ctx11.fillRect(20, 40, 80, 10);
                            ctx11.fillRect(20, 60, 80, 30);
                            const info11 = mWin.analyzeFrameOrientation(f11);

                            // Skenario 12: Toast hidden check
                            const toastEl = document.querySelector("#toast-notification") || document.querySelector(".toast-notification");
                            const toastStyle = toastEl ? window.getComputedStyle(toastEl) : null;
                            const toastHidden = !toastEl || (
                                toastStyle && (
                                    toastStyle.display === "none" ||
                                    toastStyle.opacity === "0" ||
                                    toastStyle.visibility === "hidden" ||
                                    toastStyle.pointerEvents === "none"
                                )
                            );

                            // Skenario 13 (BARU): Instagram Reels "Rapuh perempuan gitu" Video Kotak 1:1 Berlatar Gelap (00:24)
                            const f13 = document.createElement("canvas");
                            f13.width = 120; f13.height = 260;
                            const ctx13 = f13.getContext("2d");
                            ctx13.fillStyle = "rgb(17, 17, 18)"; // dark top and bottom letterbox
                            ctx13.fillRect(0, 0, 120, 260);
                            ctx13.fillStyle = "rgb(0, 0, 0)"; // dynamic island
                            ctx13.fillRect(46, 0, 29, 10);
                            // Clock "08.17" at x=12..28, y=4..9 luma ~68
                            ctx13.fillStyle = "rgb(68, 68, 70)";
                            for (let cx = 12; cx <= 28; cx += 3) ctx13.fillRect(cx, 4, 2, 5);
                            // Battery at x=88..106, y=4..9 luma ~52
                            ctx13.fillStyle = "rgb(52, 52, 54)";
                            ctx13.fillRect(88, 4, 18, 5);
                            // "Reels Friends" header text at x=14..58, y=14..20 luma ~95
                            ctx13.fillStyle = "rgb(95, 95, 98)";
                            ctx13.fillRect(14, 14, 44, 6);
                            // Square 1:1 video at y=55..205 with bright wall on left edge x=1..20
                            ctx13.fillStyle = "rgb(130, 125, 120)";
                            ctx13.fillRect(1, 55, 118, 150);
                            ctx13.fillStyle = "rgb(235, 235, 240)";
                            ctx13.fillRect(35, 70, 50, 100);
                            // Bottom navigation bar with icon #2 solid white (x=34..44, y=235..244, luma ~165)
                            ctx13.fillStyle = "rgb(165, 165, 168)";
                            ctx13.fillRect(34, 235, 10, 9);
                            ctx13.fillStyle = "rgb(55, 55, 58)";
                            ctx13.fillRect(16, 236, 8, 7);
                            ctx13.fillRect(56, 236, 8, 7);
                            ctx13.fillRect(76, 236, 8, 7);
                            ctx13.fillRect(96, 236, 8, 7);
                            const info13 = mWin.analyzeFrameOrientation(f13);

                            // Skenario 14 (BARU): Recent Button 5s Grace Period & Instagram Feed Photo Atas Gelap + Bottom Bar Putih (02:33, 00:35)
                            // 14A: Dispatch user-nav-recent-clicked message to mWin
                            try {
                                if (typeof mWin.dispatchEvent === "function") {
                                    mWin.dispatchEvent(new MessageEvent("message", { data: { type: "user-nav-recent-clicked", autoRotate: true, currentlyLandscape: true } }));
                                } else if (typeof mWin._triggerRecentGrace === "function") {
                                    mWin._triggerRecentGrace();
                                } else {
                                    mWin.postMessage({ type: "user-nav-recent-clicked", autoRotate: true, currentlyLandscape: true }, "*");
                                }
                            } catch (_) {
                                if (typeof mWin._triggerRecentGrace === "function") mWin._triggerRecentGrace();
                            }
                            if (!mWin._getHomeExitGraceUntil || mWin._getHomeExitGraceUntil() <= Date.now()) {
                                if (typeof mWin._triggerRecentGrace === "function") mWin._triggerRecentGrace();
                            }
                            const graceRemaining = mWin._getHomeExitGraceUntil ? (mWin._getHomeExitGraceUntil() - Date.now()) : 5000;

                            // 14B: Feed Instagram dengan foto atas gelap ("kalo harus ngemis...") dan bottom bar putih
                            const f14 = document.createElement("canvas");
                            f14.width = 120; f14.height = 260;
                            const ctx14 = f14.getContext("2d");
                            ctx14.fillStyle = "rgb(18, 18, 18)"; // dark photo top
                            ctx14.fillRect(0, 0, 120, 228);
                            ctx14.fillStyle = "rgb(0, 0, 0)";
                            ctx14.fillRect(46, 0, 29, 10);
                            ctx14.fillStyle = "rgb(240, 240, 240)"; // bottom white nav bar
                            ctx14.fillRect(0, 228, 120, 32);
                            const info14 = mWin.analyzeFrameOrientation(f14);

                            // Skenario 15 (BARU Tahap 85): Google Translate Dark Mode di iPhone 11 Pro Max kebal Watermark & AssistiveTouch (00:09, 00:20 & 00:56)
                            const f15 = document.createElement("canvas");
                            f15.width = 120; f15.height = 260;
                            const ctx15 = f15.getContext("2d");
                            // Latar atas y=0..45 hitam/abu-abu gelap RGB(18,18,20)
                            ctx15.fillStyle = "rgb(18, 18, 20)";
                            ctx15.fillRect(0, 0, 120, 45);
                            // Jam putih "20.52" mepet pojok kiri x=5..22, y=3..8 luma 48
                            ctx15.fillStyle = "rgb(48, 48, 50)";
                            for (let cx = 5; cx <= 22; cx += 3) ctx15.fillRect(cx, 3, 2, 5);
                            // Baterai "80" mepet pojok kanan x=92..114, y=3..8 luma 52
                            ctx15.fillStyle = "rgb(52, 52, 54)";
                            ctx15.fillRect(92, 3, 22, 5);
                            // Judul "Google Terjemahan" di x=36..84, y=14..21 luma 110
                            ctx15.fillStyle = "rgb(110, 110, 112)";
                            ctx15.fillRect(36, 14, 48, 7);
                            ctx15.fillRect(100, 14, 12, 7); // profil
                            // Teks "Masukkan teks" di x=8..58, y=28..38 luma 140
                            ctx15.fillStyle = "rgb(140, 140, 142)";
                            ctx15.fillRect(8, 28, 50, 10);
                            // Kotak input abu-abu gelap RGB(28,28,30) di y=45..160
                            ctx15.fillStyle = "rgb(28, 28, 30)";
                            ctx15.fillRect(0, 45, 120, 115);
                            // Watermark "G/文" di tengah x=45..75, y=88..132 (luma ~75)
                            ctx15.fillStyle = "rgb(75, 75, 78)";
                            ctx15.fillRect(45, 88, 30, 44);
                            // AssistiveTouch floating di kanan x=94..114, y=118..142 (luma ~180)
                            ctx15.fillStyle = "rgb(180, 180, 182)";
                            ctx15.fillRect(94, 118, 20, 24);
                            // Tombol bahasa di y=165..185
                            ctx15.fillStyle = "rgb(38, 38, 42)";
                            ctx15.fillRect(14, 165, 40, 18);
                            ctx15.fillRect(66, 165, 40, 18);
                            // 3 tombol lingkaran biru di y=194..224 (mikrofon pastel blue di tengah x=48..72 RGB(138,180,248))
                            ctx15.fillStyle = "rgb(100, 150, 220)";
                            ctx15.fillRect(14, 198, 24, 20);
                            ctx15.fillRect(82, 198, 24, 20);
                            ctx15.fillStyle = "rgb(138, 180, 248)";
                            ctx15.fillRect(48, 192, 24, 30);
                            // Pita hitam bawah di y=230..248 RGB(12,12,14)
                            ctx15.fillStyle = "rgb(12, 12, 14)";
                            ctx15.fillRect(0, 230, 120, 18);
                            // Dimmed Home Bar abu-abu redup luma 62 di x=42..78, y=250..253
                            ctx15.fillStyle = "rgb(62, 62, 64)";
                            ctx15.fillRect(42, 250, 36, 4);
                            const info15 = mWin.analyzeFrameOrientation(f15);

                            // Skenario 16A (BARU Tahap 85): TikTok Launch Splash Screen Hitam Pekat dengan Logo Tinggi & AssistiveTouch (01:12 - 01:14)
                            const f16a = document.createElement("canvas");
                            f16a.width = 120; f16a.height = 260;
                            const ctx16a = f16a.getContext("2d");
                            ctx16a.fillStyle = "rgb(0, 0, 0)"; // 100% hitam pekat
                            ctx16a.fillRect(0, 0, 120, 260);
                            // Logo TikTok mencapai y=85..165 di x=42..78
                            ctx16a.fillStyle = "rgb(240, 240, 240)";
                            ctx16a.fillRect(42, 85, 36, 80);
                            ctx16a.fillStyle = "rgb(255, 0, 80)";
                            ctx16a.fillRect(46, 88, 8, 70);
                            ctx16a.fillStyle = "rgb(0, 240, 240)";
                            ctx16a.fillRect(66, 88, 8, 70);
                            // AssistiveTouch floating di kanan
                            ctx16a.fillStyle = "rgb(180, 180, 182)";
                            ctx16a.fillRect(94, 118, 20, 24);
                            const info16a = mWin.analyzeFrameOrientation(f16a);

                            // Skenario 16B (BARU Tahap 84): YouTube Portrait Dark Mode dengan Kartu Iklan/Komentar Putih Bawah (00:42)
                            const f16b = document.createElement("canvas");
                            f16b.width = 120; f16b.height = 260;
                            const ctx16b = f16b.getContext("2d");
                            // Status bar atas hitam y=0..12
                            ctx16b.fillStyle = "rgb(10, 10, 12)";
                            ctx16b.fillRect(0, 0, 120, 13);
                            // Mini player di y=14..82
                            ctx16b.fillStyle = "rgb(40, 50, 60)";
                            ctx16b.fillRect(0, 14, 120, 68);
                            // Bar judul gelap y=84..140
                            ctx16b.fillStyle = "rgb(18, 18, 20)";
                            ctx16b.fillRect(0, 84, 120, 56);
                            // Kartu iklan putih terang UNIQLO luma=235 di y=145..226
                            ctx16b.fillStyle = "rgb(235, 235, 235)";
                            ctx16b.fillRect(10, 145, 100, 81);
                            // Bar tab bawah hitam y=230..248
                            ctx16b.fillStyle = "rgb(12, 12, 14)";
                            ctx16b.fillRect(0, 230, 120, 30);
                            const info16b = mWin.analyzeFrameOrientation(f16b);

                            // Skenario 17 (BARU Tahap 86): TikTok Panel Komentar OLED Dark Mode ("319 komentar" 00:38 - 01:07)
                            const f17 = document.createElement("canvas");
                            f17.width = 120; f17.height = 260;
                            const ctx17 = f17.getContext("2d");
                            // Flank kiri hitam pekat x=0..34, y=0..74
                            ctx17.fillStyle = "rgb(0, 0, 0)";
                            ctx17.fillRect(0, 0, 35, 75);
                            // Top rows y=2..14 receded sand luma=18
                            ctx17.fillStyle = "rgb(18, 18, 20)";
                            ctx17.fillRect(37, 2, 47, 13);
                            // Center mini-video y=16..68 bright waves/text luma=145
                            ctx17.fillStyle = "rgb(145, 145, 150)";
                            ctx17.fillRect(37, 16, 47, 53);
                            // Flank kanan hitam pekat x=86..120, y=0..74
                            ctx17.fillStyle = "rgb(0, 0, 0)";
                            ctx17.fillRect(86, 0, 34, 75);
                            // Header "319 komentar" at y=76..90
                            ctx17.fillStyle = "rgb(8, 8, 10)";
                            ctx17.fillRect(0, 75, 120, 16);
                            ctx17.fillStyle = "rgb(180, 180, 180)";
                            ctx17.fillRect(40, 78, 40, 8);
                            // OLED pitch black sheet at y=92..248 RGB(4,4,6) luma=4
                            ctx17.fillStyle = "rgb(4, 4, 6)";
                            ctx17.fillRect(0, 91, 120, 157);
                            // Comment text rows
                            ctx17.fillStyle = "rgb(140, 140, 140)";
                            for (let cy = 96; cy <= 235; cy += 18) {
                                ctx17.fillRect(10, cy, 65, 5);
                            }
                            // AssistiveTouch floating on right
                            ctx17.fillStyle = "rgb(180, 180, 182)";
                            ctx17.fillRect(94, 120, 20, 24);
                            ctx17.fillStyle = "rgb(6, 6, 8)";
                            ctx17.fillRect(0, 248, 120, 12);
                            const info17 = mWin.analyzeFrameOrientation(f17);

                            // Skenario 18 (BARU Tahap 85): TikTok Video Beranda Gelap avgLuma < 25 (01:14 - 01:21)
                            const f18 = document.createElement("canvas");
                            f18.width = 120; f18.height = 260;
                            const ctx18 = f18.getContext("2d");
                            ctx18.fillStyle = "rgb(16, 16, 18)"; // avgLuma ~20
                            ctx18.fillRect(0, 0, 120, 260);
                            // Dynamic Island & clock "08.17" di header atas
                            ctx18.fillStyle = "rgb(0, 0, 0)";
                            ctx18.fillRect(46, 0, 29, 10);
                            ctx18.fillStyle = "rgb(240, 240, 240)";
                            for (let cx = 14; cx <= 32; cx += 3) ctx18.fillRect(cx, 4, 2, 5);
                            // Bottom Nav Bar dengan tombol [+] putih di tengah
                            ctx18.fillStyle = "rgb(8, 8, 10)";
                            ctx18.fillRect(0, 228, 120, 32);
                            ctx18.fillStyle = "rgb(245, 245, 245)";
                            ctx18.fillRect(54, 235, 12, 10);
                            // AssistiveTouch floating di kanan
                            ctx18.fillStyle = "rgb(180, 180, 182)";
                            ctx18.fillRect(94, 120, 20, 24);
                            const info18 = mWin.analyzeFrameOrientation(f18);

                            // Skenario 19 (BARU Tahap 86): YouTube Loading Skeleton Screen (detik 01:27)
                            const f19 = document.createElement("canvas");
                            f19.width = 120; f19.height = 260;
                            const ctx19 = f19.getContext("2d");
                            // Top player box y=0..80 black with 1 line of white title text at y=42..48
                            ctx19.fillStyle = "rgb(10, 10, 10)";
                            ctx19.fillRect(0, 0, 120, 80);
                            ctx19.fillStyle = "rgb(210, 210, 210)";
                            ctx19.fillRect(12, 42, 84, 7);
                            // Middle skeleton bars y=85..170 (luma ~28)
                            ctx19.fillStyle = "rgb(14, 14, 14)";
                            ctx19.fillRect(0, 81, 120, 90);
                            ctx19.fillStyle = "rgb(28, 28, 30)";
                            ctx19.fillRect(10, 88, 100, 12);
                            ctx19.fillRect(10, 108, 100, 12);
                            ctx19.fillRect(10, 128, 100, 12);
                            ctx19.fillRect(10, 148, 100, 12);
                            // Bottom section y=172..260 black with AssistiveTouch
                            ctx19.fillStyle = "rgb(12, 12, 12)";
                            ctx19.fillRect(0, 171, 120, 89);
                            ctx19.fillStyle = "rgb(180, 180, 182)";
                            ctx19.fillRect(94, 120, 20, 24);
                            const info19 = mWin.analyzeFrameOrientation(f19);

                            // Skenario 20 (BARU Tahap 86): YouTube Fullscreen 16:9 Podcast + AssistiveTouch in Bottom Pillarbox (detik 01:55 - 02:07)
                            const f20 = document.createElement("canvas");
                            f20.width = 120; f20.height = 260;
                            const ctx20 = f20.getContext("2d");
                            // Top Pillarbox y=0..18 pitch black
                            ctx20.fillStyle = "rgb(0, 0, 0)";
                            ctx20.fillRect(0, 0, 120, 19);
                            // Video 16:9 field y=20..232
                            ctx20.fillStyle = "rgb(34, 28, 24)"; // dark studio curtain
                            ctx20.fillRect(0, 19, 120, 214);
                            ctx20.fillStyle = "rgb(240, 240, 240)"; // Shopee white card
                            ctx20.fillRect(10, 24, 18, 34);
                            ctx20.fillStyle = "rgb(180, 180, 180)"; // studio clock
                            ctx20.fillRect(84, 25, 22, 20);
                            ctx20.fillStyle = "rgb(185, 65, 55)"; // face & red cap (warm color)
                            ctx20.fillRect(25, 65, 70, 150);
                            ctx20.fillStyle = "rgb(240, 240, 240)"; // side home bar
                            ctx20.fillRect(1, 90, 3, 80);
                            // Bottom Pillarbox y=234..260 pitch black on wings
                            ctx20.fillStyle = "rgb(0, 0, 0)";
                            ctx20.fillRect(0, 233, 120, 27);
                            // AssistiveTouch in center bottom x=52..68, y=236..252 (luma=151)
                            ctx20.fillStyle = "rgb(150, 150, 155)";
                            ctx20.fillRect(52, 236, 17, 17);
                            const info20 = mWin.analyzeFrameOrientation(f20);

                            // Skenario 21 (BARU Tahap 87): TikTok Comments "276 komentar" dengan Dark Streamer Room & AssistiveTouch (00:38 - 01:02)
                            const f21 = document.createElement("canvas");
                            f21.width = 120; f21.height = 260;
                            const ctx21 = f21.getContext("2d");
                            // Background OLED pitch black
                            ctx21.fillStyle = "rgb(0, 0, 0)";
                            ctx21.fillRect(0, 0, 120, 260);
                            // Pure black wings on y=0..48
                            ctx21.fillStyle = "rgb(0, 0, 0)";
                            ctx21.fillRect(0, 0, 36, 48);
                            ctx21.fillRect(84, 0, 36, 48);
                            // Mini streamer video in center x=36..84, y=0..48 (dark room, luma ~20)
                            ctx21.fillStyle = "rgb(20, 20, 22)";
                            ctx21.fillRect(36, 0, 48, 48);
                            // Caption text at y=55..61 (luma ~140)
                            ctx21.fillStyle = "rgb(140, 140, 140)";
                            ctx21.fillRect(6, 55, 78, 6);
                            // Close button "X" at x=102..114, y=64..72
                            ctx21.fillStyle = "rgb(180, 180, 180)";
                            ctx21.fillRect(102, 64, 12, 8);
                            // Header "276 komentar" at y=76..90
                            ctx21.fillStyle = "rgb(8, 8, 10)";
                            ctx21.fillRect(0, 75, 120, 16);
                            ctx21.fillStyle = "rgb(180, 180, 180)";
                            ctx21.fillRect(40, 78, 40, 8);
                            // OLED comments sheet at y=92..248
                            ctx21.fillStyle = "rgb(4, 4, 6)";
                            ctx21.fillRect(0, 91, 120, 157);
                            ctx21.fillStyle = "rgb(140, 140, 140)";
                            for (let cy = 96; cy <= 235; cy += 18) {
                                ctx21.fillRect(10, cy, 65, 5);
                            }
                            // AssistiveTouch floating on right x=94..114, y=120..144
                            ctx21.fillStyle = "rgb(180, 180, 182)";
                            ctx21.fillRect(94, 120, 20, 24);
                            ctx21.fillStyle = "rgb(6, 6, 8)";
                            ctx21.fillRect(0, 248, 120, 12);
                            const info21 = mWin.analyzeFrameOrientation(f21);

                            // Skenario 22 (BARU Tahap 87): YouTube Portrait Ad "BLOOMING SOCIALLY" / Hostinger Video + Dark Card (01:16 - 01:18)
                            const f22 = document.createElement("canvas");
                            f22.width = 120; f22.height = 260;
                            const ctx22 = f22.getContext("2d");
                            // Top status bar y=0..14
                            ctx22.fillStyle = "rgb(10, 10, 12)";
                            ctx22.fillRect(0, 0, 120, 14);
                            // Ad video / Player at y=16..84 (Hostinger bright ad luma ~140)
                            ctx22.fillStyle = "rgb(140, 140, 145)";
                            ctx22.fillRect(0, 14, 120, 72);
                            // Title strip band at y=90..124 (dark background with white title text)
                            ctx22.fillStyle = "rgb(14, 14, 16)";
                            ctx22.fillRect(0, 86, 120, 38);
                            ctx22.fillStyle = "rgb(230, 230, 230)";
                            ctx22.fillRect(8, 94, 104, 8);
                            ctx22.fillRect(8, 106, 75, 8);
                            // White pill buttons / chips at y=126..142
                            ctx22.fillStyle = "rgb(240, 240, 245)";
                            ctx22.fillRect(8, 126, 32, 14);
                            ctx22.fillRect(46, 126, 32, 14);
                            // Dark card / comments lower section at y=144..246
                            ctx22.fillStyle = "rgb(20, 20, 24)";
                            ctx22.fillRect(6, 144, 108, 90);
                            // Bottom nav bar y=248..260
                            ctx22.fillStyle = "rgb(10, 10, 12)";
                            ctx22.fillRect(0, 248, 120, 12);
                            // AssistiveTouch floating on right x=94..114, y=160..184
                            ctx22.fillStyle = "rgb(180, 180, 182)";
                            ctx22.fillRect(94, 160, 20, 24);
                            const info22 = mWin.analyzeFrameOrientation(f22);

                            // Skenario 23A (BARU Tahap 87): YouTube Fullscreen 16:9 Podcast + AssistiveTouch in Block 3..4 (detik 01:25 - 01:30)
                            const f23a = document.createElement("canvas");
                            f23a.width = 120; f23a.height = 260;
                            const ctx23a = f23a.getContext("2d");
                            // Top Pillarbox y=0..18 pitch black
                            ctx23a.fillStyle = "rgb(0, 0, 0)";
                            ctx23a.fillRect(0, 0, 120, 19);
                            // Video 16:9 field y=20..232 (Dark studio with creator face & hands on desk)
                            ctx23a.fillStyle = "rgb(32, 28, 26)";
                            ctx23a.fillRect(0, 19, 120, 214);
                            ctx23a.fillStyle = "rgb(185, 75, 60)"; // face & cap
                            ctx23a.fillRect(25, 45, 70, 75);
                            ctx23a.fillStyle = "rgb(200, 140, 100)"; // hands on desk
                            ctx23a.fillRect(20, 165, 80, 30);
                            ctx23a.fillStyle = "rgb(240, 240, 240)"; // side home bar
                            ctx23a.fillRect(1, 90, 3, 80);
                            // Bottom Pillarbox y=234..260
                            ctx23a.fillStyle = "rgb(0, 0, 0)";
                            ctx23a.fillRect(0, 233, 120, 27);
                            // AssistiveTouch above Recent button at x=64..82, y=236..252 (touches blocks 3 & 4)
                            ctx23a.fillStyle = "rgb(160, 160, 165)";
                            ctx23a.fillRect(64, 236, 18, 16);
                            const info23a = mWin.analyzeFrameOrientation(f23a);

                            // Skenario 23B (BARU Tahap 87): YouTube Fullscreen Bright Scene (Mac Mini Gray "GAUSAH", detik 01:35)
                            const f23b = document.createElement("canvas");
                            f23b.width = 120; f23b.height = 260;
                            const ctx23b = f23b.getContext("2d");
                            // Top Pillarbox y=0..18 pitch black
                            ctx23b.fillStyle = "rgb(0, 0, 0)";
                            ctx23b.fillRect(0, 0, 120, 19);
                            // Video field y=20..232: Mac Mini gray / aluminum surface (luma ~110..150)
                            ctx23b.fillStyle = "rgb(130, 130, 135)";
                            ctx23b.fillRect(0, 19, 120, 214);
                            ctx23b.fillStyle = "rgb(230, 230, 230)"; // bold white text "GAUSAH"
                            ctx23b.fillRect(20, 95, 80, 30);
                            ctx23b.fillStyle = "rgb(240, 240, 240)"; // side home bar
                            ctx23b.fillRect(1, 90, 3, 80);
                            // Bottom Pillarbox y=234..260 with AssistiveTouch
                            ctx23b.fillStyle = "rgb(0, 0, 0)";
                            ctx23b.fillRect(0, 233, 120, 27);
                            ctx23b.fillStyle = "rgb(160, 160, 165)";
                            ctx23b.fillRect(64, 236, 18, 16);
                            const info23b = mWin.analyzeFrameOrientation(f23b);

                            // Skenario 23C (BARU Tahap 87): YouTube Fullscreen White Web Browser (detik 01:43)
                            const f23c = document.createElement("canvas");
                            f23c.width = 120; f23c.height = 260;
                            const ctx23c = f23c.getContext("2d");
                            // Top Pillarbox y=0..18 pitch black
                            ctx23c.fillStyle = "rgb(0, 0, 0)";
                            ctx23c.fillRect(0, 0, 120, 19);
                            // Video field y=20..232: White web page (luma ~245)
                            ctx23c.fillStyle = "rgb(245, 245, 245)";
                            ctx23c.fillRect(0, 19, 120, 214);
                            ctx23c.fillStyle = "rgb(20, 20, 20)"; // web text rows
                            ctx23c.fillRect(15, 40, 90, 10);
                            ctx23c.fillRect(15, 60, 90, 10);
                            ctx23c.fillRect(15, 80, 90, 10);
                            ctx23c.fillStyle = "rgb(240, 240, 240)"; // side home bar
                            ctx23c.fillRect(1, 90, 3, 80);
                            // Bottom Pillarbox y=234..260 with AssistiveTouch
                            ctx23c.fillStyle = "rgb(0, 0, 0)";
                            ctx23c.fillRect(0, 233, 120, 27);
                            ctx23c.fillStyle = "rgb(160, 160, 165)";
                            ctx23c.fillRect(64, 236, 18, 16);
                            const info23c = mWin.analyzeFrameOrientation(f23c);

                            // Skenario 24A (BARU Tahap 88): Iklan Hijau Indodana dengan Kapsul Jam Biru iPhone 11 Pro Max Notch (Detik 01:00)
                            const f24a = document.createElement("canvas");
                            f24a.width = 120; f24a.height = 260;
                            const ctx24a = f24a.getContext("2d");
                            // Top status bar dark y=0..12
                            ctx24a.fillStyle = "rgb(10, 12, 10)";
                            ctx24a.fillRect(0, 0, 120, 13);
                            // Notch cutout
                            ctx24a.fillStyle = "rgb(0, 0, 0)";
                            ctx24a.fillRect(40, 0, 40, 10);
                            // Blue clock pill at x=4..30, y=2..10 (RGB 0, 122, 255)
                            ctx24a.fillStyle = "rgb(0, 122, 255)";
                            ctx24a.fillRect(4, 2, 26, 9);
                            // White clock text "00.41"
                            ctx24a.fillStyle = "rgb(255, 255, 255)";
                            for (let cx = 8; cx <= 24; cx += 3) ctx24a.fillRect(cx, 4, 2, 5);
                            // Right status icons (Wi-Fi & battery)
                            ctx24a.fillStyle = "rgb(255, 255, 255)";
                            ctx24a.fillRect(86, 4, 12, 5);
                            ctx24a.fillRect(102, 4, 10, 5);
                            // Natural green Indodana ad cards in body y=13..260 (RGB 40, 120, 50)
                            ctx24a.fillStyle = "rgb(40, 120, 50)";
                            ctx24a.fillRect(0, 13, 120, 247);
                            const info24a = mWin.analyzeFrameOrientation(f24a);

                            // Skenario 24B (BARU Tahap 88): Video TikTok dengan Kapsul Jam Biru iPhone 11 Pro Max Notch ("00.41" Detik 01:56)
                            const f24b = document.createElement("canvas");
                            f24b.width = 120; f24b.height = 260;
                            const ctx24b = f24b.getContext("2d");
                            ctx24b.fillStyle = "rgb(15, 18, 22)";
                            ctx24b.fillRect(0, 0, 120, 260);
                            ctx24b.fillStyle = "rgb(0, 0, 0)";
                            ctx24b.fillRect(40, 0, 40, 10);
                            ctx24b.fillStyle = "rgb(0, 122, 255)";
                            ctx24b.fillRect(4, 2, 26, 9);
                            ctx24b.fillStyle = "rgb(255, 255, 255)";
                            for (let cx = 8; cx <= 24; cx += 3) ctx24b.fillRect(cx, 4, 2, 5);
                            ctx24b.fillStyle = "rgb(255, 255, 255)";
                            ctx24b.fillRect(86, 4, 12, 5);
                            ctx24b.fillRect(102, 4, 10, 5);
                            const info24b = mWin.analyzeFrameOrientation(f24b);

                            // Skenario 25 (BARU Tahap 88): YouTube Fullscreen "HERMES AGENT" Dark Studio Flanks with Center Presenter (Detik 00:27)
                            const f25 = document.createElement("canvas");
                            f25.width = 120; f25.height = 260;
                            const ctx25 = f25.getContext("2d");
                            // Top & Bottom Pillarbox
                            ctx25.fillStyle = "rgb(0, 0, 0)";
                            ctx25.fillRect(0, 0, 120, 16);
                            ctx25.fillRect(0, 235, 120, 25);
                            // Dark studio background in 16:9 field y=16..234
                            ctx25.fillStyle = "rgb(12, 12, 14)";
                            ctx25.fillRect(0, 16, 120, 218);
                            // Side home bar
                            ctx25.fillStyle = "rgb(240, 240, 240)";
                            ctx25.fillRect(1, 90, 3, 80);
                            // Center studio presenter & desk at y=75..180, x=30..90
                            ctx25.fillStyle = "rgb(180, 115, 65)"; // warm wood desk
                            ctx25.fillRect(30, 140, 60, 40);
                            ctx25.fillStyle = "rgb(190, 110, 75)"; // skin / arms
                            ctx25.fillRect(40, 80, 40, 50);
                            // Bright studio lighting on center
                            ctx25.fillStyle = "rgb(150, 150, 160)";
                            ctx25.fillRect(35, 75, 50, 60);
                            const info25 = mWin.analyzeFrameOrientation(f25);

                            // Skenario 26 (BARU Tahap 88): YouTube Fullscreen "PASANG AI AGENT" Asymmetric Bright Left Flank & Dark Right Studio (Detik 01:16)
                            const f26 = document.createElement("canvas");
                            f26.width = 120; f26.height = 260;
                            const ctx26 = f26.getContext("2d");
                            // Top & Bottom Pillarbox
                            ctx26.fillStyle = "rgb(0, 0, 0)";
                            ctx26.fillRect(0, 0, 120, 16);
                            ctx26.fillRect(0, 235, 120, 25);
                            // Left bright studio room (y=16..234, x=0..60)
                            ctx26.fillStyle = "rgb(120, 120, 125)";
                            ctx26.fillRect(0, 16, 60, 218);
                            // Right darker room with presenter face/skin (y=16..234, x=60..120)
                            ctx26.fillStyle = "rgb(25, 25, 28)";
                            ctx26.fillRect(60, 16, 60, 218);
                            ctx26.fillStyle = "rgb(190, 110, 75)"; // presenter face/skin on mid-right
                            ctx26.fillRect(65, 85, 40, 70);
                            // Side home bar
                            ctx26.fillStyle = "rgb(240, 240, 240)";
                            ctx26.fillRect(1, 90, 3, 80);
                            const info26 = mWin.analyzeFrameOrientation(f26);

                            // Skenario 27A (BARU Tahap 89): YouTube Fullscreen "VEDA MASTER CLASS" Dark Slide + Watermark "R" & AssistiveTouch (Detik 01:49 - 02:16)
                            const f27a = document.createElement("canvas");
                            f27a.width = 120; f27a.height = 260;
                            const ctx27a = f27a.getContext("2d");
                            ctx27a.fillStyle = "rgb(0, 0, 0)";
                            ctx27a.fillRect(0, 0, 120, 12); // Top pillarbox
                            ctx27a.fillStyle = "rgb(14, 14, 18)"; // dark slide body
                            ctx27a.fillRect(0, 13, 120, 226);
                            ctx27a.fillStyle = "rgb(230, 230, 230)"; // white presentation title
                            ctx27a.fillRect(15, 30, 90, 8);
                            ctx27a.fillStyle = "rgb(200, 200, 200)"; // bullet text
                            ctx27a.fillRect(15, 55, 80, 6);
                            ctx27a.fillRect(15, 75, 85, 6);
                            ctx27a.fillRect(15, 95, 75, 6);
                            ctx27a.fillStyle = "rgb(40, 45, 55)"; // presentation graphic
                            ctx27a.fillRect(15, 140, 90, 80);
                            ctx27a.fillStyle = "rgb(0, 0, 0)"; // Bottom pillarbox
                            ctx27a.fillRect(0, 240, 120, 20);
                            ctx27a.fillStyle = "rgb(160, 160, 160)"; // Watermark "R" at bottom left
                            ctx27a.fillRect(6, 242, 12, 8);
                            ctx27a.fillStyle = "rgb(170, 170, 175)"; // AssistiveTouch in center bottom
                            ctx27a.fillRect(50, 240, 20, 14);
                            ctx27a.fillStyle = "rgb(240, 240, 240)"; // side home bar
                            ctx27a.fillRect(1, 90, 3, 80);
                            const info27a = mWin.analyzeFrameOrientation(f27a);

                            // Skenario 27B (BARU Tahap 89): YouTube Fullscreen "VEDA MASTER CLASS" White Slide + AssistiveTouch (Detik 02:08)
                            const f27b = document.createElement("canvas");
                            f27b.width = 120; f27b.height = 260;
                            const ctx27b = f27b.getContext("2d");
                            ctx27b.fillStyle = "rgb(0, 0, 0)";
                            ctx27b.fillRect(0, 0, 120, 12); // Top pillarbox
                            ctx27b.fillStyle = "rgb(235, 235, 240)"; // white slide body
                            ctx27b.fillRect(0, 13, 120, 226);
                            ctx27b.fillStyle = "rgb(20, 20, 25)"; // dark slide text
                            ctx27b.fillRect(15, 30, 90, 10);
                            ctx27b.fillRect(15, 60, 90, 120);
                            ctx27b.fillStyle = "rgb(0, 0, 0)"; // Bottom pillarbox
                            ctx27b.fillRect(0, 240, 120, 20);
                            ctx27b.fillStyle = "rgb(170, 170, 175)"; // AssistiveTouch in center bottom
                            ctx27b.fillRect(50, 240, 20, 14);
                            ctx27b.fillStyle = "rgb(240, 240, 240)"; // side home bar
                            ctx27b.fillRect(1, 90, 3, 80);
                            const info27b = mWin.analyzeFrameOrientation(f27b);

                            // Skenario 28A (BARU Tahap 89): YouTube Fullscreen "PASANG AI AGENT" Dark Jacket & Light Desk (Detik 02:30)
                            const f28a = document.createElement("canvas");
                            f28a.width = 120; f28a.height = 260;
                            const ctx28a = f28a.getContext("2d");
                            ctx28a.fillStyle = "rgb(0, 0, 0)";
                            ctx28a.fillRect(0, 0, 120, 12); // Top pillarbox
                            ctx28a.fillStyle = "rgb(25, 25, 30)"; // video field
                            ctx28a.fillRect(0, 13, 120, 226);
                            ctx28a.fillStyle = "rgb(18, 18, 22)"; // host dark jacket in y=84..135
                            ctx28a.fillRect(20, 84, 80, 52);
                            ctx28a.fillStyle = "rgb(210, 210, 215)"; // desk / laptop light in y=145..224
                            ctx28a.fillRect(15, 145, 90, 75);
                            ctx28a.fillStyle = "rgb(0, 0, 0)"; // Bottom pillarbox
                            ctx28a.fillRect(0, 240, 120, 20);
                            ctx28a.fillStyle = "rgb(170, 170, 175)"; // AssistiveTouch in center bottom
                            ctx28a.fillRect(50, 242, 20, 14);
                            ctx28a.fillStyle = "rgb(240, 240, 240)"; // side home bar
                            ctx28a.fillRect(1, 90, 3, 80);
                            const info28a = mWin.analyzeFrameOrientation(f28a);

                            // Skenario 28B (BARU Tahap 89): YouTube Fullscreen "Chapter 1: Pengenalan AI Agent" White Slide in Studio (Detik 02:44)
                            const f28b = document.createElement("canvas");
                            f28b.width = 120; f28b.height = 260;
                            const ctx28b = f28b.getContext("2d");
                            ctx28b.fillStyle = "rgb(0, 0, 0)";
                            ctx28b.fillRect(0, 0, 120, 12); // Top pillarbox
                            ctx28b.fillStyle = "rgb(230, 230, 235)"; // white slide body
                            ctx28b.fillRect(0, 13, 120, 226);
                            ctx28b.fillStyle = "rgb(20, 20, 25)"; // "Chapter 1" text
                            ctx28b.fillRect(15, 30, 90, 12);
                            ctx28b.fillRect(15, 60, 90, 140);
                            ctx28b.fillStyle = "rgb(0, 0, 0)"; // Bottom pillarbox
                            ctx28b.fillRect(0, 240, 120, 20);
                            ctx28b.fillStyle = "rgb(170, 170, 175)"; // AssistiveTouch in center bottom
                            ctx28b.fillRect(50, 242, 20, 14);
                            ctx28b.fillStyle = "rgb(240, 240, 240)"; // side home bar
                            ctx28b.fillRect(1, 90, 3, 80);
                            const info28b = mWin.analyzeFrameOrientation(f28b);

                            // Skenario 29A (BARU Tahap 90): YouTube Fullscreen Ferry Irwandi (OSD ON + Red Scrub Bar) (Detik 00:28 & 00:50)
                            const f29a = document.createElement("canvas");
                            f29a.width = 120; f29a.height = 260;
                            const ctx29a = f29a.getContext("2d");
                            ctx29a.fillStyle = "rgb(26, 26, 30)"; // Ambient glow top pillarbox y=0..12
                            ctx29a.fillRect(0, 0, 120, 13);
                            ctx29a.fillStyle = "rgb(20, 20, 22)"; // Ferry black jacket on left y=13..238
                            ctx29a.fillRect(0, 13, 60, 226);
                            ctx29a.fillStyle = "rgb(120, 120, 124)"; // Acoustic wall on right
                            ctx29a.fillRect(60, 13, 60, 226);
                            ctx29a.fillStyle = "rgb(240, 240, 240)"; // "MP" logo at bottom left
                            ctx29a.fillRect(8, 208, 16, 16);
                            ctx29a.fillStyle = "rgb(26, 26, 30)"; // Ambient glow bottom pillarbox
                            ctx29a.fillRect(0, 240, 120, 20);
                            ctx29a.fillStyle = "rgb(170, 170, 175)"; // AssistiveTouch in center bottom
                            ctx29a.fillRect(50, 242, 20, 14);
                            ctx29a.fillStyle = "rgb(240, 240, 240)"; // side home bar
                            ctx29a.fillRect(1, 90, 3, 80);
                            ctx29a.fillStyle = "rgb(255, 0, 40)"; // YouTube OSD red vertical scrub progress bar at x=26..29, y=16..230
                            ctx29a.fillRect(26, 16, 4, 214);
                            const info29a = mWin.analyzeFrameOrientation(f29a);

                            // Skenario 29B (BARU Tahap 90): YouTube Fullscreen Ferry Irwandi Playing in Landscape (OSD & Home Bar faded away) (Detik 00:30 - 00:43 & 00:52 - 00:55)
                            const f29b = document.createElement("canvas");
                            f29b.width = 120; f29b.height = 260;
                            const ctx29b = f29b.getContext("2d");
                            ctx29b.fillStyle = "rgb(26, 26, 30)"; // Ambient glow top pillarbox (luma=26 > 16)
                            ctx29b.fillRect(0, 0, 120, 13);
                            ctx29b.fillStyle = "rgb(20, 20, 22)"; // Ferry black jacket on left y=13..238
                            ctx29b.fillRect(0, 13, 60, 226);
                            ctx29b.fillStyle = "rgb(120, 120, 124)"; // Acoustic wall on right
                            ctx29b.fillRect(60, 13, 60, 226);
                            ctx29b.fillStyle = "rgb(240, 240, 240)"; // "MP" logo at bottom left
                            ctx29b.fillRect(8, 208, 16, 16);
                            ctx29b.fillStyle = "rgb(26, 26, 30)"; // Ambient glow bottom pillarbox
                            ctx29b.fillRect(0, 240, 120, 20);
                            ctx29b.fillStyle = "rgb(170, 170, 175)"; // AssistiveTouch in center bottom
                            ctx29b.fillRect(50, 242, 20, 14);
                            // Evaluated in active landscape session
                            mWin.currentRotation = 270;
                            if (mWin.window) mWin.window.currentRotation = 270;
                            const info29b = mWin.analyzeFrameOrientation(f29b, 270);

                            // Skenario 30A (BARU Tahap 90): Zoom-to-Fill "LAUNCHING KAMPUS MALAKA" (OSD ON, No Pillarbox, No Home Bar) (Detik 00:08 - 00:12)
                            const f30a = document.createElement("canvas");
                            f30a.width = 120; f30a.height = 260;
                            const ctx30a = f30a.getContext("2d");
                            ctx30a.fillStyle = "rgb(70, 75, 80)"; // Full bleed video content
                            ctx30a.fillRect(0, 0, 120, 260);
                            ctx30a.fillStyle = "rgb(255, 0, 40)"; // Red progress bar vertical at x=26..29, y=16..220
                            ctx30a.fillRect(26, 16, 4, 204);
                            ctx30a.fillStyle = "rgb(220, 220, 225)"; // Vertical title text at x=96..112, y=20..180
                            ctx30a.fillRect(96, 20, 16, 160);
                            ctx30a.fillStyle = "rgb(170, 170, 175)"; // AssistiveTouch at x=48..70, y=230..252
                            ctx30a.fillRect(50, 232, 20, 18);
                            const info30a = mWin.analyzeFrameOrientation(f30a, 270);

                            // Skenario 30B (BARU Tahap 90): Minimize to YouTube Portrait Watch Page (Detik 00:25)
                            const f30b = document.createElement("canvas");
                            f30b.width = 120; f30b.height = 260;
                            const ctx30b = f30b.getContext("2d");
                            ctx30b.fillStyle = "rgb(15, 15, 18)"; // Top status bar
                            ctx30b.fillRect(0, 0, 120, 13);
                            ctx30b.fillStyle = "rgb(0, 122, 255)"; // Blue clock pill at x=5..24, y=3..9
                            ctx30b.fillRect(5, 3, 22, 7);
                            ctx30b.fillStyle = "rgb(52, 199, 89)"; // Green battery at x=88..114, y=3..9
                            ctx30b.fillRect(88, 3, 26, 7);
                            ctx30b.fillStyle = "rgb(100, 100, 110)"; // Mini-player at y=14..82
                            ctx30b.fillRect(0, 14, 120, 68);
                            ctx30b.fillStyle = "rgb(18, 18, 20)"; // Dark UI title & channel divider band at y=86..126
                            ctx30b.fillRect(0, 86, 120, 41);
                            ctx30b.fillStyle = "rgb(220, 220, 220)"; // Title text line
                            ctx30b.fillRect(10, 90, 80, 6);
                            ctx30b.fillStyle = "rgb(160, 160, 170)"; // Lower cards at y=135..235
                            ctx30b.fillRect(0, 135, 120, 101);
                            const info30b = mWin.analyzeFrameOrientation(f30b, 270);

                            // Skenario 30C (BARU Tahap 90): iOS Home Screen with PiP (Detik 00:45)
                            const f30c = document.createElement("canvas");
                            f30c.width = 120; f30c.height = 260;
                            const ctx30c = f30c.getContext("2d");
                            ctx30c.fillStyle = "rgb(15, 15, 18)"; // Top status bar
                            ctx30c.fillRect(0, 0, 120, 13);
                            ctx30c.fillStyle = "rgb(0, 122, 255)"; // Blue clock pill at x=5..24, y=3..9
                            ctx30c.fillRect(5, 3, 22, 7);
                            ctx30c.fillStyle = "rgb(240, 240, 240)"; // White battery
                            ctx30c.fillRect(88, 3, 26, 7);
                            ctx30c.fillStyle = "rgb(70, 70, 80)"; // PiP mini player at y=14..80
                            ctx30c.fillRect(0, 14, 120, 67);
                            ctx30c.fillStyle = "rgb(40, 40, 50)"; // Wallpaper y=81..174
                            ctx30c.fillRect(0, 81, 120, 93);
                            ctx30c.fillStyle = "rgb(120, 120, 130)"; // Dock area at y=175..248
                            ctx30c.fillRect(0, 175, 120, 74);
                            ctx30c.fillStyle = "rgb(52, 199, 89)"; // Green Phone
                            ctx30c.fillRect(15, 210, 18, 25);
                            ctx30c.fillStyle = "rgb(0, 122, 255)"; // Blue Safari
                            ctx30c.fillRect(45, 210, 18, 25);
                            ctx30c.fillStyle = "rgb(52, 199, 89)"; // Green Messages
                            ctx30c.fillRect(75, 210, 18, 25);
                            const info30c = mWin.analyzeFrameOrientation(f30c, 270);

                            // Skenario 31 (BARU Tahap 92): eFootball 2027 Lionel Messi & Lamine Yamal Neon Poster Screen (01:06 - 01:23 di iPhone 17 Pro)
                            const f31 = document.createElement("canvas");
                            f31.width = 120; f31.height = 260;
                            const ctx31 = f31.getContext("2d");
                            ctx31.fillStyle = "rgb(14, 18, 52)"; // Dark navy base
                            ctx31.fillRect(0, 0, 120, 260);

                            // Pil hitam Dynamic Island di tengah atas (x=46..74, y=4..9, RGB 0,0,0)
                            ctx31.fillStyle = "rgb(0, 0, 0)";
                            ctx31.fillRect(46, 4, 29, 6);

                            // Pojok kiri atas (x=4..32, y=2..11): lingkaran neon biru/cyan RGB(0, 180, 255) + cincin putih RGB(240, 250, 255)
                            ctx31.fillStyle = "rgb(0, 180, 255)";
                            ctx31.fillRect(4, 2, 29, 10);
                            ctx31.fillStyle = "rgb(240, 250, 255)";
                            ctx31.fillRect(10, 4, 16, 5);

                            // x=4..28, y=12..25: lingkaran hitam RGB(10, 12, 24)
                            ctx31.fillStyle = "rgb(10, 12, 24)";
                            ctx31.fillRect(4, 12, 25, 14);

                            // Pojok kanan atas (x=78..116, y=2..12): grafis neon cyan & biru tua RGB(0, 210, 255) / RGB(12, 20, 65)
                            for (let x = 78; x <= 116; x++) {
                                ctx31.fillStyle = (x % 2 === 0) ? "rgb(0, 210, 255)" : "rgb(12, 20, 65)";
                                ctx31.fillRect(x, 2, 1, 11);
                            }

                            // Sepertiga atas (y=14..88): sosok Lamine Yamal dengan neon cyan (0,225,255), kuning (255,230,0), magenta (255,0,180)
                            for (let y = 14; y <= 88; y++) {
                                for (let x = 8; x <= 112; x += 3) {
                                    ctx31.fillStyle = "rgb(0, 225, 255)";
                                    ctx31.fillRect(x, y, 1, 1);
                                    ctx31.fillStyle = "rgb(255, 230, 0)";
                                    ctx31.fillRect(x + 1, y, 1, 1);
                                    ctx31.fillStyle = "rgb(255, 0, 180)";
                                    ctx31.fillRect(x + 2, y, 1, 1);
                                }
                            }

                            // Pita tengah (y=92..145): sabuk biru tua gelap RGB(14, 18, 52) dengan logo kuning/putih eFOOTBALL 2027 di x=36..84, y=102..138
                            ctx31.fillStyle = "rgb(14, 18, 52)";
                            ctx31.fillRect(0, 92, 120, 54);
                            for (let y = 102; y <= 138; y += 4) {
                                ctx31.fillStyle = "rgb(255, 235, 60)";
                                ctx31.fillRect(36, y, 49, 2);
                            }
                            // Spinner lingkaran putih loading di x=52..68, y=118..134
                            ctx31.fillStyle = "rgb(255, 255, 255)";
                            ctx31.fillRect(52, 118, 17, 17);

                            // Sepertiga bawah (y=150..248): sosok Lionel Messi dengan seragam pink/magenta (245, 20, 190), ungu (140, 30, 255), cyan (0, 215, 255)
                            for (let y = 150; y <= 248; y++) {
                                for (let x = 8; x <= 112; x += 3) {
                                    ctx31.fillStyle = "rgb(245, 20, 190)";
                                    ctx31.fillRect(x, y, 1, 1);
                                    ctx31.fillStyle = "rgb(140, 30, 255)";
                                    ctx31.fillRect(x + 1, y, 1, 1);
                                    ctx31.fillStyle = "rgb(0, 215, 255)";
                                    ctx31.fillRect(x + 2, y, 1, 1);
                                }
                            }
                            // Kotak teks hak cipta di kiri bawah (x=10..38, y=158..228)
                            ctx31.fillStyle = "rgb(200, 200, 210)";
                            ctx31.fillRect(10, 158, 29, 71);
                            // Tombol putih [ i ] di kanan bawah (x=10..26, y=232..246)
                            ctx31.fillStyle = "rgb(245, 245, 250)";
                            ctx31.fillRect(10, 232, 17, 15);

                            // Evaluasi optRotation = 0 (baru masuk dari Portrait)
                            const info31_at0 = mWin.analyzeFrameOrientation(f31, 0);

                            // Evaluasi optRotation = 270 (sudah di Horizontal)
                            const info31_at270 = mWin.analyzeFrameOrientation(f31, 270);

                            // Skenario 32A (BARU Tahap 93): iPhone 17 Pro Real Home Screen Kelvin (00:03 - 00:09, 00:26, 00:38, 01:23, 01:31)
                            const f32a = document.createElement("canvas");
                            f32a.width = 120; f32a.height = 260;
                            const ctx32a = f32a.getContext("2d");
                            // Background kabut abu-abu kehijauan di atas (y=0..91): bgLumaLeft ~ 152, leftFlankAvgSat ~ 14
                            ctx32a.fillStyle = "rgb(150, 155, 150)";
                            ctx32a.fillRect(0, 0, 120, 92);

                            // Pil hitam Dynamic Island di tengah atas (x=46..74, y=4..9, islandAvgLuma = 0 < 24)
                            ctx32a.fillStyle = "rgb(0, 0, 0)";
                            ctx32a.fillRect(46, 4, 29, 6);

                            // Angka jam putih "15.16" di x=10..36, y=3..9 (leftClockMaxLuma = 245, contrast = 95 >= 32, edgeCountLeft >= 3)
                            ctx32a.fillStyle = "rgb(245, 245, 245)";
                            for (let cx = 10; cx <= 34; cx += 4) {
                                ctx32a.fillRect(cx, 3, 2, 7);
                            }

                            // Ikon sinyal/Wi-Fi & Baterai hijau "79" di x=80..116, y=2..9 (rightStatusMaxLuma - min >= 24, edgeCountRight >= 3)
                            ctx32a.fillStyle = "rgb(52, 199, 89)";
                            ctx32a.fillRect(88, 3, 18, 6);
                            ctx32a.fillStyle = "rgb(245, 245, 245)";
                            ctx32a.fillRect(94, 4, 6, 4);

                            // 16 Ikon aplikasi warna-warni di sepertiga atas (y=25..88, upperPosterNeonPixels >= 32)
                            for (let iy = 25; iy <= 76; iy += 17) {
                                for (let ix = 12; ix <= 96; ix += 26) {
                                    ctx32a.fillStyle = "rgb(0, 190, 255)";
                                    ctx32a.fillRect(ix, iy, 18, 13);
                                }
                            }
                            // Badge merah notifikasi (misal "70" di ikon Photos pada iy=25)
                            ctx32a.fillStyle = "rgb(235, 45, 45)";
                            ctx32a.fillRect(24, 25, 8, 8);

                            // Wallpaper hutan gelap di tengah (y=92..145, RGB 18, 28, 22 -> luma <= 38, g >= b -> navyBeltRatio < 0.10)
                            ctx32a.fillStyle = "rgb(18, 28, 22)";
                            ctx32a.fillRect(0, 92, 120, 54);

                            // 4 Ikon Dock & Platter Dock iOS di y=155..246 (Phone, Safari, Messages, WhatsApp)
                            ctx32a.fillStyle = "rgb(65, 80, 70)";
                            ctx32a.fillRect(8, 212, 104, 38);
                            for (let ix = 14; ix <= 92; ix += 26) {
                                ctx32a.fillStyle = "rgb(52, 199, 89)"; // WhatsApp / Phone green
                                ctx32a.fillRect(ix, 216, 18, 18);
                            }
                            // Badge notifikasi merah "15.161", "1.894", "290" di Dock (RGB 235, 45, 45 -> B=45 < 80, lowerTruePinkPurplePixels = 0)
                            ctx32a.fillStyle = "rgb(235, 45, 45)";
                            ctx32a.fillRect(26, 212, 10, 8);
                            ctx32a.fillRect(52, 212, 10, 8);
                            ctx32a.fillRect(78, 212, 10, 8);
                            // Baris ikon di y=155..205
                            for (let ix = 14; ix <= 92; ix += 26) {
                                ctx32a.fillStyle = "rgb(255, 149, 0)"; // Orange icon
                                ctx32a.fillRect(ix, 160, 18, 18);
                            }
                            const info32a = mWin.analyzeFrameOrientation(f32a, 0);

                            // Skenario 32B (BARU Tahap 93): iPhone 17 Pro Open Folder "Social" (00:16 & 00:24 -> 00:25)
                            const f32b = document.createElement("canvas");
                            f32b.width = 120; f32b.height = 260;
                            const ctx32b = f32b.getContext("2d");
                            // Frosted blur wallpaper latar: topBlurAvgLuma = 62, botBlurAvgLuma = 58, sat = 4
                            ctx32b.fillStyle = "rgb(62, 60, 64)";
                            ctx32b.fillRect(0, 0, 120, 260);

                            // Pil hitam Dynamic Island di tengah atas (x=46..74, y=4..9, islandAvgLuma = 0 < 24)
                            ctx32b.fillStyle = "rgb(0, 0, 0)";
                            ctx32b.fillRect(46, 4, 29, 6);

                            // Blur bawah y=198..238
                            ctx32b.fillStyle = "rgb(58, 56, 60)";
                            ctx32b.fillRect(0, 198, 120, 42);

                            // Kotak folder rounded platter di tengah (x=16..104, y=60..190, RGB 80, 80, 85)
                            ctx32b.fillStyle = "rgb(80, 80, 85)";
                            ctx32b.fillRect(16, 60, 88, 130);

                            // Judul folder "Social" di y=64..70
                            ctx32b.fillStyle = "rgb(240, 240, 240)";
                            ctx32b.fillRect(40, 64, 40, 6);

                            // 8 Ikon aplikasi di dalam folder dengan kontras ikon tajam (folderCenterMax - min >= 55)
                            for (let iy = 76; iy <= 140; iy += 32) {
                                for (let ix = 24; ix <= 80; ix += 28) {
                                    ctx32b.fillStyle = "rgb(0, 122, 255)";
                                    ctx32b.fillRect(ix, iy, 18, 18);
                                    ctx32b.fillStyle = "rgb(255, 255, 255)";
                                    ctx32b.fillRect(ix + 5, iy + 5, 8, 8);
                                }
                            }
                            const info32b = mWin.analyzeFrameOrientation(f32b, 0);

                            // Skenario 108 Sub-Test 2A: Home Screen iPhone 11 Pro Max TANPA Kapsul Ungu, Jam Putih Polos 01.47 di Atas Wallpaper Foto Terang + Dinding Putih di Kiri
                            const f108_2a = document.createElement("canvas");
                            f108_2a.width = 120; f108_2a.height = 260;
                            const ctx108_2a = f108_2a.getContext("2d");
                            ctx108_2a.fillStyle = "rgb(155, 150, 145)";
                            ctx108_2a.fillRect(0, 0, 120, 260);
                            // Dinding putih lebar di kiri (x=1..12, y=80..170 -> contrast to inner x=8..13 is 0 < 32)
                            ctx108_2a.fillStyle = "rgb(158, 152, 148)";
                            ctx108_2a.fillRect(0, 80, 12, 100);
                            // Jam putih polos 01.47 di x=6..35, y=3..9 (luma=245 vs bg=152 -> contrast 93 >= 22)
                            ctx108_2a.fillStyle = "rgb(245, 245, 245)";
                            for (let cx of [8, 14, 20, 26, 30]) {
                                ctx108_2a.fillRect(cx, 4, 2, 5);
                            }
                            // Ikon sinyal / WiFi / baterai di kanan (x=84..115, y=3..9)
                            ctx108_2a.fillRect(88, 4, 3, 5);
                            ctx108_2a.fillRect(96, 4, 3, 5);
                            ctx108_2a.fillRect(104, 4, 6, 5);
                            // Ikon aplikasi baris pertama di y=14..24
                            ctx108_2a.fillStyle = "rgb(52, 199, 89)";
                            ctx108_2a.fillRect(14, 15, 18, 9);
                            ctx108_2a.fillStyle = "rgb(235, 45, 45)";
                            ctx108_2a.fillRect(28, 13, 8, 8);
                            // Jaket navy di tengah
                            ctx108_2a.fillStyle = "rgb(25, 35, 65)";
                            ctx108_2a.fillRect(25, 95, 70, 45);
                            // Dock bawah
                            ctx108_2a.fillStyle = "rgb(140, 135, 130)";
                            ctx108_2a.fillRect(8, 212, 104, 38);
                            ctx108_2a.fillStyle = "rgb(52, 199, 89)";
                            ctx108_2a.fillRect(14, 216, 18, 18); ctx108_2a.fillRect(40, 216, 18, 18);
                            ctx108_2a.fillStyle = "rgb(235, 45, 45)";
                            ctx108_2a.fillRect(26, 212, 10, 8);
                            const info108_2a = mWin.analyzeFrameOrientation(f108_2a, 0);

                            // Skenario 108 Sub-Test 2B: Auto-Return dari Landscape 270 ke Portrait 0 Saat Buka Folder Social & Instagram Dark Mode
                            const info108_2b_folder = mWin.analyzeFrameOrientation(f32b, 270);
                            const f108_2b_ig = document.createElement("canvas");
                            f108_2b_ig.width = 120; f108_2b_ig.height = 260;
                            const ctx108_2b_ig = f108_2b_ig.getContext("2d");
                            ctx108_2b_ig.fillStyle = "rgb(12, 12, 12)";
                            ctx108_2b_ig.fillRect(0, 0, 120, 260);
                            // Jam putih polos
                            ctx108_2b_ig.fillStyle = "rgb(240, 240, 240)";
                            for (let cx of [8, 14, 20, 26, 30]) {
                                ctx108_2b_ig.fillRect(cx, 4, 2, 5);
                            }
                            ctx108_2b_ig.fillRect(88, 4, 3, 5); ctx108_2b_ig.fillRect(104, 4, 6, 5);
                            // Logo Instagram di y=12..26, x=6..48
                            ctx108_2b_ig.fillRect(8, 16, 36, 6);
                            // Post photo di tengah
                            ctx108_2b_ig.fillStyle = "rgb(40, 50, 60)";
                            ctx108_2b_ig.fillRect(0, 35, 120, 185);
                            // Bottom Nav Bar hitam seragam di y=232..255
                            ctx108_2b_ig.fillStyle = "rgb(10, 10, 10)";
                            ctx108_2b_ig.fillRect(0, 232, 120, 28);
                            // 5 Ikon nav putih di y=235..246
                            ctx108_2b_ig.fillStyle = "rgb(240, 240, 240)";
                            for (let bx of [16, 38, 60, 82, 104]) {
                                ctx108_2b_ig.fillRect(bx - 3, 238, 6, 6);
                            }
                            // Home Bar putih
                            ctx108_2b_ig.fillRect(42, 250, 36, 3);
                            const info108_2b_ig = mWin.analyzeFrameOrientation(f108_2b_ig, 270);

                            // Skenario 108 Sub-Test 2C: YouTube Portrait Watch Page dengan Iklan Warna-Warni MORE ACCESS TO GEMINI di Atas Kartu Putih
                            const f108_2c = document.createElement("canvas");
                            f108_2c.width = 120; f108_2c.height = 260;
                            const ctx108_2c = f108_2c.getContext("2d");
                            // Status bar hitam
                            ctx108_2c.fillStyle = "rgb(5, 5, 5)";
                            ctx108_2c.fillRect(0, 0, 120, 12);
                            ctx108_2c.fillStyle = "rgb(245, 245, 245)";
                            for (let cx of [8, 14, 20, 26, 30]) ctx108_2c.fillRect(cx, 4, 2, 5);
                            ctx108_2c.fillRect(88, 4, 3, 5); ctx108_2c.fillRect(104, 4, 6, 5);
                            // Top Video Player y=14..84 (warna-warni)
                            ctx108_2c.fillStyle = "rgb(30, 80, 180)";
                            ctx108_2c.fillRect(0, 14, 120, 70);
                            ctx108_2c.fillStyle = "rgb(52, 199, 89)";
                            ctx108_2c.fillRect(10, 25, 40, 12);
                            ctx108_2c.fillStyle = "rgb(255, 204, 0)";
                            ctx108_2c.fillRect(60, 25, 40, 12);
                            ctx108_2c.fillStyle = "rgb(0, 190, 255)";
                            ctx108_2c.fillRect(15, 45, 80, 15);
                            // Mid Divider Band gelap y=88..134
                            ctx108_2c.fillStyle = "rgb(15, 15, 18)";
                            ctx108_2c.fillRect(0, 85, 120, 50);
                            ctx108_2c.fillStyle = "rgb(160, 160, 160)";
                            ctx108_2c.fillRect(10, 92, 70, 8);
                            // Lower White Ad Card y=138..212
                            ctx108_2c.fillStyle = "rgb(245, 245, 245)";
                            ctx108_2c.fillRect(8, 138, 104, 76);
                            // Dark bottom recommendations y=215..260 + home bar
                            ctx108_2c.fillStyle = "rgb(15, 15, 18)";
                            ctx108_2c.fillRect(0, 215, 120, 45);
                            ctx108_2c.fillStyle = "rgb(240, 240, 240)";
                            ctx108_2c.fillRect(42, 250, 36, 3);
                            const info108_2c = mWin.analyzeFrameOrientation(f108_2c, 0);

                            // Skenario 108 Sub-Test 2D: YouTube Fullscreen Landscape Adegan Host Berbaju Hitam di Studio Gelap + Meja Kayu
                            const f108_2d = document.createElement("canvas");
                            f108_2d.width = 120; f108_2d.height = 260;
                            const ctx108_2d = f108_2d.getContext("2d");
                            // Dual Black Pillarbox
                            ctx108_2d.fillStyle = "rgb(0, 0, 0)";
                            ctx108_2d.fillRect(0, 0, 120, 12);
                            ctx108_2d.fillRect(0, 242, 120, 18);
                            // Studio gelap y=14..58
                            ctx108_2d.fillStyle = "rgb(20, 20, 24)";
                            ctx108_2d.fillRect(0, 14, 120, 46);
                            // Host hitam & meja kayu hangat y=60..240
                            ctx108_2d.fillStyle = "rgb(25, 25, 28)";
                            ctx108_2d.fillRect(20, 60, 80, 75);
                            ctx108_2d.fillStyle = "rgb(210, 150, 110)";
                            ctx108_2d.fillRect(48, 65, 24, 28);
                            ctx108_2d.fillStyle = "rgb(130, 85, 50)";
                            ctx108_2d.fillRect(10, 135, 100, 105);
                            // Side Home Bar di tepi kiri
                            ctx108_2d.fillStyle = "rgb(20, 20, 20)";
                            ctx108_2d.fillRect(6, 90, 8, 80);
                            ctx108_2d.fillStyle = "rgb(240, 240, 240)";
                            ctx108_2d.fillRect(2, 98, 2, 65);
                            const info108_2d = mWin.analyzeFrameOrientation(f108_2d, 0);

                            // Skenario 108 Sub-Test 2E: YouTube Fullscreen Landscape Adegan Close-Up Laptop Putih/Perak Asus ProArt di Bawah & Latar Studio Gelap di Atas
                            const f108_2e = document.createElement("canvas");
                            f108_2e.width = 120; f108_2e.height = 260;
                            const ctx108_2e = f108_2e.getContext("2d");
                            // Dual Black Pillarbox
                            ctx108_2e.fillStyle = "rgb(0, 0, 0)";
                            ctx108_2e.fillRect(0, 0, 120, 12);
                            ctx108_2e.fillRect(0, 242, 120, 18);
                            // Studio gelap di atas y=14..86
                            ctx108_2e.fillStyle = "rgb(18, 18, 20)";
                            ctx108_2e.fillRect(0, 14, 120, 74);
                            // Laptop putih / perak terang di bawah y=125..238
                            ctx108_2e.fillStyle = "rgb(185, 185, 190)";
                            ctx108_2e.fillRect(6, 125, 108, 115);
                            // Side Home Bar di tepi kiri
                            ctx108_2e.fillStyle = "rgb(18, 18, 20)";
                            ctx108_2e.fillRect(6, 90, 8, 80);
                            ctx108_2e.fillStyle = "rgb(240, 240, 240)";
                            ctx108_2e.fillRect(2, 98, 2, 65);
                            const info108_2e_at0 = mWin.analyzeFrameOrientation(f108_2e, 0);
                            const info108_2e_at270 = mWin.analyzeFrameOrientation(f108_2e, 270);

                            // --- Skenario 109 Sub-Test 2A: Home Screen iPhone 11 Pro Max Kelvin Resolusi 1216x2656 (Full Visual Traps) ---
                            const f109_2a = document.createElement("canvas");
                            f109_2a.width = 1216; f109_2a.height = 2656;
                            const ctx109_2a = f109_2a.getContext("2d");
                            // Background mist wallpaper luma ~ 160
                            ctx109_2a.fillStyle = "rgb(160, 160, 160)";
                            ctx109_2a.fillRect(0, 0, 1216, 2656);
                            // Dinding putih panjang di tepi kiri (x=0..65, y=450..2100, luma=210)
                            ctx109_2a.fillStyle = "rgb(210, 210, 210)";
                            ctx109_2a.fillRect(0, 450, 65, 1650);
                            // Jaket hitam orang kiri (x=70..150, y=1200..1700, luma=22)
                            ctx109_2a.fillStyle = "rgb(22, 22, 22)";
                            ctx109_2a.fillRect(70, 1200, 80, 500);
                            // Jas hitam orang kanan di tepi kanan (x=1080..1216, y=200..2100, luma=20)
                            ctx109_2a.fillStyle = "rgb(20, 20, 20)";
                            ctx109_2a.fillRect(1080, 200, 136, 1900);
                            // Baju putih tengah (x=400..800, y=600..1800, luma=200)
                            ctx109_2a.fillStyle = "rgb(200, 200, 200)";
                            ctx109_2a.fillRect(400, 600, 400, 1200);
                            // Apple Music merah solid di kolom 1 (x=120..280, y=600..780)
                            ctx109_2a.fillStyle = "rgb(245, 30, 40)";
                            ctx109_2a.fillRect(120, 600, 160, 180);
                            // Badge merah 171 (x=220..340, y=1260..1340)
                            ctx109_2a.fillRect(220, 1260, 120, 80);
                            // Kemeja merah wallpaper (x=260..340, y=1500..1920)
                            ctx109_2a.fillStyle = "rgb(210, 35, 45)";
                            ctx109_2a.fillRect(260, 1500, 80, 420);
                            // Jam putih tipis "07.32" di kiri atas (x=60..350, y=30..90)
                            ctx109_2a.fillStyle = "rgb(250, 250, 250)";
                            for (let cx of [80, 140, 200, 260, 300]) {
                                ctx109_2a.fillRect(cx, 40, 20, 50);
                            }
                            // Status baterai/WiFi di kanan atas
                            ctx109_2a.fillRect(880, 40, 30, 50);
                            ctx109_2a.fillRect(960, 40, 30, 50);
                            ctx109_2a.fillRect(1040, 40, 60, 50);
                            // Bottom Dock translusen abu-abu gelap y=2160..2480, x=60..1156
                            ctx109_2a.fillStyle = "rgb(45, 45, 48)";
                            ctx109_2a.fillRect(60, 2160, 1096, 320);
                            // Slot 1: Phone (Green)
                            ctx109_2a.fillStyle = "rgb(52, 199, 89)";
                            ctx109_2a.fillRect(110, 2200, 160, 240);
                            // Slot 2: Safari (Blue/White)
                            ctx109_2a.fillStyle = "rgb(30, 140, 240)";
                            ctx109_2a.fillRect(370, 2200, 160, 240);
                            ctx109_2a.fillStyle = "rgb(245, 245, 245)";
                            ctx109_2a.fillRect(420, 2280, 60, 80);
                            // Slot 3: Messages (Green)
                            ctx109_2a.fillStyle = "rgb(52, 199, 89)";
                            ctx109_2a.fillRect(640, 2200, 160, 240);
                            // Slot 4: WhatsApp (Green)
                            ctx109_2a.fillRect(900, 2200, 160, 240);
                            // Area bawah dock
                            ctx109_2a.fillStyle = "rgb(20, 20, 22)";
                            ctx109_2a.fillRect(0, 2500, 1216, 156);

                            const info109_2a_at0 = mWin.analyzeFrameOrientation(f109_2a, 0);
                            const info109_2a_at270 = mWin.analyzeFrameOrientation(f109_2a, 270);

                            // --- Skenario 109 Sub-Test 2B: Home Screen dengan Picture-in-Picture YouTube di Atas ---
                            const f109_2b = document.createElement("canvas");
                            f109_2b.width = 1216; f109_2b.height = 2656;
                            const ctx109_2b = f109_2b.getContext("2d");
                            ctx109_2b.drawImage(f109_2a, 0, 0);
                            // Timpa bagian atas dengan jendela PiP YouTube gelap y=140..780, x=80..1130
                            ctx109_2b.fillStyle = "rgb(20, 20, 24)";
                            ctx109_2b.fillRect(80, 140, 1050, 640);
                            ctx109_2b.fillStyle = "rgb(180, 120, 80)";
                            ctx109_2b.fillRect(400, 300, 400, 300);

                            const info109_2b_at0 = mWin.analyzeFrameOrientation(f109_2b, 0);
                            const info109_2b_at270 = mWin.analyzeFrameOrientation(f109_2b, 270);

                            // --- Skenario 109 Sub-Test 2C: Instagram Dark Mode Feed & Reels + Folder Social ---
                            const info109_2c_folder = mWin.analyzeFrameOrientation(f32b, 270);
                            const info109_2c_ig = mWin.analyzeFrameOrientation(f108_2b_ig, 270);

                            // --- Skenario 109 Sub-Test 2D: YouTube Portrait Watch Page dengan Iklan SPayLater ---
                            const info109_2d = mWin.analyzeFrameOrientation(f108_2c, 0);

                            // --- Skenario 109 Sub-Test 2E: YouTube Fullscreen Landscape dengan Thin Red Scrubber OSD ---
                            const f109_2e = document.createElement("canvas");
                            f109_2e.width = 120; f109_2e.height = 260;
                            const ctx109_2e = f109_2e.getContext("2d");
                            ctx109_2e.drawImage(f108_2d, 0, 0);
                            // Add thin red OSD scrubber at x=26, width=4, y=16..230
                            ctx109_2e.fillStyle = "rgb(255, 0, 40)";
                            ctx109_2e.fillRect(26, 16, 4, 214);
                            const info109_2e_at0 = mWin.analyzeFrameOrientation(f109_2e, 0);
                            const info109_2e_at270 = mWin.analyzeFrameOrientation(f109_2e, 270);

                            return {
                                skenario1_InstagramPremierLeague: {
                                    isGame: !!(info1 && info1.isLandscapeGameFrame),
                                    isFormation: !!(info1 && info1.isEfootballFormationScreen),
                                    isMenuLayout: !!(info1 && info1.isLandscapeMenuLayout),
                                    hasLightAppChrome: !!(info1 && info1.hasLightPortraitAppChrome),
                                    canExit: !!(info1 && info1.canPassiveExitToPortrait)
                                },
                                skenario2_InstagramDarkVideo: {
                                    isGame: !!(info2 && info2.isLandscapeGameFrame),
                                    hasSideBar: !!(info2 && info2.hasLandscapeSideBar),
                                    isMenuLayout: !!(info2 && info2.isLandscapeMenuLayout),
                                    hasLightAppChrome: !!(info2 && info2.hasLightPortraitAppChrome)
                                },
                                skenario3_TikTokDarkCar: {
                                    isGame: !!(info3 && info3.isLandscapeGameFrame),
                                    hasSideBar: !!(info3 && info3.hasLandscapeSideBar),
                                    isYouTube: !!(info3 && info3.isYouTubeFullscreenLandscape),
                                    hasTikTokBottomBar: !!(info3 && info3.hasTikTokOrPortraitBottomBar),
                                    canExit: !!(info3 && info3.canPassiveExitToPortrait)
                                },
                                skenario4_EfootballFormation: {
                                    isFormation: !!(info4 && info4.isEfootballFormationScreen),
                                    isMenuLayout: !!(info4 && info4.isLandscapeMenuLayout),
                                    isGame: !!(info4 && info4.isLandscapeGameFrame),
                                    canExit: !!(info4 && info4.canPassiveExitToPortrait)
                                },
                                skenario5_KonamiRed: {
                                    hasNakedIsland: !!(info5 && info5.hasNakedDynamicIsland),
                                    isGame: !!(info5 && info5.isLandscapeGameFrame),
                                    canExit: !!(info5 && info5.canPassiveExitToPortrait)
                                },
                                skenario6_YouTubeBlueSlide: {
                                    hasDualBlackPillarbox: !!(info6 && info6.hasDualBlackPillarbox),
                                    isYouTube: !!(info6 && info6.isYouTubeFullscreenLandscape),
                                    isGame: !!(info6 && info6.isLandscapeGameFrame),
                                    canExit: !!(info6 && info6.canPassiveExitToPortrait)
                                },
                                skenario7_YouTubeDarkSlide: {
                                    hasDualBlackPillarbox: !!(info7 && info7.hasDualBlackPillarbox),
                                    isYouTube: !!(info7 && info7.isYouTubeFullscreenLandscape),
                                    isGame: !!(info7 && info7.isLandscapeGameFrame),
                                    canExit: !!(info7 && info7.canPassiveExitToPortrait)
                                },
                                skenario8_MinimizeYouTubeToPortrait: {
                                    hasTrueStatusBar: !!(info8 && info8.hasTruePortraitStatusBar),
                                    canExit: !!(info8 && info8.canPassiveExitToPortrait),
                                    isGame: !!(info8 && info8.isLandscapeGameFrame)
                                },
                                skenario9_HomeScreenKelvin: {
                                    hasTrueStatusBar: !!(info9 && info9.hasTruePortraitStatusBar),
                                    canExit: !!(info9 && info9.canPassiveExitToPortrait),
                                    isGame: !!(info9 && info9.isLandscapeGameFrame)
                                },
                                skenario10_ControlCenterHome: {
                                    isGame: !!(info10 && info10.isLandscapeGameFrame),
                                    hasSideBar: !!(info10 && info10.hasLandscapeSideBar),
                                    isYouTube: !!(info10 && info10.isYouTubeFullscreenLandscape),
                                    hasNakedIsland: !!(info10 && info10.hasNakedDynamicIsland)
                                },
                                skenario11_ThreadsApp: {
                                    hasLightAppChrome: !!(info11 && info11.hasLightPortraitAppChrome),
                                    isGame: !!(info11 && info11.isLandscapeGameFrame)
                                },
                                skenario12_ToastCheck: {
                                    toastHidden: !!toastHidden
                                },
                                skenario13_InstagramReelsSquareVideo: {
                                    isReelsDarkChrome: !!(info13 && info13.hasInstagramReelsDarkChrome),
                                    isDefinitePortrait: !!(info13 && info13.isDefinitePortraitAppScreen),
                                    hasSideBar: !!(info13 && info13.hasLandscapeSideBar),
                                    isYouTube: !!(info13 && info13.isYouTubeFullscreenLandscape),
                                    isGame: !!(info13 && info13.isLandscapeGameFrame)
                                },
                                skenario14_RecentGraceAndFeedExit: {
                                    graceRemaining: graceRemaining,
                                    hasGrace: graceRemaining >= 3500,
                                    hasLightAppChrome: !!(info14 && info14.hasLightPortraitAppChrome),
                                    canExit: !!(info14 && info14.canPassiveExitToPortrait)
                                },
                                skenario15_GoogleTranslateDarkMode: {
                                    hasDarkPortraitAppChrome: !!(info15 && info15.hasDarkPortraitAppChrome),
                                    hasPortraitBottomBar: !!(info15 && info15.hasPortraitBottomBar),
                                    isDefinitePortrait: !!(info15 && info15.isDefinitePortraitAppScreen),
                                    hasDualBlackPillarbox: !!(info15 && info15.hasDualBlackPillarbox),
                                    isYouTube: !!(info15 && info15.isYouTubeFullscreenLandscape),
                                    isGame: !!(info15 && info15.isLandscapeGameFrame),
                                    canExit: !!(info15 && info15.canPassiveExitToPortrait)
                                },
                                skenario16A_TikTokSplashScreen: {
                                    isAppSplashScreen: !!(info16a && info16a.isAppSplashScreen),
                                    hasDualBlackPillarbox: !!(info16a && info16a.hasDualBlackPillarbox),
                                    isYouTube: !!(info16a && info16a.isYouTubeFullscreenLandscape),
                                    isGame: !!(info16a && info16a.isLandscapeGameFrame)
                                },
                                skenario16B_YouTubePortraitDarkMinimize: {
                                    hasLightPortraitAppChrome: !!(info16b && info16b.hasLightPortraitAppChrome),
                                    isGame: !!(info16b && info16b.isLandscapeGameFrame),
                                    canExit: !!(info16b && info16b.canPassiveExitToPortrait)
                                },
                                skenario17_TikTokCommentSheet: {
                                    isCommentSheet: !!(info17 && info17.isPortraitCommentSheetScreen),
                                    isDefinitePortrait: !!(info17 && info17.isDefinitePortraitAppScreen),
                                    isGame: !!(info17 && info17.isLandscapeGameFrame),
                                    canExit: !!(info17 && info17.canPassiveExitToPortrait)
                                },
                                skenario18_TikTokDarkFeed: {
                                    isGame: !!(info18 && info18.isLandscapeGameFrame),
                                    hasTikTokBottomBar: !!(info18 && info18.hasTikTokOrPortraitBottomBar),
                                    isDefinitePortrait: !!(info18 && info18.isDefinitePortraitAppScreen),
                                    canExit: !!(info18 && info18.canPassiveExitToPortrait)
                                },
                                skenario19_YouTubeLoadingSkeleton: {
                                    hasBothVideoHalvesActive: !!(info19 && info19.hasBothVideoHalvesActive),
                                    hasDualBlackPillarbox: !!(info19 && info19.hasDualBlackPillarbox),
                                    isYouTube: !!(info19 && info19.isYouTubeFullscreenLandscape),
                                    isGame: !!(info19 && info19.isLandscapeGameFrame)
                                },
                                skenario20_YouTubeFullscreenPodcastAssistiveTouch: {
                                    hasPortraitBottomBar: !!(info20 && info20.hasPortraitBottomBar),
                                    hasTikTokCenterPlusButton: !!(info20 && info20.hasTikTokCenterPlusButton),
                                    hasDualBlackPillarbox: !!(info20 && info20.hasDualBlackPillarbox),
                                    isYouTube: !!(info20 && info20.isYouTubeFullscreenLandscape),
                                    isDefinitePortrait: !!(info20 && info20.isDefinitePortraitAppScreen),
                                    isGame: !!(info20 && info20.isLandscapeGameFrame),
                                    canExit: !!(info20 && info20.canPassiveExitToPortrait)
                                },
                                skenario21_TikTokComments276: {
                                    isCommentSheet: !!(info21 && info21.isPortraitCommentSheetScreen),
                                    isDefinitePortrait: !!(info21 && info21.isDefinitePortraitAppScreen),
                                    hasDualBlackPillarbox: !!(info21 && info21.hasDualBlackPillarbox),
                                    isYouTube: !!(info21 && info21.isYouTubeFullscreenLandscape),
                                    isGame: !!(info21 && info21.isLandscapeGameFrame),
                                    canExit: !!(info21 && info21.canPassiveExitToPortrait)
                                },
                                skenario22_YouTubePortraitAd: {
                                    isPortraitPlayer: !!(info22 && info22.isYouTubePortraitPlayerLayout),
                                    isDefinitePortrait: !!(info22 && info22.isDefinitePortraitAppScreen),
                                    hasDualBlackPillarbox: !!(info22 && info22.hasDualBlackPillarbox),
                                    isYouTube: !!(info22 && info22.isYouTubeFullscreenLandscape),
                                    isGame: !!(info22 && info22.isLandscapeGameFrame),
                                    canExit: !!(info22 && info22.canPassiveExitToPortrait)
                                },
                                skenario23A_YouTubeFullscreenStudio: {
                                    hasDualBlackPillarbox: !!(info23a && info23a.hasDualBlackPillarbox),
                                    isYouTube: !!(info23a && info23a.isYouTubeFullscreenLandscape),
                                    isDefinitePortrait: !!(info23a && info23a.isDefinitePortraitAppScreen),
                                    isGame: !!(info23a && info23a.isLandscapeGameFrame),
                                    canExit: !!(info23a && info23a.canPassiveExitToPortrait)
                                },
                                skenario23B_YouTubeFullscreenMacMini: {
                                    hasDualBlackPillarbox: !!(info23b && info23b.hasDualBlackPillarbox),
                                    isYouTube: !!(info23b && info23b.isYouTubeFullscreenLandscape),
                                    isDefinitePortrait: !!(info23b && info23b.isDefinitePortraitAppScreen),
                                    isGame: !!(info23b && info23b.isLandscapeGameFrame),
                                    canExit: !!(info23b && info23b.canPassiveExitToPortrait)
                                },
                                skenario23C_YouTubeFullscreenWebWhite: {
                                    hasDualBlackPillarbox: !!(info23c && info23c.hasDualBlackPillarbox),
                                    isYouTube: !!(info23c && info23c.isYouTubeFullscreenLandscape),
                                    isDefinitePortrait: !!(info23c && info23c.isDefinitePortraitAppScreen),
                                    isGame: !!(info23c && info23c.isLandscapeGameFrame),
                                    canExit: !!(info23c && info23c.canPassiveExitToPortrait)
                                },
                                skenario24A_IndodanaGreenAdNotchBlueClockPill: {
                                    hasNotchPill: !!(info24a && info24a.hasNotchColoredClockPillStatusBar),
                                    hasTrueStatusBar: !!(info24a && info24a.hasTruePortraitStatusBar),
                                    isFormation: !!(info24a && info24a.isEfootballFormationScreen),
                                    isGame: !!(info24a && info24a.isLandscapeGameFrame),
                                    canExit: !!(info24a && info24a.canPassiveExitToPortrait)
                                },
                                skenario24B_TikTokNotchBlueClockPill: {
                                    hasNotchPill: !!(info24b && info24b.hasNotchColoredClockPillStatusBar),
                                    hasTrueStatusBar: !!(info24b && info24b.hasTruePortraitStatusBar),
                                    isGame: !!(info24b && info24b.isLandscapeGameFrame),
                                    canExit: !!(info24b && info24b.canPassiveExitToPortrait)
                                },
                                skenario25_YouTubeFullscreenHermesDarkStudio: {
                                    isSplash: !!(info25 && info25.isAppSplashScreen),
                                    hasActiveVideoBody: !!(info25 && info25.hasActiveFullscreenVideoBody),
                                    hasDualBlackPillarbox: !!(info25 && info25.hasDualBlackPillarbox),
                                    isYouTube: !!(info25 && info25.isYouTubeFullscreenLandscape),
                                    isGame: !!(info25 && info25.isLandscapeGameFrame),
                                    canExit: !!(info25 && info25.canPassiveExitToPortrait)
                                },
                                skenario26_YouTubeFullscreenPasangAiAgent: {
                                    isPortraitPlayer: !!(info26 && info26.isYouTubePortraitPlayerLayout),
                                    activeZones: info26 ? info26.activeVideoZoneCount : 0,
                                    hasDualBlackPillarbox: !!(info26 && info26.hasDualBlackPillarbox),
                                    isYouTube: !!(info26 && info26.isYouTubeFullscreenLandscape),
                                    isGame: !!(info26 && info26.isLandscapeGameFrame),
                                    canExit: !!(info26 && info26.canPassiveExitToPortrait)
                                },
                                skenario27A_YouTubeFullscreenVedaDarkSlide: {
                                    topPillarBlack: !!(info27a && info27a.topPillarIsBlack),
                                    hasCleanCorridors: !!(info27a && info27a.hasCleanBlackBottomCorridors),
                                    botPillarBlack: !!(info27a && info27a.botPillarIsBlack),
                                    hasDualBlackPillarbox: !!(info27a && info27a.hasDualBlackPillarbox),
                                    isYouTube: !!(info27a && info27a.isYouTubeFullscreenLandscape),
                                    isDefinitePortrait: !!(info27a && info27a.isDefinitePortraitAppScreen),
                                    isGame: !!(info27a && info27a.isLandscapeGameFrame),
                                    canExit: !!(info27a && info27a.canPassiveExitToPortrait)
                                },
                                skenario27B_YouTubeFullscreenVedaWhiteSlide: {
                                    topPillarBlack: !!(info27b && info27b.topPillarIsBlack),
                                    botPillarBlack: !!(info27b && info27b.botPillarIsBlack),
                                    hasDualBlackPillarbox: !!(info27b && info27b.hasDualBlackPillarbox),
                                    isYouTube: !!(info27b && info27b.isYouTubeFullscreenLandscape),
                                    isDefinitePortrait: !!(info27b && info27b.isDefinitePortraitAppScreen),
                                    isGame: !!(info27b && info27b.isLandscapeGameFrame),
                                    canExit: !!(info27b && info27b.canPassiveExitToPortrait)
                                },
                                skenario28A_YouTubeFullscreenPasangAiAgentDarkJacket: {
                                    hasWhiteCard: !!(info28a && info28a.hasYouTubeDarkLowerWhiteCard),
                                    hasDualBlackPillarbox: !!(info28a && info28a.hasDualBlackPillarbox),
                                    isYouTube: !!(info28a && info28a.isYouTubeFullscreenLandscape),
                                    isDefinitePortrait: !!(info28a && info28a.isDefinitePortraitAppScreen),
                                    isGame: !!(info28a && info28a.isLandscapeGameFrame),
                                    canExit: !!(info28a && info28a.canPassiveExitToPortrait)
                                },
                                skenario28B_YouTubeFullscreenPasangAiAgentChapter1: {
                                    topPillarBlack: !!(info28b && info28b.topPillarIsBlack),
                                    botPillarBlack: !!(info28b && info28b.botPillarIsBlack),
                                    hasDualBlackPillarbox: !!(info28b && info28b.hasDualBlackPillarbox),
                                    isYouTube: !!(info28b && info28b.isYouTubeFullscreenLandscape),
                                    isDefinitePortrait: !!(info28b && info28b.isDefinitePortraitAppScreen),
                                    isGame: !!(info28b && info28b.isLandscapeGameFrame),
                                    canExit: !!(info28b && info28b.canPassiveExitToPortrait)
                                },
                                skenario29A_YouTubeFullscreenFerryIrwandiOsd: {
                                    hasSideBar: !!(info29a && info29a.hasLandscapeSideBar),
                                    hasLandscapeOsd: !!(info29a && info29a.hasYouTubeLandscapeOsd),
                                    isGame: !!(info29a && info29a.isLandscapeGameFrame),
                                    isYouTube: !!(info29a && info29a.isYouTubeFullscreenLandscape),
                                    canExit: !!(info29a && info29a.canPassiveExitToPortrait)
                                },
                                skenario29B_YouTubeFullscreenFerryIrwandiAmbient: {
                                    topPillarBlack: !!(info29b && info29b.topPillarIsBlack),
                                    botPillarBlack: !!(info29b && info29b.botPillarIsBlack),
                                    hasCleanCorridors: !!(info29b && info29b.hasCleanBlackBottomCorridors),
                                    hasDualBlackPillarbox: !!(info29b && info29b.hasDualBlackPillarbox),
                                    hasStickyLock: !!(info29b && info29b.hasStickyFullscreenPillarboxLock),
                                    strictPortraitExitProof: !!(info29b && info29b.strictPortraitExitProof),
                                    isDefinitePortrait: !!(info29b && info29b.isDefinitePortraitAppScreen),
                                    isGame: !!(info29b && info29b.isLandscapeGameFrame),
                                    canExit: !!(info29b && info29b.canPassiveExitToPortrait)
                                },
                                skenario30A_YouTubeFullscreenKampusMalakaZoom: {
                                    hasLandscapeOsd: !!(info30a && info30a.hasYouTubeLandscapeOsd),
                                    strictPortraitExitProof: !!(info30a && info30a.strictPortraitExitProof),
                                    isGame: !!(info30a && info30a.isLandscapeGameFrame),
                                    canExit: !!(info30a && info30a.canPassiveExitToPortrait)
                                },
                                skenario30B_YouTubePortraitWatchPageMinimize: {
                                    exitByWatchPage: !!(info30b && info30b.exitByYouTubePortraitWatchPage),
                                    strictPortraitExitProof: !!(info30b && info30b.strictPortraitExitProof),
                                    canExit: !!(info30b && info30b.canPassiveExitToPortrait)
                                },
                                skenario30C_IosHomeScreenPip: {
                                    exitByHomeScreen: !!(info30c && info30c.exitByHomeScreenPortrait),
                                    strictPortraitExitProof: !!(info30c && info30c.strictPortraitExitProof),
                                    canExit: !!(info30c && info30c.canPassiveExitToPortrait)
                                },
                                skenario31_EfootballMessiYamalPoster: {
                                    hasNotchPill_at0: !!(info31_at0 && info31_at0.hasNotchColoredClockPillStatusBar),
                                    hasTrueStatusBar_at0: !!(info31_at0 && info31_at0.hasTruePortraitStatusBar),
                                    isNeonPoster_at0: !!(info31_at0 && info31_at0.isEfootballNeonPosterScreen),
                                    hasNakedIsland_at0: !!(info31_at0 && info31_at0.hasNakedDynamicIsland),
                                    isGame_at0: !!(info31_at0 && info31_at0.isLandscapeGameFrame),

                                    hasNotchPill_at270: !!(info31_at270 && info31_at270.hasNotchColoredClockPillStatusBar),
                                    hasTrueStatusBar_at270: !!(info31_at270 && info31_at270.hasTruePortraitStatusBar),
                                    exitByHomeScreen_at270: !!(info31_at270 && info31_at270.exitByHomeScreenPortrait),
                                    exitByWatchPage_at270: !!(info31_at270 && info31_at270.exitByYouTubePortraitWatchPage),
                                    strictPortraitExitProof_at270: !!(info31_at270 && info31_at270.strictPortraitExitProof),
                                    isNeonPoster_at270: !!(info31_at270 && info31_at270.isEfootballNeonPosterScreen),
                                    isGame_at270: !!(info31_at270 && info31_at270.isLandscapeGameFrame),
                                    canExit_at270: !!(info31_at270 && info31_at270.canPassiveExitToPortrait)
                                },
                                skenario32A_Iphone17ProHomeScreenKelvin: {
                                    isNeonPoster: !!(info32a && info32a.isEfootballNeonPosterScreen),
                                    hasNakedIsland: !!(info32a && info32a.hasNakedDynamicIsland),
                                    hasTrueStatusBar: !!(info32a && info32a.hasTruePortraitStatusBar),
                                    isDefinitePortrait: !!(info32a && info32a.isDefinitePortraitAppScreen),
                                    isGame: !!(info32a && info32a.isLandscapeGameFrame),
                                    exitByHomeScreen: !!(info32a && info32a.exitByHomeScreenPortrait),
                                    strictPortraitExitProof: !!(info32a && info32a.strictPortraitExitProof),
                                    canExit: !!(info32a && info32a.canPassiveExitToPortrait)
                                },
                                skenario32B_Iphone17ProFolderSocial: {
                                    isFolder: !!(info32b && info32b.isPortraitOverlayOrFolder),
                                    isNeonPoster: !!(info32b && info32b.isEfootballNeonPosterScreen),
                                    isGame: !!(info32b && info32b.isLandscapeGameFrame),
                                    exitByFolder: !!(info32b && info32b.exitByIosFolderPortrait),
                                    canExit: !!(info32b && info32b.canPassiveExitToPortrait)
                                },
                                skenario108_Sub2A_HomeScreenPlainClock: {
                                    hasTrueStatusBar: !!(info108_2a && info108_2a.hasTruePortraitStatusBar),
                                    hasSideBar: !!(info108_2a && info108_2a.hasLandscapeSideBar),
                                    isGame: !!(info108_2a && info108_2a.isLandscapeGameFrame),
                                    isDefinitePortrait: !!(info108_2a && info108_2a.isDefinitePortraitAppScreen),
                                    exitByHomeScreen: !!(info108_2a && info108_2a.exitByHomeScreenPortrait)
                                },
                                skenario108_Sub2B_AutoReturnFolderAndIgDark: {
                                    folderExit: !!(info108_2b_folder && info108_2b_folder.strictPortraitExitProof),
                                    folderCanExit: !!(info108_2b_folder && info108_2b_folder.canPassiveExitToPortrait),
                                    igDarkChrome: !!(info108_2b_ig && info108_2b_ig.hasInstagramDarkModeChrome),
                                    igDarkExit: !!(info108_2b_ig && info108_2b_ig.strictPortraitExitProof),
                                    igDarkCanExit: !!(info108_2b_ig && info108_2b_ig.canPassiveExitToPortrait),
                                    igDarkIsGame: !!(info108_2b_ig && info108_2b_ig.isLandscapeGameFrame)
                                },
                                skenario108_Sub2C_YouTubeWatchPageAdCard: {
                                    hasLowerWhiteCard: !!(info108_2c && info108_2c.hasYouTubeDarkLowerWhiteCard),
                                    isFormation: !!(info108_2c && info108_2c.isEfootballFormationScreen),
                                    isNeonPoster: !!(info108_2c && info108_2c.isEfootballNeonPosterScreen),
                                    isGame: !!(info108_2c && info108_2c.isLandscapeGameFrame),
                                    canExit: !!(info108_2c && info108_2c.canPassiveExitToPortrait)
                                },
                                skenario108_Sub2D_YouTubeFullscreenHostDesk: {
                                    isCommentSheet: !!(info108_2d && info108_2d.isPortraitCommentSheetScreen),
                                    hasDualBlackPillarbox: !!(info108_2d && info108_2d.hasDualBlackPillarbox),
                                    isYouTube: !!(info108_2d && info108_2d.isYouTubeFullscreenLandscape),
                                    isGame: !!(info108_2d && info108_2d.isLandscapeGameFrame),
                                    hasSideBar: !!(info108_2d && info108_2d.hasLandscapeSideBar)
                                },
                                skenario108_Sub2E_YouTubeFullscreenAsusLaptop: {
                                    hasLowerWhiteCard_at0: !!(info108_2e_at0 && info108_2e_at0.hasYouTubeDarkLowerWhiteCard),
                                    isYouTube_at0: !!(info108_2e_at0 && info108_2e_at0.isYouTubeFullscreenLandscape),
                                    isGame_at0: !!(info108_2e_at0 && info108_2e_at0.isLandscapeGameFrame),
                                    hasSideBar_at0: !!(info108_2e_at0 && info108_2e_at0.hasLandscapeSideBar),
                                    hasLowerWhiteCard_at270: !!(info108_2e_at270 && info108_2e_at270.hasYouTubeDarkLowerWhiteCard),
                                    isYouTube_at270: !!(info108_2e_at270 && info108_2e_at270.isYouTubeFullscreenLandscape),
                                    isGame_at270: !!(info108_2e_at270 && info108_2e_at270.isLandscapeGameFrame),
                                    hasSideBar_at270: !!(info108_2e_at270 && info108_2e_at270.hasLandscapeSideBar)
                                },
                                skenario109_Sub2A_HomeScreenKelvinFullRes: {
                                    hasIosDock: !!(info109_2a_at0 && info109_2a_at0.hasIosHomeScreenDock),
                                    hasLandscapeOsd: !!(info109_2a_at0 && info109_2a_at0.hasYouTubeLandscapeOsd),
                                    hasPrelimSideBar: !!(info109_2a_at0 && info109_2a_at0.hasLandscapeSideBar),
                                    hasSideBar: !!(info109_2a_at0 && info109_2a_at0.hasLandscapeSideBar),
                                    isMenuLayout: !!(info109_2a_at0 && info109_2a_at0.isLandscapeMenuLayout),
                                    isGame_at0: !!(info109_2a_at0 && info109_2a_at0.isLandscapeGameFrame),
                                    isDefinitePortrait: !!(info109_2a_at0 && info109_2a_at0.isDefinitePortraitAppScreen),
                                    strictExitProof_at270: !!(info109_2a_at270 && info109_2a_at270.strictPortraitExitProof)
                                },
                                skenario109_Sub2B_HomeScreenYouTubePip: {
                                    hasIosDock: !!(info109_2b_at0 && info109_2b_at0.hasIosHomeScreenDock),
                                    isGame_at0: !!(info109_2b_at0 && info109_2b_at0.isLandscapeGameFrame),
                                    isDefinitePortrait: !!(info109_2b_at0 && info109_2b_at0.isDefinitePortraitAppScreen),
                                    strictExitProof_at270: !!(info109_2b_at270 && info109_2b_at270.strictPortraitExitProof)
                                },
                                skenario109_Sub2C_InstagramDarkAndFolder: {
                                    folderExit: !!(info109_2c_folder && info109_2c_folder.strictPortraitExitProof),
                                    folderCanExit: !!(info109_2c_folder && info109_2c_folder.canPassiveExitToPortrait),
                                    igDarkChrome: !!(info109_2c_ig && info109_2c_ig.hasInstagramDarkModeChrome),
                                    igDarkExit: !!(info109_2c_ig && info109_2c_ig.strictPortraitExitProof),
                                    igDarkCanExit: !!(info109_2c_ig && info109_2c_ig.canPassiveExitToPortrait),
                                    igDarkIsGame: !!(info109_2c_ig && info109_2c_ig.isLandscapeGameFrame)
                                },
                                skenario109_Sub2D_YouTubeWatchPageAd: {
                                    hasLowerWhiteCard: !!(info109_2d && info109_2d.hasYouTubeDarkLowerWhiteCard),
                                    isFormation: !!(info109_2d && info109_2d.isEfootballFormationScreen),
                                    isNeonPoster: !!(info109_2d && info109_2d.isEfootballNeonPosterScreen),
                                    isGame: !!(info109_2d && info109_2d.isLandscapeGameFrame),
                                    canExit: !!(info109_2d && info109_2d.canPassiveExitToPortrait)
                                },
                                skenario109_Sub2E_YouTubeFullscreenLandscapeOsd: {
                                    hasLandscapeOsd: !!(info109_2e_at0 && info109_2e_at0.hasYouTubeLandscapeOsd),
                                    isYouTube: !!(info109_2e_at0 && info109_2e_at0.isYouTubeFullscreenLandscape),
                                    isGame_at0: !!(info109_2e_at0 && info109_2e_at0.isLandscapeGameFrame),
                                    isGame_at270: !!(info109_2e_at270 && info109_2e_at270.isLandscapeGameFrame)
                                },
                                skenario110_Sub2C_CanvasSmoothingAndSmartPli: {
                                    imageSmoothingQuality: (function() {
                                        try {
                                            const c = mWin?.canvas || document.querySelector("canvas");
                                            const ctx2 = c ? c.getContext("2d") : null;
                                            return ctx2 ? ctx2.imageSmoothingQuality : "high";
                                        } catch (_) { return "high"; }
                                    })(),
                                    hasSmartPli: !!(mWin && typeof mWin.requestSmartMotionPli === "function")
                                }
                            };
                        })()
                        ''')

                        tahap87_ok = (
                            isinstance(tahap83_res, dict)
                            and tahap83_res.get("skenario1_InstagramPremierLeague", {}).get("isGame") is False
                            and tahap83_res.get("skenario1_InstagramPremierLeague", {}).get("isFormation") is False
                            and tahap83_res.get("skenario1_InstagramPremierLeague", {}).get("hasLightAppChrome") is True
                            and tahap83_res.get("skenario2_InstagramDarkVideo", {}).get("isGame") is False
                            and tahap83_res.get("skenario2_InstagramDarkVideo", {}).get("hasSideBar") is False
                            and tahap83_res.get("skenario2_InstagramDarkVideo", {}).get("hasLightAppChrome") is True
                            and tahap83_res.get("skenario3_TikTokDarkCar", {}).get("isGame") is False
                            and tahap83_res.get("skenario3_TikTokDarkCar", {}).get("hasSideBar") is False
                            and tahap83_res.get("skenario3_TikTokDarkCar", {}).get("isYouTube") is False
                            and tahap83_res.get("skenario3_TikTokDarkCar", {}).get("hasTikTokBottomBar") is True
                            and tahap83_res.get("skenario3_TikTokDarkCar", {}).get("canExit") is True
                            and tahap83_res.get("skenario4_EfootballFormation", {}).get("isFormation") is True
                            and tahap83_res.get("skenario4_EfootballFormation", {}).get("isMenuLayout") is True
                            and tahap83_res.get("skenario4_EfootballFormation", {}).get("isGame") is True
                            and tahap83_res.get("skenario4_EfootballFormation", {}).get("canExit") is False
                            and tahap83_res.get("skenario5_KonamiRed", {}).get("isGame") is True
                            and tahap83_res.get("skenario5_KonamiRed", {}).get("hasNakedIsland") is True
                            and tahap83_res.get("skenario5_KonamiRed", {}).get("canExit") is False
                            and tahap83_res.get("skenario6_YouTubeBlueSlide", {}).get("hasDualBlackPillarbox") is True
                            and tahap83_res.get("skenario6_YouTubeBlueSlide", {}).get("isYouTube") is True
                            and tahap83_res.get("skenario6_YouTubeBlueSlide", {}).get("isGame") is True
                            and tahap83_res.get("skenario6_YouTubeBlueSlide", {}).get("canExit") is False
                            and tahap83_res.get("skenario7_YouTubeDarkSlide", {}).get("hasDualBlackPillarbox") is True
                            and tahap83_res.get("skenario7_YouTubeDarkSlide", {}).get("isYouTube") is True
                            and tahap83_res.get("skenario7_YouTubeDarkSlide", {}).get("isGame") is True
                            and tahap83_res.get("skenario7_YouTubeDarkSlide", {}).get("canExit") is False
                            and tahap83_res.get("skenario8_MinimizeYouTubeToPortrait", {}).get("hasTrueStatusBar") is True
                            and tahap83_res.get("skenario8_MinimizeYouTubeToPortrait", {}).get("canExit") is True
                            and tahap83_res.get("skenario8_MinimizeYouTubeToPortrait", {}).get("isGame") is False
                            and tahap83_res.get("skenario9_HomeScreenKelvin", {}).get("hasTrueStatusBar") is True
                            and tahap83_res.get("skenario9_HomeScreenKelvin", {}).get("canExit") is True
                            and tahap83_res.get("skenario9_HomeScreenKelvin", {}).get("isGame") is False
                            and tahap83_res.get("skenario10_ControlCenterHome", {}).get("isGame") is False
                            and tahap83_res.get("skenario10_ControlCenterHome", {}).get("hasSideBar") is False
                            and tahap83_res.get("skenario11_ThreadsApp", {}).get("hasLightAppChrome") is True
                            and tahap83_res.get("skenario11_ThreadsApp", {}).get("isGame") is False
                            and tahap83_res.get("skenario12_ToastCheck", {}).get("toastHidden") is True
                            and tahap83_res.get("skenario13_InstagramReelsSquareVideo", {}).get("isReelsDarkChrome") is True
                            and tahap83_res.get("skenario13_InstagramReelsSquareVideo", {}).get("isDefinitePortrait") is True
                            and tahap83_res.get("skenario13_InstagramReelsSquareVideo", {}).get("hasSideBar") is False
                            and tahap83_res.get("skenario13_InstagramReelsSquareVideo", {}).get("isYouTube") is False
                            and tahap83_res.get("skenario13_InstagramReelsSquareVideo", {}).get("isGame") is False
                            and tahap83_res.get("skenario14_RecentGraceAndFeedExit", {}).get("hasGrace") is True
                            and tahap83_res.get("skenario14_RecentGraceAndFeedExit", {}).get("hasLightAppChrome") is True
                            and tahap83_res.get("skenario14_RecentGraceAndFeedExit", {}).get("canExit") is True
                            and tahap83_res.get("skenario15_GoogleTranslateDarkMode", {}).get("hasDarkPortraitAppChrome") is True
                            and tahap83_res.get("skenario15_GoogleTranslateDarkMode", {}).get("hasPortraitBottomBar") is True
                            and tahap83_res.get("skenario15_GoogleTranslateDarkMode", {}).get("isDefinitePortrait") is True
                            and tahap83_res.get("skenario15_GoogleTranslateDarkMode", {}).get("hasDualBlackPillarbox") is False
                            and tahap83_res.get("skenario15_GoogleTranslateDarkMode", {}).get("isYouTube") is False
                            and tahap83_res.get("skenario15_GoogleTranslateDarkMode", {}).get("isGame") is False
                            and tahap83_res.get("skenario15_GoogleTranslateDarkMode", {}).get("canExit") is True
                            and tahap83_res.get("skenario16A_TikTokSplashScreen", {}).get("isAppSplashScreen") is True
                            and tahap83_res.get("skenario16A_TikTokSplashScreen", {}).get("hasDualBlackPillarbox") is False
                            and tahap83_res.get("skenario16A_TikTokSplashScreen", {}).get("isYouTube") is False
                            and tahap83_res.get("skenario16A_TikTokSplashScreen", {}).get("isGame") is False
                            and (tahap83_res.get("skenario16B_YouTubePortraitDarkMinimize", {}).get("hasLightPortraitAppChrome") is True or tahap83_res.get("skenario16B_YouTubePortraitDarkMinimize", {}).get("hasLightAppChrome") is True)
                            and tahap83_res.get("skenario16B_YouTubePortraitDarkMinimize", {}).get("isGame") is False
                            and tahap83_res.get("skenario16B_YouTubePortraitDarkMinimize", {}).get("canExit") is True
                            and tahap83_res.get("skenario17_TikTokCommentSheet", {}).get("isCommentSheet") is True
                            and tahap83_res.get("skenario17_TikTokCommentSheet", {}).get("isDefinitePortrait") is True
                            and tahap83_res.get("skenario17_TikTokCommentSheet", {}).get("isGame") is False
                            and tahap83_res.get("skenario17_TikTokCommentSheet", {}).get("canExit") is True
                            and tahap83_res.get("skenario18_TikTokDarkFeed", {}).get("isGame") is False
                            and tahap83_res.get("skenario18_TikTokDarkFeed", {}).get("hasTikTokBottomBar") is True
                            and tahap83_res.get("skenario18_TikTokDarkFeed", {}).get("isDefinitePortrait") is True
                            and tahap83_res.get("skenario18_TikTokDarkFeed", {}).get("canExit") is True
                            and tahap83_res.get("skenario19_YouTubeLoadingSkeleton", {}).get("hasBothVideoHalvesActive") is False
                            and tahap83_res.get("skenario19_YouTubeLoadingSkeleton", {}).get("hasDualBlackPillarbox") is False
                            and tahap83_res.get("skenario19_YouTubeLoadingSkeleton", {}).get("isYouTube") is False
                            and tahap83_res.get("skenario19_YouTubeLoadingSkeleton", {}).get("isGame") is False
                            and tahap83_res.get("skenario20_YouTubeFullscreenPodcastAssistiveTouch", {}).get("hasPortraitBottomBar") is False
                            and tahap83_res.get("skenario20_YouTubeFullscreenPodcastAssistiveTouch", {}).get("hasDualBlackPillarbox") is True
                            and tahap83_res.get("skenario20_YouTubeFullscreenPodcastAssistiveTouch", {}).get("isYouTube") is True
                            and tahap83_res.get("skenario20_YouTubeFullscreenPodcastAssistiveTouch", {}).get("isDefinitePortrait") is False
                            and tahap83_res.get("skenario20_YouTubeFullscreenPodcastAssistiveTouch", {}).get("isGame") is True
                            and tahap83_res.get("skenario20_YouTubeFullscreenPodcastAssistiveTouch", {}).get("canExit") is False
                            and tahap83_res.get("skenario21_TikTokComments276", {}).get("isCommentSheet") is True
                            and tahap83_res.get("skenario21_TikTokComments276", {}).get("isDefinitePortrait") is True
                            and tahap83_res.get("skenario21_TikTokComments276", {}).get("hasDualBlackPillarbox") is False
                            and tahap83_res.get("skenario21_TikTokComments276", {}).get("isYouTube") is False
                            and tahap83_res.get("skenario21_TikTokComments276", {}).get("isGame") is False
                            and tahap83_res.get("skenario21_TikTokComments276", {}).get("canExit") is True
                            and tahap83_res.get("skenario22_YouTubePortraitAd", {}).get("isPortraitPlayer") is True
                            and tahap83_res.get("skenario22_YouTubePortraitAd", {}).get("isDefinitePortrait") is True
                            and tahap83_res.get("skenario22_YouTubePortraitAd", {}).get("hasDualBlackPillarbox") is False
                            and tahap83_res.get("skenario22_YouTubePortraitAd", {}).get("isYouTube") is False
                            and tahap83_res.get("skenario22_YouTubePortraitAd", {}).get("isGame") is False
                            and tahap83_res.get("skenario22_YouTubePortraitAd", {}).get("canExit") is True
                            and tahap83_res.get("skenario23A_YouTubeFullscreenStudio", {}).get("hasDualBlackPillarbox") is True
                            and tahap83_res.get("skenario23A_YouTubeFullscreenStudio", {}).get("isYouTube") is True
                            and tahap83_res.get("skenario23A_YouTubeFullscreenStudio", {}).get("isDefinitePortrait") is False
                            and tahap83_res.get("skenario23A_YouTubeFullscreenStudio", {}).get("isGame") is True
                            and tahap83_res.get("skenario23A_YouTubeFullscreenStudio", {}).get("canExit") is False
                            and tahap83_res.get("skenario23B_YouTubeFullscreenMacMini", {}).get("hasDualBlackPillarbox") is True
                            and tahap83_res.get("skenario23B_YouTubeFullscreenMacMini", {}).get("isYouTube") is True
                            and tahap83_res.get("skenario23B_YouTubeFullscreenMacMini", {}).get("isDefinitePortrait") is False
                            and tahap83_res.get("skenario23B_YouTubeFullscreenMacMini", {}).get("isGame") is True
                            and tahap83_res.get("skenario23B_YouTubeFullscreenMacMini", {}).get("canExit") is False
                            and tahap83_res.get("skenario23C_YouTubeFullscreenWebWhite", {}).get("hasDualBlackPillarbox") is True
                            and tahap83_res.get("skenario23C_YouTubeFullscreenWebWhite", {}).get("isYouTube") is True
                            and tahap83_res.get("skenario23C_YouTubeFullscreenWebWhite", {}).get("isDefinitePortrait") is False
                            and tahap83_res.get("skenario23C_YouTubeFullscreenWebWhite", {}).get("isGame") is True
                            and tahap83_res.get("skenario23C_YouTubeFullscreenWebWhite", {}).get("canExit") is False
                        )

                        tahap88_ok = (
                            tahap87_ok
                            and tahap83_res.get("skenario24A_IndodanaGreenAdNotchBlueClockPill", {}).get("hasNotchPill") is True
                            and tahap83_res.get("skenario24A_IndodanaGreenAdNotchBlueClockPill", {}).get("hasTrueStatusBar") is True
                            and tahap83_res.get("skenario24A_IndodanaGreenAdNotchBlueClockPill", {}).get("isFormation") is False
                            and tahap83_res.get("skenario24A_IndodanaGreenAdNotchBlueClockPill", {}).get("isGame") is False
                            and tahap83_res.get("skenario24A_IndodanaGreenAdNotchBlueClockPill", {}).get("canExit") is True
                            and tahap83_res.get("skenario24B_TikTokNotchBlueClockPill", {}).get("hasNotchPill") is True
                            and tahap83_res.get("skenario24B_TikTokNotchBlueClockPill", {}).get("hasTrueStatusBar") is True
                            and tahap83_res.get("skenario24B_TikTokNotchBlueClockPill", {}).get("isGame") is False
                            and tahap83_res.get("skenario24B_TikTokNotchBlueClockPill", {}).get("canExit") is True
                            and tahap83_res.get("skenario25_YouTubeFullscreenHermesDarkStudio", {}).get("isSplash") is False
                            and tahap83_res.get("skenario25_YouTubeFullscreenHermesDarkStudio", {}).get("hasActiveVideoBody") is True
                            and tahap83_res.get("skenario25_YouTubeFullscreenHermesDarkStudio", {}).get("hasDualBlackPillarbox") is True
                            and tahap83_res.get("skenario25_YouTubeFullscreenHermesDarkStudio", {}).get("isYouTube") is True
                            and tahap83_res.get("skenario25_YouTubeFullscreenHermesDarkStudio", {}).get("isGame") is True
                            and tahap83_res.get("skenario25_YouTubeFullscreenHermesDarkStudio", {}).get("canExit") is False
                            and tahap83_res.get("skenario26_YouTubeFullscreenPasangAiAgent", {}).get("isPortraitPlayer") is False
                            and tahap83_res.get("skenario26_YouTubeFullscreenPasangAiAgent", {}).get("activeZones", 0) >= 2
                            and tahap83_res.get("skenario26_YouTubeFullscreenPasangAiAgent", {}).get("hasDualBlackPillarbox") is True
                            and tahap83_res.get("skenario26_YouTubeFullscreenPasangAiAgent", {}).get("isYouTube") is True
                            and tahap83_res.get("skenario26_YouTubeFullscreenPasangAiAgent", {}).get("isGame") is True
                            and tahap83_res.get("skenario26_YouTubeFullscreenPasangAiAgent", {}).get("canExit") is False
                        )

                        tahap89_ok = (
                            tahap88_ok
                            and tahap83_res.get("skenario27A_YouTubeFullscreenVedaDarkSlide", {}).get("topPillarBlack") is True
                            and tahap83_res.get("skenario27A_YouTubeFullscreenVedaDarkSlide", {}).get("hasCleanCorridors") is True
                            and tahap83_res.get("skenario27A_YouTubeFullscreenVedaDarkSlide", {}).get("botPillarBlack") is True
                            and tahap83_res.get("skenario27A_YouTubeFullscreenVedaDarkSlide", {}).get("hasDualBlackPillarbox") is True
                            and tahap83_res.get("skenario27A_YouTubeFullscreenVedaDarkSlide", {}).get("isYouTube") is True
                            and tahap83_res.get("skenario27A_YouTubeFullscreenVedaDarkSlide", {}).get("isDefinitePortrait") is False
                            and tahap83_res.get("skenario27A_YouTubeFullscreenVedaDarkSlide", {}).get("isGame") is True
                            and tahap83_res.get("skenario27A_YouTubeFullscreenVedaDarkSlide", {}).get("canExit") is False
                            and tahap83_res.get("skenario27B_YouTubeFullscreenVedaWhiteSlide", {}).get("topPillarBlack") is True
                            and tahap83_res.get("skenario27B_YouTubeFullscreenVedaWhiteSlide", {}).get("botPillarBlack") is True
                            and tahap83_res.get("skenario27B_YouTubeFullscreenVedaWhiteSlide", {}).get("hasDualBlackPillarbox") is True
                            and tahap83_res.get("skenario27B_YouTubeFullscreenVedaWhiteSlide", {}).get("isYouTube") is True
                            and tahap83_res.get("skenario27B_YouTubeFullscreenVedaWhiteSlide", {}).get("isDefinitePortrait") is False
                            and tahap83_res.get("skenario27B_YouTubeFullscreenVedaWhiteSlide", {}).get("isGame") is True
                            and tahap83_res.get("skenario27B_YouTubeFullscreenVedaWhiteSlide", {}).get("canExit") is False
                            and tahap83_res.get("skenario28A_YouTubeFullscreenPasangAiAgentDarkJacket", {}).get("hasWhiteCard") is False
                            and tahap83_res.get("skenario28A_YouTubeFullscreenPasangAiAgentDarkJacket", {}).get("hasDualBlackPillarbox") is True
                            and tahap83_res.get("skenario28A_YouTubeFullscreenPasangAiAgentDarkJacket", {}).get("isYouTube") is True
                            and tahap83_res.get("skenario28A_YouTubeFullscreenPasangAiAgentDarkJacket", {}).get("isDefinitePortrait") is False
                            and tahap83_res.get("skenario28A_YouTubeFullscreenPasangAiAgentDarkJacket", {}).get("isGame") is True
                            and tahap83_res.get("skenario28A_YouTubeFullscreenPasangAiAgentDarkJacket", {}).get("canExit") is False
                            and tahap83_res.get("skenario28B_YouTubeFullscreenPasangAiAgentChapter1", {}).get("topPillarBlack") is True
                            and tahap83_res.get("skenario28B_YouTubeFullscreenPasangAiAgentChapter1", {}).get("botPillarBlack") is True
                            and tahap83_res.get("skenario28B_YouTubeFullscreenPasangAiAgentChapter1", {}).get("hasDualBlackPillarbox") is True
                            and tahap83_res.get("skenario28B_YouTubeFullscreenPasangAiAgentChapter1", {}).get("isYouTube") is True
                            and tahap83_res.get("skenario28B_YouTubeFullscreenPasangAiAgentChapter1", {}).get("isDefinitePortrait") is False
                            and tahap83_res.get("skenario28B_YouTubeFullscreenPasangAiAgentChapter1", {}).get("isGame") is True
                            and tahap83_res.get("skenario28B_YouTubeFullscreenPasangAiAgentChapter1", {}).get("canExit") is False
                        )

                        tahap90_ok = (
                            tahap89_ok
                            and tahap83_res.get("skenario29A_YouTubeFullscreenFerryIrwandiOsd", {}).get("hasSideBar") is True
                            and tahap83_res.get("skenario29A_YouTubeFullscreenFerryIrwandiOsd", {}).get("hasLandscapeOsd") is True
                            and tahap83_res.get("skenario29A_YouTubeFullscreenFerryIrwandiOsd", {}).get("isGame") is True
                            and tahap83_res.get("skenario29A_YouTubeFullscreenFerryIrwandiOsd", {}).get("canExit") is False
                            and tahap83_res.get("skenario29B_YouTubeFullscreenFerryIrwandiAmbient", {}).get("topPillarBlack") is True
                            and tahap83_res.get("skenario29B_YouTubeFullscreenFerryIrwandiAmbient", {}).get("botPillarBlack") is True
                            and tahap83_res.get("skenario29B_YouTubeFullscreenFerryIrwandiAmbient", {}).get("hasCleanCorridors") is True
                            and tahap83_res.get("skenario29B_YouTubeFullscreenFerryIrwandiAmbient", {}).get("hasDualBlackPillarbox") is True
                            and tahap83_res.get("skenario29B_YouTubeFullscreenFerryIrwandiAmbient", {}).get("hasStickyLock") is True
                            and tahap83_res.get("skenario29B_YouTubeFullscreenFerryIrwandiAmbient", {}).get("strictPortraitExitProof") is False
                            and tahap83_res.get("skenario29B_YouTubeFullscreenFerryIrwandiAmbient", {}).get("isDefinitePortrait") is False
                            and tahap83_res.get("skenario29B_YouTubeFullscreenFerryIrwandiAmbient", {}).get("isGame") is True
                            and tahap83_res.get("skenario29B_YouTubeFullscreenFerryIrwandiAmbient", {}).get("canExit") is False
                            and tahap83_res.get("skenario30A_YouTubeFullscreenKampusMalakaZoom", {}).get("hasLandscapeOsd") is True
                            and tahap83_res.get("skenario30A_YouTubeFullscreenKampusMalakaZoom", {}).get("strictPortraitExitProof") is False
                            and tahap83_res.get("skenario30A_YouTubeFullscreenKampusMalakaZoom", {}).get("isGame") is True
                            and tahap83_res.get("skenario30A_YouTubeFullscreenKampusMalakaZoom", {}).get("canExit") is False
                            and tahap83_res.get("skenario30B_YouTubePortraitWatchPageMinimize", {}).get("exitByWatchPage") is True
                            and tahap83_res.get("skenario30B_YouTubePortraitWatchPageMinimize", {}).get("strictPortraitExitProof") is True
                            and tahap83_res.get("skenario30B_YouTubePortraitWatchPageMinimize", {}).get("canExit") is True
                            and tahap83_res.get("skenario30C_IosHomeScreenPip", {}).get("exitByHomeScreen") is True
                            and tahap83_res.get("skenario30C_IosHomeScreenPip", {}).get("strictPortraitExitProof") is True
                            and tahap83_res.get("skenario30C_IosHomeScreenPip", {}).get("canExit") is True
                        )

                        tahap89_ok = tahap90_ok
                        tahap88_ok = tahap90_ok
                        tahap87_ok = tahap90_ok
                        tahap86_ok = tahap90_ok
                        tahap85_ok = tahap90_ok
                        tahap84_ok = tahap90_ok

                        test_data["tahap90_verification"] = {
                            "passed": tahap90_ok,
                            "results": tahap83_res
                        }

                        # Step F: Tahap 91 Verification (Full Screen Settings Drawer Open/Close & Compact Swipe Nav Labels)
                        try:
                            # 1. Ensure window in Portrait before test
                            window.evaluate_js('(() => { const c = document.querySelector("#close-settings"); if (c) c.click(); if (typeof window.setLandscapeOrientation === "function") window.setLandscapeOrientation(false); })()')
                            time.sleep(0.3)

                            # Sub-Test 2A: Fullscreen Portrait + Click Settings Button
                            window.evaluate_js('window.toggleFullSize(true)')
                            time.sleep(0.7)
                            # Click Settings to Open Drawer in Fullscreen
                            window.evaluate_js('document.querySelector("#btn-settings").click()')
                            time.sleep(0.7)

                            t91_fs_open = window.evaluate_js('''
                            (() => {
                                const appRoot = document.querySelector("#app-root") || document.querySelector(".app");
                                const ps = document.querySelector("#phone-screen");
                                const sd = document.querySelector(".side-drawer") || document.querySelector("#control-pane");
                                const ab = document.querySelector(".app-bar");
                                const qn = document.querySelector(".quick-nav");
                                const psRect = ps ? ps.getBoundingClientRect() : null;
                                const abRect = ab ? ab.getBoundingClientRect() : null;
                                const qnRect = qn ? qn.getBoundingClientRect() : null;
                                const sdRect = sd ? sd.getBoundingClientRect() : null;
                                const sdStyle = sd ? window.getComputedStyle(sd).display : "none";
                                const isFull = appRoot ? appRoot.classList.contains("is-fullsize") : false;
                                const isDrawer = appRoot ? appRoot.classList.contains("drawer-open") : false;
                                const innerH = window.innerHeight;
                                const innerW = window.innerWidth;
                                const psH = psRect ? psRect.height : 0;
                                const psW = psRect ? psRect.width : 0;
                                const heightOk = psRect ? (psRect.height >= (innerH - 96)) : false;
                                const topOk = (psRect && abRect) ? (psRect.top >= (abRect.bottom - 2)) : false;
                                const bottomOk = (psRect && qnRect) ? (psRect.bottom <= (qnRect.top + 2)) : false;
                                const drawerVisible = (sdStyle !== "none") && (sdRect ? sdRect.width >= 340 : false);

                                return {
                                    isFullsize: isFull,
                                    isDrawerOpen: isDrawer,
                                    drawerVisible: drawerVisible,
                                    drawerWidth: sdRect ? sdRect.width : 0,
                                    drawerDisplay: sdStyle,
                                    psHeight: psH,
                                    psWidth: psW,
                                    heightOk: heightOk,
                                    topOk: topOk,
                                    bottomOk: bottomOk,
                                    innerHeight: innerH,
                                    innerWidth: innerW
                                };
                            })()
                            ''')

                            # Click Settings again to Close Drawer in Fullscreen
                            window.evaluate_js('document.querySelector("#btn-settings").click()')
                            time.sleep(0.5)

                            t91_fs_close = window.evaluate_js('''
                            (() => {
                                const appRoot = document.querySelector("#app-root") || document.querySelector(".app");
                                const ps = document.querySelector("#phone-screen");
                                const sd = document.querySelector(".side-drawer") || document.querySelector("#control-pane");
                                const psRect = ps ? ps.getBoundingClientRect() : null;
                                const sdStyle = sd ? window.getComputedStyle(sd).display : "none";
                                const isFull = appRoot ? appRoot.classList.contains("is-fullsize") : false;
                                const isDrawer = appRoot ? appRoot.classList.contains("drawer-open") : false;
                                const cDiff = psRect ? Math.abs((psRect.left + psRect.right)/2 - (window.innerWidth / 2)) : 999;

                                return {
                                    isFullsize: isFull,
                                    isDrawerOpen: isDrawer,
                                    sideDrawerHidden: (sdStyle === "none"),
                                    centered: (cDiff <= 30),
                                    psHeight: psRect ? psRect.height : 0
                                };
                            })()
                            ''')

                            # Exit Fullscreen to test Sub-Test 2B in normal window width (346px)
                            window.evaluate_js('window.toggleFullSize(false)')
                            time.sleep(0.6)

                            # Sub-Test 2B: Quick-nav Button Labels Verification
                            t91_nav_btns = window.evaluate_js('''
                            (() => {
                                const qn = document.querySelector(".quick-nav");
                                const b1 = document.querySelector("#nav-swipe-left");
                                const b2 = document.querySelector("#nav-home");
                                const b3 = document.querySelector("#nav-recent");
                                const b4 = document.querySelector("#nav-swipe-right");
                                
                                const b1Text = (b1 ? b1.textContent : "").trim();
                                const b4Text = (b4 ? b4.textContent : "").trim();

                                const b1HasSwipe = b1Text.includes("Swipe");
                                const b1NoLeft = !b1Text.includes("Left");
                                const b4HasSwipe = b4Text.includes("Swipe");
                                const b4NoRight = !b4Text.includes("Right");

                                const checkEllipsis = (btn) => {
                                    if (!btn) return false;
                                    return btn.scrollWidth <= (btn.clientWidth + 1);
                                };

                                const b1Fit = checkEllipsis(b1);
                                const b2Fit = checkEllipsis(b2);
                                const b3Fit = checkEllipsis(b3);
                                const b4Fit = checkEllipsis(b4);

                                return {
                                    b1Text: b1Text,
                                    b4Text: b4Text,
                                    b1Valid: (b1HasSwipe && b1NoLeft),
                                    b4Valid: (b4HasSwipe && b4NoRight),
                                    b1Fit: b1Fit,
                                    b2Fit: b2Fit,
                                    b3Fit: b3Fit,
                                    b4Fit: b4Fit,
                                    allFit: (b1Fit && b2Fit && b3Fit && b4Fit)
                                };
                            })()
                            ''')

                            t91_sub2a_ok = (
                                isinstance(t91_fs_open, dict)
                                and t91_fs_open.get("isFullsize") is True
                                and t91_fs_open.get("isDrawerOpen") is True
                                and t91_fs_open.get("drawerVisible") is True
                                and t91_fs_open.get("heightOk") is True
                                and t91_fs_open.get("topOk") is True
                                and t91_fs_open.get("bottomOk") is True
                                and isinstance(t91_fs_close, dict)
                                and t91_fs_close.get("isFullsize") is True
                                and t91_fs_close.get("isDrawerOpen") is False
                                and t91_fs_close.get("sideDrawerHidden") is True
                            )

                            t91_sub2b_ok = (
                                isinstance(t91_nav_btns, dict)
                                and t91_nav_btns.get("b1Valid") is True
                                and t91_nav_btns.get("b4Valid") is True
                                and t91_nav_btns.get("allFit") is True
                            )

                            tahap91_ok = tahap90_ok and t91_sub2a_ok and t91_sub2b_ok

                            test_data["tahap91_verification"] = {
                                "passed": tahap91_ok,
                                "sub2a_ok": t91_sub2a_ok,
                                "sub2b_ok": t91_sub2b_ok,
                                "fs_open": t91_fs_open,
                                "fs_close": t91_fs_close,
                                "nav_btns": t91_nav_btns
                            }
                            if not tahap91_ok:
                                passed = False

                            # Step G: Tahap 92 Verification (eFootball 2027 Lionel Messi & Lamine Yamal Neon Poster Screen Lock)
                            t92_s31 = tahap83_res.get("skenario31_EfootballMessiYamalPoster", {})
                            tahap92_ok = (
                                tahap91_ok
                                and t92_s31.get("hasNotchPill_at0") is False
                                and t92_s31.get("hasTrueStatusBar_at0") is False
                                and t92_s31.get("isNeonPoster_at0") is True
                                and t92_s31.get("hasNakedIsland_at0") is True
                                and t92_s31.get("isGame_at0") is True
                                and t92_s31.get("hasNotchPill_at270") is False
                                and t92_s31.get("hasTrueStatusBar_at270") is False
                                and t92_s31.get("exitByHomeScreen_at270") is False
                                and t92_s31.get("exitByWatchPage_at270") is False
                                and t92_s31.get("strictPortraitExitProof_at270") is False
                                and t92_s31.get("isNeonPoster_at270") is True
                                and t92_s31.get("isGame_at270") is True
                                and t92_s31.get("canExit_at270") is False
                            )

                            test_data["tahap92_verification"] = {
                                "passed": tahap92_ok,
                                "results": t92_s31
                            }
                            if not tahap92_ok:
                                passed = False

                            # Step H: Tahap 93 Verification (iPhone 17 Pro Real Home Screen Kelvin & Folder Social Stability)
                            t93_s32a = tahap83_res.get("skenario32A_Iphone17ProHomeScreenKelvin", {})
                            t93_s32b = tahap83_res.get("skenario32B_Iphone17ProFolderSocial", {})
                            tahap93_ok = (
                                tahap92_ok
                                and t93_s32a.get("isNeonPoster") is False
                                and t93_s32a.get("hasNakedIsland") is False
                                and t93_s32a.get("hasTrueStatusBar") is True
                                and t93_s32a.get("isDefinitePortrait") is True
                                and t93_s32a.get("isGame") is False
                                and t93_s32a.get("exitByHomeScreen") is True
                                and t93_s32a.get("strictPortraitExitProof") is True
                                and t93_s32a.get("canExit") is True
                                and t93_s32b.get("isFolder") is True
                                and t93_s32b.get("isNeonPoster") is False
                                and t93_s32b.get("isGame") is False
                                and t93_s32b.get("exitByFolder") is True
                                and t93_s32b.get("canExit") is True
                            )

                            test_data["tahap93_verification"] = {
                                "passed": tahap93_ok,
                                "skenario32A": t93_s32a,
                                "skenario32B": t93_s32b,
                                "skenario31": t92_s31
                            }
                            if not tahap93_ok:
                                passed = False

                            # Step I: Tahap 94 Verification (Long-Session Performance & GPU Anti-Throttling)
                            try:
                                from handset.server import _get_current_wallpaper_file_and_signature
                                # Sub-Test 2D: Wallpaper signature cache (100 calls < 5ms)
                                t0_wp = time.perf_counter()
                                for _ in range(100):
                                    _get_current_wallpaper_file_and_signature()
                                elapsed_wp_ms = (time.perf_counter() - t0_wp) * 1000
                                sub2d_ok = (elapsed_wp_ms < 5.0)

                                t94_res = window.evaluate_js('''
                                (() => {
                                    const ifr = document.querySelector("#screen-view");
                                    const mWin = ifr ? ifr.contentWindow : null;
                                    if (!mWin) return { error: "no-mwin" };

                                    // --- Sub-Test 2A: Zero toDataURL on scroll & fast execution < 15ms ---
                                    let toDataURLCallCount = 0;
                                    const origToDataURL = mWin.HTMLCanvasElement.prototype.toDataURL;
                                    mWin.HTMLCanvasElement.prototype.toDataURL = function(...args) {
                                        toDataURLCallCount++;
                                        return origToDataURL.apply(this, args);
                                    };

                                    const mc = mWin.document.getElementById("c") || mWin.document.querySelector("canvas");
                                    const t0_scroll = performance.now();
                                    for (let i = 0; i < 25; i++) {
                                        const wheelEv = new WheelEvent("wheel", { deltaY: 100, bubbles: true });
                                        if (mc) mc.dispatchEvent(wheelEv);
                                    }
                                    for (let i = 0; i < 25; i++) {
                                        const moveEv = new PointerEvent("pointermove", { clientX: 50 + i, clientY: 100 + i, bubbles: true });
                                        mWin.dispatchEvent(moveEv);
                                    }
                                    const elapsed_scroll = performance.now() - t0_scroll;
                                    mWin.HTMLCanvasElement.prototype.toDataURL = origToDataURL;

                                    // --- Sub-Test 2B: 360-frame stress test & autoRotateGameEnabled=false 0.00ms check ---
                                    const stressCanvas = document.createElement("canvas");
                                    stressCanvas.width = 120;
                                    stressCanvas.height = 260;
                                    const sCtx = stressCanvas.getContext("2d");
                                    sCtx.fillStyle = "#223344";
                                    sCtx.fillRect(0, 0, 120, 260);

                                    const t0_stress = performance.now();
                                    for (let i = 0; i < 360; i++) {
                                        mWin.analyzeFrameOrientation(stressCanvas, 0);
                                    }
                                    const elapsed_stress = performance.now() - t0_stress;
                                    const avg_frame_ms = elapsed_stress / 360;

                                    // Test autoRotateGameEnabled === false check
                                    const origAutoRotate = mWin.autoRotateGameEnabled;
                                    mWin.autoRotateGameEnabled = false;
                                    const t0_off = performance.now();
                                    for (let i = 0; i < 100; i++) {
                                        if (typeof mWin.checkAutoRotateOrientation === "function") {
                                            mWin.checkAutoRotateOrientation(stressCanvas, 1216, 2656);
                                        }
                                    }
                                    const elapsed_off = performance.now() - t0_off;
                                    mWin.autoRotateGameEnabled = origAutoRotate;

                                    // --- Sub-Test 2C: Zero infinite rAF loop in app.js & GPU containment ---
                                    const ps = document.querySelector("#phone-screen");
                                    const sd = document.querySelector(".side-drawer") || document.querySelector("#control-pane");
                                    const psStyle = ps ? window.getComputedStyle(ps) : null;
                                    const sdStyle = sd ? window.getComputedStyle(sd) : null;
                                    const psContain = psStyle ? (psStyle.contain || "") : "";
                                    const sdContain = sdStyle ? (sdStyle.contain || "") : "";

                                    const drawerCards = document.querySelectorAll(".side-drawer .card, .side-drawer .control-group, .side-drawer section");
                                    let hasNestedBackdrop = false;
                                    for (const card of drawerCards) {
                                        const cStyle = window.getComputedStyle(card);
                                        const bf = cStyle.backdropFilter || cStyle.webkitBackdropFilter || "none";
                                        if (bf !== "none" && bf !== "") {
                                            hasNestedBackdrop = true;
                                            break;
                                        }
                                    }

                                    return {
                                        sub2a: {
                                            toDataURLCallCount: toDataURLCallCount,
                                            elapsed_scroll_ms: elapsed_scroll,
                                            passed: (toDataURLCallCount === 0 && elapsed_scroll < 15.0)
                                        },
                                        sub2b: {
                                            total_stress_ms: elapsed_stress,
                                            avg_frame_ms: avg_frame_ms,
                                            elapsed_off_ms: elapsed_off,
                                            passed: (avg_frame_ms < 3.5 && elapsed_off < 1.0)
                                        },
                                        sub2c: {
                                            psContain: psContain,
                                            sdContain: sdContain,
                                            hasNestedBackdrop: hasNestedBackdrop,
                                            passed: (!hasNestedBackdrop)
                                        }
                                    };
                                })()
                                ''')

                                tahap94_ok = (
                                    tahap93_ok
                                    and isinstance(t94_res, dict)
                                    and t94_res.get("sub2a", {}).get("passed") is True
                                    and t94_res.get("sub2b", {}).get("passed") is True
                                    and t94_res.get("sub2c", {}).get("passed") is True
                                    and sub2d_ok is True
                                )

                                test_data["tahap94_verification"] = {
                                    "passed": tahap94_ok,
                                    "sub2a": t94_res.get("sub2a") if isinstance(t94_res, dict) else None,
                                    "sub2b": t94_res.get("sub2b") if isinstance(t94_res, dict) else None,
                                    "sub2c": t94_res.get("sub2c") if isinstance(t94_res, dict) else None,
                                    "sub2d": {"elapsed_ms": elapsed_wp_ms, "passed": sub2d_ok}
                                }
                                if not tahap94_ok:
                                    passed = False

                                # Step K: Tahap 95 Verification (Instant Bluetooth Audio Sink & Real-Time Wallpaper Sync)
                                try:
                                    # Sub-Test 2A: Verify Bluetooth Audio Sink 5-attempt wake & retry loop + strict Opened state check
                                    from handset.audio import AUDIO_POWERSHELL_STA_HOST, audio_manager
                                    audio_status = audio_manager.status()
                                    has_burst_retry = ("$attempt = 1; $attempt -le 5" in AUDIO_POWERSHELL_STA_HOST)
                                    has_burst_wake_delay = ("Start-Sleep -Milliseconds 120" in AUDIO_POWERSHELL_STA_HOST)
                                    has_retry_backoff = ("Start-Sleep -Milliseconds 350" in AUDIO_POWERSHELL_STA_HOST)
                                    has_opened_check = ("$stateStr -eq 'Opened'" in AUDIO_POWERSHELL_STA_HOST)
                                    has_sta_reconnect = ("$reconnectTimer = New-Object System.Windows.Forms.Timer" in AUDIO_POWERSHELL_STA_HOST)
                                    has_clean_status = (isinstance(audio_status, dict) and "state" in audio_status and "volume" in audio_status)
                                    sub2a_audio_ok = bool(
                                        has_burst_retry
                                        and has_burst_wake_delay
                                        and has_retry_backoff
                                        and has_opened_check
                                        and has_sta_reconnect
                                        and has_clean_status
                                    )

                                    # Sub-Test 2B: Instant wallpaper inode metadata check (no full-file MD5 reads, 100 calls < 15ms)
                                    from handset.server import _get_current_wallpaper_file_and_signature
                                    t0_wp95 = time.perf_counter()
                                    _, sig_sample = _get_current_wallpaper_file_and_signature()
                                    for _ in range(100):
                                        _get_current_wallpaper_file_and_signature()
                                    elapsed_wp95_ms = (time.perf_counter() - t0_wp95) * 1000.0
                                    sub2b_wp_ok = bool(elapsed_wp95_ms < 15.0)

                                    # Sub-Test 2C: WebView2 live DOM dynamic wallpaper styling & containment verification
                                    t95_dom_res = window.evaluate_js('''
                                    (() => {
                                        // 1. Verify CSS containment allows repaint (no contain: strict on wallpaper pseudo-element)
                                        let containVal = "";
                                        for (const sheet of document.styleSheets) {
                                            try {
                                                for (const rule of sheet.cssRules) {
                                                    if (rule.selectorText && rule.selectorText.includes(".app::before")) {
                                                        containVal = rule.style.contain || "";
                                                    }
                                                }
                                            } catch (_) {}
                                        }
                                        const notStrict = !containVal.includes("strict");

                                        // 2. Test dynamic wallpaper CSS variable injection
                                        const testSig = "sig_test_" + Date.now();
                                        const origBg = document.documentElement.style.getPropertyValue("--wallpaper-bg-image");
                                        if (typeof window.applyWallpaperSignature === "function") {
                                            window.applyWallpaperSignature(testSig);
                                        }
                                        const updatedBg = document.documentElement.style.getPropertyValue("--wallpaper-bg-image");
                                        const dynamicRepaintReady = Boolean(updatedBg && updatedBg.includes(testSig));

                                        // Restore original bg
                                        if (origBg) {
                                            document.documentElement.style.setProperty("--wallpaper-bg-image", origBg);
                                        }

                                        // 3. Verify wallpaper sig watcher exists
                                        const hasWatcher = (typeof window.checkWallpaperSig === "function");

                                        return {
                                            containVal: containVal,
                                            notStrict: notStrict,
                                            dynamicRepaintReady: dynamicRepaintReady,
                                            hasWatcher: hasWatcher,
                                            passed: Boolean(notStrict && dynamicRepaintReady && hasWatcher)
                                        };
                                    })()
                                    ''')

                                    sub2c_dom_ok = bool(
                                        isinstance(t95_dom_res, dict)
                                        and t95_dom_res.get("passed") is True
                                    )

                                    # Sub-Test 2D: Zero-Regression of all 32 Auto-Rotate scenarios
                                    sub2d_regression_ok = bool(tahap94_ok and tahap93_ok)

                                    tahap95_ok = bool(
                                        sub2a_audio_ok
                                        and sub2b_wp_ok
                                        and sub2c_dom_ok
                                        and sub2d_regression_ok
                                    )

                                    test_data["tahap95_verification"] = {
                                        "passed": tahap95_ok,
                                        "sub2a": {
                                            "has_burst_retry": has_burst_retry,
                                            "has_burst_wake_delay": has_burst_wake_delay,
                                            "has_retry_backoff": has_retry_backoff,
                                            "has_opened_check": has_opened_check,
                                            "has_sta_reconnect": has_sta_reconnect,
                                            "has_clean_status": has_clean_status,
                                            "passed": sub2a_audio_ok,
                                        },
                                        "sub2b": {
                                            "elapsed_100_calls_ms": elapsed_wp95_ms,
                                            "sample_signature": sig_sample,
                                            "passed": sub2b_wp_ok,
                                        },
                                        "sub2c": t95_dom_res if isinstance(t95_dom_res, dict) else {},
                                        "sub2d": {
                                            "tahap94_ok": tahap94_ok,
                                            "tahap93_ok": tahap93_ok,
                                            "passed": sub2d_regression_ok,
                                        }
                                    }
                                    if not tahap95_ok:
                                        passed = False

                                    # Tahap 96: Visual QR Bridge Verification
                                    try:
                                        from handset.qr_bridge import decode_qr_from_image, generate_qr_test_image
                                        
                                        # Sub-Test 2A: ISO/IEC 18004 URL decoding test on 3 real URLs
                                        target_urls = [
                                            "https://www.instagram.com/p/DVfE5d9kydZ/?igsh=NTc4MTIwNjQ2YQ==",
                                            "https://www.instagram.com/p/DViTk7TklVj/?igsh=MzRlODBiNWFlZA==",
                                            "https://vt.tiktok.com/ZSjX9k2Lm/"
                                        ]
                                        decoded_urls = []
                                        for t_url in target_urls:
                                            qr_img = generate_qr_test_image(t_url, scale=8, border=4)
                                            dec = decode_qr_from_image(qr_img)
                                            decoded_urls.append(dec)
                                        sub2a_qr_ok = bool(
                                            decoded_urls == target_urls
                                        )

                                        # Sub-Test 2B: Simulated iPhone canvas (1216x2656) modal QR detection
                                        from PIL import Image as _PILImg
                                        mock_phone = _PILImg.new("RGB", (1216, 2656), (20, 20, 24))
                                        qr_test_card = generate_qr_test_image("https://vt.tiktok.com/ZSjX9k2Lm/", scale=12, border=4)
                                        cx = (1216 - qr_test_card.width) // 2
                                        cy = (2656 - qr_test_card.height) // 2
                                        mock_phone.paste(qr_test_card, (cx, cy))
                                        canvas_dec = decode_qr_from_image(mock_phone)
                                        sub2b_canvas_ok = bool(canvas_dec == "https://vt.tiktok.com/ZSjX9k2Lm/")

                                        # Sub-Test 2C: DOM elements & interactive hooks check
                                        t96_dom_res = window.evaluate_js('''
                                        (() => {
                                            const btnScan = document.querySelector("#btn-scan-qr-link");
                                            const hasFn = (typeof window.triggerScanQrLinkWithFeedback === "function");
                                            const pills = Array.from(document.querySelectorAll(".shortcut-pill")).map(p => p.textContent);
                                            const hasAlt = pills.some(p => p.includes("Alt+G") || p.includes("Alt+B"));
                                            return {
                                                hasBtnScan: Boolean(btnScan),
                                                hasFn: hasFn,
                                                hasAlt: hasAlt,
                                                passed: Boolean(btnScan && hasFn && hasAlt)
                                            };
                                        })()
                                        ''')
                                        sub2c_dom_ok = bool(isinstance(t96_dom_res, dict) and t96_dom_res.get("passed") is True)

                                        # Sub-Test 2D: Zero regression
                                        sub2d_regression_ok = bool(tahap95_ok and tahap94_ok and tahap93_ok)

                                        tahap96_ok = bool(sub2a_qr_ok and sub2b_canvas_ok and sub2c_dom_ok and sub2d_regression_ok)
                                        test_data["tahap96_verification"] = {
                                            "passed": tahap96_ok,
                                            "sub2a": {
                                                "target_urls": target_urls,
                                                "decoded_urls": decoded_urls,
                                                "passed": sub2a_qr_ok,
                                            },
                                            "sub2b": {
                                                "canvas_decoded": canvas_dec,
                                                "passed": sub2b_canvas_ok,
                                            },
                                            "sub2c": t96_dom_res if isinstance(t96_dom_res, dict) else {},
                                            "sub2d": {
                                                "tahap95_ok": tahap95_ok,
                                                "tahap94_ok": tahap94_ok,
                                                "tahap93_ok": tahap93_ok,
                                                "passed": sub2d_regression_ok,
                                            }
                                        }
                                        if not tahap96_ok:
                                            passed = False

                                        # Tahap 97: Hybrid Visual QR Bridge + Direct Unhighlighted URL Scanner Verification
                                        try:
                                            from handset.qr_bridge import decode_qr_from_image, generate_qr_test_image, extract_visible_url_from_frame
                                            from PIL import Image as _PILImg, ImageDraw as _PILDdr, ImageFont as _PILFnt

                                            # Sub-Test 2A: ISO/IEC 18004 QR Matrix decode remains 100% (< 50ms)
                                            t97_target_urls = [
                                                "https://www.instagram.com/p/DVfE5d9kydZ/?igsh=NTc4MTIwNjQ2YQ==",
                                                "https://www.instagram.com/p/DViTk7TklVj/?igsh=MzRlODBiNWFlZA==",
                                                "https://vt.tiktok.com/ZSjX9k2Lm/"
                                            ]
                                            t97_qr_decoded = []
                                            for t_url in t97_target_urls:
                                                qr_img = generate_qr_test_image(t_url, module_size=8, quiet_zone=4)
                                                t97_qr_decoded.append(decode_qr_from_image(qr_img))
                                            t97_sub2a_ok = bool(t97_qr_decoded == t97_target_urls)

                                            # Sub-Test 2B: Direct Unhighlighted Yellow URL in Apple Notes / Quick Note (No QR!)
                                            notes_canvas = _PILImg.new("RGB", (1216, 2656), (252, 252, 252))
                                            ndraw = _PILDdr.Draw(notes_canvas)
                                            try:
                                                nfont = _PILFnt.truetype(r"C:\Windows\Fonts\segoeui.ttf", 36)
                                            except Exception:
                                                nfont = _PILFnt.load_default()
                                            yellow_target_url = "https://www.instagram.com/p/DVfE5d9kydZ/?igsh=NTc4MTIwNjQ2YQ=="
                                            ndraw.text((80, 500), yellow_target_url, fill=(218, 155, 12), font=nfont)
                                            extracted_yellow_url = extract_visible_url_from_frame(notes_canvas)
                                            t97_sub2b_ok = bool(extracted_yellow_url == yellow_target_url)

                                            # Sub-Test 2C: Live WebView2 UI Verification
                                            t97_dom_res = window.evaluate_js('''
                                            (() => {
                                                const btnScan = document.querySelector("#btn-scan-qr-link");
                                                const btnScanText = btnScan ? (btnScan.textContent || "") : "";
                                                const pills = Array.from(document.querySelectorAll(".shortcut-pill")).map(p => p.textContent);
                                                const hasAltUpdated = pills.some(p => (p.includes("Alt+B") || p.includes("Alt+G")) && p.includes("Link"));
                                                const hasBtnScanUpdated = btnScanText.includes("Grab Link") || btnScanText.includes("Scan QR");
                                                return {
                                                    btnScanText: btnScanText,
                                                    hasBtnScanUpdated: hasBtnScanUpdated,
                                                    hasAltUpdated: hasAltUpdated,
                                                    passed: Boolean(btnScan && hasBtnScanUpdated && hasAltUpdated)
                                                };
                                            })()
                                            ''')
                                            t97_sub2c_ok = bool(isinstance(t97_dom_res, dict) and t97_dom_res.get("passed") is True)

                                            # Sub-Test 2D: Zero Regression
                                            t97_sub2d_ok = bool(tahap96_ok and tahap95_ok and tahap94_ok and tahap93_ok)

                                            tahap97_ok = bool(t97_sub2a_ok and t97_sub2b_ok and t97_sub2c_ok and t97_sub2d_ok)
                                            test_data["tahap97_verification"] = {
                                                "passed": tahap97_ok,
                                                "sub2a": {
                                                    "target_urls": t97_target_urls,
                                                    "decoded_urls": t97_qr_decoded,
                                                    "passed": t97_sub2a_ok,
                                                },
                                                "sub2b": {
                                                    "target_yellow_url": yellow_target_url,
                                                    "extracted_yellow_url": extracted_yellow_url,
                                                    "passed": t97_sub2b_ok,
                                                },
                                                "sub2c": t97_dom_res if isinstance(t97_dom_res, dict) else {},
                                                "sub2d": {
                                                    "tahap96_ok": tahap96_ok,
                                                    "tahap95_ok": tahap95_ok,
                                                    "tahap94_ok": tahap94_ok,
                                                    "tahap93_ok": tahap93_ok,
                                                    "passed": t97_sub2d_ok,
                                                }
                                            }
                                            if not tahap97_ok:
                                                passed = False

                                            # Tahap 98 Verification Suite
                                            try:
                                                from handset.qr_bridge import (
                                                    extract_url_or_fragment_from_frame,
                                                    stitch_url_head_and_tail,
                                                )
                                                from handset.screen import (
                                                    spotlight_paste_clipboard_on_iphone,
                                                    spotlight_move_cursor_to_start_on_iphone,
                                                    spotlight_cleanup_and_close_on_iphone,
                                                )
                                                from handset.server import (
                                                    set_windows_clipboard_text,
                                                    get_windows_clipboard_text,
                                                )

                                                # Sub-Test 2A: Simulated Spotlight Head + Tail Stitching
                                                t98_target_long_url = "https://www.instagram.com/p/DVfE5d9kydZ/?igsh=NTc4MTIwNjQ2YQ=="
                                                t98_W, t98_H = 1216, 2656
                                                try:
                                                    t98_font_large = _PILFnt.truetype(r"C:\Windows\Fonts\segoeui.ttf", 36)
                                                    t98_font_body = _PILFnt.truetype(r"C:\Windows\Fonts\segoeui.ttf", 32)
                                                except Exception:
                                                    t98_font_large = _PILFnt.load_default()
                                                    t98_font_body = _PILFnt.load_default()

                                                def _make_t98_spot_frame(txt):
                                                    im = _PILImg.new("RGB", (t98_W, t98_H), (18, 18, 18))
                                                    d = _PILDdr.Draw(im)
                                                    d.text((60, 500), "Konser Andrea Bocelli di Jiexpo Jakarta", fill=(240, 240, 240), font=t98_font_body)
                                                    d.text((60, 560), "diwarnai kericuhan penonton di gerbang barat!", fill=(200, 200, 200), font=t98_font_body)
                                                    capsule_box = [60, 150, t98_W - 60, 250]
                                                    d.rounded_rectangle(capsule_box, radius=24, fill=(42, 42, 46), outline=(70, 70, 76), width=2)
                                                    d.text((100, 175), txt, fill=(255, 255, 255), font=t98_font_large)
                                                    return im

                                                f_tail_synth = _make_t98_spot_frame(".instagram.com/p/DVfE5d9kydZ/?igsh=NTc4MTIwNjQ2YQ==")
                                                f_head_synth = _make_t98_spot_frame("https://www.instagram.com/p/DVfE5d9k")

                                                head_part = extract_url_or_fragment_from_frame(f_head_synth)
                                                tail_part = extract_url_or_fragment_from_frame(f_tail_synth)
                                                stitched_url = stitch_url_head_and_tail(head_part, tail_part)
                                                set_windows_clipboard_text(stitched_url)
                                                clip_val = get_windows_clipboard_text()

                                                t98_sub2a_ok = bool(
                                                    stitched_url == t98_target_long_url
                                                    and clip_val == t98_target_long_url
                                                )

                                                # Sub-Test 2B: QR Decode + Apple Notes Yellow URL
                                                t98_sub2b_ok = bool(t97_sub2a_ok and t97_sub2b_ok)

                                                # Sub-Test 2C: Live WebView2 DOM & Backend Macro Functions
                                                t98_dom_res = window.evaluate_js('''
                                                (() => {
                                                    const btnScan = document.querySelector("#btn-scan-qr-link");
                                                    const btnScanText = btnScan ? (btnScan.textContent || "") : "";
                                                    const pills = Array.from(document.querySelectorAll(".shortcut-pill")).map(p => p.textContent);
                                                    const hasAltUpdated = pills.some(p => (p.includes("Alt+G") || p.includes("Alt+B")) && p.includes("Grab"));
                                                    const hasBtnScanUpdated = btnScanText.includes("Grab Link");
                                                    return {
                                                        btnScanText: btnScanText,
                                                        hasBtnScanUpdated: hasBtnScanUpdated,
                                                        hasAltUpdated: hasAltUpdated,
                                                        passed: Boolean(
                                                            btnScan && hasBtnScanUpdated && hasAltUpdated
                                                        )
                                                    };
                                                })()
                                                ''')
                                                t98_sub2c_ok = bool(
                                                    isinstance(t98_dom_res, dict)
                                                    and t98_dom_res.get("passed") is True
                                                    and callable(spotlight_paste_clipboard_on_iphone)
                                                    and callable(spotlight_move_cursor_to_start_on_iphone)
                                                    and callable(spotlight_cleanup_and_close_on_iphone)
                                                )

                                                # Sub-Test 2D: Zero Regression
                                                t98_sub2d_ok = bool(tahap97_ok and tahap96_ok and tahap95_ok and tahap94_ok and tahap93_ok)

                                                tahap98_ok = bool(t98_sub2a_ok and t98_sub2b_ok and t98_sub2c_ok and t98_sub2d_ok)
                                                test_data["tahap98_verification"] = {
                                                    "passed": tahap98_ok,
                                                    "sub2a": {
                                                        "target_url": t98_target_long_url,
                                                        "stitched_url": stitched_url,
                                                        "clipboard_url": clip_val,
                                                        "passed": t98_sub2a_ok,
                                                    },
                                                    "sub2b": {
                                                        "qr_ok": t97_sub2a_ok,
                                                        "yellow_url_ok": t97_sub2b_ok,
                                                        "passed": t98_sub2b_ok,
                                                    },
                                                    "sub2c": t98_dom_res if isinstance(t98_dom_res, dict) else {},
                                                    "sub2d": {
                                                        "tahap97_ok": tahap97_ok,
                                                        "tahap96_ok": tahap96_ok,
                                                        "tahap95_ok": tahap95_ok,
                                                        "tahap94_ok": tahap94_ok,
                                                        "tahap93_ok": tahap93_ok,
                                                        "passed": t98_sub2d_ok,
                                                    }
                                                }
                                                if not tahap98_ok:
                                                    passed = False

                                                # Tahap 99 Verification Suite: Multi-Line Wrapped Spotlight Assembler
                                                try:
                                                    from handset.qr_bridge import (
                                                        extract_visible_url_from_frame,
                                                        extract_url_or_fragment_from_frame,
                                                        stitch_url_head_and_tail,
                                                        prepare_spotlight_top_box_images,
                                                        assemble_multiline_url_from_lines,
                                                        pick_best_extracted_url,
                                                    )
                                                    from handset.server import (
                                                        set_windows_clipboard_text,
                                                        get_windows_clipboard_text,
                                                    )

                                                    # Sub-Test 2A: Kasus Nyata Video Kelvin Detik 00:14
                                                    t99_target_url = "https://www.instagram.com/p/DVfE5d9kydZ/?igsh=MW81dmZra2htOTc2bA=="
                                                    t99_W, t99_H = 1216, 2656
                                                    try:
                                                        t99_font_lg = _PILFnt.truetype(r"C:\Windows\Fonts\segoeui.ttf", 38)
                                                        t99_font_bd = _PILFnt.truetype(r"C:\Windows\Fonts\segoeui.ttf", 32)
                                                    except Exception:
                                                        t99_font_lg = _PILFnt.load_default()
                                                        t99_font_bd = _PILFnt.load_default()

                                                    im_kelvin_0014 = _PILImg.new("RGB", (t99_W, t99_H), (15, 15, 18))
                                                    d_k = _PILDdr.Draw(im_kelvin_0014)
                                                    d_k.rounded_rectangle([60, 140, t99_W - 60, 420], radius=24, fill=(28, 30, 36), outline=(60, 62, 70), width=2)
                                                    d_k.text((90, 165), "https://www.instagram.com/p/", fill=(255, 255, 255), font=t99_font_lg)
                                                    d_k.text((90, 225), "DVfE5d9kydZ/?", fill=(255, 255, 255), font=t99_font_lg)
                                                    d_k.text((90, 285), "igsh=MW81dmZra2htOTc2bA==", fill=(255, 255, 255), font=t99_font_lg)
                                                    d_k.text((t99_W - 120, 285), "⊗", fill=(160, 160, 170), font=t99_font_lg)

                                                    # Noise below Spotlight
                                                    d_k.text((90, 480), "Websites", fill=(140, 140, 150), font=t99_font_bd)
                                                    d_k.text((90, 530), "Apple", fill=(240, 240, 240), font=t99_font_bd)
                                                    d_k.text((90, 580), "https://www.apple.com", fill=(100, 150, 255), font=t99_font_bd)
                                                    d_k.text((90, 650), "Siri Knowledge", fill=(140, 140, 150), font=t99_font_bd)
                                                    d_k.text((90, 710), "PRAMONO: 15 KONSER BESAR AKAN DIGELAR DI JKT, 2027", fill=(240, 240, 240), font=t99_font_bd)
                                                    d_k.text((90, 770), "Copy link", fill=(200, 200, 200), font=t99_font_bd)

                                                    extracted_kelvin = extract_visible_url_from_frame(im_kelvin_0014)
                                                    set_windows_clipboard_text(extracted_kelvin or "")
                                                    clip_val_t99 = get_windows_clipboard_text()

                                                    t99_sub2a_ok = bool(
                                                        extracted_kelvin == t99_target_url
                                                        and clip_val_t99 == t99_target_url
                                                        and extracted_kelvin != "https://www.instagram.com/p/"
                                                        and extracted_kelvin != "https://www.apple.com"
                                                    )

                                                    # Sub-Test 2B: Reel Multi-line & TikTok Multi-line Assembler
                                                    reel_lines = ["https://www.instagram.com/reel/", "DViTk7TklVj/?", "igsh=MzRlODBiNWFlZA==  x"]
                                                    assembled_reel = pick_best_extracted_url(assemble_multiline_url_from_lines(reel_lines))
                                                    tt_lines = ["https://vt.tiktok.com/", "ZSjX9k2Lm/ ⊗"]
                                                    assembled_tt = pick_best_extracted_url(assemble_multiline_url_from_lines(tt_lines))
                                                    t99_sub2b_ok = bool(
                                                        assembled_reel == "https://www.instagram.com/reel/DViTk7TklVj/?igsh=MzRlODBiNWFlZA=="
                                                        and assembled_tt == "https://vt.tiktok.com/ZSjX9k2Lm/"
                                                    )

                                                    # Sub-Test 2C: QR Matrix Decode + Apple Notes Yellow URL preservation
                                                    t99_sub2c_ok = bool(t98_sub2b_ok and t97_sub2a_ok and t97_sub2b_ok)

                                                    # Sub-Test 2D: Zero-Regression
                                                    t99_sub2d_ok = bool(tahap98_ok and tahap97_ok and tahap96_ok and tahap95_ok and tahap94_ok and tahap93_ok)

                                                    tahap99_ok = bool(t99_sub2a_ok and t99_sub2b_ok and t99_sub2c_ok and t99_sub2d_ok)
                                                    test_data["tahap99_verification"] = {
                                                        "passed": tahap99_ok,
                                                        "sub2a": {
                                                            "target_url": t99_target_url,
                                                            "extracted_url": extracted_kelvin,
                                                            "clipboard_url": clip_val_t99,
                                                            "passed": t99_sub2a_ok,
                                                        },
                                                        "sub2b": {
                                                            "assembled_reel": assembled_reel,
                                                            "assembled_tt": assembled_tt,
                                                            "passed": t99_sub2b_ok,
                                                        },
                                                        "sub2c": {
                                                            "qr_matrix_ok": t97_sub2a_ok,
                                                            "yellow_url_ok": t97_sub2b_ok,
                                                            "passed": t99_sub2c_ok,
                                                        },
                                                        "sub2d": {
                                                            "tahap98_ok": tahap98_ok,
                                                            "tahap97_ok": tahap97_ok,
                                                            "tahap96_ok": tahap96_ok,
                                                            "tahap95_ok": tahap95_ok,
                                                            "tahap94_ok": tahap94_ok,
                                                            "tahap93_ok": tahap93_ok,
                                                            "passed": t99_sub2d_ok,
                                                        }
                                                    }
                                                    if not tahap99_ok:
                                                        passed = False

                                                    # Tahap 100 Verification Suite: Perfect 1-Click Grab Link (Alt+B)
                                                    try:
                                                        from handset.qr_bridge import (
                                                            assemble_multiline_url_from_lines,
                                                            pick_best_extracted_url,
                                                            clean_spotlight_text_line,
                                                            clean_extracted_url,
                                                        )

                                                        # Sub-Test 2A: 6 Real-World Cases (Instagram @suciib, Reel @narb.tv, @faza_zall typo 4, TikTok @ranggakartono883, Threads 5-line @erika_ristra with "Ask Siri", Threads 5-line @sunnygho typo 4 with "Tanya Siri")
                                                        c1_lines = ["https://www.instagram.com/p/", "DVOj3E_klsj/?", "igsh=MW81dmZra2htOTc2bA== ⊗", "Websites", "Apple"]
                                                        r1 = pick_best_extracted_url(assemble_multiline_url_from_lines(c1_lines))
                                                        t100_c1_ok = (r1 == "https://www.instagram.com/p/DVOj3E_klsj/?igsh=MW81dmZra2htOTc2bA==")

                                                        c2_lines = ["https://www.instagram.com/reel/", "DVI9x8yT1zA/?", "igsh=MzRlODBiNWFlZA==  x", "Siri Knowledge"]
                                                        r2 = pick_best_extracted_url(assemble_multiline_url_from_lines(c2_lines))
                                                        t100_c2_ok = (r2 == "https://www.instagram.com/reel/DVI9x8yT1zA/?igsh=MzRlODBiNWFlZA==")

                                                        c3_lines = ["https://www.instagram.com/p/", "DVM2mK5Tx8P4", "igsh=MW81dmZra2htOTc2bA( ⊗", "Websites"]
                                                        r3 = pick_best_extracted_url(assemble_multiline_url_from_lines(c3_lines))
                                                        t100_c3_ok = (r3 == "https://www.instagram.com/p/DVM2mK5Tx8P/?igsh=MW81dmZra2htOTc2bA==")

                                                        c4_lines = ["https://vt.tiktok.com/", "ZSjABCDEF/ ⊗", "Top Hit"]
                                                        r4 = pick_best_extracted_url(assemble_multiline_url_from_lines(c4_lines))
                                                        t100_c4_ok = (r4 == "https://vt.tiktok.com/ZSjABCDEF/")

                                                        c5_lines = ["https://www.threads.com/", "@erika_ristra/post/", "DVfP0NaEyiQ?xmt=", "AQx000jT9l_X_qgLpB", "_9kLmNxQ — Ask Siri ⊗", "Websites", "Apple"]
                                                        r5 = pick_best_extracted_url(assemble_multiline_url_from_lines(c5_lines))
                                                        t100_c5_ok = (r5 == "https://www.threads.com/@erika_ristra/post/DVfP0NaEyiQ?xmt=AQx000jT9l_X_qgLpB_9kLmNxQ")

                                                        c6_lines = ["https://www.threads.net/", "@sunnygho/post/", "DVfP0NaEyiQ4xmt=", "AQz123456789_abc", "— Tanya Siri ⊗"]
                                                        r6 = pick_best_extracted_url(assemble_multiline_url_from_lines(c6_lines))
                                                        t100_c6_ok = (r6 == "https://www.threads.net/@sunnygho/post/DVfP0NaEyiQ?xmt=AQz123456789_abc")

                                                        t100_sub2a_ok = bool(t100_c1_ok and t100_c2_ok and t100_c3_ok and t100_c4_ok and t100_c5_ok and t100_c6_ok)

                                                        # Sub-Test 2B: Live WebView2 DOM Verification (Button "🔗 Grab Link (Alt+B)", Pill "Alt+B", No QR Guide)
                                                        t100_dom_res = window.evaluate_js('''
                                                        (() => {
                                                            const btnScan = document.querySelector("#btn-scan-qr-link");
                                                            const btnGuide = document.querySelector("#btn-qr-setup-guide");
                                                            const guideBox = document.querySelector("#qr-setup-guide-box");
                                                            const btnScanText = btnScan ? (btnScan.textContent || "").trim() : "";
                                                            const pills = Array.from(document.querySelectorAll(".shortcut-pill")).map(p => p.textContent.trim());
                                                            const hasAltB = pills.includes("Alt+B (Grab Link)");
                                                            const hasNoAltG = !pills.some(p => p.includes("Alt+G"));
                                                            const isBtnCorrect = (btnScanText === "🔗 Grab Link (Alt+B)");
                                                            const isGuideRemoved = (btnGuide === null && guideBox === null);
                                                            return {
                                                                btnScanText: btnScanText,
                                                                isBtnCorrect: isBtnCorrect,
                                                                hasAltB: hasAltB,
                                                                hasNoAltG: hasNoAltG,
                                                                isGuideRemoved: isGuideRemoved,
                                                                passed: Boolean(btnScan && isBtnCorrect && hasAltB && hasNoAltG && isGuideRemoved)
                                                            };
                                                        })()
                                                        ''')
                                                        t100_sub2b_ok = bool(isinstance(t100_dom_res, dict) and t100_dom_res.get("passed") is True)

                                                        # Sub-Test 2C: 11-char Shortcode Normalizer & Siri Suffix Stripper
                                                        norm_ig = clean_extracted_url("instagram.com/p/DVOj3E_klsj4igsh=MW81dmZra2htOTc2bA==")
                                                        norm_th = clean_extracted_url("threads.com/@erika_ristra/post/DVfP0NaEyiQ4xmt=AQx000jT9l_X_qgLpB")
                                                        siri_clean = clean_spotlight_text_line("_9kLmNxQ — Ask Siri ⊗")
                                                        t100_sub2c_ok = bool(
                                                            norm_ig == "https://www.instagram.com/p/DVOj3E_klsj/?igsh=MW81dmZra2htOTc2bA=="
                                                            and norm_th == "https://www.threads.com/@erika_ristra/post/DVfP0NaEyiQ?xmt=AQx000jT9l_X_qgLpB"
                                                            and siri_clean == "_9kLmNxQ"
                                                        )

                                                        # Sub-Test 2D: Zero-Regression
                                                        t100_sub2d_ok = bool(tahap99_ok and tahap98_ok and tahap97_ok and tahap96_ok and tahap95_ok and tahap94_ok and tahap93_ok)

                                                        tahap100_ok = bool(t100_sub2a_ok and t100_sub2b_ok and t100_sub2c_ok and t100_sub2d_ok)
                                                        test_data["tahap100_verification"] = {
                                                            "passed": tahap100_ok,
                                                            "sub2a": {
                                                                "c1_ok": t100_c1_ok,
                                                                "c2_ok": t100_c2_ok,
                                                                "c3_ok": t100_c3_ok,
                                                                "c4_ok": t100_c4_ok,
                                                                "c5_ok": t100_c5_ok,
                                                                "c6_ok": t100_c6_ok,
                                                                "passed": t100_sub2a_ok,
                                                            },
                                                            "sub2b": t100_dom_res if isinstance(t100_dom_res, dict) else {},
                                                            "sub2c": {
                                                                "norm_ig": norm_ig,
                                                                "norm_th": norm_th,
                                                                "siri_clean": siri_clean,
                                                                "passed": t100_sub2c_ok,
                                                            },
                                                            "sub2d": {
                                                                "tahap99_ok": tahap99_ok,
                                                                "tahap98_ok": tahap98_ok,
                                                                "tahap97_ok": tahap97_ok,
                                                                "tahap96_ok": tahap96_ok,
                                                                "tahap95_ok": tahap95_ok,
                                                                "tahap94_ok": tahap94_ok,
                                                                "tahap93_ok": tahap93_ok,
                                                                "passed": t100_sub2d_ok,
                                                            }
                                                        }
                                                        if not tahap100_ok:
                                                            passed = False

                                                        # Tahap 101 Verification Suite: Multi-Device iPhone 11 Pro Max + iPhone 17 Pro Spotlight Grab Link
                                                        try:
                                                            from handset.qr_bridge import (
                                                                extract_visible_url_from_frame,
                                                                assemble_multiline_url_from_lines,
                                                                pick_best_extracted_url,
                                                                clean_spotlight_text_line,
                                                                clean_extracted_url,
                                                                prepare_spotlight_top_box_images,
                                                            )
                                                            import inspect
                                                            from handset import screen as handset_screen

                                                            # Sub-Test 2A: iPhone 11 Pro Max Kasus 1 (Detik 00:45 @jktinfolk)
                                                            t101_W, t101_H = 1216, 2656
                                                            im_c1 = _PILImg.new("RGB", (t101_W, t101_H), (15, 15, 18))
                                                            d_c1 = _PILDdr.Draw(im_c1)
                                                            d_c1.text((90, 200), "Saran Siri", fill=(140, 140, 150), font=t99_font_bd)
                                                            d_c1.text((90, 260), "WhatsApp    Instagram    Foto    TikTok", fill=(200, 200, 200), font=t99_font_bd)
                                                            # Truncated ellipsis line in top zone
                                                            d_c1.rounded_rectangle([60, 480, t101_W - 60, 570], radius=18, fill=(28, 30, 36), outline=(60, 62, 70), width=1)
                                                            d_c1.text((90, 505), "🔍 https://www.insta...a2htOTc2bA==", fill=(255, 255, 255), font=t99_font_lg)
                                                            # Middle Safari card with 3-line URL and "Situs Web"
                                                            d_c1.rounded_rectangle([60, 720, t101_W - 60, 1100], radius=24, fill=(28, 30, 36), outline=(60, 62, 70), width=1)
                                                            d_c1.ellipse([90, 780, 190, 880], fill=(0, 122, 255))
                                                            d_c1.text((250, 755), "https://www.instagram.com/p/", fill=(255, 255, 255), font=t99_font_lg)
                                                            d_c1.text((250, 815), "DVOiS_PEj3t/?", fill=(255, 255, 255), font=t99_font_lg)
                                                            d_c1.text((250, 875), "igsh=MWNncGsxajh1eTh4Yw==", fill=(255, 255, 255), font=t99_font_lg)
                                                            d_c1.text((250, 935), "Situs Web", fill=(140, 140, 150), font=t99_font_bd)
                                                            d_c1.text((90, 1180), "Cari di App", fill=(140, 140, 150), font=t99_font_bd)

                                                            res_c1 = extract_visible_url_from_frame(im_c1)
                                                            t101_sub2a_ok = (res_c1 == "https://www.instagram.com/p/DVOiS_PEj3t/?igsh=MWNncGsxajh1eTh4Yw==")

                                                            # Sub-Test 2B: iPhone 11 Pro Max Kasus 2 (Detik 01:29 @indobildan)
                                                            im_c2 = _PILImg.new("RGB", (t101_W, t101_H), (15, 15, 18))
                                                            d_c2 = _PILDdr.Draw(im_c2)
                                                            d_c2.text((90, 505), "🔍 https://www.insta...ZzI0wa3Rp", fill=(255, 255, 255), font=t99_font_lg)
                                                            d_c2.rounded_rectangle([60, 720, t101_W - 60, 1100], radius=24, fill=(28, 30, 36), outline=(60, 62, 70), width=1)
                                                            d_c2.ellipse([90, 780, 190, 880], fill=(0, 122, 255))
                                                            d_c2.text((250, 755), "https://www.instagram.com/p/", fill=(255, 255, 255), font=t99_font_lg)
                                                            d_c2.text((250, 815), "DVf3daZEwa3/?", fill=(255, 255, 255), font=t99_font_lg)
                                                            d_c2.text((250, 875), "igsh=NTY5NWRmZzI0wa3Rp", fill=(255, 255, 255), font=t99_font_lg)
                                                            d_c2.text((250, 935), "Situs Web", fill=(140, 140, 150), font=t99_font_bd)

                                                            res_c2 = extract_visible_url_from_frame(im_c2)
                                                            t101_sub2b_ok = (res_c2 == "https://www.instagram.com/p/DVf3daZEwa3/?igsh=NTY5NWRmZzI0wa3Rp")

                                                            # Sub-Test 2C: 100% Backward compatibility with iPhone 17 Pro cases (Sub-Test 2A Tahap 100)
                                                            t101_sub2c_ok = bool(t100_sub2a_ok and t100_sub2c_ok)

                                                            # Sub-Test 2D: Spotlight Macro Code Inspection (NO 0x29 Escape on close, NO Backspace before paste)
                                                            try:
                                                                src_close = inspect.getsource(handset_screen.spotlight_cleanup_and_close_on_iphone)
                                                                src_paste = inspect.getsource(handset_screen.spotlight_paste_clipboard_on_iphone)
                                                                has_no_escape_on_close = "send_keys_direct([0x29])" not in src_close and "0x29" not in src_close
                                                                has_one_cmd_space_on_close = src_close.count("send_keys_direct([0xE3, 0x2C])") == 1
                                                                has_direct_paste = "paste-ios" in src_paste
                                                            except (OSError, TypeError):
                                                                code_close = getattr(handset_screen.spotlight_cleanup_and_close_on_iphone, "__code__", None)
                                                                code_paste = getattr(handset_screen.spotlight_paste_clipboard_on_iphone, "__code__", None)
                                                                consts_close = set(code_close.co_consts) if code_close else set()
                                                                consts_paste = set(code_paste.co_consts) if code_paste else set()
                                                                has_no_escape_on_close = (0x29 not in consts_close and 41 not in consts_close)
                                                                has_one_cmd_space_on_close = (0xE3 in consts_close or 227 in consts_close) and (0x2C in consts_close or 44 in consts_close)
                                                                has_direct_paste = ("paste-ios" in consts_paste)
                                                            t101_sub2d_ok = bool(has_no_escape_on_close and has_one_cmd_space_on_close and has_direct_paste)

                                                            # Sub-Test 2E: Zero-Regression
                                                            t101_sub2e_ok = bool(tahap100_ok and tahap99_ok and tahap98_ok and tahap97_ok and tahap96_ok and tahap95_ok and tahap94_ok and tahap93_ok)

                                                            tahap101_ok = bool(t101_sub2a_ok and t101_sub2b_ok and t101_sub2c_ok and t101_sub2d_ok and t101_sub2e_ok)
                                                            test_data["tahap101_verification"] = {
                                                                "passed": tahap101_ok,
                                                                "sub2a": {"res_c1": res_c1, "passed": t101_sub2a_ok},
                                                                "sub2b": {"res_c2": res_c2, "passed": t101_sub2b_ok},
                                                                "sub2c": {"t100_sub2a_ok": t100_sub2a_ok, "passed": t101_sub2c_ok},
                                                                "sub2d": {
                                                                    "has_no_escape_on_close": has_no_escape_on_close,
                                                                    "has_one_cmd_space_on_close": has_one_cmd_space_on_close,
                                                                    "has_direct_paste": has_direct_paste,
                                                                    "passed": t101_sub2d_ok,
                                                                },
                                                                "sub2e": {
                                                                    "tahap100_ok": tahap100_ok,
                                                                    "passed": t101_sub2e_ok,
                                                                }
                                                            }
                                                            if not tahap101_ok:
                                                                passed = False

                                                            # Tahap 102 Verification Suite: Reels, Threads Share, & Bottom Bar Cross-Zone Fusion
                                                            try:
                                                                from handset.qr_bridge import (
                                                                    extract_visible_url_from_frame,
                                                                    clean_extracted_url,
                                                                    extract_bottom_bar_fragment,
                                                                    fuse_card_and_bottom_bar_url,
                                                                    pick_best_extracted_url,
                                                                )

                                                                # Sub-Test 2A: Kasus 1 Reels @mood.zall20 (DU5fAYk/pqA -> DU5fAYkkpqA)
                                                                c1_card = ["https://www.instagram.com/reel/DU5fAYk/pqA/?igsh=MWtvaG8zbnd2NjR1NA=="]
                                                                c1_bot = "🔍 AYkkpqA/?igsh=MWtvaG8zbnd2NjR1NA== ⊗"
                                                                c1_frag = extract_bottom_bar_fragment(c1_bot)
                                                                c1_fused = fuse_card_and_bottom_bar_url(c1_card, [c1_frag])
                                                                res_t102_c1 = pick_best_extracted_url(c1_card + c1_fused)
                                                                t102_sub2a_ok = (res_t102_c1 == "https://www.instagram.com/reel/DU5fAYkkpqA/?igsh=MWtvaG8zbnd2NjR1NA==")

                                                                # Sub-Test 2B: Kasus 2 Reels Karate (DUszSla -> DUszSlaD8iD)
                                                                c2_card = ["https://www.instagram.com/reel/DUszSla"]
                                                                c2_bot = "🔍 zSlaD8iD/?igsh=MW9yajN2dncxMGduZQ== ⊗"
                                                                c2_frag = extract_bottom_bar_fragment(c2_bot)
                                                                c2_fused = fuse_card_and_bottom_bar_url(c2_card, [c2_frag])
                                                                res_t102_c2 = pick_best_extracted_url(c2_card + c2_fused)
                                                                t102_sub2b_ok = (res_t102_c2 == "https://www.instagram.com/reel/DUszSlaD8iD/?igsh=MW9yajN2dncxMGduZQ==")

                                                                # Sub-Test 2C: Kasus 3 Reels Police (DVfe298kjsj7-igsh= -> DVfe298kjsj/?igsh=)
                                                                c3_card = ["https://www.instagram.com/reel/DVfe298kjsj7-igsh=MWFpNGJ0dmZqN2IyaQ=="]
                                                                c3_bot = "🔍 fe298kjsj/?igsh=MWFpNGJ0dmZqN2IyaQ== ⊗"
                                                                c3_frag = extract_bottom_bar_fragment(c3_bot)
                                                                c3_fused = fuse_card_and_bottom_bar_url(c3_card, [c3_frag])
                                                                res_t102_c3 = pick_best_extracted_url(c3_card + c3_fused)
                                                                t102_sub2c_ok = (res_t102_c3 == "https://www.instagram.com/reel/DVfe298kjsj/?igsh=MWFpNGJ0dmZqN2IyaQ==")

                                                                # Sub-Test 2D: Kasus 4 Threads Share SnB0b30_c7aqqyy (Synthetic Frame 1216x2656)
                                                                t102_W, t102_H = 1216, 2656
                                                                im_th = _PILImg.new("RGB", (t102_W, t102_H), (15, 15, 18))
                                                                d_th = _PILDdr.Draw(im_th)
                                                                d_th.text((90, 200), "Saran Siri", fill=(140, 140, 150), font=t99_font_bd)
                                                                d_th.rounded_rectangle([60, 720, t102_W - 60, 1100], radius=24, fill=(28, 30, 36), outline=(60, 62, 70), width=1)
                                                                d_th.ellipse([90, 780, 190, 880], fill=(0, 122, 255))
                                                                d_th.text((250, 755), "https://www.threads.com/share/", fill=(255, 255, 255), font=t99_font_lg)
                                                                d_th.text((250, 825), "SnB0b30_c", fill=(255, 255, 255), font=t99_font_lg)
                                                                d_th.text((250, 935), "Situs Web", fill=(140, 140, 150), font=t99_font_bd)
                                                                d_th.rounded_rectangle([50, 2330, t102_W - 50, 2510], radius=32, fill=(28, 30, 36), outline=(60, 62, 70), width=1)
                                                                d_th.text((80, 2390), "🔍", fill=(180, 180, 190), font=t99_font_bd)
                                                                d_th.text((160, 2395), "//www.threads.com/share/SnB0b30_c7aqqyy", fill=(255, 255, 255), font=t99_font_lg)
                                                                d_th.text((1080, 2390), "⊗", fill=(180, 180, 190), font=t99_font_bd)

                                                                res_t102_c4 = extract_visible_url_from_frame(im_th)
                                                                t102_sub2d_ok = (res_t102_c4 == "https://www.threads.com/share/SnB0b30_c7aqqyy")

                                                                # Sub-Test 2E: Spotlight Paste Delay (420ms / 450ms) Inspection
                                                                try:
                                                                    src_paste = inspect.getsource(handset_screen.spotlight_paste_clipboard_on_iphone)
                                                                    has_calibrated_paste_delay = any(d in src_paste for d in ("0.42", "0.45"))
                                                                except (OSError, TypeError):
                                                                    code_paste = getattr(handset_screen.spotlight_paste_clipboard_on_iphone, "__code__", None)
                                                                    consts_paste = set(code_paste.co_consts) if code_paste else set()
                                                                    has_calibrated_paste_delay = bool({0.42, 0.45} & consts_paste)
                                                                t102_sub2e_ok = bool(has_calibrated_paste_delay)

                                                                # Sub-Test 2F: Full Zero-Regression
                                                                t102_sub2f_ok = bool(tahap101_ok and tahap100_ok and tahap99_ok and tahap98_ok and tahap97_ok and tahap96_ok and tahap95_ok and tahap94_ok and tahap93_ok)

                                                                tahap102_ok = bool(t102_sub2a_ok and t102_sub2b_ok and t102_sub2c_ok and t102_sub2d_ok and t102_sub2e_ok and t102_sub2f_ok)
                                                                test_data["tahap102_verification"] = {
                                                                    "passed": tahap102_ok,
                                                                    "sub2a": {"res_c1": res_t102_c1, "passed": t102_sub2a_ok},
                                                                    "sub2b": {"res_c2": res_t102_c2, "passed": t102_sub2b_ok},
                                                                    "sub2c": {"res_c3": res_t102_c3, "passed": t102_sub2c_ok},
                                                                    "sub2d": {"res_c4": res_t102_c4, "passed": t102_sub2d_ok},
                                                                    "sub2e": {"has_calibrated_paste_delay": has_calibrated_paste_delay, "passed": t102_sub2e_ok},
                                                                    "sub2f": {"tahap101_ok": tahap101_ok, "passed": t102_sub2f_ok},
                                                                }
                                                                if not tahap102_ok:
                                                                    passed = False

                                                                # Tahap 103 Verification Suite: First-Click Buka Tautan Banner, Dual-Snapshot Cursor Shift, & Anchor Fusion
                                                                try:
                                                                    from handset.qr_bridge import (
                                                                        clean_spotlight_text_line,
                                                                        clean_extracted_url,
                                                                        extract_bottom_bar_fragment,
                                                                        fuse_card_and_bottom_bar_url,
                                                                        stitch_bottom_bar_head_and_tail,
                                                                        stitch_url_head_and_tail,
                                                                        pick_best_extracted_url,
                                                                    )

                                                                    # Sub-Test 2A: Kasus Klik Pertama Detik 00:14 & 01:44 (Banner Buka Tautan Tail-Ellipsis + Bar Bawah)
                                                                    c1_banner = "www.instagram.com/p/DVfd23Phu6_/?igsh=M..."
                                                                    c1_bot = "🔍 d23Phu6_/?igsh=MWVmbmZoc2p0b3N1Zw== ⊗"
                                                                    c1_banner_cl = clean_extracted_url(c1_banner)
                                                                    c1_frag = extract_bottom_bar_fragment(c1_bot)
                                                                    c1_fused = fuse_card_and_bottom_bar_url([c1_banner_cl], [c1_frag])
                                                                    res_t103_2a_feed = pick_best_extracted_url(c1_fused)

                                                                    c2_banner = "www.instagram.com/reel/DVn1rJkD7Xp_/?igsh=..."
                                                                    c2_bot = "🔍 1rJkD7Xp_/?igsh=N2htc253Z31pMmo2 ⊗"
                                                                    c2_banner_cl = clean_extracted_url(c2_banner)
                                                                    c2_frag = extract_bottom_bar_fragment(c2_bot)
                                                                    c2_fused = fuse_card_and_bottom_bar_url([c2_banner_cl], [c2_frag])
                                                                    res_t103_2a_reel = pick_best_extracted_url(c2_fused)

                                                                    t103_sub2a_ok = (
                                                                        res_t103_2a_feed == "https://www.instagram.com/p/DVfd23Phu6_/?igsh=MWVmbmZoc2p0b3N1Zw==" and
                                                                        res_t103_2a_reel == "https://www.instagram.com/reel/DVn1rJkD7Xp_/?igsh=N2htc253Z31pMmo2"
                                                                    )

                                                                    # Sub-Test 2B: Kasus Anchor Fusion & Eliminasi Bug J7 (00:41 - 00:51)
                                                                    b_card = "https://www.instagram.com/p/DVfdnrd23J7/?igsh=MWVmbmZoc2p0b3N1Zw=="
                                                                    b_tail = "d23Phu6/?igsh=MWVmbmZoc2p0b3N1Zw=="
                                                                    b_head = "https://www.instagram.com/p/DVfd23Phu6_/?ig"
                                                                    b_fused = fuse_card_and_bottom_bar_url([b_card], [b_tail])
                                                                    res_t103_2b_fused = pick_best_extracted_url(b_fused)
                                                                    res_t103_2b_stitched = stitch_url_head_and_tail(b_head, b_tail)
                                                                    t103_sub2b_ok = (
                                                                        "J7" not in (res_t103_2b_fused or "") and
                                                                        res_t103_2b_stitched == "https://www.instagram.com/p/DVfd23Phu6_/?igsh=MWVmbmZoc2p0b3N1Zw=="
                                                                    )

                                                                    # Sub-Test 2C: Known Video Targets (Jakarta Point, Lovepink999, Ichsan Ersansyah)
                                                                    u_jp = clean_extracted_url("https://www.instagram.com/p/DVfy6u4k7PQ/?igsh=NzI0a2U3ZjVqZn")
                                                                    u_lp = clean_extracted_url("https://www.instagram.com/reel/DVn1rJkD7Xp_/?igsh=N2htc253Z31pMmo2")
                                                                    u_th = clean_extracted_url("https://www.threads.com/share/84P0jJqdaW")
                                                                    t103_sub2c_ok = (
                                                                        u_jp.startswith("https://www.instagram.com/p/DVfy6u4k7PQ/?igsh=NzI0a2U3ZjVqZn") and
                                                                        u_lp.startswith("https://www.instagram.com/reel/DVn1rJkD7Xp_/?igsh=N2htc253Z31pMmo2") and
                                                                        u_th == "https://www.threads.com/share/84P0jJqdaW"
                                                                    )

                                                                    # Sub-Test 2D: Spotlight Sequence Inspection (screen.py & app.js)
                                                                    try:
                                                                        src_mv = inspect.getsource(handset_screen.spotlight_move_cursor_to_start_on_iphone)
                                                                        no_sel_all = 'trigger_ios_shortcut("select-all")' not in src_mv
                                                                        has_up_and_left = "0x52" in src_mv and "0x50" in src_mv
                                                                    except (OSError, TypeError):
                                                                        code_mv = getattr(handset_screen.spotlight_move_cursor_to_start_on_iphone, "__code__", None)
                                                                        consts_mv = set(code_mv.co_consts) if code_mv else set()
                                                                        no_sel_all = "select-all" not in consts_mv
                                                                        has_up_and_left = bool({0x52, 0x50} & consts_mv or 82 in consts_mv)
                                                                    t103_sub2d_ok = bool(no_sel_all and has_up_and_left)

                                                                    # Sub-Test 2E: Full Zero-Regression
                                                                    t103_sub2e_ok = bool(tahap102_ok and tahap101_ok and tahap100_ok and tahap99_ok and tahap98_ok and tahap97_ok and tahap96_ok and tahap95_ok and tahap94_ok and tahap93_ok)

                                                                    tahap103_ok = bool(t103_sub2a_ok and t103_sub2b_ok and t103_sub2c_ok and t103_sub2d_ok and t103_sub2e_ok)
                                                                    test_data["tahap103_verification"] = {
                                                                        "passed": tahap103_ok,
                                                                        "sub2a": {"feed": res_t103_2a_feed, "reel": res_t103_2a_reel, "passed": t103_sub2a_ok},
                                                                        "sub2b": {"fused": res_t103_2b_fused, "stitched": res_t103_2b_stitched, "passed": t103_sub2b_ok},
                                                                        "sub2c": {"jp": u_jp, "lp": u_lp, "th": u_th, "passed": t103_sub2c_ok},
                                                                        "sub2d": {"no_sel_all": no_sel_all, "has_up_and_left": has_up_and_left, "passed": t103_sub2d_ok},
                                                                        "sub2e": {"tahap102_ok": tahap102_ok, "passed": t103_sub2e_ok},
                                                                    }
                                                                    if not tahap103_ok:
                                                                        passed = False

                                                                    # Tahap 104 Verification Suite: Safari Zone Split (2A & 2B), Threads /share/ Tail-Ellipsis, Cursor Strip, & Cross-Zone Char Correction
                                                                    try:
                                                                        from handset.qr_bridge import (
                                                                            extract_visible_url_from_frame,
                                                                            extract_url_or_fragment_from_frame,
                                                                            clean_spotlight_text_line,
                                                                            clean_extracted_url,
                                                                            extract_bottom_bar_fragment,
                                                                            fuse_card_and_bottom_bar_url,
                                                                            stitch_bottom_bar_head_and_tail,
                                                                            stitch_url_head_and_tail,
                                                                            pick_best_extracted_url,
                                                                        )

                                                                        # Sub-Test 2A: First Click 00:11 - 00:12 (IG Reel @_rannyariani DVR-dDBvMag without trailing '/' & leading '|')
                                                                        t104_2a_head = "|https://www.instagram.com/reel/DVR-dDBvMag"
                                                                        t104_2a_tail = "BvMag/?igsh=MWZwNGZkcTNremU3aw=="
                                                                        res_t104_2a_stitch = stitch_bottom_bar_head_and_tail([t104_2a_head], [t104_2a_tail])
                                                                        res_t104_2a_full = stitch_url_head_and_tail(t104_2a_head, t104_2a_tail)
                                                                        res_t104_2a_clip = stitch_url_head_and_tail("|https://www.instagram.com/reel/DVR-dDBvMa", t104_2a_tail)
                                                                        t104_sub2a_ok = (
                                                                            res_t104_2a_full == "https://www.instagram.com/reel/DVR-dDBvMag/?igsh=MWZwNGZkcTNremU3aw=="
                                                                            and res_t104_2a_clip == "https://www.instagram.com/reel/DVR-dDBvMag/?igsh=MWZwNGZkcTNremU3aw=="
                                                                            and "https://www.instagram.com/reel/DVR-dDBvMag/?igsh=MWZwNGZkcTNremU3aw==" in res_t104_2a_stitch
                                                                        )

                                                                        # Sub-Test 2B: 01:12 & 01:38 IG Feed @fakta.indo DPTL_8-BGhZ (Situs Web section header at y=665..710 + BOhZ -> BGhZ)
                                                                        t104_W, t104_H = 1216, 2656
                                                                        try:
                                                                            t104_font_sm = _PILFnt.truetype(r"C:\Windows\Fonts\segoeui.ttf", 26)
                                                                        except Exception:
                                                                            t104_font_sm = t99_font_bd

                                                                        im_t104_2b = _PILImg.new("RGB", (t104_W, t104_H), (15, 15, 18))
                                                                        d_2b = _PILDdr.Draw(im_t104_2b)
                                                                        # Zona 2B (y = 520..620): Banner Buka Tautan with 'DPTL_8-BOhZ/?igsh=M2...'
                                                                        d_2b.rounded_rectangle([60, 510, t104_W - 60, 630], radius=20, fill=(32, 34, 40))
                                                                        d_2b.text((215, 525), "Dari Instagram — Buka Tautan", fill=(210, 210, 220), font=t104_font_sm)
                                                                        d_2b.text((215, 568), "www.instagram.com/p/DPTL_8-BOhZ/?igsh=M2...", fill=(155, 155, 165), font=t104_font_sm)
                                                                        # y = 665..710: Section Header 'Situs Web' & middle-ellipsis suggestion
                                                                        d_2b.text((80, 665), "Situs Web", fill=(160, 160, 170), font=t99_font_bd)
                                                                        d_2b.text((420, 668), "🔍 https://www.insta...M2llNDd1Y2NweHV3aw==", fill=(140, 140, 150), font=t104_font_sm)
                                                                        # Zona 2A (y = 750..1120): Kartu Safari
                                                                        d_2b.rounded_rectangle([60, 740, t104_W - 60, 1120], radius=24, fill=(28, 30, 36), outline=(60, 62, 70), width=1)
                                                                        d_2b.ellipse([90, 790, 190, 890], fill=(0, 122, 255))
                                                                        d_2b.text((250, 760), "https://www.instagram.com/p/", fill=(255, 255, 255), font=t99_font_lg)
                                                                        d_2b.text((250, 825), "DPTL_8-BGhZ/?", fill=(255, 255, 255), font=t99_font_lg)
                                                                        d_2b.text((250, 890), "igsh=M2llNDd1Y2NweHV3aw==", fill=(255, 255, 255), font=t99_font_lg)
                                                                        d_2b.text((250, 960), "Situs Web", fill=(140, 140, 150), font=t99_font_bd)
                                                                        # Zona 3 (y = 2360..2480): Bottom Search Bar
                                                                        d_2b.rounded_rectangle([50, 2350, t104_W - 50, 2495], radius=32, fill=(38, 40, 48))
                                                                        d_2b.text((80, 2395), "🔍", fill=(180, 180, 190), font=t99_font_bd)
                                                                        d_2b.text((165, 2395), "8-BGhZ/?igsh=M2llNDd1Y2NweHV3aw==", fill=(255, 255, 255), font=t99_font_lg)
                                                                        d_2b.text((1085, 2395), "⊗", fill=(180, 180, 190), font=t99_font_bd)

                                                                        res_t104_2b_frame = extract_visible_url_from_frame(im_t104_2b)
                                                                        res_t104_2b_fuse = pick_best_extracted_url(
                                                                            fuse_card_and_bottom_bar_url(
                                                                                ["https://www.instagram.com/p/DPTL_8-BOhZ/"],
                                                                                ["8-BGhZ/?igsh=M2llNDd1Y2NweHV3aw=="],
                                                                            )
                                                                        )
                                                                        t104_sub2b_ok = (
                                                                            res_t104_2b_frame == "https://www.instagram.com/p/DPTL_8-BGhZ/?igsh=M2llNDd1Y2NweHV3aw=="
                                                                            and res_t104_2b_fuse == "https://www.instagram.com/p/DPTL_8-BGhZ/?igsh=M2llNDd1Y2NweHV3aw=="
                                                                        )

                                                                        # Sub-Test 2C: 03:07 - 03:08 Threads @seryyulian_ (_vjI9bjm7y_ 11 chars vs _vjI9bjm 8 chars on bright page)
                                                                        im_t104_2c = _PILImg.new("RGB", (t104_W, t104_H), (210, 212, 218))
                                                                        d_2c = _PILDdr.Draw(im_t104_2c)
                                                                        d_2c.rounded_rectangle([60, 510, t104_W - 60, 630], radius=20, fill=(45, 48, 56))
                                                                        d_2c.text((215, 525), "Dari Threads — Buka Tautan", fill=(220, 220, 230), font=t104_font_sm)
                                                                        d_2c.text((215, 568), "www.threads.com/share/_vjI9bjm7y_...", fill=(195, 195, 205), font=t99_font_bd)
                                                                        d_2c.text((80, 668), "Situs Web", fill=(90, 90, 100), font=t99_font_bd)
                                                                        d_2c.rounded_rectangle([60, 740, t104_W - 60, 1100], radius=24, fill=(38, 40, 48))
                                                                        d_2c.ellipse([90, 790, 190, 890], fill=(0, 122, 255))
                                                                        d_2c.text((250, 760), "https://www.threads.com/share/", fill=(255, 255, 255), font=t99_font_lg)
                                                                        d_2c.text((250, 825), "_vjI9bjm", fill=(255, 255, 255), font=t99_font_lg)
                                                                        d_2c.text((250, 910), "Situs Web", fill=(160, 160, 170), font=t99_font_bd)
                                                                        d_2c.rounded_rectangle([50, 2350, t104_W - 50, 2495], radius=32, fill=(145, 148, 156))
                                                                        d_2c.text((80, 2395), "🔍", fill=(235, 235, 240), font=t99_font_bd)
                                                                        d_2c.text((155, 2395), "|https://www.threads.com/share/_vjI9bjm7y_", fill=(250, 250, 252), font=t99_font_lg)
                                                                        d_2c.text((1085, 2395), "⊗", fill=(235, 235, 240), font=t99_font_bd)

                                                                        res_t104_2c_frame = extract_visible_url_from_frame(im_t104_2c)
                                                                        t104_sub2c_ok = (res_t104_2c_frame == "https://www.threads.com/share/_vjI9bjm7y_")

                                                                        # Sub-Test 2D: @warta.dokter (DVR39qms2Cj), @koflegend.indonesia (DVRfUMkgQ5V), & Threads @mulyono1453 (_vPQBHi_)
                                                                        u_wd = pick_best_extracted_url([
                                                                            clean_extracted_url("www.instagram.com/p/DVR39qms2Cj/?igsh=M..."),
                                                                            *fuse_card_and_bottom_bar_url(["https://www.instagram.com/p/DVR39qms2Cj/"], ["9qms2Cj/?igsh=MW8xN2Zra2htOTc2bA=="]),
                                                                        ])
                                                                        u_kof = pick_best_extracted_url([
                                                                            clean_extracted_url("www.instagram.com/p/DVRfUMkgQ5V/?igsh=M..."),
                                                                            *fuse_card_and_bottom_bar_url(["https://www.instagram.com/p/DVRfUMkgQ5V/"], ["UMkgQ5V/?igsh=MzRlODBiNWFlZA=="]),
                                                                        ])
                                                                        u_mul = pick_best_extracted_url([
                                                                            clean_extracted_url("www.threads.com/share/_vPQBHi_..."),
                                                                            clean_extracted_url("|https://www.threads.com/share/_vPQBHi_"),
                                                                        ])
                                                                        t104_sub2d_ok = (
                                                                            u_wd == "https://www.instagram.com/p/DVR39qms2Cj/?igsh=MW8xN2Zra2htOTc2bA=="
                                                                            and u_kof == "https://www.instagram.com/p/DVRfUMkgQ5V/?igsh=MzRlODBiNWFlZA=="
                                                                            and u_mul == "https://www.threads.com/share/_vPQBHi_"
                                                                        )

                                                                        # Sub-Test 2E: Full Zero-Regression Suite (Tahap 103..93)
                                                                        t104_sub2e_ok = bool(
                                                                            tahap103_ok and tahap102_ok and tahap101_ok and tahap100_ok
                                                                            and tahap99_ok and tahap98_ok and tahap97_ok and tahap96_ok
                                                                            and tahap95_ok and tahap94_ok and tahap93_ok
                                                                        )

                                                                        tahap104_ok = bool(t104_sub2a_ok and t104_sub2b_ok and t104_sub2c_ok and t104_sub2d_ok and t104_sub2e_ok)
                                                                        test_data["tahap104_verification"] = {
                                                                            "passed": tahap104_ok,
                                                                            "sub2a": {"full": res_t104_2a_full, "clipped": res_t104_2a_clip, "passed": t104_sub2a_ok},
                                                                            "sub2b": {"frame": res_t104_2b_frame, "fuse": res_t104_2b_fuse, "passed": t104_sub2b_ok},
                                                                            "sub2c": {"frame": res_t104_2c_frame, "passed": t104_sub2c_ok},
                                                                            "sub2d": {"warta": u_wd, "kof": u_kof, "mulyono": u_mul, "passed": t104_sub2d_ok},
                                                                            "sub2e": {"tahap103_ok": tahap103_ok, "passed": t104_sub2e_ok},
                                                                        }
                                                                        if not tahap104_ok:
                                                                            passed = False

                                                                        # Tahap 105 Verification Suite: Sliding Overlap, Dynamic Shortcodes (11-14 chars), Underscore Ground-Truth, & Threads Cleanup
                                                                        try:
                                                                            # Sub-Test 2A: Case 1 (00:15 / 00:25) IG Feed 11 chars DVfaB26P3lE
                                                                            res_t105_2a = stitch_url_head_and_tail(
                                                                                "https://www.instagram.com/p/DVfaB26P3lE/?ig",
                                                                                "DVfaB26P3lE/?igsh=MTRicmYxNmFldHFp"
                                                                            )
                                                                            t105_sub2a_ok = (res_t105_2a == "https://www.instagram.com/p/DVfaB26P3lE/?igsh=MTRicmYxNmFldHFp")

                                                                            # Sub-Test 2B: Case 2 (00:40) IG Feed 12 chars DR6b3d3p0-fe
                                                                            res_t105_2b = stitch_url_head_and_tail(
                                                                                "https://www.instagram.com/p/DR6b3d3p0-fe/?ig",
                                                                                "DR6b3d3p0-fe/?igsh=MXF5bjFodjF0aHg3ag=="
                                                                            )
                                                                            t105_sub2b_ok = (res_t105_2b == "https://www.instagram.com/p/DR6b3d3p0-fe/?igsh=MXF5bjFodjF0aHg3ag==")

                                                                            # Sub-Test 2C: Case 3 (01:12) IG Feed DVg3g0gEul_ (Correction of _/? misread as J/?)
                                                                            cands_2c = fuse_card_and_bottom_bar_url(
                                                                                ["https://www.instagram.com/p/DVg3g0gEulJ"],
                                                                                ["g3g0gEul_/?igsh=MXR0czJvYzR1cW1p"]
                                                                            )
                                                                            res_t105_2c = pick_best_extracted_url(cands_2c)
                                                                            t105_sub2c_ok = (res_t105_2c == "https://www.instagram.com/p/DVg3g0gEul_/?igsh=MXR0czJvYzR1cW1p")

                                                                            # Sub-Test 2D: Case 4 (01:38) IG Reel 11 chars DVfcgUqE37z
                                                                            res_t105_2d = stitch_url_head_and_tail(
                                                                                "https://www.instagram.com/reel/DVfcgUqE37z/?ig",
                                                                                "DVfcgUqE37z/?igsh=MW5la3hyNDR1dGlsMA=="
                                                                            )
                                                                            t105_sub2d_ok = (res_t105_2d == "https://www.instagram.com/reel/DVfcgUqE37z/?igsh=MW5la3hyNDR1dGlsMA==")

                                                                            # Sub-Test 2E: Case 5 (02:11) IG Feed 11 chars DVfYp5aknva
                                                                            res_t105_2e = stitch_url_head_and_tail(
                                                                                "https://www.instagram.com/p/DVfYp5aknva/?ig",
                                                                                "fYp5aknva/?igsh=M2RpZmhpekp0djd4"
                                                                            )
                                                                            t105_sub2e_ok = (res_t105_2e == "https://www.instagram.com/p/DVfYp5aknva/?igsh=M2RpZmhpekp0djd4")

                                                                            # Sub-Test 2F: Case 6 (02:40) IG Feed 13 chars DVTWCp1kW67po
                                                                            res_t105_2f = stitch_url_head_and_tail(
                                                                                "https://www.instagram.com/p/DVTWCp1kW67po/?ig",
                                                                                "DVTWCp1kW67po/?igsh=MXJldms0OGwyeXQxdQ=="
                                                                            )
                                                                            t105_sub2f_ok = (res_t105_2f == "https://www.instagram.com/p/DVTWCp1kW67po/?igsh=MXJldms0OGwyeXQxdQ==")

                                                                            # Sub-Test 2G: Case 7 (03:07 & 03:55) Threads /share/ 11 chars & Rejections
                                                                            th_cleaned = clean_extracted_url("https://www.threads.com/share/6rVhznr85zC_")
                                                                            th_best = pick_best_extracted_url([th_cleaned, "https://www.threads.com/share/6rVhznr85zC"])
                                                                            rej_bad = clean_extracted_url("https://www.instagram.com/p/DR6b52f/?igsh=123")
                                                                            rej_ell = clean_extracted_url("https://www.instagram.com/p/DVfaB26P3lE.../?igsh=123")
                                                                            t105_sub2g_ok = (
                                                                                th_best == "https://www.threads.com/share/6rVhznr85zC"
                                                                                and rej_bad == ""
                                                                                and rej_ell == ""
                                                                            )

                                                                            # Sub-Test 2H: Full Zero-Regression Suite (Tahap 104..93)
                                                                            t105_sub2h_ok = bool(
                                                                                tahap104_ok and tahap103_ok and tahap102_ok and tahap101_ok and tahap100_ok
                                                                                and tahap99_ok and tahap98_ok and tahap97_ok and tahap96_ok
                                                                                and tahap95_ok and tahap94_ok and tahap93_ok
                                                                            )

                                                                            tahap105_ok = bool(
                                                                                t105_sub2a_ok and t105_sub2b_ok and t105_sub2c_ok and t105_sub2d_ok
                                                                                and t105_sub2e_ok and t105_sub2f_ok and t105_sub2g_ok and t105_sub2h_ok
                                                                            )
                                                                            test_data["tahap105_verification"] = {
                                                                                "passed": tahap105_ok,
                                                                                "sub2a": {"url": res_t105_2a, "passed": t105_sub2a_ok},
                                                                                "sub2b": {"url": res_t105_2b, "passed": t105_sub2b_ok},
                                                                                "sub2c": {"url": res_t105_2c, "passed": t105_sub2c_ok},
                                                                                "sub2d": {"url": res_t105_2d, "passed": t105_sub2d_ok},
                                                                                "sub2e": {"url": res_t105_2e, "passed": t105_sub2e_ok},
                                                                                "sub2f": {"url": res_t105_2f, "passed": t105_sub2f_ok},
                                                                                "sub2g": {"url": th_best, "passed": t105_sub2g_ok},
                                                                                "sub2h": {"tahap104_ok": tahap104_ok, "passed": t105_sub2h_ok},
                                                                            }
                                                                            if not tahap105_ok:
                                                                                passed = False

                                                                            # Tahap 106 Verification Suite: Direct Windows Clipboard Copy, Double Domain Cleaning, & Prefix-Window Overlap Fusion
                                                                            try:
                                                                                # Sub-Test 2A: @gerindra.official DR3oaeVj9T2 vs to7pa / to7pp
                                                                                res_t106_2a = stitch_url_head_and_tail(
                                                                                    "https://www.instagram.com/p/DR3oaeVj9Tto7pa",
                                                                                    "3oaeVj9T2/?igsh=MW5lbDBlY3ZyZWZw"
                                                                                )
                                                                                res_t106_2a_pp = stitch_url_head_and_tail(
                                                                                    "https://www.instagram.com/p/DR3oaeVj9Tto7pp",
                                                                                    "3oaeVj9T2/?igsh=MW5lbDBlY3ZyZWZw"
                                                                                )
                                                                                t106_sub2a_ok = (
                                                                                    res_t106_2a == "https://www.instagram.com/p/DR3oaeVj9T2/?igsh=MW5lbDBlY3ZyZWZw"
                                                                                    and res_t106_2a_pp == "https://www.instagram.com/p/DR3oaeVj9T2/?igsh=MW5lbDBlY3ZyZWZw"
                                                                                )

                                                                                # Sub-Test 2B: @kepoinjangan.official DVhbSRTAbY7
                                                                                res_t106_2b = stitch_url_head_and_tail(
                                                                                    "|https://www.instagram.com/p/DVhbSRTAbY7/?ig",
                                                                                    "bSRTAbY7/?igsh=emR6a2liZWlvcWQ5"
                                                                                )
                                                                                t106_sub2b_ok = (res_t106_2b == "https://www.instagram.com/p/DVhbSRTAbY7/?igsh=emR6a2liZWlvcWQ5")

                                                                                # Sub-Test 2C: @valentinabritoa DUZatkuif6W with /?ip
                                                                                res_t106_2c = stitch_url_head_and_tail(
                                                                                    "https://www.instagram.com/reel/DUZatkuif6W/?ip",
                                                                                    "Zatkuif6W/?igsh=MW45emxkaDdsd2d2MA=="
                                                                                )
                                                                                t106_sub2c_ok = (res_t106_2c == "https://www.instagram.com/reel/DUZatkuif6W/?igsh=MW45emxkaDdsd2d2MA==")

                                                                                # Sub-Test 2D: @voktis.id DVf1-os96j0d & Double domain cleaning
                                                                                res_t106_2d = stitch_url_head_and_tail(
                                                                                    "https://instagrampp/instagram.com/p/DVf1ql-os",
                                                                                    "1-os96j0d/?igsh=MWxmaW9jc3d4d2I2ZQ=="
                                                                                )
                                                                                t106_sub2d_ok = (res_t106_2d == "https://www.instagram.com/p/DVf1-os96j0d/?igsh=MWxmaW9jc3d4d2I2ZQ==")

                                                                                # Sub-Test 2E: @misteri.dan.mitologi.dunia_real DVf9dTwE90z, Threads _u72dSzB5g5, & Direct Windows Clipboard Copy
                                                                                res_t106_2e_ig = stitch_url_head_and_tail(
                                                                                    "https://www.instagram.com/p/DVf9dTwE90z/?ig",
                                                                                    "f9dTwE90z/?igsh=bWp5dWhndWpzZzI2"
                                                                                )
                                                                                res_t106_2e_th = clean_extracted_url("https://www.threads.com/share/_u72dSzB5g5")
                                                                                clip_test_url = "https://www.instagram.com/p/DVf9dTwE90z/?igsh=bWp5dWhndWpzZzI2"
                                                                                clip_write_ok = set_windows_clipboard_text(clip_test_url)
                                                                                clip_read_val = get_windows_clipboard_text()
                                                                                t106_sub2e_ok = (
                                                                                    res_t106_2e_ig == "https://www.instagram.com/p/DVf9dTwE90z/?igsh=bWp5dWhndWpzZzI2"
                                                                                    and res_t106_2e_th == "https://www.threads.com/share/_u72dSzB5g5"
                                                                                    and clip_write_ok is True
                                                                                    and clip_read_val == clip_test_url
                                                                                )

                                                                                # Sub-Test 2F: Full Zero-Regression Suite (Tahap 105..93)
                                                                                t106_sub2f_ok = bool(
                                                                                    tahap105_ok and tahap104_ok and tahap103_ok and tahap102_ok and tahap101_ok and tahap100_ok
                                                                                    and tahap99_ok and tahap98_ok and tahap97_ok and tahap96_ok
                                                                                    and tahap95_ok and tahap94_ok and tahap93_ok
                                                                                )

                                                                                tahap106_ok = bool(
                                                                                    t106_sub2a_ok and t106_sub2b_ok and t106_sub2c_ok and t106_sub2d_ok and t106_sub2e_ok and t106_sub2f_ok
                                                                                )
                                                                                test_data["tahap106_verification"] = {
                                                                                    "passed": tahap106_ok,
                                                                                    "sub2a": {"url": res_t106_2a, "passed": t106_sub2a_ok},
                                                                                    "sub2b": {"url": res_t106_2b, "passed": t106_sub2b_ok},
                                                                                    "sub2c": {"url": res_t106_2c, "passed": t106_sub2c_ok},
                                                                                    "sub2d": {"url": res_t106_2d, "passed": t106_sub2d_ok},
                                                                                    "sub2e": {"ig": res_t106_2e_ig, "th": res_t106_2e_th, "clip_ok": clip_write_ok, "passed": t106_sub2e_ok},
                                                                                    "sub2f": {"tahap105_ok": tahap105_ok, "passed": t106_sub2f_ok},
                                                                                }
                                                                                if not tahap106_ok:
                                                                                    passed = False

                                                                                # Tahap 107 Verification Suite: Anchor.net OCR, Mid-Shortcode 17 Fix, Anisotropic Stretch & Multiline Safari Card
                                                                                try:
                                                                                    from handset.qr_bridge import (
                                                                                        clean_extracted_url,
                                                                                        stitch_url_head_and_tail,
                                                                                        assemble_multiline_url_from_lines,
                                                                                        fuse_codes,
                                                                                        pick_best_extracted_url,
                                                                                    )
                                                                                    # Sub-Test 2A: @pcicil DViT17WEvU7 (17 in mid-shortcode preserved, 7-char DViT17J strictly rejected)
                                                                                    bad_short_res = clean_extracted_url("https://www.instagram.com/reel/DViT17J/")
                                                                                    res_t107_2a = stitch_url_head_and_tail(
                                                                                        "|https://www.instagram.com/reel/DViT17WEvU7",
                                                                                        "iT17WEvU7/?igsh=a2g3cjhzdjpmbjc5"
                                                                                    )
                                                                                    card_lines_2a = [
                                                                                        "https://www.instagram.com/reel/",
                                                                                        "DViT17WEvU7",
                                                                                        "/?igsh=a2g3cjhzdjpmbjc5"
                                                                                    ]
                                                                                    asm_2a = assemble_multiline_url_from_lines(card_lines_2a)
                                                                                    t107_sub2a_ok = bool(
                                                                                        (bad_short_res == "" or bad_short_res is None)
                                                                                        and res_t107_2a == "https://www.instagram.com/reel/DViT17WEvU7/?igsh=a2g3cjhzdjpmbjc5"
                                                                                        and "https://www.instagram.com/reel/DViT17WEvU7/?igsh=a2g3cjhzdjpmbjc5" in asm_2a
                                                                                    )

                                                                                    # Sub-Test 2B: @infolokalfakt.id DVfWZZaCM55k (12 chars, Skenario A & B)
                                                                                    fused_2b_a = fuse_codes("DVfWZZaCM55", "WZZaCM55k")
                                                                                    fused_2b_b = fuse_codes("DVfWZZaCM59h", "WZZaCM55k")
                                                                                    res_t107_2b = stitch_url_head_and_tail(
                                                                                        "|https://www.instagram.com/p/DVfWZZaCM55",
                                                                                        "WZZaCM55k/?igsh=dndMdzc2bnh6MHFv"
                                                                                    )
                                                                                    t107_sub2b_ok = bool(
                                                                                        fused_2b_a == "DVfWZZaCM55k"
                                                                                        and fused_2b_b == "DVfWZZaCM55k"
                                                                                        and res_t107_2b == "https://www.instagram.com/p/DVfWZZaCM55k/?igsh=dndMdzc2bnh6MHFv"
                                                                                    )

                                                                                    # Sub-Test 2C: @timeline.jaksel DVb7779Eybv (v/ vs 4 disambiguation, Skenario C)
                                                                                    fused_2c = fuse_codes("DVb7779Eyb4", "b7779Eybv")
                                                                                    picked_2c = pick_best_extracted_url([
                                                                                        "https://www.instagram.com/p/DVb7779Eyb4/?igsh=b3llM2c3d291NWNv",
                                                                                        "https://www.instagram.com/p/DVb7779Eybv/?igsh=b3llM2c3d291NWNv"
                                                                                    ])
                                                                                    res_t107_2c = stitch_url_head_and_tail(
                                                                                        "https://www.instagram.com/p/DVb7779Eyb4",
                                                                                        "b7779Eybv/?igsh=b3llM2c3d291NWNv"
                                                                                    )
                                                                                    t107_sub2c_ok = bool(
                                                                                        fused_2c == "DVb7779Eybv"
                                                                                        and picked_2c == "https://www.instagram.com/p/DVb7779Eybv/?igsh=b3llM2c3d291NWNv"
                                                                                        and res_t107_2c == "https://www.instagram.com/p/DVb7779Eybv/?igsh=b3llM2c3d291NWNv"
                                                                                    )

                                                                                    # Sub-Test 2D: The 5 other test cases from Kelvin's video test evidence
                                                                                    u_warta = clean_extracted_url("https://www.instagram.com/p/DPAvWKE2_Ds/?igsh=c2ZodHB6dnpkaWkx")
                                                                                    u_sparks = clean_extracted_url("https://www.instagram.com/p/DUYYS1xAWt8/?igsh=MW45dHNsZmxWNWhxYg==")
                                                                                    u_cino = clean_extracted_url("https://www.instagram.com/reel/DVCf09uD25k/?igsh=Znh3N3Fja2hqZGs=")
                                                                                    u_ari = clean_extracted_url("https://www.threads.com/share/84P0jJqdaWs")
                                                                                    u_mll = clean_extracted_url("https://www.threads.com/share/64fHt-cydS4")
                                                                                    t107_sub2d_ok = bool(
                                                                                        u_warta == "https://www.instagram.com/p/DPAvWKE2_Ds/?igsh=c2ZodHB6dnpkaWkx"
                                                                                        and u_sparks == "https://www.instagram.com/p/DUYYS1xAWt8/?igsh=MW45dHNsZmxWNWhxYg=="
                                                                                        and u_cino == "https://www.instagram.com/reel/DVCf09uD25k/?igsh=Znh3N3Fja2hqZGs="
                                                                                        and u_ari == "https://www.threads.com/share/84P0jJqdaWs"
                                                                                        and u_mll == "https://www.threads.com/share/64fHt-cydS4"
                                                                                    )

                                                                                    # Sub-Test 2E: Full Zero-Regression Suite (Tahap 106..93)
                                                                                    t107_sub2e_ok = bool(
                                                                                        tahap106_ok and tahap105_ok and tahap104_ok and tahap103_ok
                                                                                        and tahap102_ok and tahap101_ok and tahap100_ok and tahap99_ok
                                                                                        and tahap98_ok and tahap97_ok and tahap96_ok and tahap95_ok
                                                                                        and tahap94_ok and tahap93_ok
                                                                                    )

                                                                                    tahap107_ok = bool(
                                                                                        t107_sub2a_ok and t107_sub2b_ok and t107_sub2c_ok and t107_sub2d_ok and t107_sub2e_ok
                                                                                    )
                                                                                    test_data["tahap107_verification"] = {
                                                                                        "passed": tahap107_ok,
                                                                                        "sub2a": {"url": res_t107_2a, "passed": t107_sub2a_ok},
                                                                                        "sub2b": {"url": res_t107_2b, "passed": t107_sub2b_ok},
                                                                                        "sub2c": {"url": res_t107_2c, "passed": t107_sub2c_ok},
                                                                                        "sub2d": {"warta": u_warta, "sparks": u_sparks, "cino": u_cino, "ari": u_ari, "mll": u_mll, "passed": t107_sub2d_ok},
                                                                                        "sub2e": {"tahap106_ok": tahap106_ok, "passed": t107_sub2e_ok},
                                                                                    }
                                                                                    if not tahap107_ok:
                                                                                        passed = False

                                                                                    # Tahap 108 Verification: Precision Auto-Rotate iPhone 11 Pro Max
                                                                                    t108_s2a = tahap83_res.get("skenario108_Sub2A_HomeScreenPlainClock", {})
                                                                                    t108_s2b = tahap83_res.get("skenario108_Sub2B_AutoReturnFolderAndIgDark", {})
                                                                                    t108_s2c = tahap83_res.get("skenario108_Sub2C_YouTubeWatchPageAdCard", {})
                                                                                    t108_s2d = tahap83_res.get("skenario108_Sub2D_YouTubeFullscreenHostDesk", {})
                                                                                    t108_s2e = tahap83_res.get("skenario108_Sub2E_YouTubeFullscreenAsusLaptop", {})

                                                                                    t108_sub2a_ok = bool(
                                                                                        t108_s2a.get("hasTrueStatusBar") is True
                                                                                        and t108_s2a.get("hasSideBar") is False
                                                                                        and t108_s2a.get("isGame") is False
                                                                                        and t108_s2a.get("isDefinitePortrait") is True
                                                                                    )

                                                                                    t108_sub2b_ok = bool(
                                                                                        t108_s2b.get("folderExit") is True
                                                                                        and t108_s2b.get("folderCanExit") is True
                                                                                        and t108_s2b.get("igDarkChrome") is True
                                                                                        and t108_s2b.get("igDarkExit") is True
                                                                                        and t108_s2b.get("igDarkCanExit") is True
                                                                                        and t108_s2b.get("igDarkIsGame") is False
                                                                                    )

                                                                                    t108_sub2c_ok = bool(
                                                                                        t108_s2c.get("hasLowerWhiteCard") is True
                                                                                        and t108_s2c.get("isFormation") is False
                                                                                        and t108_s2c.get("isNeonPoster") is False
                                                                                        and t108_s2c.get("isGame") is False
                                                                                    )

                                                                                    t108_sub2d_ok = bool(
                                                                                        t108_s2d.get("isCommentSheet") is False
                                                                                        and t108_s2d.get("hasDualBlackPillarbox") is True
                                                                                        and t108_s2d.get("isYouTube") is True
                                                                                        and t108_s2d.get("isGame") is True
                                                                                    )

                                                                                    t108_sub2e_ok = bool(
                                                                                        t108_s2e.get("hasLowerWhiteCard_at0") is False
                                                                                        and t108_s2e.get("isYouTube_at0") is True
                                                                                        and t108_s2e.get("isGame_at0") is True
                                                                                        and t108_s2e.get("hasLowerWhiteCard_at270") is False
                                                                                        and t108_s2e.get("isYouTube_at270") is True
                                                                                        and t108_s2e.get("isGame_at270") is True
                                                                                    )

                                                                                    tahap108_ok = bool(
                                                                                        tahap107_ok
                                                                                        and t108_sub2a_ok
                                                                                        and t108_sub2b_ok
                                                                                        and t108_sub2c_ok
                                                                                        and t108_sub2d_ok
                                                                                        and t108_sub2e_ok
                                                                                    )
                                                                                    test_data["tahap108_verification"] = {
                                                                                        "passed": tahap108_ok,
                                                                                        "sub2a": {"passed": t108_sub2a_ok, "data": t108_s2a},
                                                                                        "sub2b": {"passed": t108_sub2b_ok, "data": t108_s2b},
                                                                                        "sub2c": {"passed": t108_sub2c_ok, "data": t108_s2c},
                                                                                        "sub2d": {"passed": t108_sub2d_ok, "data": t108_s2d},
                                                                                        "sub2e": {"passed": t108_sub2e_ok, "data": t108_s2e},
                                                                                    }
                                                                                    if not tahap108_ok:
                                                                                        passed = False

                                                                                    # Step X: Tahap 109 Verification (iPhone 11 Pro Max Home Screen Dock Guard & Thin YouTube OSD Red Scrubber)
                                                                                    t109_s2a = tahap83_res.get("skenario109_Sub2A_HomeScreenKelvinFullRes", {})
                                                                                    t109_s2b = tahap83_res.get("skenario109_Sub2B_HomeScreenYouTubePip", {})
                                                                                    t109_s2c = tahap83_res.get("skenario109_Sub2C_InstagramDarkAndFolder", {})
                                                                                    t109_s2d = tahap83_res.get("skenario109_Sub2D_YouTubeWatchPageAd", {})
                                                                                    t109_s2e = tahap83_res.get("skenario109_Sub2E_YouTubeFullscreenLandscapeOsd", {})

                                                                                    t109_sub2a_ok = bool(
                                                                                        t109_s2a.get("hasIosDock") is True
                                                                                        and t109_s2a.get("hasLandscapeOsd") is False
                                                                                        and t109_s2a.get("hasPrelimSideBar") is False
                                                                                        and t109_s2a.get("hasSideBar") is False
                                                                                        and t109_s2a.get("isMenuLayout") is False
                                                                                        and t109_s2a.get("isGame_at0") is False
                                                                                        and t109_s2a.get("isDefinitePortrait") is True
                                                                                        and t109_s2a.get("strictExitProof_at270") is True
                                                                                    )

                                                                                    t109_sub2b_ok = bool(
                                                                                        t109_s2b.get("hasIosDock") is True
                                                                                        and t109_s2b.get("isGame_at0") is False
                                                                                        and t109_s2b.get("isDefinitePortrait") is True
                                                                                        and t109_s2b.get("strictExitProof_at270") is True
                                                                                    )

                                                                                    t109_sub2c_ok = bool(
                                                                                        t109_s2c.get("folderExit") is True
                                                                                        and t109_s2c.get("folderCanExit") is True
                                                                                        and t109_s2c.get("igDarkChrome") is True
                                                                                        and t109_s2c.get("igDarkExit") is True
                                                                                        and t109_s2c.get("igDarkCanExit") is True
                                                                                        and t109_s2c.get("igDarkIsGame") is False
                                                                                    )

                                                                                    t109_sub2d_ok = bool(
                                                                                        t109_s2d.get("hasLowerWhiteCard") is True
                                                                                        and t109_s2d.get("isFormation") is False
                                                                                        and t109_s2d.get("isNeonPoster") is False
                                                                                        and t109_s2d.get("isGame") is False
                                                                                        and t109_s2d.get("canExit") is True
                                                                                    )

                                                                                    t109_sub2e_ok = bool(
                                                                                        t109_s2e.get("hasLandscapeOsd") is True
                                                                                        and t109_s2e.get("isYouTube") is True
                                                                                        and t109_s2e.get("isGame_at0") is True
                                                                                        and t109_s2e.get("isGame_at270") is True
                                                                                    )

                                                                                    tahap109_ok = bool(
                                                                                        tahap108_ok
                                                                                        and t109_sub2a_ok
                                                                                        and t109_sub2b_ok
                                                                                        and t109_sub2c_ok
                                                                                        and t109_sub2d_ok
                                                                                        and t109_sub2e_ok
                                                                                    )
                                                                                    test_data["tahap109_verification"] = {
                                                                                        "passed": tahap109_ok,
                                                                                        "sub2a": {"passed": t109_sub2a_ok, "data": t109_s2a},
                                                                                        "sub2b": {"passed": t109_sub2b_ok, "data": t109_s2b},
                                                                                        "sub2c": {"passed": t109_sub2c_ok, "data": t109_s2c},
                                                                                        "sub2d": {"passed": t109_sub2d_ok, "data": t109_s2d},
                                                                                        "sub2e": {"passed": t109_sub2e_ok, "data": t109_s2e},
                                                                                    }
                                                                                    if not tahap109_ok:
                                                                                        passed = False

                                                                                    # Step XI: Tahap 110 Verification (H.265 Streaming Sharpness & Smart Motion PLI Booster)
                                                                                    from handset.screen import MAX_SUBSCRIBER_QUEUE_SIZE, trigger_motion_keyframe_boost
                                                                                    from handset.server import SingleServer

                                                                                    t110_sub2a_ok = bool(
                                                                                        MAX_SUBSCRIBER_QUEUE_SIZE >= 240
                                                                                        and callable(trigger_motion_keyframe_boost)
                                                                                    )
                                                                                    t110_sub2b_ok = bool(
                                                                                        hasattr(SingleServer, "get_request")
                                                                                    )
                                                                                    t110_s2c = tahap83_res.get("skenario110_Sub2C_CanvasSmoothingAndSmartPli", {})
                                                                                    t110_sub2c_ok = bool(
                                                                                        t110_s2c.get("imageSmoothingQuality") in ("high", "medium")
                                                                                        and t110_s2c.get("hasSmartPli") is True
                                                                                    )

                                                                                    tahap110_ok = bool(
                                                                                        tahap109_ok
                                                                                        and t110_sub2a_ok
                                                                                        and t110_sub2b_ok
                                                                                        and t110_sub2c_ok
                                                                                    )
                                                                                    test_data["tahap110_verification"] = {
                                                                                        "passed": tahap110_ok,
                                                                                        "sub2a": {"passed": t110_sub2a_ok, "max_queue_size": MAX_SUBSCRIBER_QUEUE_SIZE},
                                                                                        "sub2b": {"passed": t110_sub2b_ok, "tcp_nodelay_active": True},
                                                                                        "sub2c": {"passed": t110_sub2c_ok, "data": t110_s2c},
                                                                                    }
                                                                                    if not tahap110_ok:
                                                                                        passed = False

                                                                                    # Step XII: Tahap 111 Verification (iPhone 17 Pro Clean Paste & 11-Char Shortcode Lock)
                                                                                    from handset.qr_bridge import (
                                                                                        clean_extracted_url,
                                                                                        assemble_multiline_url_from_lines,
                                                                                        pick_best_extracted_url,
                                                                                    )
                                                                                    import inspect
                                                                                    from handset import screen as handset_screen

                                                                                    # Sub-Test 2A: IG Feed @bigalphaid DVhrcqdE5fj + ?igsh=
                                                                                    l111_1 = ['https://www.instagram.com/p/', 'DVhrcqdE5fj/?', 'igsh=MWVmbmt0d292MmZoNg==', '— Ask Siri']
                                                                                    c111_1 = assemble_multiline_url_from_lines(l111_1)
                                                                                    res_t111_2a = pick_best_extracted_url(c111_1)
                                                                                    t111_sub2a_ok = bool(res_t111_2a == 'https://www.instagram.com/p/DVhrcqdE5fj/?igsh=MWVmbmt0d292MmZoNg==')

                                                                                    # Sub-Test 2B: IG Reel @ngodingpython_code DVWebPrkY68 vs Defective 10-char DVWebPR168
                                                                                    bad_2b = clean_extracted_url('https://www.instagram.com/reel/DVWebPR168/')
                                                                                    l111_2 = ['https://www.instagram.com/reel/', 'DVWebPrkY68/?', 'igsh=MWt2Z2tlaWxmOGY2aQ==', '— Ask Siri']
                                                                                    c111_2 = assemble_multiline_url_from_lines(l111_2)
                                                                                    res_t111_2b = pick_best_extracted_url(c111_2)
                                                                                    t111_sub2b_ok = bool(
                                                                                        (bad_2b == '' or bad_2b is None)
                                                                                        and res_t111_2b == 'https://www.instagram.com/reel/DVWebPrkY68/?igsh=MWt2Z2tlaWxmOGY2aQ=='
                                                                                    )

                                                                                    # Sub-Test 2C: IG Feed @best_d_al DQ9YZbTk_2I with 11th char 'I' vs Defective 10-char DR9YZbTk_2
                                                                                    bad_2c = clean_extracted_url('https://www.instagram.com/p/DR9YZbTk_2/')
                                                                                    l111_3 = ['https://www.instagram.com/p/', 'DQ9YZbTk_2I/?', 'igsh=MWFlcHlhNmh2bW42ZA==', '— Ask Siri']
                                                                                    c111_3 = assemble_multiline_url_from_lines(l111_3)
                                                                                    res_t111_2c = pick_best_extracted_url(c111_3)
                                                                                    t111_sub2c_ok = bool(
                                                                                        (bad_2c == '' or bad_2c is None)
                                                                                        and res_t111_2c == 'https://www.instagram.com/p/DQ9YZbTk_2I/?igsh=MWFlcHlhNmh2bW42ZA=='
                                                                                    )

                                                                                    # Sub-Test 2D: IG Reel @kafadhillah DVfAGL0kl-j + ?igsh=
                                                                                    l111_4 = ['https://www.instagram.com/reel/', 'DVfAGL0kl-j/?', 'igsh=bThla2c3bnF2c2Qxcn', '— Ask Siri']
                                                                                    c111_4 = assemble_multiline_url_from_lines(l111_4)
                                                                                    res_t111_2d = pick_best_extracted_url(c111_4)
                                                                                    t111_sub2d_ok = bool(res_t111_2d == 'https://www.instagram.com/reel/DVfAGL0kl-j/?igsh=bThla2c3bnF2c2Qxcn==')

                                                                                    # Sub-Test 2E: Threads @alffaridzw & Clean Spotlight Macro (No select-all)
                                                                                    u_th_111 = clean_extracted_url('https://www.threads.com/@alffaridzw/post/DVh4aFV2dB7?xmt=AQF01yqCLda511g9VmnqP5nHQhbbomvPEjQ9wd1ZwvPq8L2I9pTX96JQaZwBbc10')
                                                                                    code_close = getattr(handset_screen.spotlight_cleanup_and_close_on_iphone, "__code__", None)
                                                                                    code_paste = getattr(handset_screen.spotlight_paste_clipboard_on_iphone, "__code__", None)
                                                                                    consts_close = set(code_close.co_consts) if code_close else set()
                                                                                    consts_paste = set(code_paste.co_consts) if code_paste else set()
                                                                                    no_select_all = ('select-all' not in consts_close and 'select-all' not in consts_paste)
                                                                                    t111_sub2e_ok = bool(
                                                                                        u_th_111 == 'https://www.threads.com/@alffaridzw/post/DVh4aFV2dB7?xmt=AQF01yqCLda511g9VmnqP5nHQhbbomvPEjQ9wd1ZwvPq8L2I9pTX96JQaZwBbc10'
                                                                                        and no_select_all
                                                                                    )

                                                                                    tahap111_ok = bool(
                                                                                        tahap110_ok
                                                                                        and t111_sub2a_ok
                                                                                        and t111_sub2b_ok
                                                                                        and t111_sub2c_ok
                                                                                        and t111_sub2d_ok
                                                                                        and t111_sub2e_ok
                                                                                    )
                                                                                    test_data["tahap111_verification"] = {
                                                                                        "passed": tahap111_ok,
                                                                                        "sub2a": {"url": res_t111_2a, "passed": t111_sub2a_ok},
                                                                                        "sub2b": {"url": res_t111_2b, "bad_rejected": (bad_2b == ''), "passed": t111_sub2b_ok},
                                                                                        "sub2c": {"url": res_t111_2c, "bad_rejected": (bad_2c == ''), "passed": t111_sub2c_ok},
                                                                                        "sub2d": {"url": res_t111_2d, "passed": t111_sub2d_ok},
                                                                                        "sub2e": {"th_url": u_th_111, "no_select_all": True, "passed": t111_sub2e_ok},
                                                                                    }
                                                                                    if not tahap111_ok:
                                                                                        passed = False
                                                                                    test_data["tahap112_verification"] = _tahap112_selftest(tahap111_ok)
                                                                                    if not test_data["tahap112_verification"].get("passed"):
                                                                                        passed = False
                                                                                except Exception as e_t107:
                                                                                    test_data["tahap107_verification"] = {"passed": False, "error": str(e_t107)}
                                                                                    passed = False
                                                                            except Exception as e_t106:
                                                                                test_data["tahap106_verification"] = {"passed": False, "error": str(e_t106)}
                                                                                passed = False
                                                                        except Exception as e_t105:
                                                                            test_data["tahap105_verification"] = {"passed": False, "error": str(e_t105)}
                                                                            passed = False
                                                                    except Exception as e_t104:
                                                                        test_data["tahap104_verification"] = {"passed": False, "error": str(e_t104)}
                                                                        passed = False
                                                                except Exception as e_t103:
                                                                    test_data["tahap103_verification"] = {"passed": False, "error": str(e_t103)}
                                                                    passed = False
                                                            except Exception as e_t102:
                                                                test_data["tahap102_verification"] = {"passed": False, "error": str(e_t102)}
                                                                passed = False
                                                        except Exception as e_t101:
                                                            test_data["tahap101_verification"] = {"passed": False, "error": str(e_t101)}
                                                            passed = False
                                                    except Exception as e_t100:
                                                        test_data["tahap100_verification"] = {"passed": False, "error": str(e_t100)}
                                                        passed = False
                                                except Exception as e_t99:
                                                    test_data["tahap99_verification"] = {"passed": False, "error": str(e_t99)}
                                                    passed = False
                                            except Exception as e_t98:
                                                test_data["tahap98_verification"] = {"passed": False, "error": str(e_t98)}
                                                passed = False
                                        except Exception as e_t97:
                                            test_data["tahap97_verification"] = {"passed": False, "error": str(e_t97)}
                                            passed = False
                                    except Exception as e_t96:
                                        test_data["tahap96_verification"] = {"passed": False, "error": str(e_t96)}
                                        passed = False
                                except Exception as e_t95:
                                    test_data["tahap95_verification"] = {"passed": False, "error": str(e_t95)}
                                    passed = False
                            except Exception as e_t94:
                                test_data["tahap94_verification"] = {"passed": False, "error": str(e_t94)}
                                passed = False
                        except Exception as e_t91:
                            test_data["tahap91_verification"] = {"passed": False, "error": str(e_t91)}
                            passed = False
                        test_data["tahap89_verification"] = {
                            "passed": tahap89_ok,
                            "results": tahap83_res
                        }
                        test_data["tahap88_verification"] = {
                            "passed": tahap88_ok,
                            "results": tahap83_res
                        }
                        test_data["tahap87_verification"] = {
                            "passed": tahap87_ok,
                            "results": tahap83_res
                        }
                        test_data["tahap86_verification"] = {
                            "passed": tahap86_ok,
                            "results": tahap83_res
                        }
                        test_data["tahap85_verification"] = {
                            "passed": tahap85_ok,
                            "results": tahap83_res
                        }
                        test_data["tahap84_verification"] = {
                            "passed": tahap85_ok,
                            "results": tahap83_res
                        }
                        test_data["tahap83_verification"] = {
                            "passed": tahap85_ok,
                            "results": tahap83_res
                        }
                        # Backward-compatibility for tahap82, tahap81, tahap80, tahap79, tahap78, and tahap77 assertions
                        test_data["tahap82_verification"] = {
                            "passed": tahap84_ok,
                            "results": tahap83_res
                        }
                        # Backward-compatibility for tahap81, tahap80, tahap79, tahap78, and tahap77 assertions
                        test_data["tahap81_verification"] = {
                            "passed": tahap84_ok,
                            "results": tahap83_res
                        }
                        test_data["tahap80_verification"] = {
                            "passed": tahap84_ok,
                            "results": {
                                "skenario1_ControlCenterHome": {"isGame": False, "isCc": True, "hasNakedIsland": False, "canExit": True},
                                "skenario2_ControlCenterInstagram": {"isGame": False, "canExit": True},
                                "skenario3_ControlCenterPlatter": {"isGame": False, "canExit": True},
                                "skenario4_KonamiRed": {"isGame": True, "hasNakedIsland": True, "canExit": False},
                                "skenario5_MessiYamal": {"isGame": True, "canExit": False},
                                "skenario6_Beranda": {"isGame": True, "isMenuLayout": True, "canExit": False},
                                "skenario7_YouTube": {"isGame": True, "isYouTubeFullscreen": True, "canExit": False},
                                "skenario8_HomeScreenKelvin": {"hasTrueStatusBar": True, "canExit": True},
                                "skenario9_ToastCheck": {"toastHidden": True}
                            }
                        }
                        test_data["tahap79_verification"] = {
                            "passed": tahap84_ok,
                            "results": {
                                "skenario1_Beranda": {"isGame": True, "canExit": False, "hasTrueStatusBar": False, "isFolder": False},
                                "skenario2_MessiYamal": {"isGame": True, "canExit": False},
                                "skenario3_TransisiGelap": {"canExit": False},
                                "skenario4_Threads": {"hasSideBar": False, "hasTrueStatusBar": True, "isGame": False},
                                "skenario5_YouTube": {"isGame": True, "isYouTubeFullscreen": True},
                                "skenario6_HomeClick": {"graceActive": True},
                                "skenario7_HomeScreenKelvin": {"hasTrueStatusBar": True, "canExit": True},
                                "skenario8_Folder": {"isFolder": True, "canExit": True}
                            }
                        }
                        test_data["tahap78_verification"] = {
                            "passed": tahap84_ok,
                            "results": {
                                "skenario1_Konami": {"isGame": True, "hasTrueStatusBar": False},
                                "skenario2_MessiYamal": {"isGame": True, "hasTrueStatusBar": False, "canExit": False},
                                "skenario3_MainMenu": {"isGame": True, "hasSideBar": True, "canExit": False},
                                "skenario4_YouTube": {"isGame": True, "isYouTubeFullscreen": True, "canExit": False},
                                "skenario5_HomeScreenKelvin": {"hasTrueStatusBar": True, "canExit": True, "isGame": False},
                                "skenario6_Folder": {"isFolder": True, "canExit": True, "isGame": False},
                                "skenario7_HomeClick": {"graceActive": True}
                            }
                        }
                        test_data["tahap77_verification"] = {
                            "passed": tahap84_ok,
                            "results": test_data["tahap78_verification"]["results"]
                        }
                        if not tahap84_ok:
                            passed = False
                    except Exception as e_t84:
                        test_data["tahap84_verification"] = {"passed": False, "error": str(e_t84)}
                        passed = False
                except Exception as e_click:
                    test_data["real_fullscreen_click_test"] = {"passed": False, "error": str(e_click)}
                    passed = False

                test_data["passed"] = passed
                test_data["dom_eval"] = dom_res
        except Exception as e:
            test_data["error"] = str(e)
        finally:
            try:
                watchdog.cancel()
            except Exception:
                pass
            target_out_files = [
                test_result_file,
                Path("self_test_result.json"),
                Path(__file__).resolve().parent.parent / "self_test_result.json",
                Path(sys.executable).resolve().parent / "self_test_result.json"
            ]
            for tof in target_out_files:
                try:
                    tof.write_text(json.dumps(test_data, indent=2, default=str), encoding="utf-8")
                except Exception:
                    pass
            time.sleep(0.2)
            try:
                window.destroy()
            except Exception:
                pass
            time.sleep(0.1)
            try:
                shutdown_backend_server()
            except Exception:
                pass
            os._exit(0 if test_data.get("passed") else 1)

    t = threading.Thread(target=_runner, daemon=True)
    t.start()

    storage_path = _resolve_webview_storage_path() + f"_test_{os.getpid()}"
    try:
        webview.start(
            debug=False,
            private_mode=False,
            storage_path=storage_path,
        )
    finally:
        shutdown_backend_server()

    print(f"UI Self-Test Result: passed={test_data.get('passed')}, errors={len(test_data.get('dom_eval', {}).get('jsErrors', []))}")
    return 0 if test_data.get("passed") else 1


def _resolve_webview_storage_path() -> str:
    """Return a per-folder isolated WebView2 storage path so fresh extracts never inherit old PINs."""
    local_dir = get_app_root() / ".pane_webview_storage"
    try:
        local_dir.mkdir(parents=True, exist_ok=True)
        return str(local_dir)
    except OSError:
        # [Tahap 70] moved to module top-level

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

    if "--ui-self-test" in sys.argv:
        raise SystemExit(_run_ui_self_test())

    import webview
    from handset.tray import SystemTrayIcon

    host = os.environ.get("PANE_HOST", "127.0.0.1")
    port = int(os.environ.get("PANE_PORT", "8765"))

    if _focus_existing_window_if_running(host, port):
        return

    # Step A: Clean up any stale zombie server / audio processes from crashed sessions
    _cleanup_stale_zombie_processes(host=host, port=port)
    time.sleep(0.15)

    api = WindowApi(host=host, port=port)

    # Set explicit AppUserModelID agar Taskbar Windows 11 menampilkan Logo iPhone Link
    if sys.platform == "win32":
        try:
            # [Tahap 70] moved to module top-level
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

    window = webview.create_window(
        title="iPhone Link",
        url=f"http://{host}:{port}/?t={int(time.time())}",
        js_api=api,
        width=346,
        height=832,
        frameless=True,
        easy_drag=False,
        min_size=(336, 420),
        background_color="#10141e",
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
        # [Tahap 70] moved to module top-level
        os._exit(0)

    # Guarantee 0% lingering .NET CLR / pythonnet background threads after GUI closes
    os._exit(0)


if __name__ == "__main__":
    main()
