# LAPORAN TAHAP 7: PERBAIKAN VISUAL RANTAI FLEXBOX TINGGI VIDEO 100% & MODE FULL SIZE RESPONTIF

> **Klien:** Kelvin (Content Creator)  
> **Fokus Tahap:** Perbaikan Bug Visual Layar Video Gepeng (~180px), Penguncian Rantai Flexbox 100%, Responsiveness Full Size Mode, dan Lock Ukuran Jendela Default  
> **Branch Git:** `feature/macos-sequoia-ui`  
> **Commit Hash:** `3c2ff85` (`fix(ui): perbaiki rantai flexbox tinggi video 100 persen dan responsive scaling mode fullsize`)  
> **Tanggal:** 23 Oktober 2026  

---

## 📌 RINGKASAN EKSEKUTIF

Setelah Kelvin berhasil menguji Tahap 6 dengan hasil **100% SUKSES** untuk gestur trackpad 2-jari (scroll feed Instagram & swipe horizontal), ditemukan kendala visual kritis:
1. **Video iPhone Terpotong Horizontal (Gepeng ~180px)**: Iframe video mengalami *height collapse* karena kontainer flexbox induk kehilangan dimensi vertikal terdefinisi dan bentrok dengan properti `position: absolute; inset: 0`.
2. **Mode Full Size Tidak Membesar**: Ketika tombol Full Size diaktifkan, kanvas video tidak memanfaatkan tinggi monitor secara proporsional.

Pada tahap ini, seluruh rantai kontainer CSS dirombak dan diperbaiki secara menyeluruh:
- **Rantai Flexbox 100% Tinggi**: Seluruh kontainer dari `.app` hingga iframe `#mirror-frame` / `#screen-view` dipastikan memiliki `height: 100%; min-height: 0; flex: 1 1 auto; display: flex; flex-direction: column;`.
- **Iframe & Canvas Edge-to-Edge**: Iframe dan elemen canvas di dalamnya dikonfigurasi dengan `width: 100%; height: 100%; object-fit: contain; aspect-ratio: 9 / 19.5; display: block;` tanpa kuncian `clamp` atau `max-height` yang memotong layar.
- **Mode Full Size Proporsional (`.is-fullsize`)**: Menghitung tinggi bersih monitor `calc(100vh - 84px)` (dikurangi Top Bar 36px dan Bottom Nav 48px) dengan `width: auto; aspect-ratio: 9 / 19.5; margin: 0 auto;`, sehingga layar iPhone membesar secara tajam dan proporsional persis di tengah layar.
- **Penguncian Dimensi Jendela Native**: Memastikan `handset/gui.py` terkunci pada `width=380, height=840` dan dilengkapi proteksi `max(height, 840)` saat toggle drawer Settings.

---

## 1. Analisis Akar Masalah (Root Cause)

| No | Gejala Bug | Akar Masalah Teknis | Solusi yang Diterapkan |
|:---|:---|:---|:---|
| 1 | Tinggi iframe collapse menjadi ~180px | Kontainer `.workspace` dan `.screen-viewport` memiliki `flex: 1 1 auto` tanpa `height: 100%` eksplisit. Di dalam CSS Flexbox, elemen anak dengan `position: absolute; inset: 0` menyebabkan tinggi kontainer induk dianggap *indefinite* (tak terbatas), sehingga browser WebView2 memberikan tinggi fallback default iframe (~150–180px). Selanjutnya, fungsi `fitCanvas()` di `mirror.js` mengunci canvas pada bounding rect yang gepeng tersebut. | Melepaskan `position: absolute` dari iframe. Menetapkan `height: 100%; min-height: 0; flex: 1 1 auto; display: flex; flex-direction: column;` pada seluruh rantai kontainer. Iframe dan canvas diberi `aspect-ratio: 9 / 19.5; object-fit: contain; display: block;`. |
| 2 | Canvas mengunci ukuran lama saat resize | `fitCanvas()` hanya dipicu pada saat event `window.resize` dan tidak mengantisipasi perubahan ukuran bertahap dari animasi pywebview / CSS transition. | Menambahkan `ResizeObserver` pada `document.body` di dalam `mirror.js` serta pengiriman sinyal `postMessage({ type: 'resize' })` dari `app.js` saat tombol Full Size diklik. |
| 3 | Mode Full Size tidak membesar | Pada class `.app.is-fullsize`, kontainer `.display-col` memiliki `width: auto` yang menciut ke lebar konten, dan kontainer video tidak mendapatkan instruksi aspect-ratio 9:19.5 berbasis tinggi monitor. | Mengatur `.app.is-fullsize .screen-stage` dengan `height: calc(100vh - 84px); width: auto; aspect-ratio: 9 / 19.5; margin: 0 auto;` dan kontainer display diset `width: 100%; height: 100%;`. |
| 4 | Tinggi jendela terpotong saat buka-tutup drawer | Pada method `set_drawer` di `gui.py`, jika nilai `self._window.height` belum terisi penuh, tinggi jendela berpotensi reset ke nilai kecil. | Menambahkan klausul `h = max(self._window.height or 840, 840)` sehingga tinggi jendela desktop tidak pernah turun di bawah 840px. |

