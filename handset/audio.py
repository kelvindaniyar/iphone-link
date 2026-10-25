"""Audio Sink Manager for iPhone -> PC Audio Routing via Windows 11 Bluetooth A2DP."""

import json
import os
import subprocess
import sys
import threading
import time
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



class AudioManager:
    """Manages Windows 11 AudioPlaybackConnection (A2DP Sink) and Master Volume."""

    def __init__(self):
        self._enabled = False
        self._proc: Optional[subprocess.Popen] = None
        self._device_name = "iPhone"
        self._connection_state = "Closed"
        self._lock = threading.Lock()
        self._monitor_thread: Optional[threading.Thread] = None

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

            ps_script = """
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

                # Read initial output line
                def read_init():
                    try:
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

        return self.status()

    def toggle(self, enable: Optional[bool] = None) -> Dict[str, Any]:
        if enable is None:
            should_enable = not self._enabled
        else:
            should_enable = bool(enable)

        if should_enable:
            return self.enable()
        else:
            return self.disable()

    def shutdown(self):
        self.disable()


# Global Singleton instance
audio_manager = AudioManager()
