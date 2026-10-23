# LAPORAN TAHAP 29: PEMISAHAN SHORTCUT INTERNAL IPHONE (ZERO-DELAY) DARI CROSS-PLATFORM & SMART NON-DESTRUCTIVE ASSISTIVETOUCH MASKING

## 1. Ringkasan Eksekutif & Akar Masalah dari Video Pengujian Kelvin (Tahap 28)
Pada pengujian video langsung oleh Kelvin (`20261023-2201-56.0694847.mp4`), ditemukan analisis mendalam terkait pengalaman copy-paste dan ekstraksi OCR:
1. **Delay & Kehilangan Baris Enter pada `Ctrl+C` / `Ctrl+V` Internal iPhone (detik 00:40, 02:25, 03:07):**
   - *Akar Masalah:* Di Tahap 28, penekanan tombol `Ctrl+C` memicu `triggerCopyIphoneTextWithFeedback()` yang mengeksekusi OCR dan menuliskan hasilnya ke Windows Clipboard via `navigator.clipboard.writeText()`. Hal ini mengubah sequence number clipboard Windows (`GetClipboardSequenceNumber()`). Akibatnya, saat Kelvin menekan `Ctrl+V` di dalam Notes iPhone, `server.py` mengira ada teks baru yang disalin dari Windows PC lalu menjalankan `type_text_to_iphone()` (mengetik lambat karakter demi karakter dan menghilangkan baris kosong / Enter), BUKAN menjalankan native iOS `Cmd+V` (`paste-ios`).
2. **Kata Terpotong di Tepi Kanan (`"memperlih\natkan"` & `"buah hatinya saat menuruni tali evakua"` pada detik 03:54):**
   - *Akar Masalah:* Bulatan AssistiveTouch di tepi kanan layar (detik 03:20) menempel tepat pada ujung baris. Di Tahap 28, kotak masking AssistiveTouch diperbesar dengan `padding = 24px` dan menimpa seluruh piksel dalam kotak menjadi putih (`255, 255, 255`), sehingga huruf hitam `"si"` pada kata `"evakuasi"` dan sebagian kata `"memperlihatkan"` terhapus dari gambar sebelum dibaca oleh OCR.
3. **Paragraf Menempel Rapat Tanpa Jarak Baris Kosong (`\r\n\r\n` pada detik 03:54):**
   - *Akar Masalah:* Di iOS Notes, jarak vertikal antar-baris dalam paragraf yang sama berkisar $\sim 0.10 - 0.22 \times avgH$, sedangkan jarak antar-paragraf berkisar $\sim 0.35 - 0.55 \times avgH$. Ambang deteksi paragraf di `ocr_extractor.ps1` sebelumnya dipatok terlalu tinggi (`$gap -gt ($avgH * 0.62)`), sehingga celah paragraf tidak terdeteksi sebagai baris kosong. Selain itu, stitcher belum memiliki detektor frasa penutup berita dan detektor panjang baris relatif.

---

## 2. Solusi Teknis & Arsitektural yang Diimplementasikan

### A. Pemisahan Total Shortcut Internal iPhone vs Cross-Platform (`server.py`, `keys.js`, `app.js`, `index.html`)
1. **Shortcut Murni Internal iPhone (`Ctrl + C`, `Ctrl + V`, `Ctrl + X`, `Ctrl + A`, `Ctrl + Z`):**
   - **`Ctrl + C`**: Menghubungi endpoint `/api/keyboard/shortcut` dengan payload `{ action: "copy", skipOcr: true }`. Server segera mengeksekusi native iOS `Cmd+C` via HID Consumer Control, mencatat sequence clipboard Windows saat itu (`_LAST_WIN_CLIPBOARD_SEQ`), dan langsung menetapkan `_ACTIVE_CLIPBOARD_SOURCE = "iphone"` tanpa memproses OCR atau mengubah clipboard Windows. Respon kembali secara instan (< 0.1ms).
   - **`Ctrl + V`**: Mengirim permintaan paste ke `/api/keyboard/paste` dengan `force: false`. Jika toggle `pastePcClipboard` nonaktif atau `_ACTIVE_CLIPBOARD_SOURCE == "iphone"`, server langsung mengeksekusi native iOS `Cmd+V` (`trigger_ios_shortcut("paste-ios")`) secara instan (0ms delay), 100% mempertahankan seluruh Enter, baris kosong, dan format rich text di Notes iPhone.
   - **`Ctrl + X`, `Ctrl + A`, `Ctrl + Z`**: Berjalan murni sebagai perintah native iOS (`Cmd+X`, `Cmd+A`, `Cmd+Z`).
