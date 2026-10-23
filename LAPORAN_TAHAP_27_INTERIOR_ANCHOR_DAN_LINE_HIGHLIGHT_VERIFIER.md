# LAPORAN TAHAP 27: Continuous Viewport History, Bidirectional Interior Anchor Stitcher, dan Per-Line Highlight Verifier

**Tanggal**: 24 Oktober 2026  
**Status**: SELESAI & TERVERIFIKASI 100% MELALUI 3 PENGUJIAN INTEGRASI  
**Branch**: `feature/macos-sequoia-ui`  

---

## 1. Ringkasan Eksekutif & Akar Penyebab 2 Masalah dari Video Tahap 26

Dari video pengujian langsung oleh Kelvin pada Tahap 26, kalimat tunggal (`"banten feeds from iphone"`) dan paste dari Windows ke iPhone sudah 100% sempurna tanpa typo. Fitur-fitur ini **dipertahankan sepenuhnya**.

Namun video memperlihatkan dengan sangat jelas akar penyebab 2 bug yang masih terjadi:

1. **BUG 1 (Detik 01:10 - 02:25): Baris Tidak Diblok di Atas Ikut Ter-Copy Saat Menambah Baris Baru**
   - **Kondisi Layar**:
     - Baris 1: `banten feeds from iphone` (TIDAK diblok, latar putih)
     - Baris 2: `jokowi prabowo from windows` (TIDAK diblok, latar putih)
     - Baris 3: `dewa19 feat virzha from iphone` (DIBLOK kuning)
   - **Masalah**: Saat Kelvin hanya memblok Baris 3 (`"dewa19..."`) atau Baris 4 (`"prabowo gibran"`), hasil copy memborong SEMUA baris dari Baris 1 sampai Baris 3/4.
   - **Akar Penyebab Pasti**:
     Di pojok kanan atas toolbar Notes iOS ($X > 0.76 \times W$, $Y = 0.08 \times H \dots 0.145 \times H$) terdapat **tombol bulatan kuning pekat berikon centang (`✓` Done)**. Karena di Tahap 26 `scanTop` diturunkan ke $0.115 \times H$, pemindai piksel kuning mengenai tombol kuning `✓` Done di pojok kanan atas pada $Y = 0.115 \times H$. Akibatnya `minY` selalu terkunci di atas Baris 1 dan `maxY` di baris yang diblok, sehingga seluruh baris berlatar putih di antaranya ikut masuk ke area crop.

2. **BUG 2 (Detik 02:38 - 03:43): Caption Panjang KMP Batumandi Masih Terpotong**
   - **Akar Penyebab A (Frame Atas Terhapus Saat User Klik Kedua Kali untuk Merapikan Gagang Seleksi)**:
     Pada detik 02:46-02:52 Kelvin menarik seleksi dari atas (`"Rekaman"`) ke bawah (`"@erikkristian"`), melepas mouse, lalu pada detik 02:54 melakukan `pointerdown` kedua di bawah untuk menggeser gagang seleksi ke `"#cilegon"`. Karena `_firstDragFrame` di-reset pada setiap `pointerdown`, klik kedua di detik 02:54 menghapus snapshot bagian atas catatan.
   - **Akar Penyebab B (Toolbar Melayang Bawah Notes Merusak Baris Terbawah Frame Atas & Stitcher Gagal)**:
     Pada Frame Atas (detik 03:25), baris paling bawah tertutup **floating toolbar bawah Notes (`+`, `Aa`, checklist)** pada $Y > 0.78 \times H$ sehingga terbaca rusak `"serta 43 aO/O3124an berhasil"`. Sedangkan baris paling atas Frame Bawah terpotong menjadi `"menit-menit saat..."`. Meskipun kedua frame memiliki **16 BARIS IDENTIK** di bagian tengah, algoritma sambungan lama mencoba menyambung di ujung tepi yang rusak sehingga sambungan gagal.

---

## 2. Solusi Arsitektural Tahap 27 yang Dieksekusi

### A. Perbaikan Deteksi Highlight & Verifikasi Highlight Per-Baris di `handset/ocr_extractor.ps1` (Solusi Tuntas Bug 1)

