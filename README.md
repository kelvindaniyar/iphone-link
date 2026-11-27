<div align="center">

<a href="https://kelvindaniyar.github.io/iphone-link/">
  <img src="assets/logo.png" alt="iPhone Link Logo" width="116" height="116">
</a>

<h1>iPhone Link v5.0.2</h1>
<p><strong>Your iPhone screen, native on Windows.</strong><br>100% Pure Native Rust Desktop (Slint + DirectComposition) - Zero WebView2, Zero Python, Zero PowerShell</p>

<p>
  <a href="#unduh--instalasi"><img src="https://img.shields.io/badge/Version-v5.0.2_Native_Rust-0078D4?style=flat-square" alt="Version v5.0.2 Native Rust"></a>
  <a href="#perbandingan-performa-v400-vs-v502"><img src="https://img.shields.io/badge/Engine-Pure_Rust_Zero_WebView2-10B981?style=flat-square" alt="Engine Pure Rust Zero WebView2"></a>
  <a href="#kompatibilitas"><img src="https://img.shields.io/badge/Windows-10_%26_11_(64--bit)-0078D4?style=flat-square" alt="Windows 10 and 11 64-bit"></a>
  <a href="#kompatibilitas"><img src="https://img.shields.io/badge/iOS_27+-Full_USB_%26_Wi--Fi_Control-10B981?style=flat-square" alt="iOS 27+ Full USB and Wi-Fi Control"></a>
  <a href="https://github.com/kelvindaniyar/iphone-link/releases"><img src="https://img.shields.io/github/downloads/kelvindaniyar/iphone-link/total?label=Downloads&color=0EA5E9&style=flat-square" alt="Total Downloads"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-GPL--3.0-2563EB?style=flat-square" alt="License GPL-3.0"></a>
</p>

<p>
  <a href="#kompatibilitas"><img src="https://img.shields.io/badge/Tested_iOS-iOS_27.2-16A34A?style=flat-square" alt="Tested iOS 27.2"></a>
  <a href="#kompatibilitas"><img src="https://img.shields.io/badge/Tested_Devices-iPhone_17_Pro_%26_iPhone_11_Pro_Max-0284C7?style=flat-square" alt="Tested Devices iPhone 17 Pro and iPhone 11 Pro Max"></a>
  <a href="https://github.com/kelvindaniyar"><img src="https://img.shields.io/badge/Creator-Kelvin_Daniyar-1E293B?style=flat-square" alt="Creator Kelvin Daniyar"></a>
</p>

<p>
  <a href="https://github.com/kelvindaniyar/iphone-link/releases/tag/v5.0.2"><img src="https://img.shields.io/badge/Download_v5.0.2-0078D4?style=for-the-badge&logo=windows&logoColor=white" alt="Download v5.0.2"></a>
  <a href="https://kelvindaniyar.github.io/iphone-link/"><img src="https://img.shields.io/badge/Official_Website-0EA5E9?style=for-the-badge&logo=googlechrome&logoColor=white" alt="Official Website"></a>
  <a href="https://saweria.co/kelvindaniyar"><img src="https://img.shields.io/badge/Support_on_Saweria-F59E0B?style=for-the-badge&logo=buymeacoffee&logoColor=white" alt="Support on Saweria"></a>
</p>

</div>

---