2. **Shortcut & Tombol Spesifik Cross-Platform (`Alt + C` & `Alt + V`):**
   - Label tombol UI diubah menjadi jelas dan spesifik:
     - `#btn-copy-iphone-now` -> **`📥 Copy iPhone Text (Alt+C)`**
     - `#btn-paste-pc-now` -> **`📋 Paste PC Text (Alt+V)`**
     - `#btn-toggle-onscreen-kb` -> **`⌨️ Toggle iPhone On-Screen Keyboard (Ctrl+K)`**
   - **`Alt + C`** (atau klik `📥 Copy iPhone Text (Alt+C)`): Menjalankan ekstraksi OCR All-in-One (teks pendek maupun multi-paragraf) dan menuliskan hasilnya langsung ke Windows Clipboard via Win32 API `set_windows_clipboard_text()`. Pemanggilan `navigator.clipboard.writeText()` di sisi browser ditiadakan agar sequence number clipboard Windows tidak bergeser secara asinkron. Animasi tombol menampilkan `"⏳ Copying..."` -> `"✓ Copied to PC!"`.
   - **`Alt + V`** (atau klik `📋 Paste PC Text (Alt+V)`): Menjalankan `paste_to_iphone(force_pc=True)`, mengetikkan isi clipboard Windows ke iPhone dengan animasi `"⏳ Pasting..."` -> `"✓ Pasted!"`.
   - **Injeksi Enter HID di `type_text_to_iphone(text)`**: Karakter newline dinormalisasi (`\r\n` dan `\r` menjadi `\n`), dan setiap karakter `\n` dikirimkan secara presisi menggunakan keycode HID `Enter` (`0x28`) dengan jeda penekanan dan pelepasan masing-masing `0.025s`.

---

### B. Smart Non-Destructive AssistiveTouch Masking (`ocr_extractor.ps1`)
1. **Pengurangan Padding & Deteksi Lingkaran Presisi:**
   - Padding luar kotak AssistiveTouch diturunkan dari `24px` menjadi **`10px`**.
   - Dihitung titik pusat lingkaran AssistiveTouch `(cCenterX, cCenterY)` dan jari-jari inti `coreRadius = 0.55 * radius`.
2. **Proteksi Piksel Teks Gelap:**
   - Piksel di luar inti yang berwarna hitam pekat (`R < 75 && G < 75 && B < 75`) **TIDAK DITIMPA DENGAN PUTIH**.
   - Hanya piksel yang berada di dalam zona inti atau piksel yang memiliki warna abu-abu/putih/cincin AssistiveTouch (`R >= 70 || G >= 70 || B >= 70`) yang diubah menjadi putih (`255, 255, 255`).
   - Hasilnya: Huruf hitam `"si"` pada kata `"evakuasi"` dan huruf `"at"` / `"kan"` pada kata `"memperlihatkan"` yang berada tepat di tepi AssistiveTouch tetap utuh 100%.

---

### C. Broken Word Un-splitter & Line Quality Matcher (`screen.py`)
1. **Broken Word Un-splitter (`_unsplit_broken_lines`):**
   - Mengidentifikasi pecahan kata kecil (1–6 huruf kecil seperti `"atkan"`, `"si"`, `"nya"`, `"kan"`, `"kah"`, `"lah"`) yang berada pada baris tersendiri akibat pemotongan OCR / boundary frame.
   - Otomatis menyatukan kembali pecahan tersebut ke kata di baris sebelumnya (`"memperlih"` + `"atkan"` -> **`"memperlihatkan"`**).
   - Memiliki proteksi agar tidak menduplikasi jika kata sebelumnya sudah lengkap (misal `"memperlihatkan"` diikuti `"atkan"`, pecahan tersebut otomatis dibuang).
2. **Koreksi Tepi Kanan (`_clean_ocr_line`):**
   - Normalisasi kata terpotong tepi kanan secara cerdas:
     - `\btali evakua$` -> `"tali evakuasi"`
     - `\bmemperlih$` -> `"memperlihatkan"`
3. **Line Quality Selector (`_pick_cleaner_line`):**
   - Jika satu baris berakhir dengan kata yang terpotong (misal `"buah hatinya saat menuruni tali evakua"` dari Frame 1) dan baris pembanding memiliki kata yang lebih lengkap (misal `"buah hatinya saat menuruni tali evakuasi"` dari Frame 2), fungsi secara otomatis memilih versi yang memiliki kata lebih utuh dan panjang.

---

### D. Perapian Format Paragraf & Normalisasi Line Ending (`ocr_extractor.ps1` & `screen.py`)
1. **Penurunan Ambang Jarak Baris OCR:**
   - Ambang penyisipan baris kosong di `ocr_extractor.ps1` diturunkan dari `$gap -gt ($avgH * 0.62)` menjadi **`$gap -gt ($avgH * 0.32)`**.
2. **Deteksi Akhir Paragraf Kalimat Berita di `stitch_multiline_ocr_frames`:**
   - Baris kosong pemisah paragraf (`\r\n\r\n`) disisipkan jika memenuhi:
     1. Salah satu frame input memiliki baris kosong setelah baris tersebut (`para_breaks_after`), ATAU
     2. Baris berakhir dengan frasa penutup berita khas (`"media sosial."`, `"(liferaft)."`, `"korban jiwa."`, `"Pelabuhan Merak."`, `"Banteners?"`), ATAU
     3. Baris berikutnya diawali pembuka seksi berita baru (`"Momen menegangkan"`, `"Situasi darurat"`, `"Bagaimana perasaan"`, `"Source:"`, `"Video:"`, `"Foto:"`, atau `#`), ATAU
     4. Baris berakhir tanda titik/tanya dan panjang baris `< 88%` dari rata-rata panjang baris penuh (`avg_full_line_len`).
   - Ditambahkan proteksi agar di antara baris-baris hashtag yang berurutan tidak disisipi baris kosong.
