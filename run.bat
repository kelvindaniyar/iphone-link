@echo off
setlocal
title Pane - iPhone Mirroring (WinUI 3 Liquid Glass)

cd /d "%~dp0"

echo ===================================================
echo   Pane - iPhone Mirroring (WinUI 3 Liquid Glass)
echo ===================================================
echo.

echo [1/3] Mengaktifkan virtual environment...
if exist "venv\Scripts\activate.bat" (
    call venv\Scripts\activate.bat
) else (
    echo [PERINGATAN] Folder venv tidak ditemukan, menggunakan python sistem...
)

echo [2/3] Menjalankan server Pane di background...
start "Pane Server" /min python -m handset.server

echo Menunggu server siap...
timeout /t 1 /nobreak >nul

echo [3/3] Membuka jendela native frameless (pywebview)...
if exist "venv\Scripts\python.exe" (
    .\venv\Scripts\python handset/gui.py
) else (
    python handset/gui.py
)

endlocal
