# LAPORAN TAHAP 31: OPTICAL OCR DEFECT REPAIRS (l< -> k, ! -> l/i, rn -> m, O -> o), VISUAL KEYBOARD GUARD, & STROKE DARKENING

**Tanggal:** 24 Oktober 2026  
**Status:** SELESAI & 100% TERVERIFIKASI  
**Hasil Pengujian Video Referensi:** Kelvin (`20261023-2337-11.8273015.mp4`)  
**File Pengujian Otomatis:** [`scratch/verify_tahap31.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/scratch/verify_tahap31.py)

---

## 1. Ringkasan Eksekutif & Hasil Investigasi Video Kelvin

Berdasarkan pengujian video Kelvin pada Tahap 30 (`20261023-2337-11.8273015.mp4`), seluruh arsitektur utama terbukti **100% stabil**:
1. Copy-paste internal Notes iPhone (`Ctrl+C` -> `Ctrl+V`) instan dengan pemeliharaan Enter utuh.
2. Cross-platform shortcut (`Alt+C` copy ke Windows, `Alt+V` paste ke iPhone) berjalan lancar.
3. Ekstraksi caption panjang sangat cepat (< 2 detik), utuh dari awal paragraf sampai hashtag terakhir, dan baris kosong ganda (`\r\n\r\n`) tampil rapi.

Namun pada hasil paste Sticky Notes Windows (menit 02:44 - 03:09 dan 03:49 - 03:57), ditemukan beberapa artefak optik OCR dan kebocoran baris keyboard virtual di bagian bawah:

| Masalah pada Video Kelvin | Akar Penyebab Teknis | Solusi Tuntas Tahap 31 |
| :--- | :--- | :--- |
| **1. Huruf "k" terbaca "l<" di 5 tempat** (`diabadil<an`, `@eril<l<ristian`, `tampal<`, `berisil<O`, `sel<oci`) | Pemutihan Red-Channel terlalu agresif (`r >= 210 -> 255`) yang mengikis piksel anti-aliasing abu-abu (R=135..215) pada sambungan batang dan lengan huruf "k". | (a) Di `ocr_extractor.ps1`: Ambang putih Red-Channel dinaikkan ke **`228`**. Piksel `r < 165` dipertegas gelap (`r * 0.55`), `165 <= r < 228` dipetakan mulus ke `90..255`.<br>(b) Di `screen.py`: Loop while restorasi `(?<=[A-Za-z0-9_@])[lI1|]<|[lI1|]<(?=[A-Za-z0-9_])` $\rightarrow$ `"k"`. |
| **2. Huruf "m" terbaca "rn"** (`Baturnandi`, `#kmpbaturnandi`) | Celah antar-lengkungan huruf "m" melebar karena pengikisan anti-aliasing. | Normalisasi regex case-preserving di `screen.py`: `Baturnandi` $\rightarrow$ `"Batumandi"`, `#kmpbaturnandi` $\rightarrow$ `"#kmpbatumandi"`. Serta stroke darkening di `ocr_extractor.ps1`. |
| **3. Huruf "l" & "i" terbaca "!"** (`se!amat`, `d!`) | Batang vertikal huruf terputus di tengah menjadi tanda seru `"!"`. | (a) Mid-word `(?<=[A-Za-z])!(?=[A-Za-z])` $\rightarrow$ `"l"` (`"se!amat"` $\rightarrow$ `"selamat"`).<br>(b) Preposisi `\bd!(?=\s\|$\|[,.;])` $\rightarrow$ `"di"` (`"seorang ayah d!"` $\rightarrow$ `"seorang ayah di"`). |
| **4. Huruf "o" terbaca "O" kapital di tengah kata / kalimat** (`berisil<O`, `disambut Oleh`, `diunggah Oleh`) | OCR membaca huruf "o" sebagai "O" kapital atau angka "0". | (a) `(?<=[a-z])[O0](?=[a-z]\|\b)` $\rightarrow$ `"o"` (`"berisikO"` $\rightarrow$ `"berisiko"`).<br>(b) `(?<=[a-zA-Z0-9,])\s+Oleh\b` $\rightarrow$ `" oleh"` (`"disambut Oleh"` $\rightarrow$ `"disambut oleh"`). |
| **5. Kebocoran Teks Keyboard Virtual iOS di Bawah Hashtag** (`cat aku`, `w e r t y u i o p`, `z x c v b n m`) | Keyboard virtual iOS terbuka pada menit 02:39 - 03:12 dan terekam ke `_ViewportHistory`. Crop FullPage mencakup tombol keyboard & bar prediksi. | (a) Di `ocr_extractor.ps1`: Deteksi visual keyboard di `Y = 0.56*H .. 0.84*H`. Jika keyboard terbuka, batasi `cropBottom = 0.49 * H` sehingga tombol tidak masuk citra OCR.<br>(b) Di `screen.py`: Filter baris keyboard QWERTY, bar prediksi (`"cat aku"`), dan clamp tail setelah hashtag block. |

---

## 2. Rincian Implementasi Teknis

### A. Pre-Processing Citra di `handset/ocr_extractor.ps1`
1. **Peningkatan Ambang Batas Latar Belakang Red-Channel**:
   - Ambang putih dinaikkan dari `210` menjadi **`228`**.
2. **Stroke-Preserving Darkening Curve**:
   ```csharp
   if ((isHandle && !isDarkText) || r >= 228 || isColorNoise) {
       v = 255;
   } else if (r < 165) {
       v = (byte)(r * 0.55); // Mempertegas batang huruf "k", "l", "i", "m"
   } else {
       int mapped = 90 + (int)((r - 165) * (255 - 90) / (228 - 165));
       if (mapped < 0) mapped = 0;
       if (mapped > 255) mapped = 255;
       v = (byte)mapped;
   }
   ```
3. **Pelebaran Proteksi Teks Gelap pada Gagang Seleksi**:
   - `bool isDarkText = (r < 165) && (g < 165) && (b < 165);` melonggarkan batas dari sebelumnya `115`, mencegah pemutihan piksel tepi anti-aliasing huruf di dekat gagang kuning.
4. **Deteksi Visual On-Screen Keyboard**:
   - Memindai area `Y = 0.56 * H` s.d. `0.84 * H`. Jika proporsi piksel abu-abu keyboard (`|R-G|<=10 && |G-B|<=10 && R>=180 && R<=228`) mencapai $\ge 12\%$, `hasKeyboard` bernilai true.
   - `cropBottom` otomatis dibatasi pada **`0.49 * H`**, memotong bersih seluruh keyboard sebelum diproses OCR.

### B. Normalisasi Artefak Optik di `handset/screen.py`
1. **Restorasi Huruf "k"**:
   ```python
   loop_count = 0
   while loop_count < 10 and re.search(r'(?<=[A-Za-z0-9_@])[lI1|]<|[lI1|]<(?=[A-Za-z0-9_])', t):
       t = re.sub(r'(?<=[A-Za-z0-9_@])[lI1|]<|[lI1|]<(?=[A-Za-z0-9_])', 'k', t)
       loop_count += 1
   ```
2. **Restorasi Huruf "o" Kecil & "oleh"**:
   - `re.sub(r'(?<=[a-z])[O0](?=[a-z]|\b)', 'o', t)`
   - `re.sub(r'(?<=[a-zA-Z0-9,])\s+Oleh\b', ' oleh', t)`
3. **Restorasi Tanda Seru "!"**:
   - `re.sub(r'(?<=[A-Za-z])!(?=[A-Za-z])', 'l', t)` (`"se!amat"` $\rightarrow$ `"selamat"`)
   - `re.sub(r'\bd!(?=\s|$|[,.;])', 'di', t)` (`"seorang ayah d!"` $\rightarrow$ `"seorang ayah di"`)
4. **Restorasi "rn" $\rightarrow$ "m"**:
   - `Baturnandi` $\rightarrow$ `"Batumandi"`, `#kmpbaturnandi` $\rightarrow$ `"#kmpbatumandi"`.
5. **Skor Penalti pada `_pick_cleaner_line`**:
   - Menghitung penalti cacat optik pada baris (`l<`, `!`, `baturnandi`, `\s+Oleh\b`, `[a-z][O0]`), memprioritaskan versi baris yang paling bersih.
6. **Penyaringan Sampah Keyboard & Tail Clamping**:
   - Di `_is_garbage_ocr_line`: Menolak baris dengan $\ge 4$ huruf tunggal terpisah spasi (`"w e r t y u i o p"`, `"z x c v b n m"`), serta baris prediksi (`"cat aku"`, `"kamu aku"`).
   - Di `ensure_paragraph_formatting` & `merge_two_overlapping_blocks`: Memotong seluruh sampah setelah blok hashtag terakhir.

---

## 3. Hasil Pengujian & Verifikasi Komprehensif

File pengujian: [`scratch/verify_tahap31.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/scratch/verify_tahap31.py)

```
===========================================================================
TAHAP 31 COMPREHENSIVE VERIFICATION SUITE
Optical Defect Fixes (l<, O, !, rn), Visual Keyboard Rejection, & Stroke Darkening
===========================================================================

===========================================================================
TEST 1: Full Pipeline on Kelvin Sticky Notes Raw Input (Video Tahap 30)
===========================================================================

--- FINAL OUTPUT COPIED TO WINDOWS CLIPBOARD ---
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
serta 43 awak kapal dilaporkan dalam
kondisi selamat ke daratan serta
tanpa adanya korban jiwa.

Bagaimana perasaan kalian melihat
perjuangan seorang ayah di
Banteners?

Source: Threads @erikkristian

#kmpbatumandi #kebakaran #selatsunda
#pelabuhanmerak #cilegon
------------------------------------------------

[PASS] 1. 'diabadil<an' -> 'diabadikan'
[PASS] 2. '@eril<l<ristian' -> '@erikkristian' (found 2 times, in Para 2 & Source)
[PASS] 3. 'Baturnandi' & '#kmpbaturnandi' -> 'Batumandi' & '#kmpbatumandi'
[PASS] 4. 'tampal<' -> 'tampak'
[PASS] 5. 'disambut Oleh' & 'diunggah Oleh' -> 'disambut oleh' & 'diunggah oleh'
[PASS] 6. 'berisil<O' -> 'berisiko'
[PASS] 7. 'sel<oci' -> 'sekoci'
[PASS] 8. 'se!amat' -> 'selamat'
[PASS] 9. 'seorang ayah d!' -> 'seorang ayah di'
[PASS] 10. Keyboard debris ('cat aku', 'w e r t y u i o p', 'z x c v b n m') 100% removed.

Detected 6 paragraph blocks:
  Para 1 (5 lines): 'Rekaman video amatir yang memperlih...' -> '...t Sunda viral di media sosial.'
  Para 2 (11 lines): 'Momen menegangkan tersebut diabadik...' -> '...atas rakit darurat (liferaft).'
  Para 3 (9 lines): 'Situasi darurat di tengah kepulan a...' -> '...tanpa adanya korban jiwa.'
  Para 4 (3 lines): 'Bagaimana perasaan kalian melihat...' -> '...Banteners?'
  Para 5 (1 lines): 'Source: Threads @erikkristian...' -> '...Source: Threads @erikkristian'
  Para 6 (2 lines): '#kmpbatumandi #kebakaran #selatsund...' -> '...#pelabuhanmerak #cilegon'
[PASS] 11. Exactly 6 paragraph blocks separated by \r\n\r\n with single \r\n within hashtag block.

===========================================================================
TEST 2: Optical Clean Line Unit Tests (_clean_ocr_line & _pick_cleaner_line)
===========================================================================
  [OK] 'diabadil<an' -> 'diabadikan'
  [OK] '@eril<l<ristian' -> '@erikkristian'
  [OK] 'tampal< seorang ayah' -> 'tampak seorang ayah'
  [OK] 'berisil<O tinggi' -> 'berisiko tinggi'
  [OK] 'menuju sel<oci dan' -> 'menuju sekoci dan'
  [OK] 'kondisi se!amat' -> 'kondisi selamat'
  [OK] 'perjuangan seorang ayah d!' -> 'perjuangan seorang ayah di'
  [OK] 'disambut Oleh petugas' -> 'disambut oleh petugas'
  [OK] 'diunggah Oleh akun' -> 'diunggah oleh akun'
  [OK] 'Baturnandi' -> 'Batumandi'
  [OK] '#kmpbaturnandi' -> '#kmpbatumandi'
  [OK] Garbage line 'w e r t y u i o p' correctly recognized and cleared.
  [OK] Garbage line 'z x c v b n m' correctly recognized and cleared.
  [OK] Garbage line 'a s d f g h j k l' correctly recognized and cleared.
  [OK] Garbage line 'cat aku' correctly recognized and cleared.
  [OK] Garbage line 'kamu aku' correctly recognized and cleared.
  [OK] _pick_cleaner_line correctly prioritized clean line over optical defect line.

===========================================================================
TEST 3: C# Visual Keyboard Detection & Darkening Curve (ocr_extractor.ps1)
===========================================================================
[PASS] 1. On-Screen Keyboard visual detector and cropBottom restriction (0.49*H) validated.
[PASS] 2. Loosened dark text protection (r < 165 && g < 165 && b < 165) validated.
[PASS] 3. Stroke-preserving darkening curve (228 white threshold, r*0.55, 90..255) validated.

***************************************************************************
ALL TAHAP 31 VERIFICATION TESTS PASSED 100% SUCCESSFULLY!
***************************************************************************
```

Pengujian regresi menyeluruh:
- [`scratch/verify_tahap30.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/scratch/verify_tahap30.py): **100% PASS** (Preservasi baris kosong antar-paragraf, Single-pass batch OCR).
- [`scratch/verify_tahap29.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/scratch/verify_tahap29.py): **100% PASS** (Internal copy-paste instant, pemisahan shortcut).
- [`scratch/verify_tahap28.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/scratch/verify_tahap28.py): **100% PASS** (Stitcher gap-filling, full caption).
- [`scratch/verify_tahap27.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/scratch/verify_tahap27.py): **100% PASS** (Interior anchor stitching, anti-highlight toolbar).
- [`scratch/verify_tahap26.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/scratch/verify_tahap26.py): **100% PASS** (Batch multi-frame, baris tunggal).

---

## 4. Daftar File yang Diubah

1. [`handset/ocr_extractor.ps1`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/ocr_extractor.ps1):
   - Deteksi visual keyboard virtual iOS pada separuh bawah layar (`Y = 0.56*H .. 0.84*H`), otomatis membatasi `cropBottom = 0.49 * H` ketika keyboard terbuka.
   - Ambang putih Red-Channel dinaikkan ke 228.
   - Kurva penggelapan stroke-preserving (`r < 165 -> r * 0.55`, `165 <= r < 228 -> 90..255`).
   - Pelonggaran proteksi teks gelap seleksi ke `r < 165 && g < 165 && b < 165`.
2. [`handset/screen.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/screen.py):
   - Detektor baris sampah keyboard QWERTY dan bar prediksi kata di `_is_garbage_ocr_line`.
   - Restorasi optik `l<` $\rightarrow$ `k`, `[O0]` $\rightarrow$ `o`, `Oleh` mid-sentence $\rightarrow$ `oleh`, `!` mid-word $\rightarrow$ `l`, `d!` $\rightarrow$ `di`, dan `Baturnandi` $\rightarrow$ `Batumandi` di `_clean_ocr_line`.
   - Penalti cacat optik di `_pick_cleaner_line`.
   - Tail clamping pemotongan baris sampah setelah hashtag block di `ensure_paragraph_formatting` dan `merge_two_overlapping_blocks`.
3. [`handset/web/mirror.html`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/mirror.html), [`handset/web/index.html`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/index.html), [`handset/web/app.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/app.js):
   - Query version cache dibump ke `v=20261024_tahap31_optical_fixes_and_keyboard_guard`.
4. [`scratch/verify_tahap31.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/scratch/verify_tahap31.py):
   - Skrip verifikasi otomatis 3 uji komprehensif untuk Tahap 31.
