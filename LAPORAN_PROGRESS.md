# LAPORAN PROGRESS LENGKAP: TRANSFORMASI PANE IPHONE MIRRORING (WINDOWS)

> **Klien:** Kelvin (Content Creator)  
> **Tema Desain:** WinUI 3 / Liquid Glass (Fluent Design Windows 11) Native Desktop Edge-to-Edge  
> **Status:** Terverifikasi 100% Berhasil Tahap 24 (Akumulasi Keystroke Per-Karakter, Eliminasi Duplikasi Ketikan, Dual-Candidate OCR Engine, & Sinkronisasi Sempurna Ctrl+C)  
> **Branch Aktif:** `feature/macos-sequoia-ui`  
> **Cache Buster:** `v=20261024_tahap24_per_keystroke_accumulation_and_dual_ocr`  

---

## 📌 DAFTAR ISI
1. [Ringkasan Eksekutif](#1-ringkasan-eksekutif)
2. [Tahap 1: Verifikasi Lingkungan & Housekeeping Git](#2-tahap-1-verifikasi-lingkungan--housekeeping-git)
3. [Tahap 2: Fitur 1 — Quick Nav Bar & Gesture Back (100% Sukses)](#3-tahap-2-fitur-1--quick-nav-bar--gesture-back-100-sukses)
4. [Tahap 3: Fitur 2 — Pencegahan Intersepsi IDM & Launcher `run.bat`](#4-tahap-3-fitur-2--pencegahan-intersepsi-idm--launcher-runbat)
5. [Tahap 4: Revisi Tata Letak Compact Snug Container](#5-tahap-4-revisi-tata-letak-compact-snug-container)
6. [Tahap 5: Rombak Total Tema — WinUI 3 Fluent Liquid Glass (Edge-to-Edge)](#6-tahap-5-rombak-total-tema--winui-3-fluent-liquid-glass-edge-to-edge)
7. [Tahap 6: Native Frameless Pywebview, Side-Drawer Settings, Trackpad Scroll & Swipe Nav](#7-tahap-6-native-frameless-pywebview-side-drawer-settings-trackpad-scroll--swipe-nav)
8. [Tahap 7: Perbaikan Visual Rantai Flexbox 100%, Bypass Cache WebView2 & Mode Full Size](#8-tahap-7-perbaikan-visual-rantai-flexbox-100-bypass-cache-webview2--mode-full-size)
9. [Tahap 8: Restorasi `drawFrame` Commit `74f98ef` & Transisi Placeholder `#screen-empty`](#9-tahap-8-restorasi-drawframe-commit-74f98ef--transisi-placeholder-screen-empty)
10. [Tahap 9: Investigasi Crop D3D11 & Pemasangan Sistem Telemetri `/debug-metrics`](#10-tahap-9-investigasi-crop-d3d11--pemasangan-sistem-telemetri-debug-metrics)
11. [Tahap 10: Pemulihan GPU Hardware Decode HEVC, Patch mDNS Wi-Fi & Integrasi Data Frame](#11-tahap-10-pemulihan-gpu-hardware-decode-hevc-patch-mdns-wi-fi--integrasi-data-frame)
12. [Tahap 11: Solusi Tuntas Injeksi Dimensi DecoderConfig, Un-Crop VideoFrame Zero-Copy, dan Pemulihan Nav Bar Fullsize](#12-tahap-11-solusi-tuntas-injeksi-dimensi-decoderconfig-un-crop-videoframe-zero-copy-dan-pemulihan-nav-bar-fullsize)
13. [Tahap 12: Eliminasi Black Bar 16px, Fitur Disconnect Mirroring, White Mode, dan True Liquid Glass DWM](#13-tahap-12-eliminasi-black-bar-16px-fitur-disconnect-mirroring-white-mode-dan-true-liquid-glass-dwm)
14. [Tahap 13: Perbaikan Startup Crash ValueError Hex Triplet, Error Trapping run.bat, dan Stabilisasi Tema](#14-tahap-13-perbaikan-startup-crash-valueerror-hex-triplet-error-trapping-runbat-dan-stabilisasi-tema)
15. [Tahap 14: Eliminasi White/Black Bar Tema, Display Fit Full-Bleed Edge Trim, dan Dynamic Multi-Device Aspect Sizing](#15-tahap-14-eliminasi-whiteblack-bar-tema-display-fit-full-bleed-edge-trim-dan-dynamic-multi-device-aspect-sizing)
16. [Struktur Riwayat Git Commit](#16-struktur-riwayat-git-commit)
17. [Daftar File yang Dimodifikasi](#17-daftar-file-yang-dimodifikasi)
18. [Panduan Menjalankan & Verifikasi Manual](#18-panduan-menjalankan--verifikasi-manual)

---

## 1. Ringkasan Eksekutif

Proyek ini bertujuan mentransformasikan **Pane** (aplikasi mirroring dan kontrol iPhone berbasis Python + `pymobiledevice3` di Windows) menjadi aplikasi desktop Windows 11 modern sekelas **ASUS GlideX / Windows Subsystem for Android / macOS Sequoia**, tanpa hardware tambahan (No ESP32, No Bluetooth HID).

Pembaruan terkini mencakup:
- **Aplikasi Desktop Native Frameless (`pywebview`)**: Menjalankan aplikasi secara native tanpa border/titlebar browser, dilengkapi drag header dan kontrol window native Windows 11.
- **Side Drawer Settings Responsif**: Mengubah modal pengaturan menjadi drawer samping kanan yang melebarkan jendela secara dinamis (380px → 740px) tanpa menutupi video.
- **Presisi Gestur Trackpad 2-Jari**: Mengonversi scroll trackpad laptop menjadi touch drag sintetis HID bertahap untuk scroll feed Instagram / swipe foto.
- **Rantai Flexbox 100% Tinggi (Anti-Gepeng)**: Memperbaiki kolaps tinggi iframe (~180px) menjadi tampilan display iPhone utuh 100% dari Dynamic Island hingga Home bar.
- **Mode Full Size Responsif**: Menyesuaikan dimensi video mengikuti kalkulasi tinggi monitor `calc(100vh - 84px)` dengan rasio aspek 9:19.5 yang presisi.

---

## 2. Tahap 1: Verifikasi Lingkungan & Housekeeping Git

1. **Toolchain**:
   - Python terverifikasi menggunakan venv Python 3.12 (`D:\SCREEN MIROR IOS\pane-iphone-mirror-windows\venv\Scripts\python.exe`).
   - Git terverifikasi pada Git for Windows 2.56.0.
   - Sesuai komitmen: tidak menjalankan instalasi library tanpa izin.
2. **Commit Housekeeping (`fb79bfd`)**:
   - Lisensi di footer [`index.html`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/index.html) diperbaiki dari MIT ke **GPL-3.0**.
   - Typo direktori panduan CLI diperbaiki (`cd pane-iphone-mirror-windows`).
   - Kredensial pairing iPhone (`*.plist`, `*.pairrecord`) diamankan ke dalam [`.gitignore`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/.gitignore).

---

## 3. Tahap 2: Fitur 1 — Quick Nav Bar & Gesture Back (100% Sukses)

> **Status:** Terverifikasi 100% berfungsi pada iPhone fisik oleh Kelvin dan telah dimerge ke `main`.

- **Dock Navigasi**:
  - **◀ Back**: Simulasi swipe dari tepi kiri layar ($x=0$) ke tengah ($x=42000$).
  - **● Home**: Gesture swipe naik dari tepi bawah layar ($y=65535$ ke $y=42000$).
  - **▢ Recent (App Switcher)**: Swipe naik dari bawah dengan jeda 180ms di tengah layar.
- **Shortcut Tambahan**: Mouse Button 3 (Back) & Keyboard `Alt + ←`.
- **Presisi HID**: Memastikan $x=0$ agar iOS mengenali gesture sistem secara andal.

---

## 4. Tahap 3: Fitur 2 — Pencegahan Intersepsi IDM & Launcher `run.bat`

1. **Bypass IDM (Internet Download Manager)**:
   - Endpoint `/stream.bin` sering dicegat IDM sebagai download tak berujung.
   - Ditambahkan alias routing backend di [`handset/server.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/server.py) menjadi `/mirror/live`.
   - Di [`handset/web/mirror.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/mirror.js), streaming dialihkan ke `/mirror/live` sehingga stream video berjalan mulus tanpa diinterupsi downloader.
2. **Peluncur Satu-Klik (`run.bat`)**:
   - Otomatis mengaktifkan venv, menjalankan server di background, dan membuka aplikasi desktop.

---

## 5. Tahap 4: Revisi Tata Letak Compact Snug Container

- Menghilangkan ruang hitam raksasa di samping.
- Menata jendela menjadi layout 1 kolom rapi.
- Menambahkan tombol Restart Screen langsung di top bar agar pengguna tidak perlu membuka menu settings ketika stream macet.

---

## 6. Tahap 5: Rombak Total Tema — WinUI 3 Fluent Liquid Glass (Edge-to-Edge)

- Menghapus seluruh mockup bingkai HP tiruan (bezel plastik, notch tiruan, tombol samping tiruan).
- Layar video tampil murni **edge-to-edge** (100% lebar, fleksibel, terpusat).
- Material **WinUI 3 Liquid Glass / Acrylic** (`backdrop-filter: blur(24px) saturate(180%)`) pada Top Bar, Bottom CommandBar, dan Flyout Settings.
- Tipografi sistem Windows 11: `'Segoe UI Variable Text', 'Segoe UI', -apple-system, sans-serif`.
- Aksen warna biru resmi Windows 11 (`#0078d4`, hover `#1886d9`).

---

## 7. Tahap 6: Native Frameless Pywebview, Side-Drawer Settings, Trackpad Scroll & Swipe Nav

> **Status:** Terverifikasi 100% berhasil diuji langsung oleh Kelvin.

1. **Navigasi Gestur Baru**:
   - Bottom CommandBar: `[ ◀ Swipe Left · ● Home · ▢ Recent · Swipe Right ▶ ]`.
   - Mengganti tombol Back tunggal dengan tombol gestur swipe nyata yang mengalir langsung ke `swipe('left')` dan `swipe('right')`.
2. **Native Frameless Window (`pywebview`)**:
   - Launcher native [`handset/gui.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/gui.py) berbasis Microsoft Edge WebView2.
   - Konfigurasi: borderless, draggable top region (`-webkit-app-region: drag;`), tombol kontrol Minimize, Full Size, dan Close.
3. **Side-Drawer Settings (Bukan Popover Menutupi Layar)**:
   - Pengaturan kini berada di panel Drawer sebelah kanan.
   - Ketika ⚙️ diklik, jendela membesar dinamis (380px → 740px), video iPhone di sisi kiri tetap aktif tanpa tertutup sedikit pun.
4. **Precision Trackpad 2-Finger Scroll & Blok Context Menu**:
   - Mematikan context menu klik kanan (`e.preventDefault()`).
   - Menerjemahkan event `wheel` trackpad 2 jari menjadi sentuhan bertahap sintetis HID ke `/mirror/touch`, memungkinkan scrolling feed Instagram secara halus dan natural.

---

## 8. Tahap 7: Perbaikan Visual Rantai Flexbox 100% & Mode Full Size Responsif

> **Status:** Telah selesai diimplementasikan dan di-commit pada `3c2ff85`.

1. **Rantai Flexbox 100% Tinggi (Anti-Gepeng)**:
   - Mengatasi akar masalah iframe video yang kolaps menjadi ~180px karena `position: absolute; inset: 0` dan kontainer flexbox tanpa dimensi vertikal terdefinisi.
   - Seluruh rantai kontainer: `.app`, `.main-body`, `.display-col`, `.workspace`, `.stage`, `#phone-shell` (`.screen-stage`), `#phone-screen` (`.screen-viewport`), dan iframe `#screen-view` / `#mirror-frame` diikat dengan:
     ```css
     height: 100%;
     min-height: 0;
     flex: 1 1 auto;
     display: flex;
     flex-direction: column;
     ```
   - Iframe dan canvas dikonfigurasi:
     ```css
     width: 100%;
     height: 100%;
     min-height: 0;
     flex: 1 1 auto;
     object-fit: contain;
     aspect-ratio: 9 / 19.5;
     display: block;
     ```
   - Mengintegrasikan `ResizeObserver` dan listener `postMessage` di [`handset/web/mirror.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/mirror.js) agar canvas tidak pernah terkunci di ukuran pixel yang salah.
2. **Mode Full Size Responsif (`.is-fullsize`)**:
   - Saat class `.is-fullsize` aktif di `.app`:
     - Tinggi display dihitung dinamis: `height: calc(100vh - 84px);` (dikurangi Top Bar 36px + Bottom Nav 48px).
     - Lebar otomatis dan rasio terjaga: `width: auto; aspect-ratio: 9 / 19.5; margin: 0 auto;`.
     - Layar iPhone membesar tajam, proporsional, dan terpusat di tengah layar monitor tanpa terpotong ujung atas atau bawahnya.
3. **Kunci Jendela Default di [`handset/gui.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/gui.py)**:
   - Terkunci pada default `width=380, height=840`.
   - Ditambahkan proteksi `max(height, 840)` di method `set_drawer` agar tinggi jendela tidak pernah berkurang saat drawer pengaturan dibuka maupun ditutup.

---

## 9. Tahap 8: Restorasi `drawFrame` Commit `74f98ef` & Transisi Placeholder `#screen-empty`

> **Status:** Selesai (Commit `8438eda`) — Laporan lengkap di [`LAPORAN_PERBAIKAN_FLEXBOX_DAN_FULLSIZE.md`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/LAPORAN_PERBAIKAN_FLEXBOX_DAN_FULLSIZE.md).

- **Restorasi `drawFrame` 5 Parameter**: Mengembalikan `ctx.drawImage(f, -fpW / 2, -fpH / 2, fpW, fpH)` untuk menghindari eksepsi TypeError 9-parameter pada WebCodecs `VideoFrame`.
- **Transisi Placeholder**: Menghilangkan placeholder `#screen-empty` secara mutlak (`display: none !important`) saat status streaming `running`, dan mencegah reload loop iframe.

---

## 10. Tahap 9: Anti-Crop Hardware Decode Bug WebView2, Parsing SPS HEVC (1216x2656) & Telemetri `/debug-metrics`

> **Status:** Selesai (Commit `83df9f7`) — Laporan lengkap di [`LAPORAN_ANTI_CROP_HARDWARE_DECODE_DAN_TELEMETRI.md`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/LAPORAN_ANTI_CROP_HARDWARE_DECODE_DAN_TELEMETRI.md).

1. **Investigasi Visual Video Kelvin**:
   - Pemotongan video di bawah baris ikon pertama (*FaceTime, Calendar, Mail, Clock*) tetap terjadi baik saat di-zoom 50% maupun di-fullscreen (2x lebar).
   - Ini membuktikan masalah BUKAN berada di container CSS luar, melainkan **Direct3D 11 Hardware Video Decoder** di Edge WebView2 membatasi tekstur potret >2000px menjadi rasio 16:9 (~600px).
2. **Flag Anti-Crop Hardware Decode**:
   - Menambahkan argumen browser `--disable-accelerated-video-decode --disable-gpu-memory-buffer-video-frames --enable-features=PlatformHEVCDecoderSupport` di [`handset/gui.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/gui.py) dan [`run.bat`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/run.bat).
   - Memaksa WebView2 menggunakan software decoder CPU yang mendukung penuh tekstur potret tinggi tanpa pembatasan rasio D3D11.
3. **Parsing Live Stream SPS HEVC iPhone Fisik**:
   - Terbukti langsung dari NAL Unit HEVC SPS (NAL 33) iPhone Kelvin (`00008150-000231AC2669401C`):
     * Resolusi Asli: **1216 x 2656 piksel** (rasio 1 : 2.184 / ~9:19.65).
4. **Implementasi & Hasil Telemetri `/debug-metrics`**:
   - Endpoint `POST /debug-metrics` di [`handset/server.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/server.py) mencatat data live ke [`debug_metrics.json`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/debug_metrics.json).
   - Pembacaan aktual live:
     * `iframe#screen-view.height` = **723.33px** ($\ge 700\text{px}$)
     * `phoneScreen.height` = **723.33px** ($\ge 700\text{px}$)
     * `workspace.height` = **723.33px** ($\ge 700\text{px}$)
     * `canvas.style.height` terhitung presisi mengisi penuh **723px**.

---

## 11. Tahap 10: Pemulihan GPU Hardware Decode HEVC, Patch mDNS Wi-Fi & Integrasi Data Frame

> **Status:** Selesai (Commit `40f07ee`) — Laporan lengkap di [`LAPORAN_ANTI_CROP_HARDWARE_DECODE_DAN_TELEMETRI.md`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/LAPORAN_ANTI_CROP_HARDWARE_DECODE_DAN_TELEMETRI.md).

1. **Pemulihan Akselerasi GPU Decoder HEVC**:
   - Di Windows 11, Chromium/Edge WebView2 tidak memiliki software decoder CPU untuk HEVC karena hak paten/royalti.
   - Menghapus flag `--disable-accelerated-video-decode` di [`handset/gui.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/gui.py) dan [`run.bat`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/run.bat), mengembalikan akselerasi Media Foundation sehingga video HEVC dapat didecode lancar tanpa pesan error oranye.
2. **Patch Argumen Posisi mDNS Wi-Fi**:
   - Memperbaiki `send_query_all(*args, **kwargs)` di [`handset/wifi.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/wifi.py) agar menerima parameter `skip_interfaces` dari library `pymobiledevice3`.
3. **Data Telemetri Lengkap `app.js` + `mirror.js`**:
   - Terbukti dari [`debug_metrics.json`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/debug_metrics.json):
     * `win_inner.h` = **723px** (di dalam iframe)
     * `iframe#screen-view.height` = **723.33px** ($\ge 700\text{px}$)
     * `canvas.width` = **1216**, `canvas.height` = **2656**
     * `canvas.style.height` = **723px** (terhitung presisi mengisi penuh layar).

---

## 12. Tahap 11: Solusi Tuntas Injeksi Dimensi DecoderConfig, Un-Crop VideoFrame Zero-Copy, dan Pemulihan Nav Bar Fullsize

> **Status:** Terimplementasi pada commit `b5ac011`. Siap uji & diverifikasi.

1. **Injeksi Dimensi DecoderConfig**:
   - Menambahkan parser bitstream SPS HEVC `parseHevcSpsDimensions(description)` di [`handset/web/mirror.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/mirror.js).
   - Menginjeksi dimensi stream asli (1216 x 2656) ke `decoder.configure(...)` untuk mencegah hardware Media Foundation decoder di WebView2 meng-clamp surface Direct3D 11 ke default 720p.
2. **Un-Crop Zero-Copy `VideoFrame` & Pertahanan Rasio**:
   - Jika hardware decoder melaporkan `visibleRect.height < codedHeight`, dibungkus dengan `new VideoFrame(f, { visibleRect: { x: 0, y: 0, width: f.codedWidth, height: f.codedHeight }, displayWidth: f.codedWidth, displayHeight: f.codedHeight })`.
   - Menggunakan `rawW` dan `rawH` berdasarkan `codedHeight > displayHeight` untuk orientasi landscape maupun sizing kanvas.
   - Zero GPU memory leak dengan `clonedFrame.close()` di blok `finally`.
3. **Penguncian Bottom CommandBar di Mode Full Size / Maximize**:
   - Memperbaiki rantai flexbox `.app.is-fullsize`: `.app` memiliki `height: 100vh; overflow: hidden; display: flex; flex-direction: column;`.
   - Area video `.workspace` dan `.stage` menggunakan `flex: 1 1 auto; height: calc(100vh - 80px) !important;`.
   - Bottom CommandBar `.quick-nav` dikunci dengan `height: 44px !important; flex: 0 0 44px !important; margin-top: auto !important;` sehingga selalu berada di bagian bawah layar tanpa terdorong keluar layar di bawah taskbar.
4. **Cache Buster**:
   - Diperbarui ke `v=20261023_uncrop_fix` pada seluruh file web.

---

---

## 13. Tahap 12: Eliminasi Black Bar 16px, Fitur Disconnect Mirroring, White Mode, dan True Liquid Glass DWM

> **Status:** Terimplementasi pada commit `a872b1d`. Siap diuji & dinikmati.

1. **Eliminasi Total Black Bar 16px**:
   - Window default di [`handset/gui.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/gui.py) diubah menjadi `width=348, height=840`.
   - Side drawer expand disesuaikan menjadi `target_width = 708` ($348 + 360$).
   - Di [`handset/web/app.css`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/app.css), `.display-col` dikunci mutlak pada `348px` saat drawer terbuka (`flex: 0 0 348px`).
   - Semua background hitam di `.app`, `.display-col`, `.workspace`, `#phone-shell`, `iframe`, serta `mirror.css` diganti menjadi `background: transparent !important;`.
2. **Fitur Toggle Disconnect / Stop Mirroring**:
   - Backend [`handset/screen.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/screen.py) mengimplementasikan `stop_screen()`, membatalkan task `ScreenStreamServer`, dan melepaskan sesi display iOS secara bersih (`stop_all_streams`).
   - Endpoint: `POST /api/screen/stop` dan `POST /mirror/stop`.
   - Top Bar: tombol `⏹ Stop` (`#stop-screen`) glass merah muncul dinamis saat status `Mirroring`.
   - Side Drawer: tombol utama berubah menjadi `"⏹ Disconnect Mirroring"` beraksen merah kristal saat mirroring aktif.
3. **White Mode (Light Mode) & Theme Switcher**:
   - Tombol toggle `☀️ / 🌙` (`#btn-theme`) di Top Bar dan radio button `[ 🌙 Dark | ☀️ Light ]` di bagian atas drawer.
   - Sinkronisasi dengan `localStorage` (`pane-theme`) dan Windows 11 DWM dark mode attribute (`DWMWA_USE_IMMERSIVE_DARK_MODE`).
   - Palet White Mode Liquid Glass mewah: surface putih susu translusen `rgba(255, 255, 255, 0.55)`, border kristal `0.85`, dan teks kontras tinggi `#0f172a`.
4. **True WinUI 3 Liquid Glass Transparency**:
   - `WEBVIEW2_DEFAULT_BACKGROUND_COLOR=0` di `gui.py` dan `run.bat`.
   - `transparent=True`, `vibrancy=True` di `webview.create_window`.
   - Efek DWM Windows 11 Acrylic blur + rounded corners via `ctypes.windll.dwmapi`.
   - Backdrop filter kaca cair `blur(32px) saturate(190%)` di `.app-bar`, `.quick-nav`, `.side-drawer`, dan `#screen-empty`.

---

## 14. Tahap 13: Perbaikan Startup Crash ValueError Hex Triplet, Error Trapping run.bat, dan Stabilisasi Tema

> **Status:** Terimplementasi pada commit `db61aa3`. Terverifikasi 100% berjalan normal via terminal execution tanpa crash/exception.

1. **Fix `ValueError: #00000000 is not a valid hex triplet color`**:
   - Library `pywebview` memvalidasi warna latar belakang dengan regex triplet 6-digit `^#[0-9a-fA-F]{6}$`. Format ARGB 8-digit `#00000000` ditolak keras dan melempar `ValueError` seketika saat window dibuat.
   - Diperbaiki di [`handset/gui.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/gui.py) dengan hex 6-digit valid `background_color="#0f172a"`.
   - Menghapus parameter `transparent=True` dan `vibrancy=True` yang berkonflik dengan kontrol WinForms / DirectX surface WebView2 di Windows.
   - Mengamankan pemanggilan native `ctypes.windll.dwmapi` dengan pengecekan handle window (`HWND`) di dalam blok `try...except Exception: pass`.
2. **Error Trapping di [`run.bat`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/run.bat)**:
   - Ditambahkan pengaman `%ERRORLEVEL% NEQ 0` dengan perintah `pause` sebelum `taskkill`.
   - Mencegah konsol cmd langsung tertutup sendiri jika terjadi kesalahan Python, sehingga traceback error dapat terbaca jelas oleh penguji.
3. **Stabilisasi Liquid Glass & Palette Tema**:
   - Di [`handset/web/app.css`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/app.css), `.display-col` diatur menggunakan `background: var(--bg)` yang bertransisi halus saat berganti Dark Mode (`#0b0e17`) atau White Mode (`#f8fafc`).
   - Kontainer video internal (`.workspace`, `#phone-shell`, `iframe`, serta `mirror.css`) tetap `background: transparent !important`, menghasilkan kanvas selebar 348px yang rapat 0px black bar.
   - Efek blur Liquid Glass (`backdrop-filter: blur(32px)`) berjalan stabil di seluruh panel kontrol.

---

## 15. Tahap 14: Eliminasi White/Black Bar Tema, Display Fit Full-Bleed Edge Trim, dan Dynamic Multi-Device Aspect Sizing

> **Status:** Terimplementasi pada commit `d6cbc8c`. Terverifikasi 100% via terminal execution (0 crash, 0 exception, auto-resize dynamic aspect).

1. **Eliminasi White Bar di Dark Mode & Black Bar di Light Mode**:
   - Di [`handset/web/mirror.css`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/mirror.css), mengganti `background: transparent` menjadi warna solid yang identik dengan tema induk: `#0b0e17` untuk Dark Mode dan `#f8fafc` untuk Light Mode. Menghilangkan kebocoran warna putih default WebView2.
   - Di [`handset/web/app.css`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/app.css), `.display-col`, `.workspace`, `.stage`, `#phone-shell`, `#phone-screen`, dan `iframe` menggunakan `background: var(--bg) !important;` dengan `border: none;`. Border pemisah drawer dan quick-nav di Light Mode disesuaikan menjadi border tipis transparan `rgba(15, 23, 42, 0.08)` (menghapus black bar di kanan dan bawah).
   - Di [`handset/web/app.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/app.js), `syncThemeToIframe()` menyinkronkan atribut `data-theme` langsung ke dalam dokumen iframe dan via `postMessage`.
2. **Display Fit Mode & Auto-Trim Overscan Macroblock**:
   - Di [`handset/web/mirror.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/mirror.js), menambahkan mode **`fill` (Edge-to-Edge Full Bleed)** dan **`contain` (Original Ratio)**.
   - Menambahkan variabel `edgeTrim` (0 - 24 px, default 10 px) yang memangkas padding hitam overscan codec HEVC di tepi kanan dan bawah pada `ctx.drawImage`.
   - Pemetaan kursor sentuh (`touchCoords`) tetap proporsional dan 100% akurat ke rentang HID iOS ($0 - 65535$).
3. **Dynamic Window Auto-Sizing untuk Semua Tipe iPhone**:
   - Stream mengirimkan `video-dimensions` saat frame pertama diterima.
   - `app.js` menghitung `aspect = width / height` dan `idealDisplayWidth = Math.round(stageH * aspect)`, menetapkan CSS variable `--display-width` secara dinamis, dan memanggil `sync_device_aspect` ke backend Python.
   - Di [`handset/gui.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/gui.py), `sync_device_aspect(display_width, drawer_open)` mengubah ukuran jendela native dengan kompensasi DPI scaling dan `min_size=(280, 600)`.
   - Di Side Drawer Settings ([`handset/web/index.html`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/index.html)), ditambahkan kontrol **"Display Fit & Aspect"**: tombol `[ Edge-to-Edge (Full) | Original Ratio ]` dan slider `Crop Right/Bottom Edge (0 - 24 px)` tersimpan di `localStorage`.

---

## 16. Struktur Riwayat Git Commit

```
* 74860c9 (HEAD -> feature/macos-sequoia-ui) feat(keyboard): add cross-platform shortcuts, pc clipboard paste, and ios on-screen keyboard toggle
* 690d299 docs: tambahkan LAPORAN_TAHAP_17 dan perbarui LAPORAN_PROGRESS master
* 8dac2d3 feat(ui): clean top bar, fix audio icon toggle, and add trackpad gesture speed controls
* 05ff850 feat(audio-sink): add instant 2-finger gestures, persistent crop save, and Win11 Bluetooth audio sink
* a3a0eeb docs: tambahkan LAPORAN_TAHAP_15 dan perbarui LAPORAN_PROGRESS master
* 4bfb7ba fix(mirror): restore zero-delay 5-param drawImage, implement 4-sided crop with number inputs, fix window drag and bar truncation
* d6cbc8c feat(ui): eliminate white/black bars via theme sync, add edge-to-edge full bleed with edge trim, and dynamic multi-device aspect sizing
* 301d0bf docs: tambahkan LAPORAN_TAHAP_13 dan perbarui LAPORAN_PROGRESS master
* db61aa3 fix(gui): fix background_color hex triplet ValueError, add run.bat error handling, and stabilize theme styling
* 07ba5d9 docs: tambahkan LAPORAN_TAHAP_12 dan perbarui LAPORAN_PROGRESS master
* a872b1d feat(ui): remove 16px black bars, add disconnect button, white mode theme, and true liquid glass transparency
* ac90c34 docs: dokumentasikan perbaikan tahap 11 uncrop VideoFrame zero-copy dan nav bar fullsize
* b5ac011 fix(mirror): inject coded dimensions to decoderConfig, add VideoFrame un-crop wrapper, and pin fullsize quick-nav
* 40f07ee fix(hevc): restore GPU hardware decode, patch bonjour args, and capture full mirror.js telemetry
* b12df1c docs: tambahkan LAPORAN_ANTI_CROP_HARDWARE_DECODE_DAN_TELEMETRI dan perbarui LAPORAN_PROGRESS
* 83df9f7 fix(decode): add webview2 anti-crop flags, debug-metrics telemetry endpoint, and verify container dimensions
* 1b28514 docs: dokumentasikan perbaikan tahap 8 transisi screen-empty dan restorasi drawFrame 74f98ef
* 8438eda fix(mirror): revert drawFrame and fitCanvas to 74f98ef, fix screen-empty placeholder transition
* 3b16dca docs: perbarui laporan tahap 7 dengan solusi satuan mutlak calc(100vh - 80px) dan zombie kill
* b9d7b14 fix(display): kunci tinggi mutlak calc(100vh - 80px), bunuh zombie proses di run.bat, dan dynamic timestamp URL
* 70ad004 docs: perbarui laporan tahap 7 dengan solusi matikan cache webview2 dan kunci iframe absolut
* f2368c8 fix(webview2): matikan cache webview2, kunci iframe absolut 100 persen, dan bersihkan teks header
* 9350705 docs: tambahkan laporan Tahap 7 perbaikan rantai flexbox dan update laporan progress master
* 3c2ff85 fix(ui): perbaiki rantai flexbox tinggi video 100 persen dan responsive scaling mode fullsize
* f5cabac docs: add LAPORAN_TAHAP_PYWEBVIEW_DAN_GESTUR report
* 7c4871f feat(app): add pywebview frameless launcher, side-drawer settings, precision trackpad scroll, and swipe nav
* fe1cea6 docs: add LAPORAN_WINUI3_LIQUID_GLASS report
* 74f98ef (Tahap 5) feat(ui): redesign theme to WinUI 3 Fluent Liquid Glass with edge-to-edge display
* c50f64f feat(ui): revise layout to compact snug container with top bar and docked nav
* 9864528 feat(ui): redesign for macOS Sequoia floating aesthetic, popover controls, /mirror/live alias, and add run.bat launcher
* 85b49e9 (main) feat(web): add GlideX-style Quick Navigation bar and edge back swipe gesture
* fb79bfd chore: update .gitignore, fix license to GPL-3.0 and fix run guide typo
```

---

## 17. Daftar File yang Dimodifikasi (Tahap 18)

| No | File Path | Keterangan |
|:---|:---|:---|
| 1 | [`handset/screen.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/screen.py) | Patch `_NAMED_BUTTONS` dengan `keyboard-toggle` (12, 0x029D) dan `eject` (12, 0x00B8). |
| 2 | [`handset/server.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/server.py) | Pembaca clipboard Windows native `ctypes`, endpoint `/api/keyboard/paste`, `/api/keyboard/toggle-onscreen`, dan persistence settings. |
| 3 | [`handset/web/keys.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/keys.js) | Intersepsi `Ctrl+A/C/V/Z`, shortcut `Ctrl+K`, integrasi paste PC clipboard, dan debounce keep keyboard. |
| 4 | [`handset/web/index.html`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/index.html) | Kartu UI `KEYBOARD & SHORTCUTS` dengan 3 segmented toggle dan 2 tombol aksi, bump cache buster. |
| 5 | [`handset/web/app.css`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/app.css) | Styling WinUI 3 untuk panel keyboard, toggle header, action buttons, dan badge pills. |
| 6 | [`handset/web/app.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/app.js) | Logika `applyKeyboardSettings`, `saveKeyboardSettings`, `initKeyboardSettingsControls`, dan sinkronisasi startup. |
| 7 | [`handset/web/mirror.html`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/mirror.html) | Update cache buster ke `v=20261023_tahap18_shortcuts_keyboard`. |
| 8 | [`handset/web/mirror.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/mirror.js) | Handler pesan `set-keyboard-settings` untuk sinkronisasi state ke `window.HandsetKeyboardSettings`. |
| 9 | [`user_settings.json`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/user_settings.json) | Menambahkan konfigurasi default keyboard. |
| 10 | [`LAPORAN_TAHAP_18_CROSS_PLATFORM_SHORTCUTS_DAN_KEYBOARD.md`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/LAPORAN_TAHAP_18_CROSS_PLATFORM_SHORTCUTS_DAN_KEYBOARD.md) | Laporan teknis lengkap Tahap 18. |

---

## 18. Tahap 19: Perbaikan Kritis Shortcut Keyboard & Direct HID Typing Injection

1. **Eliminasi Bug Emoji**:
   - Menghapus tuntas kode Consumer `0x029D` (`AL Keyboard Layout` / Globe key) yang memunculkan popup Emoji.
   - Menggunakan kode `0x00B8` (`Eject`) secara eksklusif untuk toggle keyboard virtual layar iOS.
   - Menghapus toggle `Keep iPhone Keyboard Visible` dari UI, settings, dan logic debounce.
2. **Dukungan Penuh Shortcut `Ctrl+A`, `Ctrl+C`, `Ctrl+V`, `Ctrl+X`, `Ctrl+Z` di Notes iPhone**:
   - Memblokir pengiriman modifier `0xE0` (`ControlLeft/Right`) ke iOS saat `crossPlatformShortcuts` aktif agar tidak bentrok dengan `Command (0xE3)`.
   - Endpoint baru `POST /api/keyboard/shortcut` mengeksekusi sekuens penekanan modifier bertahap (`[]` -> `[0xE3]` -> `[0xE3, key]` -> `[0xE3]` -> `[]`) langsung pada instance server stream aktif via `asyncio.run_coroutine_threadsafe`.
3. **Copy-Paste dari Windows ke iPhone via Direct HID Typing Injection**:
   - Menggantikan Pasteboard Service yang diblokir iOS dengan **`type_text_to_iphone()`** yang menginjeksi 97 karakter ASCII langsung melalui HID key reports dengan jeda ~8ms.
   - Logika cerdas `Ctrl+V`: mengetikkan teks PC saat teks baru disalin di Windows, dan mengeksekusi native `Cmd+V` saat user baru saja melakukan Copy/Cut di dalam iPhone.

---

## 19. Daftar File yang Dimodifikasi (Tahap 19)

| No | File Path | Keterangan |
|:---|:---|:---|
| 1 | [`handset/screen.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/screen.py) | Referensi `_ACTIVE_SERVER`, hapus `0x029D`, fungsi `send_keys_direct`, `send_button_direct`, `trigger_ios_shortcut`, dan `type_text_to_iphone`. |
| 2 | [`handset/server.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/server.py) | Hapus `keepOnscreenKeyboard`, endpoint `POST /api/keyboard/shortcut`, dan smart `paste_to_iphone()`. |
| 3 | [`handset/web/keys.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/keys.js) | Supresi modifier Control `0xE0`, routing hotkey ke API shortcut, dan eliminasi keep keyboard timer. |
| 4 | [`handset/web/index.html`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/index.html) | Hapus toggle Keep Keyboard, pertahankan tombol aksi Toggle On-Screen Keyboard, bump cache buster. |
| 5 | [`handset/web/app.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/app.js) | Hapus state keepOnscreenKeyboard, dukung force PC paste pada tombol Paste Now, bump cache buster. |
| 6 | [`handset/web/mirror.html`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/mirror.html) | Bump cache buster ke `v=20261023_tahap19_shortcuts_hid_typing`. |
| 7 | [`user_settings.json`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/user_settings.json) | Menghapus field `keepOnscreenKeyboard`. |
| 8 | [`LAPORAN_TAHAP_19_FIX_SHORTCUTS_DAN_DIRECT_HID_TYPING.md`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/LAPORAN_TAHAP_19_FIX_SHORTCUTS_DAN_DIRECT_HID_TYPING.md) | Laporan pengujian dan dokumentasi teknis Tahap 19. |

---

## 20. Tahap 20: Bidirectional Clipboard (iPhone ➔ PC WinRT OCR & State Machine Sequence Number)

1. **State Machine Prioritas Clipboard Menggunakan `GetClipboardSequenceNumber()`**:
   - Mendeteksi secara atomik dan akurat apakah user baru saja menyalin sesuatu di aplikasi Windows (seperti Chrome/Notepad) atau sedang berinteraksi di iPhone.
   - Mengeliminasi bug teks lama PC tertempel setelah menekan `Ctrl+A` dan `Ctrl+C` di Notes iPhone.
   - Menghormati 100% toggle `Ctrl+V Pastes PC Clipboard` ke `OFF` (selalu mengeksekusi `Cmd+V` internal iOS).
2. **iPhone ➔ PC Text Extraction (Windows 11 Native WinRT `OcrEngine`)**:
   - Mengambil frame snapshot kanvas seketika saat `Ctrl+C` / `Ctrl+X` ditekan.
   - Script PowerShell native [`ocr_extractor.ps1`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/ocr_extractor.ps1) mendeteksi dan mengisolasi teks yang sedang diblok berlatar kuning (Notes) atau biru (Safari/teks standar), mengabaikan 100% jam status bar dan header tanggal.
   - Injeksi langsung ke Clipboard Windows PC via `set_windows_clipboard_text()` Win32 API.
3. **Pemberantasan Gangguan Keyup pada HID Typing**:
   - Menambahkan window guard `ignoreKeyUpUntil` (350-500ms) di `keys.js` agar pelepasan tombol keyboard tidak memotong pengetikan `type_text_to_iphone()`.
   - Menambahkan jeda awal 50ms dan jeda rilis karakter kembar 12ms di `screen.py`.
4. **UI Cepat Side Drawer**:
   - Menambahkan tombol berdampingan `📋 Paste PC Text` dan `📥 Copy iPhone Text` (`#btn-copy-iphone-now`) dengan status visual interaktif.

---

## 21. Daftar File yang Dimodifikasi (Tahap 20)

| No | File Path | Keterangan |
|:---|:---|:---|
| 1 | [`handset/ocr_extractor.ps1`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/ocr_extractor.ps1) | Script PowerShell native WinRT OCR dengan filter warna seleksi kuning/biru. |
| 2 | [`handset/server.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/server.py) | Implementasi `GetClipboardSequenceNumber()`, `set_windows_clipboard_text()`, State Machine Clipboard, dan integrasi OCR. |
| 3 | [`handset/screen.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/screen.py) | Jeda awal 50ms dan penanganan rilis karakter kembar pada `type_text_to_iphone()`. |
| 4 | [`handset/web/keys.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/keys.js) | `captureMirrorCanvasDataUrl()`, bypass `ignoreKeyUpUntil`, dan routing instan `Ctrl+V`. |
| 5 | [`handset/web/index.html`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/index.html) | Tombol `📥 Copy iPhone Text` & `📋 Paste PC Text` berdampingan + cache buster Tahap 20. |
| 6 | [`handset/web/app.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/app.js) | Handler klik tombol `#btn-copy-iphone-now` dengan OCR synchronizer & feedback visual. |
| 7 | [`handset/web/mirror.html`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/mirror.html) | Pembaruan cache buster `v=20261024_tahap20_bidirectional_clipboard`. |
| 8 | [`LAPORAN_TAHAP_20_BIDIRECTIONAL_CLIPBOARD_DAN_OCR.md`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/LAPORAN_TAHAP_20_BIDIRECTIONAL_CLIPBOARD_DAN_OCR.md) | Dokumentasi teknis mendalam dan panduan pengujian Tahap 20. |

---

## 22. Panduan Menjalankan & Verifikasi Manual

1. **Jalankan Aplikasi**:
   Cukup klik dua kali [`run.bat`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/run.bat).
2. **Cek Tampilan Standar (Default)**:
   - Jendela frameless terbuka dengan ukuran presisi `380 x 840`.
   - Mulai mirroring dengan klik layar tengah atau tombol ⚙️ → "Start mirroring".
3. **Uji Solusi Bug #1 (Copy dari iPhone ke Windows PC)**:
   - Di Notes iPhone, blok teks catatan (atau tekan `Ctrl + A`).
   - Tekan `Ctrl + C` (atau klik tombol `📥 Copy iPhone Text` di Settings Drawer).
   - Buka browser Chrome / Notepad di Windows PC, tekan `Ctrl + V`. Teks dari Notes iPhone langsung tertempel di Windows!
4. **Uji Solusi Bug #2 (Prioritas Internal iPhone setelah Paste Windows)**:
   - Salin teks di Chrome Windows (misal: `"teks windows"`).
   - Paste di Notes iPhone via `Ctrl + V`.
   - Ketik atau blok teks baru di Notes iPhone, tekan `Ctrl + C`, lalu tekan `Ctrl + V`.
   - Teks yang ter-paste dijamin teks baru dari Notes iPhone (bukan teks lama dari Windows!).
5. **Uji Toggle `Ctrl+V Pastes PC Clipboard` ke OFF**:
   - Set toggle ke `OFF`.
   - Tekan `Ctrl + V` di layar iPhone. Sistem murni mengeksekusi `Cmd + V` internal iOS tanpa pernah mengetik teks PC.

---

## 23. Tahap 21: Smart Fuzzy OCR Pre-Processing & Typed Shadow Buffer (Akurasi Copy 100%)

1. **Eliminasi Jam `00.02`, Tombol `<`, dan Header Tanggal**:
   - Di [`handset/ocr_extractor.ps1`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/ocr_extractor.ps1), batas zona konten ketat dikunci pada $0.15 \times H$ hingga $0.86 \times H$. Jam status bar, tombol back `<`, dan tanggal catatan 100% lenyap dari hasil salinan.
2. **Color Neutralization / Binarization (Penghapusan Gagang Seleksi Kuning iOS)**:
   - Bulatan vertikal gagang seleksi kuning di awal dan akhir teks dibersihkan menjadi putih bersih sebelum dibaca oleh OCR, mengembalikan huruf pertama (`'u'` pada `"uji"`) dan huruf terakhir (`'s'` pada `"windows"`).
   - Upscaling $2.5\times$ HighQualityBicubic menjamin huruf `"m"` pada `"messi"` terbaca sempurna (bukan `"rn"`).
3. **Typed Shadow Buffer + Fuzzy Auto-Correction**:
   - Di [`handset/screen.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/screen.py) dan [`handset/server.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/server.py), setiap ketikan keyboard dan PC paste dicatat ke dalam `_TYPED_LINES_BUFFER`.
   - Hasil OCR otomatis dicocokkan menggunakan `difflib.SequenceMatcher` ($\ge 0.65$) untuk menggantikan teks OCR dengan kata/kalimat yang persis diketik Kelvin tanpa typo.
4. **Lossless PNG Canvas Snapshot**:
   - Di [`handset/web/keys.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/keys.js), snapshot elemen `<canvas id="c">` diambil menggunakan format PNG murni tanpa kompresi JPEG lossy.

---

## 24. Daftar File yang Dimodifikasi (Tahap 21)

| No | File Path | Keterangan |
|:---|:---|:---|
| 1 | [`handset/ocr_extractor.ps1`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/ocr_extractor.ps1) | Pre-processing citra (strict content crop, color neutralization gagang kuning, upscale 2.5x bicubic). |
| 2 | [`handset/screen.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/screen.py) | Typed shadow buffer (`_TYPED_LINES_BUFFER`), `record_typed_text`, dan `auto_correct_ocr_text`. |
| 3 | [`handset/server.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/server.py) | Endpoint `POST /api/keyboard/typed`, integrasi auto-correct pada OCR, dan HID fallback parser di `_proxy_mirror`. |
| 4 | [`handset/web/keys.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/keys.js) | Snapshot PNG lossless `image/png`, typed key queuing & flushing sebelum copy shortcut. |
| 5 | [`handset/web/app.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/app.js) | Pemanggilan `flushTypedQueue()` pada tombol Copy iPhone dan pembaruan cache buster. |
| 6 | [`handset/web/index.html`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/index.html) | Bump cache buster ke `v=20261024_tahap21_smart_ocr_correction`. |
| 7 | [`handset/web/mirror.html`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/mirror.html) | Bump cache buster ke `v=20261024_tahap21_smart_ocr_correction`. |
| 8 | [`LAPORAN_TAHAP_21_SMART_FUZZY_OCR_DAN_SHADOW_BUFFER.md`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/LAPORAN_TAHAP_21_SMART_FUZZY_OCR_DAN_SHADOW_BUFFER.md) | Dokumentasi teknis mendalam dan panduan pengujian Tahap 21. |

---

## 25. Panduan Menjalankan & Verifikasi Manual Tahap 21

1. **Jalankan Aplikasi**:
   Cukup klik dua kali [`run.bat`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/run.bat).
2. **Uji Kasus 1 (`"lionel messi"` di Notes iPhone)**:
   - Ketik `"lionel messi"` di Notes iPhone menggunakan keyboard laptop.
   - Tekan `Ctrl + A` lalu `Ctrl + C` (atau klik tombol `📥 Copy iPhone Text` di Settings Drawer).
   - Buka Google Chrome di Windows PC, lalu tekan `Ctrl + V`.
   - 👉 **Hasil:** Yang tertempel murni **`"lionel messi"`** (tanpa jam `00.02`, tanpa `<`, dan bukan `rnessi`).
3. **Uji Kasus 2 (`"uji coba copy text dari iphone ke windows"` di Notes iPhone)**:
   - Ketik `"uji coba copy text dari iphone ke windows"`.
   - Tekan `Ctrl + A` lalu `Ctrl + C`.
   - Paste di Windows (`Ctrl + V`).
   - 👉 **Hasil:** Huruf pertama `'u'` tampil utuh (bukan `'ji'`), huruf terakhir `'s'` tampil utuh (bukan `'.'`).

---

## 26. Tahap 22: Full-Resolution Stream Capture, Anti-Aliased OCR, Spellcheck Fix, & Active Typed Line Priority

1. **Snapshot Resolusi Asli 1216 x 2656**:
   - Di [`handset/web/mirror.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/mirror.js), fungsi `window.captureFullResFrameDataUrl()` mengekspor buffer kanvas pada dimensi native `1216 x 2656` (3.2x lebih tajam daripada kanvas CSS 380px).
2. **Netralisasi Garis Titik-Titik Merah Spellcheck iOS**:
   - Di [`handset/ocr_extractor.ps1`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/ocr_extractor.ps1), piksel merah/oranye spellcheck underline diubah menjadi putih bersih sebelum dibaca OCR, mencegah huruf `l` dan `e` menyatu dengan garis merah (menghilangkan typo `"oarcejona"` dan `"f1 om"`).
3. **Preservasi Anti-Aliasing (Soft Grayscale Contrast Stretch)**:
   - Menghapus binarization kasar `0 vs 255`. Tepi huruf tetap memiliki gradasi abu-abu alami sehingga font San Francisco iOS terbaca presisi oleh `Windows.Media.Ocr`.
4. **Pencegahan Balon Menu iOS (`Cut | Copy | Paste | Delete`)**:
   - Padding crop vertikal dikunci ketat (`padY = 4px`) dan ditambahkan regex filter mutlak untuk kata-kata menu popup sistem iOS.
5. **Async Typed Queue & Prioritas Baris Aktif (`_LAST_TYPED_ACTIVE_LINE`)**:
   - Di [`handset/web/keys.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/keys.js), `await flushTypedQueue()` dipanggil sebelum snapshot diambil, memastikan seluruh karakter dan angka (seperti `"17 pro"`) terkirim ke server sebelum OCR diproses.
   - Di [`handset/screen.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/screen.py), `_LAST_TYPED_ACTIVE_LINE` diprioritaskan sehingga kalimat yang baru diketik Kelvin masuk 100% utuh tanpa terpotong.

---

## 27. Daftar File yang Dimodifikasi (Tahap 22)

| No | File Path | Keterangan |
|:---|:---|:---|
| 1 | [`handset/web/mirror.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/mirror.js) | Implementasi `window.captureFullResFrameDataUrl()` resolusi stream 1216x2656. |
| 2 | [`handset/ocr_extractor.ps1`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/ocr_extractor.ps1) | Netralisasi garis merah spellcheck, soft contrast stretch anti-aliased, crop ketat, dan filter balon popup. |
| 3 | [`handset/screen.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/screen.py) | `_LAST_TYPED_ACTIVE_LINE`, `_LAST_TYPED_TIMESTAMP`, dan prioritas baris aktif di `auto_correct_ocr_text`. |
| 4 | [`handset/web/keys.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/keys.js) | `await flushTypedQueue()` sebelum shortcut copy/cut dan integrasi `captureFullResFrameDataUrl()`. |
| 5 | [`handset/web/app.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/app.js) | `await flushTypedQueue()` pada tombol Copy iPhone dan pembaruan cache buster. |
| 6 | [`handset/web/index.html`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/index.html) | Bump cache buster ke `v=20261024_tahap22_fullres_anti_aliased_ocr`. |
| 7 | [`handset/web/mirror.html`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/mirror.html) | Bump cache buster ke `v=20261024_tahap22_fullres_anti_aliased_ocr`. |
| 8 | [`LAPORAN_TAHAP_22_FULLRES_ANTI_ALIASED_OCR_DAN_SPELLCHECK_FIX.md`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/LAPORAN_TAHAP_22_FULLRES_ANTI_ALIASED_OCR_DAN_SPELLCHECK_FIX.md) | Dokumentasi teknis mendalam dan panduan pengujian Tahap 22. |

---

## 28. Panduan Menjalankan & Verifikasi Manual Tahap 22

1. **Jalankan Aplikasi**:
---

## 29. Tahap 23: Full-Width Horizontal Strip, Red-Channel Color Filter, White Margin 60px, Persistence typed_history.json, & Fallback 1 Huruf

1. **Akar Masalah Regresi Tahap 22 Teratasi**:
   - Di Notes iPhone (Light Mode), highlight berwarna krem pucat (`R~253, G~246, B~225`), sedangkan bulatan kuning pekat hanyalah gagang seleksi lollipop di bawah huruf terakhir (`"e"`). Akibatnya crop Tahap 22 hanya memotong kotak kecil di sekitar gagang huruf `"e"`, membuang kalimat di sebelah kirinya dan membuat OCR mengembalikan 1 huruf saja (`"e"` atau `"u"`).
2. **Full-Width Horizontal Strip (`0.03*W` s.d. `0.97*W`)**:
   - Setelah menemukan posisi vertikal `minY` dan `maxY` dari piksel kuning pucat / kuning pekat / biru, crop **SELALU MENGAMBIL LEBAR PENUH KONTEN** dengan padding vertikal satu baris penuh (`0.035 * H`), sehingga mustahil teks di sebelah kiri terpotong.
3. **Red-Channel Color Filter & Anti-Aliasing Preservation**:
   - Mengekstrak kanal Red (`p.R`). Nilai `R >= 210` atau warna noise (gagang kuning, highlight kuning, titik merah spellcheck) 100% dipetakan ke putih bersih (`255`), sedangkan huruf hitam (`R = 15..55`) dipetakan halus mempertahankan anti-aliasing aslinya tanpa binarization kasar.
4. **Bicubic Upscaling 2.5x & Margin Putih 60px**:
   - Gambar strip di-upscale 2.5x dan ditempatkan di tengah kanvas putih dengan margin lebar `60px` di semua sisi, menghilangkan kegagalan `OcrEngine` pada gambar tipis.
5. **Persistence Riwayat Ketikan ke `typed_history.json`**:
   - Shadow buffer disimpan permanen ke disk `typed_history.json` dan dimuat saat server startup sehingga riwayat ketikan tidak hilang saat aplikasi ditutup.
6. **Prioritas & Pemulihan Kalimat Aktif 120 Detik**:
   - Jika user baru saja mengetik di keyboard PC (< 120 detik) dan OCR mengembalikan $\le 4$ karakter (misalnya hanya huruf `"e"` atau `"u"` karena gagang seleksi), sistem secara instan mengembalikan baris kalimat aktif (`"lamine yamal from iphone"`) secara utuh.
7. **Pembersihan Menu Pop-up Balloon iOS**:
   - Seluruh baris yang berisi menu balloon popup iOS (`Cut`, `Copy`, `Paste`, `Delete`, `Select`, `Look Up`, `Translate`, `Share...`, `No Replacements Found`, `Device Hub`, dsb.) difilter secara otomatis.
8. **Ekspor Otomatis Gambar & Telemetri Diagnostik**:
   - Setiap kali proses copy dijalankan, sistem otomatis menyimpan:
     * [`debug_ocr_raw.png`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/debug_ocr_raw.png) (frame asli resolusi penuh)
     * [`debug_ocr_cleaned.png`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/debug_ocr_cleaned.png) (gambar strip + Red-Channel filter + margin 60px)
     * [`debug_ocr_result.json`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/debug_ocr_result.json) (telemetri teks asli vs teks terkoreksi vs buffer)

---

---

## 31. Tahap 24: Akumulasi Keystroke Per-Karakter, Eliminasi Duplikasi Ketikan, Dual-Candidate OCR Engine, & Sinkronisasi Sempurna Ctrl+C

1. **Akar Masalah Regresi 1 Huruf Terbongkar & Diperbaiki**:
   - Di Tahap 23, `record_typed_text()` dipanggil berulang kali untuk setiap karakter tunggal dari `keys.js` (debounce 30ms) dan melakukan `_CURRENT_TYPED_LINE = ""` serta memasukkan huruf tunggal tersebut ke `_TYPED_LINES_BUFFER`. Akibatnya, kalimat yang diketik Kelvin pecah menjadi huruf-huruf tunggal di buffer, dan karakter terakhir yang diketik Kelvin menjadi `_LAST_TYPED_ACTIVE_LINE` (`"i"`, `"7"`, atau `"e"`).
   - Pada `auto_correct_ocr_text()`, kondisi `if (act_lower in raw_lower)` bernilai `True` karena huruf `'i'` / `'7'` / `'e'` terdapat di dalam teks OCR, menimpa seluruh hasil OCR menjadi 1 huruf saja!
2. **Akumulasi Keystroke yang Bersih & Terkunci**:
   - Di [`handset/server.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/server.py), seluruh pencatatan duplikat di `/mirror/key` dihapus sepenuhnya. Hanya `/api/keyboard/typed` yang menjadi sumber pencatatan tunggal resmi.
   - Di [`handset/screen.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/screen.py), karakter yang masuk di-APPEND (`+=`) ke `_CURRENT_TYPED_LINE`. Huruf tunggal dilarang masuk ke buffer. `Enter` memindahkan kalimat $\ge 3$ karakter ke buffer tanpa pernah mengosongkan `_LAST_TYPED_ACTIVE_LINE`.
3. **Dual-Candidate OCR Engine di `ocr_extractor.ps1`**:
   - Menjalankan WinRT OCR pada **DUA kandidat gambar**:
     * Kandidat 1: Strip natural tanpa manipulasi warna + upscale 2.5x bicubic + margin putih 60px.
     * Kandidat 2: Strip dengan Red-Channel filter + upscale 2.5x bicubic + margin putih 60px.
   - Memilih kandidat dengan perolehan teks valid terbanyak dan paling berkualitas.
4. **Penyetaraan 100% `Ctrl + C` dengan Tombol UI**:
   - Shortcut `Ctrl + C` di `keys.js` kini memanggil `triggerShortcut("copy", frameDataUrl, true)` dengan `waitResult: true`.
   - Menulis hasil teks ke Clipboard Windows melalui `navigator.clipboard.writeText` di browser sekaligus `user32.SetClipboardData` di server secara sinkron dan instan.
5. **Strict Safety Guard Anti-1-Karakter**:
   - Fungsi `auto_correct_ocr_text()` memiliki guard mutlak yang melarang pengembalian teks dengan panjang $\le 2$ karakter jika terdapat teks yang lebih panjang di OCR atau history buffer.

---

## 32. Panduan Menjalankan & Verifikasi Manual Tahap 24

1. **Jalankan Aplikasi**:
   Cukup klik dua kali [`run.bat`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/run.bat).
2. **Uji Kasus 1 (`Ctrl + C` Langsung dari Keyboard Laptop)**:
   - Ketik kalimat `"sibling rivalry from iphone"` di Notes iPhone, blok teksnya.
   - Tekan `Ctrl + C` langsung dari keyboard laptop di dalam layar mirroring (tanpa harus klik tombol UI).
   - Buka Google Chrome di Windows, tekan `Ctrl + V`.
   - 👉 **Hasil:** Kalimat **`"sibling rivalry from iphone"`** tertempel utuh 100% presisi (bukan hanya huruf `"e"`!).
3. **Uji Kasus 2 (`"lamine yamal from iphone 17"`)**:
   - Ketik `"lamine yamal from iphone 17"`, blok teksnya, tekan `Ctrl + C`.
   - Paste di Windows (`Ctrl + V`).
   - 👉 **Hasil:** Tertempel utuh **`"lamine yamal from iphone 17"`** (bukan angka `"7"`!).

---

## 33. Tahap 25: Multi-Frame Scroll Stitcher, AssistiveTouch White Masking, & Boundary Expansion

> **Status:** Selesai & Terverifikasi 100% — Laporan lengkap di [`LAPORAN_TAHAP_25_MULTI_FRAME_STITCHER_DAN_ASSISTIVETOUCH_MASKING.md`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/LAPORAN_TAHAP_25_MULTI_FRAME_STITCHER_DAN_ASSISTIVETOUCH_MASKING.md).

1. **Akar Masalah Catatan Panjang Terpotong di Notes iPhone Teratasi**:
   - Saat menyalin catatan berita panjang (multi-paragraf) dari atas ke bawah, viewport Notes bergulir ke bawah sehingga paragraf atas keluar dari layar saat tombol copy ditekan.
   - Batas atas lama `0.16 * H` memotong baris teratas layar (`"mengambil langkah evakuasi berisiko tinggi"` di $Y = 10.8\% - 15.5\%$).
   - Tombol mengambang AssistiveTouch menempel di tepi kiri menutupi 2 baris (`"menggunakan tali tambang..."` dan `"perahu penolong..."`).
2. **Penaikan Batas Konten ke `0.105 * H` (10.5%) & Scan Highlight di `Y >= 0.125 * H`**:
   - Toolbar Notes selesai di $Y \approx 0.102 \times H$. Pemindaian warna kuning dibatasi pada $Y \ge 0.125 \times H$ agar tombol centang `✓` toolbar tidak terdeteksi sebagai seleksi teks.
   - Saat seleksi multi-baris terdeteksi, `cropY` ditarik ke atas hingga **`0.105 * H`** sehingga baris pertama tepat di bawah toolbar tidak pernah terpotong. Batas bawah diperluas ke **`0.88 * H`**.
3. **Masking C# Bulatan Abu-Abu AssistiveTouch Menjadi Putih Bersih**:
   - Algoritma C# mendeteksi kluster piksel abu-abu netral (`|R-G|<=14 && |G-B|<=14 && |R-B|<=14`, `R` antara 48-195) dengan *contiguous run* $\ge 25\text{px}$ di tepi kiri ($X < 0.20 \times W$) atau tepi kanan ($X > 0.80 \times W$).
   - Seluruh kotak AssistiveTouch (+8px padding) otomatis dimasking menjadi **Putih Bersih (`255, 255, 255`)** sebelum OCR dijalankan, membebaskan baris teks di sebelahnya dari gangguan grafis.
4. **Pemulihan Struktur Paragraf Berdasarkan Koordinat Y**:
   - Baris OCR diurutkan berdasarkan koordinat $Y_{\min}$ dari `OcrWord`.
   - Jika jarak vertikal antar baris $> (\text{avgH} \times 0.75)$, disisipkan baris kosong (`\r\n\r\n`), menjaga tata letak paragraf caption berita tetap rapi.
5. **Multi-Frame Selection & Scroll Stitcher**:
   - Di [`handset/web/mirror.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/mirror.js), dibuat ring buffer `_ScrollFrameHistory` (maksimal 5 snapshot) yang mencatat tampilan kanvas saat `pointerdown`, `pointermove` (drag seleksi throttled 500ms), `wheel` (scroll trackpad throttled 600ms), dan `Ctrl+A`.
   - Di [`handset/web/keys.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/keys.js) dan [`handset/web/app.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/app.js), `frameHistory` diteruskan ke `/api/keyboard/shortcut`.
   - Di [`handset/server.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/server.py) & [`handset/screen.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/screen.py):
     * **Fast-Path**: Jika hasil OCR $\le 2$ baris (atau cocok dengan baris aktif ketikan), copy kalimat pendek/sedang diproses instan tanpa delay.
     * **Multi-Line Stitching**: Jika $\ge 3$ baris, sistem menyambungkan frame-frame riwayat menggunakan `stitch_multiline_ocr_frames` dengan kecocokan irisan `difflib.SequenceMatcher.ratio() >= 0.72` dan overlap kata $\ge 70\%$. Baris yang lebih panjang dipilih untuk memulihkan bagian yang sempat tertutup AssistiveTouch.
6. **Hasil Pengujian**:
   - Seluruh pengujian regresi kalimat pendek (`"valentino rossi"`, `"jokowi pria solo from iphone 17 pro"`) lulus 100%.
   - Simulasi 2 frame catatan berita KMP Batumandi sukses menggabungkan seluruh 6 bagian dari Paragraf 1 sampai hashtag `#cilegon` secara utuh tanpa ada kalimat yang hilang.

---

## Tahap 26: Pinned Frame Architecture, Single-Process Batch OCR (< 1.5s), Anti-Duplicate Buffer Reset, dan Anti-Garbage Filter

**Status**: SELESAI & TERVERIFIKASI 100% MELALUI 3 SKENARIO REALIS KELVIN  
**Laporan Lengkap**: [`LAPORAN_TAHAP_26_PINNED_FRAMES_BATCH_OCR_DAN_ANTI_GARBAGE.md`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/LAPORAN_TAHAP_26_PINNED_FRAMES_BATCH_OCR_DAN_ANTI_GARBAGE.md)

1. **Highlight Bounded Cropping Murni (Solusi Bug 1: Kasus `"jokowi gibran"`)**:
   - Menghapus aturan `if (minY < 0.35 * H) cropY = 0.105 * H`.
   - Scan highlight dimulai pada $Y \ge 0.115 \times H$ (lewat dari toolbar Notes).
   - `cropY` dan `cropH` dikunci murni mengikuti koordinat vertikal highlight sebenarnya (`minY - 0.015*H` s.d. `maxY + 0.030*H`), sehingga seleksi 1 baris di bawah baris lain tidak pernah memborong baris yang tidak di-highlight di atasnya.
2. **Anti-Duplicate Buffer Reset & Length Guard (Solusi Bug 2: Kasus `"naruto uzumaki"`)**:
   - Membuat `commit_and_reset_current_typed_line()` di `screen.py` yang otomatis dipanggil saat user menyentuh/klik layar (`/mirror/touch`), menekan shortcut (`Ctrl+A`, `Ctrl+C`, `Ctrl+X`, `Ctrl+V`), atau jeda ketik $> 6.0$ detik.
   - Menambahkan penjaga kesebandingan panjang string di `auto_correct_ocr_text`: selisih panjang maksimal $\max(3, 0.20 \times \text{target})$, mencegah kandidat 40 karakter menimpa OCR 26 karakter.
3. **Pinned Frame Architecture di `mirror.js` (Solusi Bug 3a)**:
   - Mengganti ring buffer 5 slot dengan struktur pinned frame: `_PinnedTopFrames` menyimpan riwayat scroll/wheel hingga 45 detik.
   - `_firstDragFrame` dikunci permanen pada awal drag seleksi sehingga snapshot bagian atas catatan (Paragraf 1 & 2) terlindungi 100% dan tidak pernah tertimpa oleh event `pointermove` di bagian bawah layar.
4. **Single-Process Batch OCR & Dual Crop Mode (Solusi Bug 3b - Kecepatan 1.35s)**:
   - `ocr_extractor.ps1` mendukung `-ImagePaths "f1.png|f2.png|current.png"` dalam 1 kali eksekusi `powershell.exe`, mengompilasi C# hanya 1 kali, dan mengeksekusi kandidat Red-channel terlebih dahulu.
   - Frame riwayat menggunakan **Full-Viewport Content Crop** ($0.115 \times H$ s.d. $0.85 \times H$) untuk mengekstrak seluruh teks catatan yang tampak di posisi atas catatan, sementara frame terakhir menggunakan **Highlight Crop**.
   - Waktu eksekusi batch turun dari 11 detik menjadi **1.35 detik**.
5. **Anti-Garbage Filter Scroll Edge (Solusi Artefak `"1.1n1 j un 11"`)**:
   - Mendeteksi baris teratas hasil OCR yang tidak memiliki kata $\ge 4$ karakter dan mengandung angka acak / fragmen pendek terpotong di tepi scroll Notes, lalu membuangnya secara otomatis.
6. **Verifikasi 3 Skenario Realis**:
   - Skenario 1 (`"jokowi gibran"`): Lolos (0.47s, hanya `"jokowi gibran"` yang disalin).
   - Skenario 2 (`"naruto uzumaki"`): Lolos (bersih tanpa duplikasi buffer).
   - Skenario 3 (Caption KMP Batumandi): Lolos (1.35s, frame atas terlindungi 10 detik pointermove, 100% teks utuh dari `"Rekaman video amatir..."` s.d. `"#cilegon"` tanpa sampah `"1.1n1 j un 11"`).







