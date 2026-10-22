# LAPORAN TAHAP 6: NATIVE FRAMELESS PYWEBVIEW, SIDE-DRAWER SETTINGS, PRECISION TRACKPAD SCROLL & GESTURE SWIPE NAV

> **Klien:** Kelvin (Content Creator)  
> **Fokus Tahap:** Native Desktop Window (pywebview), Side-Drawer Settings, Precision Trackpad Scroll, dan Tombol Gestur Swipe  
> **Branch Git:** `feature/macos-sequoia-ui`  
> **Commit Hash:** `7c4871f` (`feat(app): add pywebview frameless launcher, side-drawer settings, precision trackpad scroll, and swipe nav`)  
> **Tanggal:** 23 Oktober 2026  

---

## 📌 RINGKASAN EKSEKUTIF

Tahapan ini merupakan lompatan besar dari aplikasi web browser biasa (`--app=chrome.exe`) menjadi **aplikasi desktop native Windows 11 murni** berbasis Microsoft Edge WebView2 (`pywebview`).

Semua 5 poin spesifikasi yang diminta Kelvin telah diimplementasikan secara menyeluruh:
1. **Navigasi Gestur Baru**: Mengganti tombol Back tunggal dengan susunan 4 tombol: `[ ◀ Swipe Left · ● Home · ▢ Recent · Swipe Right ▶ ]`, terhubung langsung ke fungsi `swipe('left')` dan `swipe('right')`.
2. **Native Frameless Window (pywebview)**: Modul `pywebview` terpasang di venv, launcher `handset/gui.py` dibuat dengan window borderless, Top Bar drag region (`-webkit-app-region: drag;`), kontrol jendela native (Minimize & Close), dan `run.bat` diperbarui.
3. **Side-Drawer Settings (Tidak Menutupi Layar)**: Panel Settings tidak lagi menjadi modal popover yang menutupi video. Settings kini berupa **Side Drawer di sebelah kanan**. Saat tombol ⚙️ diklik, jendela membesar secara dinamis (380px → 740px), video iPhone di sisi kiri tetap aktif 100%, dan settings tampil leluasa di kanan.
4. **Full Size & Scaling Responsif**: Tombol "Full Size" di Top Bar untuk beralih ke mode fullscreen/maximize dengan video iPhone otomatis diskalakan mengikuti tinggi monitor secara proporsional.
5. **Precision Trackpad Scroll & Matikan Klik Kanan**: Context menu browser dimatikan (`e.preventDefault()`), dan event `wheel` (scroll 2 jari trackpad laptop) diterjemahkan menjadi touch drag sintetis HID bertahap ke `/mirror/touch`, memungkinkan scroll feed/reels Instagram dan swipe foto horizontal langsung lewat trackpad tanpa klik-tahan mouse.

---

## 1. Rincian Implementasi per Fitur

### A. Tombol Navigasi Bawah Gestur Swipe
- **Susunan Baru**:
  `[ ◀ Swipe Left ]  ·  [ ● Home ]  ·  [ ▢ Recent ]  ·  [ Swipe Right ▶ ]`
- **Mekanisme Eksekusi**:
  - Tombol **◀ Swipe Left**: Menjalankan `swipe('left')` via atribut `data-swipe="left"`. Menggerakkan usapan layar dari kanan ke kiri untuk berpindah foto carousel atau swipe story.
  - Tombol **● Home**: Menjalankan gesture home ke SpringBoard iOS.
  - Tombol **▢ Recent**: Menjalankan gesture app switcher untuk membuka multitasking tray.
  - Tombol **Swipe Right ▶**: Menjalankan `swipe('right')` via atribut `data-swipe="right"`. Menggerakkan usapan layar dari kiri ke kanan (berfungsi juga sebagai swipe gesture back alami di iOS!).

---

### B. Transisi ke Native Frameless Window (`pywebview`)
- **Instalasi Paket**:
  `pywebview 6.2.1` dan dependensi Windows WebView2 (`pythonnet`, `clr_loader`, `bottle`) berhasil diinstall ke virtual environment `.\venv\Scripts\pip install pywebview`.
