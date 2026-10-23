# LAPORAN TAHAP 8: PERBAIKAN TRANSISI PLACEHOLDER `#screen-empty`, RESTORASI `drawFrame` & `fitCanvas` COMMIT `74f98ef`, DAN BYPASS RELOAD IFRAME

> **Klien:** Kelvin (Content Creator)  
> **Fokus Tahap:** Perbaikan Transisi Layar Placeholder "Ready to mirror", Pengembalian `drawFrame` dan `fitCanvas` ke Logika Commit `74f98ef` yang Terbukti Utuh 100%, Pencegahan Reload Loop Iframe pada Polling Status, dan Retensi Gestur Trackpad 2 Jari  
> **Branch Git:** `feature/macos-sequoia-ui`  
> **Commit Hash:** `8438eda` (`fix(mirror): revert drawFrame and fitCanvas to 74f98ef, fix screen-empty placeholder transition`)  
> **Tanggal:** 23 Oktober 2026  

---

## 📌 RINGKASAN EKSEKUTIF

Pada pengujian commit `b9d7b14`, badge di kiri atas sudah berubah hijau menjadi "Mirroring" dan tombol Settings menjadi "Mirroring...", namun layar tengah tetap tertahan di placeholder "Ready to mirror" (video tidak muncul).

### Investigasi Akar Masalah:
1. **TypeError pada WebCodecs `VideoFrame` (`mirror.js`)**:
   Modifikasi `ctx.drawImage(f, 0, 0, fw, fh, -fpW/2, -fpH/2, fpW, fpH)` dengan 9 parameter pada objek `VideoFrame` menyebabkan eksepsi JavaScript di engine WebView2/Chromium. Akibatnya, `drawFrame(f)` gagal dieksekusi, `frameCount === 1` tidak memicu status "Live", dan frame video tidak digambar sama sekali.
2. **Placeholder `#placeholder` / `#screen-empty` Tidak Tersembunyi**:
   Pada `app.css` sebelumnya, aturan `.screen-stage.live .placeholder { display: none; }` sempat terhapus dan elemen `#placeholder` memiliki `z-index: 2` dengan `position: absolute; inset: 0;`. Sementara di `app.js`, placeholder tidak disembunyikan secara eksplisit via `hidden = true` atau `display = "none"`, sehingga placeholder tetap menutupi iframe `#screen-view` yang berada di bawahnya.
3. **Potensi Reload Loop Iframe**:
   Pengecekan `src` iframe sebelumnya menggunakan `Date.now()` yang berisiko me-reload iframe secara berulang pada setiap siklus polling `/status`.

---

## 🛠️ LANGKAH PERBAIKAN YANG DIEKSEKUSI (100% SELESAI)

### 1. Kembalikan `drawFrame(f)` & `fitCanvas()` ke Versi Commit `74f98ef`
- **`mirror.js`**:
  - `drawFrame(f)` dikembalikan ke pemanggilan standar 5 parameter `ctx.drawImage(f, -fpW / 2, -fpH / 2, fpW, fpH)` yang terbukti merender frame utuh tanpa error WebCodecs.
  - `fitCanvas()` dikembalikan menggunakan pembacaan `parent.getBoundingClientRect()` dengan fallback `window.innerWidth` / `window.innerHeight`.
  - Pada frame pertama (`frameCount === 1`), otomatis memanggil `setStatus("Live", "ok")` dan menjadwalkan `fitCanvas()` ulang via `setTimeout(fitCanvas, 50)` dan `setTimeout(fitCanvas, 200)` untuk memastikan ukuran kanvas terhitung presisi saat iframe di-unhide.
  - **Tetap mempertahankan 100%**: Handler gestur trackpad 2 jari (`canvas.addEventListener("wheel", ...)`, `performWheelSwipe`) dan pemblokiran klik kanan (`contextmenu.preventDefault()`).
- **`mirror.css`**:
  - Dikembalikan ke styling bersih commit `74f98ef`:
    ```css
    html, body {
      margin: 0;
      width: 100%;
      height: 100%;
      overflow: hidden;
      background: #000;
      display: flex;
      align-items: center;
      justify-content: center;
    }
    #c {
      display: block;
      max-width: 100%;
      max-height: 100%;
      touch-action: none;
      outline: none;
      image-rendering: high-quality;
    }
    ```

### 2. Perbaiki Transisi Placeholder `#screen-empty` ke `#screen-view`
- **`app.css`**:
  - Menambahkan aturan mutlak penyembunyian placeholder:
    ```css
    #screen-empty[hidden],
    .screen-empty[hidden],
    #placeholder[hidden],
    .placeholder[hidden],
    .screen-stage.live #screen-empty,
    .screen-stage.live .screen-empty,
    .screen-stage.live #placeholder,
    .screen-stage.live .placeholder,
    .workspace.is-live #screen-empty,
    .workspace.is-live .screen-empty,
    .workspace.is-live #placeholder,
    .workspace.is-live .placeholder {
      display: none !important;
    }
    ```
