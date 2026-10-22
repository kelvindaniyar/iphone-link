# LAPORAN TAHAP 7: PERBAIKAN VISUAL RANTAI FLEXBOX TINGGI VIDEO 100%, BYPASS CACHE WEBVIEW2 & MODE FULL SIZE RESPONTIF

> **Klien:** Kelvin (Content Creator)  
> **Fokus Tahap:** Matikan Cache WebView2 Permanen, Kunci Selektor Tag `iframe` Secara Mutlak, Responsiveness Full Size Mode, Sinkronisasi Canvas terhadap `window.innerWidth/innerHeight`, dan Pembersihan Teks Header  
> **Branch Git:** `feature/macos-sequoia-ui`  
> **Commit Hash:** `f2368c8` (`fix(webview2): matikan cache webview2, kunci iframe absolut 100 persen, dan bersihkan teks header`)  
> **Tanggal:** 23 Oktober 2026  

---

## 📌 RINGKASAN EKSEKUTIF

Ketika Kelvin menguji Tahap 7, ditemukan kendala bahwa **tampilan video masih belum berubah (tetap gepeng ~180px)**.
Setelah investigasi mendalam, ditemukan 2 akar masalah krusial:
1. **Agresivitas Cache WebView2 Windows**: Microsoft Edge WebView2 menyimpan cache file statis (`app.css`, `mirror.css`, `mirror.js`) ke direktori lokal `%LOCALAPPDATA%`, sehingga browser desktop tetap memuat file CSS lama tanpa menerapkan aturan baru.
2. **Selektor Tag `iframe` & Perhitungan Canvas**: Selektor CSS sebelumnya mengandalkan tebakan nama class, dan `fitCanvas()` mengukur `parentElement.getBoundingClientRect()` yang saat render awal masih berupa nilai default (150–180px).

Pada pembaruan ini, seluruh instruksi perbaikan menyeluruh Kelvin telah dieksekusi 100%:
1. **Matikan Cache WebView2 Secara Permanen (`server.py` + Cache-Busting)**:
   - Override method `end_headers` di `server.py` untuk menyuntikkan header anti-cache pada setiap respons HTTP:
     ```http
     Cache-Control: no-store, no-cache, must-revalidate, max-age=0
     Pragma: no-cache
     Expires: 0
     ```
   - Seluruh pemanggilan resource di `index.html` dan `mirror.html` ditambahkan query parameter versi unik: `?v=20261023_fix`.
   - URL awal di `gui.py` dan `MIRROR_PAGE` di `app.js` juga disematkan query parameter unik `?v=20261023_fix`.
2. **Kunci Kontainer Layar & Tag `iframe` Secara Mutlak**:
   - `#phone-screen` dan wrapper display dikunci dengan:
     ```css
     flex: 1 1 auto !important;
     width: 100% !important;
     height: 100% !important;
     min-height: 0 !important;
     position: relative !important;
     display: flex !important;
     overflow: hidden !important;
     ```
   - Menargetkan tag `iframe` murni tanpa ketergantungan class:
     ```css
     iframe {
       position: absolute !important;
       inset: 0 !important;
       width: 100% !important;
       height: 100% !important;
       border: none !important;
       display: block !important;
     }
     ```
3. **Penyelarasan `mirror.css` & `mirror.js`**:
   - `mirror.css`: `html, body { width: 100%; height: 100%; margin: 0; padding: 0; overflow: hidden; background: #000; display: flex; align-items: center; justify-content: center; }` dan `#c { max-width: 100%; max-height: 100%; object-fit: contain; display: block; }`.
   - `mirror.js`: `fitCanvas()` kini murni menghitung aspek rasio video stream terhadap `window.innerWidth` dan `window.innerHeight` iframe, serta dipanggil setiap kali frame baru diterima (`drawFrame()`) dan saat jendela di-resize.
4. **Pembersihan Teks Header**:
   - Teks "Plug in iPhone" dan "Checking…" di pojok kiri atas Top Bar telah dihapus secara bersih sehingga Top Bar tampil minimalis dan rapi.

---

## 1. Analisis Akar Masalah (Root Cause) & Solusi

| No | Masalah | Penyebab Teknis | Solusi yang Diterapkan |
|:---|:---|:---|:---|
| 1 | File CSS baru tidak diterapkan | Microsoft Edge WebView2 melakukan agresif caching terhadap file statis lokal. Server sebelumnya hanya mengirim `Cache-Control: no-store` tanpa `Pragma: no-cache` dan `Expires: 0`, serta tidak ada query parameter versi. | Override `end_headers` di `handset/server.py` menyuntikkan `Cache-Control: no-store, no-cache, must-revalidate, max-age=0`, `Pragma: no-cache`, `Expires: 0`. Menambahkan query string `?v=20261023_fix` pada pemanggilan CSS/JS dan URL pywebview. |
| 2 | Iframe collapse ~180px | Tag `iframe` tidak terkunci secara mutlak ke kontainer relatif induk, dan CSS bergantung pada kecocokan nama class selector (`.screen-frame`). | Kunci kontainer `#phone-screen` dengan `position: relative !important; flex: 1 1 auto !important; width: 100% !important; height: 100% !important;`. Kunci tag `iframe` dengan `position: absolute !important; inset: 0 !important; width: 100% !important; height: 100% !important; border: none !important;`. |
| 3 | Canvas gepeng saat video live | `fitCanvas()` mengukur `parent.getBoundingClientRect()` yang membaca tinggi lama atau 0 saat awal stream, sehingga mengunci ukuran pixel gepeng. | `fitCanvas()` kini murni membaca `window.innerWidth` dan `window.innerHeight` iframe, lalu dipanggil secara reaktif pada setiap frame video yang diterima (`drawFrame()`). |
| 4 | Teks header "Plug in iPhone" | Logika `renderPills` di `app.js` menyetel teks "Plug in iPhone" saat kabel belum terhubung. | Menghapus teks tersebut dan menyembunyikan pill (`pillConnection.hidden = true;`) saat perangkat tidak terhubung agar header bersih. |

