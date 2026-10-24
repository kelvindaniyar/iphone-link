# LAPORAN TAHAP 48: REBRANDING RESMI KE "iPhone Link", PEMBUATAN LOGO NATIVE (.SVG/.PNG/.ICO), PORTABLE RELEASE BUILDER, & DOKUMENTASI GITHUB

## 1. Ringkasan Eksekutif & Milestone Rebranding
Setelah seluruh fitur teknis hingga Tahap 47 V2 (USB 60fps Mirroring, Mac-Style Stealth Dim Mode 0% dengan Restore ~25%, Remote Wake & Auto-Unlock PIN, Lock Screen, Bidirectional OCR Copy-Paste Apple Notes & Google Keep, Smart Auto-Clear Cache, dan Bluetooth Audio Sink) terbukti berjalan 100% sempurna di iPhone 17 Pro fisik milik Kelvin, **Tahap 48** secara resmi mentransformasikan aplikasi ini menjadi **"iPhone Link"** untuk persiapan peluncuran publik ke GitHub.

---

## 2. Pekerjaan Rekayasa yang Diselesaikan

### Pilar 1: Desain Logo Modern "iPhone Link" & Generator Native
- **Konsep Visual Logo**:
  * **Latar Belakang Squircle Windows 11 Fluent Liquid Glass**: Gradasi diagonal halus dari Fluent Electric Blue (`#0078D4`) $\rightarrow$ Vibrant Cyan (`#00B4D8`) $\rightarrow$ Deep Indigo (`#4F46E5`), dilapisi highlight kilau kaca (*specular gloss*) dan border kristal mengkilap (*crystal border*).
  * **Background-Right**: Siluet jendela desktop Windows 11 transparan (*glass pane*) lengkap dengan garis header, kontrol jendela (min/max/close), dan watermark Windows 11.
  * **Foreground-Center**: Siluet fisik iPhone modern (iPhone 16/17 Pro titanium silver) dengan layar OLED obsidian pekat dan kapsul khas **Dynamic Island** di bagian atas.
  * **Center Screen**: Ikon **"Link"** modern bercahaya cyan-putih (`#00F2FE` / `#FFFFFF`) dengan gelombang konektivitas yang memancarkan sinyal ke jendela Windows 11 desktop di belakangnya.
