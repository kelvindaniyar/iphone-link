@echo off
setlocal
set "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--enable-features=PlatformHEVCDecoderSupport"
taskkill /F /IM python.exe >nul 2>&1
title iPhone Link — Native iPhone Mirroring for Windows 11

cd /d "%~dp0"

echo ===================================================
echo   iPhone Link — Native iPhone Mirroring
echo ===================================================
echo.

echo [1/3] Mengaktifkan virtual environment...
if exist "venv\Scripts\activate.bat" (
    call venv\Scripts\activate.bat
) else (
    echo [PERINGATAN] Folder venv tidak ditemukan, menggunakan python sistem...
)

echo [2/3] Menjalankan server Pane di background...
if exist "venv\Scripts\python.exe" (
    start "Pane Server" /min .\venv\Scripts\python -m handset.server
) else (
    start "Pane Server" /min python -m handset.server
)

echo Menunggu server siap...
timeout /t 1 /nobreak >nul

echo [3/3] Membuka jendela native frameless (pywebview)...
if exist "venv\Scripts\python.exe" (
    .\venv\Scripts\python handset/gui.py
) else (
    python handset/gui.py
)

if %ERRORLEVEL% NEQ 0 (
    echo.
    echo ===================================================
    echo [ERROR] Aplikasi berhenti dengan kode error: %ERRORLEVEL%
    echo ===================================================
    echo.
    pause
)

echo Menutup background server...
taskkill /F /IM python.exe >nul 2>&1

endlocal
