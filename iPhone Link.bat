@echo off
chcp 65001 >nul
setlocal EnableDelayedExpansion
cd /d "%~dp0"

:: Hardware HEVC Decoder Acceleration for WebView2
set "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--enable-features=PlatformHEVCDecoderSupport --mute-audio --disable-audio-output --disable-background-networking --disable-component-update --disable-domain-reliability --disable-sync --disable-features=Translate,OptimizationHints,MediaRouter,CalculateNativeWinOcclusion,AudioServiceOutOfProcess --js-flags=--max-old-space-size=96"

:: 1. Fast Path: Launch immediately if local venv is ready with webview & pymobiledevice3
if exist "%~dp0venv\Scripts\python.exe" (
    "%~dp0venv\Scripts\python.exe" -c "import webview, pymobiledevice3" >nul 2>&1
    if !ERRORLEVEL! EQU 0 goto :LAUNCH_VENV_SILENT
)

:: 2. Standalone EXE Fast Path: Launch directly if bundled iPhone Link.exe exists
if exist "%~dp0iPhone Link.exe" (
    start "" "%~dp0iPhone Link.exe"
    endlocal
    exit /b 0
)

:: 3. First-Time Automatic Setup (Runs only once when extracted in a new folder)
title iPhone Link - First-Time Automatic Setup
echo ==============================================================
echo   iPhone Link - First-Time Automatic Setup (Hanya 1x Saja)
echo ==============================================================
echo.
echo [1/3] Memeriksa instalasi Python di Windows...

set "SYS_PY="
for %%M in (3.12 3.11 3.13 3.10) do (
    if not defined SYS_PY (
        py -%%M -c "import sys; raise SystemExit(0)" >nul 2>&1
        if !ERRORLEVEL! EQU 0 set "SYS_PY=py -%%M"
    )
)

if not defined SYS_PY (
    for %%V in (312 311 313 310 39) do (
        if not defined SYS_PY if exist "%LOCALAPPDATA%\Programs\Python\Python%%V\python.exe" (
            set "SYS_PY=""%LOCALAPPDATA%\Programs\Python\Python%%V\python.exe"""
        )
        if not defined SYS_PY if exist "%ProgramFiles%\Python%%V\python.exe" (
            set "SYS_PY=""%ProgramFiles%\Python%%V\python.exe"""
        )
    )
)

if not defined SYS_PY (
    py -3 -c "import sys; raise SystemExit(0 if sys.version_info >= (3, 9) else 1)" >nul 2>&1
    if !ERRORLEVEL! EQU 0 set "SYS_PY=py -3"
)

if not defined SYS_PY (
    python -c "import sys; raise SystemExit(0 if sys.version_info >= (3, 9) else 1)" >nul 2>&1
    if !ERRORLEVEL! EQU 0 set "SYS_PY=python"
)

if not defined SYS_PY goto :NO_PYTHON_FOUND

echo       [OK] Python terdeteksi siap digunakan (!SYS_PY!).
echo.
echo [2/3] Membuat virtual environment lokal (venv)...
if not exist "%~dp0venv\Scripts\python.exe" (
    !SYS_PY! -m venv "%~dp0venv"
    if !ERRORLEVEL! NEQ 0 goto :SETUP_FAILED
)
echo       [OK] Virtual environment siap.
echo.
echo [3/3] Menginstal komponen pywebview ^& pymobiledevice3... (Mohon tunggu 1-2 menit)
if exist "%~dp0handset\_bootstrap_shims.py" (
    "%~dp0venv\Scripts\python.exe" "%~dp0handset\_bootstrap_shims.py" >nul 2>&1
)
"%~dp0venv\Scripts\python.exe" -m pip install --upgrade pip --quiet
"%~dp0venv\Scripts\python.exe" -m pip install -r "%~dp0requirements.txt"
if !ERRORLEVEL! NEQ 0 goto :SETUP_FAILED

"%~dp0venv\Scripts\python.exe" -c "import webview, pymobiledevice3" >nul 2>&1
if !ERRORLEVEL! NEQ 0 goto :SETUP_FAILED

echo.
echo ==============================================================
echo   [SUKSES] Setup selesai! Membuka iPhone Link sekarang...
echo ==============================================================
goto :LAUNCH_VENV_SILENT

:NO_PYTHON_FOUND
echo.
echo [PERINGATAN] Python 3.12 belum terinstal di PC Windows ini.
echo iPhone Link Portable membutuhkan Python 3.12 untuk setup awal,
echo ATAU unduh paket "Standalone EXE" tanpa Python di GitHub Releases.
echo.
set /p INSTALL_PY="Instal Python 3.12 otomatis sekarang via Windows Winget? (Y/N): "
if /i "!INSTALL_PY!"=="Y" (
    echo.
    echo Mengunduh dan menginstal Python 3.12 via winget...
    winget install -e --id Python.Python.3.12 --accept-package-agreements --accept-source-agreements
    if exist "%LOCALAPPDATA%\Programs\Python\Python312\python.exe" (
        echo.
        echo [OK] Python 3.12 berhasil diinstal! Melanjutkan setup venv...
        if exist "%~dp0venv" rmdir /s /q "%~dp0venv"
        "%LOCALAPPDATA%\Programs\Python\Python312\python.exe" -m venv "%~dp0venv"
        "%~dp0venv\Scripts\python.exe" -m pip install --upgrade pip --quiet
        "%~dp0venv\Scripts\python.exe" -m pip install -r "%~dp0requirements.txt"
        "%~dp0venv\Scripts\python.exe" -c "import webview, pymobiledevice3" >nul 2>&1
        if !ERRORLEVEL! EQU 0 goto :LAUNCH_VENV_SILENT
    )
)
echo.
echo Silakan buka ulang "iPhone Link.bat" setelah Python selesai terinstal.
pause
endlocal
exit /b 1

:SETUP_FAILED
echo.
echo ==============================================================
echo   [ERROR] Gagal menginstal komponen yang dibutuhkan.
echo   Pastikan koneksi internet aktif (dan Python 3.12 tersedia),
echo   lalu jalankan ulang "iPhone Link.bat".
echo ==============================================================
echo.
pause
endlocal
exit /b 1

:LAUNCH_VENV_SILENT
if exist "%~dp0venv\Scripts\pythonw.exe" (
    start "" "%~dp0venv\Scripts\pythonw.exe" "%~dp0handset\gui.py"
) else (
    start "" "%~dp0venv\Scripts\python.exe" "%~dp0handset\gui.py"
)
endlocal
exit /b 0
