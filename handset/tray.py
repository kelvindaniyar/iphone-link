"""Native Windows 11 System Tray Integration for iPhone Link (Pure Win32 API via ctypes).

Tahap 56:
- Zero external third-party dependencies (works identically in Python venv & PyInstaller .exe).
- Uses Shell_NotifyIconW (NOTIFYICONDATAW) with official assets/iphone_link.ico.
- Left-click / Double-click toggles Show/Restore or Hide to System Tray.
- Right-click opens native Windows popup menu with 7 quick actions + clean Quit.
- Guarantees NIM_DELETE cleanup on exit so zero ghost icons remain in Windows 11 System Tray.
"""

from __future__ import annotations

import ctypes
import os
import sys
import threading
from ctypes import wintypes
from pathlib import Path
from typing import Callable, Dict, List, Optional, Tuple

# Win32 Constants
WM_NULL = 0x0000
WM_DESTROY = 0x0002
WM_CLOSE = 0x0010
WM_QUIT = 0x0012
WM_CONTEXTMENU = 0x007B
WM_LBUTTONUP = 0x0202
WM_LBUTTONDBLCLK = 0x0203
WM_RBUTTONUP = 0x0205
WM_APP = 0x8000
WM_TRAYICON = WM_APP + 20

NIM_ADD = 0x00000000
NIM_MODIFY = 0x00000001
NIM_DELETE = 0x00000002

NIF_MESSAGE = 0x00000001
NIF_ICON = 0x00000002
NIF_TIP = 0x00000004

IMAGE_ICON = 1
LR_LOADFROMFILE = 0x0010
LR_DEFAULTSIZE = 0x0040
IDI_APPLICATION = 32512
SM_CXSMICON = 49
SM_CYSMICON = 50

MF_STRING = 0x00000000
MF_CHECKED = 0x00000008
MF_SEPARATOR = 0x00000800

TPM_RIGHTBUTTON = 0x0002
TPM_NONOTIFY = 0x0080
TPM_RETURNCMD = 0x0100
TPM_BOTTOMALIGN = 0x0020

# Tray Popup Menu Command IDs
ID_RESTORE = 1001
ID_HIDE = 1002
ID_LANDSCAPE = 1003
ID_STEALTH = 1004
ID_AUDIO = 1005
ID_CLEAR_CACHE = 1006
ID_QUIT = 1009

TRAY_TOOLTIP = "iPhone Link - Native iPhone Mirroring"

if sys.platform == "win32":
    LRESULT = ctypes.c_ssize_t
    WNDPROC = ctypes.WINFUNCTYPE(LRESULT, wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM)

    class GUID(ctypes.Structure):
        _fields_ = [
            ("Data1", wintypes.DWORD),
            ("Data2", wintypes.WORD),
            ("Data3", wintypes.WORD),
            ("Data4", ctypes.c_ubyte * 8),
        ]

    class NOTIFYICONDATAW(ctypes.Structure):
        _fields_ = [
            ("cbSize", wintypes.DWORD),
            ("hWnd", wintypes.HWND),
            ("uID", wintypes.UINT),
            ("uFlags", wintypes.UINT),
            ("uCallbackMessage", wintypes.UINT),
            ("hIcon", wintypes.HICON),
            ("szTip", wintypes.WCHAR * 128),
            ("dwState", wintypes.DWORD),
            ("dwStateMask", wintypes.DWORD),
            ("szInfo", wintypes.WCHAR * 256),
            ("uVersion", wintypes.UINT),
            ("szInfoTitle", wintypes.WCHAR * 64),
            ("dwInfoFlags", wintypes.DWORD),
            ("guidItem", GUID),
            ("hBalloonIcon", wintypes.HICON),
        ]

    class WNDCLASSW(ctypes.Structure):
        _fields_ = [
            ("style", wintypes.UINT),
            ("lpfnWndProc", WNDPROC),
            ("cbClsExtra", ctypes.c_int),
            ("cbWndExtra", ctypes.c_int),
            ("hInstance", wintypes.HINSTANCE),
            ("hIcon", wintypes.HICON),
            ("hCursor", wintypes.HANDLE),
            ("hbrBackground", wintypes.HANDLE),
            ("lpszMenuName", wintypes.LPCWSTR),
            ("lpszClassName", wintypes.LPCWSTR),
        ]


