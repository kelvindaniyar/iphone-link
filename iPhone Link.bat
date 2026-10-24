@echo off
setlocal
cd /d "%~dp0"

:: Hardware HEVC Decoder Acceleration for WebView2
set "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--enable-features=PlatformHEVCDecoderSupport"

:: Kill any orphaned server processes from previous sessions
taskkill /F /IM pythonw.exe >nul 2>&1

:: Detect Python and Pythonw executable (prefer venv)
if exist "venv\Scripts\pythonw.exe" (
    set "PYW=.\venv\Scripts\pythonw.exe"
    set "PY=.\venv\Scripts\python.exe"
) else if exist "pythonw.exe" (
    set "PYW=pythonw.exe"
    set "PY=python.exe"
) else (
    set "PYW=python.exe"
    set "PY=python.exe"
)

:: 1. Launch background server silently
start "" /b "%PYW%" -m handset.server

:: 2. Brief wait for server readiness
timeout /t 1 /nobreak >nul

:: 3. Launch native frameless window silently
start "" "%PYW%" handset/gui.py

endlocal
exit /b 0
