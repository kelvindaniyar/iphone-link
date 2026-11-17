"""Local device window. Binds to loopback only."""

from __future__ import annotations

import asyncio
import base64
import contextlib
import hashlib
import http.client
import os
import json
import queue
import re
import shutil
import socket
import sys
import subprocess
import tempfile
import threading
import time
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, unquote, urlparse

_WIN_NO_WINDOW_KWARGS: dict = {}
if sys.platform == "win32":
    _si = subprocess.STARTUPINFO()
    _si.dwFlags |= subprocess.STARTF_USESHOWWINDOW
    _si.wShowWindow = 0  # SW_HIDE
    _WIN_NO_WINDOW_KWARGS = {
        "creationflags": getattr(subprocess, "CREATE_NO_WINDOW", 0x08000000),
        "startupinfo": _si,
    }


from handset.device import classify_device_family, read_handset, snapshot
from handset.mux import MuxError
from handset.screen import (
    _clean_ocr_line,
    _is_garbage_ocr_line,
    _is_generic_template_line,
    _is_notes_folder_list_frame,
    _score_gap_quality,
    _strip_top_unselected_debris,
    auto_correct_ocr_text,
    clear_recent_long_ocr_chunks,
    commit_and_reset_current_typed_line,
    ensure_paragraph_formatting,
    get_debug_typed_state,
    has_overlapping_body_line,
    lock_iphone_screen,
    process_ocr_batch_results,
    record_typed_char,
    record_typed_text,
    restart_screen,
    screen_status,
    send_button_direct,
    send_keys_direct,
    set_iphone_physical_brightness,
    start_screen,
    stitch_multiline_ocr_frames,
    stop_screen,
    toggle_onscreen_keyboard,
    trigger_ios_shortcut,
    type_text_to_iphone,
    wake_and_unlock_iphone,
    wipe_all_privacy_data,
)
from handset.wifi import (
    ensure_wifi_enabled_on_usb,
    get_wifi_status,
    pair_for_wifi,
    probe_wifi_device,
    wifi_paired_udids,
)
from handset.audio import audio_manager

from handset import get_app_root, get_bundle_root, patch_qr_bridge_hot_ocr


def get_writable_temp_dir() -> Path:
    """Return writable per-user temp directory (%TEMP%\\iPhoneLink) for OCR & debug artifacts."""
    temp_dir = Path(tempfile.gettempdir()) / "iPhoneLink"
    try:
        temp_dir.mkdir(parents=True, exist_ok=True)
    except Exception:
        pass
    return temp_dir


def get_writable_appdata_dir() -> Path:
    """Return writable per-user LocalAppData directory (%LOCALAPPDATA%\\iPhoneLink) with temp fallback."""
    base = os.environ.get("LOCALAPPDATA") or tempfile.gettempdir()
    app_dir = Path(base) / "iPhoneLink"
    try:
        app_dir.mkdir(parents=True, exist_ok=True)
        return app_dir
    except Exception:
        return get_writable_temp_dir()


def _is_writable_app_dir(path: Path) -> bool:
    """Check if directory is writable and not inside a UAC-protected Program Files location."""
    try:
        norm = os.path.normcase(str(path.resolve()))
        if "\\program files\\" in norm or "\\program files (x86)\\" in norm or norm.endswith("\\program files") or norm.endswith("\\program files (x86)"):
            return False
        if not path.is_dir():
            return False
        probe = path / f".writetest_{os.getpid()}"
        probe.write_text("ok", encoding="utf-8")
        probe.unlink(missing_ok=True)
        return True
    except Exception:
        return False


def _resolve_web_root() -> Path:
    app_root = get_app_root()
    bundle_root = get_bundle_root()
    for cand in (
        app_root.parent / "web",
        app_root / "web",
        app_root / "handset" / "web",
        bundle_root / "handset" / "web",
        Path(__file__).resolve().parent / "web",
    ):
        if cand.is_dir() and (cand / "index.html").is_file():
            return cand
    return Path(__file__).resolve().parent / "web"


def _sync_temp_ocr_script(src: Path) -> Path:
    """Mirror ocr_extractor.ps1 into %TEMP%\\iPhoneLink as an always-readable fallback."""
    dst = get_writable_temp_dir() / "ocr_extractor.ps1"
    try:
        src_stat = src.stat()
        needs_copy = (not dst.is_file()) or (dst.stat().st_size != src_stat.st_size) or (int(dst.stat().st_mtime) < int(src_stat.st_mtime))
        if needs_copy:
            shutil.copy2(src, dst)
    except Exception:
        pass
    return dst


def _resolve_ocr_script_path() -> str:
    for cand in (
        get_app_root() / "handset" / "ocr_extractor.ps1",
        get_app_root() / "ocr_extractor.ps1",
        get_app_root().parent / "ocr_extractor.ps1",
        get_bundle_root() / "handset" / "ocr_extractor.ps1",
        Path(__file__).resolve().parent / "ocr_extractor.ps1",
    ):
        if cand.is_file():
            _sync_temp_ocr_script(cand)
            return str(cand)
    temp_copy = get_writable_temp_dir() / "ocr_extractor.ps1"
    if temp_copy.is_file():
        return str(temp_copy)
    return str(Path(__file__).resolve().parent / "ocr_extractor.ps1")


ROOT = _resolve_web_root()


def get_settings_file_path() -> Path:
    app_root = get_app_root()
    if _is_writable_app_dir(app_root):
        return app_root / "user_settings.json"
    return get_writable_appdata_dir() / "user_settings.json"


SETTINGS_FILE = get_settings_file_path()


def get_folder_instance_id(app_root: Path | None = None) -> str:
    """Deterministic 12-char SHA-256 instance ID unique to this installation folder."""
    target = (app_root or get_app_root()).resolve()
    normalized = os.path.normcase(str(target))
    return hashlib.sha256(normalized.encode("utf-8")).hexdigest()[:12]


_IS_FRESH_EXTRACTED_FOLDER: bool = not get_settings_file_path().is_file()


def is_fresh_extracted_folder() -> bool:
    settings_file = get_settings_file_path()
    if not settings_file.is_file():
        return True
    try:
        data = json.loads(settings_file.read_text(encoding="utf-8"))
        if isinstance(data, dict) and data.get("folderInstanceId") == get_folder_instance_id():
            return False
    except Exception:
        pass
    return True
HOST = "0.0.0.0"
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
    "audioOut": "pc",
    "audioSinkEnabled": False,
    "audioVolume": 100,
    "audioMuted": False,
    "audioBufferPreset": "normal",
    "lipSync": True,
    "trackpadSpeed": 100,
    "crossPlatformShortcuts": True,
    "pastePcClipboard": True,
    "autoRotate": False,
    "deviceProfile": "auto",
    "customWindowSize": None,
    "autoCheckUpdate": True,
    "updateChannel": "stable",
    "language": "id",
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
_LAST_GRABBED_VALID_URL: tuple[float, str] = (0.0, "")
_LATEST_SHORTCUT_URL_CACHE: dict = {"url": "", "ts": 0.0}


def _get_pc_lan_ipv4() -> str:
    """Detect the primary LAN IPv4 address of this Windows PC without blocking on DNS."""
    try:
        sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        sock.settimeout(0.25)
        try:
            sock.connect(("8.8.8.8", 80))
            ip = sock.getsockname()[0]
            if ip and not ip.startswith("127.") and not ip.startswith("169.254."):
                return ip
        finally:
            sock.close()
    except Exception:
        pass
    try:
        for ip in socket.gethostbyname_ex(socket.gethostname())[2]:
            if ip and not ip.startswith("127.") and not ip.startswith("169.254."):
                return ip
    except Exception:
        pass
    return "192.168.1.X"


def _get_cached_iphone_lan_ipv4(pc_ip: str = "") -> str:
    """Read last known iPhone LAN IPv4 from wifi_state.json without touching frozen wifi.py."""
    import ipaddress

    candidates: list[Path] = []
    for env_key in ("APPDATA", "LOCALAPPDATA"):
        env_val = (os.environ.get(env_key) or "").strip()
        if env_val:
            candidates.append(Path(env_val) / "iPhone Link" / "wifi_state.json")
    candidates.extend([
        get_app_root() / "wifi_state.json",
        get_writable_appdata_dir() / "wifi_state.json",
        Path.cwd() / "wifi_state.json",
    ])
    for state_path in candidates:
        try:
            if not state_path.is_file():
                continue
            data = json.loads(state_path.read_text(encoding="utf-8", errors="ignore") or "{}")
            if not isinstance(data, dict):
                continue
            raw_ips: list[str] = []
            if isinstance(data.get("last_ip"), str):
                raw_ips.append(data["last_ip"])
            if isinstance(data.get("ip"), str):
                raw_ips.append(data["ip"])
            devs = data.get("devices")
            if isinstance(devs, dict):
                for dev in devs.values():
                    if isinstance(dev, dict) and isinstance(dev.get("last_ip"), str):
                        raw_ips.append(dev["last_ip"])
            for cand in raw_ips:
                cand_s = cand.strip()
                if not cand_s or cand_s == pc_ip:
                    continue
                addr = ipaddress.IPv4Address(cand_s)
                if not addr.is_loopback and not addr.is_link_local and not addr.is_unspecified:
                    return str(addr)
        except Exception:
            continue
    return ""


ICLOUD_SHORTCUT_OFFICIAL_URL = "https://www.icloud.com/shortcuts/bbb096d97e174c1386f15e31aedef8ff"


def get_shortcut_setup_info(port: int = 8765) -> dict:
    """Return dynamic LAN endpoint info, official iCloud link, and Dual-Mode Siri Prompt."""
    ip = _get_pc_lan_ipv4()
    iphone_ip = _get_cached_iphone_lan_ipv4(ip)
    template = f"http://{ip}:{int(port)}/c?u="
    siri_prompt = (
        'Create a new Shortcut named "Send to iPhone Link" that appears in the Share Sheet '
        "(with If there's no input: Get Clipboard). "
        "First action: Get URLs from Shortcut Input. "
        "Second action: URL Encode the URLs. "
        f'Third action: Get Contents of URL "{template}" followed by the URL Encoded Text.'
    )
    if iphone_ip and iphone_ip != ip:
        ip_warning_id = f"[GUNAKAN IP PC: {ip}] — Jangan gunakan IP iPhone ({iphone_ip}) di Shortcut!"
        ip_warning_en = f"[USE PC IP: {ip}] — Do not use iPhone IP ({iphone_ip}) in Shortcut!"
    else:
        ip_warning_id = f"[GUNAKAN IP PC: {ip}] — Pastikan memakai IP PC di atas (bukan IP iPhone)."
        ip_warning_en = f"[USE PC IP: {ip}] — Make sure to use the PC IP above (not iPhone IP)."
    return {
        "ok": True,
        "pc_lan_ip": ip,
        "iphone_lan_ip": iphone_ip,
        "ip_warning_id": ip_warning_id,
        "ip_warning_en": ip_warning_en,
        "port": int(port),
        "shortcut_endpoint_template": template,
        "icloud_shortcut_url": ICLOUD_SHORTCUT_OFFICIAL_URL,
        "siri_prompt_en": siri_prompt,
    }


def get_windows_clipboard_text() -> str | None:
    """Read plain/unicode text from the Windows PC clipboard safely in 64-bit."""
    try:
        import ctypes
        from ctypes import wintypes

        user32 = ctypes.windll.user32
        kernel32 = ctypes.windll.kernel32
        user32.OpenClipboard.argtypes = [wintypes.HWND]
        user32.OpenClipboard.restype = wintypes.BOOL
        user32.GetClipboardData.argtypes = [wintypes.UINT]
        user32.GetClipboardData.restype = wintypes.HANDLE
        user32.CloseClipboard.argtypes = []
        user32.CloseClipboard.restype = wintypes.BOOL
        kernel32.GlobalLock.argtypes = [wintypes.HGLOBAL]
        kernel32.GlobalLock.restype = ctypes.c_void_p
        kernel32.GlobalUnlock.argtypes = [wintypes.HGLOBAL]
        kernel32.GlobalUnlock.restype = wintypes.BOOL

        opened = False
        for _ in range(5):
            if user32.OpenClipboard(0):
                opened = True
                break
            time.sleep(0.015)
        if not opened:
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

        opened = False
        for attempt in range(5):
            if user32.OpenClipboard(0):
                opened = True
                break
            time.sleep(0.015)
        if not opened:
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


def _resolve_powershell_exe() -> str:
    """Resolve powershell.exe cleanly even under stripped worker PATH environments."""
    found = shutil.which("powershell.exe")
    if found:
        return found
    sys_root = os.environ.get("SystemRoot") or r"C:\Windows"
    cand = Path(sys_root) / "System32" / "WindowsPowerShell" / "v1.0" / "powershell.exe"
    if cand.is_file():
        return str(cand)
    return "powershell.exe"


class _Win32MemApi:
    """Cached Win32 ctypes signatures for accurate 64-bit HANDLE working-set queries & trimming."""
    _initialized = False
    _pmc_cls = None
    _k32 = None
    _psapi = None

    @classmethod
    def ensure(cls) -> bool:
        if cls._initialized:
            return cls._k32 is not None
        cls._initialized = True
        if sys.platform != "win32":
            return False
        try:
            import ctypes
            from ctypes import wintypes

            class _PROCESS_MEMORY_COUNTERS(ctypes.Structure):
                _fields_ = [
                    ("cb", wintypes.DWORD),
                    ("PageFaultCount", wintypes.DWORD),
                    ("PeakWorkingSetSize", ctypes.c_size_t),
                    ("WorkingSetSize", ctypes.c_size_t),
                    ("QuotaPeakPagedPoolUsage", ctypes.c_size_t),
                    ("QuotaPagedPoolUsage", ctypes.c_size_t),
                    ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t),
                    ("QuotaNonPagedPoolUsage", ctypes.c_size_t),
                    ("PagefileUsage", ctypes.c_size_t),
                    ("PeakPagefileUsage", ctypes.c_size_t),
                ]

            k32 = ctypes.WinDLL("kernel32", use_last_error=True)
            k32.GetCurrentProcess.argtypes = []
            k32.GetCurrentProcess.restype = wintypes.HANDLE
            k32.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
            k32.OpenProcess.restype = wintypes.HANDLE
            k32.CloseHandle.argtypes = [wintypes.HANDLE]
            k32.CloseHandle.restype = wintypes.BOOL
            if hasattr(k32, "K32GetProcessMemoryInfo"):
                k32.K32GetProcessMemoryInfo.argtypes = [
                    wintypes.HANDLE,
                    ctypes.POINTER(_PROCESS_MEMORY_COUNTERS),
                    wintypes.DWORD,
                ]
                k32.K32GetProcessMemoryInfo.restype = wintypes.BOOL
            if hasattr(k32, "K32EmptyWorkingSet"):
                k32.K32EmptyWorkingSet.argtypes = [wintypes.HANDLE]
                k32.K32EmptyWorkingSet.restype = wintypes.BOOL
            if hasattr(k32, "SetProcessWorkingSetSizeEx"):
                k32.SetProcessWorkingSetSizeEx.argtypes = [
                    wintypes.HANDLE,
                    ctypes.c_size_t,
                    ctypes.c_size_t,
                    wintypes.DWORD,
                ]
                k32.SetProcessWorkingSetSizeEx.restype = wintypes.BOOL
            if hasattr(k32, "SetProcessWorkingSetSize"):
                k32.SetProcessWorkingSetSize.argtypes = [
                    wintypes.HANDLE,
                    ctypes.c_size_t,
                    ctypes.c_size_t,
                ]
                k32.SetProcessWorkingSetSize.restype = wintypes.BOOL

            psapi = None
            try:
                psapi = ctypes.WinDLL("psapi", use_last_error=True)
                psapi.GetProcessMemoryInfo.argtypes = [
                    wintypes.HANDLE,
                    ctypes.POINTER(_PROCESS_MEMORY_COUNTERS),
                    wintypes.DWORD,
                ]
                psapi.GetProcessMemoryInfo.restype = wintypes.BOOL
                psapi.EmptyWorkingSet.argtypes = [wintypes.HANDLE]
                psapi.EmptyWorkingSet.restype = wintypes.BOOL
            except Exception:
                psapi = None

            cls._pmc_cls = _PROCESS_MEMORY_COUNTERS
            cls._k32 = k32
            cls._psapi = psapi
            return True
        except Exception:
            cls._k32 = None
            return False


