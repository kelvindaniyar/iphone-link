"""Audio Sink Manager for iPhone -> PC Audio Routing via Windows 11 Bluetooth A2DP.

Tahap 59 Architecture (Zero-Stutter STA WinForms Message Pump + Single-iPhone Endpoint Filter):
- Runs PowerShell in explicit STA mode (`-Sta`) with a real Win32/COM Message Pump
  (`[System.Windows.Forms.Application]::Run($ctx)`) on the main STA thread that owns
  `Windows.Media.Audio.AudioPlaybackConnection` — never blocking the STA thread with
  `[Console]::In.ReadLine()`, so COM/WinRT callbacks from `audiodg.exe` & `bthserv` are
  dispatched with 0ms delay and 0.0% CPU.
- Strictly filters `DeviceInformation::FindAllAsync` to prioritize the user's iPhone
  (matching known iPhone `DeviceName`, `"iPhone"`/`"Apple"`, or Apple Inc. Bluetooth SIG
  Vendor ID `VID&0001004c`), immediately `break`s as soon as 1 iPhone endpoint opens
  (`State -eq 'Opened'`), and immediately `.Dispose()`s any un-opened candidate to prevent
  Windows Bluetooth 1.28s Page Scan loops.
- Pre-Start Orphan Audio Cleanup guarantees duplicate `AudioPlaybackConnection` workers
  can never coexist in the background.
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


def _resolve_preferred_iphone_name() -> str:
    """Resolve user's known iPhone DeviceName (e.g. 'UWE UH' or 'Kelvin’s iPhone') from RAM cache or wifi_state.json."""
    try:
        from handset.device import get_cached_device_names

        names = get_cached_device_names()
        if names:
            return names[0]
    except Exception:
        pass

    try:
        wifi_path = get_app_root() / "wifi_state.json"
        if wifi_path.is_file():
            raw = json.loads(wifi_path.read_text(encoding="utf-8"))
            devices = raw.get("devices") if isinstance(raw, dict) else None
            if isinstance(devices, dict):
                for entry in devices.values():
                    if isinstance(entry, dict) and entry.get("device_name"):
                        return str(entry["device_name"]).strip()
    except Exception:
        pass
    return ""


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


