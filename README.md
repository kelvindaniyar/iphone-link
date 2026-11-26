<div align="center">

<a href="https://kelvindaniyar.github.io/iphone-link/">
  <img src="assets/logo.png" alt="iPhone Link Logo" width="116" height="116">
</a>

<h1>iPhone Link v5.0.0</h1>
<p><strong>Your iPhone screen, native on Windows.</strong><br>100% Pure Native Rust Desktop (Slint + DirectComposition) - Zero WebView2, Zero Python, Zero PowerShell</p>

<p>
  <a href="#unduh--instalasi"><img src="https://img.shields.io/badge/Version-v5.0.0_Native_Rust-0078D4?style=flat-square" alt="Version v5.0.0 Native Rust"></a>
  <a href="#perbandingan-performa-v400-vs-v500"><img src="https://img.shields.io/badge/Engine-Pure_Rust_Zero_WebView2-10B981?style=flat-square" alt="Engine Pure Rust Zero WebView2"></a>
  <a href="#kompatibilitas"><img src="https://img.shields.io/badge/Windows-10_%26_11_(64--bit)-0078D4?style=flat-square" alt="Windows 10 and 11 64-bit"></a>
  <a href="#kompatibilitas"><img src="https://img.shields.io/badge/iOS_%26_iPadOS-17--27-111827?style=flat-square" alt="iOS and iPadOS 17-27"></a>
  <a href="https://github.com/kelvindaniyar/iphone-link/releases"><img src="https://img.shields.io/github/downloads/kelvindaniyar/iphone-link/total?label=Downloads&color=0EA5E9&style=flat-square" alt="Total Downloads"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-GPL--3.0-2563EB?style=flat-square" alt="License GPL-3.0"></a>
</p>

<p>
  <a href="#kompatibilitas"><img src="https://img.shields.io/badge/Tested_iOS-iOS_27.2-16A34A?style=flat-square" alt="Tested iOS 27.2"></a>
  <a href="#kompatibilitas"><img src="https://img.shields.io/badge/Tested_Devices-iPhone_17_Pro_%26_iPhone_11_Pro_Max-0284C7?style=flat-square" alt="Tested Devices iPhone 17 Pro and iPhone 11 Pro Max"></a>
  <a href="https://github.com/kelvindaniyar"><img src="https://img.shields.io/badge/Creator-Kelvin_Daniyar-1E293B?style=flat-square" alt="Creator Kelvin Daniyar"></a>
</p>

<p>
  <a href="https://github.com/kelvindaniyar/iphone-link/releases/tag/v5.0.0"><img src="https://img.shields.io/badge/Download_v5.0.0-0078D4?style=for-the-badge&logo=windows&logoColor=white" alt="Download v5.0.0"></a>
  <a href="https://kelvindaniyar.github.io/iphone-link/"><img src="https://img.shields.io/badge/Official_Website-0EA5E9?style=for-the-badge&logo=googlechrome&logoColor=white" alt="Official Website"></a>
  <a href="https://saweria.co/kelvindaniyar"><img src="https://img.shields.io/badge/Support_on_Saweria-F59E0B?style=for-the-badge&logo=buymeacoffee&logoColor=white" alt="Support on Saweria"></a>
</p>

</div>

---

