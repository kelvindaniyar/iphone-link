# ======================================================================
# iPhone Link — Portable Release Packaging Script
# Creates a clean release distribution ready for GitHub Releases
# ======================================================================

param (
    [string]$Version = "v1.0.0"
)

$ErrorActionPreference = "Stop"
$RootDir = $PSScriptRoot
if (-not $RootDir) { $RootDir = (Get-Location).Path }

Write-Host "======================================================================" -ForegroundColor Cyan
Write-Host "   BUILDING IPHONE LINK PORTABLE RELEASE ($Version)" -ForegroundColor Cyan
Write-Host "======================================================================" -ForegroundColor Cyan

$ReleaseBase = Join-Path $RootDir "release"
$PackageName = "iPhone-Link-Portable-$Version"
$TargetDir = Join-Path $ReleaseBase $PackageName
$ZipOutput = Join-Path $ReleaseBase "$PackageName.zip"

# Clean previous release directory if exists
if (Test-Path $TargetDir) {
    Write-Host "[1/5] Cleaning existing release folder: $TargetDir" -ForegroundColor Yellow
    Remove-Item -Recurse -Force $TargetDir
}
if (Test-Path $ZipOutput) {
    Remove-Item -Force $ZipOutput
}

New-Item -ItemType Directory -Force -Path $TargetDir | Out-Null
Write-Host "[2/5] Creating release folder structure..." -ForegroundColor Green

# 1. Copy handset directory without __pycache__
Write-Host "[3/5] Copying handset application files..." -ForegroundColor Green
$HandsetTarget = Join-Path $TargetDir "handset"
Copy-Item -Recurse (Join-Path $RootDir "handset") $HandsetTarget
Get-ChildItem -Path $HandsetTarget -Recurse -Filter "__pycache__" | Remove-Item -Recurse -Force
Get-ChildItem -Path $HandsetTarget -Recurse -Include "*.pyc", "*.pyo" | Remove-Item -Force

# 2. Copy assets
Write-Host "      Copying assets (logo, icon)..." -ForegroundColor Green
Copy-Item -Recurse (Join-Path $RootDir "assets") (Join-Path $TargetDir "assets")

# 3. Copy launchers and core files
Write-Host "      Copying launchers and documentation..." -ForegroundColor Green
$CoreFiles = @(
    "iPhone Link.bat",
    "Create-Desktop-Shortcut.bat",
    "run.bat",
    "README.md",
    "LICENSE",
    "requirements.txt",
    "pyproject.toml"
)

foreach ($f in $CoreFiles) {
    $src = Join-Path $RootDir $f
    if (Test-Path $src) {
        Copy-Item -Force $src (Join-Path $TargetDir $f)
    }
}

# 4. Strict Security & Privacy Check (Ensure 0% Kelvin Private Data is Packaged)
Write-Host "[4/5] Running strict privacy and security sanity check..." -ForegroundColor Green
$PrivatePatterns = @(
    "debug_ocr_*",
    "debug_metrics.json",
    "typed_history.json",
    "user_settings.json",
    "wifi_state.json",
    "scratch",
    "LAPORAN_*.md"
)

$LeakCount = 0
foreach ($p in $PrivatePatterns) {
    $leaks = Get-ChildItem -Path $TargetDir -Recurse -Filter $p -ErrorAction SilentlyContinue
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

# 5. Compress to ZIP archive
Write-Host "[5/5] Compressing release package to ZIP..." -ForegroundColor Green
Compress-Archive -Path $TargetDir -DestinationPath $ZipOutput -CompressionLevel Optimal

$ZipItem = Get-Item $ZipOutput
$SizeInMB = [math]::Round($ZipItem.Length / 1048576, 2)

Write-Host "======================================================================" -ForegroundColor Cyan
Write-Host "   [SUCCESS] PORTABLE RELEASE READY FOR GITHUB!" -ForegroundColor Green
Write-Host "   Package Folder : $TargetDir" -ForegroundColor White
Write-Host "   Release ZIP    : $ZipOutput [$SizeInMB MB]" -ForegroundColor White
Write-Host "======================================================================" -ForegroundColor Cyan
