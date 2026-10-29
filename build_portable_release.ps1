# ======================================================================
# iPhone Link - Portable & Standalone EXE Release Packaging Script
# Creates two clean release distributions ready for GitHub Releases:
#   1) release/iPhone-Link-Portable-v1.0.0.zip (Smart Auto-Bootstrap .bat)
#   2) release/iPhone-Link-Win64-Standalone-EXE-v1.0.0.zip (Standalone .exe)
# ======================================================================

param (
    [string]$Version = "v1.0.0",
    [switch]$SkipExe = $false
)

$ErrorActionPreference = "Stop"
$RootDir = $PSScriptRoot
if (-not $RootDir) { $RootDir = (Get-Location).Path }

Write-Host "======================================================================" -ForegroundColor Cyan
Write-Host "   BUILDING IPHONE LINK RELEASE PACKAGES ($Version)" -ForegroundColor Cyan
Write-Host "======================================================================" -ForegroundColor Cyan

$ReleaseBase = Join-Path $RootDir "release"
New-Item -ItemType Directory -Force -Path $ReleaseBase | Out-Null

$CoreFiles = @(
    "iPhone Link.bat",
    "Create-Desktop-Shortcut.bat",
    "Uninstall-iPhone-Link.bat",
    "run.bat",
    "README.md",
    "LICENSE",
    "requirements.txt",
    "pyproject.toml"
)

$PrivatePatterns = @(
    "debug_ocr_*",
    "debug_metrics.json",
    "typed_history.json",
    "user_settings.json",
    "wifi_state.json",
    ".pane_webview_storage",
    "*.pairrecord",
    "scratch",
    "LAPORAN_*.md"
)

function Invoke-PrivacySanityCheck {
    param([string]$ScanDir)
    $LeakCount = 0
    foreach ($p in $PrivatePatterns) {
        $leaks = Get-ChildItem -Path $ScanDir -Recurse -Filter $p -ErrorAction SilentlyContinue
        if ($leaks) {
            foreach ($l in $leaks) {
                Write-Host "      [WARNING] Removing leaked item: $($l.FullName)" -ForegroundColor Red
                Remove-Item -Recurse -Force $l.FullName
                $LeakCount++
            }
        }
    }
    if ($LeakCount -eq 0) {
        Write-Host "      [OK] 100% CLEAN: Zero personal PINs, debug dumps, or metrics found!" -ForegroundColor Green
    }
}

# ======================================================================
# PACKAGE 1: Smart Auto-Bootstrapping Portable ZIP
# ======================================================================
$PackageName = "iPhone-Link-Portable-$Version"
$TargetDir = Join-Path $ReleaseBase $PackageName
$ZipOutput = Join-Path $ReleaseBase "$PackageName.zip"

if (Test-Path $TargetDir) {
    Write-Host "[1/6] Cleaning existing portable folder: $TargetDir" -ForegroundColor Yellow
    Remove-Item -Recurse -Force $TargetDir
}
if (Test-Path $ZipOutput) {
    Remove-Item -Force $ZipOutput
}

New-Item -ItemType Directory -Force -Path $TargetDir | Out-Null
Write-Host "[2/6] Assembling Package 1 (Smart Auto-Bootstrap Portable)..." -ForegroundColor Green

$HandsetTarget = Join-Path $TargetDir "handset"
Copy-Item -Recurse (Join-Path $RootDir "handset") $HandsetTarget
Get-ChildItem -Path $HandsetTarget -Recurse -Filter "__pycache__" | Remove-Item -Recurse -Force
Get-ChildItem -Path $HandsetTarget -Recurse -Include "*.pyc", "*.pyo" | Remove-Item -Force

Copy-Item -Recurse (Join-Path $RootDir "assets") (Join-Path $TargetDir "assets")

foreach ($f in $CoreFiles) {
    $src = Join-Path $RootDir $f
    if (Test-Path $src) {
        Copy-Item -Force $src (Join-Path $TargetDir $f)
    }
}

Write-Host "[3/6] Running strict privacy sanity check on Package 1..." -ForegroundColor Green
Invoke-PrivacySanityCheck -ScanDir $TargetDir

