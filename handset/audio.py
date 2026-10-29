"""Audio Sink Manager for iPhone -> PC Audio Routing via Windows 11 Bluetooth A2DP.

Tahap 56–57 Zero-Stutter Bluetooth A2DP Architecture:
- Runs the PowerShell WinRT AudioPlaybackConnection host explicitly in Multi-Threaded Apartment (-MTA)
  with an active Win32 message pump (MsgWaitForMultipleObjectsEx + PeekMessageW) and async STDIN reader
  ([Console]::In.ReadLineAsync()), eliminating STA COM message starvation during Reels/TikTok playback.
- Pins $Global:ActiveAudioConn with [System.GC]::KeepAlive so .NET GC never collects or relocates the RCW.
- Caches Bluetooth endpoint DeviceId (_cached_device_id) so subsequent connects call TryCreateFromId()
  directly without scanning all radios via DeviceInformation::FindAllAsync.
- Disables Windows 11 EcoQoS Power Throttling, sets High Priority (including audiodg.exe when permitted),
  requests 1ms multimedia timer resolution, and registers MMCSS ("Pro Audio").
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import threading
from typing import Any, Dict, Optional

_WIN_NO_WINDOW_KWARGS: dict = {}
if sys.platform == "win32":
    _si = subprocess.STARTUPINFO()
    _si.dwFlags |= subprocess.STARTF_USESHOWWINDOW
    _si.wShowWindow = 0  # SW_HIDE
    _WIN_NO_WINDOW_KWARGS = {
        "creationflags": getattr(subprocess, "CREATE_NO_WINDOW", 0x08000000),
        "startupinfo": _si,
    }

ABOVE_NORMAL_PRIORITY_CLASS = 0x00008000
HIGH_PRIORITY_CLASS = 0x00000080
PROCESS_POWER_THROTTLING = 4
PROCESS_POWER_THROTTLING_CURRENT_VERSION = 1
PROCESS_POWER_THROTTLING_EXECUTION_SPEED = 0x1
PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION = 0x4

_TIMER_LOCK = threading.Lock()
_TIMER_1MS_ACTIVE = False


def disable_process_power_throttling() -> bool:
    """Disable Windows 11 EcoQoS / Efficiency Mode throttling on the current process."""
    if sys.platform != "win32":
        return False
    try:
        import ctypes
        from ctypes import wintypes

        class PROCESS_POWER_THROTTLING_STATE(ctypes.Structure):
            _fields_ = [
                ("Version", wintypes.ULONG),
                ("ControlMask", wintypes.ULONG),
                ("StateMask", wintypes.ULONG),
            ]

        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel32.GetCurrentProcess.argtypes = []
        kernel32.GetCurrentProcess.restype = wintypes.HANDLE
        kernel32.SetProcessInformation.argtypes = [
            wintypes.HANDLE,
            ctypes.c_int,
            wintypes.LPVOID,
            wintypes.DWORD,
        ]
        kernel32.SetProcessInformation.restype = wintypes.BOOL

        hproc = kernel32.GetCurrentProcess()
        state = PROCESS_POWER_THROTTLING_STATE(
            Version=PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask=(
                PROCESS_POWER_THROTTLING_EXECUTION_SPEED
                | PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION
            ),
            StateMask=0,
        )
        ok = bool(
            kernel32.SetProcessInformation(
                hproc,
                PROCESS_POWER_THROTTLING,
                ctypes.byref(state),
                ctypes.sizeof(state),
            )
        )
        if not ok:
            state_fallback = PROCESS_POWER_THROTTLING_STATE(
                Version=PROCESS_POWER_THROTTLING_CURRENT_VERSION,
                ControlMask=PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
                StateMask=0,
            )
            ok = bool(
                kernel32.SetProcessInformation(
                    hproc,
                    PROCESS_POWER_THROTTLING,
                    ctypes.byref(state_fallback),
                    ctypes.sizeof(state_fallback),
                )
            )
        return ok
    except Exception:
        return False


def set_audio_process_priority(high_for_audio: bool = False) -> bool:
    """Set HIGH_PRIORITY_CLASS when A2DP audio is active, else ABOVE_NORMAL_PRIORITY_CLASS."""
    if sys.platform != "win32":
        return False
    try:
        import ctypes
        from ctypes import wintypes

        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel32.GetCurrentProcess.argtypes = []
        kernel32.GetCurrentProcess.restype = wintypes.HANDLE
        kernel32.SetPriorityClass.argtypes = [wintypes.HANDLE, wintypes.DWORD]
        kernel32.SetPriorityClass.restype = wintypes.BOOL

        prio = HIGH_PRIORITY_CLASS if high_for_audio else ABOVE_NORMAL_PRIORITY_CLASS
        return bool(kernel32.SetPriorityClass(kernel32.GetCurrentProcess(), prio))
    except Exception:
        return False


def set_multimedia_timer_1ms(enable: bool) -> bool:
    """Request 1ms Windows multimedia scheduler resolution while A2DP audio is streaming."""
    global _TIMER_1MS_ACTIVE
    if sys.platform != "win32":
        return False
    with _TIMER_LOCK:
        try:
            import ctypes

            winmm = ctypes.WinDLL("winmm", use_last_error=True)
            if enable and not _TIMER_1MS_ACTIVE:
                res = int(winmm.timeBeginPeriod(1))
                if res == 0:
                    _TIMER_1MS_ACTIVE = True
                return _TIMER_1MS_ACTIVE
            if not enable and _TIMER_1MS_ACTIVE:
                winmm.timeEndPeriod(1)
                _TIMER_1MS_ACTIVE = False
                return True
            return True
        except Exception:
            return False


def register_mmcss_audio_thread() -> int:
    """Register current thread with Windows Multimedia Class Scheduler Service (MMCSS)."""
    if sys.platform != "win32":
        return 0
    try:
        import ctypes
        from ctypes import wintypes

        avrt = ctypes.WinDLL("avrt", use_last_error=True)
        avrt.AvSetMmThreadCharacteristicsW.argtypes = [wintypes.LPCWSTR, ctypes.POINTER(wintypes.DWORD)]
        avrt.AvSetMmThreadCharacteristicsW.restype = wintypes.HANDLE

        task_idx = wintypes.DWORD(0)
        handle = avrt.AvSetMmThreadCharacteristicsW("Pro Audio", ctypes.byref(task_idx))
        if not handle:
            handle = avrt.AvSetMmThreadCharacteristicsW("Audio", ctypes.byref(task_idx))
        return int(handle or 0)
    except Exception:
        return 0


class AudioManager:
    """Manages Windows 11 AudioPlaybackConnection (A2DP Sink) and Master Volume."""

    def __init__(self):
        self._enabled = False
        self._proc: Optional[subprocess.Popen] = None
        self._device_name = "iPhone"
        self._cached_device_id: str = ""
        self._connection_state = "Closed"
        self._lock = threading.Lock()
        self._monitor_thread: Optional[threading.Thread] = None
        self._cached_volume: int = 100
        self._volume_initialized: bool = False

    def is_active(self) -> bool:
        """Fast non-blocking check whether A2DP audio sink worker is currently live."""
        proc = self._proc
        return bool(self._enabled and proc is not None and proc.poll() is None)

    def status(self) -> Dict[str, Any]:
        """Return instant in-memory A2DP status (0 radio scans, 0 subprocesses, 0 COM stalls)."""
        need_initial_vol = False
        with self._lock:
            if self._proc is not None and self._proc.poll() is not None:
                self._proc = None
                self._connection_state = "Closed"
                set_multimedia_timer_1ms(False)
                set_audio_process_priority(high_for_audio=False)

            is_active = self._enabled and (self._proc is not None)
            if not self._volume_initialized and not is_active:
                need_initial_vol = True

            snapshot_state = {
                "enabled": self._enabled,
                "active": is_active,
                "device": self._device_name,
                "state": self._connection_state if is_active else "Off",
                "volume": self._cached_volume,
            }

        if need_initial_vol:
            vol = self.get_volume()
            with self._lock:
                self._cached_volume = vol
                self._volume_initialized = True
                snapshot_state["volume"] = vol

        return snapshot_state

    def get_volume(self) -> int:
        try:
            import ctypes

            ctypes.windll.ole32.CoInitialize(None)
            try:
                from pycaw.pycaw import AudioUtilities

                dev = AudioUtilities.GetSpeakers()
                if dev and hasattr(dev, "EndpointVolume"):
                    vol = round(dev.EndpointVolume.GetMasterVolumeLevelScalar() * 100)
                    self._cached_volume = vol
                    self._volume_initialized = True
                    return vol
            finally:
                ctypes.windll.ole32.CoUninitialize()
        except Exception:
            pass
        return self._cached_volume

    def set_volume(self, level_percent: int) -> Dict[str, Any]:
        vol = max(0, min(100, int(level_percent)))
        scalar = vol / 100.0
        self._cached_volume = vol
        self._volume_initialized = True
        try:
            import ctypes

            ctypes.windll.ole32.CoInitialize(None)
            try:
                from pycaw.pycaw import AudioUtilities

                dev = AudioUtilities.GetSpeakers()
                if dev and hasattr(dev, "EndpointVolume"):
                    dev.EndpointVolume.SetMasterVolumeLevelScalar(scalar, None)
                    return {"ok": True, "volume": vol}
            finally:
                ctypes.windll.ole32.CoUninitialize()
        except Exception as e:
            return {"ok": False, "volume": vol, "error": str(e)}
        return {"ok": True, "volume": vol}

    def open_bluetooth_settings(self) -> Dict[str, Any]:
        try:
            os.startfile("ms-settings:bluetooth")
            return {"ok": True}
        except Exception as e:
            return {"ok": False, "error": str(e)}

    def enable(self) -> Dict[str, Any]:
        disable_process_power_throttling()
        set_audio_process_priority(high_for_audio=True)
        set_multimedia_timer_1ms(True)

        with self._lock:
            if self._proc is not None and self._proc.poll() is None:
                self._enabled = True
                return {
                    "enabled": True,
                    "active": True,
                    "device": self._device_name,
                    "state": self._connection_state,
                    "volume": self._cached_volume,
                }

            ps_script = """
