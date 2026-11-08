"""Zero-Bluetooth USB Audio Engine: Adaptive Jitter Buffer + Windows WASAPI Output.

Streams decoded CoreDevice 48 kHz stereo s16le PCM from ``http://127.0.0.1:8766/audio.bin``
through a deterministic ``AdaptiveJitterBuffer`` directly into Windows 11 WASAPI Shared Mode
via PortAudio (``sounddevice`` + ``libportaudio64bit.dll``).
"""

from __future__ import annotations

import array
import http.client
import logging
import socket
import sys
import threading
import time
from typing import Any, Dict, Optional
from urllib.parse import urlparse

logger = logging.getLogger(__name__)

RATE = 48000
CHANNELS = 2
BYTES_PER_FRAME = 4  # stereo int16 (2 ch * 2 bytes)
MIN_MARGIN = 480     # 10 ms @ 48 kHz
CALM_S = 30.0        # 30.0 seconds calm window before -1 ms margin decay
DEFAULT_PACKET_FRAMES = 480  # 10 ms AAC-ELD Access Unit

PRESET_FLOORS: Dict[str, int] = {
    "low": 0,        # 0 ms floor  (initial margin 480 = 10 ms -> ~20 ms target)
    "normal": 1440,  # 30 ms floor (~40 ms target)
    "high": 3840,    # 80 ms floor (~90 ms target)
}

_PRESET_ALIASES: Dict[str, str] = {
    "low": "low",
    "rendah": "low",
    "normal": "normal",
    "sedang": "normal",
    "medium": "normal",
    "high": "high",
    "tinggi": "high",
}


def normalize_buffer_preset(preset: Optional[str]) -> str:
    """Normalize Indonesian or English buffer preset names to ``'low'|'normal'|'high'``."""
    key = str(preset or "normal").strip().lower()
    return _PRESET_ALIASES.get(key, "normal")


def scale_pcm_s16le(pcm: bytes, volume: int = 100, muted: bool = False) -> bytes:
    """Apply zero-click software volume scaling (0..100%) or mute to s16le stereo PCM."""
    aligned = len(pcm) & ~3
    if aligned <= 0:
        return b""
    if muted or volume <= 0:
        return b"\x00" * aligned
    if volume >= 100:
        return pcm if aligned == len(pcm) else pcm[:aligned]

    # Fixed-point Q15 linear amplitude scaling on native int16 array
    gain_q15 = int(round((max(0, min(100, int(volume))) / 100.0) * 32768.0))
    samples = array.array("h")
    samples.frombytes(pcm[:aligned])
    for idx, val in enumerate(samples):
        scaled = (val * gain_q15) >> 15
        if scaled > 32767:
            scaled = 32767
        elif scaled < -32768:
            scaled = -32768
        samples[idx] = scaled
    return samples.tobytes()