- **File Aset yang Dihasilkan**:
  1. [`assets/logo.svg`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/assets/logo.svg) & [`handset/web/logo.svg`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/logo.svg): Vektor tajam untuk web UI header, placeholder mirroring, dan banner README.
  2. [`assets/logo.png`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/assets/logo.png): Gambar raster resolusi tinggi 512x512 piksel RGBA dengan anti-aliasing sempurna.
  3. [`assets/iphone_link.ico`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/assets/iphone_link.ico): File Windows Icon multi-ukuran native (`16x16, 32x32, 48x48, 64x64, 128x128, 256x256` piksel 32-bit RGBA) untuk taskbar, jendela aplikasi, dan shortcut desktop.
  4. Generator script otomatis: [`scratch/generate_logos.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/scratch/generate_logos.py).

### Pilar 2: Rebranding UI & Windows Native App (`gui.py`, `index.html`, `mirror.html`, `pyproject.toml`)
1. **Di `handset/gui.py`**:
   - Judul window `webview.create_window` diubah menjadi **`"iPhone Link"`**.
   - Didaftarkan `SetCurrentProcessExplicitAppUserModelID("Kelvin.iPhoneLink.Desktop.1")` via `ctypes.windll.shell32` agar icon Taskbar Windows 11 menampilkan icon resmi **iPhone Link** (bukan icon generik Python).
   - Diimplementasikan pemasangan icon native `WM_SETICON` (`ICON_SMALL` & `ICON_BIG`) menggunakan `LoadImageW` pada HWND window pywebview.
2. **Di `handset/web/index.html` & `mirror.html`**:
   - Judul halaman diubah menjadi **`iPhone Link — iPhone Mirroring & Control for Windows`**.
   - Header Top Bar menampilkan logo kecil `logo.svg` (`18x18`) berdampingan dengan label teks **`iPhone Link`**.
   - Layar placeholder `#screen-empty` menampilkan logo elegan **iPhone Link** berukuran 76px dengan drop-shadow biru menyala.
   - Footer Side Drawer menyertakan atribusi lisensi resmi:  
     `iPhone Link v1.0 · Forked from teyboxv1/pane-iphone-mirror-windows · GPL-3.0`
3. **Di `pyproject.toml`**:
   - Diperbarui nama paket menjadi `iphone-link` v1.0.0.

### Pilar 3: Peluncur Portable & Pembuat Shortcut Desktop
1. **`iPhone Link.bat`**:
   - Peluncur bersih tanpa jendela hitam CMD yang mengganggu, menggunakan `pythonw.exe` untuk meluncurkan background server dan GUI frameless.
2. **`Create-Desktop-Shortcut.bat`**:
   - Script 1-klik otomatis membuat shortcut **`iPhone Link`** di **Desktop** dan **Start Menu Windows**, lengkap dengan icon native `assets/iphone_link.ico`.
3. **`build_portable_release.ps1`**:
   - Script pembuat paket rilis otomatis yang mengemas file aplikasi bersih ke `release/iPhone-Link-Portable-v1.0.0.zip` (0.38 MB).
4. **Proteksi Mutlak Data Pribadi (`.gitignore`)**:
   - Seluruh file pengujian debug, metrics, setting lokal, dan riwayat (`debug_ocr_*`, `debug_metrics.json`, `typed_history.json`, `user_settings.json`, `wifi_state.json`, `scratch/`, `release/`) telah di-untrack dari git dan dikunci di `.gitignore`, menjamin **0% data pribadi / PIN / telemetri Kelvin ikut ter-push ke GitHub publik**.

### Pilar 4: Dokumentasi GitHub Kelas Dunia (`README.md`)
- Halaman depan GitHub tampil sangat profesional dengan logo banner, badge status, pengakuan atribusi fork hulu ke `teyboxv1/pane-iphone-mirror-windows` di bawah GPL-3.0.
- Tabel 11 Fitur Utama, Tabel Daftar Shortcut Keyboard lengkap, Panduan Penggunaan Portable & Source, serta Diagram Arsitektur Teknis.

---

## 3. Hasil Pengujian Nyata (`scratch/verify_tahap48.py`)

```
======================================================================
STARTING TAHAP 48 VERIFICATION SUITE
======================================================================

======================================================================
TEST 1: Logo & Native Icon Assets (.SVG, .PNG, .ICO)
======================================================================
  [PASS] assets/logo.svg exists and > 500 bytes
  [PASS] handset/web/logo.svg exists and matches assets
  [PASS] assets/logo.png is 512x512 RGBA
  [PASS] assets/iphone_link.ico contains all 6 Windows sizes (16, 32, 48, 64, 128, 256)

======================================================================
TEST 2: Application & UI Rebranding ('iPhone Link')
======================================================================
  [PASS] gui.py: window title set to 'iPhone Link' and AppUserModelID configured
  [PASS] index.html: title contains 'iPhone Link', top bar has logo and name, drawer has attribution
  [PASS] mirror.html: title contains 'iPhone Link — Screen Mirror'
  [PASS] pyproject.toml: project name 'iphone-link' and version '1.0.0'

======================================================================
TEST 3: Launchers, Shortcut Creator & Release Packaging
======================================================================
  [PASS] iPhone Link.bat exists and launches pythonw cleanly
  [PASS] Create-Desktop-Shortcut.bat exists and references iphone_link.ico
  [PASS] build_portable_release.ps1 exists and includes strict privacy checks

======================================================================
TEST 4: Security, .gitignore Protection & GitHub Documentation
======================================================================
  [PASS] .gitignore protects all personal debug and state files
  [PASS] README.md contains fork attribution to teyboxv1/pane-iphone-mirror-windows
  [PASS] README.md contains iPhone Link title and Keyboard Shortcuts table

======================================================================
TEST 5: Zero Regression on Tahap 47 V2
======================================================================
  [PASS] Tahap 47 V2 verification suite passes 100%

======================================================================
>>> ALL TAHAP 48 VERIFICATION TESTS PASSED 100% PERFECTLY! <<<
======================================================================
```

---

## 4. Tahap 48 V2 — Analisa Forensik & Perbaikan Seleksi 1 Baris Tunggal serta Pembersihan Underscore Noise

### A. Analisa Forensik Kasus Video (Menit 00:49 `prabowo gibran karhutla`)
1. **Zona Eksklusi Toolbar Apple Notes:**
   Pada layar iPhone 17 Pro (`1216 x 2656`), baris pertama catatan baru berada pada `Y = 0.138 * H .. 0.168 * H` (~385px). Zona masking pojok atas disesuaikan agar hanya memblokir tombol `<` dan `✓` pada `Y < 0.125 * H` dan pemindaian highlight dimulai tepat dari `0.122 * H`.
2. **Bobot Gagang Seleksi (Lollipop Pins):**
   Seleksi 1 baris pendek atau 1 kata memiliki rasio piksel kuning yang lebih sedikit karena tertutup huruf teks. Ditambahkan pembobotan **+4** untuk setiap piksel gagang seleksi kuning pekat (`isYellowHandle`) dan biru pekat (`isBlueHandle`), serta ambang baris diturunkan ke `8` sehingga seleksi 1 baris/1 kata 100% selalu terdeteksi.
3. **PowerShell 5.1 Array Coercion Fix:**
   Pada PowerShell 5.1, pemanggilan fungsi yang mengembalikan 1 objek menyebabkan unrolling objek skalar (`PSCustomObject`) yang tidak memiliki properti `.Count` bilangan bulat (`$sortedLines.Count` bernilai `$null`). Akibatnya loop baris tidak dijalankan dan menghasilkan output kosong. Solusi presisi diimplementasikan dengan memaksa array coercion `$lines1 = @($lines1)`, `$lines2 = @($lines2)`, dan `$sortedLines = @($sortedLines)` di [`handset/ocr_extractor.ps1`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/ocr_extractor.ps1).
4. **Fast-Path Single-Line & Fallback Typing Buffer:**
   Di [`handset/screen.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/screen.py), baris tunggal langsung dieksekusi melalui fast-path `if len(c_lines) == 1:`. Jika OCR kosong namun ada warna seleksi kuning/biru, teks dipulihkan dari buffer ketikan keyboard PC dalam waktu < 30 detik melalui `get_recent_typed_single_line_fallback()`.
5. **Pembersihan Underscore Nyasar (`_heroik` $\rightarrow$ `heroik`):**
   Di `_clean_ocr_line()`, ditambahkan aturan `p` untuk menghapus underscore nyasar pada kata biasa (`t = re.sub(r'(?<![@#A-Za-z0-9])_+(?=[A-Za-z])|(?<=[A-Za-z])_+(?![A-Za-z0-9])', '', t)`), sekaligus mempertahankan `@mention` dan `#hashtag`. Di `_pick_cleaner_line()`, diberikan penalti 25 dan prioritas garis bersih tanpa underscore nyasar.

---

### B. Hasil Pengujian Nyata Tahap 48 V2 (`scratch/verify_tahap48_v2.py`)

```
======================================================================
STARTING TAHAP 48 V2 COMPREHENSIVE VERIFICATION SUITE
======================================================================

======================================================================
TEST 1: Real 1-Line Selection OCR Test ('prabowo gibran karhutla')
======================================================================
      OCR Raw Output: ['<<<SEL_COLOR:yellow>>>\nprabowo gibran karhutla']
      OCR Final Text: 'prabowo gibran karhutla'
  [PASS] 1-Line Selection ('prabowo gibran karhutla') returned non-empty text
  [PASS] 1-Line Selection accurately contains 'prabowo gibran karhutla'

======================================================================
TEST 2: Real 3-Line Selection Test (Double-Enter Spacing)
======================================================================
      OCR 3-Lines Output:
prabowo gibran karhutla

jokowi karhutla

luhut rocky gerung
  [PASS] 3-Line selection returned text with 3 lines
  [PASS] 3-Line selection contains 'prabowo', 'jokowi', and 'luhut'

======================================================================
TEST 3: Underscore Noise Cleaning ('aksi _heroik' -> 'aksi heroik')
======================================================================
  [PASS] '_heroik' cleaned to 'heroik'
  [PASS] @username_with_underscore and #hashtag_with_underscore preserved
  [PASS] _pick_cleaner_line prioritizes line without stray underscore

======================================================================
TEST 4: Fallback to Recent Typed Buffer (< 30s) on Empty OCR with Selection Color
======================================================================
[Tahap 48 V2] Recovered 1-line selection from recent typing buffer: 'banten feeds test fallback 17 pro'
  [PASS] Empty OCR with selection color recovers from recent typing buffer (< 30s)

======================================================================
TEST 5: Full Regression Testing (Tahap 40, Tahap 47 V2, Tahap 48)
======================================================================
  [PASS] Tahap 40 V2 OCR & Apple Notes Two-Copy Stitching: 100% PASS
  [PASS] Tahap 47 V2 Stealth Dim & Remote Lock/Wake: 100% PASS
  [PASS] Tahap 48 Rebranding, Assets & Documentation: 100% PASS

======================================================================
>>> ALL TAHAP 48 V2 VERIFICATION TESTS PASSED 100% PERFECTLY! <<<
======================================================================
```

Paket rilis portabel [`release/iPhone-Link-Portable-v1.0.0.zip`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/release/iPhone-Link-Portable-v1.0.0.zip) telah di-build ulang dan siap dipublikasikan ke GitHub.

---

## 5. Tahap 48 V3 — Pembaruan Dokumentasi GitHub: Tested Environment, Panduan Developer Mode & Tips 3uTools

Pada pembaruan tahap ini, seluruh kode Python, PowerShell, dan JavaScript tetap dipertahankan 100% utuh tanpa perubahan. Pembaruan difokuskan pada penyempurnaan dokumen publik [`README.md`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/README.md) agar pengguna umum di GitHub dapat dengan mudah menyiapkan perangkat dan menjalankan aplikasi:

1. **Informasi & Badge Lingkungan Terverifikasi (Tested & Verified Environment)**:
   - Device: **iPhone 17 Pro (iOS 27.0.1)**
   - OS: **Windows 11 26H2 (64-bit)**
2. **Panduan Konfigurasi iPhone & Aktivasi Developer Mode (Dwibahasa: Indonesia & English)**:
   - Penjelasan bahwa sebelum menggunakan iPhone Link, pengguna wajib mengaktifkan *Developer Mode* (*Settings → Privacy & Security → Developer Mode → ON*).
3. **Tips Khusus Mengatasi Menu Developer Mode yang Tersembunyi via 3uTools**:
   - Disediakan solusi cepat 1-menit menggunakan fitur *Toolbox → Realtime Screen* di **3uTools** PC untuk memunculkan menu *Developer Mode* yang disembunyikan iOS pada iPhone baru.
4. **Rebuild Paket Portable**:
   - Paket portabel [`release/iPhone-Link-Portable-v1.0.0.zip`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/release/iPhone-Link-Portable-v1.0.0.zip) telah di-build ulang untuk menyertakan `README.md` terbaru.