def _get_win32_process_working_set_bytes(pid: int | None = None) -> int:
    """Return physical WorkingSetSize in bytes for current process (or target PID) on Windows."""
    if not _Win32MemApi.ensure():
        return 0
    try:
        import ctypes

        k32 = _Win32MemApi._k32
        psapi = _Win32MemApi._psapi
        pmc_cls = _Win32MemApi._pmc_cls
        assert k32 is not None and pmc_cls is not None

        close_after = False
        if pid is None or pid == os.getpid():
            h_proc = k32.GetCurrentProcess()
        else:
            # PROCESS_QUERY_INFORMATION (0x0400) | PROCESS_VM_READ (0x0010)
            h_proc = k32.OpenProcess(0x0410, False, int(pid))
            close_after = True
        if not h_proc:
            return 0
        try:
            pmc = pmc_cls()
            pmc.cb = ctypes.sizeof(pmc_cls)
            ok = False
            if hasattr(k32, "K32GetProcessMemoryInfo"):
                try:
                    ok = bool(k32.K32GetProcessMemoryInfo(h_proc, ctypes.byref(pmc), pmc.cb))
                except Exception:
                    ok = False
            if not ok and psapi is not None:
                try:
                    ok = bool(psapi.GetProcessMemoryInfo(h_proc, ctypes.byref(pmc), pmc.cb))
                except Exception:
                    ok = False
            return int(pmc.WorkingSetSize) if ok else 0
        finally:
            if close_after:
                k32.CloseHandle(h_proc)
    except Exception:
        return 0


def _trim_win32_process_working_set(pid: int | None = None) -> bool:
    """Trim Win32 physical working set via K32EmptyWorkingSet + SetProcessWorkingSetSizeEx(-1, -1)."""
    if not _Win32MemApi.ensure():
        return False
    try:
        import ctypes

        k32 = _Win32MemApi._k32
        psapi = _Win32MemApi._psapi
        assert k32 is not None

        close_after = False
        if pid is None or pid == os.getpid():
            h_proc = k32.GetCurrentProcess()
        else:
            # PROCESS_QUERY_INFORMATION (0x0400) | PROCESS_SET_QUOTA (0x0100)
            h_proc = k32.OpenProcess(0x0500, False, int(pid))
            close_after = True
        if not h_proc:
            return False
        try:
            ok_empty = False
            if hasattr(k32, "K32EmptyWorkingSet"):
                try:
                    ok_empty = bool(k32.K32EmptyWorkingSet(h_proc))
                except Exception:
                    pass
            if not ok_empty and psapi is not None:
                try:
                    ok_empty = bool(psapi.EmptyWorkingSet(h_proc))
                except Exception:
                    pass
            ok_ws = False
            neg_one = ctypes.c_size_t(-1)
            if hasattr(k32, "SetProcessWorkingSetSizeEx"):
                try:
                    ok_ws = bool(k32.SetProcessWorkingSetSizeEx(h_proc, neg_one, neg_one, 0))
                except Exception:
                    pass
            if not ok_ws and hasattr(k32, "SetProcessWorkingSetSize"):
                try:
                    ok_ws = bool(k32.SetProcessWorkingSetSize(h_proc, neg_one, neg_one))
                except Exception:
                    pass
            return bool(ok_empty or ok_ws)
        finally:
            if close_after:
                k32.CloseHandle(h_proc)
    except Exception:
        return False


def _heapmin_crt_arenas() -> None:
    """Release unused MSVCRT / UCRT native C-heap arenas back to Windows."""
    if sys.platform != "win32":
        return
    try:
        import ctypes
        for dll_name in ("ucrtbase", "msvcrt"):
            try:
                getattr(ctypes.cdll, dll_name)._heapmin()
            except Exception:
                pass
    except Exception:
        pass


_RECLAIM_MEMORY_LOCK = threading.RLock()


def reclaim_python_worker_memory(trim_hot_ocr: bool = False) -> dict:
    """Deterministically free transient PIL/frame caches, compact C heaps, and trim Win32 Working Set (Tahap 214)."""
    import gc

    with _RECLAIM_MEMORY_LOCK:
        before_bytes = _get_win32_process_working_set_bytes(None)
        try:
            qr_mod = sys.modules.get("handset.qr_bridge")
            if qr_mod is not None:
                cache = getattr(qr_mod, "_SPOTLIGHT_FRAME_CACHE", None)
                if isinstance(cache, list):
                    cache.clear()
        except Exception:
            pass

        try:
            gc.collect(0)
            gc.collect(1)
            gc.collect(2)
        except Exception:
            pass

        _heapmin_crt_arenas()
        _trim_win32_process_working_set(None)

        if trim_hot_ocr:
            try:
                worker = globals().get("_hot_ocr_worker")
                if worker is not None and hasattr(worker, "trim_memory"):
                    worker.trim_memory()
            except Exception:
                pass

        after_bytes = _get_win32_process_working_set_bytes(None)
        return {
            "ok": True,
            "python_before_mb": round(before_bytes / (1024 * 1024), 2),
            "python_after_mb": round(after_bytes / (1024 * 1024), 2),
            "reclaimed_mb": round(max(0, before_bytes - after_bytes) / (1024 * 1024), 2),
        }


sys._iphone_link_reclaim_memory_fn = reclaim_python_worker_memory


def _build_hot_ocr_driver_script(ocr_script_path: str) -> str:
    """Dynamically wrap handset/ocr_extractor.ps1 into a persistent stdin/stdout PowerShell loop without altering ocr_extractor.ps1."""
    raw = Path(ocr_script_path).read_text(encoding="utf-8", errors="replace")
    idx_start = raw.find("Add-Type -AssemblyName 'System.Runtime.WindowsRuntime'")
    idx_pathlist = raw.find("$pathList = @()")
    idx_proc = raw.find("function Process-SingleFrame")
    idx_last = raw.find("$lastIdx = $pathList.Count - 1")
    idx_catch = raw.rfind("} catch {")
    if not (0 <= idx_start < idx_pathlist < idx_proc < idx_last < idx_catch):
        raise RuntimeError("Failed to locate OCR driver markers in ocr_extractor.ps1")

    preamble = raw[idx_start:idx_pathlist]
    # Tahap 214: Dispose WinRT SoftwareBitmap & BitmapDecoder inside Get-OcrParsedLines and reuse $script:HotOcrEngine (0-diff on ocr_extractor.ps1).
    preamble = preamble.replace(
        "$engine = [Windows.Media.Ocr.OcrEngine]::TryCreateFromUserProfileLanguages()",
        "$engine = if ($null -ne $script:HotOcrEngine) { $script:HotOcrEngine } else { [Windows.Media.Ocr.OcrEngine]::TryCreateFromUserProfileLanguages(); $script:HotOcrEngine = $_ }",
    )
    preamble = preamble.replace(
        "$engine = if ($null -ne $script:HotOcrEngine) { $script:HotOcrEngine } else { [Windows.Media.Ocr.OcrEngine]::TryCreateFromUserProfileLanguages(); $script:HotOcrEngine = $_ }",
        "if ($null -eq $script:HotOcrEngine) { $script:HotOcrEngine = [Windows.Media.Ocr.OcrEngine]::TryCreateFromUserProfileLanguages() }\r\n        $engine = $script:HotOcrEngine",
    )
    preamble = preamble.replace(
        "$ocrResult = Await-Op ($engine.RecognizeAsync($softwareBitmap)) ([Windows.Media.Ocr.OcrResult])",
        "$ocrResult = Await-Op ($engine.RecognizeAsync($softwareBitmap)) ([Windows.Media.Ocr.OcrResult])\r\n        if ($null -ne $softwareBitmap) { try { $softwareBitmap.Dispose() } catch {} }\r\n        $softwareBitmap = $null\r\n        $decoder = $null\r\n        $storageFile = $null",
    )
    preamble = preamble.replace(
        "$stream.Dispose()",
        "if ($null -ne $softwareBitmap) { try { $softwareBitmap.Dispose() } catch {} }\r\n            $stream.Dispose()",
        1,
    )

    proc_fn = raw[idx_proc:idx_last]
    batch_body = raw[idx_last:idx_catch]
    batch_body = re.sub(
        r"\[Console\]::WriteLine\(\$outText\)\s*\r?\n\s*exit\s+0",
        "return $outText",
        batch_body,
    )
    batch_body = batch_body.replace(
        '[Console]::WriteLine($allOutputs -join "`r`n---FRAME_SPLIT---`r`n")',
        'return ($allOutputs -join "`r`n---FRAME_SPLIT---`r`n")',
    )
    # Tahap 213: Ensure no stray `exit <code >` inside Invoke-HotOcrBatch can ever kill the persistent PowerShell process.
    batch_body = re.sub(r"\bexit\s+\d+\b", 'return ""', batch_body)

    return (
        "$ErrorActionPreference = 'Stop'\r\n"
        "[Console]::InputEncoding = [System.Text.Encoding]::UTF8\r\n"
        "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8\r\n\r\n"
        + "if (-not (\"NativeMemReclaimer\" -as [type])) {\r\n"
        + "    Add-Type -TypeDefinition @\"\r\n"
        + "using System;\r\n"
        + "using System.Diagnostics;\r\n"
        + "using System.Runtime.InteropServices;\r\n"
        + "public static class NativeMemReclaimer {\r\n"
        + "    [DllImport(\"kernel32.dll\", SetLastError = true)]\r\n"
        + "    public static extern bool SetProcessWorkingSetSize(IntPtr hProcess, IntPtr dwMinimumWorkingSetSize, IntPtr dwMaximumWorkingSetSize);\r\n"
        + "    [DllImport(\"kernel32.dll\", EntryPoint = \"K32EmptyWorkingSet\", SetLastError = true)]\r\n"
        + "    public static extern bool K32EmptyWorkingSet(IntPtr hProcess);\r\n"
        + "    [DllImport(\"psapi.dll\", EntryPoint = \"EmptyWorkingSet\", SetLastError = true)]\r\n"
        + "    public static extern bool PsapiEmptyWorkingSet(IntPtr hProcess);\r\n"
        + "    public static void TrimCurrentProcess() {\r\n"
        + "        try {\r\n"
        + "            IntPtr h = Process.GetCurrentProcess().Handle;\r\n"
        + "            try { K32EmptyWorkingSet(h); } catch { try { PsapiEmptyWorkingSet(h); } catch {} }\r\n"
        + "            SetProcessWorkingSetSize(h, new IntPtr(-1), new IntPtr(-1));\r\n"
        + "        } catch {}\r\n"
        + "    }\r\n"
        + "}\r\n"
        + "\"@ -ErrorAction SilentlyContinue\r\n"
        + "}\r\n\r\n"
        + "function Invoke-HotOcrMemoryTrim {\r\n"
        + "    try {\r\n"
        + "        [System.GC]::Collect()\r\n"
        + "        [System.GC]::WaitForPendingFinalizers()\r\n"
        + "        [System.GC]::Collect()\r\n"
        + "        [NativeMemReclaimer]::TrimCurrentProcess()\r\n"
        + "    } catch {}\r\n"
        + "}\r\n\r\n"
        + preamble
        + "\r\n"
        + proc_fn
        + "\r\nfunction Invoke-HotOcrBatch([string[]]$pathList) {\r\n"
        + "    if ($null -eq $pathList -or $pathList.Count -eq 0) { return \"\" }\r\n"
        + batch_body
        + "\r\n}\r\n\r\n"
        + "function Invoke-RawWinRtBatch([string[]]$pathList) {\r\n"
        + "    $eng = $script:HotOcrEngine\r\n"
        + "    if ($null -eq $eng) {\r\n"
        + "        $eng = [Windows.Media.Ocr.OcrEngine]::TryCreateFromUserProfileLanguages()\r\n"
        + "        $script:HotOcrEngine = $eng\r\n"
        + "    }\r\n"
        + "    $chunks = @()\r\n"
        + "    foreach ($p in $pathList) {\r\n"
        + "        $linesOut = @()\r\n"
        + "        if ($null -ne $eng -and $p -and (Test-Path -LiteralPath $p)) {\r\n"
        + "            $stream = $null\r\n"
        + "            $sb = $null\r\n"
        + "            $dec = $null\r\n"
        + "            $sf = $null\r\n"
        + "            $res = $null\r\n"
        + "            try {\r\n"
        + "                $fullP = (Resolve-Path -LiteralPath $p).Path\r\n"
        + "                $sf = Await-Op ([Windows.Storage.StorageFile]::GetFileFromPathAsync($fullP)) ([Windows.Storage.StorageFile])\r\n"
        + "                $stream = Await-Op ($sf.OpenAsync([Windows.Storage.FileAccessMode]::Read)) ([Windows.Storage.Streams.IRandomAccessStream])\r\n"
        + "                $dec = Await-Op ([Windows.Graphics.Imaging.BitmapDecoder]::CreateAsync($stream)) ([Windows.Graphics.Imaging.BitmapDecoder])\r\n"
        + "                $sb = Await-Op ($dec.GetSoftwareBitmapAsync()) ([Windows.Graphics.Imaging.SoftwareBitmap])\r\n"
        + "                $res = Await-Op ($eng.RecognizeAsync($sb)) ([Windows.Media.Ocr.OcrResult])\r\n"
        + "                foreach ($l in $res.Lines) {\r\n"
        + "                    $lt = $l.Text.Trim()\r\n"
        + "                    if ($lt) { $linesOut += $lt }\r\n"
        + "                }\r\n"
        + "            } catch {\r\n"
        + "            } finally {\r\n"
        + "                if ($null -ne $sb) { try { $sb.Dispose() } catch {} }\r\n"
        + "                if ($null -ne $stream) { try { $stream.Dispose() } catch {} }\r\n"
        + "                $sb = $null; $stream = $null; $dec = $null; $sf = $null; $res = $null\r\n"
        + "            }\r\n"
        + "        }\r\n"
        + "        $chunks += ($linesOut -join \"`r`n\")\r\n"
        + "    }\r\n"
        + "    return ($chunks -join \"`r`n<<<RAW_IMG_SEP>>>`r`n\")\r\n"
        + "}\r\n\r\n"
        + "# Pre-warm WinRT OcrEngine, BitmapDecoder, StorageFile, GDI+, and C# DualOcrPreprocessor once\r\n"
        + "try {\r\n"
        + "    $script:HotOcrEngine = [Windows.Media.Ocr.OcrEngine]::TryCreateFromUserProfileLanguages()\r\n"
        + "    $warmupPng = Join-Path ([System.IO.Path]::GetTempPath()) (\"iphone_link_hot_ocr_warmup_\" + $PID + \".png\")\r\n"
        + "    try {\r\n"
        + "        $wb = New-Object System.Drawing.Bitmap(96, 48)\r\n"
        + "        $wg = [System.Drawing.Graphics]::FromImage($wb)\r\n"
        + "        $wg.Clear([System.Drawing.Color]::White)\r\n"
        + "        $wf = New-Object System.Drawing.Font(\"Arial\", 16)\r\n"
        + "        $wg.DrawString(\"TEST\", $wf, [System.Drawing.Brushes]::Black, 8, 8)\r\n"
        + "        $wg.Dispose()\r\n"
        + "        $wf.Dispose()\r\n"
        + "        $wb.Save($warmupPng, [System.Drawing.Imaging.ImageFormat]::Png)\r\n"
        + "        $wb.Dispose()\r\n"
        + "        [void](Process-SingleFrame -imageFilePath $warmupPng -isLast $true -isFullViewport $false -onlyCand1 $false)\r\n"
        + "    } finally {\r\n"
        + "        if (Test-Path $warmupPng) { Remove-Item $warmupPng -Force -ErrorAction SilentlyContinue }\r\n"
        + "        Invoke-HotOcrMemoryTrim\r\n"
        + "    }\r\n"
        + "} catch {}\r\n\r\n"
        + "[Console]::Out.WriteLine(\"<<<HOT_OCR_READY>>>\")\r\n"
        + "[Console]::Out.Flush()\r\n\r\n"
        + "while ($true) {\r\n"
        + "    $line = [Console]::In.ReadLine()\r\n"
        + "    if ($null -eq $line) { break }\r\n"
        + "    $trimmed = $line.Trim()\r\n"
        + "    if (-not $trimmed) { continue }\r\n"
        + "    try {\r\n"
        + "        $req = ConvertFrom-Json -InputObject $trimmed\r\n"
        + "        if ($req.cmd -eq \"exit\") { break }\r\n"
        + "        if ($req.cmd -eq \"trim_memory\") {\r\n"
        + "            Invoke-HotOcrMemoryTrim\r\n"
        + "            $tb64 = [Convert]::ToBase64String([System.Text.Encoding]::UTF8.GetBytes('{\"ok\":true,\"trimmed\":true}'))\r\n"
        + "            [Console]::Out.WriteLine(\"<<<OCR_B64:\" + $tb64 + \">>>\")\r\n"
        + "            [Console]::Out.WriteLine(\"<<<OCR_BATCH_DONE>>>\")\r\n"
        + "            [Console]::Out.Flush()\r\n"
        + "            continue\r\n"
        + "        }\r\n"
        + "        $reqPaths = @()\r\n"
        + "        if ($null -ne $req.paths) {\r\n"
        + "            $reqPaths = @($req.paths | ForEach-Object { [string]$_ })\r\n"
        + "        }\r\n"
        + "        if ($req.cmd -eq \"raw_winrt_batch\") {\r\n"
        + "            $resOut = Invoke-RawWinRtBatch -pathList $reqPaths\r\n"
        + "        } else {\r\n"
        + "            $resOut = Invoke-HotOcrBatch -pathList $reqPaths\r\n"
        + "        }\r\n"
        + "        $outText = \"\"\r\n"
        + "        if ($null -ne $resOut) {\r\n"
        + "            if ($resOut -is [System.Array]) {\r\n"
        + "                $outText = ($resOut -join \"`r`n\")\r\n"
        + "            } else {\r\n"
        + "                $outText = [string]$resOut\r\n"
        + "            }\r\n"
        + "        }\r\n"
        + "        $resOut = $null\r\n"
        + "        $b64 = [Convert]::ToBase64String([System.Text.Encoding]::UTF8.GetBytes($outText))\r\n"
        + "        $outText = \"\"\r\n"
        + "        [Console]::Out.WriteLine(\"<<<OCR_B64:\" + $b64 + \">>>\")\r\n"
        + "        [Console]::Out.WriteLine(\"<<<OCR_BATCH_DONE>>>\")\r\n"
        + "        [Console]::Out.Flush()\r\n"
        + "        Invoke-HotOcrMemoryTrim\r\n"
        + "    } catch {\r\n"
        + "        $errMsg = $_.Exception.Message\r\n"
        + "        $eb64 = [Convert]::ToBase64String([System.Text.Encoding]::UTF8.GetBytes($errMsg))\r\n"
        + "        [Console]::Out.WriteLine(\"<<<OCR_ERR:\" + $eb64 + \">>>\")\r\n"
        + "        [Console]::Out.WriteLine(\"<<<OCR_BATCH_DONE>>>\")\r\n"
        + "        [Console]::Out.Flush()\r\n"
        + "        Invoke-HotOcrMemoryTrim\r\n"
        + "    }\r\n"
        + "}\r\n"
    )