class AdaptiveJitterBuffer:
    """Bounded, drift-proof Adaptive Jitter Buffer for 48 kHz stereo s16le audio.

    Formulas (frames @ 48 kHz, 4 bytes/frame):
      - ``cap = max(4800, floor + 2400, 3 * block)``
      - ``target_frames = min(cap, packet + max(margin, floor, block))``
      - ``target_bytes = 4 * target_frames``
      - ``ceiling_bytes = 2 * target_bytes + 4 * max(128, block)``
      - Ceiling Trim: when ``len(_buf) + len(pcm) > ceiling_bytes``, drop oldest
        4-byte-aligned bytes so buffer returns cleanly to ``target_bytes``.
      - Underrun Adaptation: output silence, reset ``_started = False``, raise
        ``margin`` by ``+240`` frames (+5 ms USB) or ``+480`` frames (+10 ms Wi-Fi)
        capped at ``max(1440, floor + 2400, 2 * block)``.
      - Calm Decay: after ``30.0`` seconds without underruns, decay ``margin`` by
        ``48`` frames (-1 ms) down to ``max(480, floor)``.
    """

    RATE = RATE
    BYTES_PER_FRAME = BYTES_PER_FRAME
    MIN_MARGIN = MIN_MARGIN
    CALM_S = CALM_S

    def __init__(
        self,
        preset: str = "normal",
        *,
        transport: str = "usb",
        volume: int = 100,
        muted: bool = False,
        packet: int = DEFAULT_PACKET_FRAMES,
        block: int = DEFAULT_PACKET_FRAMES,
    ) -> None:
        self._lock = threading.Lock()
        self._buf = bytearray()
        self.packet = max(48, int(packet))
        self.block = max(48, int(block))
        self.transport = "wifi" if str(transport).lower() == "wifi" else "usb"
        self.preset = normalize_buffer_preset(preset)
        self.floor = PRESET_FLOORS[self.preset]
        self.margin = max(MIN_MARGIN, self.floor)
        self.volume = max(0, min(100, int(volume)))
        self.muted = bool(muted)

        self._started = False
        self._last_event_ts = time.monotonic()
        self.underruns = 0
        self.trimmed_frames = 0
        self.pushed_packets = 0

    @property
    def cap(self) -> int:
        return max(4800, self.floor + 2400, 3 * self.block)

    @property
    def target_frames(self) -> int:
        return min(self.cap, self.packet + max(self.margin, self.floor, self.block))

    @property
    def target_bytes(self) -> int:
        return BYTES_PER_FRAME * self.target_frames

    @property
    def ceiling_bytes(self) -> int:
        return 2 * self.target_bytes + BYTES_PER_FRAME * max(128, self.block)

    @property
    def buffered_bytes(self) -> int:
        with self._lock:
            return len(self._buf)

    @property
    def buffered_frames(self) -> int:
        with self._lock:
            return len(self._buf) // BYTES_PER_FRAME

    @property
    def started(self) -> bool:
        return self._started

    def set_preset(self, preset: str) -> str:
        norm = normalize_buffer_preset(preset)
        with self._lock:
            self.preset = norm
            self.floor = PRESET_FLOORS[norm]
            self.margin = max(MIN_MARGIN, self.floor)
            c_bytes = self.ceiling_bytes
            if len(self._buf) > c_bytes:
                drop = (len(self._buf) - self.target_bytes) & ~3
                if drop > 0:
                    del self._buf[:drop]
                    self.trimmed_frames += drop // BYTES_PER_FRAME
        return norm

    def set_transport(self, transport: str) -> None:
        with self._lock:
            self.transport = "wifi" if str(transport).lower() == "wifi" else "usb"

    def set_volume(self, volume: int) -> int:
        vol = max(0, min(100, int(volume)))
        with self._lock:
            self.volume = vol
        return vol

    def set_muted(self, muted: bool) -> bool:
        flag = bool(muted)
        with self._lock:
            self.muted = flag
        return flag

    def reset(self) -> None:
        with self._lock:
            self._buf.clear()
            self._started = False
            self.margin = max(MIN_MARGIN, self.floor)
            self._last_event_ts = time.monotonic()

    def push(self, pcm: bytes) -> int:
        """Push raw s16le stereo PCM bytes, applying volume/mute & Anti-Lag Ceiling Trim."""
        aligned = len(pcm) & ~3
        if aligned <= 0:
            return 0
        with self._lock:
            vol = self.volume
            muted = self.muted
        scaled = scale_pcm_s16le(pcm[:aligned], vol, muted)

        with self._lock:
            pkt_frames = aligned // BYTES_PER_FRAME
            if 48 <= pkt_frames <= 4096:
                self.packet = pkt_frames
            self._buf.extend(scaled)
            self.pushed_packets += 1

            c_bytes = self.ceiling_bytes
            if len(self._buf) > c_bytes:
                t_bytes = self.target_bytes
                drop = (len(self._buf) - t_bytes) & ~3
                if drop > 0:
                    del self._buf[:drop]
                    self.trimmed_frames += drop // BYTES_PER_FRAME
            return len(self._buf) // BYTES_PER_FRAME

    def pull(self, frames: int, now: Optional[float] = None) -> bytes:
        """Pull ``frames`` stereo s16le frames for WASAPI callback (never blocks OS audio thread)."""
        if frames <= 0:
            return b""
        need = frames * BYTES_PER_FRAME
        now_t = time.monotonic() if now is None else float(now)

        with self._lock:
            self.block = max(48, int(frames))
            if not self._started:
                if len(self._buf) < self.target_bytes:
                    return b"\x00" * need
                self._started = True
                self._last_event_ts = now_t

            if len(self._buf) < need:
                # Underrun after active start: output silence, wait for clean prefill, raise margin
                self._started = False
                self.underruns += 1
                step = 480 if self.transport == "wifi" else 240
                max_margin = max(1440, self.floor + 2400, 2 * self.block)
                self.margin = min(max_margin, self.margin + step)
                self._last_event_ts = now_t
                return b"\x00" * need

            out = bytes(self._buf[:need])
            del self._buf[:need]

            if now_t - self._last_event_ts >= CALM_S:
                min_m = max(MIN_MARGIN, self.floor)
                if self.margin > min_m:
                    self.margin = max(min_m, self.margin - 48)
                self._last_event_ts = now_t
            return out