def _setup_win32_apis():
    """Create isolated WinDLL bindings with explicit 64-bit safe argtypes/restype."""
    user32 = ctypes.WinDLL("user32", use_last_error=True)
    shell32 = ctypes.WinDLL("shell32", use_last_error=True)
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)

    kernel32.GetModuleHandleW.argtypes = [wintypes.LPCWSTR]
    kernel32.GetModuleHandleW.restype = wintypes.HMODULE

    user32.DefWindowProcW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM]
    user32.DefWindowProcW.restype = LRESULT

    user32.RegisterClassW.argtypes = [ctypes.POINTER(WNDCLASSW)]
    user32.RegisterClassW.restype = wintypes.ATOM

    user32.UnregisterClassW.argtypes = [wintypes.LPCWSTR, wintypes.HINSTANCE]
    user32.UnregisterClassW.restype = wintypes.BOOL

    user32.CreateWindowExW.argtypes = [
        wintypes.DWORD,
        wintypes.LPCWSTR,
        wintypes.LPCWSTR,
        wintypes.DWORD,
        ctypes.c_int,
        ctypes.c_int,
        ctypes.c_int,
        ctypes.c_int,
        wintypes.HWND,
        wintypes.HMENU,
        wintypes.HINSTANCE,
        wintypes.LPVOID,
    ]
    user32.CreateWindowExW.restype = wintypes.HWND

    user32.DestroyWindow.argtypes = [wintypes.HWND]
    user32.DestroyWindow.restype = wintypes.BOOL

    user32.GetMessageW.argtypes = [ctypes.POINTER(wintypes.MSG), wintypes.HWND, wintypes.UINT, wintypes.UINT]
    user32.GetMessageW.restype = wintypes.BOOL

    user32.TranslateMessage.argtypes = [ctypes.POINTER(wintypes.MSG)]
    user32.TranslateMessage.restype = wintypes.BOOL

    user32.DispatchMessageW.argtypes = [ctypes.POINTER(wintypes.MSG)]
    user32.DispatchMessageW.restype = LRESULT

    user32.PostMessageW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM]
    user32.PostMessageW.restype = wintypes.BOOL

    user32.PostQuitMessage.argtypes = [ctypes.c_int]
    user32.PostQuitMessage.restype = None

    user32.RegisterWindowMessageW.argtypes = [wintypes.LPCWSTR]
    user32.RegisterWindowMessageW.restype = wintypes.UINT

    user32.GetSystemMetrics.argtypes = [ctypes.c_int]
    user32.GetSystemMetrics.restype = ctypes.c_int

    user32.LoadImageW.argtypes = [
        wintypes.HINSTANCE,
        wintypes.LPCWSTR,
        wintypes.UINT,
        ctypes.c_int,
        ctypes.c_int,
        wintypes.UINT,
    ]
    user32.LoadImageW.restype = wintypes.HANDLE

    user32.LoadIconW.argtypes = [wintypes.HINSTANCE, wintypes.LPCWSTR]
    user32.LoadIconW.restype = wintypes.HICON

    user32.DestroyIcon.argtypes = [wintypes.HICON]
    user32.DestroyIcon.restype = wintypes.BOOL

    user32.CreatePopupMenu.argtypes = []
    user32.CreatePopupMenu.restype = wintypes.HMENU

    user32.AppendMenuW.argtypes = [wintypes.HMENU, wintypes.UINT, ctypes.c_size_t, wintypes.LPCWSTR]
    user32.AppendMenuW.restype = wintypes.BOOL

    user32.SetMenuDefaultItem.argtypes = [wintypes.HMENU, wintypes.UINT, wintypes.UINT]
    user32.SetMenuDefaultItem.restype = wintypes.BOOL

    user32.GetCursorPos.argtypes = [ctypes.POINTER(wintypes.POINT)]
    user32.GetCursorPos.restype = wintypes.BOOL

    user32.SetForegroundWindow.argtypes = [wintypes.HWND]
    user32.SetForegroundWindow.restype = wintypes.BOOL

    user32.TrackPopupMenu.argtypes = [
        wintypes.HMENU,
        wintypes.UINT,
        ctypes.c_int,
        ctypes.c_int,
        ctypes.c_int,
        wintypes.HWND,
        wintypes.LPVOID,
    ]
    user32.TrackPopupMenu.restype = wintypes.BOOL

    user32.DestroyMenu.argtypes = [wintypes.HMENU]
    user32.DestroyMenu.restype = wintypes.BOOL

    shell32.Shell_NotifyIconW.argtypes = [wintypes.DWORD, ctypes.POINTER(NOTIFYICONDATAW)]
    shell32.Shell_NotifyIconW.restype = wintypes.BOOL

    return user32, shell32, kernel32