class _PersistentHotOcrWorker:
    """Thread-safe persistent PowerShell WinRT OCR worker eliminating ~800ms cold-spawn & Add-Type JIT overhead."""

    def __init__(self) -> None:
        self._state_lock = threading.Lock()
        self._io_lock = threading.Lock()
        self._proc: subprocess.Popen | None = None
        self._line_queue: queue.Queue[str | None] | None = None
        self._reader_thread: threading.Thread | None = None
        self._ready = False
        self._starting = False
        self._shutdown_requested = False
        self._ready_event = threading.Event()
        self._warmup_ms: float | None = None
        self._last_batch_ms: float | None = None
        self._batches_served: int = 0
        self._fallbacks_triggered: int = 0

    def _spawn_and_warm_sync(self, wait_timeout: float = 12.0) -> bool:
        if sys.platform != "win32":
            return False
        t0 = time.perf_counter()
        with self._state_lock:
            if self._shutdown_requested:
                return False
            if self._ready and self._proc is not None and self._proc.poll() is None:
                return True
            if self._starting:
                evt = self._ready_event
            else:
                self._starting = True
                self._ready = False
                self._ready_event.clear()
                evt = None

        if evt is not None:
            evt.wait(timeout=wait_timeout)
            return self._ready and self._proc is not None and self._proc.poll() is None

        try:
            script_path = _resolve_ocr_script_path()
            driver_ps1 = _build_hot_ocr_driver_script(script_path)
            driver_path = get_writable_temp_dir() / "_hot_ocr_driver.ps1"
            driver_path.write_text(driver_ps1, encoding="utf-8")

            line_q: queue.Queue[str | None] = queue.Queue()
            proc = subprocess.Popen(
                [_resolve_powershell_exe(), "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", str(driver_path)],
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL,
                text=True,
                encoding="utf-8",
                errors="replace",
                bufsize=1,
                **_WIN_NO_WINDOW_KWARGS,
            )

            def _reader() -> None:
                try:
                    assert proc.stdout is not None
                    for raw_line in proc.stdout:
                        line_q.put(raw_line.rstrip("\r\n"))
                except Exception:
                    pass
                finally:
                    line_q.put(None)

            rt = threading.Thread(target=_reader, name="hot-ocr-stdout-reader", daemon=True)
            rt.start()

            deadline = time.perf_counter() + wait_timeout
            ready_seen = False
            while time.perf_counter() < deadline:
                rem = max(0.05, deadline - time.perf_counter())
                try:
                    line = line_q.get(timeout=min(0.5, rem))
                except queue.Empty:
                    if proc.poll() is not None:
                        break
                    continue
                if line is None:
                    break
                if line.strip() == "<<<HOT_OCR_READY>>>":
                    ready_seen = True
                    break

            with self._state_lock:
                if ready_seen and not self._shutdown_requested and proc.poll() is None:
                    self._proc = proc
                    self._line_queue = line_q
                    self._reader_thread = rt
                    self._ready = True
                    self._warmup_ms = round((time.perf_counter() - t0) * 1000.0, 2)
                else:
                    self._ready = False
                    self._proc = None
                    self._terminate_proc(proc)
            if self._ready and self._proc is not None and self._proc.poll() is None:
                _trim_win32_process_working_set(self._proc.pid)
        except Exception as e:
            print(f"_PersistentHotOcrWorker warm error: {e}", flush=True)
            with self._state_lock:
                self._ready = False
                self._proc = None
        finally:
            with self._state_lock:
                self._starting = False
                self._ready_event.set()
        return self._ready

    def prewarm_async(self) -> None:
        if sys.platform != "win32" or self._shutdown_requested:
            return
        with self._state_lock:
            if self._ready or self._starting:
                return
        threading.Thread(target=self._spawn_and_warm_sync, kwargs={"wait_timeout": 15.0}, daemon=True, name="hot-ocr-prewarm").start()

    def _send_driver_command(self, cmd_payload: dict, timeout: float = 8.0) -> str | None:
        """Execute a single JSON command against the persistent PowerShell WinRT OCR worker."""
        if sys.platform != "win32" or self._shutdown_requested:
            return None
        with self._io_lock:
            if not (self._ready and self._proc is not None and self._proc.poll() is None):
                if not self._spawn_and_warm_sync(wait_timeout=12.0):
                    self._fallbacks_triggered += 1
                    return None

            proc = self._proc
            line_q = self._line_queue
            if proc is None or proc.poll() is not None or proc.stdin is None or line_q is None:
                self._reset_worker()
                self._fallbacks_triggered += 1
                return None

            t0 = time.perf_counter()
            try:
                while not line_q.empty():
                    try:
                        line_q.get_nowait()
                    except queue.Empty:
                        break
                payload = json.dumps(cmd_payload, ensure_ascii=False)
                proc.stdin.write(payload + "\n")
                proc.stdin.flush()

                deadline = time.perf_counter() + timeout
                decoded_text: str | None = None
                err_text: str | None = None
                done = False
                while time.perf_counter() < deadline:
                    rem = max(0.02, deadline - time.perf_counter())
                    try:
                        line = line_q.get(timeout=min(0.25, rem))
                    except queue.Empty:
                        if proc.poll() is not None:
                            break
                        continue
                    if line is None:
                        break
                    s = line.strip()
                    if s.startswith("<<<OCR_B64:") and s.endswith(">>>"):
                        b64 = s[len("<<<OCR_B64:"):-3]
                        decoded_text = base64.b64decode(b64).decode("utf-8", errors="replace")
                    elif s.startswith("<<<OCR_ERR:") and s.endswith(">>>"):
                        eb64 = s[len("<<<OCR_ERR:"):-3]
                        err_text = base64.b64decode(eb64).decode("utf-8", errors="replace")
                    elif s == "<<<OCR_BATCH_DONE>>>":
                        done = True
                        break

                if not done or err_text is not None or decoded_text is None:
                    if err_text:
                        print(f"Hot OCR worker batch error (falling back): {err_text}", flush=True)
                    self._reset_worker()
                    self._fallbacks_triggered += 1
                    return None

                self._last_batch_ms = round((time.perf_counter() - t0) * 1000.0, 2)
                if cmd_payload.get("cmd") != "trim_memory":
                    self._batches_served += 1
                _trim_win32_process_working_set(proc.pid)
                return decoded_text
            except Exception as e:
                print(f"Hot OCR worker pipe failure (falling back): {e}", flush=True)
                self._reset_worker()
                self._fallbacks_triggered += 1
                return None

    def run_batch(self, tmp_files: list[str], timeout: float = 8.0) -> str | None:
        if not tmp_files:
            return None
        return self._send_driver_command({"paths": tmp_files}, timeout=timeout)

    def run_raw_winrt_batch(self, imgs: list, timeout: float = 8.0) -> list[list[str]] | None:
        """Run raw WinRT OcrResult line extraction on PIL images via the persistent PowerShell worker."""
        if sys.platform != "win32" or self._shutdown_requested:
            return None
        if not imgs:
            return []

        results: list[list[str]] = [[] for _ in imgs]
        temp_ocr_dir = get_writable_temp_dir()
        active_indices: list[int] = []
        active_paths: list[str] = []
        pid = os.getpid()
        tid = threading.get_ident()
        try:
            for orig_i, im in enumerate(imgs):
                gray = None
                gray_crop = None
                try:
                    gray = im.convert("L")
                    w_im, h_im = gray.size
                    if w_im > 80 and h_im > 80:
                        gray_crop = gray.crop((38, 38, w_im - 38, h_im - 38))
                        lo_c, hi_c = gray_crop.getextrema()
                    else:
                        lo_c, hi_c = gray.getextrema()
                    if (hi_c - lo_c) < 18:
                        continue
                except Exception:
                    pass
                finally:
                    if gray_crop is not None:
                        try:
                            gray_crop.close()
                        except Exception:
                            pass
                    if gray is not None:
                        try:
                            gray.close()
                        except Exception:
                            pass
                out_path = temp_ocr_dir / f"hot_raw_{pid}_{tid}_{orig_i}.png"
                im.save(out_path, format="PNG")
                active_indices.append(orig_i)
                active_paths.append(str(out_path))

            if not active_paths:
                return results

            raw_out = self._send_driver_command({"cmd": "raw_winrt_batch", "paths": active_paths}, timeout=timeout)
            if raw_out is None:
                return None

            chunks = raw_out.split("<<<RAW_IMG_SEP>>>")
            for orig_i, chunk in zip(active_indices, chunks):
                results[orig_i] = [ln.strip() for ln in chunk.splitlines() if ln.strip()]
            return results
        except Exception as e:
            print(f"Hot OCR raw WinRT batch error (falling back): {e}", flush=True)
            return None
        finally:
            for p_str in active_paths:
                try:
                    os.remove(p_str)
                except Exception:
                    pass

    def trim_memory(self) -> dict:
        """Actively run .NET GC + Win32 Working Set Trim on persistent Hot OCR PowerShell process."""
        with self._state_lock:
            proc = self._proc
            alive = proc is not None and proc.poll() is None and proc.pid is not None
            ps_pid = proc.pid if alive and proc is not None else None
        if not ps_pid:
            return {"ok": True, "hot_ocr_alive": False, "hot_ocr_ps_working_set_mb": 0.0}
        before_b = _get_win32_process_working_set_bytes(ps_pid)
        _trim_win32_process_working_set(ps_pid)
        try:
            self._send_driver_command({"cmd": "trim_memory"}, timeout=3.0)
        except Exception:
            pass
        time.sleep(0.015)
        _trim_win32_process_working_set(ps_pid)
        after_b = _get_win32_process_working_set_bytes(ps_pid)
        return {
            "ok": True,
            "hot_ocr_alive": True,
            "pid": ps_pid,
            "before_mb": round(before_b / (1024 * 1024), 2),
            "hot_ocr_ps_working_set_mb": round(after_b / (1024 * 1024), 2),
        }

    def _terminate_proc(self, proc: subprocess.Popen | None) -> None:
        if proc is None:
            return
        try:
            if proc.stdin is not None:
                try:
                    proc.stdin.write('{"cmd":"exit"}\n')
                    proc.stdin.flush()
                    proc.stdin.close()
                except Exception:
                    pass
            try:
                proc.wait(timeout=0.8)
            except Exception:
                proc.kill()
        except Exception:
            pass

    def _reset_worker(self) -> None:
        with self._state_lock:
            proc = self._proc
            self._proc = None
            self._line_queue = None
            self._ready = False
        self._terminate_proc(proc)

    def shutdown(self) -> None:
        with self._state_lock:
            self._shutdown_requested = True
            proc = self._proc
            self._proc = None
            self._line_queue = None
            self._ready = False
        self._terminate_proc(proc)

    def status(self) -> dict:
        with self._state_lock:
            proc = self._proc
            alive = proc is not None and proc.poll() is None
            return {
                "ok": True,
                "hot_ocr_engine_ready": bool(self._ready and alive),
                "worker_alive": bool(alive),
                "pid": proc.pid if alive and proc is not None else None,
                "batches_served": self._batches_served,
                "fallbacks_triggered": self._fallbacks_triggered,
                "warmup_ms": self._warmup_ms,
                "last_batch_ms": self._last_batch_ms,
            }


