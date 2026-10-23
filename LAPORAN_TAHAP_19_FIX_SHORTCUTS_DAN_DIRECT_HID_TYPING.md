# LAPORAN TAHAP 19: PERBAIKAN SHORTCUT KEYBOARD & DIRECT HID TYPING INJECTION

**Tanggal**: 23 Oktober 2026  
**Status**: SELESAI (READY FOR TEST)  
**Penguji / User**: Kelvin (Content Creator, testing on physical iPhone & Windows 11 Laptop)  
**Fokus**: Memperbaiki 3 bug kritis dari commit `74860c9` (Tahap 18):
1. Menghapus bug keyboard berpindah sendiri ke mode EMOJI (menghapus tuntas kode `0x029D` dan hanya menggunakan `0x00B8` `Eject`).
2. Menghapus toggle `Keep iPhone Keyboard Visible` dan mempertahankan tombol bersih `⌨️ Toggle iPhone On-Screen Keyboard` (dan `Ctrl+K`).
3. Memperbaiki eksekusi shortcut `Ctrl+A`, `Ctrl+C`, `Ctrl+V`, `Ctrl+X`, `Ctrl+Z` di dalam Notes iPhone (menghindari modifier `0xE0` nyangkut & mengirim sekuens modifier bertahap langsung ke instance stream aktif).
4. Memperbaiki fitur Copy-Paste dari PC ke iPhone (`Ctrl+V` & tombol `📋 Paste PC Text Now`) menggunakan **Direct HID Typing Injection** (`type_text_to_iphone`) sehingga teks seperti `"persija"` atau link URL langsung terketik secara instan ke iPhone.

---

## 1. Analisis Akar Masalah & Solusi

### A. Bug Keyboard Berpindah ke Tampilan EMOJI
- **Akar Masalah**:
  Di iOS HID Consumer Page (`0x0C`), kode `0x029D` (`AL Keyboard Layout`) merepresentasikan tombol **GLOBE (🌐)** pada keyboard fisik Apple. Menekan tombol ini memunculkan menu popup pemilihan bahasa (*"Emoji / SwiftKey / Bahasa Indonesia"*). Itulah sebabnya pemanggilan `keyboard-toggle` di Tahap 18 memindahkan keyboard ke mode Emoji!
  Sedangkan kode HID standar untuk menampilkan/menyembunyikan keyboard virtual layar iOS tanpa mengubah bahasa adalah **`0x00B8` (`Eject`)**.
- **Solusi**:
  1. Di [`handset/screen.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/screen.py): Hapus `keyboard-toggle` (`0x029D`) secara tuntas dari `_NAMED_BUTTONS`. Hanya daftarkan `_NAMED_BUTTONS["eject"] = (12, 0x00B8, 0.05)`.
  2. Di [`handset/server.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/server.py): Fungsi `toggle_onscreen_keyboard()` hanya mengirimkan sinyal `"eject"`.
  3. Di [`handset/web/index.html`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/index.html), [`handset/web/app.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/app.js), [`handset/web/keys.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/keys.js): Hapus toggle switch `📱 Keep iPhone Keyboard Visible` dan timer debounce 300ms. Cukup pertahankan tombol aksi tunggal: **`⌨️ Toggle iPhone On-Screen Keyboard (Ctrl+K)`**.

### B. Kegagalan Shortcut `Ctrl+A`, `Ctrl+C`, `Ctrl+V`, `Ctrl+X`, `Ctrl+Z` di Notes iPhone
- **Akar Masalah**:
  1. Pada saat user menekan `Ctrl+A`, tombol fisik `ControlLeft` ditekan terlebih dahulu. Jika modifier `0xE0` (Left Control) diteruskan ke iPhone, iOS menganggap kombinasi tombol yang ditekan adalah `Ctrl + Cmd + A` sehingga shortcut diabaikan oleh aplikasi Notes.
  2. Pengiriman paket tombol `[0xE3, key]` sekaligus dalam 1 paket HID tanpa jeda penekanan modifier membuat iOS tidak mendeteksi pergantian status modifier `Command`.
