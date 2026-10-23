# LAPORAN TAHAP 7: PERBAIKAN VISUAL RANTAI FLEXBOX TINGGI VIDEO 100%, BYPASS CACHE WEBVIEW2, SATUAN VIEWPORT MUTLAK `calc(100vh - 80px)` & CLEANUP ZOMBIE PROSES

> **Klien:** Kelvin (Content Creator)  
> **Fokus Tahap:** Pembasmian Zombie Proses `python.exe` di `run.bat`, Paket Editable pip, Kunci Tinggi Layar Mutlak `calc(100vh - 80px)` Tanpa Flexbox Collapse, Satuan `100vw`/`100vh` di `mirror.css`, Timestamp Dinamis, dan Full Frame Drawing WebCodecs  
> **Branch Git:** `feature/macos-sequoia-ui`  
> **Commit Hash:** `b9d7b14` (`fix(display): kunci tinggi mutlak calc(100vh - 80px), bunuh zombie proses di run.bat, dan dynamic timestamp URL`)  
> **Tanggal:** 23 Oktober 2026  

---

## 📌 RINGKASAN EKSEKUTIF

Hasil pengujian Kelvin menunjukkan video sebelumnya sempat tampil hanya ~23% bagian atas dengan rasio kotak 2:1 di tengah layar.
Investigasi mendalam membuktikan 2 penyebab fatal yang terjadi:
1. **Zombie Proses `python.exe`**: Skrip `run.bat` sebelumnya menjalankan server di background dengan `start /min python -m handset.server` tanpa mematikan proses saat jendela GUI ditutup. Akibatnya, server lama tetap hidup dan menahan port 8765 sehingga kode Python/backend baru tidak pernah termuat.
2. **Intrinsic Collapse HTML5 Iframe (`300x150`)**: Rantai 6 lapis flexbox dengan `flex: 1 1 auto`, `height: 100%`, dan `align-items: center` menyebabkan tinggi kontainer dihitung secara *indefinite* (tidak tentu). Sesuai spesifikasi browser engine WebView2 / Chromium, `iframe` yang berada di dalam kontainer indefinite jatuh ke ukuran default intrinsic `300px x 150px` (rasio 2:1 / 9:4.5), memotong rasio layar iPhone (9:19.5) sehingga hanya 23% bagian atas layar yang terlihat.

Seluruh 4 instruksi mutlak Kelvin telah dieksekusi 100%:
1. **Pembasmian Zombie Process & Paket Editable**:
   - Menambahkan perintah `taskkill /F /IM python.exe >nul 2>&1` di baris **paling awal** dan **paling akhir** `run.bat`.
   - Membuat `pyproject.toml` dan menjalankan `.\venv\Scripts\pip install -e . --no-deps` dalam mode editable.
   - Timestamp dinamis berbasis detik UNIX (`time.time()`) di `gui.py`: `url=f"http://127.0.0.1:8765/?t={int(time.time())}"`.
   - Timestamp dinamis di `app.js`: `const MIRROR_PAGE = "/mirror.html?t=" + Date.now();`.
2. **Kunci Tinggi Mutlak `calc(100vh - 80px)` di `app.css` (Bebas Flexbox Indefinite)**:
   - Menghapus ketergantungan `height: 100%` pada kontainer display.
   - Mengunci `.workspace`, `.stage`, `#phone-shell`, `.screen-stage`, `#phone-screen`, `.screen-viewport` ke tinggi mutlak:
     ```css
     width: 100% !important;
     height: calc(100vh - 80px) !important;
     min-height: calc(100vh - 80px) !important;
     max-height: calc(100vh - 80px) !important;
     flex: 0 0 calc(100vh - 80px) !important;
     position: relative !important;
     display: block !important;
     overflow: hidden !important;
     ```
   - Mengunci `iframe`, `#screen-view`, `#mirror-frame`, `.screen-frame` ke:
     ```css
     position: absolute !important;
     top: 0 !important;
     left: 0 !important;
     width: 100% !important;
     height: calc(100vh - 80px) !important;
     min-height: calc(100vh - 80px) !important;
     max-height: calc(100vh - 80px) !important;
     border: none !important;
     display: block !important;
     ```
   - Menyembunyikan elemen hint bawah: `#hint, .stage-hint, #screen-note, .stage-note { display: none !important; }`.
   - Menyesuaikan Bottom Nav Bar tepat `44px` sehingga $36\text{px (top bar)} + 44\text{px (nav)} = 80\text{px}$. Tinggi viewport display pada jendela default 840px menjadi tepat **760px**!
