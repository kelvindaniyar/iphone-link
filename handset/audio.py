"""Audio Sink Manager for iPhone -> PC Audio Routing via Windows 11 Bluetooth A2DP.

Tahap 58: Restored to pure Tahap 53 (31b03c6) WinRT AudioPlaybackConnection architecture
with _WIN_NO_WINDOW_KWARGS (zero PowerShell popup) + Pre-Start Orphan Audio Cleanup
so duplicate AudioPlaybackConnection workers can never coexist in the background.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import threading
from pathlib import Path
from typing import Any, Dict, Optional

from handset import get_app_root

_WIN_NO_WINDOW_KWARGS: dict = {}
if sys.platform == "win32":
    _si = subprocess.STARTUPINFO()
    _si.dwFlags |= subprocess.STARTF_USESHOWWINDOW
    _si.wShowWindow = 0  # SW_HIDE
    _WIN_NO_WINDOW_KWARGS = {
        "creationflags": getattr(subprocess, "CREATE_NO_WINDOW", 0x08000000),
        "startupinfo": _si,
    }

AUDIO_WORKER_MARKER = "iPhoneLink-AudioPlaybackWorker"


def _audio_pid_file() -> Path:
    return get_app_root() / ".audio_worker.pid"


def cleanup_orphan_audio_workers(exclude_pid: Optional[int] = None) -> int:
    """Terminate any leftover PowerShell AudioPlaybackConnection or orphan audio.py workers."""
    if sys.platform != "win32":
        return 0
    killed = 0
    my_pid = os.getpid()

    # 1. Fast PID-file cleanup if previous session left a tracked worker PID
    pid_file = _audio_pid_file()
    if pid_file.is_file():
        try:
            old_pid = int(pid_file.read_text(encoding="utf-8").strip())
            if old_pid > 0 and old_pid != my_pid and old_pid != exclude_pid:
                subprocess.run(
                    ["taskkill", "/F", "/T", "/PID", str(old_pid)],
                    capture_output=True,
                    timeout=3,
                    **_WIN_NO_WINDOW_KWARGS,
                )
                killed += 1
        except Exception:
            pass
        try:
            pid_file.unlink(missing_ok=True)
        except Exception:
            pass

    # 2. Sweep any stray powershell.exe holding AudioPlaybackConnection or orphan python running audio.py
    ps_cleanup = f"""
$myPid = {my_pid}
$exPid = {int(exclude_pid or 0)}
$killed = 0
Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Where-Object {{
    $_.ProcessId -ne $PID -and $_.ProcessId -ne $myPid -and $_.ProcessId -ne $exPid -and (
        ($_.Name -match '^powershell(\\.exe)?$' -and $_.CommandLine -match 'AudioPlaybackConnection') -or
        ($_.Name -match '^pythonw?(\\.exe)?$' -and $_.CommandLine -match 'audio\\.py')
    )
}} | ForEach-Object {{
    try {{
        Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue
        $killed++
    }} catch {{}}
}}
Write-Output $killed
"""
    try:
        res = subprocess.run(
            ["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", ps_cleanup],
            capture_output=True,
            text=True,
            timeout=5,
            **_WIN_NO_WINDOW_KWARGS,
        )
        out = (res.stdout or "").strip().splitlines()
        if out and out[-1].strip().isdigit():
            killed += int(out[-1].strip())
    except Exception:
        pass
    return killed


class AudioManager:
    """Manages Windows 11 AudioPlaybackConnection (A2DP Sink) and Master Volume."""

    def __init__(self):
        self._enabled = False
        self._proc: Optional[subprocess.Popen] = None
        self._device_name = "iPhone"
        self._connection_state = "Closed"
        self._lock = threading.RLock()
        self._monitor_thread: Optional[threading.Thread] = None

    def is_active(self) -> bool:
        """Fast non-blocking check whether the A2DP audio worker process is alive."""
        proc = self._proc
        return bool(self._enabled and proc is not None and proc.poll() is None)

    def status(self) -> Dict[str, Any]:
        with self._lock:
            # Check if process is still alive
            if self._proc is not None:
                if self._proc.poll() is not None:
                    # Process died unexpectedly
                    self._proc = None
                    self._connection_state = "Closed"

            is_active = self._enabled and (self._proc is not None)

            return {
                "enabled": self._enabled,
                "active": is_active,
                "device": self._device_name,
                "state": self._connection_state if is_active else "Off",
                "volume": self.get_volume(),
            }

    def get_volume(self) -> int:
        try:
            import ctypes
            ctypes.windll.ole32.CoInitialize(None)
            try:
                from pycaw.pycaw import AudioUtilities
                dev = AudioUtilities.GetSpeakers()
                if dev and hasattr(dev, "EndpointVolume"):
                    return round(dev.EndpointVolume.GetMasterVolumeLevelScalar() * 100)
            finally:
                ctypes.windll.ole32.CoUninitialize()
        except Exception:
            pass
        return 100

    def set_volume(self, level_percent: int) -> Dict[str, Any]:
        vol = max(0, min(100, int(level_percent)))
        scalar = vol / 100.0
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
        with self._lock:
            if self._proc is not None and self._proc.poll() is None:
                self._enabled = True
                return self.status()

            # Pre-Start Orphan Audio Cleanup: ensure zero duplicate AudioPlaybackConnection sessions exist
            cleanup_orphan_audio_workers()

            ps_script = """
# iPhoneLink-AudioPlaybackWorker
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
    while ($true) { Start-Sleep -Seconds 2 }
} else {
    $dev = $devices[0]
    $conn = [Windows.Media.Audio.AudioPlaybackConnection]::TryCreateFromId($dev.Id)
    if ($conn -eq $null) {
        Write-Output '{"status": "error", "message": "Failed to create AudioPlaybackConnection"}'
        while ($true) { Start-Sleep -Seconds 2 }
    } else {
        $conn.Start()
        $null = $conn.Open()
        Write-Output ("{`"status`": `"active`", `"device`": `"" + $dev.Name + "`", `"state`": `"" + $conn.State.ToString() + "`"}")

        $running = $true
        while ($running) {
            $line = [Console]::In.ReadLine()
            if ($line -eq "stop" -or $line -eq $null) {
                $running = $false
            }
        }
        $conn.Dispose()
    }
}
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
                try:
                    _audio_pid_file().write_text(str(self._proc.pid), encoding="utf-8")
                except Exception:
                    pass

                # Read initial output line
                def read_init():
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

        try:
            _audio_pid_file().unlink(missing_ok=True)
        except Exception:
            pass
        cleanup_orphan_audio_workers()
        return self.status()

    def shutdown(self) -> None:
        self.disable()

    def stop(self) -> Dict[str, Any]:
        return self.disable()

    def toggle(self, enable: Optional[bool] = None) -> Dict[str, Any]:
        if enable is None:
            should_enable = not self._enabled
        else:
            should_enable = bool(enable)

        if should_enable:
            return self.enable()
        else:
            return self.disable()


# Global Singleton instance
audio_manager = AudioManager()
