@echo off
chcp 65001 >nul
setlocal EnableDelayedExpansion
cd /d "%~dp0"

echo ======================================================================
echo   IPHONE LINK - CLEAN UNINSTALLER ^& PRIVACY DATA WIPER
echo ======================================================================
echo.
echo PERHATIAN:
echo Script ini akan membersihkan secara tuntas:
echo  1. Seluruh proses iPhone Link yang sedang berjalan
echo  2. Cache WebView2 / pywebview (termasuk Auto-Unlock PIN di localStorage)
echo  3. Riwayat ketik, pengaturan lokal, dan log debug iPhone Link
echo  4. Shortcut di Desktop dan Start Menu Windows
echo.
echo ======================================================================
set /p CONFIRM="Lanjutkan pembersihan privasi dan uninstall? (Y/N): "
if /i not "%CONFIRM%"=="Y" (
    echo.
    echo [BATAL] Pembersihan dibatalkan oleh pengguna.
    echo.
    pause
    exit /b 0
)

echo.
echo [1/4] Menghentikan proses iPhone Link yang berjalan...
powershell -NoProfile -ExecutionPolicy Bypass -Command ^
  "Get-Process -Name 'iPhone Link' -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue; " ^
  "Get-Process -Name python, pythonw -ErrorAction SilentlyContinue | Where-Object { $_.Path -like '*iphone-link*' -or $_.Path -like '*iPhone-Link*' -or $_.Path -like '*pane-iphone-mirror*' } | Stop-Process -Force -ErrorAction SilentlyContinue"

echo [2/4] Menghapus Cache WebView2 / pywebview (PIN ^& localStorage)...
powershell -NoProfile -ExecutionPolicy Bypass -Command ^
  "$paths = @(Join-Path $env:APPDATA 'pywebview', Join-Path $env:LOCALAPPDATA 'pywebview', Join-Path $HOME '.pane_webview_storage'); " ^
  "foreach ($p in $paths) { if (Test-Path $p) { Remove-Item -Path $p -Recurse -Force -ErrorAction SilentlyContinue; Write-Host '  -> Terhapus:' $p -ForegroundColor Green } }"

echo [3/4] Menghapus data riwayat lokal, cache OCR, dan pengaturan...
powershell -NoProfile -ExecutionPolicy Bypass -Command ^
  "$appDir = (Get-Location).Path; " ^
  "$files = @('user_settings.json', 'typed_history.json', 'wifi_state.json', 'debug_metrics.json'); " ^
  "foreach ($f in $files) { $p = Join-Path $appDir $f; if (Test-Path $p) { Remove-Item -Force $p -ErrorAction SilentlyContinue; Write-Host '  -> Terhapus:' $f -ForegroundColor Green } }; " ^
  "Get-ChildItem -Path $appDir -Filter 'debug_ocr_*' -File -ErrorAction SilentlyContinue | ForEach-Object { Remove-Item -Force $_.FullName -ErrorAction SilentlyContinue; Write-Host '  -> Terhapus:' $_.Name -ForegroundColor Green }"

echo [4/4] Menghapus shortcut di Desktop dan Start Menu...
powershell -NoProfile -ExecutionPolicy Bypass -Command ^
  "$desktop = [Environment]::GetFolderPath('Desktop'); " ^
  "$startMenu = [Environment]::GetFolderPath('Programs'); " ^
  "$shortcuts = @( " ^
  "  Join-Path $desktop 'iPhone Link.lnk', " ^
  "  Join-Path $startMenu 'iPhone Link.lnk', " ^
  "  Join-Path $desktop 'iPhone Mirroring.lnk', " ^
  "  Join-Path $startMenu 'iPhone Mirroring.lnk' " ^
  "); " ^
  "foreach ($s in $shortcuts) { if (Test-Path $s) { Remove-Item -Force $s -ErrorAction SilentlyContinue; Write-Host '  -> Shortcut terhapus:' $s -ForegroundColor Green } }"

echo.
echo ======================================================================
echo   [SUKSES] Seluruh PIN, Cache Privasi, dan Shortcut telah BERSIH 100%%!
echo ======================================================================
echo.

set /p DEL_FOLDER="Apakah Anda juga ingin menghapus seluruh file ^& folder aplikasi ini dari harddisk? (Y/N): "
if /i "%DEL_FOLDER%"=="Y" (
    echo.
    echo Menghapus folder aplikasi dari harddisk...
    start "" cmd /c "timeout /t 1 /nobreak >nul & rmdir /s /q \"%~dp0\""
    exit /b 0
)

echo.
echo Selesai! Terima kasih telah menggunakan iPhone Link.
pause
endlocal
