@echo off
chcp 65001 >nul
setlocal EnableDelayedExpansion
cd /d "%~dp0"

set "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--enable-features=PlatformHEVCDecoderSupport --disable-background-networking --disable-component-update --disable-domain-reliability --disable-sync --disable-features=Translate,OptimizationHints,MediaRouter,CalculateNativeWinOcclusion --js-flags=--max-old-space-size=96"
title iPhone Link - Native iPhone Mirroring for Windows 11

echo ==============================================================
echo   iPhone Link - Native iPhone Mirroring for Windows 11
echo ==============================================================
echo.

:: Check if local venv is ready with webview & pymobiledevice3
if exist "%~dp0venv\Scripts\python.exe" (
    "%~dp0venv\Scripts\python.exe" -c "import webview, pymobiledevice3" >nul 2>&1
    if !ERRORLEVEL! EQU 0 goto :RUN_APP
)

:: Check if standalone EXE exists and venv is not ready
if exist "%~dp0iPhone Link.exe" (
    echo Membuka iPhone Link Standalone EXE...
    start "" "%~dp0iPhone Link.exe"
    endlocal
    exit /b 0
)

echo ==============================================================
echo   iPhone Link - First-Time Automatic Setup (Hanya 1x Saja)
echo ==============================================================
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

if not defined SYS_PY goto :NO_PYTHON

echo       [OK] Python ditemukan (!SYS_PY!).
echo [2/3] Membuat virtual environment lokal (venv)...
if not exist "%~dp0venv\Scripts\python.exe" (
    !SYS_PY! -m venv "%~dp0venv"
    if !ERRORLEVEL! NEQ 0 goto :INSTALL_ERR
)

echo [3/3] Menginstal komponen pywebview ^& pymobiledevice3... (Mohon tunggu 1-2 menit)
if exist "%~dp0handset\_bootstrap_shims.py" (
    "%~dp0venv\Scripts\python.exe" "%~dp0handset\_bootstrap_shims.py" >nul 2>&1
)
"%~dp0venv\Scripts\python.exe" -m pip install --upgrade pip --quiet
"%~dp0venv\Scripts\python.exe" -m pip install -r "%~dp0requirements.txt"
if !ERRORLEVEL! NEQ 0 goto :INSTALL_ERR

"%~dp0venv\Scripts\python.exe" -c "import webview, pymobiledevice3" >nul 2>&1
if !ERRORLEVEL! NEQ 0 goto :INSTALL_ERR

echo.
echo [OK] Setup pertama berhasil diselesaikan!
echo.
goto :RUN_APP

:NO_PYTHON
echo.
echo [PERINGATAN] Python 3.12 belum terinstal di PC Windows ini.
set /p INSTALL_PY="Instal Python 3.12 otomatis via Windows Winget sekarang? (Y/N): "
if /i "!INSTALL_PY!"=="Y" (
    winget install -e --id Python.Python.3.12 --accept-package-agreements --accept-source-agreements
    if exist "%LOCALAPPDATA%\Programs\Python\Python312\python.exe" (
        if exist "%~dp0venv" rmdir /s /q "%~dp0venv"
        "%LOCALAPPDATA%\Programs\Python\Python312\python.exe" -m venv "%~dp0venv"
        "%~dp0venv\Scripts\python.exe" -m pip install --upgrade pip --quiet
        "%~dp0venv\Scripts\python.exe" -m pip install -r "%~dp0requirements.txt"
        "%~dp0venv\Scripts\python.exe" -c "import webview, pymobiledevice3" >nul 2>&1
        if !ERRORLEVEL! EQU 0 goto :RUN_APP
    )
)
echo Silakan jalankan ulang "run.bat" setelah Python terinstal.
pause
endlocal
exit /b 1

:INSTALL_ERR
echo.
echo [ERROR] Gagal menginstal dependensi Python. Periksa koneksi internet Anda lalu coba lagi.
pause
endlocal
exit /b 1

:RUN_APP
echo Menjalankan iPhone Link (In-Process Server + Native Frameless Window)...
"%~dp0venv\Scripts\python.exe" "%~dp0handset\gui.py"

if !ERRORLEVEL! NEQ 0 (
    echo.
    echo ==============================================================
    echo [ERROR] Aplikasi berhenti dengan kode error: !ERRORLEVEL!
    echo ==============================================================
    echo.
    pause
)

endlocal
exit /b 0