def find_wasapi_default_output_device() -> tuple[Optional[int], str]:
    """Return ``(device_index, device_name)`` for the Windows WASAPI default output endpoint."""
    import sounddevice as sd

    hostapis = sd.query_hostapis()
    devices = sd.query_devices()
    wasapi_api_idx: Optional[int] = None

    for idx, api in enumerate(hostapis):
        name = str(api.get("name", ""))
        if "WASAPI" in name.upper():
            wasapi_api_idx = idx
            default_out = int(api.get("default_output_device", -1))
            if 0 <= default_out < len(devices):
                dev = devices[default_out]
                if int(dev.get("max_output_channels", 0)) >= 1:
                    return default_out, str(dev.get("name", "Windows WASAPI Output"))

    if wasapi_api_idx is not None:
        for idx, dev in enumerate(devices):
            if int(dev.get("hostapi", -1)) == wasapi_api_idx and int(dev.get("max_output_channels", 0)) >= 2:
                return idx, str(dev.get("name", "Windows WASAPI Output"))

    default_pair = sd.default.device
    default_out = default_pair[1] if isinstance(default_pair, (list, tuple)) and len(default_pair) >= 2 else -1
    if isinstance(default_out, int) and 0 <= default_out < len(devices):
        return default_out, str(devices[default_out].get("name", "Default Output"))
    return None, "Unavailable"


def _ensure_thread_com_initialized() -> None:
    """Ensure Windows COM is initialized on the current Python thread before opening WASAPI."""
    if sys.platform != "win32":
        return
    try:
        import ctypes

        # COINIT_MULTITHREADED (0x0) works cleanly for PortAudio WASAPI IAudioClient;
        # RPC_E_CHANGED_MODE (0x80010106) means the thread already initialized STA, which is also valid.
        hr = ctypes.windll.ole32.CoInitializeEx(None, 0)
        if hr not in (0, 1, -2147417850, 0x80010106):
            ctypes.windll.ole32.CoInitialize(None)
    except Exception:
        pass