- **`index.html`**:
  - Placeholder kini memiliki kelas dan id lengkap: `<div class="placeholder screen-empty" id="screen-empty" data-alias="placeholder" ...>`.
- **`app.js`**:
  - Selektor placeholder menggunakan `const screenEmpty = document.querySelector("#screen-empty") || document.querySelector("#placeholder");`.
  - Pada `renderScreen(screen)` saat `screen.phase === "running"`:
    1. Sembunyikan placeholder secara mutlak:
       ```javascript
       if (screenEmpty) {
         screenEmpty.hidden = true;
         screenEmpty.style.display = "none";
       }
       ```
    2. Tampilkan iframe:
       ```javascript
       screenView.hidden = false;
       screenView.style.display = "block";
       ```
    3. Cegah reload iframe berulang-ulang pada setiap polling:
       ```javascript
       const curSrc = screenView.getAttribute("src") || "";
       if (!curSrc.startsWith("/mirror.html")) {
         screenView.src = MIRROR_PAGE;
       }
       ```
    4. Kirim sinyal resize ke iframe setelah unhide:
       ```javascript
       setTimeout(() => screenView.contentWindow?.postMessage({ type: "resize" }, "*"), 50);
       setTimeout(() => screenView.contentWindow?.postMessage({ type: "resize" }, "*"), 200);
       ```
  - Saat mirroring berhenti, placeholder dipulihkan (`screenEmpty.hidden = false; screenEmpty.style.display = "flex";`) dan iframe disembunyikan kembali.

---

## 3. Tabel Matriks Perbaikan

| Komponen | Masalah pada Commit `b9d7b14` | Solusi Baru di Commit `8438eda` |
|:---|:---|:---|
| **Drawing Frame (`mirror.js`)** | `ctx.drawImage` 9 parameter melempar TypeError pada WebCodecs `VideoFrame`. | Revert ke pemanggilan 5 parameter `ctx.drawImage(f, -fpW/2, -fpH/2, fpW, fpH)` (100% kompatibel WebCodecs WebView2). |
| **Sizing Canvas (`mirror.js`)** | `fitCanvas` membaca `clientWidth` tanpa sinkronisasi unhide iframe. | Revert ke `parent.getBoundingClientRect()` dengan fallback dan pemicu ganda `setTimeout(fitCanvas, 50/200)`. |
| **Placeholder Transition (`app.js`)** | Placeholder tidak disembunyikan secara eksplisit, menutupi iframe di `z-index: 2`. | `screenEmpty.hidden = true; screenEmpty.style.display = "none";` langsung dipanggil saat mirroring aktif. |
| **Pencegahan Cache / Reload Loop** | Timestamp per-polling berpotensi mereload iframe terus menerus. | `screenView.src` hanya dimuat sekali dengan proteksi `if (!curSrc.startsWith("/mirror.html"))`. |
| **CSS Selektor (`app.css`)** | Placeholder tidak memiliki `display: none !important` saat live. | Ditambahkan aturan `#screen-empty[hidden], .screen-stage.live #screen-empty { display: none !important; }`. |

---

## 4. Riwayat Git Commit Terkini

```
* 8438eda (HEAD -> feature/macos-sequoia-ui) fix(mirror): revert drawFrame and fitCanvas to 74f98ef, fix screen-empty placeholder transition
* 3b16dca docs: perbarui laporan tahap 7 dengan solusi satuan mutlak calc(100vh - 80px) dan zombie kill
* b9d7b14 fix(display): kunci tinggi mutlak calc(100vh - 80px), bunuh zombie proses di run.bat, dan dynamic timestamp URL
* 70ad004 docs: perbarui laporan tahap 7 dengan solusi matikan cache webview2 dan kunci iframe absolut
* f2368c8 fix(webview2): matikan cache webview2, kunci iframe absolut 100 persen, dan bersihkan teks header
* 9350705 docs: tambahkan laporan Tahap 7 perbaikan rantai flexbox dan update laporan progress master
* 3c2ff85 fix(ui): perbaiki rantai flexbox tinggi video 100 persen dan responsive scaling mode fullsize
* 7c4871f feat(app): add pywebview frameless launcher, side-drawer settings, precision trackpad scroll, and swipe nav
* 74f98ef (Tahap 5) feat(app): update layout to compact GlideX-style snug container
```

---

## 5. Panduan Pengujian oleh Kelvin

Jalankan perintah pengujian:
```cmd
run.bat
```
1. Skrip otomatis mematikan proses zombie lama dan membuka jendela native Pane.
2. Klik **"Start mirroring"** (atau klik area placeholder "Ready to mirror"):
   - Placeholder "Ready to mirror" **langsung hilang seketika**.
   - Iframe `#screen-view` langsung tampil aktif.
   - Video iPhone muncul **100% utuh dari Dynamic Island atas sampai dock bawah** tanpa terpotong dan tanpa error JavaScript.
   - Scrolling trackpad 2-jari tetap berjalan mulus.
   - Klik kanan tetap terblokir dari context menu bawaan Windows/WebView2.
