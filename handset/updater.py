"""GitHub OTA Auto-Updater for iPhone Link.

Checks for releases on GitHub (kelvindaniyar/iphone-link), compares semantic
versions across stable and beta channels, downloads update packages,
protects user configuration files from being overwritten, and prepares
detached background installation.
"""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import threading
import time
from typing import Any
import urllib.error
import urllib.request

from handset import get_app_root

CURRENT_VERSION = "4.0.0"
GITHUB_REPO = "kelvindaniyar/iphone-link"
RELEASES_API_URL = "https://api.github.com/repos/kelvindaniyar/iphone-link/releases"

PROTECTED_CONFIG_FILENAMES = (
    "user_settings.json",
    "wifi_state.json",
    "typed_history.json",
    ".pane_webview_storage",
)

_STATE_LOCK = threading.Lock()
_UPDATE_STATE: dict[str, Any] = {
    "checking": False,
    "updateAvailable": False,
    "currentVersion": CURRENT_VERSION,
    "latestVersion": CURRENT_VERSION,
    "channel": "stable",
    "downloadUrl": "",
    "releaseUrl": "",
    "releaseNotes": "",
    "expectedSha256": "",
    "verifiedSha256": "",
    "offlineFallback": False,
    "protectedFiles": list(PROTECTED_CONFIG_FILENAMES),
    "progress": 0,
    "status": "up_to_date",  # up_to_date | checking | update_available | downloading | extracting | ready_to_restart | failed
    "message": "iPhone Link sudah versi terbaru.",
    "error": None,
}


def _parse_semver(tag_str: str) -> tuple:
    """Parse semver string into comparable tuple:
    ((major, minor, patch), is_stable, pre_level, pre_num)
    Example:
    "v2.0.0" -> ((2, 0, 0), 1, 0, 0)
    "v3.0.0" -> ((3, 0, 0), 1, 0, 0)
    "v3.1.0-beta.1" -> ((3, 1, 0), 0, 2, 1)
    "v3.1.0" -> ((3, 1, 0), 1, 0, 0)
    """
    if not tag_str:
        return ((0, 0, 0), 0, 0, 0)

    cleaned = str(tag_str).strip().lstrip("vV")
    main_part, _, pre_part = cleaned.partition("-")

    raw_nums = [int(x) for x in re.findall(r"\d+", main_part)]
    while len(raw_nums) < 3:
        raw_nums.append(0)
    core_ver = tuple(raw_nums[:3])

    if not pre_part:
        # Stable releases are ranked higher than pre-releases of the same core version
        return (core_ver, 1, 0, 0)

    pre_clean = pre_part.lower()
    pre_level = 0
    if "alpha" in pre_clean:
        pre_level = 1
    elif "beta" in pre_clean:
        pre_level = 2
    elif "rc" in pre_clean:
        pre_level = 3

    pre_nums = [int(x) for x in re.findall(r"\d+", pre_clean)]
    pre_num = pre_nums[0] if pre_nums else 0
    return (core_ver, 0, pre_level, pre_num)


def is_newer_version(current_version: str, candidate_version: str) -> bool:
    """Return True strictly when candidate_version is a newer Semver than current_version."""
    return _parse_semver(candidate_version) > _parse_semver(current_version)


def compute_sha256_file(file_path: Path) -> str:
    """Stream-compute lowercase SHA-256 hex digest of a file on disk."""
    h = hashlib.sha256()
    with open(file_path, "rb") as f:
        while True:
            chunk = f.read(65536)
            if not chunk:
                break
            h.update(chunk)
    return h.hexdigest()


def verify_sha256(data_or_path: bytes | bytearray | Path | str, expected_hex: str) -> bool:
    """Verify SHA-256 digest for bytes or a file path against expected hex."""
    expected = (expected_hex or "").strip().lower()
    if expected.startswith("sha256:"):
        expected = expected.split(":", 1)[1].strip()
    if not expected:
        return False
    if isinstance(data_or_path, (bytes, bytearray)):
        actual = hashlib.sha256(data_or_path).hexdigest().lower()
    else:
        actual = compute_sha256_file(Path(data_or_path)).lower()
    return actual == expected


def _extract_expected_sha256(release: dict[str, Any], chosen_asset: dict[str, Any] | None) -> str:
    """Extract SHA-256 hex digest from GitHub asset digest metadata or release body if present."""
    if isinstance(chosen_asset, dict):
        digest = str(chosen_asset.get("digest") or "").strip()
        if digest.lower().startswith("sha256:"):
            candidate = digest.split(":", 1)[1].strip().lower()
            if re.fullmatch(r"[0-9a-f]{64}", candidate):
                return candidate
        asset_name = str(chosen_asset.get("name") or "").strip()
    else:
        asset_name = ""

    body = str(release.get("body") or "")
    if asset_name:
        for line in body.splitlines():
            if asset_name.lower() in line.lower():
                m = re.search(r"\b([0-9a-fA-F]{64})\b", line)
                if m:
                    return m.group(1).lower()
    return ""


