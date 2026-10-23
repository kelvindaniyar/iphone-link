# LAPORAN TAHAP 16: INSTANT 2-FINGER GESTURES, PENYIMPANAN PERMANEN CROP ("SAVE CROP"), DAN FITUR AUDIO SINK (IPHONE ➔ PC SPEAKER)

**Tanggal:** 23 Oktober 2026  
**Commit Referensi:** `05ff850`  
**Status Pengujian:** Lolos Uji Coba Unit & Endpoint, Siap Dijalankan Kelvin

---

## 1. Ringkasan Eksekutif & Solusi Masalah

Tahap 16 menghadirkan 3 pembaruan besar yang diminta oleh Kelvin:

| No | Fitur / Masalah | Solusi Teknis yang Diterapkan di Commit `05ff850` |
|---|---|---|
| **1** | Gestur trackpad 2 jari lambat / delay | 1. `canvas.getBoundingClientRect()` di-cache di dalam `cachedCanvasRect` (hanya diperbarui saat resize/crop), menghilangkan *forced synchronous layout reflow* pada setiap pointer event.<br>2. **Leading-edge Horizontal Swipe**: Saat gerakan horizontal terdeteksi dominan (`Math.abs(e.deltaX) > Math.abs(e.deltaY) * 1.3`), gesture langsung ditembakkan di awal pergerakan tanpa debounce lama, dengan cooldown 260ms.<br>3. **Coalesced Non-blocking Vertical Scroll**: Menggunakan in-flight lock `isWheelSending` dan akumulator `pendingWheelY`. Event wheel digabungkan tanpa menumpuk request HTTP berulang. |
| **2** | Setting crop hilang setelah aplikasi ditutup | 1. Pada `handset/gui.py`, `webview.start` kini menggunakan `private_mode=False` dan `storage_path=Path.home() / ".pane_webview_storage"`.<br>2. Di `handset/server.py`, ditambahkan persistence ke file `user_settings.json` serta endpoint `GET /api/settings` dan `POST /api/settings`.<br>3. Di `index.html` & `app.js`, ditambahkan tombol **"💾 Save Crop"** (`#btn-save-crop`) dengan animasi feedback **"✓ Crop Saved!"** dan auto-load crop saat startup/iframe reload. |
| **3** | Fitur Audio Sink (iPhone ➔ PC Audio Routing) | 1. Modul baru `handset/audio.py` mengelola **Windows 11 Bluetooth A2DP Audio Sink (`AudioPlaybackConnection`)** melalui WinRT background helper.<br>2. Endpoint `POST /api/audio/toggle`, `POST /api/audio/volume` (native COM via `pycaw`), dan `POST /api/audio/pair` (`ms-settings:bluetooth`).<br>3. **Toggle Top Bar** (`#btn-audio-top`) dengan indikator audio aktif/glow hijau, serta **Card Audio Sink di Side Drawer** lengkap dengan slider volume dan status pairing perangkat iPhone. |

---

## 2. Rincian Modifikasi Kode

### A. Instant 2-Finger Trackpad Gesture & Layout Cache
- **File:** [`handset/web/mirror.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/mirror.js)
  - Layout Caching:
    ```javascript
    let cachedCanvasRect = null;
    function fitCanvas() {
      // ... set styles ...
      cachedCanvasRect = canvas.getBoundingClientRect();
    }
    function touchCoords(e) {
      const rect = cachedCanvasRect || canvas.getBoundingClientRect();
      // ... hitung koordinat tanpa reflow DOM ...
    }
    ```
  - Leading-Edge Fast Horizontal Swipe:
    ```javascript
    if (absX > absY * 1.3) {
      accWheelX += e.deltaX;
      window.clearTimeout(hSwipeTimer);
      hSwipeTimer = window.setTimeout(() => { accWheelX = 0; }, 200);
      const now = performance.now();
      if (Math.abs(accWheelX) >= 28 && now - lastHorizontalSwipeTime > 260) {
        const dir = accWheelX > 0 ? "left" : "right";
        accWheelX = 0;
        lastHorizontalSwipeTime = now;
        performFastHorizontalSwipe(dir);
      }
      return;
    }
    ```
  - Coalesced Non-Blocking Vertical Scroll:
    ```javascript
    pendingWheelY += e.deltaY;
    if (!isWheelSending && Math.abs(pendingWheelY) >= 18) {
      pumpVerticalWheel();
    }
    ```

---

### B. Penyimpanan Permanen Lintas Sesi ("Save Crop")
- **File:** [`handset/gui.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/gui.py)
  ```python
  storage_path = str(Path.home() / ".pane_webview_storage")
  os.makedirs(storage_path, exist_ok=True)
  webview.start(
      debug=False,
      private_mode=False,
      storage_path=storage_path,
  )
  ```
