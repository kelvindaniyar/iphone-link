# LAPORAN TAHAP 12: ELIMINASI BLACK BAR 16PX, FITUR DISCONNECT MIRRORING, OPSI WHITE MODE, DAN TRUE LIQUID GLASS WINDOWS 11 DWM

> **Fokus Tahap:**  
> 1. Eliminasi Total Black Bar 16px di Kiri-Kanan Video (Window `348px x 840px`, Drawer `708px`, Container `flex: 0 0 348px`, `background: transparent !important`).  
> 2. Fitur Toggle Disconnect / Stop Mirroring (Endpoint `/api/screen/stop` & `/mirror/stop`, tombol `⏹ Stop` di Top Bar, teks `"⏹ Disconnect Mirroring"` merah glass di Drawer).  
> 3. White Mode (Light Mode) & Theme Toggle (`☀️ / 🌙` di Top Bar, Radio Switcher `Dark Mode | White Mode` di Drawer, variabel CSS Liquid White Glass).  
> 4. True WinUI 3 Liquid Glass Transparency (`WEBVIEW2_DEFAULT_BACKGROUND_COLOR=0`, `transparent=True`, DWM Acrylic blur + rounded corners via Windows 11 `dwmapi`, backdrop blur 32px-36px).  
> **Commit Hash:** `a872b1d` (`feat(ui): remove 16px black bars, add disconnect button, white mode theme, and true liquid glass transparency`)  
> **Integritas Sistem:** Logika decoding HEVC, `parseHevcSpsDimensions`, `drawFrame()`, handler `wheel` trackpad di `mirror.js`, dan struktur `calc(100vh - 80px)` **100% UTUH DAN TIDAK DIUBAH**.

---

## 1. Eliminasi Black Bar 16px di Kiri-Kanan Video

### Analisis Matematis
- Tinggi area video vertikal: $840\text{px} - 80\text{px (bar)} = 760\text{px}$.
- Rasio layar stream iPhone fisik: $1216 : 2656 = 1 : 2.1842$.
- Lebar proporsional canvas di dalam area $760\text{px}$:
  $$760 \times \frac{1216}{2656} = 348.16\text{px} \approx 348\text{px}$$
- Ketika window sebelumnya berukuran $380\text{px}$, timbul sisa $(380 - 348) / 2 = 16\text{px}$ di sisi kiri dan kanan (menghasilkan black bar 16px).