try {
    [System.Diagnostics.Process]::GetCurrentProcess().PriorityClass = [System.Diagnostics.ProcessPriorityClass]::High
} catch {}

try {
    Get-Process -Name audiodg -ErrorAction SilentlyContinue | ForEach-Object {
        try { $_.PriorityClass = [System.Diagnostics.ProcessPriorityClass]::High } catch {}
    }
} catch {}

try {
    Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public static class WinAudioBoost {
    [StructLayout(LayoutKind.Sequential)]
    public struct PROCESS_POWER_THROTTLING_STATE {
        public uint Version;
        public uint ControlMask;
        public uint StateMask;
    }
    [StructLayout(LayoutKind.Sequential)]
    public struct POINT {
        public int X;
        public int Y;
    }
    [StructLayout(LayoutKind.Sequential)]
    public struct MSG {
        public IntPtr hwnd;
        public uint message;
        public IntPtr wParam;
        public IntPtr lParam;
        public uint time;
        public POINT pt;
    }
    [DllImport("kernel32.dll", SetLastError=true)]
    public static extern IntPtr GetCurrentProcess();
    [DllImport("kernel32.dll", SetLastError=true)]
    public static extern bool SetProcessInformation(IntPtr hProcess, int ProcessInformationClass, ref PROCESS_POWER_THROTTLING_STATE State, int Size);
    [DllImport("winmm.dll")]
    public static extern uint timeBeginPeriod(uint uPeriod);
    [DllImport("winmm.dll")]
    public static extern uint timeEndPeriod(uint uPeriod);
    [DllImport("avrt.dll", CharSet=CharSet.Unicode)]
    public static extern IntPtr AvSetMmThreadCharacteristicsW(string TaskName, out uint TaskIndex);
    [DllImport("avrt.dll")]
    public static extern bool AvRevertMmThreadCharacteristics(IntPtr AvrtHandle);
    [DllImport("user32.dll")]
    public static extern uint MsgWaitForMultipleObjectsEx(uint nCount, IntPtr pHandles, uint dwMilliseconds, uint dwWakeMask, uint dwFlags);
    [DllImport("user32.dll")]
    public static extern bool PeekMessageW(out MSG lpMsg, IntPtr hWnd, uint wMsgFilterMin, uint wMsgFilterMax, uint wRemoveMsg);
    [DllImport("user32.dll")]
    public static extern bool TranslateMessage(ref MSG lpMsg);
    [DllImport("user32.dll")]
    public static extern IntPtr DispatchMessageW(ref MSG lpMsg);

    private static IntPtr _mmcss = IntPtr.Zero;

    public static void Boost() {
        var st = new PROCESS_POWER_THROTTLING_STATE { Version = 1, ControlMask = 5, StateMask = 0 };
        if (!SetProcessInformation(GetCurrentProcess(), 4, ref st, Marshal.SizeOf(st))) {
            var st2 = new PROCESS_POWER_THROTTLING_STATE { Version = 1, ControlMask = 1, StateMask = 0 };
            SetProcessInformation(GetCurrentProcess(), 4, ref st2, Marshal.SizeOf(st2));
        }
        timeBeginPeriod(1);
        uint idx = 0;
        _mmcss = AvSetMmThreadCharacteristicsW("Pro Audio", out idx);
        if (_mmcss == IntPtr.Zero) {
            _mmcss = AvSetMmThreadCharacteristicsW("Audio", out idx);
        }
        MSG initMsg;
        PeekMessageW(out initMsg, IntPtr.Zero, 0, 0, 0x0000);
    }

    public static void PumpWait(uint timeoutMs) {
        MSG msg;
        while (PeekMessageW(out msg, IntPtr.Zero, 0, 0, 0x0001)) {
            TranslateMessage(ref msg);
            DispatchMessageW(ref msg);
        }
        uint res = MsgWaitForMultipleObjectsEx(0, IntPtr.Zero, timeoutMs, 0x04FF, 0x0004);
        if (res == 0xFFFFFFFF) {
            System.Threading.Thread.Sleep((int)timeoutMs);
        }
        while (PeekMessageW(out msg, IntPtr.Zero, 0, 0, 0x0001)) {
            TranslateMessage(ref msg);
            DispatchMessageW(ref msg);
        }
    }

    public static void Unboost() {
        if (_mmcss != IntPtr.Zero) {
            AvRevertMmThreadCharacteristics(_mmcss);
            _mmcss = IntPtr.Zero;
        }
        timeEndPeriod(1);
    }
}
"@ -ErrorAction SilentlyContinue
    [WinAudioBoost]::Boost()
} catch {}