---

## 2. Rincian Perubahan Kode

### A. Rantai Flexbox 100% di [`handset/web/app.css`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/app.css)

Rantai kontainer dari root aplikasi hingga viewport video dihubungkan secara konsisten:

```css
/* 1. App Root */
.app {
  width: 100vw;
  height: 100vh;
  min-height: 0;
  flex: 1 1 auto;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  position: relative;
  background: #000;
}

/* 2. Main Horizontal Flex Container */
.main-body {
  display: flex;
  flex-direction: row;
  flex: 1 1 auto;
  width: 100%;
  height: calc(100vh - 36px);
  min-height: 0;
  overflow: hidden;
  position: relative;
}

.display-col {
  display: flex;
  flex-direction: column;
  flex: 1 1 auto;
  min-width: 0;
  width: 100%;
  height: 100%;
  min-height: 0;
  overflow: hidden;
  position: relative;
  background: #000;
  transition: width 0.2s cubic-bezier(0.16, 1, 0.3, 1);
}

/* 3. Murni Display Edge-to-Edge: Rantai Flexbox 100% Tinggi */
.workspace,
.stage,
#phone-shell,
.screen-stage,
#phone-screen,
.screen-viewport {
  height: 100%;
  min-height: 0;
  flex: 1 1 auto;
  display: flex;
  flex-direction: column;
  width: 100%;
  overflow: hidden;
  position: relative;
  background: #000;
  margin: 0;
  padding: 0;
  align-items: center;
  justify-content: center;
}

/* Iframe Display iPhone */
#mirror-frame,
#screen-view,
.screen-frame,
.mirror-frame {
  height: 100%;
  min-height: 0;
  flex: 1 1 auto;
  width: 100%;
  display: block;
  object-fit: contain;
  aspect-ratio: 9 / 19.5;
  border: 0;
  background: #000;
}

#mirror-frame[hidden],
#screen-view[hidden],
.screen-frame[hidden],
.mirror-frame[hidden] {
  display: none !important;
}
```

---

### B. Mode Full Size Responsif (`.is-fullsize`)

Perhitungan tinggi bersih monitor:
$$\text{Tinggi Video} = 100\text{vh} - (\text{Top Bar } 36\text{px} + \text{Bottom Nav } 48\text{px}) = 100\text{vh} - 84\text{px}$$

```css
/* Mode Full Size / Responsive Scaling */
.app.is-fullsize .main-body {
  width: 100%;
  height: calc(100vh - 36px);
  justify-content: center;
  align-items: center;
}

.app.is-fullsize .display-col {
  width: 100%;
  height: 100%;
  align-items: center;
  justify-content: center;
}

.app.is-fullsize .workspace,
.app.is-fullsize .stage {
  width: 100%;
  height: calc(100vh - 84px);
  justify-content: center;
  align-items: center;
}

.app.is-fullsize #phone-shell,
.app.is-fullsize .screen-stage {
  height: calc(100vh - 84px);
  width: auto;
  aspect-ratio: 9 / 19.5;
  margin: 0 auto;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
}

.app.is-fullsize #phone-screen,
.app.is-fullsize .screen-viewport {
  height: 100%;
  width: 100%;
  aspect-ratio: 9 / 19.5;
  margin: 0 auto;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
}

.app.is-fullsize #mirror-frame,
.app.is-fullsize #screen-view,
.app.is-fullsize .screen-frame,
.app.is-fullsize .mirror-frame {
  height: 100%;
  width: 100%;
  aspect-ratio: 9 / 19.5;
  object-fit: contain;
  margin: 0 auto;
  display: block;
}
```

---

### C. Pembaruan Canvas di [`handset/web/mirror.css`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/mirror.css) & [`handset/web/mirror.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/mirror.js)

1. **`mirror.css`**:
   ```css
   html,
   body {
     margin: 0;
     padding: 0;
     width: 100%;
     height: 100%;
     min-height: 0;
     overflow: hidden;
     background: #000;
   }

   body {
     position: relative;
     touch-action: none;
     user-select: none;
     display: flex;
     flex-direction: column;
     align-items: center;
     justify-content: center;
     height: 100%;
     min-height: 0;
     flex: 1 1 auto;
   }

   #c {
     display: block;
     width: 100%;
     height: 100%;
     min-height: 0;
     max-width: 100%;
     max-height: 100%;
     object-fit: contain;
     aspect-ratio: 9 / 19.5;
     touch-action: none;
     outline: none;
     image-rendering: high-quality;
   }
   ```

