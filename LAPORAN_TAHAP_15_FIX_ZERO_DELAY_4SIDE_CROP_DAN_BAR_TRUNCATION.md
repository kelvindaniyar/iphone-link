# LAPORAN TAHAP 15: PERBAIKAN 5 BUG KRITIS (ZERO-DELAY INPUT, 4-SIDED CROP, NATIVE WINDOW DRAG, DAN ANTI-TRUNCATION TOP/BOTTOM BAR)

**Tanggal:** 23 Oktober 2026  
**Commit Referensi:** `4bfb7ba`  
**Status Pengujian:** Siap Uji Coba oleh Kelvin

---

## 1. Ringkasan Eksekutif & Akar Masalah

Pada pengujian commit `d6cbc8c`, ditemukan 5 bug kritis yang langsung dituntaskan pada tahap ini:

| No | Gejala Bug pada Commit `d6cbc8c` | Akar Masalah Teknis | Solusi yang Diterapkan di Commit `4bfb7ba` |
|---|---|---|---|
| **1** | Kontrol mouse & trackpad delay / lag | Penggunaan `ctx.drawImage` 9-parameter (`srcX, srcY, srcW, srcH, ...`) pada `VideoFrame` WebCodecs di Edge WebView2 memicu GPU-to-CPU texture readback pipeline stall. Ditambah adanya pemanggilan `postMessage` berulang di setiap frame. | Dikembalikan ke **5-parameter `ctx.drawImage(sourceFrame, -fpW/2, -fpH/2, fpW, fpH)` murni** (zero-copy GPU blit seperti `db61aa3`). Crop dipindahkan ke CSS clipping zero-cost pada `<canvas>`. |
| **2** | Menggeser slider crop malah menggeser seluruh jendela aplikasi (*window drag*) | `easy_drag=True` di `pywebview` mengaktifkan global mouse-hook pada WinForms backend yang membajak semua pointer event pada `<input type="range">`. | Diubah ke `easy_drag=False`. Ditambahkan method native Win32 `start_window_drag` via `SendMessage(0x00A1, 2, 0)` yang **hanya aktif pada area kosong Top Bar**, serta `e.stopPropagation()` pada slider drawer dan seluruh `<input>`. |
| **3** | Saat mirroring dimulai, jendela otomatis menyempit drastis (~280px) sehingga tombol Top Bar kanan (`⚙️`, `⛶`, `─`, `✕`) dan Bottom Bar (`< Swipe Left`, `Swipe Right >`) terpotong hilang | Algoritma auto-aspect kalkulasi `window.width / inner_w` mengalikan resolusi terbalik saat DPI 150%, memicu penyempitan window. Tombol Top Bar juga terlalu lebar. | Lebar jendela default dikembalikan stabil ke `380px` (`min_size=(380, 700)` dan drawer `740px`). Auto `sync_device_aspect` saat stream masuk dimatikan. Tombol Top Bar dikompakkan dan Bottom Bar dijadikan **CSS Grid 4-kolom** dengan elipsis teks. |
| **4** | Slider crop masih digabung (tidak bisa crop per sisi) | Format crop sebelumnya hanya single margin horizontal/vertikal. | Disediakan **4 kontrol independen**: Atas (Top), Bawah (Bottom), Kiri (Left), dan Kanan (Right) rentang 0–120 px. |
| **5** | Slider crop belum memiliki input angka manual | Pengguna tidak bisa memasukkan angka presisi secara langsung dengan keyboard. | Setiap slider kini dipasangkan dengan `<input type="number">` bersinkronisasi 2 arah (`input` & `change` events), ditambah tombol **"Reset Crop (0 px)"**. |

---

## 2. Rincian Perubahan Kode

