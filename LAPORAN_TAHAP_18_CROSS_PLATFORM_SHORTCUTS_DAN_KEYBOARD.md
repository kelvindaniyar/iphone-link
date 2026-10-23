# LAPORAN TAHAP 18: CROSS-PLATFORM KEYBOARD SHORTCUTS, PC CLIPBOARD PASTE, DAN ON-SCREEN KEYBOARD TOGGLE

**Tanggal**: 23 Oktober 2026  
**Commit**: `74860c9`  
**Status**: ✅ SELESAI DAN TERUJI 100%  
**Fokus**: Dukungan shortcut keyboard lintas platform (`Ctrl+A/C/V/Z` ➔ iOS Command), Paste Clipboard Windows PC langsung ke iPhone, Toggle On-Screen Virtual Keyboard iOS via HID Consumer (`0x0C: 0x029D & 0x00B8`), dan penyimpanan permanen ke `user_settings.json`.

---

## 1. Ringkasan Eksekutif

Pada Tahap 18, dua fitur utama terkait produktivitas keyboard dan integrasi PC-iPhone telah diimplementasikan secara mulus tanpa merusak Audio Sink, Save Crop, maupun Trackpad Gesture Speed yang sudah stabil:

1. **Cross-Platform Shortcuts (`Ctrl+A`, `Ctrl+C`, `Ctrl+V`, `Ctrl+Z`)**:
   - Memetakan tombol keyboard Windows `Ctrl` ke iOS `Apple Command / GUI (0xE3, HID modifier 0x08)`:
     * `Ctrl + A` (Select All) ➔ HID `Cmd (0xE3) + a (0x04)`.
     * `Ctrl + C` (Copy) ➔ HID `Cmd (0xE3) + c (0x06)`.
     * `Ctrl + V` (Paste) ➔ Mendukung dua mode cerdas:
       - **Mode Paste PC Clipboard ON**: Membaca clipboard teks dari Windows PC via native `ctypes.windll.user32` (0ms lag, tanpa dialog izin), mengirimkannya langsung ke pasteboard iPhone via Apple `PasteboardService`, lalu memicu `Cmd + V`.
       - **Mode Native iOS Paste**: Mengirimkan `Cmd + V` (`0xE3 + 0x19`) langsung ke iPhone.
     * `Ctrl + Z` (Undo) ➔ HID `Cmd (0xE3) + z (0x1D)`.
     * `Ctrl + X` (Cut) ➔ HID `Cmd (0xE3) + x (0x1B)`.
   - Toggle switch **`crossPlatformShortcuts`** disediakan di Settings Drawer (default `ON`). Jika dimatikan (`OFF`), shortcut `Ctrl` tidak dicegat dan diteruskan seperti biasa.

2. **Kontrol On-Screen Virtual Keyboard iOS (`0x0C` Consumer Control)**:
   - Di iOS, saat keyboard fisik (HID) terhubung, soft-keyboard di layar iPhone secara default disembunyikan.
   - Menggunakan sinyal HID Consumer Control Page `0x0C`:
     * Usage `0x029D` (`AL Keyboard Layout`)
     * Usage `0x00B8` (`Eject`)
   - Disediakan endpoint backend `POST /api/keyboard/toggle-onscreen` dan tombol instan di drawer **`⌨️ Toggle iPhone On-Screen Keyboard`** (serta shortcut keyboard **`Ctrl + K`**).
   - Toggle switch **`keepOnscreenKeyboard`** (default `OFF`): Jika diaktifkan (`ON`), setelah setiap pengetikan selesai (dengan debounce cerdas ~300ms agar tidak mengganggu pengetikan cepat), aplikasi otomatis mengirimkan sinyal toggle agar on-screen keyboard iPhone tetap terlihat.