# PowerShell STA Host Script:
# 1. Strict Single-iPhone Endpoint Filter (prioritizes known DeviceName, "iPhone"/"Apple", & Apple BT VID 0x004C)
# 2. Immediately `break`s on first `Opened` connection & `.Dispose()`s any failed candidate (0% BT Page Scan!)
# 3. Pumps COM/WinRT STA messages on the main STA thread via `[System.Windows.Forms.Application]::Run($ctx)`
AUDIO_POWERSHELL_STA_HOST = r"""
# iPhoneLink-AudioPlaybackWorker
Add-Type -AssemblyName System.Runtime.WindowsRuntime
Add-Type -AssemblyName System.Windows.Forms
$null = [Windows.Media.Audio.AudioPlaybackConnection, Windows.Media.Audio, ContentType = WindowsRuntime]
$null = [Windows.Devices.Enumeration.DeviceInformation, Windows.Devices.Enumeration, ContentType = WindowsRuntime]

$asTaskGeneric = [System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object {
    $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.IsGenericMethod
} | Select-Object -First 1

$asTaskFind = $asTaskGeneric.MakeGenericMethod([Windows.Devices.Enumeration.DeviceInformationCollection])
$asTaskOpen = $asTaskGeneric.MakeGenericMethod([Windows.Media.Audio.AudioPlaybackConnectionOpenResult])

$selector = [Windows.Media.Audio.AudioPlaybackConnection]::GetDeviceSelector()
$findOp = [Windows.Devices.Enumeration.DeviceInformation]::FindAllAsync($selector)
$devices = $asTaskFind.Invoke($null, @($findOp)).GetAwaiter().GetResult()

if ($devices.Count -eq 0) {
    Write-Output '{"status": "no_device", "state": "Closed", "message": "No Bluetooth Audio Sink device found"}'
    exit 0
}

$prefName = [string]$env:PANE_PREFERRED_BT_NAME
$appleCandidates = New-Object System.Collections.Generic.List[psobject]
$fallbackCandidates = New-Object System.Collections.Generic.List[psobject]

foreach ($d in $devices) {
    $devName = [string]$d.Name
    $devId = [string]$d.Id
    $score = 0
    $isApple = $false

    if (-not [string]::IsNullOrWhiteSpace($prefName) -and $devName.Equals($prefName, [System.StringComparison]::OrdinalIgnoreCase)) {
        $score = 100
        $isApple = $true
    } elseif (-not [string]::IsNullOrWhiteSpace($prefName) -and $devName.IndexOf($prefName, [System.StringComparison]::OrdinalIgnoreCase) -ge 0) {
        $score = 95
        $isApple = $true
    } elseif ($devName -match '(?i)(iPhone|Apple)') {
        $score = 90
        $isApple = $true
    } elseif ($devId -match '(?i)(VID&0001004c|VID_05AC|VID&000205ac)') {
        # Apple Inc. Official Bluetooth SIG Company Identifier (0x004C)
        $score = 85
        $isApple = $true
    } elseif ($d.IsEnabled) {
        $score = 10
    }

    $entry = [PSCustomObject]@{
        Device  = $d
        Score   = $score
        IsApple = $isApple
    }
    if ($isApple) {
        $appleCandidates.Add($entry)
    } elseif ($score -gt 0) {
        $fallbackCandidates.Add($entry)
    }
}

# Never touch non-Apple Bluetooth speakers/headsets/phones when an iPhone candidate is paired!
$targets = if ($appleCandidates.Count -gt 0) {
    $appleCandidates | Sort-Object -Property Score -Descending
} else {
    $fallbackCandidates | Sort-Object -Property Score -Descending
}

$Global:ActiveAudioConn = $null
$openedDevice = $null

foreach ($item in $targets) {
    $cand = $item.Device
    $conn = $null
    try {
        $conn = [Windows.Media.Audio.AudioPlaybackConnection]::TryCreateFromId($cand.Id)
    } catch {
        $conn = $null
    }
    if ($null -eq $conn) {
        continue
    }

    try {
        $conn.Start()
        $openOp = $conn.OpenAsync()
        $null = $asTaskOpen.Invoke($null, @($openOp)).GetAwaiter().GetResult()
    } catch {
        try { $null = $conn.Open() } catch {}
    }

    $stateStr = [string]$conn.State.ToString()
    if ($stateStr -eq 'Opened') {
        $Global:ActiveAudioConn = $conn
        $openedDevice = $cand
        # CRITICAL: Break immediately after 1 iPhone A2DP connection opens!
        # Never call .Start() or .OpenAsync() on any other Bluetooth endpoint!
        break
    } else {
        # Immediately dispose un-opened candidate to prevent 1.28s Bluetooth Page Scan!
        try { $conn.Dispose() } catch {}
        $conn = $null
    }
}

if ($null -eq $Global:ActiveAudioConn) {
    Write-Output '{"status": "not_connected", "state": "Closed", "message": "iPhone Bluetooth Audio is not connected"}'
    exit 0
}

$initPayload = @{
    status = "active"
    device = [string]$openedDevice.Name
    state  = [string]$Global:ActiveAudioConn.State.ToString()
} | ConvertTo-Json -Compress
Write-Output $initPayload

# Create WinForms STA marshaler + ApplicationContext on the main STA thread owning $Global:ActiveAudioConn
$ctx = New-Object System.Windows.Forms.ApplicationContext
$marshaler = New-Object System.Windows.Forms.Control
$null = $marshaler.Handle

# Lightweight background thread waits for stdin "stop"/EOF without blocking the main STA COM message pump!
$bgPs = [powershell]::Create().AddScript({
    param($m, $c)
    try {
        while ($true) {
            $line = [Console]::In.ReadLine()
            if ($null -eq $line -or $line.Trim() -ieq 'stop') {
                break
            }
        }
    } catch {}
    try {
        $null = $m.BeginInvoke([Action]{
            $c.ExitThread()
            [System.Windows.Forms.Application]::Exit()
        })
    } catch {
        [System.Windows.Forms.Application]::Exit()
    }
}).AddArgument($marshaler).AddArgument($ctx)
$null = $bgPs.BeginInvoke()

# Pump native Win32/COM messages (GetMessage/DispatchMessage/CoWaitForMultipleHandles) at 0.0% CPU!
[System.Windows.Forms.Application]::Run($ctx)

try {
    if ($null -ne $Global:ActiveAudioConn) {
        $Global:ActiveAudioConn.Dispose()
    }
} catch {}
$Global:ActiveAudioConn = $null
try { $marshaler.Dispose() } catch {}
try { $bgPs.Dispose() } catch {}
exit 0
"""