class WasapiAudioOutput:
    """Low-latency PortAudio Windows WASAPI Shared-Mode output stream wrapper."""

    def __init__(self, jitter: AdaptiveJitterBuffer) -> None:
        self.jitter = jitter
        self._lock = threading.Lock()
        self._stream: Any = None
        self.device_index: Optional[int] = None
        self.device_name: str = "Windows WASAPI"
        self.latency_ms: float = 10.0
        self.active: bool = False
        self.last_error: Optional[str] = None

    def _callback(self, outdata, frames: int, time_info, status) -> None:
        try:
            outdata[:] = self.jitter.pull(frames)
        except Exception:
            outdata[:] = b"\x00" * (frames * BYTES_PER_FRAME)

    def start(self) -> bool:
        _ensure_thread_com_initialized()
        with self._lock:
            if self._stream is not None and self.active:
                return True
            self._close_locked()
            try:
                import sounddevice as sd

                dev_idx, dev_name = find_wasapi_default_output_device()
                if dev_idx is None:
                    self.last_error = "No Windows WASAPI output device available"
                    return False

                extra = None
                if hasattr(sd, "WasapiSettings"):
                    try:
                        extra = sd.WasapiSettings(auto_convert=True)
                    except TypeError:
                        extra = sd.WasapiSettings()

                stream = sd.RawOutputStream(
                    samplerate=RATE,
                    blocksize=0,
                    device=dev_idx,
                    channels=CHANNELS,
                    dtype="int16",
                    latency="low",
                    callback=self._callback,
                    extra_settings=extra,
                )
                stream.start()
                self._stream = stream
                self.device_index = dev_idx
                self.device_name = dev_name
                raw_lat = getattr(stream, "latency", 0.010)
                if isinstance(raw_lat, (tuple, list)):
                    raw_lat = raw_lat[-1]
                self.latency_ms = round(max(2.0, float(raw_lat or 0.010) * 1000.0), 1)
                self.active = True
                self.last_error = None
                return True
            except Exception as exc:
                self.last_error = str(exc)
                logger.warning("WASAPI output stream failed to open: %s", exc)
                self._close_locked()
                return False

    def _close_locked(self) -> None:
        stream = self._stream
        self._stream = None
        self.active = False
        if stream is not None:
            try:
                stream.stop()
            except Exception:
                pass
            try:
                stream.close()
            except Exception:
                pass

    def stop(self) -> None:
        with self._lock:
            self._close_locked()