- **Launcher Baru [`handset/gui.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/gui.py)**:
  - Dibuat dengan konfigurasi:
    - `title="iPhone Mirroring"`
    - `url="http://127.0.0.1:8765"`
    - `width=380, height=840`
    - `frameless=True`
    - `easy_drag=True`
  - Mengekspos API bridge Python ke JavaScript (`window.pywebview.api`):
    - `minimize()`: meminimalkan jendela ke taskbar.
    - `close()`: menutup jendela dan aplikasi.
    - `toggle_fullscreen()`: beralih ke mode layar penuh.
    - `set_drawer(is_open)`: mengubah lebar jendela secara dinamis dari `380px` ke `740px` saat drawer dibuka, dan sebaliknya saat ditutup.
- **Top Bar Window Controls & Drag Region**:
  - Top Bar memiliki class `pywebview-drag-region` dan CSS `-webkit-app-region: drag;` sehingga jendela frameless dapat digeser bebas di desktop Windows.
  - Tombol-tombol di Top Bar memiliki CSS `-webkit-app-region: no-drag;` agar klik tombol responsif.
  - Sisi kanan Top Bar dilengkapi tombol kontrol native Windows 11:
    - Tombol Minimize (`─`)
    - Tombol Full Size (`⛶`)
    - Tombol Close (`✕`) beraksen merah saat di-hover (`#c42b1c`).
- **Pembaruan [`run.bat`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/run.bat)**:
  - Menggantikan pemanggilan `chrome.exe` dengan eksekusi langsung:
    ```cmd
    .\venv\Scripts\python handset/gui.py
    ```

---

### C. Layout Side-Drawer untuk Settings (Tidak Menutupi Layar)
- **Desain Sebelumnya**: Popover modal yang muncul menutupi layar video iPhone.
- **Desain Baru (Side Drawer)**:
  - Struktur horizontal flex: `[ Kolom Display iPhone (380px) ] + [ Kolom Side Drawer (360px) ]`.
  - **Saat tombol ⚙️ diklik**:
    1. Drawer di sebelah kanan muncul dengan mulus.
    2. Jendela aplikasi otomatis melebar dari **380px menjadi 740px** via API `window.pywebview.api.set_drawer(true)`.
    3. Layar video iPhone di sebelah kiri tetap aktif 100% dan tidak tertutup sama sekali.
    4. Pengaturan koneksi (Cable/Wi-Fi), tombol Start Mirroring, gestures, info perangkat, dan panduan tampil leluasa di sebelah kanan.
  - **Saat tombol ⚙️ atau ✕ diklik kembali**:
    1. Drawer menutup.
    2. Jendela kembali ramping ke ukuran awal **380px**.

---

### D. Fitur Full Size & Scaling Responsif
- **Tombol Full Size di Top Bar (`#btn-fullsize`)**:
  - Mengaktifkan mode fullscreen native (`window.pywebview.api.toggle_fullscreen()`) dan kelas CSS `.app.is-fullsize`.
  - Pada mode Full Size:
    - Video iPhone otomatis diskalakan mengikuti tinggi monitor monitor (`height: calc(100vh - 80px); aspect-ratio: 9 / 19.5; margin: 0 auto;`).
    - Latar belakang hitam pekat dan bersih, video berada persis di tengah layar tanpa distorsi rasio aspek.
    - Tombol navigasi bawah tetap menempel rapi di bawah video.

---

### E. Precision Trackpad Scroll & Matikan Klik Kanan di [`mirror.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/mirror.js)
1. **Matikan Context Menu Browser**:
   ```javascript
   canvas.addEventListener("contextmenu", (e) => e.preventDefault());
   document.addEventListener("contextmenu", (e) => e.preventDefault());
   ```
   Klik kanan tidak lagi memunculkan menu popup browser yang mengganggu.
2. **Precision Trackpad 2-Finger Scroll & Swipe**:
   - Mendengarkan event `wheel` pada canvas video.
   - Mengakumulasikan pergerakan 2 jari trackpad (`e.deltaY` dan `e.deltaX`).
   - Menerjemahkan pergerakan ke touch drag sintetis HID iOS secara bertahap:
     - **Scroll 2 Jari ke Bawah (deltaY > 0)**: Mengusap layar ke atas untuk men-scroll feed Instagram, Reels, atau halaman web ke bawah.
     - **Scroll 2 Jari ke Atas (deltaY < 0)**: Mengusap layar ke bawah untuk men-scroll feed Instagram ke atas.
     - **Swipe 2 Jari Horizontal (deltaX)**: Mengusap layar secara horizontal untuk swipe postingan carousel atau stories Instagram.
   - Menggunakan sistem locking `wheelBusy` dan interpolasi 6-step gerakan berjarak 12ms agar scrolling terasa halus dan tidak membanjiri antrean backend.

---

## 2. Struktur Riwayat Git Commit

```
* 7c4871f (HEAD -> feature/macos-sequoia-ui) feat(app): add pywebview frameless launcher, side-drawer settings, precision trackpad scroll, and swipe nav
* fe1cea6 docs: add LAPORAN_WINUI3_LIQUID_GLASS report
* 74f98ef feat(ui): redesign theme to WinUI 3 Fluent Liquid Glass with edge-to-edge display
* c50f64f feat(ui): revise layout to compact snug container with top bar and docked nav
* 9864528 feat(ui): redesign for macOS Sequoia floating aesthetic, popover controls, /mirror/live alias, and add run.bat launcher
| * 85b49e9 (main) feat(web): add GlideX-style Quick Navigation bar and edge back swipe gesture
|/  
* fb79bfd chore: update .gitignore, fix license to GPL-3.0 and fix run guide typo
* de5e97d (origin/main) update
```

---

## 3. Daftar File yang Dimodifikasi

| No | File | Status | Keterangan |
|:---:|:---|:---:|:---|
| 1 | [`handset/gui.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/gui.py) | **Baru** | Launcher native frameless desktop berbasis `pywebview` dengan window bridge API. |
| 2 | [`run.bat`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/run.bat) | Modified | Diperbarui untuk mengeksekusi `.\venv\Scripts\python handset/gui.py`. |
| 3 | [`handset/web/index.html`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/index.html) | Modified | Top Bar drag region, window controls, Side Drawer kanan, dan 4 tombol nav gestur. |
| 4 | [`handset/web/app.css`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/app.css) | Modified | Styling WinUI 3 drag region, window buttons, horizontal drawer flex, dan full size mode. |
| 5 | [`handset/web/app.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/app.js) | Modified | Dynamic window resizing (380px <-> 740px), window controls, dan full size toggle. |
| 6 | [`handset/web/mirror.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/mirror.js) | Modified | Blokir contextmenu dan konversi wheel Precision Trackpad ke touch drag sintetis HID. |

---

## 4. Panduan Menjalankan & Verifikasi Manual oleh Kelvin

1. **Jalankan Aplikasi**:
   Cukup klik dua kali [`run.bat`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/run.bat).
2. **Verifikasi Jendela Native Frameless**:
   - Jendela akan terbuka tanpa bingkai browser Chrome (murni native frameless window).
   - Klik dan geser (drag) Top Bar untuk memindahkan jendela di layar monitor.
   - Coba tombol Minimize (`─`) dan Close (`✕`) di pojok kanan atas.
3. **Verifikasi Side Drawer Settings**:
   - Klik tombol Gear (⚙️) di Top Bar.
   - Jendela akan melebar secara otomatis ke samping (380px → 740px).
   - Panel settings muncul di sisi kanan, sementara layar video iPhone di sisi kiri tetap aktif 100%.
   - Klik tombol ⚙️ lagi untuk menutup drawer dan melihat jendela kembali ke ukuran 380px.
4. **Verifikasi Tombol Gestur Bawah**:
   - Mulai mirroring.
   - Coba tombol `◀ Swipe Left` dan `Swipe Right ▶` untuk berpindah foto carousel atau story.
   - Coba tombol `● Home` dan `▢ Recent`.
5. **Verifikasi Precision Trackpad Scrolling**:
   - Di laptop kamu, letakkan 2 jari di trackpad di atas canvas video iPhone, lalu usap ke atas dan ke bawah.
   - Feed Instagram / Reels akan ter-scroll secara halus langsung via trackpad tanpa perlu klik-tahan mouse!
   - Coba usap 2 jari ke kiri atau ke kanan untuk swipe horizontal.
6. **Verifikasi Full Size**:
   - Klik tombol Full Size (`⛶`) di Top Bar untuk melihat layar iPhone mengisi monitor secara proporsional.