- **File:** [`handset/server.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/server.py)
  - File konfigurasi: `user_settings.json` di root folder aplikasi.
  - Endpoint `GET /api/settings`: mengembalikan `{ crop, fitMode, theme, audioSinkEnabled, audioVolume }`.
  - Endpoint `POST /api/settings`: menyimpan crop dan setelan ke disk secara atomik.
- **File:** [`handset/web/index.html`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/index.html) & [`handset/web/app.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/app.js)
  - Tombol **"💾 Save Crop"** (`#btn-save-crop`) disematkan di sebelah tombol Reset.
  - Saat diklik: menyimpan ke server dan `localStorage`, lalu menampilkan feedback visual **"✓ Crop Saved!"** selama 1.5 detik.
  - Pada inisialisasi aplikasi (`fetchUserSettings`), data crop otomatis diunduh dari server dan diterapkan ke 4 slider + 4 kotak angka dan di-post ke iframe `mirror.js`.

---

### C. Fitur Audio Sink (iPhone ➔ PC Speaker Routing)
- **File Baru:** [`handset/audio.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/audio.py)
  - Kelas `AudioManager`:
    * Menjalankan helper background PowerShell yang menginisialisasi `Windows.Media.Audio.AudioPlaybackConnection` untuk perangkat Bluetooth yang terhubung (iPhone `UWE UH`).
    * Membuka koneksi streaming A2DP Sink ke speaker PC Windows saat toggle di-set **ON**.
    * Mengontrol volume master Windows secara instan menggunakan `pycaw` dengan inisialisasi thread COM `CoInitialize`.
    * Membuka halaman Windows Bluetooth Settings via `os.startfile("ms-settings:bluetooth")`.
- **File:** [`handset/server.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/server.py)
  - Integrasi audio ke `/api/status`, serta endpoint `/api/audio/toggle`, `/api/audio/volume`, dan `/api/audio/pair`.
- **File:** [`handset/web/index.html`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/index.html) & [`handset/web/app.css`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/app.css)
  - **Top Bar Quick Toggle (`#btn-audio-top`)**: Tombol dengan ikon speaker yang menyala hijau saat audio aktif.
  - **Drawer Card "AUDIO SINK (IPHONE ➔ PC)"**:
    * Badge status: `"● Active (UWE UH)"` (hijau) atau `"○ Off / Muted"`.
    * Tombol Utama: `"Enable Audio Sink"` / `"Disable Audio Sink (Active)"`.
    * Slider Volume PC (`0% - 100%`) dengan teks persen real-time.
    * Tombol `"⚙️ Pair Bluetooth in Windows"` untuk pairing pertama kali.

---

## 3. Hasil Pengujian Unit & Endpoint

Skrip pengujian otomatis `scratch/test_endpoints.py` memverifikasi seluruh fungsionalitas:
1. `GET /api/settings` ➔ Berhasil mereturn struktur JSON lengkap.
2. `POST /api/settings` ➔ Berhasil menyimpan nilai crop permanen ke `user_settings.json`.
3. `GET /api/status` ➔ Objek status memuat informasi audio aktif & volume.
4. `POST /api/audio/toggle` (`enabled: true`) ➔ Berhasil mengaktifkan koneksi AudioPlaybackConnection untuk iPhone `"UWE UH"`.
5. `POST /api/audio/volume` (`volume: 85`) ➔ Berhasil mengatur volume master PC ke 85%.
6. `POST /api/audio/toggle` (`enabled: false`) ➔ Berhasil menonaktifkan dan melepaskan koneksi secara bersih.
7. Cache buster telah dinaikkan ke `v=20261023_tahap16_audiosink_savecrop`.