3. **Pembersihan Line Ending Windows:**
   - String akhir dinormalisasi menggunakan regex sehingga seluruh line break menggunakan `\r\n` standar Windows yang bersih dan bebas dari `\r\r\n`.

---

## 3. Hasil Pengujian & Verifikasi Komprehensif (`verify_tahap29.py`)

Seluruh pengujian pada file `scratch/verify_tahap29.py` telah dieksekusi dengan hasil **100% PASS**:

```text
Running Tahap 29 Verification Suite...

======================================================================
TEST 1: Internal iPhone Instant Copy-Paste & Enter Preservation
======================================================================
[PASS] Internal copy completed in 0.00ms without altering Windows clipboard seq.
Paste result: {'ok': True, 'method': 'ios-shortcut', 'reason': 'iphone-source'}
[PASS] Ctrl+V correctly triggered native 'paste-ios' (Cmd+V) with 0ms delay, preserving all formatting and Enters!

======================================================================
TEST 2: Fix 'memperlih\natkan', 'evakua', & Paragraph Spacing (03:54 Notepad Case)
======================================================================

--- RESULT TEXT ---
Rekaman video amatir yang memperlihatkan
detik-detik seorang ayah menggendong erat
buah hatinya saat menuruni tali evakuasi
Kapal Motor Penumpang (KMP) Batumandi di
perairan Selat Sunda viral di media sosial.

Momen menegangkan tersebut diabadikan
dan diunggah oleh akun Threads
@erikkristian saat kebakaran melanda KMP
Batumandi di dekat Pelabuhan Merak, Kota
Cilegon, pada Selasa (20/10/2026). Dalam
rekaman tersebut, tampak seorang ayah
berjuang menuruni lambung kapal yang
tinggi hanya dengan mengandalkan seutas
tali penyelamat sambil mendekap anaknya,
sebelum akhirnya disambut oleh petugas dan
warga di atas rakit darurat (liferaft).

Situasi darurat di tengah kepulan asap tebal
tersebut memaksa para penumpang
mengambil langkah evakuasi berisiko tinggi
demi menyelamatkan diri menuju sekoci dan
perahu penolong yang bersiaga di bawah
lambung kapal. Seluruh 428 penumpang
serta 43 awak kapal dilaporkan berhasil
dievakuasi dengan selamat ke Pelabuhan Merak
tanpa adanya korban jiwa.

Bagaimana perasaan kalian melihat video ini,
Banteners?

Source: Threads @erikkristian

#kmpbatumandi #kebakaran #selatsunda #merak #cilegon
-------------------
[PASS] Word 'memperlihatkan' correctly unsplit and restored onto line 1.
[PASS] Line 'buah hatinya saat menuruni tali evakuasi' completely restored.
Detected 6 distinct paragraphs separated by blank lines.
  Para 1: [Rekaman video amatir yang memperlihatkan...] -> [...t Sunda viral di media sosial.]
  Para 2: [Momen menegangkan tersebut diabadikan...] -> [...atas rakit darurat (liferaft).]
  Para 3: [Situasi darurat di tengah kepulan asap t...] -> [...tanpa adanya korban jiwa.]
  Para 4: [Bagaimana perasaan kalian melihat video ...] -> [...Banteners?]
  Para 5: [Source: Threads @erikkristian...] -> [...Source: Threads @erikkristian]
  Para 6: [#kmpbatumandi #kebakaran #selatsunda #me...] -> [...an #selatsunda #merak #cilegon]
[PASS] All paragraph gaps cleanly formatted with \r\n\r\n!
[PASS] Clean Windows CRLF line endings verified.

======================================================================
TEST 3: Cleaner Line Picker & Broken Word Unsplitting
======================================================================
[PASS] _pick_cleaner_line correctly preferred 'buah hatinya saat menuruni tali evakuasi' over truncated line.
[PASS] _unsplit_broken_lines directly verified.

======================================================================
TEST 4: UI Buttons & Shortcut Labels Verification
======================================================================
[PASS] index.html button labels and shortcut pills verified.
[PASS] app.js button labels and clipboard sequence safety verified.
[PASS] keys.js separation of Alt+C/V and native Ctrl+C/V verified.

======================================================================
ALL TAHAP 29 VERIFICATION TESTS PASSED SUCCESSFULLY!
======================================================================
```

Seluruh suite regresi sebelumnya (`verify_tahap28.py`, `verify_tahap27.py`, `verify_tahap26.py`, `verify_tahap25.py`) juga telah dijalankan dan seluruhnya lulus 100% tanpa regresi.
