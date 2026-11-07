import sys
from pathlib import Path


def get_bundle_root() -> Path:
    """Return read-only bundle root (supports normal Python & PyInstaller _MEIPASS)."""
    if getattr(sys, "frozen", False):
        return Path(getattr(sys, "_MEIPASS", Path(sys.executable).resolve().parent))
    return Path(__file__).resolve().parent.parent


def get_app_root() -> Path:
    """Return writable application directory (folder of .exe when frozen, repo root otherwise)."""
    if getattr(sys, "frozen", False):
        return Path(sys.executable).resolve().parent
    return Path(__file__).resolve().parent.parent


# Tahap 58: Prefer native C extensions (pylzss / lzfse .pyd) at all times.
# Only append pure-Python _shims at the very end of sys.path if native C import genuinely raises ImportError.
try:
    import lzfse as _native_lzfse  # noqa: F401
    import lzss as _native_lzss  # noqa: F401
except ImportError:
    _shims_candidates = [
        get_bundle_root() / "handset" / "_shims",
        Path(__file__).resolve().parent / "_shims",
    ]
    for _cand in _shims_candidates:
        if _cand.is_dir() and str(_cand) not in sys.path:
            sys.path.append(str(_cand))
            break