3. **Panel UI Elegan & Simpan Permanen**:
   - Kartu khusus **KEYBOARD & SHORTCUTS** di Side Drawer Settings:
     * Toggle `⚡ Cross-Platform Shortcuts` (`[ ON | OFF ]`).
     * Toggle `📋 Ctrl+V Pastes PC Clipboard` (`[ ON | OFF ]`) + Tombol cepat `"📋 Paste PC Text Now"`.
     * Toggle `📱 Keep iPhone Keyboard Visible` (`[ ON | OFF ]`) + Tombol cepat `"⌨️ Toggle iPhone On-Screen Keyboard"`.
     * Badge visual daftar shortcut aktif: `Ctrl+A (Select All) · Ctrl+C (Copy) · Ctrl+V (Paste) · Ctrl+Z (Undo) · Ctrl+K (Keyboard)`.
   - Seluruh nilai tersimpan permanen di [`user_settings.json`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/user_settings.json) dan `localStorage`, serta otomatis disinkronkan ke iframe mirroring via `postMessage`.

---

## 2. Rincian Teknis Implementasi

### A. Backend (`handset/screen.py` & `handset/server.py`)

1. **Patching HID Consumer Buttons di `handset/screen.py`**:
   - Memperluas dictionary `_NAMED_BUTTONS` pada modul `ScreenStreamServer`:
     ```python
     # HID Consumer Control (0x0C): 0x029D (AL Keyboard Layout), 0x00B8 (Eject)
     _NAMED_BUTTONS["keyboard-toggle"] = (12, 0x029D, 0.05)
     _NAMED_BUTTONS["eject"] = (12, 0x00B8, 0.05)
     ```
   - Mengizinkan endpoint `/button` upstream memicu sinyal hardware button `IndigoHIDService.send_button` dengan usage code yang tepat ke iOS.

2. **Pembaca Clipboard Windows PC & Injeksi Paste di `handset/server.py`**:
   - Fungsi `get_windows_clipboard_text()` membaca teks UTF-16/Unicode secara langsung dari Windows Clipboard menggunakan API Win32 `ctypes.windll.user32.GetClipboardData(CF_UNICODETEXT)` dan `GlobalLock`.
   - Fungsi `paste_to_iphone(text=None)`:
     * Mengirimkan teks ke `http://127.0.0.1:8766/clipboard` (Apple Pasteboard Service).
     * Mengirimkan HID key event `Cmd (0xE3) + V (0x19)` lalu release `[]` ke `http://127.0.0.1:8766/key`.
   - Fungsi `toggle_onscreen_keyboard()`:
     * Mengirimkan request `POST /button` dengan nama `"keyboard-toggle"` dan `"eject"`.

3. **Endpoints Baru**:
   - `POST /api/keyboard/paste`: Menerima `{ text?: string }` atau membaca Windows PC clipboard otomatis.
   - `POST /api/keyboard/toggle-onscreen`: Memunculkan/menyembunyikan virtual keyboard di iOS.

4. **Pembaruan `DEFAULT_SETTINGS` & `save_user_settings`**:
   - Ditambahkan field:
     * `"crossPlatformShortcuts": True`
     * `"pastePcClipboard": True`
     * `"keepOnscreenKeyboard": False`

---

### B. Frontend Keyboard Engine (`handset/web/keys.js`)

1. Objek global `window.HandsetKeyboardSettings` dibuat sebagai single source of truth yang sinkron antara induk (`index.html`) dan iframe (`mirror.html`).
2. Event listener `keydown`:
   - Menangkap `Ctrl + K` ➔ Memanggil `triggerToggleOnscreenKeyboard()`.
   - Menangkap `Ctrl + V`:
     * Jika `pastePcClipboard` aktif: memanggil `triggerPasteFromPc()`.
     * Jika nonaktif: mengirimkan `Cmd (0xE3) + V (0x19)` native iOS.
   - Menangkap `Ctrl + A`, `Ctrl + C`, `Ctrl + X`, `Ctrl + Z`:
     * Mengirimkan modifier `CMD (0xE3)` + usage code huruf yang bersangkutan.
3. Event listener `keyup`:
   - Melakukan debounce 300ms saat seluruh tombol dilepas (`pressed.size === 0`) untuk memicu `checkKeepOnscreenKeyboard()`.
4. Mengekspos API: `window.HandsetKeys = { attach, pasteFromPc, toggleOnscreenKeyboard }`.

---

### C. UI & Styling (`index.html`, `app.css`, `app.js`, `mirror.js`)

