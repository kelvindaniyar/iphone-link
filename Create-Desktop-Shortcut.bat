@echo off
setlocal
cd /d "%~dp0"

echo ======================================================================
echo   iPhone Link — Creating Desktop and Start Menu Shortcuts
echo ======================================================================
echo.

powershell -NoProfile -ExecutionPolicy Bypass -Command ^
  "$ws = New-Object -ComObject WScript.Shell; " ^
  "$currDir = (Get-Location).Path; " ^
  "$target = Join-Path $currDir 'iPhone Link.bat'; " ^
  "$icon = Join-Path $currDir 'assets\iphone_link.ico'; " ^
  "if (-not (Test-Path $icon)) { $icon = Join-Path $currDir 'packaging\pane.ico' }; " ^
  "$desktop = [Environment]::GetFolderPath('Desktop'); " ^
  "$startMenu = [Environment]::GetFolderPath('Programs'); " ^
  "$deskLnk = Join-Path $desktop 'iPhone Link.lnk'; " ^
  "$scDesk = $ws.CreateShortcut($deskLnk); " ^
  "$scDesk.TargetPath = $target; " ^
  "$scDesk.WorkingDirectory = $currDir; " ^
  "$scDesk.IconLocation = $icon; " ^
  "$scDesk.Description = 'iPhone Link — Native iPhone Mirroring and Control for Windows 11'; " ^
  "$scDesk.Save(); " ^
  "Write-Host '[OK] Desktop Shortcut created:' $deskLnk -ForegroundColor Green; " ^
  "$startLnk = Join-Path $startMenu 'iPhone Link.lnk'; " ^
  "$scStart = $ws.CreateShortcut($startLnk); " ^
  "$scStart.TargetPath = $target; " ^
  "$scStart.WorkingDirectory = $currDir; " ^
  "$scStart.IconLocation = $icon; " ^
  "$scStart.Description = 'iPhone Link — Native iPhone Mirroring and Control for Windows 11'; " ^
  "$scStart.Save(); " ^
  "Write-Host '[OK] Start Menu Shortcut created:' $startLnk -ForegroundColor Green;"

if %ERRORLEVEL% EQU 0 (
    echo.
    echo ======================================================================
    echo   [BERHASIL] Shortcut "iPhone Link" dengan icon native telah dibuat
    echo   di Desktop dan Start Menu Windows Anda!
    echo ======================================================================
) else (
    echo.
    echo [ERROR] Gagal membuat shortcut. Pastikan izin akses PowerShell aktif.
)

echo.
pause
endlocal