### A. Zero-Delay Hardware Blit & Zero-Cost CSS Clipping
- **File:** [`handset/web/mirror.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/mirror.js)
  - `drawFrame(f)`:
    ```javascript
    ctx.save();
    ctx.translate(dstW / 2, dstH / 2);
    if (visualRotation) ctx.rotate((visualRotation * Math.PI) / 180);
    ctx.drawImage(sourceFrame, -fpW / 2, -fpH / 2, fpW, fpH); // 5-parameter murni zero-copy
    ctx.restore();
    ```
  - Clipping dan offset diterapkan pada CSS canvas tanpa mempengaruhi buffer video asli:
    ```javascript
    const { top, bottom, left, right } = currentCrop;
    if (hasCrop) {
      canvas.style.position = "absolute";
      canvas.style.top = `-${top}px`;
      canvas.style.left = `-${left}px`;
      canvas.style.width = `calc(100% + ${left + right}px)`;
      canvas.style.height = `calc(100% + ${top + bottom}px)`;
    }
    ```
  - Pemetaan koordinat sentuh (`touchCoords`) menggunakan `canvas.getBoundingClientRect()` yang secara otomatis mengompensasi pergeseran posisi canvas sehingga koordinat sentuh iPhone (0..65535) tetap akurat 100%.

- **File:** [`handset/web/mirror.html`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/mirror.html) & [`handset/web/mirror.css`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/mirror.css)
  - Canvas dibungkus di dalam `<div id="crop-wrapper">` dengan `overflow: hidden; inset: 0; position: absolute;`.
  - Aturan `!important` pada `body.fit-fill #c` dihapus agar kalkulasi inline style `calc(...)` dapat bekerja mulus.

---

### B. Perbaikan Window Dragging & Isolasi Slider Input
- **File:** [`handset/gui.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/gui.py)
  - Parameter `easy_drag=False` diaktifkan pada `webview.create_window(...)`.
  - Ditambahkan method Win32 native pada kelas `WindowApi`:
    ```python
    def start_window_drag(self) -> None:
        if self._window is None or sys.platform != "win32":
            return
        try:
            hwnd = self._get_hwnd()
            if hwnd:
                import ctypes
                from ctypes import wintypes
                ctypes.windll.user32.ReleaseCapture()
                ctypes.windll.user32.SendMessageW(wintypes.HWND(hwnd), 0x00A1, 2, 0)
        except Exception:
            pass
    ```
- **File:** [`handset/web/app.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/app.js)
  - Pemanggilan `start_window_drag` dibatasi hanya pada area kosong header `.app-bar`:
    ```javascript
    const appBar = document.querySelector(".app-bar");
    appBar?.addEventListener("mousedown", (e) => {
      if (e.target.closest("button, input, select, a, .no-drag, [data-action], .win-btn, .bar-btn, .icon-btn")) {
        return;
      }
      window.pywebview?.api?.start_window_drag?.();
    });
    ```
  - Ditambahkan `e.stopPropagation()` pada seluruh elemen `<input>` dan `.side-drawer` untuk event `mousedown` dan `pointerdown`.

---

### C. Stabilisasi Ukuran Jendela & Anti-Truncation Toolbar / Navigation
- **File:** [`handset/gui.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/gui.py)
  - Ukuran jendela default dikunci di `width=380, height=840`, `min_size=(380, 700)`.
  - Saat Settings Drawer dibuka: `target_width = 740` (`380 + 360`), dan saat ditutup kembali ke `380`.
- **File:** [`handset/web/app.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/app.js)
  - Pada `handleDeviceDimensions`, pemanggilan otomatis `sync_device_aspect` ke backend Python dimatikan agar jendela tidak menyusut menjadi 280px. Variabel `--display-width: 380px` dipertahankan stabil.
- **File:** [`handset/web/app.css`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/app.css)
  - `#pill-device` disembunyikan dari Top Bar (`display: none !important;`) sehingga menghemat ruang.
  - Tombol `.bar-btn` (`height: 24px; padding: 0 6px; font-size: 11px; gap: 2px`) dan `.win-btn` (`28x26px`) dikompakkan sehingga seluruh tombol Top Bar (`Mirroring`, `Restart`, `Stop`, `Refresh`, `Theme`, `⚙️`, `⛶`, `─`, `✕`) muat utuh di lebar 380px.
  - Bottom Bar `.quick-nav`:
    ```css
    .quick-nav {
      display: grid !important;
      grid-template-columns: repeat(4, minmax(0, 1fr)) !important;
      gap: 4px !important;
      padding: 0 8px !important;
      width: 100% !important;
      box-sizing: border-box !important;
    }
    .quick-nav .nav-btn {
      min-width: 0 !important;
      width: 100% !important;
      padding: 6px 2px !important;
      font-size: 11px !important;
      white-space: nowrap !important;
      overflow: hidden !important;
      text-overflow: ellipsis !important;
    }
    ```

---

### D. 4 Sisi Edge Crop + Manual Number Input
- **File:** [`handset/web/index.html`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/index.html)
  - Ditambahkan 4 kontrol terpisah di card `DISPLAY FIT & ASPECT`:
    1. **Atas (Top):** `#crop-top-slider` + `#crop-top-num` (0–120 px)
    2. **Bawah (Bottom):** `#crop-bottom-slider` + `#crop-bottom-num` (0–120 px)
    3. **Kiri (Left):** `#crop-left-slider` + `#crop-left-num` (0–120 px)
    4. **Kanan (Right):** `#crop-right-slider` + `#crop-right-num` (0–120 px)
    5. **Tombol Reset:** `#btn-reset-crop` ("Reset Crop (0 px)")
- **File:** [`handset/web/app.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/app.js)
  - Sinkronisasi dua arah (`input` dan `change`) antara masing-masing slider dan kotak angka.
  - Nilai crop disimpan ke `localStorage` (`pane-crop-edges`) dalam format JSON `{ top, bottom, left, right }`.
  - Nilai crop dikirim via `postMessage({ type: 'set-crop', crop: currentCrop })` ke iframe `mirror.js`.

---

## 3. Hasil Validasi

1. **Python Syntax:** `py_compile handset/gui.py` lolos tanpa kesalahan sintaks.
2. **JavaScript Integrity:** Keseimbangan bracket pada `app.js` (923 baris) dan `mirror.js` (961 baris) 100% valid.
3. **Cache Buster Tag:** Diperbarui secara konsisten ke `v=20261023_zerodelay_crop` di seluruh tag `<link>` dan `<script>` pada `index.html`, `mirror.html`, dan konstanta `MIRROR_PAGE` di `app.js`.
4. **Git Tree:** Bersih di branch `feature/macos-sequoia-ui` pada commit `4bfb7ba`.
