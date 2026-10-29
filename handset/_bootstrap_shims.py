"""Pre-seed pure-Python lzss & lzfse shims ONLY when native C wheels are unavailable (ImportError).

Tahap 58: Always preserve native C extensions (`lzss.cp312-win_amd64.pyd` / `lzfse.cp312-win_amd64.pyd`)
on Python <= 3.12 or whenever already installed. Only activate fallback shims if importing raises ImportError
on Python 3.13+ environments without MSVC C++ Build Tools.
"""

from __future__ import annotations

import shutil
import site
import sys
from pathlib import Path


def main() -> int:
    try:
        import lzfse  # noqa: F401
        import lzss  # noqa: F401
        return 0
    except ImportError:
        pass

    if sys.version_info < (3, 13):
        return 0

    shims_dir = Path(__file__).resolve().parent / "_shims"
    if not shims_dir.is_dir():
        return 0
    site_dirs = [Path(p) for p in site.getsitepackages() if "site-packages" in p]
    if not site_dirs:
        return 0
    sp = site_dirs[0]
    sp.mkdir(parents=True, exist_ok=True)

    for mod_file, pkg_name, pkg_ver in (
        ("lzss.py", "pylzss", "0.3.4"),
        ("lzfse.py", "lzfse", "0.4.2.post1"),
    ):
        try:
            __import__(mod_file.removesuffix(".py"))
            continue
        except ImportError:
            pass
        src = shims_dir / mod_file
        if src.is_file() and not (sp / mod_file).exists():
            shutil.copy2(src, sp / mod_file)
        dist_info = sp / f"{pkg_name}-{pkg_ver}.dist-info"
        if not dist_info.exists():
            dist_info.mkdir(parents=True, exist_ok=True)
            (dist_info / "METADATA").write_text(
                f"Metadata-Version: 2.1\nName: {pkg_name}\nVersion: {pkg_ver}\n",
                encoding="utf-8",
            )
            (dist_info / "INSTALLER").write_text("pip\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
