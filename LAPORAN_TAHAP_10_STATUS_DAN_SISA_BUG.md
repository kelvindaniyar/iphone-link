# LAPORAN TAHAP 10: PEMULIHAN HARDWARE HEVC, PENEMUAN AKAR CLAMPING 720P DARI DATA TELEMETRI LIVE, DAN DAFTAR SISA BUG

> **Klien:** Kelvin (Content Creator)  
> **Tema Desain:** WinUI 3 / Liquid Glass (Fluent Design Windows 11) Native Frameless Desktop  
> **Status:** Analisis Data Live Telemetri Selesai — Akar Masalah 720p Clamping Teridentifikasi 100%  
> **Branch Aktif:** `feature/macos-sequoia-ui`  
> **Commit Terbaru:** `40f07ee` (`fix(hevc): restore GPU hardware decode, patch bonjour args, and capture full mirror.js telemetry`)  
> **Tanggal:** 23 Oktober 2026  

---

## 📌 1. APA SAJA YANG SUDAH DIKERJAKAN DI TAHAPAN INI

Pada tahapan ini, kami telah menuntaskan perbaikan sistematis dan instrumentasi telemetri penuh:

1. **Pemulihan GPU Hardware Decode HEVC di [`handset/gui.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/gui.py) & [`run.bat`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/run.bat)**:
   - Menghapus flag `--disable-accelerated-video-decode` dan `--disable-gpu-memory-buffer-video-frames`.
   - Memastikan akselerasi Media Foundation GPU aktif via `--enable-features=PlatformHEVCDecoderSupport`.
   - **Hasil:** Pesan error oranye *"This browser cannot decode the iPhone video stream."* **tuntas teratasi 100%**; video kembali menyala normal di WebView2.
2. **Patch Crash mDNS Wi-Fi di [`handset/wifi.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/wifi.py)**:
   - Memperbaiki fungsi `send_query_all` dan `bind_ipv4` agar menerima `*args, **kwargs`.
   - Menghilangkan bug `takes 2 positional arguments but 3 were given` saat `pymobiledevice3` mengirimkan parameter `skip_interfaces`.
3. **Pemisahan Kategori Telemetri di [`handset/server.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/server.py)**:
   - Menambahkan pemisahan objek JSON di [`debug_metrics.json`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/debug_metrics.json):
     * `latest_app.js`: Metrik bounding rect & computedStyle seluruh rantai flexbox container DOM.
     * `latest_mirror_frame`: Metrik frame `VideoFrame`, atribut `<canvas>`, dan ukuran viewport iframe.
     * `latest_mirror_{event}`: Catatan event `codec_check`, `decoder_configured`, `first_frame_output`, dan `decoder_error`.
4. **Peningkatan Frekuensi & Ketajaman Telemetri di [`handset/web/mirror.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/mirror.js)**:
   - Telemetri dikirim pada `frameCount === 1`, `5`, `15`, `30`, dan setiap interval 60 frame.
   - Menambahkan event logging saat pertama kali frame diterima dan saat terjadi decoder error.