---

## 2. Rincian Perubahan Kode

### A. Nonaktifkan Cache WebView2 di [`handset/server.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/server.py)

```python
class Handler(BaseHTTPRequestHandler):
    def end_headers(self) -> None:
        self.send_header("Cache-Control", "no-store, no-cache, must-revalidate, max-age=0")
        self.send_header("Pragma", "no-cache")
        self.send_header("Expires", "0")
        super().end_headers()
```

### B. Cache-Busting Version Query Parameter

1. **[`handset/gui.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/gui.py)**:
   ```python
   window = webview.create_window(
       title="iPhone Mirroring",
       url="http://127.0.0.1:8765/?v=20261023_fix",
       ...
   )
   ```
2. **[`handset/web/index.html`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/index.html)**:
   ```html
   <link rel="stylesheet" href="/app.css?v=20261023_fix">
   ...
   <script src="/keys.js?v=20261023_fix"></script>
   <script src="/app.js?v=20261023_fix"></script>
   ```
3. **[`handset/web/mirror.html`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/mirror.html)**:
   ```html
   <link rel="stylesheet" href="/mirror.css?v=20261023_fix">
   ...
   <script src="/keys.js?v=20261023_fix"></script>
   <script src="/mirror.js?v=20261023_fix"></script>
   ```
4. **[`handset/web/app.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/app.js)**:
   ```javascript
   const MIRROR_PAGE = "/mirror.html?v=20261023_fix";
   ```

### C. Penguncian Mutlak Kontainer & Tag `iframe` di [`handset/web/app.css`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/app.css)

```css
#phone-screen,
.screen-viewport {
  flex: 1 1 auto !important;
  width: 100% !important;
  height: 100% !important;
  min-height: 0 !important;
  position: relative !important;
  display: flex !important;
  overflow: hidden !important;
  align-items: center;
  justify-content: center;
  background: #000;
}

iframe {
  position: absolute !important;
  inset: 0 !important;
  width: 100% !important;
  height: 100% !important;
  border: none !important;
  display: block !important;
  background: #000;
}

iframe[hidden],
#screen-view[hidden],
#mirror-frame[hidden],
.screen-frame[hidden] {
  display: none !important;
}
```

### D. Perbaikan [`handset/web/mirror.css`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/mirror.css) & [`handset/web/mirror.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/mirror.js)

1. **`mirror.css`**:
   ```css
   html,
   body {
     width: 100%;
     height: 100%;
     margin: 0;
     padding: 0;
     overflow: hidden;
     background: #000;
     display: flex;
     align-items: center;
     justify-content: center;
   }

   body {
     position: relative;
     touch-action: none;
     user-select: none;
   }

   #c {
     max-width: 100%;
     max-height: 100%;
     object-fit: contain;
     display: block;
     touch-action: none;
     outline: none;
     image-rendering: high-quality;
   }
   ```
2. **`mirror.js`**:
   ```javascript
   function fitCanvas() {
     const winW = window.innerWidth;
     const winH = window.innerHeight;
     if (!winW || !winH) return;
     const naturalW = canvas.width || 1170;
     const naturalH = canvas.height || 2532;
     const base = Math.min(winW / naturalW, winH / naturalH);
     const scale = base * zoom;
     canvas.style.width = `${Math.round(naturalW * scale)}px`;
     canvas.style.height = `${Math.round(naturalH * scale)}px`;
   }

   function drawFrame(f) {
     ...
     if (targetW !== canvas.width || targetH !== canvas.height) {
       canvas.width = targetW;
       canvas.height = targetH;
     }
     fitCanvas(); // <-- selalu dipanggil setiap frame baru tiba!
     ...
   }
   ```

### E. Pembersihan Header di [`handset/web/app.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/app.js) & [`handset/web/index.html`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/index.html)

- Inisialisasi pill di `index.html`:
  ```html
  <div class="app-title-area">
    <span class="pill" id="pill-connection" hidden></span>
    <span class="pill pill-muted" id="pill-device" hidden></span>
  </div>
  ```
- Saat tidak terhubung di `app.js`:
  ```javascript
  } else if (!connected) {
    setPill(pillConnection, "", "");
  }
  ```
- Teks "Plug in iPhone" tidak lagi muncul di Top Bar.

---

## 3. Panduan Verifikasi Pengujian oleh Kelvin

1. **Jalankan Ulang**:
   ```cmd
   run.bat
   ```
2. **Verifikasi**:
   - Header atas bersih dan minimalis tanpa teks "Plug in iPhone".
   - Klik "Start mirroring" — video iPhone langsung tampil tinggi, utuh, dan proporsional dari Dynamic Island atas sampai dock bawah (bebas dari bug collapse ~180px).
   - Klik tombol **Full Size (⛶)** — layar iPhone membesar penuh mengikuti tinggi layar monitor secara tajam dan proporsional.