_existing_hot_ocr = getattr(sys, "_iphone_link_hot_ocr_worker", None)
if (
    hasattr(_existing_hot_ocr, "run_batch")
    and hasattr(_existing_hot_ocr, "run_raw_winrt_batch")
    and hasattr(_existing_hot_ocr, "trim_memory")
    and not getattr(_existing_hot_ocr, "_shutdown_requested", False)
):
    _hot_ocr_worker = _existing_hot_ocr
else:
    if hasattr(_existing_hot_ocr, "shutdown"):
        try:
            _existing_hot_ocr.shutdown()
        except Exception:
            pass
    _hot_ocr_worker = _PersistentHotOcrWorker()
    sys._iphone_link_hot_ocr_worker = _hot_ocr_worker

if "handset.qr_bridge" in sys.modules:
    patch_qr_bridge_hot_ocr(sys.modules["handset.qr_bridge"])


def get_hot_ocr_status() -> dict:
    """Return runtime health & telemetry for the persistent Hot WinRT OCR engine."""
    return _hot_ocr_worker.status()


def get_system_memory_telemetry(reclaim: bool = False) -> dict:
    """Return exact Win32 physical WorkingSetSize for Python worker and persistent PowerShell Hot OCR worker (Tahap 214)."""
    with _RECLAIM_MEMORY_LOCK:
        reclaim_report = None
        if reclaim:
            reclaim_report = reclaim_python_worker_memory(trim_hot_ocr=True)
        py_ws = _get_win32_process_working_set_bytes(None)
        st = _hot_ocr_worker.status()
        ps_pid = st.get("pid")
        ps_ws = _get_win32_process_working_set_bytes(ps_pid) if ps_pid else 0
        py_mb = round(py_ws / (1024 * 1024), 2)
        ps_mb = round(ps_ws / (1024 * 1024), 2)
        out = {
            "ok": True,
            "reclaimed": bool(reclaim),
            "python_worker_pid": os.getpid(),
            "python_worker_working_set_bytes": py_ws,
            "python_worker_working_set_mb": py_mb,
            "hot_ocr_ps_pid": ps_pid,
            "hot_ocr_ps_working_set_bytes": ps_ws,
            "hot_ocr_ps_working_set_mb": ps_mb,
            "hot_ocr_alive": bool(st.get("worker_alive")),
            "rust_shell_working_set_mb": 0.0,
            "total_backend_working_set_mb": round(py_mb + ps_mb, 2),
        }
        if reclaim_report is not None:
            out["reclaim_detail"] = reclaim_report
        return out


# ======================================================================
# Tahap 221: 3uTools-Style Diagnostics, Offline Driver Repair, & 1-Click Unlock Developer Mode
# ======================================================================
_DIAGNOSTICS_LOCK = threading.RLock()
_HEVC_STATUS_CACHE: dict = {"ts": 0.0, "installed": None}
_AUTO_REVEALED_DEVMODE_UDIDS: set[str] = set()


def _check_hevc_codec_installed(force: bool = False) -> bool:
    """Detect whether Microsoft HEVC Video Extensions is installed (60s TTL cached)."""
    now = time.monotonic()
    with _DIAGNOSTICS_LOCK:
        cached = _HEVC_STATUS_CACHE.get("installed")
        if not force and cached is not None and (now - float(_HEVC_STATUS_CACHE.get("ts", 0.0))) < 60.0:
            return bool(cached)

    installed = False
    if sys.platform == "win32":
        try:
            win_apps = Path(os.environ.get("ProgramFiles", r"C:\Program Files")) / "WindowsApps"
            if any(win_apps.glob("Microsoft.HEVCVideoExtension*")):
                installed = True
        except OSError:
            pass
        if not installed:
            try:
                proc = subprocess.run(
                    [
                        "powershell",
                        "-NoProfile",
                        "-NonInteractive",
                        "-ExecutionPolicy",
                        "Bypass",
                        "-Command",
                        "if (Get-AppxPackage -Name '*HEVCVideoExtension*' -ErrorAction SilentlyContinue) { '1' } else { '0' }",
                    ],
                    capture_output=True,
                    text=True,
                    timeout=8,
                    check=False,
                    **_WIN_NO_WINDOW_KWARGS,
                )
                installed = proc.returncode == 0 and "1" in (proc.stdout or "").strip()
            except Exception:
                installed = False
    with _DIAGNOSTICS_LOCK:
        _HEVC_STATUS_CACHE["ts"] = now
        _HEVC_STATUS_CACHE["installed"] = bool(installed)
    return bool(installed)


def _find_bundled_apple_driver_files() -> tuple[Path | None, Path | None]:
    """Locate bundled install_apple_driver.ps1 and AppleMobileDeviceSupport64.msi."""
    roots: list[Path] = []
    for candidate_root in (get_app_root(), get_bundle_root()):
        if isinstance(candidate_root, Path) and candidate_root not in roots:
            roots.append(candidate_root)
    ps1_path: Path | None = None
    msi_path: Path | None = None
    for root in roots:
        for rel_ps1 in (Path("drivers") / "install_apple_driver.ps1", Path("release") / "install_apple_driver.ps1"):
            p = root / rel_ps1
            if ps1_path is None and p.is_file():
                ps1_path = p
        for rel_msi in (
            Path("drivers") / "AppleMobileDeviceSupport64.msi",
            Path("release") / "AppleMobileDeviceSupport64.msi",
            Path("assets") / "AppleMobileDeviceSupport64.msi",
        ):
            p = root / rel_msi
            if msi_path is None and p.is_file():
                msi_path = p
    if msi_path is None:
        cache_cand = Path(r"C:\ProgramData\Apple\Installer Cache\Apple Mobile Device Support 19.4.0.10\AppleMobileDeviceSupport64.msi")
        if cache_cand.is_file():
            msi_path = cache_cand
    return ps1_path, msi_path


def _is_apple_usb_driver_installed(drv: dict) -> bool:
    """Check Apple USB Driver directories, DriverStore usbaapl64.inf, or active service."""
    from handset.device import DRIVER_PATHS

    if bool(drv.get("ready")):
        return True
    if any(p.exists() for p in DRIVER_PATHS):
        return True
    try:
        repo = Path(r"C:\Windows\System32\DriverStore\FileRepository")
        if repo.is_dir() and any(repo.glob("usbaapl64.inf*")):
            return True
    except OSError:
        pass
    return False


def _query_devmode_lockdown_fallback(udid: str) -> bool | None:
    """Fallback direct Lockdown/pymobiledevice3 query when cached developerMode is None."""
    from handset.mux import list_devices, lockdown_request, open_lockdown

    try:
        connected = list_devices()
        match = next(
            (d for d in connected if d.get("udid") == udid and str(d.get("connection", "USB")).upper() == "USB"),
            None,
        )
        if match is not None:
            with open_lockdown(int(match["deviceId"])) as sock:
                res = lockdown_request(sock, "GetValue", {"Domain": "com.apple.security.mac.amfi", "Key": "DeveloperModeStatus"})
                val = res.get("Value")
                if isinstance(val, bool):
                    return val
    except Exception:
        pass

    async def _async_query() -> bool | None:
        from pymobiledevice3.lockdown import create_using_usbmux

        lockdown = await create_using_usbmux(serial=udid, autopair=False)
        try:
            return bool(await lockdown.get_developer_mode_status())
        finally:
            with contextlib.suppress(Exception):
                await lockdown.close()

    try:
        return asyncio.run(_async_query())
    except Exception:
        return None


def _resolve_developer_mode_status(
    udid: str | None,
    has_usb: bool,
    force_refresh: bool = False,
) -> tuple[bool | None, str]:
    """Resolve exact Developer Mode state and trigger background AMFI auto-reveal if disabled."""
    from handset.device import (
        _DEVELOPER_MODE_CACHE,
        _DEVICE_CACHE_LOCK,
        _DEV_MODE_RETRY_AFTER,
        developer_mode_enabled,
    )

    if has_usb and udid:
        if force_refresh:
            with _DEVICE_CACHE_LOCK:
                _DEVELOPER_MODE_CACHE.pop(udid, None)
                _DEV_MODE_RETRY_AFTER.pop(udid, None)
        dev_bool = developer_mode_enabled(udid)
        if dev_bool is None:
            dev_bool = _query_devmode_lockdown_fallback(udid)
            if dev_bool is not None:
                with _DEVICE_CACHE_LOCK:
                    _DEVELOPER_MODE_CACHE[udid] = bool(dev_bool)
                    _DEV_MODE_RETRY_AFTER.pop(udid, None)
        if dev_bool is None:
            # Never return '-' when an iPhone is connected via USB: if already streaming it's True, else False
            st = screen_status()
            dev_bool = bool(st.get("phase") == "running")
        if dev_bool is False:
            with _DIAGNOSTICS_LOCK:
                should_reveal = udid not in _AUTO_REVEALED_DEVMODE_UDIDS
                if should_reveal:
                    _AUTO_REVEALED_DEVMODE_UDIDS.add(udid)
            if should_reveal:
                threading.Thread(
                    target=_unlock_developer_mode_on_device,
                    kwargs={"udid": udid, "arm_enable": False},
                    daemon=True,
                ).start()
        return bool(dev_bool), ("enabled" if dev_bool else "disabled")

    if udid:
        with _DEVICE_CACHE_LOCK:
            cached = _DEVELOPER_MODE_CACHE.get(udid)
        if isinstance(cached, bool):
            return cached, ("enabled" if cached else "disabled")
    return None, "disconnected"


def _get_detailed_driver_and_device_diagnostics(
    udid: str | None = None,
    has_usb: bool | None = None,
    force_refresh: bool = False,
) -> dict:
    """Return 3uTools-style 5-point diagnostics for Device & Connection Details card."""
    from handset.device import _mux_listening, clear_device_caches, driver_status
    from handset.mux import list_devices

    if force_refresh:
        clear_device_caches(udid)

    if has_usb is None or udid is None:
        try:
            devs = list_devices()
        except Exception:
            devs = []
        usb_devs = [d for d in devs if str(d.get("connection", "")).upper() == "USB" and d.get("udid")]
        if has_usb is None:
            has_usb = len(usb_devs) > 0
        if udid is None and usb_devs:
            udid = usb_devs[0].get("udid")

    drv = driver_status()
    apple_driver_installed = _is_apple_usb_driver_installed(drv)
    mux_up = _mux_listening()
    apple_service_running = bool(drv.get("service") == "RUNNING" or mux_up)
    tcp_port_27015_normal = bool(mux_up or apple_service_running)
    hevc_codec_installed = _check_hevc_codec_installed(force=force_refresh)
    dev_bool, dev_status = _resolve_developer_mode_status(udid, bool(has_usb), force_refresh=force_refresh)

    if dev_status == "enabled":
        dev_label_en = "Enabled (Active)"
        dev_label_id = "Aktif (Enabled)"
    elif dev_status == "disabled":
        dev_label_en = "Disabled (Click Unlock)"
        dev_label_id = "Nonaktif (Klik Unlock)"
    else:
        dev_label_en = "Connect iPhone via USB"
        dev_label_id = "Hubungkan iPhone via USB"

    return {
        "ok": True,
        "udid": udid,
        "usb_connected": bool(has_usb),
        "apple_driver_installed": apple_driver_installed,
        "apple_driver_label": "Installed" if apple_driver_installed else "Not Installed",
        "apple_driver_label_id": "Terpasang" if apple_driver_installed else "Belum Terpasang",
        "apple_service_running": apple_service_running,
        "apple_service_label": "Running (Started)" if apple_service_running else "Stopped",
        "apple_service_label_id": "Berjalan (Started)" if apple_service_running else "Berhenti",
        "tcp_port_27015_normal": tcp_port_27015_normal,
        "tcp_port_label": "Normal" if tcp_port_27015_normal else "Unreachable",
        "tcp_port_label_id": "Normal" if tcp_port_27015_normal else "Tidak Terjangkau",
        "hevc_codec_installed": hevc_codec_installed,
        "hevc_codec_label": "Installed (60fps)" if hevc_codec_installed else "Missing",
        "hevc_codec_label_id": "Terpasang (60fps)" if hevc_codec_installed else "Belum Ada",
        "developer_mode_status": dev_status,
        "developer_mode_bool": dev_bool,
        "developer_mode_label": dev_label_en,
        "developer_mode_label_id": dev_label_id,
    }


