"""Zero-Bluetooth AAC / AAC-ELD Native Decoder via LGPL zlaac.dll (Static FFmpeg 9.0.2).

Provides hardware-free, zero-dependency MPEG-4 AAC-LC & AAC-ELD decoding to
48 kHz stereo signed 16-bit little-endian PCM (s16le) on Windows using the
compact static LGPL library ``zlaac.dll`` (libavcodec + libavutil + libswresample).

Includes the critical 480-sample ``frameLengthFlag`` bit patch (``cookie[2] |= 0x10``)
required for Apple CoreDevice & AirPlay AAC-ELD streams.
"""

from __future__ import annotations

import ctypes
import threading
from pathlib import Path
from typing import Optional

from handset import get_app_root, get_bundle_root

_CAP_FRAMES = 8192
_CHANNELS = 2
_BYTES_PER_FRAME = 4  # stereo int16 (2 * 2 bytes)

# Standard CoreDevice / AirPlay AAC-ELD AudioSpecificConfig cookies
AAC_ELD_COOKIE_48K = b"\xf8\xe6\x40\x00"
AAC_ELD_COOKIE_DEFAULT = b"\xf8\xe8\x50\x00"

_LIB_LOCK = threading.Lock()
_LIB: Optional[ctypes.CDLL] = None
_LIB_PATH: Optional[Path] = None


class AacError(RuntimeError):
    """Raised when zlaac.dll cannot be loaded, opened, or fails to decode an AU."""


def find_zlaac_dll() -> Path:
    """Locate ``zlaac.dll`` across source tree and PyInstaller frozen layouts."""
    bundle_root = get_bundle_root()
    app_root = get_app_root()
    here = Path(__file__).resolve().parent

    candidates = [
        bundle_root / "handset" / "bin" / "zlaac.dll",
        bundle_root / "zlaac.dll",
        bundle_root.parent / "zlaac.dll",
        here / "bin" / "zlaac.dll",
        app_root / "handset" / "bin" / "zlaac.dll",
        app_root / "_internal" / "handset" / "bin" / "zlaac.dll",
        app_root / "_internal" / "zlaac.dll",
        app_root / "zlaac.dll",
        app_root.parent / "zlaac.dll",
    ]
    for cand in candidates:
        if cand.is_file():
            return cand
    searched = ", ".join(str(p) for p in candidates)
    raise AacError(f"zlaac.dll not found (searched: {searched})")


def _load_lib() -> ctypes.CDLL:
    """Load and bind C ABI signatures for ``zlaac.dll`` once per process."""
    global _LIB, _LIB_PATH
    if _LIB is not None:
        return _LIB
    with _LIB_LOCK:
        if _LIB is not None:
            return _LIB
        dll_path = find_zlaac_dll()
        try:
            lib = ctypes.CDLL(str(dll_path))
        except OSError as exc:
            raise AacError(f"Failed to load zlaac.dll from {dll_path}: {exc}") from exc

        lib.zl_aac_open.argtypes = (
            ctypes.c_char_p,
            ctypes.c_int,
            ctypes.c_int,
            ctypes.c_int,
            ctypes.c_int,
        )
        lib.zl_aac_open.restype = ctypes.c_void_p

        lib.zl_aac_decode.argtypes = (
            ctypes.c_void_p,
            ctypes.c_char_p,
            ctypes.c_int,
            ctypes.c_void_p,
            ctypes.c_int,
        )
        lib.zl_aac_decode.restype = ctypes.c_int

        lib.zl_aac_close.argtypes = (ctypes.c_void_p,)
        lib.zl_aac_close.restype = None

        lib.zl_aac_avcodec_version.argtypes = ()
        lib.zl_aac_avcodec_version.restype = ctypes.c_uint

        _LIB_PATH = dll_path
        _LIB = lib
        return lib


def avcodec_version() -> tuple[int, int, int]:
    """Return embedded FFmpeg libavcodec version tuple ``(major, minor, micro)``."""
    v = int(_load_lib().zl_aac_avcodec_version())
    return ((v >> 16) & 0xFFFF, (v >> 8) & 0xFF, v & 0xFF)


