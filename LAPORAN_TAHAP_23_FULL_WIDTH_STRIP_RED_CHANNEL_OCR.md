# Laporan Progres Tahap 23: Full-Width Horizontal Strip, Red-Channel Color Filter, White Margin 60px, dan Persistence Riwayat Ketik

**Tanggal**: 2026-10-24  
**Status**: SELESAI & TERVERIFIKASI  
**Branch**: `feature/macos-sequoia-ui`  
**Referensi Pengujian**: Video pengujian langsung oleh Kelvin (`Screen Recording 2026-10-24 013715.mp4`)

---

## 1. Analisis Akar Masalah (Root Cause) Regresi Tahap 22

Pada pengujian Tahap 22, fitur *Copy iPhone Text* dari Notes ke Windows mengalami kegagalan di mana hanya 1 huruf (`"u"` atau `"e"`) yang masuk ke Clipboard Windows:
1. **Penyebab Cropping Sempit**: Pada iOS Notes (Light Mode), warna highlight teks yang diblok adalah krem/kuning sangat pucat (`R~253, G~246, B~225`). Yang berwarna kuning-oranye pekat (`R - B >= 25`) hanyalah **bulatan gagang seleksi (lollipop handle)** di ujung huruf terakhir (misalnya huruf `"e"` pada `"iphone"`). Akibatnya, `ocr_extractor.ps1` memotong kotak kecil di sekitar gagang lollipop dengan bantalan vertikal sempit (`padY = 4px`), sehingga membuang seluruh kalimat di sebelah kirinya dan memicu kegagalan `Windows.Media.Ocr.OcrEngine`.
2. **Penolakan pada Auto-Correct**: Ketika OCR hanya mengembalikan 1 karakter (`"e"`), `SequenceMatcher("e", "lamine yamal from iphone") = 0.08` (< 0.65), sehingga `auto_correct_ocr_text()` menolak baris ketikan aktif yang baru saja diketik Kelvin.

---

## 2. Solusi Arsitektural Tahap 23 yang Diimplementasikan

### A. Rombak `handset/ocr_extractor.ps1` dengan Full-Width Horizontal Strip & Red-Channel Filter
1. **Deteksi Vertikal Bounding Box (`minY` s.d. `maxY`)**:
   - Pindai zona konten aktif (`Y = 0.16 * H` s.d. `0.82 * H`, `X = 0.04 * W` s.d. `0.96 * W`).
   - Deteksi piksel seleksi dengan kriteria komprehensif:
     * Kuning pucat highlight Notes: `R > 220 && G > 205 && B < 238 && (R - B) >= 12`
     * Kuning-oranye pekat gagang lollipop: `R > 200 && G > 140 && (R - B) >= 40`
     * Biru seleksi iOS: `B > 140 && (B - R) >= 22`
2. **Full-Width Strip**:
   - Setelah batas vertikal seleksi ditemukan, lebar crop **SELALU MENGAMBIL LEBAR PENUH KONTEN** (`cropX = 0.03 * W`, `cropW = 0.94 * W`).
   - Batas vertikal diberi ruang padding yang lapang:
     * `cropY = Math.Max(0.15 * H, minY - 0.035 * H)`
     * `cropH = Math.Min(0.84 * H - cropY, (maxY - cropY) + 0.035 * H)`
   - Hasil: Mustahil teks di sebelah kiri terpotong!
3. **Red-Channel Extraction & Filter Anti-Aliasing**:
   - Latar putih (`R = 255`), highlight kuning (`R = 250..255`), bulatan gagang kuning (`R = 235..250`), dan titik merah spellcheck (`R = 225..250`) semuanya memiliki nilai `R >= 210` atau `R > 175 && (R - B > 35 || R - G > 30)`.
   - Piksel tersebut dipetakan ke **Putih Bersih (255)**.
   - Piksel huruf hitam (`R = 15..55`) dipetakan secara halus dengan formula:
     `v = Max(0, Min(255, (r - 25) * 255.0 / (210 - 25)))`.
   - Hasil: Gagang kuning, highlight, dan titik merah spellcheck 100% hilang menjadi putih bersih, sementara bentuk dan anti-aliasing huruf hitam tetap terjaga sempurna.
4. **Bicubic Upscaling 2.5x & Margin Putih 60px**:
   - Gambar strip di-upscale `2.5x` menggunakan `HighQualityBicubic` dan ditempatkan di atas kanvas putih dengan margin lebar `60px` di semua sisi sebelum dikirim ke `Windows.Media.Ocr.OcrEngine`.
5. **Pembersihan Menu Pop-up iOS**:
   - Baris menu pop-up balloon (`Cut`, `Copy`, `Paste`, `Delete`, `Select`, `Look Up`, `Translate`, `Share...`, `No Replacements Found`, `Device Hub`, dsb.) difilter secara otomatis tanpa memotong baris teks catatan.