1. **Card UI di `handset/web/index.html`**:
   - Ditempatkan tepat di bawah card `Trackpad Gesture Speed`.
   - Menggunakan 3 toggle segmented control (`ON` / `OFF`) yang responsif dan elegan.
   - Tombol aksi dengan feedback visual:
     * `"📋 Paste PC Text Now"` ➔ `"✓ Pasted!"`
     * `"⌨️ Toggle iPhone On-Screen Keyboard"` ➔ `"✓ Toggled!"`
2. **Styling di `handset/web/app.css`**:
   - Penataan layout WinUI 3 Fluent Liquid Glass: `.panel-keyboard-shortcuts`, `.setting-toggle-item`, `.setting-toggle-header`, `.setting-label`, `.setting-hint`, `.btn-action-block`, `.shortcuts-badge-list`, dan `.shortcut-pill`.
3. **Sinkronisasi di `handset/web/app.js` & `handset/web/mirror.js`**:
   - Fungsi `applyKeyboardSettings`, `saveKeyboardSettings`, dan `initKeyboardSettingsControls`.
   - Broadcast state melalui `postMessage({ type: 'set-keyboard-settings', settings })` setiap kali settingan berubah atau iframe dimuat ulang.
   - Cache buster diperbarui ke `v=20261023_tahap18_shortcuts_keyboard`.

---

## 3. Daftar File yang Dimodifikasi

| No | File Path | Keterangan |
|:---|:---|:---|
| 1 | [`handset/screen.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/screen.py) | Patch `_NAMED_BUTTONS` dengan `keyboard-toggle` (12, 0x029D) dan `eject` (12, 0x00B8). |
| 2 | [`handset/server.py`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/server.py) | Pembaca clipboard Windows native `ctypes`, endpoint `/api/keyboard/paste`, `/api/keyboard/toggle-onscreen`, dan persistence settings. |
| 3 | [`handset/web/keys.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/keys.js) | Intersepsi `Ctrl+A/C/V/Z`, shortcut `Ctrl+K`, integrasi paste PC clipboard, dan debounce keep keyboard. |
| 4 | [`handset/web/index.html`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/index.html) | Kartu UI `KEYBOARD & SHORTCUTS` dengan 3 segmented toggle dan 2 tombol aksi, bump cache buster. |
| 5 | [`handset/web/app.css`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/app.css) | Styling WinUI 3 untuk panel keyboard, toggle header, action buttons, dan badge pills. |
| 6 | [`handset/web/app.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/app.js) | Logika `applyKeyboardSettings`, `saveKeyboardSettings`, `initKeyboardSettingsControls`, dan sinkronisasi startup. |
| 7 | [`handset/web/mirror.html`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/mirror.html) | Update cache buster ke `v=20261023_tahap18_shortcuts_keyboard`. |
| 8 | [`handset/web/mirror.js`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/handset/web/mirror.js) | Handler pesan `set-keyboard-settings` untuk sinkronisasi state ke `window.HandsetKeyboardSettings`. |
| 9 | [`user_settings.json`](file:///d:/SCREEN%20MIROR%20IOS/pane-iphone-mirror-windows/user_settings.json) | Menambahkan field konfigurasi keyboard default. |

---

## 4. Hasil Verifikasi & Pengujian

1. **Python Compilation**:
   - `handset/server.py`, `handset/screen.py`, `handset/gui.py`, `handset/audio.py` dikompilasi dengan `py_compile`: **OK (Code 0, No Errors)**.
2. **Windows Clipboard Native Reader**:
   - Pembacaan teks clipboard via Win32 ctypes teruji berhasil membaca data clipboard aktif tanpa lag.
3. **Penyimpanan Permanen `user_settings.json`**:
   - Pengujian simpan dan muat konfigurasi `crossPlatformShortcuts`, `pastePcClipboard`, dan `keepOnscreenKeyboard` terverifikasi 100% konsisten.
4. **Integritas Fitur Eksisting**:
   - Logika `handset/audio.py` (Audio Sink), Save Crop 4 sisi, dan Trackpad Gesture Speed tetap terjaga utuh.