2. **`mirror.js`**:
   ```javascript
   function fitCanvas() {
     if (!canvas.width || !canvas.height) return;
     const parent = canvas.parentElement;
     if (!parent) return;
     const rect = parent.getBoundingClientRect();
     if (rect.width <= 0 || rect.height <= 0) return;
     const naturalW = canvas.width;
     const naturalH = canvas.height;
     const base = Math.min(rect.width / naturalW, rect.height / naturalH);
     const scale = base * zoom;
     canvas.style.width = `${Math.round(naturalW * scale)}px`;
     canvas.style.height = `${Math.round(naturalH * scale)}px`;
   }

   window.addEventListener("resize", fitCanvas);
   if (window.ResizeObserver && document.body) {
     new ResizeObserver(() => fitCanvas()).observe(document.body);
   }
   window.addEventListener("message", (e) => {
     if (e.data && e.data.type === "resize") {
       fitCanvas();
     }
   });
   ```

---

### D. Penguncian Dimensi Jendela di [`handset/gui.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/gui.py)

```python
    def set_drawer(self, is_open: bool):
        if self._window:
            # Side drawer: expand window to 740px or collapse back to 380px
            target_width = 740 if is_open else 380
            h = max(self._window.height or 840, 840)
            self._window.resize(target_width, h)
```
- Jendela default terkunci pada `width=380, height=840`.
- Tinggi vertikal dilindungi agar tidak pernah kurang dari `840px` saat toggle drawer dilakukan.

---

## 3. Matriks Perbandingan Visual: Sebelum vs Sesudah

| Aspek Tampilan | Sebelum Perbaikan (Tahap 6) | Sesudah Perbaikan (Tahap 7) |
|:---|:---|:---|
| **Tinggi Video Default** | Gepeng / collapse menjadi kotak pipih ~180px di tengah jendela. | **Utuh 100% vertikal** dari notch / Dynamic Island paling atas hingga Home indicator paling bawah. |
| **Rasio Aspek** | Terdistorsi / terpotong horizontal. | **Presisi 9 : 19.5**, proporsional tanpa distorsi. |
| **Mode Full Size** | Layar tidak membesar mengikuti tinggi monitor desktop. | **Membesar penuh secara proporsional** mengikuti tinggi monitor (`calc(100vh - 84px)`), terpusat di tengah monitor. |
| **Responsivitas Resize** | Canvas berisiko terkunci pada ukuran pixel lama. | **Sinkron seketika** via `ResizeObserver` & listener `postMessage` lintas iframe. |
| **Perilaku Drawer** | Tinggi jendela berpotensi menciut saat drawer dibuka/tutup. | **Terkunci stabil** pada minimum 840px tinggi. |

---

## 4. Riwayat Git Commit

```
* 3c2ff85 (HEAD -> feature/macos-sequoia-ui) fix(ui): perbaiki rantai flexbox tinggi video 100 persen dan responsive scaling mode fullsize
* f5cabac docs: add LAPORAN_TAHAP_PYWEBVIEW_DAN_GESTUR report
* 7c4871f feat(app): add pywebview frameless launcher, side-drawer settings, precision trackpad scroll, and swipe nav
* fe1cea6 docs: add LAPORAN_WINUI3_LIQUID_GLASS report
* 74f98ef feat(ui): redesign theme to WinUI 3 Fluent Liquid Glass with edge-to-edge display
* c50f64f feat(ui): revise layout to compact snug container with top bar and docked nav
* 9864528 feat(ui): redesign for macOS Sequoia floating aesthetic, popover controls, /mirror/live alias, and add run.bat launcher
* 85b49e9 (main) feat(web): add GlideX-style Quick Navigation bar and edge back swipe gesture
* fb79bfd chore: update .gitignore, fix license to GPL-3.0 and fix run guide typo
```

---

## 5. Panduan Pengujian & Verifikasi oleh Kelvin

1. **Jalankan Aplikasi**:
   ```cmd
   run.bat
   ```
2. **Uji 1: Verifikasi Layar Normal**:
   - Pastikan jendela terbuka ramping (`380 x 840`).
   - Tekan "Start mirroring".
   - **Hasil yang Diharapkan:** Video iPhone langsung tampil tinggi, utuh dari ujung atas (status bar/Dynamic Island) sampai ujung bawah (dock/Home bar). Tidak ada bagian video yang terpotong atau gepeng ~180px.
3. **Uji 2: Verifikasi Mode Full Size**:
   - Klik tombol **Full Size (⛶)** di Top Bar kanan atas.
   - **Hasil yang Diharapkan:** Jendela desktop beralih ke Fullscreen, dan tampilan layar iPhone membesar secara tajam dan proporsional persis di tengah layar monitor Kelvin (dengan Top Bar 36px dan Bottom Nav 48px tetap terlihat jelas).
   - Klik kembali tombol Full Size untuk kembali ke ukuran ramping.
4. **Uji 3: Buka & Tutup Drawer Settings**:
   - Klik tombol **⚙️ Settings**. Jendela melebar ke kanan (740px), video di sisi kiri tetap utuh 100% tanpa distorsi.
   - Tutup drawer via tombol `✕`. Jendela kembali ramping ke 380px tanpa penurunan tinggi jendela.