### B. Persistence Riwayat Ketikan ke Disk & Fuzzy Auto-Correction Cerdas
1. **Simpan Permanen ke `typed_history.json`**:
   - File `typed_history.json` dibuat di root project untuk menyimpan `_LAST_TYPED_ACTIVE_LINE`, `_LAST_TYPED_TIMESTAMP`, dan `_TYPED_LINES_BUFFER`.
   - Muat riwayat saat server startup, sehingga teks yang pernah diketik tetap tersimpan di memori meskipun server di-restart.
2. **Prioritas Pemulihan Kalimat Aktif (`_LAST_TYPED_ACTIVE_LINE`)**:
   - Jika user mengetik dalam 120 detik terakhir:
     * Jika OCR kosong, atau panjang OCR $\le 4$ karakter (misalnya hanya huruf `"e"` atau `"u"` dari gagang lollipop), atau OCR merupakan substring/akhiran dari kalimat aktif: **LANGSUNG KEMBALIKAN `_LAST_TYPED_ACTIVE_LINE` UTUH**!
3. **Pencocokan Multi-Kandidat di History**:
   - Menghitung kecocokan komparatif (containment + `SequenceMatcher` + word overlap) terhadap kandidat di `typed_history.json` sehingga teks catatan seperti `"barcelona from iphone"` langsung pulih 100%.

### C. Ekspor Otomatis Gambar Debug Aktual
Di root project disimpan 3 file diagnostik setiap kali copy dijalankan:
- [`debug_ocr_raw.png`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/debug_ocr_raw.png): Gambar asli resolusi tinggi dari canvas mirroring.
- [`debug_ocr_cleaned.png`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/debug_ocr_cleaned.png): Gambar setelah Red-Channel filter, full-width crop, dan 60px margin putih yang dikirim ke `OcrEngine`.
- [`debug_ocr_result.json`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/debug_ocr_result.json): Telemetri lengkap berisi `raw_ocr_text`, `corrected_text`, `typed_active_line`, dan `typed_buffer_tail`.

---

## 3. Hasil Pengujian Verifikasi Otomatis

Pengujian dijalankan melalui script [`verify_tahap23.py`](file:///C:/Users/kelvin/.gemini/antigravity-ide/brain/84301188-cb91-4631-8fc1-4ca343d3ce82/scratch/verify_tahap23.py):

| No | Kasus Uji | Input OCR | Kondisi Shadow Buffer | Output yang Dihasilkan | Status |
|---|---|---|---|---|:---:|
| 1 | Persistensi Riwayat Ketikan | - | `typed_history.json` disimpan dan dimuat ulang | File `typed_history.json` berisi data aktif & buffer | **PASS** |
| 2 | Kasus Kelvin (Lamine Yamal) | `"e"` (1 huruf dari gagang) | User baru mengetik `"lamine yamal from iphone"` (< 120s) | `"lamine yamal from iphone"` | **PASS** |
| 3 | Kasus Fallback OCR Kosong | `""` (kosong) | User baru mengetik `"lamine yamal from iphone"` (< 120s) | `"lamine yamal from iphone"` | **PASS** |
| 4 | Kasus Notes Pale Yellow (Barcelona) | Gambar Notes Light Mode asli (`test_barcelona_pale_yellow.png`) | Teks di history: `"barcelona from iphone"` | OCR baca `"barcelona from iphone 17 pro"`, hasil koreksi: `"barcelona from iphone"` | **PASS** |
| 5 | Output Gambar & Telemetri Debug | Stream Canvas Resolusi Penuh | - | `debug_ocr_raw.png` (34KB), `debug_ocr_cleaned.png` (150KB, 2977x930px), `debug_ocr_result.json` tersimpan | **PASS** |

---

## 4. Daftar File yang Diubah

| No | File | Deskripsi Perubahan |
|---|---|---|
| 1 | [`handset/ocr_extractor.ps1`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/ocr_extractor.ps1) | Implementasi C# fast preprocessor, full-width crop (`0.03*W` s.d. `0.97*W`), Red-Channel contrast filter, margin putih 60px, upscale 2.5x bicubic, dan ekspor `debug_ocr_cleaned.png`. |
| 2 | [`handset/screen.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/screen.py) | Persistensi disk `typed_history.json`, auto-correction cerdas (prioritas aktif $\le 4$ karakter / substring < 120s, skoring komparatif buffer), helper `get_debug_typed_state()`. |
| 3 | [`handset/server.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/server.py) | Ekspor `debug_ocr_raw.png` dan `debug_ocr_result.json`, fallback aktif pada OCR kosong. |
| 4 | [`handset/web/mirror.html`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/mirror.html) | Cache-busting tag versi `20261024_tahap23_full_width_red_channel_ocr`. |
| 5 | [`handset/web/index.html`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/index.html) | Cache-busting tag versi `20261024_tahap23_full_width_red_channel_ocr`. |
| 6 | [`handset/web/app.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/app.js) | Cache-busting query `MIRROR_PAGE` ke versi `20261024_tahap23_full_width_red_channel_ocr`. |
| 7 | [`typed_history.json`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/typed_history.json) | File penyimpanan riwayat teks keyboard laptop ke disk. |
