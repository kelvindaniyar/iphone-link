# 🚀 iPhone Link v2.0.0 Major Official Release (60fps HEVC Sharpness, 37-Scenario Smart Auto-Rotate, 1-Click Grab Link `Alt+B`, Bluetooth Audio & WinUI 3 Liquid Glass)

🌐 **Official Interactive Website (ID / EN):** [**https://kelvindaniyar.github.io/iphone-link/**](https://kelvindaniyar.github.io/iphone-link/)  
☕ **Support Creator on Saweria:** [**https://saweria.co/kelvindaniyar**](https://saweria.co/kelvindaniyar)

**iPhone Link v2.0.0** is our largest milestone upgrade delivering a faster, sharper, cooler, and smarter **60fps H.265 (HEVC) iPhone Screen Mirroring & USB HID Control** experience on **Windows 10 & Windows 11 (64-bit)**, physically calibrated across both **Dynamic Island** (**iPhone 17 Pro**, iOS 27.0.1) and **Notch** (**iPhone 11 Pro Max**, iOS 27.0+) devices.

---

## ✨ What's New in iPhone Link v2.0.0

- 🔗 **1-Click Social Link Grabber (`Alt+B` in `~1.3s`)**  
  Tap *Copy Link* in **Instagram Feeds, Instagram Reels (`?igsh=`), or Threads** on your iPhone, then press **`Alt+B`** on Windows. Automatically extracts full 11 14 character shortcodes + query parameters straight into the **Windows Win32 Clipboard** (even if the mirror window loses focus), auto-unselects selection handles cleanly, and dismisses Spotlight immediately.
- 🔄 **37-Scenario Smart Auto-Rotate Engine (`Alt+H`)**  
  Automatically pivots into **270° Widescreen Landscape** (with 1:1 inverse capacitive touch mapping) for *eFootball* and *YouTube Fullscreen*, while staying strictly **0° Portrait** on the iOS Home Screen (4-icon Dock capsule guard), App Folders, YouTube PiP, Portrait Watch Pages, TikTok, and Instagram Dark Mode.
- ⚡ **Zero-Drop 60fps H.265 Motion Sharpness + Smart Motion PLI Booster**  
  Upgraded subscriber ring buffer (`240` NAL packets), `TCP_NODELAY` low-latency socket transport, `prefer-hardware` WebCodecs decoding, high-quality canvas interpolation, and automatic **Picture Loss Indication (PLI)** keyframe boosting (`180ms` post-swipe settle) eliminate blockiness during fast trackpad scrolling and folder animations.
- 🔊 **Instant 1-Click Bluetooth Audio Sink (`~1.2s`)**  
  Stream iPhone music, Reels, TikTok, and game audio directly through your Windows PC speakers or headphones via native WinRT `AudioPlaybackConnection` with EcoQoS anti-throttling and `1ms` multimedia timer precision.
- 📋 **Multi-Paragraph Smart OCR Copy-Paste (`Alt+C`) & PC Paste (`Alt+V`)**  
  Extracts clean multi-paragraph text from *Apple Notes*, *Google Keep*, *WhatsApp*, or screenshots directly to your PC Clipboard with original line/paragraph breaks preserved.
- 🌑 **USB Stealth Dim `0%` (`Alt+D`) + Calibrated `~25%` Restore**  
  Dims your physical iPhone OLED display to `0%` pitch-black on your desk while keeping the Windows mirror `100%` bright, and safely restores `~25%` brightness (4 iOS bars) when toggled off or exited.
- 🪟 **WinUI 3 Liquid Glass (Dark & Crystal Light) + Live Wallpaper Accent**  
  Frameless Windows 11 aesthetic that absorbs your desktop wallpaper accent colors in `< 0.8s`, paired with **4-Side Live Edge Crop (`0 24 px`)**, **System Tray (`Shell_NotifyIconW`)**, and **Win32 Job Object** zero-orphan process protection (`~1.8% CPU`, `~88 MB RAM`).

---

## 📥 Download Assets (`v2.0.0`)

| Asset File | Purpose | Requirement |
| :--- | :--- | :--- |
| 📦 **`iPhone-Link-Win64-Standalone-EXE-v2.0.0.zip`** | **Recommended** Self-contained Windows 64-bit `iPhone Link.exe` bundle (`~48 MB` ZIP) | **Zero Python needed** (Extract & Run) |
| 🎬 **`Microsoft.HEVCVideoExtension_2.5.48.0_neutral_._8wekyb3d8bbwe.AppxBundle`** | **Mandatory 1x Install** Hardware H.265 (HEVC) video codec so mirroring never shows a blank/black screen | Double-click → Click **Install** once |
| ⚡ **`iPhone-Link-Portable-v2.0.0.zip`** | Ultra-lightweight Smart-Portable package (`~536 KB`) | Auto-bootstraps Python 3.12 `venv` on 1st run |

---

## 🚀 4-Step Anti-Blank Screen Quick Setup

1. **Apple USB Driver**: Install **[Apple Devices](https://apps.microsoft.com/detail/9np83lwlpz9k)** from Microsoft Store, connect your iPhone via USB data cable, and tap **Trust This Computer**.
2. **HEVC Video Codec (1x Only)**: Double-click **`Microsoft.HEVCVideoExtension_2.5.48.0_neutral_._8wekyb3d8bbwe.AppxBundle`** (attached below) and click **Install** so Edge WebView2 can decode 60fps H.265 hardware frames.
3. **iPhone Developer Mode (Requires iOS 27.0+ Only):** Enable **Settings → Privacy & Security → Developer Mode** on your iPhone. *(Note: iPhone Link v2.0 exclusively requires iOS 27.0+. Older iOS versions, iOS 26 and below, are not supported yet due to blank screen streaming limitations).*  
   *(Tip: If Developer Mode is hidden on your iPhone, open free **[3uTools](https://www.3u.com/) → Toolbox → Realtime Screen** once over USB the menu appears immediately!)*
4. **Launch**: Extract **`iPhone-Link-Win64-Standalone-EXE-v2.0.0.zip`**, run **`iPhone Link.exe`**, and click **Start Mirroring**!

---

## 🛡️ Windows SmartScreen & SHA-256 Checksums

Because **iPhone Link** is a free, open-source (**GPL-3.0**) community release without a paid commercial EV certificate:
- If your browser warns *"File isn't commonly downloaded"*, choose **`⋯` → Keep (Simpan)**.
- If Windows SmartScreen shows *"Windows protected your PC"*, click **More info (Info selengkapnya)** → **Run anyway (Tetap jalankan)**.

| File | Verified SHA-256 Checksum |
| :--- | :--- |
| `iPhone Link.exe` | `58ED95FBE9DAE36F9AB137DB1841ACF6BA7C00107AFE0B8B659B3F7BE7C63DAA` |
| `iPhone-Link-Win64-Standalone-EXE-v2.0.0.zip` | `007C6A393B8091FC8F50D7B1BC97E95D71134CF88F890FE47C579D3487E050A7` |
| `iPhone-Link-Portable-v2.0.0.zip` | `BB5BD3E9EB51D98A6BACE7A02341DF8AD4CC478FA838946999E2AC7F59822359` |

---

## 🙏 Credits & License

- **Creator & Lead Developer**: [**Kelvin Daniyar (`@kelvindaniyar`)**](https://github.com/kelvindaniyar) · [Official Website](https://kelvindaniyar.github.io/iphone-link/) · [Saweria](https://saweria.co/kelvindaniyar)
- **License**: GNU General Public License v3.0 (`GPL-3.0`)
- **Upstream Base Fork**: Originally forked from [`teyboxv1/pane-iphone-mirror-windows`](https://github.com/teyboxv1/pane-iphone-mirror-windows), powered by [`pymobiledevice3`](https://github.com/doronz88/pymobiledevice3).

