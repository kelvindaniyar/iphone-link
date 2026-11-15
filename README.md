<p align="center">
  <a href="https://kelvindaniyar.github.io/iphone-link/">
    <img src="assets/logo.png" width="176" height="176" alt="iPhone Link v4.0.0 Logo - Native Rust iPhone &amp; iPad Screen Mirroring for Windows 11" style="border-radius: 38px; box-shadow: 0 16px 36px rgba(0, 120, 212, 0.35);">
  </a>
</p>

<h1 align="center">iPhone Link v4.0.0 (Native Rust Spatial Edition) for Windows 10 &amp; 11</h1>

<p align="center">
  <b>One crystal window for your iPhone &amp; iPad on Windows: 60fps H.265 USB &amp; Wi-Fi Low-Latency Mirroring, 100% Native Rust Core (1.33 MB, &lt; 0.25s cold boot, 0.00% idle CPU), True DWM Acrylic Blur Lock, 32-Scenario Smart Auto-Rotate, Smart OCR (<code>Alt+C</code>/<code>Alt+V</code>), Stealth Dim (<code>Alt+D</code>), Wireless Audio + Lip-Sync, &amp; Zero-Python 64-Bit Installer.</b><br>
  <b>Official Interactive Product Website (Live Preview):</b> <a href="https://kelvindaniyar.github.io/iphone-link/"><b>https://kelvindaniyar.github.io/iphone-link/</b></a>
</p>

<p align="center">
  <a href="https://kelvindaniyar.github.io/iphone-link/"><img src="https://img.shields.io/badge/Website-kelvindaniyar.github.io%2Fiphone--link-0EA5E9?style=flat-square&logo=googlechrome&logoColor=white" alt="Official Website"></a>
  <a href="https://github.com/kelvindaniyar/iphone-link/releases/latest"><img src="https://img.shields.io/badge/Release-v4.0.0%20(Win64%20Rust)-22C55E?style=flat-square&logo=github" alt="Release v4.0.0"></a>
  <a href="https://github.com/kelvindaniyar"><img src="https://img.shields.io/badge/Creator-Kelvin%20Daniyar%20(%40kelvindaniyar)-4F46E5?style=flat-square&logo=github" alt="Creator: Kelvin Daniyar"></a>
  <a href="https://saweria.co/kelvindaniyar"><img src="https://img.shields.io/badge/Support_on-Saweria-F59E0B?style=flat-square&logo=buymeacoffee&logoColor=white" alt="Support on Saweria"></a>
  <a href="https://github.com/teyboxv1/pane-iphone-mirror-windows"><img src="https://img.shields.io/badge/Base%20Fork-teyboxv1%2Fpane--iphone--mirror--windows-0284C7?style=flat-square&logo=github" alt="Base Fork: teyboxv1/pane-iphone-mirror-windows"></a>
  <img src="https://img.shields.io/badge/License-GPL--3.0-16A34A.svg?style=flat-square" alt="License: GPL-3.0">
  <img src="https://img.shields.io/badge/OS-Windows%2010%20%2F%2011%20(64--bit)-0078D4.svg?style=flat-square&logo=windows" alt="Windows 10/11 64-bit">
  <img src="https://img.shields.io/badge/iOS-27.2%20Verified%20(27.0%2B)-10b981?style=flat-square&logo=apple" alt="iOS 27.2 Verified">
</p>

---

## Overview & Physically Verified Devices (`v4.0.0`)