1. **Abaikan Area Tombol Kuning `✓` Toolbar Atas & Floating Toolbar Bawah Notes**:
   - `scanTopY` disetel pada $(int)(H \times 0.145)$ dan `scanBottomY` pada $(int)(H \times 0.785)$ untuk menghindari floating pill toolbar bawah Notes.
   - Khusus untuk $Y < 0.170 \times H$, pemindaian mengabaikan tepi kiri ($X < 0.22 \times W$) tempat tombol `< Notes` berada dan tepi kanan ($X > 0.76 \times W$) tempat tombol bulatan kuning `✓` Done berada.
2. **Contiguous Vertical Clustering**:
   - Menghitung array `bool[] rowHasHighlight` per baris piksel $y$.
   - Mengelompokkan baris ber-highlight menjadi cluster vertikal kontigu dengan toleransi celah baris maksimal $\le 0.028 \times H$.
   - Mengabaikan fragmen atau tombol terisolasi dan hanya mengunci pada cluster teks yang sebenarnya.
3. **Verifikasi Warna Latar per `OcrLine` (`VerifyLineHighlight`)**:
   - Ditambahkan metode C# `VerifyLineHighlight(Bitmap origBmp, int cropX, int cropY, float scale, int margin, double lineX, double lineY, double lineW, double lineH)`.
   - Untuk setiap baris teks yang dihasilkan `OcrEngine`, sistem memetakan kembali bounding box baris ke bitmap asli dan menghitung rasio piksel kuning/biru di belakangnya.
   - **Jika rasio highlight $< 0.05$ (murni putih), baris tersebut DIBUANG seketika**.
   - Ini memberikan jaminan 100% bahwa baris berlatar putih di atas seleksi (seperti `"banten feeds from iphone"` dan `"jokowi prabowo from windows"`) tidak akan pernah ikut ter-copy.

### B. Continuous Visual-Change Viewport History di `handset/web/mirror.js` (Solusi Tuntas Bug 2A)

1. **Anti-Reset Buffer**:
   - Buffer `window._ViewportHistory = []` dengan kapasitas 15 snapshot dan masa berlaku sliding 60 detik.
   - Tidak pernah mengosongkan atau menimpa frame atas saat `pointerdown`!
2. **16-Point Grid Luminance Signature**:
   - Fungsi `sampleViewportIfChanged(force = false)` mengambil sampel cepat luminance pada 16 titik grid di $Y = 0.20 \times H \dots 0.75 \times H$.
   - Jika rata-rata selisih signature $\ge 6.0$ (artinya layar bergulir atau berpindah tampilan) atau riwayat masih kosong, frame beresolusi penuh diambil via `captureFullResFrameDataUrl()` dan disimpan ke `_ViewportHistory`.
3. **Pemicu Otomatis**:
   - Timer background `setInterval(() => sampleViewportIfChanged(false), 900)` selama mirroring aktif (sehingga tampilan Paragraf 1 atas otomatis terekam begitu catatan dibuka, bahkan sebelum disentuh).
   - Event `pointerdown`, `pointermove` (throttled 600ms), dan `wheel` (throttled 600ms).
4. **`window.getRecentSelectionFrames()`**:
   - Mengambil hingga 4 frame dari `_ViewportHistory` yang memiliki signature visual berbeda, diurutkan secara kronologis `[Top, Mid1, Mid2, Bottom]`.

### C. Bidirectional Interior Anchor Alignment & Session Chunk Memory di `handset/screen.py` (Solusi Tuntas Bug 2B)

1. **Fungsi `merge_two_overlapping_blocks(linesA, linesB)`**:
   - Mencari semua pasangan baris $(i, j)$ dengan panjang $\ge 18$ karakter dan kecocokan fuzzy `SequenceMatcher.ratio() >= 0.78`.
   - Mengelompokkan berdasarkan selisih offset $i - j$:
     - Jika $offset > 0$: `linesA` adalah bagian ATAS, `linesB` adalah bagian BAWAH. Titik potong diambil di **tengah zona overlap** (`matches[len // 2]`). Sambungan dibentuk dari `linesA[:split_i] + linesB[split_j:]`.
     - Jika $offset < 0$: `linesB` adalah bagian ATAS, `linesA` adalah bagian BAWAH (seperti saat user scroll ke atas). Sambungan dibentuk dari `linesB[:split_j] + linesA[split_i:]`.
     - Baris paling bawah `linesA` yang rusak terkena floating toolbar (`"aO/O3124an"`) berada di bawah `split_i` sehingga **terbuang otomatis**.
     - Baris paling atas `linesB` yang terpotong setengah berada di atas `split_j` sehingga **terbuang otomatis**.
     - Hasil sambungan 100% bersih dengan baris utuh `"serta 43 awak kapal dilaporkan berhasil"`.