class UsbCoreDeviceAudioEngine:
    """Streams ``[uint32_be byte_len][PCM s16le 48 kHz]`` from ``/audio.bin`` to WASAPI."""

    DEFAULT_URL = "http://127.0.0.1:8766/audio.bin"

    def __init__(
        self,
        *,
        audio_bin_url: str = DEFAULT_URL,
        preset: str = "normal",
        volume: int = 100,
        muted: bool = False,
    ) -> None:
        self._lock = threading.RLock()
        self.audio_bin_url = audio_bin_url
        self.jitter = AdaptiveJitterBuffer(preset=preset, volume=volume, muted=muted)
        self.wasapi = WasapiAudioOutput(self.jitter)
        self._running = False
        self._streaming = False
        self._stop_evt = threading.Event()
        self._thread: Optional[threading.Thread] = None
        self._active_conn: Optional[http.client.HTTPConnection] = None
        self.last_error: Optional[str] = None
        self.bytes_received: int = 0

    @property
    def is_running(self) -> bool:
        return self._running

    @property
    def is_streaming(self) -> bool:
        return bool(self._running and self._streaming)

    @property
    def delay_ms(self) -> float:
        if not self._running:
            return 0.0
        buffered = self.jitter.buffered_frames
        if not self._streaming and buffered == 0:
            return 0.0
        wasapi_lat = self.wasapi.latency_ms if self.wasapi.active else 10.0
        return round((buffered / 48.0) + wasapi_lat, 1)

    @property
    def estimated_preset_delay_ms(self) -> float:
        wasapi_lat = self.wasapi.latency_ms if self.wasapi.active else 10.0
        return round((self.jitter.target_frames / 48.0) + wasapi_lat, 1)

    def set_audio_bin_url(self, url: str) -> None:
        with self._lock:
            self.audio_bin_url = str(url or self.DEFAULT_URL)

    def start(self, transport: str = "usb") -> bool:
        with self._lock:
            self.jitter.set_transport(transport)
            if self._running and self._thread is not None and self._thread.is_alive():
                return True
            self.stop(release_coredevice=False)
            self._stop_evt.clear()
            self.jitter.reset()
            self.wasapi.start()
            self._running = True
            self._streaming = False
            t = threading.Thread(
                target=self._reader_loop,
                name="handset-usb-audio",
                daemon=True,
            )
            self._thread = t
            t.start()
            return True

    def _abort_active_conn(self) -> None:
        with self._lock:
            conn = self._active_conn
            self._active_conn = None
        if conn is not None:
            sock = getattr(conn, "sock", None)
            if sock is not None:
                try:
                    sock.shutdown(socket.SHUT_RDWR)
                except Exception:
                    pass
                try:
                    sock.close()
                except Exception:
                    pass
            try:
                conn.close()
            except Exception:
                pass

    def stop(self, *, release_coredevice: bool = True) -> None:
        with self._lock:
            was_running = self._running
            self._running = False
            self._streaming = False
            self._stop_evt.set()
            thread = self._thread
            self._thread = None

        self._abort_active_conn()
        if thread is not None and thread.is_alive() and thread is not threading.current_thread():
            thread.join(timeout=1.2)

        self.wasapi.stop()
        self.jitter.reset()

        if release_coredevice and was_running:
            try:
                from handset.screen import stop_coredevice_audio_stream

                stop_coredevice_audio_stream(timeout=1.2)
            except Exception:
                pass

    def _reader_loop(self) -> None:
        # Small initial grace period if ScreenStreamServer is finishing H.265 handshake
        backoff = 0.25
        while not self._stop_evt.is_set():
            parsed = urlparse(self.audio_bin_url)
            host = parsed.hostname or "127.0.0.1"
            port = parsed.port or 8766
            path = parsed.path or "/audio.bin"
            if parsed.query:
                path = f"{path}?{parsed.query}"

            conn: Optional[http.client.HTTPConnection] = None
            try:
                conn = http.client.HTTPConnection(host, port, timeout=5.0)
                with self._lock:
                    if self._stop_evt.is_set():
                        conn.close()
                        break
                    self._active_conn = conn

                conn.request("GET", path, headers={"Cache-Control": "no-cache"})
                resp = conn.getresponse()
                if resp.status != 200:
                    body = resp.read(256)
                    self.last_error = f"HTTP {resp.status}: {body.decode('utf-8', 'ignore').strip()}"
                    self._streaming = False
                    conn.close()
                    if self._stop_evt.wait(backoff):
                        break
                    backoff = min(1.5, backoff * 1.5)
                    continue

                if not self.wasapi.active:
                    self.wasapi.start()

                self._streaming = True
                self.last_error = None
                backoff = 0.25
                buf = bytearray()

                while not self._stop_evt.is_set():
                    chunk = resp.read1(65536) if hasattr(resp, "read1") else resp.read(4096)
                    if not chunk:
                        break
                    self.bytes_received += len(chunk)
                    buf.extend(chunk)
                    while len(buf) >= 4:
                        frame_len = int.from_bytes(buf[:4], "big")
                        if frame_len <= 0 or frame_len > 262144:
                            # Resync on corrupt frame length
                            del buf[:4]
                            continue
                        if len(buf) < 4 + frame_len:
                            break
                        pcm = bytes(buf[4 : 4 + frame_len])
                        del buf[: 4 + frame_len]
                        self.jitter.push(pcm)
            except Exception as exc:
                if not self._stop_evt.is_set():
                    self.last_error = str(exc)
            finally:
                self._streaming = False
                with self._lock:
                    if self._active_conn is conn:
                        self._active_conn = None
                if conn is not None:
                    try:
                        conn.close()
                    except Exception:
                        pass

            if self._stop_evt.is_set():
                break
            if self._stop_evt.wait(backoff):
                break
            backoff = min(1.5, backoff * 1.4)