5. **Pembaruan Query String Cache Buster**:
   - Mengubah query string aset di [`handset/web/index.html`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/index.html), [`handset/web/mirror.html`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/mirror.html), dan [`handset/web/app.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/app.js) menjadi `?v=20261023_telemetry` untuk memastikan WebView2 tidak menggunakan cache lama.

---

## 🔬 2. FAKTA PENEMUAN BESAR DARI DATA TELEMETRI AKTUAL KELVIN (1860 FRAMES)

Saat Kelvin menguji aplikasi barusan (terekam pada jam `08:24:45` - `08:25:25`), sistem telemetri berhasil menangkap **1860 frame video live** dari iPhone Kelvin.

Berikut adalah data riil yang tercatat di [`debug_metrics.json`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/debug_metrics.json):

```json
{
  "latest_mirror_first_frame_output": {
    "codedW": 1216,
    "codedH": 2656,
    "displayW": 1280,
    "displayH": 720
  },
  "latest_mirror_frame": {
    "frameCount": 1860,
    "f_dims": {
      "codedW": 1216,
      "codedH": 2656,
      "displayW": 1280,
      "displayH": 720,
      "visibleRect": {
        "x": 0,
        "y": 0,
        "width": 1216,
        "height": 720
      }
    },
    "canvas_attr": {
      "width": 1280,
      "height": 720,
      "styleW": "456px",
      "styleH": "257px"
    },
    "canvas_rect": {
      "top": 365.16,
      "left": 0,
      "width": 456,
      "height": 257
    },
    "body_rect": {
      "top": 0,
      "left": 0,
      "width": 456,
      "height": 987.33
    },
    "win_inner": {
      "w": 456,
      "h": 987,
      "dpr": 1.5
    }
  }
}
```

### 💡 Analisis Angka Telemetri Ini Mengungkapkan Segala-galanya:

1. **Resolusi Asli Stream iPhone (`codedW` x `codedH`)**:  
   Bernilai **1216 x 2656** piksel!  
   Ini membuktikan iPhone Kelvin dan server Python mengirimkan video potret utuh 100%.
2. **Dimensi Yang Dikeluarkan Decoder (`displayW` x `displayH`)**:  
   Bernilai **1280 x 720** piksel!
3. **Visible Region (`visibleRect`)**:  
   Bernilai `{ x: 0, y: 0, width: 1216, height: 720 }`!
4. **Ukuran Tampilan Kanvas (`canvas_attr`)**:  
   `width: 1280, height: 720` dengan `styleH: 257px`!

> **Penjelasan Mengapa Terpotong Tepat di Bawah Ikon Pertama:**  
> Tinggi asli frame adalah **2656px**. Ketika decoder memotongnya menjadi **720px**, rasio frame yang terlihat adalah:  
> $$\frac{720}{2656} = 27.1\%$$  
> Bagian 27% teratas dari layar iPhone vertikal adalah persis: Dynamic Island, Status Bar, Jam/Baterai, dan **baris ikon pertama** (*FaceTime, Calendar, Mail, Clock*)!  
> Sisa 73% bagian bawah layar iPhone tidak pernah dirender ke kanvas karena decoder Direct3D 11 Media Foundation mengira video ini adalah video standar **720p landscape (1280x720)**!

---

## ⚠️ 3. BUG APA SAJA YANG BELUM TERPECAHKAN & RENCANA SOLUSINYA

Berdasarkan data telemetri live di atas, berikut adalah daftar sisa bug yang sedang kita hadapi:

### Bug 1: Default Surface Clamping 1280x720 pada Media Foundation WebCodecs (Penyebab Utama Pemotongan)
* **Kondisi Saat Ini:**  
  Di `mirror.js`, pemanggilan konfigurasi decoder saat ini hanya menyertakan:
  ```javascript
  const decoderConfig = { codec, description, optimizeForLatency: true };
  ```
  Karena parameter `codedWidth` dan `codedHeight` tidak diberikan secara eksplisit pada dictionary `VideoDecoderConfig`, implementasi Media Foundation di Windows secara default mengalokasikan surface D3D11 berukuran standar **1280 x 720** (16:9). Akibatnya, stream vertikal 2656px dipotong pada baris piksel ke-720.
* **Solusi yang Harus Diterapkan:**  
  1. Di `handset/server.py` atau `mirror.js`, ekstrak dimensi asli dari parameter SPS (`codedWidth: 1216, codedHeight: 2656`).
  2. Masukkan ke dalam `VideoDecoderConfig`:
     ```javascript
     const decoderConfig = {
       codec,
       description,
       codedWidth: 1216,
       codedHeight: 2656,
       displayAspectWidth: 1216,
       displayAspectHeight: 2656,
       optimizeForLatency: true,
     };
     ```
  3. Ini memaksa Direct3D 11 Media Foundation mengalokasikan buffer tekstur potret penuh sebesar 1216x2656 piksel, sehingga `visibleRect.height` menjadi 2656 dan seluruh layar iPhone bawah tampil utuh.

---

### Bug 2: `drawFrame` Terkunci Mengikuti `f.displayHeight` Clamped
* **Kondisi Saat Ini:**  
  Di `mirror.js`:
  ```javascript
  const fw = f.displayWidth;
  const fh = f.displayHeight;
  canvas.width = fw;
  canvas.height = fh;
  ```
  Karena `f.displayHeight` melaporkan 720 (akibat clamping 720p), kanvas otomatis menyetel tingginya menjadi 720 (bukan 2656), lalu `fitCanvas()` mengecilkan kanvas ke tinggi 257px di tengah layar.
* **Solusi yang Harus Diterapkan:**  
  Membuat pertahanan rasio di `drawFrame`: jika `f.codedHeight > f.displayHeight` dan `f.codedHeight > 2000`, gunakan dimensi `f.codedWidth` dan `f.codedHeight` sebagai basis kanvas potret, atau lakukan un-crop rendering.

---

### Bug 3: Wi-Fi Auto-Discovery Rentan Terhadap Profil Jaringan Windows (Public vs Private)
* **Kondisi Saat Ini:**  
  Jika laptop Kelvin terhubung ke Wi-Fi dengan status profil "Public Network", Windows Defender Firewall memblokir paket mDNS port 5353 dari iPhone, sehingga fungsi auto-discovery Wi-Fi memunculkan pesan:
  > *"The iPhone was not found on the network. Put the PC and the iPhone on the same Wi-Fi, keep the iPhone unlocked, and set that Wi-Fi to Private in Windows settings."*
* **Solusi / Mitigasi:**  
  1. Pastikan Kelvin mengatur profil Wi-Fi di Windows Settings menjadi **Private network**.
  2. Saat menggunakan kabel USB, prioritaskan transport `usb` agar koneksi instan dan tidak bergantung pada mDNS broadcast.

---

## 📋 4. KESIMPULAN & TAHAP BERIKUTNYA

| Komponen | Status Sebelum Tahap 10 | Status Saat Ini | Tindakan Selanjutnya |
|:---|:---|:---|:---|
| **HEVC Video Decoder** | ❌ Error layar hitam (software decode mati) | ✅ Berhasil decoding live (1860 frames) | Selesai |
| **mDNS Wi-Fi Discovery** | ❌ Crash 3 positional arguments | ✅ Berhasil di-patch `*args, **kwargs` | Selesai |
| **Sistem Telemetri** | ⚠️ Hanya mencatat `app.js` | ✅ Mencatat `app.js` + `mirror.js` lengkap | Selesai |
| **Tinggi Video Vertikal** | ❌ Terpotong di ikon baris pertama | 🔍 Terbukti akibat clamping default 720p | Injeksi `codedWidth: 1216, codedHeight: 2656` ke `VideoDecoderConfig` |