**iPhone Link v5.0.2** is a **free, open-source (GPL-3.0) macOS-style iPhone and iPad screen mirroring and remote control app for Windows 10/11** (often described as **scrcpy for iOS / iPhone**), created by **Kelvin Daniyar**. Rebuilt in **100% Pure Native Rust** (Slint + DirectComposition, **Zero WebView2**, **Zero Python**), it runs as a single lightweight process (**~45 MB idle RAM**, **~79 MB live mirroring**) on stock non-jailbroken devices: **full 60fps USB & Wi-Fi HEVC with full mouse, keyboard, trackpad, OCR copy-paste, Stealth Dim, & Windows DPAPI PIN unlock on iOS 27+** (physically tested on **iOS 27.2** on **iPhone 17 Pro & iPhone 11 Pro Max**), plus a built-in **AirPlay H.264 Receiver (View & Audio Only)** on **iOS 17-26**. Visit the [official website](https://kelvindaniyar.github.io/iphone-link/) or read [llms.txt](https://kelvindaniyar.github.io/iphone-link/llms.txt).

> [!IMPORTANT]
> **Rilis `v5.0.2` (USB DDI iPhone 11-14, Wi-Fi Tanpa Python, & Zero Black Bar Multi-iPhone):** `iPhone Link.exe` langsung menjalankan UI Native Rust (**Slint + Windows DirectComposition**) tanpa `WebView2Loader.dll` maupun `msedgewebview2.exe`. Rilis **`v5.0.2`** membuat pemasangan awal iPhone 11-14 di iOS 27 via USB lancar seketika, menghadirkan pairing Wi-Fi native tanpa Python, menghilangkan garis hitam tepi secara otomatis dari **iPhone 11 Pro Max hingga 17 Pro**, serta menjaga sesi USB tetap stabil dan bebas UDID mentah di layar.

---

## Apa yang Baru di `v5.0.2`

Colok kabel, langsung tampil. Kini iPhone 11-14 di iOS 27 tersambung lebih cepat via USB, Wi-Fi 60fps aktif tanpa aplikasi tambahan, dan tampilan layar otomatis penuh ujung ke ujung dari iPhone 11 Pro Max hingga 17 Pro.

- **Koneksi USB Instan untuk iPhone 11-14 (`iOS 27`) & Detektor Konflik 3uTools:**
  - *Sliding-window TCP* `CdTcpRw` (`MSS=1340, WScale=6` dengan retransmisi 3-DupACK/RTO), permintaan Apple TSS HTTPS-first non-blocking, batas watchdog khusus DDI `45s/120s` dengan progres live `71%..90%`, dan detektor proses bentrok USB (`3uTools.exe`, `iTunes.exe`, `AppleDevices.exe`) + perbaikan layanan `Apple Mobile Device Service`.
- **Pairing Wi-Fi Murni Native Rust (Zero Python) & Stabilitas USB Sesi Panjang:**
  - Implementasi native `RPPairing` (`SRP-6a 3072-bit` + `OPACK` `altIRK` + `ChaCha20-Poly1305`) langsung di Rust tanpa Python/`pymobiledevice3` sehingga cukup 1-klik **Atur Wi-Fi** lalu cabut kabel, disertai mode **Zero-Poll** saat stream USB berjalan, respons cabut kabel `< 1s`, pemeliharaan `last_ip` Wi-Fi, badge jujur `USB` vs `USB + Wi-Fi`, & penyensoran otomatis UDID mentah di UI (`Zero Raw-UDID`).
- **Layar Penuh Ujung ke Ujung (`0 px` Black Bar) Multi-Generasi iPhone:**
  - Rekonsiliasi dinamis MFT Output Aperture (`MINIMUM_DISPLAY` / `GEOMETRIC` / `PAN_SCAN`) dengan SPS *conformance window* (`0` pemotongan ganda), detektor *baked padding* adaptif `HevcEdgePadDetector` (`<= 96 px` pada 2 strip mini BGRA8, aman bagi iPad 4:3 maupun YouTube Dark Mode, dan bebas hardcode model), kompensasi sentuh HID saat *manual crop*, telemetri `[video_geom]` Zero-PII, & pembersihan decoder saat *hot-switch* antar-iPhone.

---

## Perbandingan Performa (`v4.0.0` vs `v5.0.2`)

| Metrik (Idle & Mirroring 60 FPS) | `v4.0.0` (Embedded WebView2) | `v5.0.2` (100% Native Rust Zero-WebView2) |
|---|---:|---:|
| **Proses Aplikasi Aktif** | 6-8 proses (`iPhone Link.exe` + `msedgewebview2.exe`) | **1 proses tunggal** (`0` proses WebView2) |
| **Child HWND Tambahan** | Multi-HWND browser host | **`0` child HWND** (DirectComposition visual tree) |
| **RAM Standby (Idle)** | ~210-290 MB | **~43.6 MB** (Working Set terisolasi UI < 15 MB saat selftest) |
| **RAM Live Mirroring 60 FPS + Audio BT** | ~320-420 MB | **~74-78 MB** |
| **Penggunaan CPU Standby** | 2-5% | **< 1.5%** |
| **Waktu Cold Boot** | ~1.5-2.5 detik (menunggu WebView2 init) | **Instan (< 0.3 detik)** |
| **Dependensi Runtime Eksternal** | WebView2 Runtime / legacy helper | **Mandiri (`0` Python, `0` PowerShell, `0` WebView2)** |

---

## Fitur Utama

- **Konektivitas Fleksibel Sesuai Versi iOS:**
  - **iOS 27+ - Full Control (USB & Wi-Fi HEVC 60 FPS):** Streaming HD 60fps tanpa batas dengan kontrol penuh mouse, keyboard (`Alt+V`), trackpad (`30%-200%`), Auto-Unlock PIN DPAPI (`Alt+U`), Stealth Dim (`Alt+D`), & OCR (`Alt+C`). `Setup.exe` sudah membundel driver USB Apple & codec HEVC offline.
  - **iOS 17-26 - AirPlay H.264/AAC (View & Audio Only):** Receiver AirPlay HD bawaan via Wi-Fi lokal dengan shader GPU NV12 BT.709, `zlaac.dll`, dan `playfair.dll` untuk tampilan layar & suara ke PC (tanpa kontrol input dari PC karena Apple membatasi protokol CoreDevice `DisplayService` ke **iOS 27+**).
- **Always-On True Frosted Thin Acrylic Blur (`Windows 10 & 11`):**
  - Windows 11 native **`DWMSBT_TRANSIENTWINDOW (3)`** tetap aktif meskipun jendela tidak fokus (`WM_NCACTIVATE` patch) serta *drag-guard* **`SWCA`** untuk Windows 10.
  - Mode **Dark Smoky**, **Light Crystal (Putih Susu)**, serta **Fullscreen Portrait Frosted Wings** di sisi kiri/kanan kanvas potret saat layar penuh.
- **Transisi Seamless macOS-Sequoia Start & Stop:**
  - *Cold-start frosted translucent reveal* pada koneksi awal dan *warm-start last-frame blur cache* (`sequoia_last_blur.bin`, lokal & otomatis dibersihkan oleh uninstaller) untuk transisi blur-ke-tajam tanpa kotak buram datar.
- **Auto-Rotate 32 Skenario & Frame Penuh Dinamis:** Sinkronisasi otomatis orientasi potret / lanskap mengikuti iPhone/iPad pada jendela maupun layar penuh tanpa black bar di seluruh generasi iPhone (iPhone X/11/12/13/14/15/16/17 Pro) maupun iPad 4:3.
- **Kontrol Input Presisi Tinggi (`iOS 27+`) & Quick Actions:**
  - **Trackpad Gestures (`30%-200%`)** dengan kurva kecepatan terkalibrasi (mode lambat tanpa fling berlebih) + **Keyboard HID** (termasuk `Alt+V` Paste langsung ke iPhone).
  - *Quick Actions & Hotkeys*: Stealth Dim (`Alt+D`), Lock (`Alt+L`), Unlock PIN (`Alt+U`, terenkripsi Windows DPAPI lokal), Multi-Frame OCR (`Alt+C`), LAN Shortcut iOS Receiver (`Alt+B` port `8765`), Audio Mute (`Alt+M`), **1-Klik AMFI Developer Mode**, & **Salin Log Diagnostik Zero-PII**.
- **Audio Sink Otomatis + Bluetooth A2DP:** Auto-start sinkronisasi audio saat mirroring dimulai dengan status sinkron antara Top Bar (`Alt+M`) dan kartu kontrol audio.
- **WinHTTP In-App Auto-Updater:** Mulai `v5.0.0`, pemeriksaan dan pemasangan pembaruan rilis resmi berjalan otomatis di dalam aplikasi via WinHTTP native.

---

## Panduan Upgrade (`v5.0.0` / `v5.0.1` / `v4.0.0` -> `v5.0.2`)

> [!NOTE]
> - **Pengguna `v5.0.0` / `v5.0.1`:** Anda dapat langsung menekan **Periksa Pembaruan** (*Check for Updates*) di dalam aplikasi untuk naik ke **`v5.0.2`**, atau mengunduh ulang `iPhone-Link-v5.0.2-Setup.exe`.
> - **Pengguna `v4.0.0` (upgrade manual `1x`):** Tutup **iPhone Link `v4.0.0`**, lalu jalankan **`iPhone-Link-v5.0.2-Setup.exe`** (direkomendasikan) atau ekstrak **`iPhone-Link-Win64-Native-Rust-v5.0.2.zip`** ke folder bersih. Pengaturan & PIN lama tetap termigrasi otomatis.

---

## Kompatibilitas

| Platform | Dukungan | Catatan Jujur |
|---|---|---|
| **iPhone / iPad (`iOS 27+`)** | **Full USB & Wi-Fi 60 FPS + Full Control** | **Teruji fisik pada iPhone 17 Pro (`iOS 27.2`) & iPhone 11 Pro Max (`iOS 27.2.1`)** - USB Zero-Copy HEVC, Wi-Fi 60fps tanpa Python, `0 px` black bar, mouse, keyboard, trackpad `30%-200%`, Stealth Dim, OCR, & DPAPI Unlock PIN |
| **iPhone / iPad (`iOS 17-26`)** | **AirPlay H.264 (View & Audio Only)** | Tampilan layar HD + audio ke PC melalui Wi-Fi lokal (AirPlay Receiver); **tanpa kontrol mouse/keyboard PC** karena Apple membatasi CoreDevice DisplayService pada iOS 27+ |
| **Windows 11 (64-bit)** | **Penuh** | Native `DWMSBT_TRANSIENTWINDOW` Frosted Acrylic + HEVC bawaan |
| **Windows 10 (64-bit)** | **Penuh** | `Setup.exe` sudah membundel **HEVC Video Extension** & **Apple USB Driver** offline; pengguna **Portable ZIP** tanpa codec HEVC dapat memilih pasang HEVC gratis atau **AirPlay (H.264)** via Kartu Keputusan jujur |

---

## Unduh & Instalasi

Unduh aset resmi rilis **`v5.0.2`** dari halaman [GitHub Releases](https://github.com/kelvindaniyar/iphone-link/releases/tag/v5.0.2):

1. **`iPhone-Link-v5.0.2-Setup.exe`** *(1-Click Offline Installer - Direkomendasikan, `51.67 MB` / `54,178,170 B`)*
   - Berisi `iPhone Link.exe` (Zero-WebView2, `18.86 MB` / `19,773,440 B`), `zlaac.dll`, `playfair.dll`, **Apple Mobile Device Support (`AppleMobileDeviceSupport64.msi`)**, dan **Microsoft HEVC Video Extension**.
2. **`iPhone-Link-Win64-Native-Rust-v5.0.2.zip`** *(Portable ZIP - 10 Berkas Bersih, `9.31 MB` / `9,763,611 B`)*
   - Berisi `iPhone Link.exe`, `zlaac.dll`, `playfair.dll`, aset ikon, `README.md`, `LICENSE`, `Create-Desktop-Shortcut.bat`, dan `Uninstall-iPhone-Link.bat` (`0` `WebView2Loader.dll`).
   - Ekstrak ke folder baru lalu klik ganda **`iPhone Link.exe`**.

*(Nilai verifikasi SHA-256 resmi untuk ketiga berkas rilis tercantum pada halaman **GitHub Releases v5.0.2** dan website resmi **https://kelvindaniyar.github.io/iphone-link/**.)*

---

## Support This Project & Atribusi

<div align="center">

<p>Jika <strong>iPhone Link v5.0.2</strong> membantu alur kerja Anda, pertimbangkan untuk mendukung pengembangan lanjutan proyek ini:</p>

<p>
  <a href="https://saweria.co/kelvindaniyar"><img src="https://img.shields.io/badge/Support_on_Saweria-F59E0B?style=for-the-badge&logo=buymeacoffee&logoColor=white" alt="Support on Saweria"></a>
</p>

</div>

- **Creator & Maintainer:** [**Kelvin Daniyar (`@kelvindaniyar`)**](https://github.com/kelvindaniyar) | [Official Website](https://kelvindaniyar.github.io/iphone-link/) | [Instagram](https://www.instagram.com/kelvin.daniyar/) | [TikTok](https://www.tiktok.com/@kelvindaniyar) | [Saweria](https://saweria.co/kelvindaniyar)
- **Upstream & Kredit Open-Source (GPL-3.0):**
  - [`teyboxv1/pane-iphone-mirror-windows`](https://github.com/teyboxv1/pane-iphone-mirror-windows) - Arsitektur awal proyek mirror Windows.
  - [`jkcoxson/idevice`](https://github.com/jkcoxson/idevice) - Referensi ekosistem Rust iOS device & RemoteXPC/USBMuxD.
  - [`doronz88/pymobiledevice3`](https://github.com/doronz88/pymobiledevice3) - Referensi protokol RemoteXPC, USBMuxD, Lockdown, & HID iOS.
- **Lisensi:** Dirilis di bawah lisensi **GNU General Public License v3.0 ([LICENSE](LICENSE))**.