Write-Host "[4/6] Compressing Package 1 -> $ZipOutput..." -ForegroundColor Green
Compress-Archive -Path $TargetDir -DestinationPath $ZipOutput -CompressionLevel Optimal
$ZipItem = Get-Item $ZipOutput
$PortableSizeMB = [math]::Round($ZipItem.Length / 1048576, 2)

# ======================================================================
# PACKAGE 2: Standalone Windows Executable (iPhone Link.exe)
# ======================================================================
$ExePackageName = "iPhone-Link-Win64-Standalone-EXE-$Version"
$ExeTargetDir = Join-Path $ReleaseBase $ExePackageName
$ExeZipOutput = Join-Path $ReleaseBase "$ExePackageName.zip"

if (-not $SkipExe) {
    Write-Host "[5/6] Building Package 2 (Standalone iPhone Link.exe via PyInstaller)..." -ForegroundColor Green
    $VenvPy = Join-Path $RootDir "venv\Scripts\python.exe"
    if (-not (Test-Path $VenvPy)) {
        throw "Local venv Python not found at $VenvPy. Create venv first before building Standalone EXE."
    }

    & $VenvPy -c "import PyInstaller" 2>$null
    if ($LASTEXITCODE -ne 0) {
        Write-Host "      Installing PyInstaller into local venv..." -ForegroundColor Yellow
        & $VenvPy -m pip install pyinstaller --quiet
    }

    $SpecPath = Join-Path $RootDir "packaging\iphone_link.spec"
    $BuildWorkDir = Join-Path $RootDir "build\pyinstaller_work"
    $BuildDistDir = Join-Path $RootDir "build\pyinstaller_dist"
    if (Test-Path $BuildDistDir) { Remove-Item -Recurse -Force $BuildDistDir }
    New-Item -ItemType Directory -Force -Path $BuildWorkDir | Out-Null
    New-Item -ItemType Directory -Force -Path $BuildDistDir | Out-Null

    if (Test-Path $SpecPath) {
        & $VenvPy -m PyInstaller --noconfirm --clean --workpath $BuildWorkDir --distpath $BuildDistDir $SpecPath
    }
    else {
        $IconArg = Join-Path $RootDir "assets\iphone_link.ico"
        & $VenvPy -m PyInstaller --noconfirm --clean --onedir --windowed `
            --name "iPhone Link" `
            --icon $IconArg `
            --workpath $BuildWorkDir `
            --distpath $BuildDistDir `
            --paths $RootDir `
            --paths (Join-Path $RootDir "handset\_shims") `
            --add-data "$(Join-Path $RootDir 'handset\web');handset/web" `
            --add-data "$(Join-Path $RootDir 'handset\ocr_extractor.ps1');handset" `
            --add-data "$(Join-Path $RootDir 'handset\_shims');handset/_shims" `
            --add-data "$(Join-Path $RootDir 'assets');assets" `
            --collect-all webview `
            --collect-all pythonnet `
            --collect-all clr_loader `
            --collect-all pycaw `
            --collect-all comtypes `
            --collect-all pymobiledevice3 `
            --collect-all pmd_pytcp `
            --collect-all pytun_pmd3 `
            --collect-all qh3 `
            --collect-all pyimg4 `
            --collect-all ipsw_parser `
            --collect-all developer_disk_image `
            --collect-all opack2 `
            --collect-all bpylist2 `
            --collect-all construct `
            --collect-all srptools `
            --collect-all ifaddr `
            --exclude-module tkinter `
            --exclude-module matplotlib `
            --exclude-module IPython `
            --exclude-module jedi `
            --exclude-module pytest `
            --exclude-module av `
        (Join-Path $RootDir "handset\gui.py")
    }

    if ($LASTEXITCODE -ne 0) {
        throw "PyInstaller build failed with exit code $LASTEXITCODE"
    }

    $BuiltDir = Join-Path $BuildDistDir "iPhone Link"
    $BuiltExe = Join-Path $BuiltDir "iPhone Link.exe"
    if (-not (Test-Path $BuiltExe)) {
        throw "Standalone executable was not generated at $BuiltExe"
    }

    if (Test-Path $ExeTargetDir) { Remove-Item -Recurse -Force $ExeTargetDir }
    if (Test-Path $ExeZipOutput) { Remove-Item -Force $ExeZipOutput }
    New-Item -ItemType Directory -Force -Path $ExeTargetDir | Out-Null

    Copy-Item -Recurse -Force (Join-Path $BuiltDir "*") $ExeTargetDir
    Copy-Item -Recurse -Force (Join-Path $RootDir "assets") (Join-Path $ExeTargetDir "assets")

    $ExeCompanionFiles = @(
        "iPhone Link.bat",
        "Create-Desktop-Shortcut.bat",
        "Uninstall-iPhone-Link.bat",
        "run.bat",
        "README.md",
        "LICENSE"
    )
    foreach ($f in $ExeCompanionFiles) {
        $src = Join-Path $RootDir $f
        if (Test-Path $src) {
            Copy-Item -Force $src (Join-Path $ExeTargetDir $f)
        }
    }

    Invoke-PrivacySanityCheck -ScanDir $ExeTargetDir

    Write-Host "[6/6] Compressing Package 2 -> $ExeZipOutput..." -ForegroundColor Green
    & $VenvPy -c "import zipfile, pathlib, sys; src = pathlib.Path(sys.argv[1]); zpath = pathlib.Path(sys.argv[2]); zf = zipfile.ZipFile(zpath, 'w', zipfile.ZIP_DEFLATED, compresslevel=6); [zf.write(f, str(f.relative_to(src.parent))) for f in src.rglob('*') if f.is_file()]; zf.close()" $ExeTargetDir $ExeZipOutput
    $ExeZipItem = Get-Item $ExeZipOutput
    $ExeSizeMB = [math]::Round($ExeZipItem.Length / 1048576, 2)
}

