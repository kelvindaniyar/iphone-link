# LAPORAN TAHAP 25: MULTI-FRAME SCROLL STITCHER, ASSISTIVETOUCH MASKING, DAN BOUNDARY EXPANSION

## 1. Ringkasan Eksekutif
Pada pengujian Tahap 24 oleh Kelvin, fitur copy kalimat pendek (`"valentino rossi"`, `"jokowi pria solo from iphone 17 pro"`) dan paste dua arah Windows-iPhone telah mencapai akurasi **100% tanpa typo**.

Namun, ditemukan kendala saat Kelvin menyalin **catatan berita multi-paragraf yang panjang** di Notes iOS (kasus evakuasi kapal KMP Batumandi dari awal sampai `#cilegon`):
- Hasil paste di Windows Notepad hanya memuat bagian bawah (mulai dari `"lambung kapal. Seluruh 428 penumpang..."` ke bawah), sedangkan Paragraf 1, Paragraf 2, dan 3 baris di atas `"lambung kapal"` hilang.

Berdasarkan analisis frame-by-frame:
1. **Viewport Scrolling**: Layar Notes bergulir ke bawah saat Kelvin menarik gagang seleksi, menyebabkan paragraf atas keluar dari viewport saat tombol Copy ditekan.
2. **Top Boundary 16%**: Batas potong atas `0.16 * H` membuang baris teratas layar (`"mengambil langkah evakuasi berisiko tinggi"` di $Y = 10.8\% - 15.5\%$).
3. **AssistiveTouch Obstruction**: Lingkaran abu-abu AssistiveTouch menempel di tepi kiri menutupi 2 baris (`"menggunakan tali tambang..."` dan `"perahu penolong..."`), membingungkan Windows Media OCR.

Tahap 25 mengeksekusi perbaikan tuntas pada ketiga faktor tersebut sekaligus mempertahankan 100% kestabilan fitur Tahap 24.

---

## 2. Rincian Implementasi Teknis

### A. Perbaikan Batas Atas & Bawah di `handset/ocr_extractor.ps1`
1. **Penaikan Batas Pemindaian & Cropping ke `0.105 * H` (10.5%)**:
   - Toolbar Notes iOS (`<`, undo, redo, share, dan tombol centang kuning `✓`) selesai pada $Y \approx 0.102 \times H$.
   - Batas atas diperluas ke **`0.105 * H`** (sebelumnya `0.16 * H`) dan batas bawah diperluas hingga **`0.88 * H`**.
   - Untuk mencegah tombol centang kuning `✓` di toolbar atas memicu seleksi palsu, pemindaian warna highlight dibatasi pada **$Y \ge 0.125 \times H$**. Namun saat seleksi ditemukan di area atas (`minY < 0.35 * H`) atau seleksi multi-baris (`(maxY - minY) > 0.10 * H`), `cropY` ditarik ke atas ke **`0.105 * H`** sehingga baris pertama tepat di bawah toolbar tidak pernah terpotong.

### B. Masking Otomatis Bulatan Abu-Abu AssistiveTouch Menjadi Putih Bersih
1. **Deteksi Kluster Abu-Abu di Tepi Kiri/Kanan**:
   - Deteksi piksel abu-abu netral: `|R - G| <= 14 && |G - B| <= 14 && |R - B| <= 14` dengan `R` antara `48` hingga `195`.
   - Pemindaian di zona kiri ($X < 0.20 \times W$) dan zona kanan ($X > 0.80 \times W$).
   - Pola lari horizontal (*contiguous run*) $\ge 25$ px yang membentang $\ge 35$ baris vertikal (AssistiveTouch docking/floating).
2. **Masking ke Pure White (`255, 255, 255`)**:
   - Seluruh piksel di dalam kotak AssistiveTouch (ditambah padding 8 px) diubah menjadi putih bersih langsung pada buffer raw sebelum ekstraksi Kandidat 1 (Natural) maupun Kandidat 2 (Red-Filtered).
   - Teks di samping AssistiveTouch (`"menggunakan tali tambang..."` dan `"perahu penolong..."`) menjadi bersih dari gangguan grafis dan langsung terbaca oleh `Windows.Media.Ocr`.

### C. Pemulihan Struktur Paragraf Berdasarkan Koordinat Y
1. **Perhitungan Bounding Box Baris dari `Words`**:
   - Dalam Windows Media OCR WinRT, `OcrLine` mengelompokkan `OcrWord`.
   - $Y_{\min}$ dan $Y_{\max}$ dihitung dari masing-masing kata, lalu baris diurutkan berdasarkan koordinat vertikal ($Y_{\min}$).
2. **Pendeteksian Jarak Antar-Baris (`avgH * 0.75`)**:
   - Dihitung rata-rata tinggi baris (`avgH`). Jika jarak vertikal antara bagian bawah baris sebelumnya dan bagian atas baris saat ini $> (\text{avgH} \times 0.75)$, disisipkan baris kosong (`\r\n\r\n`) sehingga struktur paragraf berita/caption tetap rapi seperti aslinya.