**iPhone Link v5.0.0** is a **free, open-source (GPL-3.0) macOS-style iPhone and iPad screen mirroring and remote control app for Windows 10/11** (often described as **scrcpy for iOS / iPhone**), created by **Kelvin Daniyar**. Rebuilt in **100% Pure Native Rust** (Slint + DirectComposition, **Zero WebView2**, **Zero Python**), it runs as a single lightweight process (**~45 MB idle RAM**, **~79 MB live mirroring**) on stock non-jailbroken **iOS 17-27** and **Windows 10/11 64-bit** PCs. It delivers **Triple Connectivity** (**60fps USB HEVC**, **60fps Wi-Fi HEVC** with full mouse, keyboard, and trackpad control, plus a built-in **AirPlay Receiver**), **PC Audio (Bluetooth A2DP & AirPlay)**, **OCR Copy-Paste (`Alt+C`/`Alt+V`)**, **Stealth Dim (`Alt+D`)**, and a **Windows DPAPI encrypted local Auto-Unlock PIN**. Visit the [official website](https://kelvindaniyar.github.io/iphone-link/) or read the machine-readable [llms.txt](https://kelvindaniyar.github.io/iphone-link/llms.txt).

> [!IMPORTANT]
> **Rilis `v5.0.0` (Cutover Total Zero-WebView2):** Mengklik ganda `iPhone Link.exe` kini langsung menjalankan UI Native Rust (**Slint + Windows DirectComposition**). `WebView2Loader.dll` dan seluruh ketergantungan `msedgewebview2.exe` telah dicabut sepenuhnya dari paket distribusi resmi.

---

## Perbandingan Performa (`v4.0.0` vs `v5.0.0`)

| Metrik (Idle & Mirroring 60 FPS) | `v4.0.0` (Embedded WebView2) | `v5.0.0` (100% Native Rust Zero-WebView2) |
|---|---:|---:|
| **Proses Aplikasi Aktif** | 6-8 proses (`iPhone Link.exe` + `msedgewebview2.exe`) | **1 proses tunggal** (`0` proses WebView2) |
| **Child HWND Tambahan** | Multi-HWND browser host | **`0` child HWND** (DirectComposition visual tree) |
| **RAM Standby (Idle)** | ~210-290 MB | **~43.6 MB** (Working Set terisolasi UI ~0.6 MB saat selftest) |
| **RAM Live Mirroring 60 FPS + Audio BT** | ~320-420 MB | **~74-78 MB** |
| **Penggunaan CPU Standby** | 2-5% | **< 1.5%** |
| **Waktu Cold Boot** | ~1.5-2.5 detik (menunggu WebView2 init) | **Instan (< 0.3 detik)** |
| **Dependensi Runtime Eksternal** | WebView2 Runtime / legacy helper | **Mandiri (`0` Python, `0` PowerShell, `0` WebView2)** |

---

## Fitur Utama

- **Triple Connectivity Mirroring HD 60 FPS:**
  - **Wi-Fi HEVC (H.265) 60 FPS** via RemoteXPC / TLS pairing tanpa kabel (`iOS 17-27`).
  - **USB Zero-Copy HEVC** via USBMuxD + **Driver USB Apple resmi** yang sudah terpasang otomatis di `Setup.exe`.
  - **Hybrid AirPlay H.264/AAC Receiver** dengan shader GPU **`HdNv12ShaderRenderer`** (BT.709 full-range NV12 -> RGBA tanpa pecah warna), `zlaac.dll`, dan `playfair.dll`.
- **Always-On True Frosted Thin Acrylic Blur (`Windows 10 & 11`):**
  - Windows 11 native **`DWMSBT_TRANSIENTWINDOW (3)`** tetap aktif meskipun jendela tidak fokus (`WM_NCACTIVATE` patch) serta *drag-guard* **`SWCA`** untuk Windows 10.
  - Mode **Dark Smoky**, **Light Crystal (Putih Susu)**, serta **Fullscreen Portrait Frosted Wings** di sisi kiri/kanan kanvas potret saat layar penuh.
- **Transisi Seamless macOS-Sequoia Start & Stop:**
  - *Cold-start frosted translucent reveal* pada koneksi awal dan *warm-start last-frame blur cache* (`sequoia_last_blur.bin`, lokal & otomatis dibersihkan oleh uninstaller) untuk transisi blur-ke-tajam tanpa kotak buram datar.
- **Auto-Rotate 32 Skenario:** Sinkronisasi otomatis orientasi potret / lanskap mengikuti iPhone/iPad pada jendela maupun layar penuh dengan sinkronisasi frame atomik.
- **Kontrol Input Presisi Tinggi:**
  - **Trackpad Gestures (`30%-200%`)** dengan kurva kecepatan terkalibrasi (mode lambat tanpa fling berlebih) + **Keyboard HID** (termasuk `Alt+V` Paste langsung ke iPhone).
  - *Quick Actions & Hotkeys*: Stealth Dim (`Alt+D`), Lock (`Alt+L`), Unlock PIN (`Alt+U`, terenkripsi Windows DPAPI lokal), Multi-Frame OCR (`Alt+C`), LAN Shortcut iOS Receiver (`Alt+B` port `8765`), Audio Mute (`Alt+M`), dan **1-Klik AMFI Developer Mode** untuk iOS 17+.
- **Audio Sink Otomatis + Bluetooth A2DP:** Auto-start sinkronisasi audio saat mirroring dimulai dengan status sinkron antara Top Bar (`Alt+M`) dan kartu kontrol audio.
- **WinHTTP In-App Auto-Updater:** Mulai `v5.0.0`, pemeriksaan dan pemasangan pembaruan rilis resmi berjalan otomatis di dalam aplikasi via WinHTTP native.

---

## Panduan Upgrade (`v4.0.0` -> `v5.0.0`)

> [!NOTE]
> **Upgrade dari `v4.0.0` ke `v5.0.0` memerlukan unduh manual `1x`:**
> Pembaru lama di `v4.0.0` publik masih bergantung pada skema lama sehingga tidak dapat melakukan migrasi otomatis ke mesin Pure Native Rust `v5.0.0`.
> 1. Tutup **iPhone Link `v4.0.0`** yang sedang berjalan.
> 2. Unduh dan jalankan **`iPhone-Link-v5.0.0-Setup.exe`** (direkomendasikan) atau ekstrak **`iPhone-Link-Win64-Native-Rust-v5.0.0.zip`** ke folder bersih.
> 3. Pengaturan Anda (`user_settings.json`, `wifi_state.json`, dll.) tetap kompatibel. Mulai **`v5.0.0` -> `v5.1.0+`**, seluruh pembaruan berikutnya berjalan **100% otomatis di dalam aplikasi**.

---

## Kompatibilitas

| Platform | Dukungan | Catatan |
|---|---|---|
| **iPhone / iPad** | **iOS / iPadOS 17 - 27** | **Teruji fisik pada iPhone 17 Pro & iPhone 11 Pro Max (iOS 27.2)**; mendukung Wi-Fi RemoteXPC HEVC, USB Wire, & AirPlay Fallback |
| **Windows 11 (64-bit)** | **Penuh** | Native `DWMSBT_TRANSIENTWINDOW` Frosted Acrylic + HEVC bawaan |
| **Windows 10 (64-bit)** | **Penuh** | `Setup.exe` sudah membundel **HEVC Video Extension** & **Apple USB Driver** offline; pengguna **Portable ZIP** di Windows 10 yang belum memasang codec HEVC dapat memakai mode fallback **AirPlay (H.264)** secara otomatis atau memasang HEVC Video Extension |

---

## Unduh & Instalasi

Unduh aset resmi rilis **`v5.0.0`** dari halaman [GitHub Releases](https://github.com/kelvindaniyar/iphone-link/releases/tag/v5.0.0):

1. **`iPhone-Link-v5.0.0-Setup.exe`** *(1-Click Offline Installer - Direkomendasikan, `51.58 MB`)*
   - Berisi `iPhone Link.exe` (Zero-WebView2), `zlaac.dll`, `playfair.dll`, **Apple Mobile Device Support (`AppleMobileDeviceSupport64.msi`)**, dan **Microsoft HEVC Video Extension**.
2. **`iPhone-Link-Win64-Native-Rust-v5.0.0.zip`** *(Portable ZIP - 10 Berkas Bersih, `9.16 MB`)*
   - Berisi `iPhone Link.exe`, `zlaac.dll`, `playfair.dll`, aset ikon, `README.md`, `LICENSE`, `Create-Desktop-Shortcut.bat`, dan `Uninstall-iPhone-Link.bat` (`0` `WebView2Loader.dll`).
   - Ekstrak ke folder baru lalu klik ganda **`iPhone Link.exe`**.

*(Nilai verifikasi SHA-256 resmi untuk kedua berkas rilis tercantum pada halaman **GitHub Releases v5.0.0** dan website resmi **https://kelvindaniyar.github.io/iphone-link/**.)*

---

## Support This Project & Atribusi

<div align="center">

<p>Jika <strong>iPhone Link v5.0.0</strong> membantu alur kerja Anda, pertimbangkan untuk mendukung pengembangan lanjutan proyek ini:</p>

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