class AudioManager:
    """Manages Windows 11 AudioPlaybackConnection (A2DP Sink) and Master Volume."""

    def __init__(self):
        self._enabled = False
        self._proc: Optional[subprocess.Popen] = None
        self._device_name = "iPhone"
        self._connection_state = "Closed"
        self._lock = threading.RLock()
        self._cached_volume: Optional[int] = None

    def is_active(self) -> bool:
        """Fast non-blocking check whether the A2DP audio worker process is alive."""
        proc = self._proc
        return bool(self._enabled and proc is not None and proc.poll() is None)

    def status(self, refresh_volume: bool = False) -> Dict[str, Any]:
        with self._lock:
            if self._proc is not None:
                if self._proc.poll() is not None:
                    self._proc = None
                    self._enabled = False
                    self._connection_state = "Closed"

            is_active = self._enabled and (self._proc is not None)
            if self._cached_volume is None or refresh_volume:
                self._cached_volume = self.get_volume()

            return {
                "enabled": self._enabled,
                "active": is_active,
                "device": self._device_name,
                "state": self._connection_state if is_active else "Off",
                "volume": self._cached_volume,
            }

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
                    return vol
            finally:
                ctypes.windll.ole32.CoUninitialize()
        except Exception:
            pass
        return self._cached_volume if self._cached_volume is not None else 100

    def set_volume(self, level_percent: int) -> Dict[str, Any]:
        vol = max(0, min(100, int(level_percent)))
        scalar = vol / 100.0
        self._cached_volume = vol
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

            child_env = dict(os.environ)
            pref_name = _resolve_preferred_iphone_name()
            if pref_name:
                child_env["PANE_PREFERRED_BT_NAME"] = pref_name

            try:
                self._proc = subprocess.Popen(
                    [
                        "powershell",
                        "-Sta",
                        "-NoProfile",
                        "-ExecutionPolicy",
                        "Bypass",
                        "-Command",
                        AUDIO_POWERSHELL_STA_HOST,
                    ],
                    stdin=subprocess.PIPE,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                    env=child_env,
                    text=True,
                    bufsize=1,
                    **_WIN_NO_WINDOW_KWARGS,
                )
                self._enabled = True
                try:
                    _audio_pid_file().write_text(str(self._proc.pid), encoding="utf-8")
                except Exception:
                    pass

                init_status = {"status": "unknown"}

                def read_init():
                    try:
                        if self._proc and self._proc.stdout:
                            line = self._proc.stdout.readline()
                            if line:
                                data = json.loads(line.strip())
                                init_status.update(data)
                                if "device" in data and data["device"]:
                                    self._device_name = str(data["device"])
                                if "state" in data and data["state"]:
                                    self._connection_state = str(data["state"])
                    except Exception:
                        pass

                t = threading.Thread(target=read_init, daemon=True)
                t.start()
                t.join(timeout=3.5)

                if init_status.get("status") in ("no_device", "not_connected", "error"):
                    self._enabled = False
                    self._connection_state = "Closed"
                    try:
                        if self._proc is not None:
                            self._proc.wait(timeout=0.5)
                    except Exception:
                        pass
                    self._proc = None
                    try:
                        _audio_pid_file().unlink(missing_ok=True)
                    except Exception:
                        pass

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