def _repair_apple_driver_and_service() -> dict:
    """Repair or install Apple USB Driver & start Apple Mobile Device Service silently."""
    from handset.device import clear_device_caches, driver_status

    svc_name = "Apple Mobile Device Service"
    if sys.platform == "win32":
        with contextlib.suppress(Exception):
            subprocess.run(
                ["sc", "config", svc_name, "start=", "auto"],
                capture_output=True,
                text=True,
                timeout=10,
                check=False,
                **_WIN_NO_WINDOW_KWARGS,
            )
        with contextlib.suppress(Exception):
            subprocess.run(
                ["sc", "start", svc_name],
                capture_output=True,
                text=True,
                timeout=12,
                check=False,
                **_WIN_NO_WINDOW_KWARGS,
            )

    clear_device_caches()
    drv_now = driver_status()
    ps1_path, msi_path = _find_bundled_apple_driver_files()

    if sys.platform == "win32" and (not _is_apple_usb_driver_installed(drv_now) or drv_now.get("service") != "RUNNING"):
        if ps1_path is not None:
            with contextlib.suppress(Exception):
                subprocess.run(
                    [
                        "powershell",
                        "-WindowStyle",
                        "Hidden",
                        "-NoProfile",
                        "-NonInteractive",
                        "-ExecutionPolicy",
                        "Bypass",
                        "-File",
                        str(ps1_path),
                    ],
                    capture_output=True,
                    text=True,
                    timeout=90,
                    check=False,
                    **_WIN_NO_WINDOW_KWARGS,
                )
        elif msi_path is not None and not _is_apple_usb_driver_installed(drv_now):
            with contextlib.suppress(Exception):
                subprocess.run(
                    ["msiexec.exe", "/i", str(msi_path), "/qn", "/norestart"],
                    capture_output=True,
                    text=True,
                    timeout=90,
                    check=False,
                    **_WIN_NO_WINDOW_KWARGS,
                )
        with contextlib.suppress(Exception):
            subprocess.run(
                ["sc", "config", svc_name, "start=", "auto"],
                capture_output=True,
                text=True,
                timeout=10,
                check=False,
                **_WIN_NO_WINDOW_KWARGS,
            )
        with contextlib.suppress(Exception):
            subprocess.run(
                ["sc", "start", svc_name],
                capture_output=True,
                text=True,
                timeout=12,
                check=False,
                **_WIN_NO_WINDOW_KWARGS,
            )

    clear_device_caches()
    diag = _get_detailed_driver_and_device_diagnostics(force_refresh=True)
    ok = bool(diag.get("apple_driver_installed") and diag.get("apple_service_running"))
    return {
        "ok": True,
        "repaired": ok,
        "driver_ready": bool(diag.get("apple_driver_installed")),
        "service_running": bool(diag.get("apple_service_running")),
        "bundled_msi_found": msi_path is not None,
        "diagnostics": diag,
        "driver": driver_status(),
        "message_id": (
            "Layanan Apple Mobile Device Service & Driver USB aktif dan siap digunakan."
            if ok
            else "Perbaikan driver selesai dijalankan. Jika belum aktif, jalankan Repair sebagai Administrator atau pasang ulang Setup.exe."
        ),
        "message_en": (
            "Apple Mobile Device Service & USB Driver verified and running."
            if ok
            else "Driver repair finished. If still inactive, run Repair as Administrator or re-run Setup.exe."
        ),
    }


def _unlock_developer_mode_on_device(udid: str | None = None, arm_enable: bool = False) -> dict:
    """Reveal Developer Mode menu on a connected USB iPhone via pymobiledevice3 AmfiService."""
    from handset.device import _DEVELOPER_MODE_CACHE, _DEVICE_CACHE_LOCK, _DEV_MODE_RETRY_AFTER
    from handset.mux import list_devices

    try:
        connected = list_devices()
    except Exception:
        connected = []
    usb_devices = [d for d in connected if str(d.get("connection", "")).upper() == "USB" and d.get("udid")]
    if not usb_devices:
        return {
            "ok": False,
            "connected": False,
            "udid": None,
            "revealed": False,
            "developer_mode_status": "disconnected",
            "error": "No USB iPhone connected",
            "message_id": "Hubungkan iPhone menggunakan kabel data USB terlebih dahulu, buka kunci layar, dan ketuk Trust This Computer.",
            "message_en": "Connect your iPhone with a USB data cable first, unlock the screen, and tap Trust This Computer.",
        }

    target_udid = udid if udid and any(d.get("udid") == udid for d in usb_devices) else str(usb_devices[0]["udid"])

    async def _run_amfi() -> tuple[bool, str]:
        from pymobiledevice3.lockdown import create_using_usbmux
        from pymobiledevice3.services.amfi import AmfiService

        lockdown = await create_using_usbmux(serial=target_udid, autopair=True)
        try:
            amfi = AmfiService(lockdown)
            await amfi.reveal_developer_mode_option_in_ui()
            if arm_enable:
                with contextlib.suppress(Exception):
                    await amfi.enable_developer_mode(enable_post_restart=False)
            dev_status = bool(await lockdown.get_developer_mode_status())
            return dev_status, str(getattr(lockdown, "udid", target_udid) or target_udid)
        finally:
            with contextlib.suppress(Exception):
                await lockdown.close()

    try:
        dev_enabled, resolved_udid = asyncio.run(_run_amfi())
        with _DEVICE_CACHE_LOCK:
            _DEVELOPER_MODE_CACHE[resolved_udid] = bool(dev_enabled)
            _DEV_MODE_RETRY_AFTER.pop(resolved_udid, None)
        with _DIAGNOSTICS_LOCK:
            _AUTO_REVEALED_DEVMODE_UDIDS.add(resolved_udid)
        if dev_enabled:
            msg_id = "Developer Mode sudah aktif di iPhone Anda. Siap langsung melakukan mirroring."
            msg_en = "Developer Mode is already enabled on your iPhone. Ready to start mirroring."
        else:
            msg_id = (
                "Berhasil! Menu Developer Mode telah dimunculkan di iPhone Anda. "
                "Buka Settings -> Privacy & Security -> Developer Mode (paling bawah) -> Nyalakan -> Restart iPhone."
            )
            msg_en = (
                "Success! Developer Mode menu is now visible on your iPhone. "
                "Open Settings -> Privacy & Security -> Developer Mode -> Turn On -> Restart iPhone."
            )
        return {
            "ok": True,
            "connected": True,
            "udid": resolved_udid,
            "revealed": True,
            "developer_mode_status": "enabled" if dev_enabled else "disabled",
            "message_id": msg_id,
            "message_en": msg_en,
        }
    except Exception as exc:
        return {
            "ok": False,
            "connected": True,
            "udid": target_udid,
            "revealed": False,
            "developer_mode_status": "disabled",
            "error": str(exc),
            "message_id": f"Pastikan iPhone dalam keadaan terbuka kuncinya & telah mengetuk Trust This Computer. ({exc})",
            "message_en": f"Make sure your iPhone is unlocked and Trust This Computer was accepted. ({exc})",
        }


def extract_text_batch(frame_data_urls: list[str], save_debug: bool = True) -> list[str]:
    """Extract text from one or more iPhone frames in a single PowerShell process.
    The last frame in frame_data_urls uses Highlight-Crop; earlier frames use Full-Viewport Content Crop.
    Returns a list of extracted strings (one per input frame).
    """
    if not frame_data_urls:
        return []
    created_tmp_files: list[str] = []
    valid_frames = []
    tmp_files: list[str] = []
    try:
        import base64, tempfile, subprocess, os, json
        temp_ocr_dir = get_writable_temp_dir()

        for idx, furl in enumerate(frame_data_urls):
            if not furl:
                continue
            if furl.startswith("data:") and "," in furl:
                header, b64data = furl.split(",", 1)
                raw_bytes = base64.b64decode(b64data)
                if len(raw_bytes) < 100:
                    continue
                suffix = ".png" if "image/png" in header else ".jpg"
                tmp = tempfile.NamedTemporaryFile(prefix="ocr_batch_", suffix=suffix, dir=str(temp_ocr_dir), delete=False)
                tmp.write(raw_bytes)
                tmp.close()
                tmp_files.append(tmp.name)
                created_tmp_files.append(tmp.name)
                valid_frames.append((idx, tmp.name, raw_bytes))
            elif os.path.isfile(furl):
                abs_f = os.path.abspath(furl)
                tmp_files.append(abs_f)
                try:
                    with open(abs_f, "rb") as rf:
                        raw_bytes = rf.read()
                    valid_frames.append((idx, abs_f, raw_bytes))
                except Exception:
                    pass

        if not tmp_files:
            return []

        if save_debug and valid_frames:
            try:
                with open(temp_ocr_dir / "debug_ocr_raw.png", "wb") as f_raw:
                    f_raw.write(valid_frames[-1][2])
            except Exception:
                pass
        valid_frames.clear()

        stdout_raw = _hot_ocr_worker.run_batch(tmp_files, timeout=8.0)
        if stdout_raw is None:
            joined_paths = "|".join(tmp_files)
            script_path = _resolve_ocr_script_path()
            timeout_sec = 18.0

            res = subprocess.run(
                [_resolve_powershell_exe(), "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", script_path, "-ImagePaths", joined_paths],
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                timeout=timeout_sec,
                **_WIN_NO_WINDOW_KWARGS,
            )

            stdout_raw = res.stdout or ""
            _hot_ocr_worker.prewarm_async()

        frame_chunks = stdout_raw.split("---FRAME_SPLIT---")
        results = [chunk.strip() for chunk in frame_chunks if chunk.strip()]
        return results
    except Exception as e:
        print(f"extract_text_batch error: {e}", flush=True)
        return []
    finally:
        for tpath in created_tmp_files:
            try:
                os.remove(tpath)
            except Exception:
                pass
        valid_frames.clear()
        tmp_files.clear()
        if not getattr(sys, "_iphone_link_in_http_scan_handler", False):
            reclaim_python_worker_memory(trim_hot_ocr=True)


def extract_text_from_frame(frame_data_url: str, save_debug: bool = True) -> str | None:
    """Extract highlighted/selected text from iPhone screen frame using Windows 11 WinRT OCR."""
    if not frame_data_url or "," not in frame_data_url:
        return None
    try:
        results = extract_text_batch([frame_data_url], save_debug=save_debug)
        final_text = process_ocr_batch_results(results)

        if save_debug:
            try:
                import json
                temp_ocr_dir = get_writable_temp_dir()
                active_line, buffer_tail = get_debug_typed_state()
                result_info = {
                    "raw_ocr_text": results[-1] if results else "",
                    "batch_count": len(results),
                    "corrected_text": final_text or "",
                    "typed_active_line": active_line,
                    "typed_buffer_tail": buffer_tail,
                }
                with open(temp_ocr_dir / "debug_ocr_result.json", "w", encoding="utf-8") as f_res:
                    json.dump(result_info, f_res, ensure_ascii=False, indent=2)
            except Exception as err:
                print(f"debug_ocr_result.json save error: {err}", flush=True)

        return final_text if final_text else None
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


def _normalize_device_profile(value: Any) -> str:
    val = str(value or "auto").strip().lower()
    if val in ("iphone", "ipad", "auto"):
        return val
    return "auto"


def _normalize_custom_window_size(value: Any) -> dict[str, int] | None:
    if not isinstance(value, dict):
        return None
    try:
        w = int(round(float(value.get("width", 0))))
        h = int(round(float(value.get("height", 0))))
    except Exception:
        return None
    if w < 336 or h < 420:
        return None
    return {
        "width": max(336, min(4096, w)),
        "height": max(420, min(2560, h)),
    }


def load_user_settings(persist_instance: bool = False) -> dict:
    global _IS_FRESH_EXTRACTED_FOLDER
    settings_file = get_settings_file_path()
    app_root_id = get_folder_instance_id()
    merged = dict(DEFAULT_SETTINGS)
    merged["crop"] = dict(DEFAULT_SETTINGS["crop"])
    is_fresh = True

    if settings_file.is_file():
        try:
            data = json.loads(settings_file.read_text(encoding="utf-8"))
            if isinstance(data, dict):
                merged.update(data)
                if isinstance(data.get("crop"), dict):
                    merged["crop"] = {**DEFAULT_SETTINGS["crop"], **data["crop"]}
                if data.get("folderInstanceId") == app_root_id:
                    is_fresh = False
        except Exception as e:
            print(f"load_user_settings error: {e}", flush=True)

    merged["deviceProfile"] = _normalize_device_profile(merged.get("deviceProfile"))
    merged["customWindowSize"] = _normalize_custom_window_size(merged.get("customWindowSize"))
    merged["language"] = "en" if str(merged.get("language", "id")).strip().lower() == "en" else "id"
    _IS_FRESH_EXTRACTED_FOLDER = is_fresh
    merged["folderInstanceId"] = app_root_id
    merged["isFreshFolder"] = is_fresh

    if persist_instance and is_fresh:
        to_save = {k: v for k, v in merged.items() if k != "isFreshFolder"}
        try:
            settings_file.write_text(json.dumps(to_save, indent=2), encoding="utf-8")
            _IS_FRESH_EXTRACTED_FOLDER = False
        except Exception as e:
            print(f"persist folderInstanceId error: {e}", flush=True)

    return merged


def save_user_settings(new_settings: dict) -> dict:
    current = load_user_settings(persist_instance=False)
    current.pop("isFreshFolder", None)
    current["folderInstanceId"] = get_folder_instance_id()
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
        elif k == "audioVolume":
            try:
                current["audioVolume"] = max(0, min(100, int(v if v is not None else 100)))
            except Exception:
                current["audioVolume"] = 100
        elif k == "audioOut":
            from handset.audio import normalize_audio_out
            current["audioOut"] = normalize_audio_out(str(v or "pc"))
        elif k == "audioBufferPreset":
            from handset.usb_audio import normalize_buffer_preset
            current["audioBufferPreset"] = normalize_buffer_preset(str(v or "normal"))
        elif k == "deviceProfile":
            current["deviceProfile"] = _normalize_device_profile(v)
        elif k == "autoCheckUpdate":
            current["autoCheckUpdate"] = bool(v)
        elif k == "updateChannel":
            current["updateChannel"] = "beta" if str(v).strip().lower() == "beta" else "stable"
        elif k == "language":
            current["language"] = "en" if str(v).strip().lower() == "en" else "id"
        elif k == "customWindowSize":
            current["customWindowSize"] = _normalize_custom_window_size(v)
        elif k in ("crossPlatformShortcuts", "pastePcClipboard", "autoRotate", "lipSync", "audioMuted"):
            current[k] = bool(v)
        elif k in current and k not in ("folderInstanceId", "isFreshFolder"):
            current[k] = v
    try:
        get_settings_file_path().write_text(json.dumps(current, indent=2), encoding="utf-8")
    except Exception as e:
        print(f"save_user_settings error: {e}", flush=True)
    return current


_WALLPAPER_SIG_CACHE: dict[str, Any] = {"ts": 0.0, "path": None, "sig": ""}