### Solusi Kode
1. **[`handset/gui.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/gui.py)**:
   - Lebar default window diubah menjadi `width=348, height=840` (`min_size=(348, 600)`).
   - Saat side drawer dibuka (`set_drawer(True)`), ukuran jendela membesar pas menjadi `target_width = 708` ($348\text{px video} + 360\text{px drawer}$).
2. **[`handset/web/app.css`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/app.css)**:
   - Saat side drawer terbuka, `.display-col` dikunci mutlak pada `348px` (`flex: 0 0 348px; width: 348px !important; max-width: 348px !important;`).
   - Seluruh background hitam statis dihapus dan diubah menjadi transparan:
     `.app`, `.display-col`, `.workspace`, `.stage`, `#phone-shell`, `#phone-screen`, `iframe`, `#screen-view`, `#mirror-frame` menggunakan `background: transparent !important;`.
3. **[`handset/web/mirror.css`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/mirror.css)**:
   - `html, body` menggunakan `background: transparent !important;`.

Hasil: Kanvas video iPhone kini menempel presisi edge-to-edge tanpa sisa hitam di kiri maupun kanan jendela.

---

## 2. Fitur Toggle Disconnect / Stop Mirroring

### Backend (`handset/screen.py` & `handset/server.py`)
- Dibuat fungsi `stop_screen() -> dict` di `screen.py` yang menyimpan referensi `_CURRENT_LOOP` dan `_CURRENT_TASK`.
- Saat dipanggil, loop asyncio membatalkan task server (`_CURRENT_TASK.cancel()`), yang seketika memicu pembersihan di blok `finally` `ScreenStreamServer.serve()`:
  * Menghentikan server HTTP internal (port 8766).
  * Menutup thread HID worker.
  * Memanggil `DisplayService.stop_all_streams(self._rsd)` untuk mematikan sesi display pada perangkat iPhone fisik (membebaskan resource dan mematikan indikator remote sharing di iOS).
  * Mengubah status internal kembali ke `phase="idle"`.
- Diexpose melalui endpoint `POST /api/screen/stop` dan `POST /mirror/stop`.

### Frontend (`index.html` & `app.js`)
- **Top Bar**: Tombol `⏹ Stop` (`#stop-screen`) dengan gaya glass merah (`bar-btn-danger`) yang otomatis muncul saat status `phase === "running"` dan tersembunyi saat idle.
- **Side Drawer**: Saat mirroring aktif, tombol `#show-screen` berubah menjadi `"⏹ Disconnect Mirroring"` dengan aksen merah kristal (`rgba(239, 68, 68, 0.28)` dan border `rgba(248, 113, 113, 0.45)`).
- Mengklik tombol Disconnect akan memanggil `/api/screen/stop`, membersihkan timer, mereset `screenView.src = "about:blank"`, menyembunyikan iframe, dan mengembalikan tampilan ke placeholder `"Ready to mirror"`.

---

## 3. White Mode (Light Mode) & Theme Switcher

### Mekanisme Pengalihan Tema
- Preferensi disimpan di `localStorage` (`pane-theme = 'dark' | 'light'`).
- Atribut `data-theme="light"` / `data-theme="dark"` diterapkan pada `<html>` dan container `#app-root`.
- Backend native dihubungkan via `window.pywebview.api.set_theme(currentTheme)` untuk mengubah atribut DWM Windows 11 (`DWMWA_USE_IMMERSIVE_DARK_MODE`).
- Dua kontrol intuitif:
  1. Tombol ikonik matahari/bulan (`#btn-theme`) di Top Bar sebelah tombol ⚙️.
  2. Radio selector `[ 🌙 Dark | ☀️ Light ]` (`.panel-theme`) di bagian atas panel Settings & Controls.

### Desain Visual White Mode (`app.css`)
- **Glass Acrylic Putih Kristal**:
  * `--bg-acrylic: rgba(255, 255, 255, 0.58)`
  * `--bg-flyout: rgba(255, 255, 255, 0.75)`
  * Specular top highlight: `inset 0 1px 0 rgba(255, 255, 255, 0.90)`
- **Tipografi Kontras Tinggi**:
  * Teks utama: `#0f172a` (Slate-900)
  * Teks sekunder: `#334155` (Slate-700)
  * Muted: `#64748b` (Slate-500)
- **Komponen**:
  * Kartu pengaturan: `background: rgba(255, 255, 255, 0.55)` dengan border putih susu dan shadow `0 4px 16px rgba(0, 0, 0, 0.05)`.
  * Tombol navigasi bawah (`.quick-nav .nav-btn`): kaca putih semi-transparan yang bersih dan elegan.
  * Placeholder `"Ready to mirror"`: latar putih transparan dengan aksen radial biru lembut.

---

## 4. True WinUI 3 Liquid Glass Transparency (DWM Windows 11)

1. **Environment Variables**:
   - `os.environ["WEBVIEW2_DEFAULT_BACKGROUND_COLOR"] = "0"` diatur di [`handset/gui.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/gui.py) dan [`run.bat`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/run.bat) untuk transparansi WebView2 ARGB penuh.
2. **Pywebview Window Configuration**:
   - `transparent=True`, `vibrancy=True`, `background_color="#00000000"`.
3. **Native Windows 11 DWM Blur Integration**:
   - Diterapkan melalui `ctypes.windll.dwmapi`:
     * `DWMWA_WINDOW_CORNER_PREFERENCE = 33` (Nilai 2: sudut membulat native Windows 11).
     * `DWMWA_SYSTEMBACKDROP_TYPE = 38` (Nilai 3: Efek Acrylic blur langsung dari Desktop Window Manager Windows 11).
4. **CSS Backdrop Filter**:
   - `.app-bar`, `.quick-nav`, `.side-drawer`, dan `#screen-empty` ditingkatkan ke `backdrop-filter: blur(32px) saturate(190%)` dengan alpha `0.42 - 0.58`, memberikan nuansa kaca cair mewah (*true Liquid Glass*).

---

## 5. Ringkasan File yang Dimodifikasi

| No | File Path | Perubahan Kunci |
|:---|:---|:---|
| 1 | [`handset/gui.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/gui.py) | Lebar default 348px, drawer 708px, `transparent=True`, `vibrancy=True`, integrasi DWM Acrylic + rounded corners ctypes. |
| 2 | [`run.bat`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/run.bat) | Penambahan `set WEBVIEW2_DEFAULT_BACKGROUND_COLOR=0`. |
| 3 | [`handset/screen.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/screen.py) | Implementasi `stop_screen()`, pembatalan task asyncio bersih, pelepasan sesi display iOS. |
| 4 | [`handset/server.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/server.py) | Penambahan rute GET/POST `/api/screen/stop` dan `/mirror/stop`. |
| 5 | [`handset/web/mirror.css`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/mirror.css) | `html, body` menjadi `background: transparent !important`. |
| 6 | [`handset/web/app.css`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/app.css) | Background transparan, kunci 348px display-col, tema White Mode, tombol disconnect merah, Liquid Glass 32px blur. |
| 7 | [`handset/web/app.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/app.js) | Logika toggle tema, sinkronisasi DWM, penanganan stop/disconnect mirror, update state tombol. |
| 8 | [`handset/web/index.html`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/index.html) | Tombol Stop di Top Bar, tombol toggle tema di Top Bar, selector tema di Drawer. |
| 9 | [`handset/web/mirror.html`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/mirror.html) | Cache buster `v=20261023_theme_stop_fix`. |