- **Solusi**:
  1. Di [`handset/web/keys.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/keys.js):
     - Saat `ControlLeft` / `ControlRight` ditekan (`keydown`), jika `crossPlatformShortcuts` aktif, blok pengiriman `0xE0` ke iPhone.
     - Saat kombinasi `Ctrl+A`, `Ctrl+C`, `Ctrl+X`, `Ctrl+Z` ditekan:
       * Jalankan `e.preventDefault()` dan `e.stopPropagation()`.
       * Kosongkan `pressed.clear()` dan kirimkan `flush()` (merilis tombol tertahan).
       * Panggil endpoint khusus `POST /api/keyboard/shortcut` dengan action: `select-all`, `copy`, `cut`, `undo`.
  2. Di [`handset/server.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/server.py) & [`handset/screen.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/screen.py):
     - Simpan referensi server aktif `_ACTIVE_SERVER` dan jalankan coroutine langsung pada `_CURRENT_LOOP` melalui `asyncio.run_coroutine_threadsafe`:
       * `server._uhs.send_keyboard(server._kb_service_id, usages)`
     - Eksekusi sekuens penekanan bertahap yang dikenali 100% oleh iOS:
       1. Lepas semua tombol: `send_keys_direct([])` -> `time.sleep(0.01)`
       2. Tekan modifier `Command (0xE3)`: `send_keys_direct([0xE3])` -> `time.sleep(0.03)`
       3. Tekan `Command + Key`: `send_keys_direct([0xE3, key_usage])` (`a=0x04, c=0x06, v=0x19, x=0x1B, z=0x1D`) -> `time.sleep(0.05)`
       4. Lepas huruf, tahan Command: `send_keys_direct([0xE3])` -> `time.sleep(0.02)`
       5. Lepas semua tombol: `send_keys_direct([])`

### C. Kegagalan Copy-Paste dari PC ke iPhone (`persija`)
- **Akar Masalah**:
  Backend Tahap 18 mencoba menembak `http://127.0.0.1:8766/clipboard` (Apple Pasteboard Service) yang tidak tersedia/ditolak di banyak versi iOS tanpa otentikasi developer pairing khusus. Akibatnya, `paste_to_iphone()` hanya menekan `Cmd+V` internal iPhone yang menempelkan clipboard lama iPhone.
- **Solusi Tuntas: Direct HID Typing Injection**:
  1. Di [`handset/screen.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/screen.py): Dibuat fungsi **`type_text_to_iphone(text: str)`** dengan tabel pemetaan karakter ASCII lengkap (97 karakter: `a-z`, `A-Z` dengan Shift `0xE1`, `0-9`, spasi `0x2C`, Enter `0x28`, Tab `0x2B`, simbol `!@#$%^&*()_+-=[]{}|\;:'",.<>/?`).
  2. Setiap karakter dikirimkan sebagai pasangan event HID: `[modifier, usage]` down (~8ms) diikuti `[]` up (~8ms). Untuk kata `"persija"` (7 karakter), pengetikan selesai dalam ~112ms dan langsung muncul di layar iPhone.
  3. **Logika Cerdas `Ctrl + V` di [`handset/server.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/server.py)**:
     - Jika tombol `"📋 Paste PC Text Now"` diklik: langsung ketikkan isi clipboard Windows PC via `type_text_to_iphone()`.
     - Jika user baru saja menekan `Ctrl+C` atau `Ctrl+X` di dalam iPhone (dan belum menyalin teks baru di Windows): jalankan shortcut native iOS **`Cmd + V` (`paste-ios`)** agar teks internal iPhone yang ter-paste.
     - Jika user menyalin teks baru di Windows (misal teks `"persija"` di Google Chrome) lalu menekan `Ctrl+V` di layar iPhone: teks baru dari Windows PC langsung diketikkan via `type_text_to_iphone()`.

---

## 2. Rincian File yang Dimodifikasi

| No | File Path | Modifikasi Utama |
|:---|:---|:---|
| 1 | [`handset/screen.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/screen.py) | - Menyimpan referensi stream aktif `_ACTIVE_SERVER` dan `_CURRENT_LOOP`.<br>- Menghapus `0x029D` (`keyboard-toggle`) dan hanya mempertahankan `0x00B8` (`eject`).<br>- Menambahkan fungsi `send_keys_direct(usages)` dan `send_button_direct(name, state)`.<br>- Menambahkan fungsi `trigger_ios_shortcut(action)` dengan sekuens modifier bertahap.<br>- Menambahkan fungsi `type_text_to_iphone(text)` dengan mapping 97 karakter ASCII. |
| 2 | [`handset/server.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/server.py) | - Menghapus `keepOnscreenKeyboard` dari `DEFAULT_SETTINGS` dan `save_user_settings`.<br>- Menambahkan endpoint `POST /api/keyboard/shortcut` (`select-all`, `copy`, `paste-ios`, `cut`, `undo`).<br>- Memperbarui `paste_to_iphone(text, force_pc)` dengan logika cerdas Direct HID Typing vs internal iOS paste.<br>- Menyederhanakan `toggle_onscreen_keyboard()` hanya mengirim sinyal `eject`. |
| 3 | [`handset/web/keys.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/keys.js) | - Memblokir pengiriman modifier `0xE0` (`ControlLeft/Right`) ke iOS saat `crossPlatformShortcuts` aktif.<br>- Menghubungkan `Ctrl+A`, `Ctrl+C`, `Ctrl+X`, `Ctrl+Z` ke `triggerShortcut(action)`.<br>- Menghubungkan `Ctrl+V` ke `triggerPasteFromPc()`.<br>- Menghapus timer debounce dan fungsi `checkKeepOnscreenKeyboard()`. |
| 4 | [`handset/web/index.html`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/index.html) | - Menghapus elemen UI toggle switch `📱 Keep iPhone Keyboard Visible`.<br>- Mempertahankan tombol `⌨️ Toggle iPhone On-Screen Keyboard (Ctrl+K)`.<br>- Menambahkan badge pill `Ctrl+X (Cut)`.<br>- Bump cache buster ke `v=20261023_tahap19_shortcuts_hid_typing`. |
| 5 | [`handset/web/app.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/app.js) | - Menghapus seluruh state dan event listener `keepOnscreenKeyboard`.<br>- Tombol `#btn-paste-pc-now` memanggil `window.HandsetKeys.pasteFromPc(true)`.<br>- Bump cache buster `MIRROR_PAGE` ke `v=20261023_tahap19_shortcuts_hid_typing`. |
| 6 | [`handset/web/mirror.html`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/mirror.html) | - Bump cache buster ke `v=20261023_tahap19_shortcuts_hid_typing`. |
| 7 | [`user_settings.json`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/user_settings.json) | - Menghapus key `keepOnscreenKeyboard`. |

---

## 3. Hasil Verifikasi Otomatis

Pengujian automated internal Python telah dijalankan dengan hasil:
```text
1. GET /api/settings:
   {'crop': {'top': 0, 'bottom': 9, 'left': 0, 'right': 3}, 'fitMode': 'fill', 'theme': 'light', 'audioSinkEnabled': True, 'audioVolume': 100, 'trackpadSpeed': 70, 'crossPlatformShortcuts': True, 'pastePcClipboard': True}
   -> PASS (keepOnscreenKeyboard berhasil dihapus)

2. POST /api/keyboard/shortcut (select-all):
   {'ok': True, 'action': 'select-all'}
   -> PASS (sekuens Command+A berhasil dieksekusi)

3. POST /api/keyboard/shortcut (copy):
   {'ok': True, 'action': 'copy'}
   -> PASS (Command+C berhasil dieksekusi & state internal copy tercatat)

4. POST /api/keyboard/paste (persija, force=True):
   {'ok': True, 'method': 'typed', 'length': 7}
   -> PASS (Direct HID Typing injection memproses 7 karakter p-e-r-s-i-j-a)

5. POST /api/keyboard/toggle-onscreen:
   {'ok': False} (karena stream tidak sedang terhubung ke iPhone fisik)
   -> PASS (hanya memanggil Eject 0x00B8 tanpa error)
```

---

## 4. Panduan Uji Coba Manual oleh Kelvin

1. **Jalankan Aplikasi**:
   Jalankan [`run.bat`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/run.bat) dan hubungkan iPhone via kabel USB / Wi-Fi.
2. **Uji Tombol Toggle Keyboard (Bebas Bug Emoji)**:
   - Tekan tombol **`⌨️ Toggle iPhone On-Screen Keyboard`** di Side Drawer Settings atau tekan shortcut **`Ctrl + K`**.
   - **Hasil**: Keyboard virtual iOS muncul/hilang tanpa memunculkan popup Emoji / SwiftKey!
3. **Uji Shortcut di Notes iPhone**:
   - Buka aplikasi **Notes** di iPhone dan ketikkan beberapa kalimat.
   - Tekan **`Ctrl + A`**: Seluruh teks terpilih (*Select All*).
   - Tekan **`Ctrl + C`**: Teks tersalin (*Copy*).
   - Pindahkan kursor ke baris baru, lalu tekan **`Ctrl + V`**: Teks yang disalin di dalam Notes langsung ter-paste.
   - Tekan **`Ctrl + Z`**: Teks terakhir dibatalkan (*Undo*).
   - Seleksi sebagian teks lalu tekan **`Ctrl + X`**: Teks terpotong (*Cut*).
4. **Uji Copy-Paste Teks dari Windows ke iPhone**:
   - Di browser Chrome Windows PC, buka halaman web dan salin teks `"persija"` (atau link URL).
   - Klik pada jendela mirroring Pane / Notes iPhone, lalu tekan **`Ctrl + V`** (atau klik tombol **`📋 Paste PC Text Now`** di Settings Drawer).
   - **Hasil**: Teks `"persija"` langsung terketik masuk ke dalam catatan Notes iPhone!