def _get_current_wallpaper_file_and_signature() -> tuple[Path | None, str]:
    now = time.monotonic()
    if (now - _WALLPAPER_SIG_CACHE["ts"]) < 0.4 and _WALLPAPER_SIG_CACHE["sig"]:
        return _WALLPAPER_SIG_CACHE["path"], _WALLPAPER_SIG_CACHE["sig"]
    """Tahap 95: Ultra-fast (0.03ms) Windows 11 wallpaper inode & registry signature.
    NO full-file MD5 reads of multi-megabyte wallpaper files!
    Inspects:
    1. Win32 API SystemParametersInfoW(SPI_GETDESKWALLPAPER=0x0073) or HKCU\\Control Panel\\Desktop\\WallPaper
    2. %APPDATA%\\Microsoft\\Windows\\Themes\\TranscodedWallpaper
    Computes instant signature from (path, st_mtime_ns, st_size) with short 0.4s coalesce cache.
    """
    candidates: list[Path] = []
    appdata = os.environ.get("APPDATA", "")
    if appdata:
        tw = Path(appdata) / "Microsoft" / "Windows" / "Themes" / "TranscodedWallpaper"
        try:
            if tw.is_file() and tw.stat().st_size > 1000:
                candidates.append(tw)
        except Exception:
            pass

    if sys.platform == "win32":
        spi_path = None
        try:
            import ctypes
            buf = ctypes.create_unicode_buffer(512)
            # SPI_GETDESKWALLPAPER = 0x0073
            if ctypes.windll.user32.SystemParametersInfoW(0x0073, 512, buf, 0):
                p = Path(buf.value)
                if p.is_file() and p.stat().st_size > 1000:
                    spi_path = p
        except Exception:
            pass

        if not spi_path:
            try:
                import winreg
                with winreg.OpenKey(winreg.HKEY_CURRENT_USER, r"Control Panel\Desktop") as key:
                    val, _ = winreg.QueryValueEx(key, "WallPaper")
                    if val:
                        p = Path(val)
                        if p.is_file() and p.stat().st_size > 1000:
                            spi_path = p
            except Exception:
                pass

        if spi_path and spi_path not in candidates:
            candidates.append(spi_path)

    if not candidates:
        _WALLPAPER_SIG_CACHE["ts"] = now
        _WALLPAPER_SIG_CACHE["path"] = None
        _WALLPAPER_SIG_CACHE["sig"] = ""
        return None, ""

    try:
        candidate_stats = []
        for c in candidates:
            try:
                st = c.stat()
                candidate_stats.append((c, st))
            except Exception:
                pass

        if not candidate_stats:
            _WALLPAPER_SIG_CACHE["ts"] = now
            _WALLPAPER_SIG_CACHE["path"] = None
            _WALLPAPER_SIG_CACHE["sig"] = ""
            return None, ""

        best, best_st = max(candidate_stats, key=lambda pair: pair[1].st_mtime_ns)
        import hashlib
        sig_str = f"{best.as_posix()}:{best_st.st_mtime_ns}:{best_st.st_size}"
        sig = hashlib.md5(sig_str.encode("utf-8")).hexdigest()[:16]
        _WALLPAPER_SIG_CACHE["ts"] = now
        _WALLPAPER_SIG_CACHE["path"] = best
        _WALLPAPER_SIG_CACHE["sig"] = sig
        return best, sig
    except Exception:
        _WALLPAPER_SIG_CACHE["ts"] = now
        _WALLPAPER_SIG_CACHE["path"] = None
        _WALLPAPER_SIG_CACHE["sig"] = ""
        return None, ""