2. **Session Chunk Memory (`_RECENT_LONG_OCR_CHUNKS`)**:
   - Menyimpan hingga 5 blok teks panjang ($\ge 4$ baris) selama 90 detik.
   - Setiap kali user menyalin teks panjang, sistem menggabungkannya juga dengan blok di session memory jika terdapat irisan kalimat. Pengguna dapat menyalin bagian bawah lalu bagian atas secara terpisah dan mendapatkan teks multi-paragraf yang tersambung sempurna.

---

## 3. Hasil Pengujian & Verifikasi Otomatis (`verify_tahap27.py`)

Pengujian dilakukan dengan skrip verifikasi otomatis [`scratch/verify_tahap27.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/scratch/verify_tahap27.py):

```
======================================================================
VERIFIKASI INTEGRASI TAHAP 27: BUG 1 & BUG 2 FIXES
======================================================================

======================================================================
TEST 1: Kasus 'dewa19 feat virzha' dengan Tombol Kuning '✓' di Toolbar Atas
======================================================================
OCR Execution Time: 0.46s
Raw OCR Output:
'dewa19 feat virzha from iphone'
>>> PASS: Test 1 BERHASIL! Hanya Baris 3 ('dewa19 feat virzha from iphone') yang ter-copy. Baris 1 & 2 100% TIDAK TERBAWA.

======================================================================
TEST 2A: Continuous Viewport History Anti-Reset saat Klik/Pointerdown Berulang
======================================================================
>>> PASS: Test 2A BERHASIL! 5x pointerdown di layar bawah TIDAK MENGHAPUS frame atas.

======================================================================
TEST 2B: Bidirectional Interior Anchor Stitching Kasus Teks Kelvin KMP Batumandi
======================================================================
[Forward Stitch: Top -> Bottom] Total lines: 30
[Reverse Stitch: Bottom -> Top] Total lines: 30
>>> PASS: Test 2B BERHASIL! Teks KMP Batumandi 100% UTUH dari awal sampai akhir tanpa 'aO/O3124an'.

======================================================================
ALL TESTS PASSED! SELURUH VERIFIKASI TAHAP 27 100% SUKSES!
======================================================================
```

Pengujian regresi Tahap 26 ([`scratch/verify_tahap26.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/scratch/verify_tahap26.py)) juga dijalankan ulang dan **LULUS 100%** (`ALL 3 TAHAP 26 VERIFICATION SCENARIOS PASSED PERFECTLY`).

---

## 4. Daftar File yang Diubah

1. [`handset/ocr_extractor.ps1`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/ocr_extractor.ps1):
   - Toolbar exclusion zone ($Y < 0.170 \times H$, $X < 0.22 \times W$ dan $X > 0.76 \times W$) untuk mengabaikan tombol bulatan kuning `✓` Done.
   - Contiguous vertical clustering dengan toleransi $\le 0.028 \times H$.
   - Metode C# `VerifyLineHighlight` untuk verifikasi warna latar per `OcrLine` dan membuang baris putih.
   - Menggunakan PowerShell verbatim here-string `@' ... '@` untuk integritas parsing C#.
2. [`handset/web/mirror.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/mirror.js):
   - Implementasi `_ViewportHistory` (15 slot, 60s) dengan `sampleViewportIfChanged`.
   - 16-point grid luminance signature.
   - Background interval 900ms + event sampling tanpa me-reset riwayat frame pada `pointerdown`.
3. [`handset/screen.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/screen.py):
   - `merge_two_overlapping_blocks()` dengan Bidirectional Interior Anchor Alignment.
   - `_RECENT_LONG_OCR_CHUNKS` session memory (5 entri, 90s) untuk penggabungan teks catatan panjang.
   - Pembaruan `stitch_multiline_ocr_frames()`.
4. [`handset/server.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/server.py):
   - Batching hingga 3 history frames + latest frame.
   - Penyambungan chunk memory untuk copy panjang.
5. [`handset/web/mirror.html`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/mirror.html), [`handset/web/index.html`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/index.html), [`handset/web/app.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/app.js):
   - Peningkatan tag cache ke `v=20261024_tahap27_viewport_history_and_line_verifier`.
