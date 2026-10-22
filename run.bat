@echo off
setlocal
title Pane - iPhone Mirroring

cd /d "%~dp0"

echo ===================================================
echo   Pane - iPhone Mirroring (macOS Sequoia UI)
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

echo Menunggu server siap (2 detik)...
timeout /t 2 /nobreak >nul

echo [3/3] Membuka jendela iPhone Mirroring...

:: Cek lokasi Google Chrome
set "CHROME_CMD="
if exist "%ProgramFiles%\Google\Chrome\Application\chrome.exe" (
    set CHROME_CMD="%ProgramFiles%\Google\Chrome\Application\chrome.exe"
) else if exist "%ProgramFiles(x86)%\Google\Chrome\Application\chrome.exe" (
    set CHROME_CMD="%ProgramFiles(x86)%\Google\Chrome\Application\chrome.exe"
) else if exist "%LocalAppData%\Google\Chrome\Application\chrome.exe" (
    set CHROME_CMD="%LocalAppData%\Google\Chrome\Application\chrome.exe"
)

if defined CHROME_CMD (
    start "" %CHROME_CMD% --app=http://127.0.0.1:8765 --window-size=375,820
) else (
    where chrome.exe >nul 2>&1
    if %errorlevel% equ 0 (
        start "" chrome.exe --app=http://127.0.0.1:8765 --window-size=375,820
    ) else (
        echo [INFO] Chrome tidak ditemukan di path default, membuka via Microsoft Edge App Mode...
        start "" msedge.exe --app=http://127.0.0.1:8765 --window-size=375,820
    )
)

echo.
echo Selesai! Pane sudah berjalan di http://127.0.0.1:8765.
echo Kamu bisa menutup jendela terminal ini jika server sudah berjalan di background.
endlocal