class Handler(BaseHTTPRequestHandler):
    def end_headers(self) -> None:
        self.send_header("Cache-Control", "no-store, no-cache, must-revalidate, max-age=0")
        self.send_header("Pragma", "no-cache")
        self.send_header("Expires", "0")
        super().end_headers()

    def _html(self, html_text: str, status: int = 200) -> None:
        data = html_text.encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Access-Control-Allow-Origin", "*")
        self.end_headers()
        self.wfile.write(data)

    @staticmethod
    def _extract_digital_clipboard_url(raw_req_path: str) -> str:
        from urllib.parse import unquote
        parsed = urlparse(raw_req_path)
        if parsed.path == "/c":
            q = parsed.query or ""
            for pair in q.split("&"):
                if "=" in pair:
                    k, v = pair.split("=", 1)
                    if k in ("u", "url"):
                        return unquote(v.strip())
            return unquote(q.strip()) if (q.startswith("http") or q.startswith("https%3A") or q.startswith("http%3A")) else ""
        if parsed.path.startswith("/c/") and len(raw_req_path) > 3:
            raw_tail = raw_req_path[3:].split("#", 1)[0]
            return unquote(raw_tail.strip())
        return ""

    @staticmethod
    def _handle_digital_clipboard_push(raw_url: str, preserve_exact: bool = False) -> tuple[bool, str, bool]:
        global _ACTIVE_CLIPBOARD_SOURCE, _LAST_WIN_CLIPBOARD_SEQ, _LAST_GRABBED_VALID_URL, _LATEST_SHORTCUT_URL_CACHE
        from urllib.parse import unquote
        from handset import qr_bridge as _q

        candidate = str(raw_url or "").strip()
        for _ in range(2):
            if re.match(r'^https?%3A', candidate, re.IGNORECASE):
                candidate = unquote(candidate).strip()
            else:
                break
        if not candidate:
            return False, "", False

        clean_url = candidate
        if not preserve_exact:
            q_cleaned = _q.clean_extracted_url(candidate)
            if q_cleaned and _q.is_strict_valid_grabbed_url(q_cleaned):
                clean_url = q_cleaned
            else:
                # Allow general valid HTTP(S) URLs (e.g. Shopee, YouTube, TikTok) while rejecting broken social stubs
                parsed_c = urlparse(candidate)
                host = (parsed_c.hostname or "").lower()
                is_broken_social = (
                    ("threads.com" in host or "threads.net" in host)
                    or ("instagram.com" in host)
                )
                if (
                    parsed_c.scheme.lower() not in ("http", "https")
                    or not host
                    or "." not in host
                    or any(ch.isspace() for ch in candidate)
                    or len(host.rsplit(".", 1)[-1]) < 2
                    or is_broken_social
                ):
                    return False, "", False
        else:
            parsed_c = urlparse(candidate)
            host = (parsed_c.hostname or "").lower()
            if (
                parsed_c.scheme.lower() not in ("http", "https")
                or not host
                or "." not in host
                or any(ch.isspace() for ch in candidate)
                or len(host.rsplit(".", 1)[-1]) < 2
                or (host.endswith("threads.com") and parsed_c.path in ("/sha", "/share", "/"))
            ):
                return False, "", False

        clip_ok = set_windows_clipboard_text(clean_url)
        if clip_ok:
            _ACTIVE_CLIPBOARD_SOURCE = "iphone"
            _LAST_WIN_CLIPBOARD_SEQ = get_windows_clipboard_seq()
        now_ts = time.time()
        _LAST_GRABBED_VALID_URL = (now_ts, clean_url)
        _LATEST_SHORTCUT_URL_CACHE["url"] = clean_url
        _LATEST_SHORTCUT_URL_CACHE["ts"] = now_ts
        return True, clean_url, clip_ok

    def do_GET(self) -> None:
        path = urlparse(self.path).path
        if path == "/c" or path.startswith("/c/"):
            raw_u = self._extract_digital_clipboard_url(self.path)
            ok, clean_url, clip_ok = self._handle_digital_clipboard_push(raw_u)
            if ok:
                body = (
                    "<!doctype html><html><head><meta charset='utf-8'><title>iPhone Link</title></head>"
                    "<body style='font-family:-apple-system,sans-serif;text-align:center;padding:24px;'>"
                    "<script>window.close();</script>&#x2713; Link Copied to PC!"
                    "<p style='font-size:14px;margin-top:8px;'>\u2713 Link Copied to PC!</p>"
                    f"<!-- {clean_url} clip={int(clip_ok)} -->"
                    "</body></html>"
                )
                # Ensure exact ASCII/UTF-8 marker requested by Tahap 202
                body = body.replace("&#x2713; Link Copied to PC!", "\u2713 Link Copied to PC!")
                self._html(body, status=200)
            else:
                self._html(
                    "<!doctype html><html><head><meta charset='utf-8'></head>"
                    "<body>Invalid or missing social URL.</body></html>",
                    status=400,
                )
            return
        if path == MIRROR_PREFIX or path.startswith(f"{MIRROR_PREFIX}/"):
            self._proxy_mirror("GET")
            return
        if path == "/api/system/wallpaper":
            self._system_wallpaper()
            return
        if path == "/api/system/wallpaper-sig":
            _, wallpaper_sig = _get_current_wallpaper_file_and_signature()
            self._json({"ok": True, "wallpaperSig": wallpaper_sig})
            return
        if path == "/api/shortcut/setup-info":
            self._json(get_shortcut_setup_info(port=8765))
            return
        if path == "/api/ocr/status":
            self._json(get_hot_ocr_status())
            return
        if path == "/api/driver/diagnostics":
            query = parse_qs(urlparse(self.path).query)
            target_udid = (query.get("udid") or [None])[0]
            force_ref = str((query.get("refresh") or ["0"])[0]).lower() in ("1", "true", "yes")
            self._json({"ok": True, "diagnostics": _get_detailed_driver_and_device_diagnostics(udid=target_udid, force_refresh=force_ref)})
            return
        if path == "/api/system/memory":
            self._json(get_system_memory_telemetry(reclaim=False))
            return
        if path == "/api/status":
            payload = snapshot()
            payload["screen"] = screen_status()
            devices = payload.get("devices") or []
            usb_devices = [d for d in devices if str(d.get("connection", "")).upper() == "USB" and d.get("udid")]
            has_usb = len(usb_devices) > 0
            selected_udid = None
            if has_usb:
                selected_udid = usb_devices[0]["udid"]
                ensure_wifi_enabled_on_usb(selected_udid)
                payload["connected"] = True
                payload["udid"] = selected_udid
                payload["connectionType"] = "USB"
            else:
                paired_list = wifi_paired_udids()
                if paired_list:
                    selected_udid = paired_list[0]
                payload["connected"] = False
                payload["udid"] = selected_udid
                payload["connectionType"] = "Network" if selected_udid else None
            diag = _get_detailed_driver_and_device_diagnostics(udid=selected_udid, has_usb=has_usb)
            payload["detailedDiagnostics"] = diag
            if has_usb and payload.get("developerMode") is None:
                payload["developerMode"] = diag.get("developer_mode_bool")
            # Zero network / ARP / TCP scan on routine /api/status polls!
            payload["wifi"] = get_wifi_status(selected_udid, probe_network=False)
            payload["audio"] = audio_manager.status()
            payload["folderInstanceId"] = get_folder_instance_id()
            payload["isFreshFolder"] = is_fresh_extracted_folder()
            _, wallpaper_sig = _get_current_wallpaper_file_and_signature()
            payload["wallpaperSig"] = wallpaper_sig
            payload["deviceFamily"] = next(
                (d.get("deviceFamily") for d in devices if isinstance(d, dict) and d.get("deviceFamily")),
                "iphone",
            )
            payload["deviceProfile"] = _normalize_device_profile(
                load_user_settings(persist_instance=False).get("deviceProfile")
            )
            ocr_st = get_hot_ocr_status()
            payload["hot_ocr_engine_ready"] = bool(ocr_st.get("hot_ocr_engine_ready", False))
            payload["hot_ocr_worker"] = ocr_st
            self._json(payload)
            return
        if path == "/api/tray-state":
            from handset.screen import _STEALTH_DIM_ACTIVE
            self._json({
                "ok": True,
                "stealth": bool(_STEALTH_DIM_ACTIVE),
                "audio": bool(audio_manager.is_active()),
            })
            return
        if path.startswith("/api/wifi/probe"):
            query = parse_qs(urlparse(self.path).query)
            target_udid = (query.get("udid") or [None])[0]
            self._json({"ok": True, "wifi": get_wifi_status(target_udid, probe_network=True)})
            return
        if path == "/api/update/status":
            from handset.updater import get_update_status
            self._json(get_update_status())
            return
        if path in ("/api/settings", "/api/keyboard/settings"):
            self._json(load_user_settings(persist_instance=True))
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
        if path == "/api/device/stealth":
            from handset.screen import _STEALTH_DIM_ACTIVE
            self._json({"ok": True, "stealth": _STEALTH_DIM_ACTIVE})
            return
        if path in ("/api/ocr/clear-chunks", "/api/keyboard/clear-chunks"):
            clear_recent_long_ocr_chunks()
            self._json({"ok": True, "action": "clear_chunks"})
            reclaim_python_worker_memory(trim_hot_ocr=True)
            return
        if path == "/debug-metrics":
            log_path = get_app_root() / "debug_metrics.json"
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
        global _ACTIVE_CLIPBOARD_SOURCE, _LAST_WIN_CLIPBOARD_SEQ, _LAST_GRABBED_VALID_URL
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

        if path in ("/api/clipboard/push", "/api/shortcut/push"):
            raw_u = payload.get("url") or payload.get("u") or payload.get("text") or ""
            if not raw_u:
                raw_txt = raw.decode("utf-8", errors="ignore").strip()
                if raw_txt.startswith("http"):
                    raw_u = raw_txt
            del raw
            ok, clean_url, clip_ok = self._handle_digital_clipboard_push(
                str(raw_u),
                preserve_exact=(path == "/api/shortcut/push"),
            )
            if ok:
                self._json({
                    "ok": True,
                    "source": "ios_shortcut_digital",
                    "url": clean_url,
                    "clipboardWritten": clip_ok,
                    "copied_to_clipboard": clip_ok,
                }, status=200)
            else:
                self._json({"ok": False, "error": "Invalid or missing URL", "url": ""}, status=400)
            return

        del raw

        if path == "/api/system/memory":
            self._json(get_system_memory_telemetry(reclaim=False))
            return
        if path == "/api/system/reclaim-memory":
            self._json(get_system_memory_telemetry(reclaim=True))
            return

        if path == "/api/update/check":
            from handset.updater import check_for_updates
            ch = payload.get("channel") or load_user_settings(persist_instance=False).get("updateChannel", "stable")
            self._json(check_for_updates(channel=str(ch)))
            return
        if path == "/api/update/install":
            from handset.updater import start_download_and_apply_update
            dl_url = str(payload.get("downloadUrl") or "")
            target_v = str(payload.get("targetVersion") or "")
            self._json(start_download_and_apply_update(download_url=dl_url, target_version=target_v))
            return
        if path == "/api/settings":
            saved = save_user_settings(payload)
            self._json({"ok": True, "settings": saved})
            return
        if path == "/api/settings/device-profile":
            raw_prof = payload.get("deviceProfile") if "deviceProfile" in payload else payload.get("profile")
            prof = _normalize_device_profile(raw_prof)
            saved = save_user_settings({"deviceProfile": prof})
            self._json({"ok": True, "deviceProfile": prof, "settings": saved})
            return
        if path == "/api/server/shutdown":
            import threading
            try:
                from handset import screen as _scr
                if getattr(_scr, "_STEALTH_DIM_ACTIVE", False):
                    _scr.set_iphone_physical_brightness("restore", steps=4)
                _scr.set_iphone_physical_speaker_mute(False)
                _scr.stop_screen()
            except Exception:
                pass
            try:
                audio_manager.shutdown()
            except Exception:
                pass
            try:
                _hot_ocr_worker.shutdown()
            except Exception:
                pass
            self._json({"ok": True, "shuttingDown": True})
            threading.Thread(target=self.server.shutdown, daemon=True).start()
            return
        if path in ("/api/audio", "/api/audio/config"):
            st = audio_manager.configure(
                audio_out=payload.get("audioOut") if "audioOut" in payload else payload.get("mode"),
                volume=payload.get("volume") if "volume" in payload else payload.get("audioVolume"),
                muted=payload.get("muted") if "muted" in payload else payload.get("audioMuted"),
                buffer_preset=payload.get("bufferPreset") if "bufferPreset" in payload else payload.get("audioBufferPreset"),
                lip_sync=payload.get("lipSync") if "lipSync" in payload else None,
                force_stream=payload.get("forceStream") if "forceStream" in payload else None,
            )
            save_user_settings({
                "audioOut": st.get("audioOut", "pc"),
                "audioSinkEnabled": st.get("audioOut") == "bluetooth",
                "audioVolume": st.get("volume", 100),
                "audioMuted": st.get("muted", False),
                "audioBufferPreset": st.get("bufferPreset", "normal"),
                "lipSync": st.get("lipSync", True),
            })
            self._json(st)
            return
        if path == "/api/audio/mute":
            if "muted" in payload and payload.get("muted") is not None:
                st = audio_manager.set_muted(bool(payload.get("muted")))
            else:
                st = audio_manager.toggle_mute()
            save_user_settings({"audioMuted": st.get("muted", False)})
            self._json(st)
            return
        if path == "/api/audio/toggle":
            enabled = payload.get("enabled") if "enabled" in payload else None
            st = audio_manager.toggle(enabled)
            save_user_settings({
                "audioOut": st.get("audioOut", "pc"),
                "audioSinkEnabled": st.get("audioOut") == "bluetooth",
            })
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
        if path in ("/api/qr/scan", "/api/social/grab-link"):
            frame_data_url = payload.get("frameDataUrl")
            tail_frame_data_url = payload.get("tailFrameDataUrl")
            pass_mode = payload.get("passMode", "")
            is_retry = bool(payload.get("isRetry", False))
            frames = payload.get("frames")
            if isinstance(frames, list):
                if not frame_data_url and len(frames) >= 1:
                    frame_data_url = frames[0]
                if not tail_frame_data_url and len(frames) >= 2:
                    tail_frame_data_url = frames[1]
            frame_history = payload.get("frameHistory") or []
            payload.clear()
            qr_text = None
            source_tag = "qr_bridge"

            # Tahap 111..194: Completeness verification for Pass 1 clean paste
            def _is_url_complete(u: Optional[str]) -> bool:
                if not u:
                    return False
                m_ig_seg = re.search(r'instagram\.com/(?:p|reel|reels|tv)/([^/?#\s]+)', u, re.IGNORECASE)
                if m_ig_seg:
                    code = m_ig_seg.group(1)
                    if not re.match(r'^[A-Za-z0-9_\-]{10,15}$', code):
                        return False
                    return True
                if re.search(r'threads\.(?:com|net)/@[A-Za-z0-9_.\-]+/post/[A-Za-z0-9_-]{11,13}(?:[/?#]|$)', u, re.IGNORECASE):
                    return True
                if re.search(r'threads\.(?:com|net)/share/(?!share\b|search\b|threads\b|thread\b|websites\b|situsweb\b|openlink\b|instagram\b)[A-Za-z0-9_-]{8,15}(?:[/?#]|$)', u, re.IGNORECASE):
                    return True
                if any(k in u.lower() for k in ("tiktok.com", "youtu.be", "youtube.com", "x.com")):
                    return True
                return False

            with _RECLAIM_MEMORY_LOCK:
                try:
                    sys._iphone_link_in_http_scan_handler = True
                    # Tahap 98..111: Auto-Spotlight Multi-line URL Assembler & Grabber mode
                    if tail_frame_data_url and frame_data_url:
                        try:
                            from handset.qr_bridge import (
                                decode_qr_from_image,
                                extract_visible_url_from_frame,
                                extract_url_or_fragment_from_frame,
                                stitch_url_head_and_tail,
                                pick_best_extracted_url,
                            )
                            head_qr = decode_qr_from_image(frame_data_url) or decode_qr_from_image(tail_frame_data_url)
                            if head_qr:
                                qr_text = head_qr
                                source_tag = "qr_bridge"
                            else:
                                qr_text = extract_visible_url_from_frame(
                                    frame_data_url,
                                    tail_frame_data_url=tail_frame_data_url,
                                    frames=frames if isinstance(frames, list) else None,
                                )
                                if not qr_text:
                                    head_part = extract_url_or_fragment_from_frame(frame_data_url)
                                    tail_part = extract_url_or_fragment_from_frame(tail_frame_data_url)
                                    qr_text = stitch_url_head_and_tail(head_part, tail_part)
                                if qr_text:
                                    source_tag = "spotlight_grabber"
                        except Exception as stitch_err:
                            print(f"[/api/qr/scan] Spotlight Head+Tail error: {stitch_err}", flush=True)

                    elif frame_data_url:
                        try:
                            from handset.qr_bridge import (
                                decode_qr_from_image,
                                extract_visible_url_from_frame,
                                extract_url_or_fragment_from_frame,
                            )
                            # Priority 1: ISO/IEC 18004 QR Matrix (< 15ms, 100% precision)
                            qr_text = decode_qr_from_image(frame_data_url)
                            if not qr_text and frame_history:
                                for h_url in reversed(frame_history):
                                    if h_url:
                                        qr_text = decode_qr_from_image(h_url)
                                        if qr_text:
                                            break
                            # Priority 2: Direct Unhighlighted URL Scanner
                            if not qr_text:
                                qr_text = extract_url_or_fragment_from_frame(frame_data_url)
                                if qr_text:
                                    source_tag = "url_scanner"
                        except Exception as qr_err:
                            print(f"[/api/qr/scan] Error: {qr_err}", flush=True)

                    is_complete = _is_url_complete(qr_text)

                    if pass_mode == "clean_paste_only" and not is_complete:
                        # Pass 1 did not produce a complete link yet; request Pass 2 cursor shift
                        self._json({
                            "ok": False,
                            "action": "scan-qr",
                            "extractedText": qr_text,
                            "complete": False,
                            "error": "Pass 1 incomplete, proceeding to Pass 2 cursor shift",
                            "source": source_tag,
                            "qrDetected": bool(qr_text),
                        })
                        return

                    from handset.qr_bridge import is_strict_valid_grabbed_url
                    if qr_text and not is_strict_valid_grabbed_url(qr_text):
                        print(f"[{source_tag}] Rejected non-HTTP(S)/invalid fragment from Grab Link: {qr_text!r}", flush=True)
                        qr_text = None
                    if qr_text:
                        clip_ok = set_windows_clipboard_text(qr_text)
                        if clip_ok:
                            _LAST_WIN_CLIPBOARD_SEQ = get_windows_clipboard_seq()
                            _ACTIVE_CLIPBOARD_SOURCE = "pc"
                            print(f"[{source_tag}] Scanned link to PC clipboard ({len(qr_text)} chars): {qr_text}", flush=True)
                        _LAST_GRABBED_VALID_URL = (time.time(), qr_text)
                        self._json({
                            "ok": True,
                            "action": "scan-qr",
                            "text": qr_text,
                            "extractedText": qr_text,
                            "url": qr_text,
                            "clipboardWritten": bool(clip_ok),
                            "complete": True,
                            "source": source_tag,
                            "qrDetected": True,
                            "length": len(qr_text),
                        })
                        return
                    if (
                        not qr_text
                        and _LATEST_SHORTCUT_URL_CACHE.get("url")
                        and (time.time() - float(_LATEST_SHORTCUT_URL_CACHE.get("ts") or 0.0)) < 12.0
                    ):
                        cached_sc_url = str(_LATEST_SHORTCUT_URL_CACHE["url"])
                        clip_ok = set_windows_clipboard_text(cached_sc_url)
                        if clip_ok:
                            _LAST_WIN_CLIPBOARD_SEQ = get_windows_clipboard_seq()
                            _ACTIVE_CLIPBOARD_SOURCE = "iphone"
                        self._json({
                            "ok": True,
                            "action": "scan-qr",
                            "text": cached_sc_url,
                            "extractedText": cached_sc_url,
                            "url": cached_sc_url,
                            "clipboardWritten": bool(clip_ok),
                            "complete": True,
                            "source": "ios_shortcut_digital",
                            "qrDetected": True,
                            "length": len(cached_sc_url),
                        })
                        return
                    from handset.qr_bridge import was_last_spotlight_empty_or_placeholder
                    spotlight_empty = bool(was_last_spotlight_empty_or_placeholder())
                    self._json({
                        "ok": False,
                        "action": "scan-qr",
                        "extractedText": None,
                        "complete": False,
                        "error": "QR Code atau link belum tampil di layar iPhone",
                        "source": "qr_bridge",
                        "qrDetected": False,
                        "spotlightEmpty": spotlight_empty,
                        "retrySuggested": bool(not is_retry),
                    })
                    return
                finally:
                    sys._iphone_link_in_http_scan_handler = False
                    frame_data_url = None
                    tail_frame_data_url = None
                    frames = None
                    frame_history = []
                    reclaim_python_worker_memory(trim_hot_ocr=True)
        if path == "/api/keyboard/shortcut":
            commit_and_reset_current_typed_line()
            action = str(payload.get("action", ""))
            if action == "clear-copy-cache":
                clear_recent_long_ocr_chunks()
                self._json({"ok": True, "action": "clear-copy-cache"})
                reclaim_python_worker_memory(trim_hot_ocr=True)
                return

            frame_data_url = payload.get("frameDataUrl")
            frame_history = payload.get("frameHistory") or []
            wait_result = bool(payload.get("waitResult", False))
            skip_ocr = bool(payload.get("skipOcr", False))

            if action in ("scan-qr", "grab-link"):
                qr_text = None
                source_tag = "qr_bridge"
                tail_frame_data_url = payload.get("tailFrameDataUrl")
                payload.clear()
                with _RECLAIM_MEMORY_LOCK:
                    try:
                        sys._iphone_link_in_http_scan_handler = True
                        if tail_frame_data_url and frame_data_url:
                            try:
                                from handset.qr_bridge import (
                                    decode_qr_from_image,
                                    extract_visible_url_from_frame,
                                    extract_url_or_fragment_from_frame,
                                    stitch_url_head_and_tail,
                                    pick_best_extracted_url,
                                )
                                head_qr = decode_qr_from_image(frame_data_url) or decode_qr_from_image(tail_frame_data_url)
                                if head_qr:
                                    qr_text = head_qr
                                    source_tag = "qr_bridge"
                                else:
                                    head_part = extract_visible_url_from_frame(frame_data_url) or extract_url_or_fragment_from_frame(frame_data_url)
                                    tail_part = extract_visible_url_from_frame(tail_frame_data_url) or extract_url_or_fragment_from_frame(tail_frame_data_url)
                                    final_url = stitch_url_head_and_tail(head_part, tail_part)
                                    if final_url:
                                        qr_text = final_url
                                        source_tag = "spotlight_grabber"
                            except Exception as stitch_err:
                                print(f"[Visual QR Bridge] Shortcut Spotlight Head+Tail error: {stitch_err}", flush=True)
                        elif frame_data_url:
                            try:
                                from handset.qr_bridge import (
                                    decode_qr_from_image,
                                    extract_visible_url_from_frame,
                                    extract_url_or_fragment_from_frame,
                                )
                                # Priority 1: ISO/IEC 18004 QR Matrix (< 15ms, 100% precision)
                                qr_text = decode_qr_from_image(frame_data_url)
                                if not qr_text and frame_history:
                                    for h_url in reversed(frame_history):
                                        if h_url:
                                            qr_text = decode_qr_from_image(h_url)
                                            if qr_text:
                                                break
                                # Priority 2: Tahap 97 Smart Fallback - Direct Unhighlighted URL Scanner
                                if not qr_text:
                                    qr_text = extract_url_or_fragment_from_frame(frame_data_url)
                                    if qr_text:
                                        source_tag = "url_scanner"
                            except Exception as qr_err:
                                print(f"[Visual QR Bridge] Shortcut scan error: {qr_err}", flush=True)

                        from handset.qr_bridge import is_strict_valid_grabbed_url
                        if qr_text and not is_strict_valid_grabbed_url(qr_text):
                            print(f"[shortcut] Rejected non-HTTP(S)/invalid fragment from Grab Link: {qr_text!r}", flush=True)
                            qr_text = None
                        if qr_text:
                            clip_ok = set_windows_clipboard_text(qr_text)
                            if clip_ok:
                                _LAST_WIN_CLIPBOARD_SEQ = get_windows_clipboard_seq()
                                _ACTIVE_CLIPBOARD_SOURCE = "pc"
                                print(f"[{source_tag}] Scanned link to PC clipboard ({len(qr_text)} chars): {qr_text}", flush=True)
                            _LAST_GRABBED_VALID_URL = (time.time(), qr_text)
                            self._json({
                                "ok": True,
                                "action": "scan-qr",
                                "text": qr_text,
                                "extractedText": qr_text,
                                "url": qr_text,
                                "clipboardWritten": bool(clip_ok),
                                "source": source_tag,
                                "qrDetected": True,
                                "length": len(qr_text),
                            })
                            return
                        self._json({
                            "ok": False,
                            "action": "scan-qr",
                            "extractedText": None,
                            "error": "QR Code atau link belum tampil di layar iPhone",
                            "source": "qr_bridge",
                            "qrDetected": False,
                        })
                        return
                    finally:
                        sys._iphone_link_in_http_scan_handler = False
                        frame_data_url = None
                        tail_frame_data_url = None
                        frame_history = []
                        reclaim_python_worker_memory(trim_hot_ocr=True)

            # Trigger native iOS hotkey
            payload.clear()
            ok = trigger_ios_shortcut(action)

            if action in ("copy", "cut"):
                _ACTIVE_CLIPBOARD_SOURCE = "iphone"
                _LAST_WIN_CLIPBOARD_SEQ = get_windows_clipboard_seq()

                if skip_ocr or not frame_data_url:
                    frame_data_url = None
                    frame_history = []
                    self._json({"ok": ok, "action": action, "internal": True})
                    reclaim_python_worker_memory(trim_hot_ocr=True)
                    return

                # Tahap 213: Execute fast Hot-OCR _do_extract directly (< 130ms for short snippets / early-exit)
                # without blocking Alt+C on multi-frame QR scanning first. If text extraction yields empty,
                # _do_extract transparently checks Visual QR Bridge + Hot-OCR URL scanner as fallback.
                extracted_text = None
                auto_cleared_cache = False
                fallback_source = None

                def _do_extract(frame_url: str, hist_urls: list[str]):
                    try:
                        recent_hist = [h for h in hist_urls if h and h != frame_url][:3] if hist_urls else []
                        frames_to_batch = recent_hist + [frame_url]

                        # Single-Pass Batch OCR via persistent Hot WinRT OCR engine (< 130ms)
                        batch_texts = extract_text_batch(frames_to_batch, save_debug=True)

                        # Centralized OCR post-processing pipeline (Tahap 39/40)
                        final_text, auto_cleared = process_ocr_batch_results(batch_texts, return_meta=True)
                        src_tag = None

                        # Save debug info strictly inside writable %TEMP%\iPhoneLink
                        try:
                            import json
                            temp_ocr_dir = get_writable_temp_dir()
                            active_line, buffer_tail = get_debug_typed_state()
                            result_info = {
                                "raw_ocr_text": batch_texts[-1] if batch_texts else "",
                                "batch_count": len(batch_texts),
                                "corrected_text": final_text or "",
                                "auto_cleared_cache": auto_cleared,
                                "typed_active_line": active_line,
                                "typed_buffer_tail": buffer_tail,
                            }
                            with open(temp_ocr_dir / "debug_ocr_result.json", "w", encoding="utf-8") as f_res:
                                json.dump(result_info, f_res, ensure_ascii=False, indent=2)
                        except Exception as err:
                            print(f"debug_ocr_result.json save error: {err}", flush=True)

                        if not final_text and frame_url:
                            try:
                                from handset.qr_bridge import decode_qr_from_image, extract_visible_url_from_frame
                                qr_fb = decode_qr_from_image(frame_url)
                                if not qr_fb and hist_urls:
                                    for h_url in reversed(hist_urls):
                                        if h_url:
                                            qr_fb = decode_qr_from_image(h_url)
                                            if qr_fb:
                                                break
                                if qr_fb:
                                    final_text = qr_fb
                                    src_tag = "qr_bridge"
                                else:
                                    fallback_url = extract_visible_url_from_frame(frame_url)
                                    if fallback_url:
                                        final_text = fallback_url
                                        src_tag = "url_scanner"
                            except Exception:
                                pass

                        if final_text:
                            if set_windows_clipboard_text(final_text):
                                _LAST_WIN_CLIPBOARD_SEQ = get_windows_clipboard_seq()
                                _ACTIVE_CLIPBOARD_SOURCE = "iphone"
                                print(f"Copied iPhone selection to Windows PC clipboard ({len(final_text)} chars, {len(final_text.splitlines())} lines)", flush=True)
                        return final_text, auto_cleared, src_tag
                    except Exception as err:
                        import traceback
                        err_trace = traceback.format_exc()
                        print(f"Async extract error: {err}\n{err_trace}", flush=True)
                        try:
                            temp_ocr_dir = get_writable_temp_dir()
                            with open(temp_ocr_dir / "debug_ocr_error.log", "w", encoding="utf-8") as f_err:
                                f_err.write(f"[{time.strftime('%Y-%m-%d %H:%M:%S')}] OCR Extract Error: {err}\n{err_trace}\n")
                        except Exception:
                            pass
                        return None, False, None
                    finally:
                        if not getattr(sys, "_iphone_link_in_http_scan_handler", False):
                            reclaim_python_worker_memory(trim_hot_ocr=True)

                with _RECLAIM_MEMORY_LOCK:
                    try:
                        if frame_data_url:
                            if wait_result:
                                sys._iphone_link_in_http_scan_handler = True
                                res = _do_extract(frame_data_url, frame_history)
                                frame_data_url = None
                                frame_history = []
                                if isinstance(res, tuple):
                                    extracted_text, auto_cleared_cache = res[0], res[1]
                                    fallback_source = res[2] if len(res) > 2 else None
                                else:
                                    extracted_text = res
                            else:
                                import threading
                                threading.Thread(target=_do_extract, args=(frame_data_url, frame_history), daemon=True).start()
                                frame_data_url = None
                                frame_history = []

                        resp_payload = {
                            "ok": ok,
                            "action": action,
                            "text": extracted_text,
                            "autoClearedCache": auto_cleared_cache,
                            "length": len(extracted_text) if extracted_text else 0,
                        }
                        if fallback_source:
                            resp_payload["source"] = fallback_source
                            if fallback_source == "qr_bridge":
                                resp_payload["qrDetected"] = True
                        self._json(resp_payload)
                        return
                    finally:
                        if wait_result:
                            sys._iphone_link_in_http_scan_handler = False
                            reclaim_python_worker_memory(trim_hot_ocr=True)

            self._json({"ok": ok, "action": action})
            return
        if path == "/api/keyboard/paste":
            commit_and_reset_current_typed_line()
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
        if path == "/api/keyboard/typed":
            text_val = payload.get("text")
            key_val = payload.get("key")
            if payload.get("commitLine"):
                commit_and_reset_current_typed_line()
            elif text_val:
                record_typed_text(str(text_val))
            elif key_val:
                record_typed_char(str(key_val))
            self._json({"ok": True})
            return
        if path in ("/api/ocr/clear-chunks", "/api/keyboard/clear-chunks"):
            clear_recent_long_ocr_chunks()
            self._json({"ok": True, "action": "clear_chunks"})
            reclaim_python_worker_memory(trim_hot_ocr=True)
            return

        if path == "/api/device/wake-unlock":
            passcode = str(payload.get("passcode") or "")
            self._json(wake_and_unlock_iphone(passcode))
            return
        if path == "/api/device/lock":
            self._json(lock_iphone_screen())
            return
        if path == "/api/device/stealth":
            enabled = bool(payload.get("enabled", True))
            restore_steps = int(payload.get("restoreSteps") or 4)
            level = "min" if enabled else "restore"
            steps = 16 if enabled else restore_steps
            self._json(set_iphone_physical_brightness(level, steps=steps))
            return
        if path == "/api/device/wipe-privacy-data":
            self._json(wipe_all_privacy_data())
            return
        if path == "/api/driver/repair":
            self._json(_repair_apple_driver_and_service())
            return
        if path == "/api/device/unlock-developer-mode":
            req_udid = payload.get("udid") if isinstance(payload.get("udid"), str) else None
            arm_enable = bool(payload.get("arm_enable", False))
            self._json(_unlock_developer_mode_on_device(udid=req_udid, arm_enable=arm_enable))
            return
        if path in ("/api/device/spotlight-paste", "/api/device/spotlight-paste-tail"):
            from handset.screen import spotlight_paste_clipboard_on_iphone
            res = spotlight_paste_clipboard_on_iphone()
            self._json(res)
            return
        if path == "/api/device/spotlight-move-head":
            from handset.screen import spotlight_move_cursor_to_start_on_iphone
            res = spotlight_move_cursor_to_start_on_iphone()
            self._json(res)
            return
        if path == "/api/device/spotlight-close":
            from handset.screen import spotlight_cleanup_and_close_on_iphone
            res = spotlight_cleanup_and_close_on_iphone()
            self._json(res)
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
                snap = snapshot()
                for d in snap.get("devices", []):
                    if str(d.get("connection", "")).upper() == "USB" and d.get("udid"):
                        udid = d["udid"]
                        break
                if not udid and snap.get("devices"):
                    udid = snap["devices"][0].get("udid")
            if not udid:
                self._json({"ok": False, "error": "Plug in the iPhone first."}, status=400)
                return
            self._json(pair_for_wifi(udid))
            return
        if path == "/api/wifi/scan":
            from handset.wifi import discover_iphone_ip_on_lan
            found_ip = discover_iphone_ip_on_lan(udid)
            self._json({"ok": True, "ip": found_ip, "wifi": get_wifi_status(udid, probe_network=True)})
            return
        if path != "/api/screen":
            self._json({"error": "Unknown action."}, status=404)
            return
        transport = payload.get("transport")
        if transport not in {"usb", "wifi", "cable"}:
            transport = "cable"
        self._json(start_screen(udid, transport))

    def log_message(self, fmt: str, *args) -> None:
        return

    def _record_debug_metrics(self, payload: dict) -> None:
        payload["received_at"] = time.strftime("%Y-%m-%d %H:%M:%S")
        source = payload.get("source", "unknown")
        print(f"\n==================== [DEBUG-METRICS: {source}] ====================", flush=True)
        print(json.dumps(payload, indent=2), flush=True)
        print("==================================================================\n", flush=True)
        
        log_dir = get_app_root() if _is_writable_app_dir(get_app_root()) else get_writable_temp_dir()
        log_path = log_dir / "debug_metrics.json"
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
        except Exception:
            try:
                (get_writable_temp_dir() / "debug_metrics.json").write_text(json.dumps(data, indent=2), encoding="utf-8")
            except Exception as e:
                print(f"Failed to write debug_metrics.json: {e}", flush=True)

    def _device(self, udid: str) -> None:
        try:
            self._json(read_handset(udid))
        except MuxError as error:
            from handset.wifi import get_device_wifi_state, wifi_paired_udids
            cached = get_device_wifi_state(udid)
            if cached.get("wifi_enabled_on_device") or udid in wifi_paired_udids():
                name = cached.get("device_name") or "iPhone"
                product_type = cached.get("product_type") or ""
                family = classify_device_family(product_type=product_type, device_name=name)
                info_obj = {
                    "DeviceName": name,
                    "DeviceClass": "iPad" if family == "ipad" else "iPhone",
                    "WiFiAddress": cached.get("mac_address"),
                    "LastIP": cached.get("last_ip"),
                }
                if product_type:
                    info_obj["ProductType"] = product_type
                self._json({
                    "udid": udid,
                    "paired": True,
                    "deviceFamily": family,
                    "info": info_obj,
                    "wifi": True,
                })
                return
            self._json({"udid": udid, "paired": False, "error": str(error)}, status=409)

    def _json(self, payload: dict, status: int = 200) -> None:
        body = json.dumps(payload).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        try:
            self.wfile.write(body)
        except _CLIENT_GONE:
            pass

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
        try:
            if hasattr(self, "connection") and self.connection:
                self.connection.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        except Exception:
            pass
        conn = http.client.HTTPConnection(STREAM_HOST, STREAM_PORT, timeout=600)
        started = False
        self.close_connection = True
        try:
            conn.request(method, target, headers={"Connection": "close"})
            try:
                if conn.sock:
                    conn.sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
            except Exception:
                pass
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

        # Reset active typing line on user screen interactions (touches/taps/drags)
        if upstream_path == "/touch" and method in ("POST", "PUT"):
            commit_and_reset_current_typed_line()

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

    def _system_wallpaper(self) -> None:
        best_path, _ = _get_current_wallpaper_file_and_signature()
        wallpaper_bytes = None
        content_type = "image/jpeg"
        if best_path and best_path.is_file():
            try:
                wallpaper_bytes = best_path.read_bytes()
            except Exception as e:
                print(f"Error reading wallpaper: {e}", flush=True)

        if not wallpaper_bytes:
            # Fallback 1x1 neutral dark grey JPEG
            wallpaper_bytes = b"\xff\xd8\xff\xe0\x00\x10JFIF\x00\x01\x01\x01\x00`\x00`\x00\x00\xff\xdb\x00C\x00\x08\x06\x06\x07\x06\x05\x08\x07\x07\x07\t\t\x08\n\x0c\x14\r\x0c\x0b\x0b\x0c\x19\x12\x13\x0f\x14\x1d\x1a\x1f\x1e\x1d\x1a\x1c\x1c $.' \",#\x1c\x1c(7),01444\x1f'9=82<.342\xff\xc0\x00\x0b\x08\x00\x01\x00\x01\x01\x01\x11\x00\xff\xc4\x00\x1f\x00\x00\x01\x05\x01\x01\x01\x01\x01\x01\x00\x00\x00\x00\x00\x00\x00\x00\x01\x02\x03\x04\x05\x06\x07\x08\t\n\x0b\xff\xda\x00\x08\x01\x01\x00\x00?\x00\xbf\x00\xff\xd9"

        if wallpaper_bytes.startswith(b"\x89PNG"):
            content_type = "image/png"
        else:
            content_type = "image/jpeg"

        self.send_response(200)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(wallpaper_bytes)))
        self.send_header("Cache-Control", "no-store, no-cache, must-revalidate, max-age=0")
        self.send_header("Pragma", "no-cache")
        self.end_headers()
        try:
            self.wfile.write(wallpaper_bytes)
        except _CLIENT_GONE:
            pass

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
            ".png": "image/png",
            ".ico": "image/x-icon",
        }.get(target.suffix, "application/octet-stream")
        body = target.read_bytes()
        self.send_response(200)
        self.send_header("Content-Type", kind)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        try:
            self.wfile.write(body)
        except _CLIENT_GONE:
            pass


