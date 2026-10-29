"""Audio Sink Manager for iPhone -> PC Audio Routing via Windows 11 Bluetooth A2DP.

Tahap 56 Zero-Stutter Bluetooth A2DP Upgrades:
- Disables Windows 11 EcoQoS Power Throttling (ProcessPowerThrottling = 4) so background/minimized
  execution never starves A2DP buffers or coarsens timer resolution.
- Elevates process priority (HIGH_PRIORITY_CLASS during active audio, ABOVE_NORMAL_PRIORITY_CLASS baseline)
  and requests 1ms multimedia timer resolution (winmm.timeBeginPeriod(1)) + MMCSS thread registration.
- Zero-Enumeration Status Polling: status() returns instant in-memory cached state & cached volume
  without re-running WinRT DeviceInformation.FindAllAsync or CoreAudio COM enumeration on status polls.
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
        # Disable both Execution Speed throttling (EcoQoS) and Timer Resolution throttling
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
            # Fallback for older Windows 10 builds supporting only ExecutionSpeed mask (0x1)
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
                is_active = True
                return {
                    "enabled": True,
                    "active": is_active,
                    "device": self._device_name,
                    "state": self._connection_state,
                    "volume": self._cached_volume,
                }

            ps_script = """
try {
    [System.Diagnostics.Process]::GetCurrentProcess().PriorityClass = [System.Diagnostics.ProcessPriorityClass]::High
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
    [DllImport("kernel32.dll", SetLastError=true)]
    public static extern IntPtr GetCurrentProcess();
    [DllImport("kernel32.dll", SetLastError=true)]
    public static extern bool SetProcessInformation(IntPtr hProcess, int ProcessInformationClass, ref PROCESS_POWER_THROTTLING_STATE State, int Size);
    [DllImport("winmm.dll")]
    public static extern uint timeBeginPeriod(uint uPeriod);
    [DllImport("winmm.dll")]
    public static extern uint timeEndPeriod(uint uPeriod);
    public static void Boost() {
        var st = new PROCESS_POWER_THROTTLING_STATE { Version = 1, ControlMask = 5, StateMask = 0 };
        SetProcessInformation(GetCurrentProcess(), 4, ref st, Marshal.SizeOf(st));
        timeBeginPeriod(1);
    }
    public static void Unboost() {
        timeEndPeriod(1);
    }
}
"@ -ErrorAction SilentlyContinue
    [WinAudioBoost]::Boost()
} catch {}

Add-Type -AssemblyName System.Runtime.WindowsRuntime
$null = [Windows.Media.Audio.AudioPlaybackConnection, Windows.Media.Audio, ContentType = WindowsRuntime]
$null = [Windows.Devices.Enumeration.DeviceInformation, Windows.Devices.Enumeration, ContentType = WindowsRuntime]

$selector = [Windows.Media.Audio.AudioPlaybackConnection]::GetDeviceSelector()
$asTaskGeneric = [System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object { $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.IsGenericMethod } | Select-Object -First 1
$asTask = $asTaskGeneric.MakeGenericMethod([Windows.Devices.Enumeration.DeviceInformationCollection])
$op = [Windows.Devices.Enumeration.DeviceInformation]::FindAllAsync($selector)
$task = $asTask.Invoke($null, @($op))
$devices = $task.GetAwaiter().GetResult()

if ($devices.Count -eq 0) {
    Write-Output '{"status": "no_device", "message": "No Bluetooth Audio Sink device found"}'
    $null = [Console]::In.ReadLine()
} else {
    $dev = $devices[0]
    $conn = [Windows.Media.Audio.AudioPlaybackConnection]::TryCreateFromId($dev.Id)
    if ($conn -eq $null) {
        Write-Output '{"status": "error", "message": "Failed to create AudioPlaybackConnection"}'
        $null = [Console]::In.ReadLine()
    } else {
        $conn.Start()
        $openRes = $conn.Open()
        Write-Output ("{`"status`": `"active`", `"device`": `"" + $dev.Name + "`", `"state`": `"" + $conn.State.ToString() + "`"}")

        while ($true) {
            $line = [Console]::In.ReadLine()
            if ($line -eq "stop" -or $line -eq $null) {
                break
            }
        }
        $conn.Dispose()
    }
}
try { [WinAudioBoost]::Unboost() } catch {}
"""
            try:
                self._proc = subprocess.Popen(
                    [
                        "powershell",
                        "-NoProfile",
                        "-ExecutionPolicy",
                        "Bypass",
                        "-Command",
                        ps_script,
                    ],
                    stdin=subprocess.PIPE,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
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
                                if "device" in data:
                                    self._device_name = data["device"]
                                if "state" in data:
                                    self._connection_state = data["state"]
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