def _build_up_to_date_response(
    active_channel: str,
    *,
    offline_fallback: bool = False,
    error_detail: str | None = None,
) -> dict[str, Any]:
    """Update shared state and return a clean HTTP-200 up-to-date payload."""
    msg = "iPhone Link sudah versi terbaru."
    with _STATE_LOCK:
        _UPDATE_STATE["checking"] = False
        _UPDATE_STATE["updateAvailable"] = False
        _UPDATE_STATE["currentVersion"] = CURRENT_VERSION
        _UPDATE_STATE["latestVersion"] = CURRENT_VERSION
        _UPDATE_STATE["channel"] = active_channel
        _UPDATE_STATE["downloadUrl"] = ""
        _UPDATE_STATE["expectedSha256"] = ""
        _UPDATE_STATE["offlineFallback"] = offline_fallback
        _UPDATE_STATE["protectedFiles"] = list(PROTECTED_CONFIG_FILENAMES)
        _UPDATE_STATE["status"] = "up_to_date"
        _UPDATE_STATE["message"] = msg
        _UPDATE_STATE["error"] = error_detail
    return {
        "ok": True,
        "status": "up_to_date",
        "updateAvailable": False,
        "currentVersion": CURRENT_VERSION,
        "latestVersion": CURRENT_VERSION,
        "channel": active_channel,
        "offlineFallback": offline_fallback,
        "protectedFiles": list(PROTECTED_CONFIG_FILENAMES),
        "message": msg,
    }


def protect_user_files_in_staging(staging_dir: Path) -> list[str]:
    """Search and eliminate any user configuration file from the extracted staging folder
    so that user settings are never overwritten upon update.
    """
    removed: list[str] = []
    if not staging_dir.is_dir():
        return removed

    for root, dirs, files in os.walk(str(staging_dir), topdown=False):
        for f in files:
            if f.lower() in [p.lower() for p in PROTECTED_CONFIG_FILENAMES]:
                full_path = Path(root) / f
                try:
                    full_path.unlink()
                    removed.append(str(full_path))
                except Exception:
                    pass
        for d in dirs:
            if d.lower() in [p.lower() for p in PROTECTED_CONFIG_FILENAMES]:
                full_dir = Path(root) / d
                try:
                    shutil.rmtree(full_dir, ignore_errors=True)
                    removed.append(str(full_dir))
                except Exception:
                    pass
    return removed