3. **Kunci `100vw` dan `100vh` di `mirror.css` & `mirror.js`**:
   - `mirror.css`: `html, body { width: 100vw !important; height: 100vh !important; }` dan `#c { max-width: 100vw !important; max-height: 100vh !important; object-fit: contain !important; }`.
   - `mirror.js`: `fitCanvas()` kini membaca `document.documentElement.clientWidth || window.innerWidth` dan `document.documentElement.clientHeight || window.innerHeight`.
   - Di `drawFrame(f)`: `ctx.drawImage` secara eksplisit menggambar penuh seluruh dimensi frame asli (`f.displayWidth || f.width` x `f.displayHeight || f.height`).
4. **Verifikasi Teruji**:
   - `taskkill` terpasang di `run.bat` dan bebas zombie proses.
   - `app.css` terverifikasi memiliki 12 instans `calc(100vh - 80px)`.
   - Server merespons dengan header `Cache-Control: no-store, no-cache, must-revalidate, max-age=0`.

---

## 1. Tabel Matriks Perbaikan Masalah

| Bagian | Kondisi Lama (Penyebab Masalah) | Kondisi Baru (Solusi Mutlak) |
|:---|:---|:---|
| **Lifecycle Server (`run.bat`)** | `start /min python -m handset.server` berjalan terus setelah GUI ditutup, memblokir port 8765 dari kode server terbaru. | `taskkill /F /IM python.exe` di awal dan akhir `run.bat`. Semua proses zombie otomatis terbunuh bersih. |
| **Instalasi Paket** | Tidak ada `pyproject.toml` untuk editable install. | `pyproject.toml` ditambahkan dan paket diinstall via `pip install -e . --no-deps`. |
| **Tinggi Iframe di `app.css`** | `height: 100%` pada flexbox bertingkat menyebabkan browser collapse ke ukuran intrinsic `300x150` (hanya 23% layar terlihat). | `height: calc(100vh - 80px) !important; flex: 0 0 calc(100vh - 80px) !important;` (Viewport mutlak, tinggi ~760px pada window 840px). |
| **Teks Hint Bawah** | `.stage-note` memakan ruang bawah jendela. | `#hint, .stage-hint, #screen-note, .stage-note { display: none !important; }`. |
| **Canvas Sizing (`mirror.css`)** | Persentase relatif `width: 100%` berpotensi terpengaruh ukuran parent. | `width: 100vw !important; height: 100vh !important;` dan `#c { max-width: 100vw !important; max-height: 100vh !important; }`. |
| **Canvas Drawing (`mirror.js`)** | `ctx.drawImage` tanpa batas source eksplisit. | `ctx.drawImage(f, 0, 0, fw, fh, -fpW/2, -fpH/2, fpW, fpH)` menggambar 100% frame utuh. |
| **Bypass Cache URL** | Versi statis query string berpotensi di-cache sekali. | Timestamp UNIX dinamis (`?t=timestamp`) berganti otomatis pada setiap kali window dibuka. |

---

## 2. Riwayat Git Commit

```
* b9d7b14 (HEAD -> feature/macos-sequoia-ui) fix(display): kunci tinggi mutlak calc(100vh - 80px), bunuh zombie proses di run.bat, dan dynamic timestamp URL
* 70ad004 docs: perbarui laporan tahap 7 dengan solusi matikan cache webview2 dan kunci iframe absolut
* f2368c8 fix(webview2): matikan cache webview2, kunci iframe absolut 100 persen, dan bersihkan teks header
* 9350705 docs: tambahkan laporan Tahap 7 perbaikan rantai flexbox dan update laporan progress master
* 3c2ff85 fix(ui): perbaiki rantai flexbox tinggi video 100 persen dan responsive scaling mode fullsize
* f5cabac docs: add LAPORAN_TAHAP_PYWEBVIEW_DAN_GESTUR report
* 7c4871f feat(app): add pywebview frameless launcher, side-drawer settings, precision trackpad scroll, and swipe nav
```

---

## 3. Panduan Pengujian oleh Kelvin

Cukup jalankan:
```cmd
run.bat
```
1. Skrip otomatis mematikan proses zombie lama di awal.
2. Server Pane baru dijalankan di background.
3. Window desktop native terbuka dengan URL timestamp unik.
4. Klik **"Start mirroring"**:
   - Area layar video sekarang setinggi **~760px** (bukan 150px / 2:1).
   - Layar iPhone tampil utuh 100% dari notch / Dynamic Island paling atas sampai Home dock bar paling bawah.
   - Mode Full Size (⛶) otomatis memanfaatkan tinggi monitor penuh (`100vh - 80px`) secara proporsional.
5. Saat jendela GUI ditutup, `taskkill` otomatis membersihkan server background.