def patch_aac_eld_cookie(magic_cookie: bytes) -> bytes:
    """Ensure ELD ``frameLengthFlag`` (480 samples/AU) bit ``0x10`` is set on byte 2."""
    if not magic_cookie:
        magic_cookie = AAC_ELD_COOKIE_DEFAULT
    cookie = bytearray(magic_cookie)
    if len(cookie) >= 3:
        cookie[2] |= 0x10
    return bytes(cookie)


class AacDecoder:
    """Low-level wrapper over ``zlaac.dll`` producing interleaved s16le stereo PCM."""

    _CAP = _CAP_FRAMES

    def __init__(
        self,
        asc: bytes,
        sample_rate: int = 48000,
        channels: int = 2,
        out_rate: int = 48000,
    ) -> None:
        lib = _load_lib()
        asc_bytes = bytes(asc or b"")
        if not asc_bytes:
            raise AacError("AudioSpecificConfig (asc) must not be empty")

        self._lib = lib
        self.asc = asc_bytes
        self.sample_rate = int(sample_rate)
        self.channels = int(channels)
        self.out_rate = int(out_rate)
        self._buf = (ctypes.c_int16 * (8192 * 2))()
        self._lock = threading.Lock()
        self._h: Optional[int] = lib.zl_aac_open(
            asc_bytes,
            len(asc_bytes),
            self.sample_rate,
            self.channels,
            self.out_rate,
        )
        if not self._h:
            raise AacError(
                f"zl_aac_open rejected ASC {asc_bytes.hex()} "
                f"(sr={self.sample_rate}, ch={self.channels}, out={self.out_rate})"
            )

    def decode(self, au: bytes) -> bytes:
        """Decode a single raw AAC / AAC-ELD Access Unit into s16le stereo PCM bytes."""
        if not au:
            return b""
        au_bytes = bytes(au)
        with self._lock:
            if not self._h:
                raise AacError("AacDecoder is closed")
            n = int(
                self._lib.zl_aac_decode(
                    self._h,
                    au_bytes,
                    len(au_bytes),
                    self._buf,
                    self._CAP,
                )
            )
            if n < 0:
                raise AacError(f"zl_aac_decode failed with error code {n}")
            if n == 0:
                return b""
            return ctypes.string_at(self._buf, n * _BYTES_PER_FRAME)

    def close(self) -> None:
        """Release underlying native FFmpeg decoder & swresample context."""
        with self._lock:
            h = getattr(self, "_h", None)
            self._h = None
            if h and getattr(self, "_lib", None) is not None:
                self._lib.zl_aac_close(h)

    def __enter__(self) -> "AacDecoder":
        return self

    def __exit__(self, exc_type, exc, tb) -> None:
        self.close()

    def __del__(self) -> None:
        try:
            self.close()
        except Exception:
            pass


class AacEldDecoder:
    """Drop-in CoreDevice ``AACELDDecoder`` replacement using ``zlaac.dll``.

    Applies the mandatory 480-sample ``frameLengthFlag`` bit patch
    (``cookie[2] |= 0x10``) to Apple's ``AudioSpecificConfig`` cookie before
    initializing ``AacDecoder(..., 48000, 2, 48000)``.
    """

    _FRAMES = 480
    _PCM_BYTES = _FRAMES * _BYTES_PER_FRAME

    def __init__(self, magic_cookie: bytes = AAC_ELD_COOKIE_DEFAULT) -> None:
        raw_cookie = bytes(magic_cookie) if magic_cookie else AAC_ELD_COOKIE_DEFAULT
        cookie = bytearray(raw_cookie)
        if len(cookie) >= 3:
            cookie[2] |= 0x10
        self.raw_cookie = raw_cookie
        self.patched_cookie = bytes(cookie)
        self._dec = AacDecoder(self.patched_cookie, 48000, 2, 48000)

    def decode(self, au: bytes) -> bytes:
        """Decode one AAC-ELD Access Unit to 48 kHz s16le stereo PCM."""
        return self._dec.decode(au)

    def close(self) -> None:
        dec = getattr(self, "_dec", None)
        if dec is not None:
            dec.close()

    def __enter__(self) -> "AacEldDecoder":
        return self

    def __exit__(self, exc_type, exc, tb) -> None:
        self.close()

    def __del__(self) -> None:
        try:
            self.close()
        except Exception:
            pass