class SystemTrayIcon:
    """Pure Win32 System Tray controller for Windows 11 notification area."""

    def __init__(
        self,
        icon_path: Optional[Path] = None,
        tooltip: str = TRAY_TOOLTIP,
        on_restore: Optional[Callable[[], None]] = None,
        on_hide: Optional[Callable[[], None]] = None,
        on_toggle_window: Optional[Callable[[], None]] = None,
        on_toggle_landscape: Optional[Callable[[], None]] = None,
        on_toggle_stealth: Optional[Callable[[], None]] = None,
        on_toggle_audio: Optional[Callable[[], None]] = None,
        on_clear_cache: Optional[Callable[[], None]] = None,
        on_quit: Optional[Callable[[], None]] = None,
        state_provider: Optional[Callable[[], Dict[str, bool]]] = None,
    ) -> None:
        self.icon_path = Path(icon_path) if icon_path else None
        self.tooltip = tooltip[:127]
        self.on_restore = on_restore
        self.on_hide = on_hide
        self.on_toggle_window = on_toggle_window
        self.on_toggle_landscape = on_toggle_landscape
        self.on_toggle_stealth = on_toggle_stealth
        self.on_toggle_audio = on_toggle_audio
        self.on_clear_cache = on_clear_cache
        self.on_quit = on_quit
        self.state_provider = state_provider

        self._lock = threading.Lock()
        self._thread: Optional[threading.Thread] = None
        self._ready_event = threading.Event()
        self._stopped = False
        self._registered = False
        self._hwnd: Optional[int] = None
        self._hicon: Optional[int] = None
        self._owns_icon = False
        self._nid = None
        self._wndproc_ref = None
        self._class_name = f"iPhoneLinkTrayWnd_{os.getpid()}_{id(self)}"
        self._wm_taskbar_created = 0
        self._user32 = None
        self._shell32 = None
        self._kernel32 = None

    @property
    def is_running(self) -> bool:
        return bool(self._hwnd and not self._stopped)

    @property
    def is_icon_registered(self) -> bool:
        return bool(self._registered and not self._stopped)

    def get_menu_spec(self) -> List[Tuple[Optional[int], str, int]]:
        """Return ordered specification of popup menu entries: (cmd_id, label, flags)."""
        st: Dict[str, bool] = {}
        if self.state_provider is not None:
            try:
                st = self.state_provider() or {}
            except Exception:
                st = {}

        landscape_flag = MF_STRING | (MF_CHECKED if st.get("landscape") else 0)
        stealth_flag = MF_STRING | (MF_CHECKED if st.get("stealth") else 0)
        audio_flag = MF_STRING | (MF_CHECKED if st.get("audio") else 0)

        return [
            (ID_RESTORE, "Show / Restore iPhone Link", MF_STRING),
            (ID_HIDE, "Hide to System Tray", MF_STRING),
            (None, "", MF_SEPARATOR),
            (ID_LANDSCAPE, "Landscape Mode (Alt+H)", landscape_flag),
            (ID_STEALTH, "Stealth Dim Mode (Alt+D)", stealth_flag),
            (ID_AUDIO, "Stream Audio to PC", audio_flag),
            (ID_CLEAR_CACHE, "Clear Copy Cache (Alt+R)", MF_STRING),
            (None, "", MF_SEPARATOR),
            (ID_QUIT, "Quit iPhone Link", MF_STRING),
        ]

    def start(self, wait_ready: float = 2.0) -> bool:
        """Start background Win32 message loop and add tray icon cleanly."""
        if sys.platform != "win32":
            return False
        with self._lock:
            if self._thread is not None and self._thread.is_alive():
                return self.is_running
            self._stopped = False
            self._ready_event.clear()
            self._thread = threading.Thread(
                target=self._message_loop,
                name="iphone-link-tray-thread",
                daemon=True,
            )
            self._thread.start()

        if wait_ready > 0:
            self._ready_event.wait(timeout=wait_ready)
        return self.is_running

    def stop(self) -> None:
        """Remove tray icon (NIM_DELETE) and terminate message loop with zero ghost icons."""
        if sys.platform != "win32":
            return
        with self._lock:
            if self._stopped:
                return
            self._stopped = True
            hwnd = self._hwnd
            nid = self._nid
            shell32 = self._shell32
            user32 = self._user32

            if self._registered and shell32 is not None and nid is not None:
                try:
                    shell32.Shell_NotifyIconW(NIM_DELETE, ctypes.byref(nid))
                except Exception:
                    pass
                self._registered = False

            if hwnd and user32 is not None:
                try:
                    user32.PostMessageW(wintypes.HWND(hwnd), WM_CLOSE, 0, 0)
                except Exception:
                    pass

        t = self._thread
        if t is not None and t.is_alive() and t is not threading.current_thread():
            t.join(timeout=1.5)

    def _load_tray_hicon(self, user32) -> int:
        if self.icon_path and self.icon_path.is_file():
            cx = user32.GetSystemMetrics(SM_CXSMICON) or 16
            cy = user32.GetSystemMetrics(SM_CYSMICON) or 16
            hicon = user32.LoadImageW(
                None,
                str(self.icon_path),
                IMAGE_ICON,
                cx,
                cy,
                LR_LOADFROMFILE,
            )
            if not hicon:
                hicon = user32.LoadImageW(
                    None,
                    str(self.icon_path),
                    IMAGE_ICON,
                    0,
                    0,
                    LR_LOADFROMFILE | LR_DEFAULTSIZE,
                )
            if hicon:
                self._owns_icon = True
                return hicon

        self._owns_icon = False
        return user32.LoadIconW(None, ctypes.cast(IDI_APPLICATION, wintypes.LPCWSTR))

    def _dispatch_action_async(self, fn: Optional[Callable[[], None]]) -> None:
        if fn is None:
            return
        threading.Thread(target=self._safe_call, args=(fn,), daemon=True).start()

    @staticmethod
    def _safe_call(fn: Callable[[], None]) -> None:
        try:
            fn()
        except Exception as err:
            print(f"SystemTrayIcon callback error: {err}", flush=True)

    def _handle_command(self, cmd: int) -> None:
        if cmd == ID_RESTORE:
            self._dispatch_action_async(self.on_restore)
        elif cmd == ID_HIDE:
            self._dispatch_action_async(self.on_hide)
        elif cmd == ID_LANDSCAPE:
            self._dispatch_action_async(self.on_toggle_landscape)
        elif cmd == ID_STEALTH:
            self._dispatch_action_async(self.on_toggle_stealth)
        elif cmd == ID_AUDIO:
            self._dispatch_action_async(self.on_toggle_audio)
        elif cmd == ID_CLEAR_CACHE:
            self._dispatch_action_async(self.on_clear_cache)
        elif cmd == ID_QUIT:
            self._dispatch_action_async(self.on_quit)

    def _try_register_icon(self) -> bool:
        """Register tray icon with Windows Explorer, falling back to V2 struct size if needed."""
        if self._shell32 is None or self._nid is None or self._stopped:
            return False
        for candidate_size in (ctypes.sizeof(NOTIFYICONDATAW), NOTIFYICONDATAW.guidItem.offset):
            self._nid.cbSize = candidate_size
            if bool(self._shell32.Shell_NotifyIconW(NIM_ADD, ctypes.byref(self._nid))):
                self._registered = True
                return True
        return False

    def create_popup_menu_handle(self):
        """Build native Win32 HMENU with all 7 quick actions, 2 separators, and bold default item."""
        user32 = self._user32
        if user32 is None:
            return None
        hmenu = user32.CreatePopupMenu()
        if not hmenu:
            return None
        for cmd_id, label, flags in self.get_menu_spec():
            if flags & MF_SEPARATOR:
                user32.AppendMenuW(hmenu, MF_SEPARATOR, 0, None)
            elif cmd_id is not None:
                user32.AppendMenuW(hmenu, flags, cmd_id, label)
        user32.SetMenuDefaultItem(hmenu, ID_RESTORE, 0)
        return hmenu

    def _show_context_menu(self, hwnd: int) -> None:
        user32 = self._user32
        if user32 is None:
            return
        hmenu = self.create_popup_menu_handle()
        if not hmenu:
            return
        try:
            pt = wintypes.POINT()
            user32.GetCursorPos(ctypes.byref(pt))
            user32.SetForegroundWindow(wintypes.HWND(hwnd))

            cmd = int(
                user32.TrackPopupMenu(
                    hmenu,
                    TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
                    pt.x,
                    pt.y,
                    0,
                    wintypes.HWND(hwnd),
                    None,
                )
            )
            user32.PostMessageW(wintypes.HWND(hwnd), WM_NULL, 0, 0)
            if cmd > 0:
                self._handle_command(cmd)
        finally:
            user32.DestroyMenu(hmenu)

    def _message_loop(self) -> None:
        try:
            user32, shell32, kernel32 = _setup_win32_apis()
            self._user32 = user32
            self._shell32 = shell32
            self._kernel32 = kernel32

            hinstance = kernel32.GetModuleHandleW(None)
            self._wm_taskbar_created = user32.RegisterWindowMessageW("TaskbarCreated")

            def _wnd_proc(hwnd, msg, wparam, lparam):
                if msg == WM_TRAYICON:
                    event = lparam & 0xFFFF
                    if event == WM_LBUTTONUP:
                        cb = self.on_toggle_window or self.on_restore
                        self._dispatch_action_async(cb)
                        return 0
                    if event == WM_LBUTTONDBLCLK:
                        self._dispatch_action_async(self.on_restore)
                        return 0
                    if event in (WM_RBUTTONUP, WM_CONTEXTMENU):
                        self._show_context_menu(hwnd)
                        return 0
                    return 0

                if self._wm_taskbar_created and msg == self._wm_taskbar_created:
                    # Re-register tray icon automatically if Windows Explorer starts/restarts
                    if not self._stopped and self._nid is not None:
                        self._try_register_icon()
                    return 0

                if msg == WM_CLOSE:
                    if self._registered and self._nid is not None:
                        shell32.Shell_NotifyIconW(NIM_DELETE, ctypes.byref(self._nid))
                        self._registered = False
                    user32.DestroyWindow(hwnd)
                    return 0

                if msg == WM_DESTROY:
                    user32.PostQuitMessage(0)
                    return 0

                return user32.DefWindowProcW(hwnd, msg, wparam, lparam)

            self._wndproc_ref = WNDPROC(_wnd_proc)

            wc = WNDCLASSW()
            wc.style = 0
            wc.lpfnWndProc = self._wndproc_ref
            wc.cbClsExtra = 0
            wc.cbWndExtra = 0
            wc.hInstance = hinstance
            wc.hIcon = None
            wc.hCursor = None
            wc.hbrBackground = None
            wc.lpszMenuName = None
            wc.lpszClassName = self._class_name

            user32.RegisterClassW(ctypes.byref(wc))

            hwnd = user32.CreateWindowExW(
                0,
                self._class_name,
                "iPhoneLinkHiddenTrayHost",
                0,
                0,
                0,
                0,
                0,
                None,
                None,
                hinstance,
                None,
            )
            if not hwnd:
                self._ready_event.set()
                return

            self._hwnd = hwnd
            self._hicon = self._load_tray_hicon(user32)

            nid = NOTIFYICONDATAW()
            ctypes.memset(ctypes.byref(nid), 0, ctypes.sizeof(nid))
            nid.cbSize = ctypes.sizeof(NOTIFYICONDATAW)
            nid.hWnd = hwnd
            nid.uID = 1
            nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP
            nid.uCallbackMessage = WM_TRAYICON
            nid.hIcon = self._hicon
            nid.szTip = self.tooltip
            self._nid = nid

            if not self._stopped:
                self._try_register_icon()

            self._ready_event.set()

            msg = wintypes.MSG()
            while user32.GetMessageW(ctypes.byref(msg), None, 0, 0) > 0:
                user32.TranslateMessage(ctypes.byref(msg))
                user32.DispatchMessageW(ctypes.byref(msg))
        except Exception as err:
            print(f"SystemTrayIcon message loop error: {err}", flush=True)
            self._ready_event.set()
        finally:
            try:
                if self._registered and self._shell32 is not None and self._nid is not None:
                    self._shell32.Shell_NotifyIconW(NIM_DELETE, ctypes.byref(self._nid))
                    self._registered = False
            except Exception:
                pass
            try:
                if self._owns_icon and self._hicon and self._user32 is not None:
                    self._user32.DestroyIcon(wintypes.HICON(self._hicon))
            except Exception:
                pass
            try:
                if self._user32 is not None and self._kernel32 is not None:
                    self._user32.UnregisterClassW(self._class_name, self._kernel32.GetModuleHandleW(None))
            except Exception:
                pass
            self._hwnd = None
            self._hicon = None