Add-Type -AssemblyName System.Runtime.WindowsRuntime
$null = [Windows.Media.Audio.AudioPlaybackConnection, Windows.Media.Audio, ContentType = WindowsRuntime]
$null = [Windows.Devices.Enumeration.DeviceInformation, Windows.Devices.Enumeration, ContentType = WindowsRuntime]

$conn = $null
$devId = $env:PANE_CACHED_AUDIO_DEVICE_ID
$devName = if ($env:PANE_CACHED_AUDIO_DEVICE_NAME) { $env:PANE_CACHED_AUDIO_DEVICE_NAME } else { "iPhone" }

if (-not [string]::IsNullOrWhiteSpace($devId)) {
    try {
        $conn = [Windows.Media.Audio.AudioPlaybackConnection]::TryCreateFromId($devId)
    } catch {
        $conn = $null
    }
}

if ($conn -eq $null) {
    $selector = [Windows.Media.Audio.AudioPlaybackConnection]::GetDeviceSelector()
    $asTaskGeneric = [System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object { $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.IsGenericMethod } | Select-Object -First 1
    $asTask = $asTaskGeneric.MakeGenericMethod([Windows.Devices.Enumeration.DeviceInformationCollection])
    $op = [Windows.Devices.Enumeration.DeviceInformation]::FindAllAsync($selector)
    $task = $asTask.Invoke($null, @($op))
    $devices = $task.GetAwaiter().GetResult()

    if ($devices.Count -gt 0) {
        $dev = $devices[0]
        $devId = $dev.Id
        $devName = $dev.Name
        $conn = [Windows.Media.Audio.AudioPlaybackConnection]::TryCreateFromId($devId)
    } else {
        Write-Output (@{ status = "no_device"; message = "No Bluetooth Audio Sink device found" } | ConvertTo-Json -Compress)
    }
}

if ($conn -eq $null) {
    if (-not [string]::IsNullOrWhiteSpace($devId)) {
        Write-Output (@{ status = "error"; message = "Failed to create AudioPlaybackConnection" } | ConvertTo-Json -Compress)
    }
    $hasBoost = $null -ne ([System.Management.Automation.PSTypeName]'WinAudioBoost').Type
    $readStop = [Console]::In.ReadLineAsync()
    while (-not $readStop.IsCompleted) {
        if ($hasBoost) { [WinAudioBoost]::PumpWait(250) } else { Start-Sleep -Milliseconds 250 }
    }
} else {
    $Global:ActiveAudioConn = $conn
    $Global:ActiveAudioConn.Start()
    $openRes = $Global:ActiveAudioConn.Open()
    [System.GC]::KeepAlive($Global:ActiveAudioConn)
    Write-Output (@{
        status = "active"
        device = $devName
        deviceId = $devId
        state = $Global:ActiveAudioConn.State.ToString()
        apartment = [System.Threading.Thread]::CurrentThread.GetApartmentState().ToString()
    } | ConvertTo-Json -Compress)

    $hasBoost = $null -ne ([System.Management.Automation.PSTypeName]'WinAudioBoost').Type
    $readTask = [Console]::In.ReadLineAsync()
    while ($true) {
        if ($readTask.IsCompleted) {
            $line = $readTask.Result
            if ($line -eq "stop" -or $null -eq $line) {
                break
            }
            $readTask = [Console]::In.ReadLineAsync()
        }
        if ($hasBoost) { [WinAudioBoost]::PumpWait(200) } else { Start-Sleep -Milliseconds 200 }
        [System.GC]::KeepAlive($Global:ActiveAudioConn)
    }
    $Global:ActiveAudioConn.Dispose()
    $Global:ActiveAudioConn = $null
}
try { [WinAudioBoost]::Unboost() } catch {}
"""
            try:
                worker_env = dict(os.environ)
                if self._cached_device_id:
                    worker_env["PANE_CACHED_AUDIO_DEVICE_ID"] = self._cached_device_id
                if self._device_name:
                    worker_env["PANE_CACHED_AUDIO_DEVICE_NAME"] = self._device_name

                self._proc = subprocess.Popen(
                    [
                        "powershell",
                        "-NoProfile",
                        "-NonInteractive",
                        "-MTA",
                        "-ExecutionPolicy",
                        "Bypass",
                        "-Command",
                        ps_script,
                    ],
                    stdin=subprocess.PIPE,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                    env=worker_env,
                    text=True,
                    bufsize=1,
                    **_WIN_NO_WINDOW_KWARGS,
                )
                self._enabled = True

                def read_init():
                    register_mmcss_audio_thread()
                    try:
                        if self._proc and self._proc.stdout:
                            line = self._proc.stdout.readline()
                            if line:
                                data = json.loads(line.strip())
                                if data.get("device"):
                                    self._device_name = str(data["device"])
                                if data.get("deviceId"):
                                    self._cached_device_id = str(data["deviceId"])
                                if data.get("state"):
                                    self._connection_state = str(data["state"])
                    except Exception:
                        pass

                t = threading.Thread(target=read_init, daemon=True)
                t.start()
                t.join(timeout=2.0)

            except Exception as e:
                print(f"AudioManager enable failed: {e}", flush=True)
                self._proc = None
                self._enabled = False
                set_multimedia_timer_1ms(False)
                set_audio_process_priority(high_for_audio=False)

        return self.status()

    def disable(self) -> Dict[str, Any]:
        with self._lock:
            self._enabled = False
            proc = self._proc
            self._proc = None
            self._connection_state = "Off"

        if proc is not None and proc.poll() is None:
            try:
                if proc.stdin:
                    proc.stdin.write("stop\n")
                    proc.stdin.flush()
                proc.wait(timeout=1.5)
            except Exception:
                try:
                    proc.terminate()
                except Exception:
                    pass

        set_multimedia_timer_1ms(False)
        set_audio_process_priority(high_for_audio=False)
        return self.status()

    def toggle(self, enable: Optional[bool] = None) -> Dict[str, Any]:
        if enable is None:
            should_enable = not self._enabled
        else:
            should_enable = bool(enable)

        if should_enable:
            return self.enable()
        return self.disable()

    def shutdown(self) -> None:
        self.disable()


# Global Singleton instance
audio_manager = AudioManager()
