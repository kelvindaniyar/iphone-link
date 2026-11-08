"""Unified Audio Routing Manager for iPhone Link v3.0:
1. Primary (Default): Zero-Bluetooth USB Cable Audio Pipe (CoreDevice AAC-ELD -> zlaac.dll ->
   AdaptiveJitterBuffer -> PortAudio Windows WASAPI Shared Mode @ 48 kHz Stereo s16le).
2. Device-Only Mode ('device' / 'Hanya perangkat'): Immediately closes /audio.bin and
   tears down the CoreDevice audio session so sound plays on the physical iPhone speaker.
3. Optional Legacy Fallback ('bluetooth'): Windows 11 Bluetooth A2DP Sink via STA WinForms pump.
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
from handset.usb_audio import UsbCoreDeviceAudioEngine, normalize_buffer_preset

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

_AUDIO_OUT_ALIASES = {
    "pc": "pc",
    "usb": "pc",
    "this_pc": "pc",
    "di pc ini": "pc",
    "di pc ini (usb)": "pc",
    "device": "device",
    "phone": "device",
    "iphone": "device",
    "hanya perangkat": "device",
    "perangkat": "device",
    "bluetooth": "bluetooth",
    "bt": "bluetooth",
}


def normalize_audio_out(mode: Optional[str]) -> str:
    """Normalize output mode strings to ``'pc' | 'device' | 'bluetooth'``."""
    key = str(mode or "pc").strip().lower()
    return _AUDIO_OUT_ALIASES.get(key, "pc")


def _audio_pid_file() -> Path:
    return get_app_root() / ".audio_worker.pid"


def _resolve_preferred_iphone_name() -> str:
    """Resolve user's known iPhone DeviceName from RAM cache or wifi_state.json."""
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


# Legacy Bluetooth A2DP Sink STA Host Script (retained as optional 'bluetooth' fallback)
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

$targets = if ($appleCandidates.Count -gt 0) {
    $appleCandidates | Sort-Object -Property Score -Descending
} else {
    $fallbackCandidates | Sort-Object -Property Score -Descending
}

$Global:ActiveAudioConn = $null
$openedDevice = $null