class SingleServer(ThreadingHTTPServer):
    # On Windows, address reuse lets a second copy bind the same port and steal requests.
    allow_reuse_address = False

    def get_request(self):
        sock, addr = super().get_request()
        try:
            sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        except Exception:
            pass
        return sock, addr


def create_http_server(host: str = HOST, port: int = PORT) -> SingleServer:
    """Create and configure the local ThreadingHTTPServer without blocking."""
    server = SingleServer((host, port), Handler)
    settings = load_user_settings()
    audio_manager.apply_saved_settings(settings)
    if settings.get("audioOut") == "bluetooth":
        import threading

        def _restore_audio_async() -> None:
            try:
                audio_manager._enable_bt()
            except Exception:
                pass

        threading.Thread(target=_restore_audio_async, name="handset-audio-restore", daemon=True).start()
    _hot_ocr_worker.prewarm_async()
    return server


def serve(host: str = HOST, port: int = PORT) -> None:
    """Run the local backend HTTP server forever (blocking)."""
    server = create_http_server(host=host, port=port)
    try:
        if sys.stdout is not None:
            print(f"Handset listening on http://{host}:{port}", flush=True)
    except Exception:
        pass
    try:
        server.serve_forever()
    finally:
        audio_manager.shutdown()
        _hot_ocr_worker.shutdown()


def main() -> None:
    host = os.environ.get("PANE_HOST", HOST)
    port = int(os.environ.get("PANE_PORT", PORT))
    argv = sys.argv[1:]
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
    serve(host=host, port=port)


if __name__ == "__main__":
    main()