def check_for_updates(
    channel: str = "stable",
    timeout: float = 6.0,
    mock_releases: list[dict] | None = None,
) -> dict[str, Any]:
    """Check for new releases on GitHub repository kelvindaniyar/iphone-link.
    Supports 'stable' and 'beta' channels with semver comparison.
    """
    active_channel = "beta" if str(channel).strip().lower() == "beta" else "stable"

    with _STATE_LOCK:
        _UPDATE_STATE["checking"] = True
        _UPDATE_STATE["channel"] = active_channel
        _UPDATE_STATE["status"] = "checking"
        _UPDATE_STATE["message"] = "Memeriksa pembaruan..."

    raw_releases: list[dict] = []
    if mock_releases is not None:
        raw_releases = mock_releases
    else:
        try:
            req = urllib.request.Request(
                RELEASES_API_URL,
                headers={
                    "Accept": "application/vnd.github+json",
                    "User-Agent": f"iPhone-Link-Updater/{CURRENT_VERSION}",
                },
            )
            with urllib.request.urlopen(req, timeout=timeout) as resp:
                data = resp.read()
                parsed = json.loads(data.decode("utf-8"))
                if isinstance(parsed, list):
                    raw_releases = parsed
        except urllib.error.HTTPError as he:
            # Handle 404, rate-limit, or empty repository releases cleanly with HTTP 200 offline fallback
            return _build_up_to_date_response(
                active_channel,
                offline_fallback=(he.code != 404),
                error_detail=None if he.code == 404 else f"HTTP {he.code}",
            )
        except Exception as e:
            return _build_up_to_date_response(
                active_channel,
                offline_fallback=True,
                error_detail=str(e),
            )

    # Filter releases by channel
    filtered: list[dict] = []
    for r in raw_releases:
        if not isinstance(r, dict):
            continue
        if r.get("draft"):
            continue
        is_prerelease = bool(r.get("prerelease"))
        if active_channel == "stable" and is_prerelease:
            continue
        filtered.append(r)

    # Sort releases in descending order of semver
    filtered.sort(
        key=lambda r: _parse_semver(r.get("tag_name") or r.get("name") or ""),
        reverse=True,
    )

    if not filtered:
        return _build_up_to_date_response(active_channel, offline_fallback=False)

    latest_rel = filtered[0]
    tag_name = str(latest_rel.get("tag_name") or latest_rel.get("name") or "")
    clean_version = tag_name.strip().lstrip("vV")

    if is_newer_version(CURRENT_VERSION, tag_name):
        # Search for download asset (.zip preferred, or .exe)
        assets = latest_rel.get("assets") or []
        asset_url = ""
        chosen_asset: dict[str, Any] | None = None
        zip_candidates = [
            a for a in assets
            if str(a.get("name", "")).lower().endswith(".zip")
        ]
        if zip_candidates:
            # Prefer standalone zip if multiple
            standalone = [
                a for a in zip_candidates
                if "standalone" in str(a.get("name", "")).lower() or "win" in str(a.get("name", "")).lower()
            ]
            chosen_asset = standalone[0] if standalone else zip_candidates[0]
            asset_url = chosen_asset.get("browser_download_url", "")
        elif assets:
            chosen_asset = assets[0]
            asset_url = chosen_asset.get("browser_download_url", "")
        else:
            asset_url = latest_rel.get("zipball_url") or latest_rel.get("html_url") or ""

        expected_sha256 = _extract_expected_sha256(latest_rel, chosen_asset)
        release_notes = str(latest_rel.get("body") or "").strip()
        if len(release_notes) > 400:
            release_notes = release_notes[:397] + "..."

        msg = f"Tersedia pembaruan: iPhone Link v{clean_version}"
        with _STATE_LOCK:
            _UPDATE_STATE["checking"] = False
            _UPDATE_STATE["updateAvailable"] = True
            _UPDATE_STATE["currentVersion"] = CURRENT_VERSION
            _UPDATE_STATE["latestVersion"] = clean_version
            _UPDATE_STATE["channel"] = active_channel
            _UPDATE_STATE["downloadUrl"] = asset_url
            _UPDATE_STATE["releaseUrl"] = latest_rel.get("html_url", "")
            _UPDATE_STATE["releaseNotes"] = release_notes
            _UPDATE_STATE["expectedSha256"] = expected_sha256
            _UPDATE_STATE["offlineFallback"] = False
            _UPDATE_STATE["protectedFiles"] = list(PROTECTED_CONFIG_FILENAMES)
            _UPDATE_STATE["status"] = "update_available"
            _UPDATE_STATE["message"] = msg
            _UPDATE_STATE["error"] = None

        return {
            "ok": True,
            "status": "update_available",
            "updateAvailable": True,
            "currentVersion": CURRENT_VERSION,
            "latestVersion": clean_version,
            "channel": active_channel,
            "downloadUrl": asset_url,
            "releaseUrl": latest_rel.get("html_url", ""),
            "releaseNotes": release_notes,
            "expectedSha256": expected_sha256,
            "offlineFallback": False,
            "protectedFiles": list(PROTECTED_CONFIG_FILENAMES),
            "message": msg,
        }

    # Already up to date
    return _build_up_to_date_response(active_channel, offline_fallback=False)