foreach ($item in $targets) {
    $cand = $item.Device
    for ($attempt = 1; $attempt -le 5; $attempt++) {
        $conn = $null
        try {
            $conn = [Windows.Media.Audio.AudioPlaybackConnection]::TryCreateFromId($cand.Id)
        } catch {
            $conn = $null
        }
        if ($null -eq $conn) {
            Start-Sleep -Milliseconds 150
            continue
        }

        try {
            $conn.Start()
            if ($attempt -eq 1) {
                Start-Sleep -Milliseconds 120
            }
            $openOp = $conn.OpenAsync()
            $null = $asTaskOpen.Invoke($null, @($openOp)).GetAwaiter().GetResult()
        } catch {
            try { $null = $conn.Open() } catch {}
        }

        $stateStr = [string]$conn.State.ToString()
        if ($stateStr -eq 'Opened') {
            $Global:ActiveAudioConn = $conn
            $openedDevice = $cand
            break
        } else {
            try { $conn.Dispose() } catch {}
            $conn = $null
            if ($attempt -lt 5) {
                Start-Sleep -Milliseconds 350
            }
        }
    }
    if ($null -ne $Global:ActiveAudioConn) {
        break
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

$ctx = New-Object System.Windows.Forms.ApplicationContext
$marshaler = New-Object System.Windows.Forms.Control
$null = $marshaler.Handle

$reconnectTimer = New-Object System.Windows.Forms.Timer
$reconnectTimer.Interval = 2500
$reconnectTimer.add_Tick({
    if ($null -ne $Global:ActiveAudioConn) {
        try {
            if ($Global:ActiveAudioConn.State.ToString() -ne 'Opened') {
                try { $Global:ActiveAudioConn.Open() } catch {}
            }
        } catch {}
    }
})
$reconnectTimer.Start()

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

[System.Windows.Forms.Application]::Run($ctx)

try {
    if ($null -ne $reconnectTimer) {
        $reconnectTimer.Stop()
        $reconnectTimer.Dispose()
    }
} catch {}
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
    """Manages Zero-Bluetooth USB WASAPI Audio Pipe, Device-Only Switch, & Bluetooth Fallback."""

    def __init__(self) -> None:
        self._lock = threading.RLock()
        self._audio_out: str = "pc"  # Default: 'pc' (Di PC ini via USB)
        self._volume: int = 100
        self._muted: bool = False
        self._buffer_preset: str = "normal"
        self._lip_sync: bool = True

        self._screen_phase: str = "idle"
        self._screen_transport: str = "usb"

        self._usb_engine = UsbCoreDeviceAudioEngine(
            preset=self._buffer_preset,
            volume=self._volume,
            muted=self._muted,
        )

        # Legacy Bluetooth A2DP Sink fields
        self._bt_enabled = False
        self._proc: Optional[subprocess.Popen] = None
        self._device_name = "iPhone"
        self._connection_state = "Closed"

    @property
    def usb_engine(self) -> UsbCoreDeviceAudioEngine:
        return self._usb_engine

    def apply_saved_settings(self, settings: Dict[str, Any]) -> None:
        """Initialize preferences from user_settings.json without blocking."""
        if not isinstance(settings, dict):
            return
        with self._lock:
            if "audioOut" in settings and settings["audioOut"]:
                self._audio_out = normalize_audio_out(settings.get("audioOut"))
            elif settings.get("audioSinkEnabled") is True:
                self._audio_out = "bluetooth"
            if "audioVolume" in settings and settings["audioVolume"] is not None:
                try:
                    self._volume = max(0, min(100, int(settings["audioVolume"])))
                except Exception:
                    self._volume = 100
            if "audioMuted" in settings:
                self._muted = bool(settings.get("audioMuted"))
            if "audioBufferPreset" in settings and settings["audioBufferPreset"]:
                self._buffer_preset = normalize_buffer_preset(settings.get("audioBufferPreset"))
            if "lipSync" in settings:
                self._lip_sync = bool(settings.get("lipSync"))

            self._usb_engine.jitter.set_volume(self._volume)
            self._usb_engine.jitter.set_muted(self._muted)
            self._usb_engine.jitter.set_preset(self._buffer_preset)

    def on_screen_phase(self, phase: str, transport: str = "usb") -> None:
        """Automatically start or stop USB WASAPI audio when mirroring session changes state."""
        norm_phase = str(phase or "idle").strip().lower()
        norm_transport = "wifi" if str(transport or "usb").strip().lower() == "wifi" else "usb"

        with self._lock:
            self._screen_phase = norm_phase
            self._screen_transport = norm_transport
            mode = self._audio_out

        if norm_phase == "running" and mode == "pc":
            self._usb_engine.start(transport=norm_transport)
        elif norm_phase != "running":
            self._usb_engine.stop(release_coredevice=False)

    def is_active(self) -> bool:
        """Fast non-blocking check whether PC audio routing is enabled/active."""
        with self._lock:
            if self._audio_out == "pc":
                return True
            if self._audio_out == "bluetooth":
                proc = self._proc
                return bool(self._bt_enabled and proc is not None and proc.poll() is None)
            return False

    def status(self, refresh_volume: bool = False) -> Dict[str, Any]:
        with self._lock:
            if self._proc is not None and self._proc.poll() is not None:
                self._proc = None
                self._bt_enabled = False
                self._connection_state = "Closed"

            mode = self._audio_out
            bt_active = bool(self._bt_enabled and self._proc is not None)
            usb_streaming = bool(mode == "pc" and self._usb_engine.is_streaming)
            usb_running = bool(mode == "pc" and self._usb_engine.is_running)

            if mode == "pc":
                delay_ms = self._usb_engine.delay_ms if usb_streaming else self._usb_engine.estimated_preset_delay_ms
                enabled_flag = True
                active_flag = usb_streaming or usb_running
                dev_label = self._usb_engine.wasapi.device_name or "Windows WASAPI (48 kHz)"
                state_label = "Streaming" if usb_streaming else ("Ready" if self._screen_phase != "running" else "Connecting")
            elif mode == "bluetooth":
                delay_ms = 140.0 if bt_active else 0.0
                enabled_flag = bt_active
                active_flag = bt_active
                dev_label = self._device_name
                state_label = self._connection_state if bt_active else "Off"
            else:
                delay_ms = 0.0
                enabled_flag = False
                active_flag = False
                dev_label = "iPhone Speaker"
                state_label = "DeviceOnly"

            live_delay_ms = self._usb_engine.delay_ms if (mode == "pc" and usb_streaming) else (140.0 if (mode == "bluetooth" and bt_active) else 0.0)

            return {
                "audioOut": mode,
                "volume": self._volume,
                "muted": self._muted,
                "bufferPreset": self._buffer_preset,
                "lipSync": self._lip_sync,
                "streaming": bool(usb_streaming or bt_active),
                "delayMs": round(float(live_delay_ms), 1),
                "targetDelayMs": round(float(delay_ms), 1),
                "underruns": int(self._usb_engine.jitter.underruns),
                "wasapiDevice": self._usb_engine.wasapi.device_name,
                # Legacy keys for UI & tray backward compatibility
                "enabled": enabled_flag,
                "active": active_flag,
                "device": dev_label,
                "state": state_label,
            }

    def configure(
        self,
        *,
        audio_out: Optional[str] = None,
        volume: Optional[int] = None,
        muted: Optional[bool] = None,
        buffer_preset: Optional[str] = None,
        lip_sync: Optional[bool] = None,
        force_stream: Optional[bool] = None,
    ) -> Dict[str, Any]:
        """Update one or more audio parameters atomically and enforce routing immediately."""
        target_mode: Optional[str] = None
        with self._lock:
            if volume is not None:
                self._volume = self._usb_engine.jitter.set_volume(int(volume))
            if muted is not None:
                self._muted = self._usb_engine.jitter.set_muted(bool(muted))
            if buffer_preset is not None:
                self._buffer_preset = self._usb_engine.jitter.set_preset(str(buffer_preset))
            if lip_sync is not None:
                self._lip_sync = bool(lip_sync)
            if audio_out is not None:
                target_mode = normalize_audio_out(audio_out)
                self._audio_out = target_mode
            phase = self._screen_phase
            transport = self._screen_transport

        # Sync live screen phase if available
        if force_stream is True:
            phase = "running"
        elif force_stream is None:
            try:
                from handset.screen import screen_status

                scr = screen_status()
                phase = str(scr.get("phase") or phase)
                transport = "wifi" if str(scr.get("transport") or transport).lower() == "wifi" else "usb"
                with self._lock:
                    self._screen_phase = phase
                    self._screen_transport = transport
            except Exception:
                pass

        if target_mode == "device":
            self._usb_engine.stop(release_coredevice=True)
            self._disable_bt()
        elif target_mode == "pc":
            self._disable_bt()
            if phase == "running":
                self._usb_engine.start(transport=transport)
        elif target_mode == "bluetooth":
            self._usb_engine.stop(release_coredevice=True)
            self._enable_bt()
        else:
            # Ensure USB engine matches current phase if already in 'pc' mode
            with self._lock:
                cur_mode = self._audio_out
            if cur_mode == "pc" and phase == "running" and not self._usb_engine.is_running:
                self._usb_engine.start(transport=transport)

        return self.status()

    def set_audio_out(self, mode: str, *, force_stream: Optional[bool] = None) -> Dict[str, Any]:
        return self.configure(audio_out=mode, force_stream=force_stream)

    def set_buffer_preset(self, preset: str) -> Dict[str, Any]:
        return self.configure(buffer_preset=preset)

    def set_lip_sync(self, enabled: bool) -> Dict[str, Any]:
        return self.configure(lip_sync=bool(enabled))

    def set_muted(self, muted: bool) -> Dict[str, Any]:
        return self.configure(muted=bool(muted))

    def toggle_mute(self) -> Dict[str, Any]:
        with self._lock:
            next_muted = not self._muted
        return self.configure(muted=next_muted)

    def get_volume(self) -> int:
        with self._lock:
            return self._volume

    def set_volume(self, level_percent: int) -> Dict[str, Any]:
        vol = max(0, min(100, int(level_percent)))
        with self._lock:
            self._volume = self._usb_engine.jitter.set_volume(vol)
            mode = self._audio_out

        if mode == "bluetooth" and sys.platform == "win32":
            scalar = vol / 100.0
            try:
                import ctypes

                ctypes.windll.ole32.CoInitialize(None)
                try:
                    from pycaw.pycaw import AudioUtilities

                    dev = AudioUtilities.GetSpeakers()
                    if dev and hasattr(dev, "EndpointVolume"):
                        dev.EndpointVolume.SetMasterVolumeLevelScalar(scalar, None)
                finally:
                    ctypes.windll.ole32.CoUninitialize()
            except Exception:
                pass
        st = self.status()
        st["ok"] = True
        return st

    def open_bluetooth_settings(self) -> Dict[str, Any]:
        try:
            os.startfile("ms-settings:bluetooth")
            return {"ok": True}
        except Exception as e:
            return {"ok": False, "error": str(e)}

    def _enable_bt(self) -> Dict[str, Any]:
        with self._lock:
            if self._proc is not None and self._proc.poll() is None:
                self._bt_enabled = True
                return self.status()

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
                self._bt_enabled = True
                try:
                    _audio_pid_file().write_text(str(self._proc.pid), encoding="utf-8")
                except Exception:
                    pass

                init_status = {"status": "unknown"}

                def read_init() -> None:
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
                t.join(timeout=6.5)

                if init_status.get("status") in ("no_device", "not_connected", "error") or self._connection_state != "Opened":
                    self._bt_enabled = False
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
                print(f"AudioManager Bluetooth enable failed: {e}", flush=True)
                self._proc = None
                self._bt_enabled = False
        return self.status()

    def _disable_bt(self) -> None:
        with self._lock:
            self._bt_enabled = False
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

    def enable(self) -> Dict[str, Any]:
        return self.configure(audio_out="pc")

    def disable(self) -> Dict[str, Any]:
        return self.configure(audio_out="device")

    def shutdown(self) -> None:
        self._usb_engine.stop(release_coredevice=False)
        self._disable_bt()

    def stop(self) -> Dict[str, Any]:
        return self.disable()

    def toggle(self, enable: Optional[bool] = None) -> Dict[str, Any]:
        with self._lock:
            currently_pc = self._audio_out in ("pc", "bluetooth")
        should_enable = (not currently_pc) if enable is None else bool(enable)
        return self.configure(audio_out="pc" if should_enable else "device")


# Global Singleton instance
audio_manager = AudioManager()


def notify_screen_phase_changed(phase: str, transport: str = "usb") -> None:
    """Non-blocking hook invoked by handset.screen whenever mirroring phase transitions."""
    threading.Thread(
        target=audio_manager.on_screen_phase,
        args=(phase, transport),
        name="handset-audio-phase",
        daemon=True,
    ).start()