**iPhone Link v4.0.0 (Native Rust Spatial Edition)** (engineered by **[Kelvin Daniyar](https://github.com/kelvindaniyar)**) brings **macOS Sequoia-class iPhone &amp; iPad Mirroring** to Windows 10 and Windows 11 (64-bit) with both **USB Cable (60fps H.265 Zero-Copy)** and **Wi-Fi Low-Latency** modes.

- **Official Interactive Website (ID / EN + Live Simulator):** [https://kelvindaniyar.github.io/iphone-link/](https://kelvindaniyar.github.io/iphone-link/)
- **Latest Official Installer &amp; Portable Releases (`v4.0.0`):** [https://github.com/kelvindaniyar/iphone-link/releases/latest](https://github.com/kelvindaniyar/iphone-link/releases/latest)
- **Physically Verified Devices (iOS 27.0+ / iOS 27.2):**
  - **Dynamic Island Generation:** **iPhone 17 Pro** running **iOS 27.2** (and iPhone 15 / 16 series running **iOS 27.0+**)
  - **Notch Generation:** **iPhone 11 Pro Max** running **iOS 27.2** (and iPhone 11 / 12 / 13 / 14 series running **iOS 27.0+**)
  - **Tablet Profile:** **iPad (`4:3`)** precision framing (`924x832` drawer open / `564x832` drawer closed, `0px` scrollbar gap)

---

## Generational Evolution: `🆕 NEW` in v4.0.0 vs `🚀 UPGRADE` from v2.0.0

### `🆕 NEW` - 12 New Features &amp; Architecture in v4.0.0

| # | New Feature (`NEW IN v4.0`) | Technical Capability &amp; User Benefit |
| :---: | :--- | :--- |
| 1 | **Wi-Fi Low-Latency Mirroring** | Wireless local-network AirPlay/Lockdown mirroring with instant USB Cable / Wi-Fi switching inside the app and drawer. |
| 2 | **100% Native Rust Engine (`1.33 MB`)** | Win32 + WebView2 executable shrunk by `93%` (from `19.26 MB` to `~1.33 MB`) with `< 0.25s` cold boot and `0.00%` idle CPU. |
| 3 | **64-Bit Official Installer (`Setup.exe`) Zero Python Needed** | Modern Inno Setup 64-bit wizard (`C:\Program Files\iPhone Link`) packs an isolated Python 3.12 runtime and automatic LAN firewall rules. |
| 4 | **Bundled Offline HEVC Codec** | Embeds `Microsoft.HEVCVideoExtension_2.5.48.0` directly in `Setup.exe` and installs silently to prevent blank black screens. |
| 5 | **Zero `Python.Runtime.dll` Bug + Auto-Unblock** | Eliminates `clr`/`pythonnet` crashes and strips Windows `:Zone.Identifier` MOTW attributes automatically on startup. |
| 6 | **Built-in Software Updater (OTA)** | Drawer `#card-updater` card plus System Tray `Check for Updates...` with `Stable` / `Beta` channel selection and SHA-256 verification. |
| 7 | **iPad (`4:3`) Aspect Ratio Support** | Calibrated `4:3` iPad window geometry (`924x832` open / `564x832` closed) with zero drawer clipping or scrollbar gap. |
| 8 | **Full Bilingual UI (`ID` &amp; `EN`)** | Instant Indonesian/English switching across all drawer cards, updater labels, and status messages with guaranteed toggle consistency. |
| 9 | **Unfocused DWM Blur Lock** | Intercepts `WM_NCACTIVATE` so Windows 11 True Acrylic stays translucent and blurred even when window focus moves elsewhere. |
| 10 | **Audio Buffer &amp; Lip-Sync Presets** | Low/Normal/High jitter buffer presets paired with Lip-Sync calibration and WASAPI MMCSS Pro Audio priority. |
| 11 | **Universal 18-Control Auto-Save** | Dual persistence (`user_settings.json` + `localStorage`) restores every drawer toggle, slider, PIN, and language seamlessly across restarts. |
| 12 | **9-Item System Tray + `Start with Windows`** | Isolated `HKCU Run\iPhoneLink` startup integration plus automatic tray popup dismissal when clicking back into WebView2. |

### `🚀 UPGRADE` - 13 Core v2.0.0 Pillars Upgraded for v4.0.0

| # | Upgraded Pillar (`UPGRADE`) | Enhancement in v4.0.0 |
| :---: | :--- | :--- |
| 1 | **USB Cable 60fps Frame-0 Lock** | Instant Frame-0 `1178x2598` calibration on iPhone 17 Pro (`346px` Super Retina) with zero startup resize jump. |
| 2 | **Physically Tested on iOS 27.2** | Validated end-to-end on physical **iPhone 17 Pro (iOS 27.2)** and **iPhone 11 Pro Max (iOS 27.2)**. |
| 3 | **True DWM Transient Acrylic (Dark &amp; Light)** | Frosted Dark &amp; Warm Frosted Pearl palettes with `0` black rounded corner pixels. |
| 4 | **Dual Wireless Audio + EcoQoS Bypass** | Streams iOS audio over Windows speakers or Bluetooth A2DP with EcoQoS throttling disabled. |
| 5 | **Smart Auto-Rotate 32 Scenarios (`270°`)** | Enters `270°` widescreen automatically for *eFootball 2027* and *YouTube Fullscreen* with 1:1 inverse touch mapping. |
| 6 | **Anti-False Rotate Guard** | Locks iOS Home Screen, App Dock, Instagram, TikTok, and PiP overlays safely at `0°` portrait. |
| 7 | **Dual-Storage PIN Persistence (`Alt+U` / `Alt+L`)** | Persists optional 6-digit PIN reliably across restarts and wipes cleanly via 1-click Clear Privacy Data. |
| 8 | **Single-Channel Stealth Dim `0x0070` (`Alt+D`)** | Drops physical OLED screen to `0%` while PC mirror stays `100%` bright and restores `~25%` cleanly on exit. |
| 9 | **Multi-Frame OCR Stitcher (`Alt+C`)** | Preserves paragraph breaks across *Apple Notes* / *WhatsApp* while filtering floating balloons and UI noise. |
| 10 | **Direct HID Paste + Shift Latching (`Alt+V` / `Ctrl+K`)** | Accurate PC-to-iPhone text typing including `?` and uppercase symbols with proper HID shift-latch ordering. |
| 11 | **120Hz Touch &amp; Natural 2-Finger Trackpad Scroll** | Crisp capacitive clicks, drags, swipes, and laptop trackpad scrolling aligned with orientation. |
| 12 | **4-Side Edge Crop &amp; Dynamic Macroblock Trim** | Persistent `0-24px` bezel crop paired with H.265 `38px` right / `58px` bottom trim for `0` black bars. |
| 13 | **Official Uninstaller (`unins000.exe` &amp; `.bat`)** | Cleanly terminates worker processes, removes firewall rules, shortcuts, and WebView2 storage. |

---

## Comparison: iPhone Link v4.0 vs v2.0 vs Phone Link vs AirPlay

| Capability | **iPhone Link v4.0** | iPhone Link v2.0 | Microsoft Phone Link (iOS) | Generic AirPlay Receiver |
| :--- | :--- | :--- | :--- | :--- |
| **Live 60fps H.265 Mirroring** | Yes (Frame-0 Lock) | Yes (USB Only) | No (Notifications/Calls only) | Wi-Fi compressed, stutters |
| **USB Cable + Wi-Fi Low-Latency** | Both (1-click switch) | USB Only | Bluetooth relays | Wi-Fi only |
| **Engine Architecture** | Native Rust (`1.33 MB`, `< 0.25s` boot) | Python + PyWebView (`19.26 MB`) | Proprietary UWP | Heavy proprietary runtime |
| **iPhone &amp; iPad (`4:3`) Support** | iPhone + iPad `4:3` (`0px` gap) | iPhone focused | No | Often letterboxed |
| **Installer Without System Python** | Yes (`Setup.exe` + HEVC) | Separate ZIP + HEVC | Built-in | Installer varies |
| **120Hz Mouse, Trackpad &amp; Keyboard** | Direct HID 1:1 + Shift Latch | USB HID | No screen control | View-only |
| **32-Scenario Auto-Rotate (`270°`)** | Yes + Anti-False Guard | Yes | No | View-only |
| **OCR (`Alt+C`), Paste (`Alt+V`), Stealth (`Alt+D`)** | Dual-Storage + `0x0070` | Per-session | No | No |
| **Wireless Audio + Lip-Sync** | WASAPI + A2DP + Buffer Presets | Basic A2DP | Phone calls only | Network lag |
| **Privacy &amp; License** | 100% Local (`127.0.0.1`), GPL-3.0 | 100% Local, GPL-3.0 | Microsoft Account | Paid / Watermarked |

---

## Downloads, Quick Setup &amp; Verified SHA-256 (`v4.0.0`)

### 1. Choose Your Package
- **Recommended: `iPhone-Link-v4.0.0-Setup.exe` (`60.62 MB`)** - Official 64-bit Windows Installer (`C:\Program Files\iPhone Link`). Includes isolated Python 3.12 runtime, automatic LAN Firewall rules, Desktop/Start Menu shortcuts, and **Microsoft HEVC Codec (`2.5.48.0`)** bundled offline.
- **Portable: `iPhone-Link-Win64-Native-Rust-v4.0.0.zip` (`1.54 MB`)** - 21 files (`3.67 MB` uncompressed) with Native Rust `:Zone.Identifier` unblocker, `Create-Desktop-Shortcut.bat`, and `Uninstall-iPhone-Link.bat` for systems that already have Python 3.12 installed.
- **Optional HEVC Standalone: `Microsoft.HEVCVideoExtension_2.5.48.0_neutral_._8wekyb3d8bbwe.AppxBundle` (`11.2 MB`)** - Already installed automatically by `Setup.exe`; download manually only if using Portable ZIP on a fresh Windows PC without H.265 codecs.

### 2. Prerequisites on Windows &amp; iPhone (`iOS 27.2` / `27.0+`)
1. Install **[Apple Devices](https://apps.microsoft.com/detail/9np83lwlpz9k)** (or iTunes) from Microsoft Store for the Apple USB driver and tap **Trust This Computer** on your iPhone/iPad.
2. Enable **Developer Mode** on iOS (`Settings -> Privacy &amp; Security -> Developer Mode`). If hidden, connect via USB and open **3uTools -> Toolbox -> Realtime Screen** once to reveal the toggle.
3. Verify checksums anytime in PowerShell (`Get-FileHash <file> -Algorithm SHA256`):

| Artifact (`v4.0.0`) | Size | SHA-256 Checksum |
| :--- | :--- | :--- |
| **`iPhone-Link-v4.0.0-Setup.exe`** | `60.62 MB` (`63,561,835 B`) | `F70DDE4A1018DE7CC2B18DA802095FEFF15885943AEC580B86B2769F80C8DE8C` |
| **`iPhone-Link-Win64-Native-Rust-v4.0.0.zip`** | `1.54 MB` (`1,611,609 B`) | `48F3AA2C48107512DDFB4D107230D9BDA1DD26645D890A912FFA1BAC5B54CE82` |
| **`iPhone Link.exe`** | `1.44 MiB` (`1,504,768 B`) | `2D23A81AE4F31B9F2A2E6A1D77F6AF2A987BCD3EBC9933288610B52DD3718CD8` |
| **`Microsoft.HEVCVideoExtension_2.5.48.0...AppxBundle`** | `11.2 MB` (`11,222,383 B`) | `275FB4C0AE406D51892E87281EFF0BB3B9456D9A9493813CC059EEB8A83D39BA` |

---

## Support, License &amp; Credits

- **Creator &amp; Lead Developer:** [**Kelvin Daniyar (`@kelvindaniyar`)**](https://github.com/kelvindaniyar) | [Official Website](https://kelvindaniyar.github.io/iphone-link/) | [Instagram](https://www.instagram.com/kelvin.daniyar/) | [TikTok](https://www.tiktok.com/@kelvindaniyar) | [Saweria](https://saweria.co/kelvindaniyar)
- **License:** GNU General Public License v3.0 ([`LICENSE`](LICENSE))
- **Base Fork Attribution:** Originally forked from [**teyboxv1/pane-iphone-mirror-windows**](https://github.com/teyboxv1/pane-iphone-mirror-windows) (GPL-3.0)
- **USB &amp; Apple Service Protocol:** Powered by [**pymobiledevice3**](https://github.com/doronz88/pymobiledevice3)
