import importlib.util
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


if importlib.util.find_spec("lzss") is None or importlib.util.find_spec("lzfse") is None:
    _shims_candidates = [
        get_bundle_root() / "handset" / "_shims",
        Path(__file__).resolve().parent / "_shims",
    ]
    for _cand in _shims_candidates:
        if _cand.is_dir() and str(_cand) not in sys.path:
            sys.path.append(str(_cand))
            break