# Tahap 55: Zero-Secret Archive & Binary Audit + Legacy Global WebView2 Cache Cleanup
$AuditPy = Join-Path $RootDir "venv\Scripts\python.exe"
if (Test-Path $AuditPy) {
    & $AuditPy -c "
import zipfile, pathlib, sys
forbidden = ('user_settings.json', 'typed_history.json', 'wifi_state.json', 'debug_metrics.json', '.pane_webview_storage', '.pairrecord')
for zp_str in sys.argv[1:]:
    zp = pathlib.Path(zp_str)
    if not zp.is_file(): continue
    with zipfile.ZipFile(zp, 'r') as zf:
        for name in zf.namelist():
            low = name.lower()
            for f in forbidden:
                if f in low:
                    raise SystemExit(f'SECURITY FAIL: Forbidden artifact {name} found in {zp.name}')
print('      [AUDIT OK] 0% private files or PIN artifacts inside release ZIP archives!')
" $ZipOutput $ExeZipOutput
    if ($LASTEXITCODE -ne 0) { throw "Release archive zero-secret audit failed!" }
}

$RunningGui = Get-Process -Name "iPhone Link" -ErrorAction SilentlyContinue
if (-not $RunningGui) {
    foreach ($legacyCache in @(
        (Join-Path $HOME ".pane_webview_storage"),
        (Join-Path $env:APPDATA "pywebview"),
        (Join-Path $env:LOCALAPPDATA "pywebview")
    )) {
        if ($legacyCache -and (Test-Path $legacyCache)) {
            Remove-Item -Path $legacyCache -Recurse -Force -ErrorAction SilentlyContinue
            Write-Host "      [CLEAN SLATE] Purged legacy global WebView2 storage: $legacyCache" -ForegroundColor Green
        }
    }
}

Write-Host "======================================================================" -ForegroundColor Cyan
Write-Host "   [SUCCESS] RELEASE PACKAGES READY FOR GITHUB RELEASES!" -ForegroundColor Green
Write-Host "   1) Portable ZIP   : $ZipOutput [$PortableSizeMB MB]" -ForegroundColor White
if (-not $SkipExe -and (Test-Path $ExeZipOutput)) {
    Write-Host "   2) Standalone EXE : $ExeZipOutput [$ExeSizeMB MB]" -ForegroundColor White
}
Write-Host "======================================================================" -ForegroundColor Cyan