### D. Multi-Frame Selection & Scroll Stitcher
1. **Ring Buffer Riwayat Frame (`mirror.js`)**:
   - Dibuat ring buffer `window._ScrollFrameHistory` (menyimpan maksimal 5 snapshot `{ ts, dataUrl }`).
   - Fungsi `recordSelectionSnapshot(force = false)` throttled pada $\ge 450$ ms.
   - Perekaman snapshot dipicu secara otomatis saat:
     - `pointerdown`: Menangkap posisi layar *sebelum* pengguna menarik seleksi/menggulir.
     - `pointermove`: Menangkap frame tengah saat mouse/jari ditahan menarik seleksi (throttled 500 ms).
     - `wheel`: Menangkap frame saat layar digulir via trackpad (throttled 600 ms).
     - `keydown` (`Ctrl + A`): Menangkap frame tampilan penuh.
   - Fungsi `window.getRecentSelectionFrames()` mengekspor snapshot dari 25 detik terakhir.
2. **Pengiriman Frame History (`keys.js` & `app.js`)**:
   - Saat `Ctrl + C`, `Ctrl + X`, atau tombol UI `📥 Copy iPhone Text` diklik, frame aktif (`frameDataUrl`) dan daftar riwayat frame (`frameHistory`) dikirim ke endpoint `/api/keyboard/shortcut`.
3. **Penyambungan Pintar (*Fuzzy Overlap Stitcher*) di Backend (`server.py` & `screen.py`)**:
   - **Fast-Path**: Jika hasil OCR pada `frameDataUrl` $\le 2$ baris (atau cocok dengan `_LAST_TYPED_ACTIVE_LINE`), frame riwayat diabaikan dan teks langsung disalin instan (0 ms latency overhead untuk kalimat pendek/sedang).
   - **Multi-Line Stitching**: Jika $\ge 3$ baris, sistem mengekstrak teks dari frame riwayat dan menggabungkannya secara berurutan menggunakan `stitch_multiline_ocr_frames()`.
   - Irisan baris antara Frame $i$ dan Frame $i+1$ dicocokkan menggunakan `difflib.SequenceMatcher.ratio() >= 0.72` dan substring/word-set overlap $\ge 70\%$.
   - Untuk baris beririsan yang terpotong di salah satu frame (misalnya karena tertutup AssistiveTouch), sistem otomatis memilih baris yang lebih panjang dan lengkap.

---

## 3. Hasil Verifikasi Otomatis (`verify_tahap25.py`)

Seluruh pengujian otomatis dijalankan pada lingkungan Windows 11:
1. **Test 1 (Regresi Kalimat Pendek)**:
   - Kalimat `"valentino rossi"` dan `"jokowi pria solo from iphone 17 pro"` diakumulasi per-keystroke dan berhasil terkoreksi 100% tanpa regresi.
2. **Test 2 (Logika Stitching Catatan Berita)**:
   - Penggabungan multi-frame berhasil menyatukan seluruh 6 bagian penting (Paragraf 1, Paragraf 2, baris evakuasi, lambung kapal 428 penumpang, pertanyaan penutup, dan hashtag `#cilegon`).
3. **Test 3 (AssistiveTouch Masking & Boundary pada Gambar Nyata)**:
   - Dijalankan pada `debug_ocr_raw.png` dari sesi Kelvin:
     - AssistiveTouch berhasil dideteksi dan dimasking menjadi putih bersih.
     - Baris `"perahu penolong yang bersiaga di bawah"` yang sebelumnya hilang kini berhasil terbaca.
     - Jarak vertikal antar-paragraf menghasilkan double newline (`\r\n\r\n`) secara natural.
4. **Test 4 (End-to-End Synthetic Multi-Frame)**:
   - Dua frame simulasi layar penuh $1216 \times 2656$ diekstraksi via WinRT OCR dan digabungkan. Hasil akhir memuat 100% teks berita dari atas ke bawah tanpa ada duplikasi maupun baris yang hilang.

---

## 4. File yang Diperbarui
- [`handset/ocr_extractor.ps1`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/ocr_extractor.ps1): Masking AssistiveTouch C#, batas atas $0.105 \times H$, batas bawah $0.88 \times H$, pelestarian paragraf.
- [`handset/screen.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/screen.py): Fungsi `stitch_two_ocr_texts` dan `stitch_multiline_ocr_frames`.
- [`handset/server.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/server.py): Fast-path vs multi-frame stitching di endpoint shortcut clipboard.
- [`handset/web/mirror.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/mirror.js): Ring buffer `_ScrollFrameHistory`, perekam seleksi di pointerdown, onMove, wheel, Ctrl+A.
- [`handset/web/keys.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/keys.js): Pengambilan dan penerusan `frameHistory` pada `Ctrl+C` dan `Ctrl+X`.
- [`handset/web/app.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/app.js): Penerusan `frameHistory` pada tombol UI `Copy iPhone Text` dan sinkronisasi clipboard.
- [`handset/web/index.html`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/index.html) & [`handset/web/mirror.html`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/mirror.html): Pembaruan cache tag `v=20261024_tahap25_multi_frame_stitcher_and_assistivetouch_masking`.