def _download_and_extract_worker(
    download_url: str,
    target_version: str,
    expected_sha256: str = "",
) -> None:
    """Worker thread that downloads, verifies SHA-256, extracts, and launches detached installer."""
    staging_dir = get_app_root() / "_update_staging"
    try:
        if not expected_sha256:
            with _STATE_LOCK:
                expected_sha256 = str(_UPDATE_STATE.get("expectedSha256") or "")

        if staging_dir.exists():
            shutil.rmtree(staging_dir, ignore_errors=True)
        staging_dir.mkdir(parents=True, exist_ok=True)

        zip_dest = staging_dir / "update.zip"

        with _STATE_LOCK:
            _UPDATE_STATE["status"] = "downloading"
            _UPDATE_STATE["progress"] = 5
            _UPDATE_STATE["message"] = f"Mengunduh iPhone Link v{target_version}..."

        # Download with progress tracking
        req = urllib.request.Request(
            download_url,
            headers={
                "User-Agent": f"iPhone-Link-Updater/{CURRENT_VERSION}",
                "Accept": "*/*",
            },
        )
        with urllib.request.urlopen(req, timeout=30.0) as resp:
            total_size = int(resp.headers.get("Content-Length") or 0)
            downloaded = 0
            with open(zip_dest, "wb") as out_f:
                while True:
                    chunk = resp.read(65536)
                    if not chunk:
                        break
                    out_f.write(chunk)
                    downloaded += len(chunk)
                    if total_size > 0:
                        pct = int((downloaded / total_size) * 75) + 5
                        with _STATE_LOCK:
                            _UPDATE_STATE["progress"] = min(80, max(5, pct))

        actual_sha256 = compute_sha256_file(zip_dest)
        if expected_sha256 and not verify_sha256(zip_dest, expected_sha256):
            try:
                zip_dest.unlink(missing_ok=True)
            except OSError:
                pass
            raise ValueError(
                f"Verifikasi SHA-256 paket pembaruan gagal (expected={expected_sha256}, actual={actual_sha256})"
            )

        with _STATE_LOCK:
            _UPDATE_STATE["verifiedSha256"] = actual_sha256
            _UPDATE_STATE["status"] = "extracting"
            _UPDATE_STATE["progress"] = 85
            _UPDATE_STATE["message"] = "Mengekstrak berkas pembaruan..."

        # Extract archive
        extracted_dir = staging_dir / "extracted"
        extracted_dir.mkdir(parents=True, exist_ok=True)
        shutil.unpack_archive(str(zip_dest), str(extracted_dir), "zip")

        # Find actual app payload inside extracted directory
        app_source_dir = extracted_dir
        subdirs = [d for d in extracted_dir.iterdir() if d.is_dir()]
        for sd in subdirs:
            if (sd / "iPhone Link.exe").is_file():
                app_source_dir = sd
                break

        # Protect user settings: eliminate any user configuration file from payload
        protect_user_files_in_staging(app_source_dir)

        # Write detached PowerShell updater script
        script_path = staging_dir / "apply_update.ps1"
        ps_script = """param(
    [int]$TargetPid,
    [string]$SourceDir,
    [string]$DestDir,
    [string]$ExeName
)

$ErrorActionPreference = 'SilentlyContinue'

try {
    $proc = Get-Process -Id $TargetPid -ErrorAction SilentlyContinue
    if ($proc) {
        $proc.WaitForExit(30000)
    }
} catch {}

Start-Sleep -Seconds 1

$excludeFiles = @('user_settings.json', 'wifi_state.json', 'typed_history.json', 'debug_metrics.json')
$excludeDirs = @('.pane_webview_storage', '_update_staging')

robocopy $SourceDir $DestDir /E /R:2 /W:1 /XF $excludeFiles /XD $excludeDirs /NP /NFL /NDL

Start-Sleep -Milliseconds 500

$exePath = Join-Path $DestDir $ExeName
if (Test-Path $exePath) {
    Start-Process -FilePath $exePath
}
"""
        script_path.write_text(ps_script, encoding="utf-8")

        with _STATE_LOCK:
            _UPDATE_STATE["status"] = "ready_to_restart"
            _UPDATE_STATE["progress"] = 100
            _UPDATE_STATE["message"] = "Pembaruan siap dipasang. Memulai ulang aplikasi..."

        # Launch detached PowerShell updater
        if sys.platform == "win32":
            DETACHED_PROCESS = 0x00000008
            CREATE_NO_WINDOW = 0x08000000
            cmd = [
                "powershell.exe",
                "-NoProfile",
                "-ExecutionPolicy", "Bypass",
                "-File", str(script_path),
                "-TargetPid", str(os.getpid()),
                "-SourceDir", str(app_source_dir),
                "-DestDir", str(get_app_root()),
                "-ExeName", "iPhone Link.exe",
            ]
            subprocess.Popen(cmd, creationflags=DETACHED_PROCESS | CREATE_NO_WINDOW)

    except Exception as e:
        with _STATE_LOCK:
            _UPDATE_STATE["status"] = "failed"
            _UPDATE_STATE["error"] = str(e)
            _UPDATE_STATE["message"] = f"Gagal mengunduh pembaruan: {e}"


def start_download_and_apply_update(
    download_url: str,
    target_version: str,
    expected_sha256: str = "",
) -> dict[str, Any]:
    """Initiate update download and staging in a non-blocking background thread."""
    if not download_url:
        return {"ok": False, "message": "URL unduhan pembaruan tidak ditemukan."}

    with _STATE_LOCK:
        if _UPDATE_STATE.get("status") in ("downloading", "extracting"):
            return {"ok": False, "message": "Proses unduhan sedang berjalan."}

    worker = threading.Thread(
        target=_download_and_extract_worker,
        args=(download_url, target_version or CURRENT_VERSION, expected_sha256),
        daemon=True,
    )
    worker.start()
    return {"ok": True, "message": "Mengunduh pembaruan di latar belakang..."}


def get_update_status() -> dict[str, Any]:
    """Retrieve current updater status merged with saved user settings."""
    from handset.server import load_user_settings
    with _STATE_LOCK:
        res = dict(_UPDATE_STATE)

    settings = load_user_settings(persist_instance=False)
    res["ok"] = True
    res["currentVersion"] = CURRENT_VERSION
    res["autoCheckUpdate"] = bool(settings.get("autoCheckUpdate", True))
    res["channel"] = str(settings.get("updateChannel", "stable"))
    res["protectedFiles"] = list(PROTECTED_CONFIG_FILENAMES)
    return res
