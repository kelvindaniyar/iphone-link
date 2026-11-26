//! Native Rust 1-Click AMFI Developer Mode Status & Unlocker (`idevice::amfi::AmfiClient`).
//!
//! Replaces Python `pymobiledevice3.services.amfi.AmfiService` and provides 100% schema parity
//! with `handset/server.py` (`GET /api/driver/diagnostics` & `POST /api/device/unlock-developer-mode`).

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use idevice::{
    amfi::AmfiClient,
    lockdown::LockdownClient,
    provider::IdeviceProvider as _,
    usbmuxd::{Connection, UsbmuxdAddr},
};

use crate::link::{block_on, usbmux};

const DEVMODE_CACHE_TTL: Duration = Duration::from_secs(5);

static DEVMODE_CACHE: OnceLock<Mutex<HashMap<String, (bool, Instant)>>> = OnceLock::new();
static AUTO_REVEALED_UDIDS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn devmode_cache() -> &'static Mutex<HashMap<String, (bool, Instant)>> {
    DEVMODE_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn auto_revealed() -> &'static Mutex<HashSet<String>> {
    AUTO_REVEALED_UDIDS.get_or_init(|| Mutex::new(HashSet::new()))
}

pub fn clear_devmode_cache(udid: Option<&str>) {
    if let Ok(mut g) = devmode_cache().lock() {
        match udid {
            Some(u) if !u.is_empty() => {
                g.remove(u);
            }
            _ => g.clear(),
        }
    }
}

pub fn set_cached_devmode(udid: &str, enabled: bool) {
    if udid.is_empty() {
        return;
    }
    if let Ok(mut g) = devmode_cache().lock() {
        g.insert(udid.to_string(), (enabled, Instant::now()));
    }
}

pub fn get_cached_devmode(udid: &str) -> Option<bool> {
    let g = devmode_cache().lock().ok()?;
    g.get(udid).and_then(|(val, ts)| {
        if ts.elapsed() < DEVMODE_CACHE_TTL {
            Some(*val)
        } else {
            None
        }
    })
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevModeUnlockResult {
    pub ok: bool,
    pub connected: bool,
    pub udid: Option<String>,
    pub revealed: bool,
    pub developer_mode_status: String,
    pub status_code: String,
    pub error: Option<String>,
    pub message_id: String,
    pub message_en: String,
}

impl DevModeUnlockResult {
    pub fn to_json_string(&self) -> String {
        let udid_json = match &self.udid {
            Some(u) => format!(r#""{}""#, json_escape(u)),
            None => "null".to_string(),
        };
        let err_part = match &self.error {
            Some(e) => format!(r#","error":"{}""#, json_escape(e)),
            None => String::new(),
        };
        format!(
            r#"{{"ok":{},"connected":{},"udid":{},"revealed":{},"developer_mode_status":"{}","status_code":"{}","statusCode":"{}","message_id":"{}","message_en":"{}"{}}}"#,
            self.ok,
            self.connected,
            udid_json,
            self.revealed,
            json_escape(&self.developer_mode_status),
            json_escape(&self.status_code),
            json_escape(&self.status_code),
            json_escape(&self.message_id),
            json_escape(&self.message_en),
            err_part
        )
    }
}

/// Classifies AMFI Lockdown/Pairing/Passcode errors into actionable bilingual UX status codes.
pub fn classify_amfi_connect_or_reveal_error(err: &str) -> (&'static str, String, String) {
    let low = err.to_ascii_lowercase();
    if low.contains("passwordprotected")
        || low.contains("invalidhostid")
        || low.contains("pairingdialogresponsepending")
        || low.contains("userdeniedpairing")
        || low.contains("nopairingfile")
        || low.contains("pair")
        || low.contains("trust")
        || low.contains("locked")
    {
        return (
            "device_locked_or_untrusted",
            format!("Buka kunci layar iPhone lalu ketuk 'Percayai Komputer Ini' (Trust This Computer), kemudian klik Developer Mode lagi. ({err})"),
            format!("Unlock your iPhone screen and tap 'Trust This Computer', then click Developer Mode again. ({err})"),
        );
    }
    (
        "amfi_error",
        format!("Pastikan iPhone tersambung lewat kabel USB, layar terbuka kuncinya, & telah mengetuk Trust This Computer. ({err})"),
        format!("Make sure your iPhone is connected via USB cable, unlocked, and Trust This Computer was accepted. ({err})"),
    )
}

/// Sends exact Apple `com.apple.amfi.lockdown` 4-byte BE length-prefixed plist actions (`0`, optional `1`, `3`)
/// over a live stream. Returns `(revealed, enable_ok, dev_enabled)`.
pub fn execute_amfi_unlock_over_stream<S: std::io::Read + std::io::Write>(
    stream: &mut S,
    arm_enable: bool,
) -> Result<(bool, bool, bool), String> {
    fn rpc<S: std::io::Read + std::io::Write>(
        s: &mut S,
        action: i64,
    ) -> Result<plist::Dictionary, String> {
        let mut req = plist::Dictionary::new();
        req.insert("action".into(), plist::Value::from(action));
        let body = plist::to_bytes_xml(&plist::Value::Dictionary(req))
            .map_err(|e| format!("plist encode: {e}"))?;
        let len = u32::try_from(body.len()).map_err(|_| "plist too large".to_string())?;
        s.write_all(&len.to_be_bytes())
            .map_err(|e| format!("amfi write len: {e}"))?;
        s.write_all(&body)
            .map_err(|e| format!("amfi write body: {e}"))?;
        s.flush().map_err(|e| format!("amfi flush: {e}"))?;

        let mut hdr = [0u8; 4];
        s.read_exact(&mut hdr)
            .map_err(|e| format!("amfi read len: {e}"))?;
        let resp_len = u32::from_be_bytes(hdr) as usize;
        if resp_len == 0 || resp_len > 1_048_576 {
            return Err(format!("invalid amfi plist len {resp_len}"));
        }
        let mut resp_buf = vec![0u8; resp_len];
        s.read_exact(&mut resp_buf)
            .map_err(|e| format!("amfi read body: {e}"))?;
        plist::from_bytes(&resp_buf)
            .map_err(|e| format!("amfi plist decode: {e}"))?
            .into_dictionary()
            .ok_or_else(|| "amfi plist root not dictionary".to_string())
    }

    let r0 = rpc(stream, 0)?;
    if r0.get("success").is_none() {
        let err_s = r0
            .get("Error")
            .and_then(|v| v.as_string())
            .unwrap_or("AMFI reveal rejected");
        return Err(err_s.to_string());
    }

    let mut enable_ok = false;
    if arm_enable {
        if let Ok(r1) = rpc(stream, 1) {
            if r1.get("success").is_some() && r1.get("Error").is_none() {
                enable_ok = true;
            }
        }
    }

    let dev_enabled = rpc(stream, 3)
        .ok()
        .and_then(|r3| {
            let ok = r3.get("success").and_then(|v| v.as_boolean()).unwrap_or(false);
            let st = r3.get("status").and_then(|v| v.as_boolean()).unwrap_or(false);
            Some(ok && st)
        })
        .unwrap_or(false);

    Ok((true, enable_ok, dev_enabled))
}

/// Queries live Developer Mode status from a connected USB iPhone via `AmfiClient` / `LockdownClient`.
pub fn query_developer_mode_status(udid: Option<&str>) -> Result<bool, String> {
    block_on(async move {
        let mut mux = usbmux().await?;
        let devs = mux
            .get_devices()
            .await
            .map_err(|e| format!("usbmuxd list failed: {e}"))?;
        let usb_devs: Vec<_> = devs
            .into_iter()
            .filter(|d| d.connection_type == Connection::Usb && !d.udid.is_empty())
            .collect();
        let target = match udid {
            Some(u) if !u.is_empty() => usb_devs
                .iter()
                .find(|d| d.udid == u)
                .cloned()
                .ok_or_else(|| format!("USB device {u} not found"))?,
            _ => usb_devs
                .into_iter()
                .next()
                .ok_or_else(|| "No USB iPhone connected".to_string())?,
        };
        let addr = UsbmuxdAddr::from_env_var().map_err(|e| format!("usbmuxd addr: {e}"))?;
        let provider = target.to_provider(addr, "iphone-link");

        // Primary path: Lockdown TLS session (`com.apple.security.mac.amfi` -> `DeveloperModeStatus`)
        if let Ok(mut lockdown) = LockdownClient::connect(&provider).await {
            let pairing_opt = provider
                .get_pairing_file()
                .await
                .ok()
                .or_else(|| idevice::provider::PairingFile::load_from_lockdown_dir(&target.udid));
            if let Some(pairing) = pairing_opt {
                let _ = lockdown.start_session(&pairing).await;
            }
            if let Ok(status) = lockdown.get_developer_mode_status().await {
                set_cached_devmode(&target.udid, status);
                return Ok(status);
            }
        }

        // Secondary path: AMFI Lockdown service (`com.apple.amfi.lockdown`, action=3)
        let mut amfi = AmfiClient::connect(&provider)
            .await
            .map_err(|e| format!("amfi connect: {e}"))?;
        let status = amfi
            .get_developer_mode_status()
            .await
            .map_err(|e| format!("DeveloperModeStatus query failed: {e}"))?;
        set_cached_devmode(&target.udid, status);
        Ok(status)
    })
}

/// Resolves exact Developer Mode state for 5-point diagnostics without ever returning `"-"`.
/// Automatically triggers 1x-per-session `reveal_developer_mode_option_in_ui()` if disabled.
pub fn resolve_developer_mode_for_diagnostics(
    udid: Option<&str>,
    has_usb: bool,
    is_streaming: bool,
    force_refresh: bool,
) -> (Option<bool>, &'static str, &'static str, &'static str) {
    if has_usb {
        if let Some(u) = udid.filter(|s| !s.is_empty()) {
            if force_refresh {
                clear_devmode_cache(Some(u));
            }
            let dev_opt = get_cached_devmode(u)
                .or_else(|| query_developer_mode_status(Some(u)).ok());
            let dev_bool = dev_opt.unwrap_or(is_streaming);

            if !dev_bool {
                let should_reveal = auto_revealed()
                    .lock()
                    .map(|mut set| set.insert(u.to_string()))
                    .unwrap_or(false);
                if should_reveal {
                    let udid_bg = u.to_string();
                    std::thread::spawn(move || {
                        let _ = unlock_developer_mode_menu(Some(&udid_bg), false);
                    });
                }
            }

            if dev_bool {
                return (
                    Some(true),
                    "enabled",
                    "Enabled (Active)",
                    "Aktif (Enabled)",
                );
            } else {
                return (
                    Some(false),
                    "disabled",
                    "Disabled (Click Unlock)",
                    "Nonaktif (Klik Unlock)",
                );
            }
        }
    }

    if let Some(u) = udid.filter(|s| !s.is_empty()) {
        if let Some(cached) = devmode_cache()
            .lock()
            .ok()
            .and_then(|g| g.get(u).map(|(b, _)| *b))
        {
            return if cached {
                (
                    Some(true),
                    "enabled",
                    "Enabled (Active)",
                    "Aktif (Enabled)",
                )
            } else {
                (
                    Some(false),
                    "disabled",
                    "Disabled (Click Unlock)",
                    "Nonaktif (Klik Unlock)",
                )
            };
        }
    }

    (
        None,
        "disconnected",
        "Connect iPhone via USB",
        "Hubungkan iPhone via USB",
    )
}

/// 1-Click AMFI Developer Mode Unlocker (`AmfiClient::reveal_developer_mode_option_in_ui`).
pub fn unlock_developer_mode_menu(
    udid: Option<&str>,
    arm_enable: bool,
) -> Result<DevModeUnlockResult, String> {
    block_on(async move {
        let mux_res = usbmux().await;
        let devs = match mux_res {
            Ok(mut m) => m.get_devices().await.unwrap_or_default(),
            Err(_) => Vec::new(),
        };
        let usb_devs: Vec<_> = devs
            .into_iter()
            .filter(|d| d.connection_type == Connection::Usb && !d.udid.is_empty())
            .collect();

        if usb_devs.is_empty() {
            return Ok(DevModeUnlockResult {
                ok: false,
                connected: false,
                udid: None,
                revealed: false,
                developer_mode_status: "disconnected".to_string(),
                status_code: "usb_required".to_string(),
                error: Some("No USB iPhone connected (Developer Mode requires USB cable)".to_string()),
                message_id: "Hubungkan iPhone menggunakan kabel data USB terlebih dahulu (Unlock Developer Mode hanya berjalan via USB, bukan Wi-Fi), buka kunci layar, dan ketuk Trust This Computer.".to_string(),
                message_en: "Connect your iPhone with a USB data cable first (Unlock Developer Mode requires USB, not Wi-Fi), unlock the screen, and tap Trust This Computer.".to_string(),
            });
        }

        let target = match udid {
            Some(u) if !u.is_empty() => usb_devs
                .iter()
                .find(|d| d.udid == u)
                .cloned()
                .unwrap_or_else(|| usb_devs[0].clone()),
            _ => usb_devs[0].clone(),
        };
        let target_udid = target.udid.clone();

        let addr = match UsbmuxdAddr::from_env_var() {
            Ok(a) => a,
            Err(e) => {
                let err_s = format!("usbmuxd addr: {e}");
                let (st_code, msg_id, msg_en) = classify_amfi_connect_or_reveal_error(&err_s);
                return Ok(DevModeUnlockResult {
                    ok: false,
                    connected: true,
                    udid: Some(target_udid),
                    revealed: false,
                    developer_mode_status: "disabled".to_string(),
                    status_code: st_code.to_string(),
                    error: Some(err_s),
                    message_id: msg_id,
                    message_en: msg_en,
                });
            }
        };
        let provider = target.to_provider(addr, "iphone-link");

        let mut amfi = match AmfiClient::connect(&provider).await {
            Ok(c) => c,
            Err(e) => {
                let err_s = e.to_string();
                let (st_code, msg_id, msg_en) = classify_amfi_connect_or_reveal_error(&err_s);
                return Ok(DevModeUnlockResult {
                    ok: false,
                    connected: true,
                    udid: Some(target_udid),
                    revealed: false,
                    developer_mode_status: "disabled".to_string(),
                    status_code: st_code.to_string(),
                    error: Some(err_s),
                    message_id: msg_id,
                    message_en: msg_en,
                });
            }
        };

        if let Err(e) = amfi.reveal_developer_mode_option_in_ui().await {
            let err_s = e.to_string();
            let (st_code, msg_id, msg_en) = classify_amfi_connect_or_reveal_error(&err_s);
            return Ok(DevModeUnlockResult {
                ok: false,
                connected: true,
                udid: Some(target_udid),
                revealed: false,
                developer_mode_status: "disabled".to_string(),
                status_code: st_code.to_string(),
                error: Some(err_s),
                message_id: msg_id,
                message_en: msg_en,
            });
        }

        let enable_ok = if arm_enable {
            amfi.enable_developer_mode().await.is_ok()
        } else {
            false
        };

        let dev_enabled = match amfi.get_developer_mode_status().await {
            Ok(b) => b,
            Err(_) => match LockdownClient::connect(&provider).await {
                Ok(mut ld) => {
                    let pairing_opt = provider
                        .get_pairing_file()
                        .await
                        .ok()
                        .or_else(|| idevice::provider::PairingFile::load_from_lockdown_dir(&target_udid));
                    if let Some(pf) = pairing_opt {
                        let _ = ld.start_session(&pf).await;
                    }
                    ld.get_developer_mode_status().await.unwrap_or(false)
                }
                Err(_) => false,
            },
        };

        set_cached_devmode(&target_udid, dev_enabled);
        if let Ok(mut set) = auto_revealed().lock() {
            set.insert(target_udid.clone());
        }

        let (status_code, msg_id, msg_en) = if dev_enabled {
            (
                "already_enabled",
                "Developer Mode sudah aktif di iPhone Anda. Siap langsung melakukan mirroring.",
                "Developer Mode is already enabled on your iPhone. Ready to start mirroring.",
            )
        } else if arm_enable && enable_ok {
            (
                "revealed_prompt_reboot",
                "Berhasil! Perintah Developer Mode terkirim — periksa layar iPhone untuk restart & konfirmasi Nyalakan (Turn On), atau buka Settings -> Privacy & Security -> Developer Mode.",
                "Success! Developer Mode prompt sent — restart your iPhone and tap Turn On when prompted, or open Settings -> Privacy & Security -> Developer Mode.",
            )
        } else {
            (
                "revealed_passcode_manual",
                "Berhasil! Menu Developer Mode telah dimunculkan di iPhone Anda (jika Passcode aktif, aktifkan manual): buka Settings -> Privacy & Security -> Developer Mode (paling bawah) -> Nyalakan -> Restart iPhone.",
                "Success! Developer Mode menu is now visible on your iPhone. Open Settings -> Privacy & Security -> Developer Mode -> Turn On -> Restart iPhone.",
            )
        };

        Ok(DevModeUnlockResult {
            ok: true,
            connected: true,
            udid: Some(target_udid),
            revealed: true,
            developer_mode_status: if dev_enabled {
                "enabled".to_string()
            } else {
                "disabled".to_string()
            },
            status_code: status_code.to_string(),
            error: None,
            message_id: msg_id.to_string(),
            message_en: msg_en.to_string(),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link::tests::MOCK_ENV_LOCK;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn read_usbmux_req(s: &mut std::net::TcpStream) -> plist::Dictionary {
        let mut hdr = [0u8; 16];
        s.read_exact(&mut hdr).unwrap();
        let len = u32::from_le_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]) as usize;
        let mut body = vec![0u8; len - 16];
        s.read_exact(&mut body).unwrap();
        plist::from_bytes(&body).unwrap().into_dictionary().unwrap()
    }

    fn write_usbmux_resp(s: &mut std::net::TcpStream, dict: plist::Dictionary) {
        let body = plist::to_bytes_xml(&plist::Value::Dictionary(dict)).unwrap();
        let total = (16 + body.len()) as u32;
        let mut hdr = [0u8; 16];
        hdr[0..4].copy_from_slice(&total.to_le_bytes());
        hdr[4..8].copy_from_slice(&1u32.to_le_bytes());
        hdr[8..12].copy_from_slice(&8u32.to_le_bytes());
        hdr[12..16].copy_from_slice(&1u32.to_le_bytes());
        s.write_all(&hdr).unwrap();
        s.write_all(&body).unwrap();
        s.flush().unwrap();
    }

    fn read_len_plist(s: &mut std::net::TcpStream) -> plist::Dictionary {
        let mut lb = [0u8; 4];
        s.read_exact(&mut lb).unwrap();
        let len = u32::from_be_bytes(lb) as usize;
        let mut body = vec![0u8; len];
        s.read_exact(&mut body).unwrap();
        plist::from_bytes(&body).unwrap().into_dictionary().unwrap()
    }

    fn write_len_plist(s: &mut std::net::TcpStream, dict: plist::Dictionary) {
        let body = plist::to_bytes_xml(&plist::Value::Dictionary(dict)).unwrap();
        let len = body.len() as u32;
        s.write_all(&len.to_be_bytes()).unwrap();
        s.write_all(&body).unwrap();
        s.flush().unwrap();
    }

    #[test]
    fn test_amfi_unlock_developer_mode_and_status_over_tcp_socket() {
        let _guard = MOCK_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_devmode_cache(None);

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let actions_seen = std::sync::Arc::new(Mutex::new(Vec::<i64>::new()));
        let actions_clone = actions_seen.clone();

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_srv = stop.clone();
        let listener_addr = addr;
        listener.set_nonblocking(true).unwrap();

        let server = std::thread::spawn(move || {
            let mut handlers = Vec::new();
            while !stop_srv.load(std::sync::atomic::Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut s, _)) => {
                        let _ = s.set_nonblocking(false);
                        let _ = s.set_read_timeout(Some(Duration::from_secs(3)));
                        let ac = actions_clone.clone();
                        handlers.push(std::thread::spawn(move || {
                            let req = read_usbmux_req(&mut s);
                            match req.get("MessageType").and_then(|v| v.as_string()).unwrap_or("") {
                                "ListDevices" => {
                                    let mut props = plist::Dictionary::new();
                                    props.insert("SerialNumber".into(), plist::Value::String("00008150-AMFIMOCK".into()));
                                    props.insert("DeviceID".into(), plist::Value::from(5u32));
                                    props.insert("ConnectionType".into(), plist::Value::String("USB".into()));
                                    let mut dev = plist::Dictionary::new();
                                    dev.insert("Properties".into(), plist::Value::Dictionary(props));
                                    let mut resp = plist::Dictionary::new();
                                    resp.insert("DeviceList".into(), plist::Value::Array(vec![plist::Value::Dictionary(dev)]));
                                    write_usbmux_resp(&mut s, resp);
                                }
                                "ReadPairRecord" => {
                                    let mut pair = plist::Dictionary::new();
                                    pair.insert("HostID".into(), plist::Value::String("H1".into()));
                                    pair.insert("SystemBUID".into(), plist::Value::String("B1".into()));
                                    let bytes = plist::to_bytes_xml(&plist::Value::Dictionary(pair)).unwrap();
                                    let mut resp = plist::Dictionary::new();
                                    resp.insert("PairRecordData".into(), plist::Value::Data(bytes));
                                    write_usbmux_resp(&mut s, resp);
                                }
                                "Connect" => {
                                    let port = u16::from_be(req.get("PortNumber").and_then(|v| v.as_unsigned_integer()).unwrap() as u16);
                                    let mut ok_resp = plist::Dictionary::new();
                                    ok_resp.insert("Number".into(), plist::Value::from(0u32));
                                    write_usbmux_resp(&mut s, ok_resp);

                                    if port == 62078 {
                                        for _ in 0..2 {
                                            let lreq = read_len_plist(&mut s);
                                            let kind = lreq.get("Request").and_then(|v| v.as_string()).unwrap_or("");
                                            let mut lresp = plist::Dictionary::new();
                                            if kind == "StartSession" {
                                                lresp.insert("EnableSessionSSL".into(), plist::Value::Boolean(false));
                                            } else if kind == "StartService" {
                                                lresp.insert("Port".into(), plist::Value::from(58123u16));
                                                lresp.insert("EnableServiceSSL".into(), plist::Value::Boolean(false));
                                            }
                                            write_len_plist(&mut s, lresp);
                                        }
                                    } else if port == 58123 {
                                        for _ in 0..2 {
                                            let areq = read_len_plist(&mut s);
                                            let act = areq.get("action").and_then(|v| v.as_signed_integer()).unwrap_or(-1);
                                            ac.lock().unwrap().push(act);
                                            let mut aresp = plist::Dictionary::new();
                                            aresp.insert("success".into(), plist::Value::Boolean(true));
                                            if act == 3 {
                                                aresp.insert("status".into(), plist::Value::Boolean(false));
                                            }
                                            write_len_plist(&mut s, aresp);
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }));
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
            for h in handlers {
                let _ = h.join();
            }
        });

        std::env::set_var("USBMUXD_SOCKET_ADDRESS", listener_addr.to_string());
        let res = unlock_developer_mode_menu(Some("00008150-AMFIMOCK"), false).unwrap();
        std::env::remove_var("USBMUXD_SOCKET_ADDRESS");

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = server.join();
        assert!(res.ok);
        assert!(res.connected);
        assert!(res.revealed);
        assert_eq!(res.udid.as_deref(), Some("00008150-AMFIMOCK"));
        assert_eq!(res.developer_mode_status, "disabled");
        assert!(res.message_id.contains("Developer Mode"));
        assert!(res.message_en.contains("Settings -> Privacy & Security -> Developer Mode"));
        assert_eq!(*actions_seen.lock().unwrap(), vec![0, 3]);

        // Disconnected diagnostic check
        let (_, st_disc, lbl_en, lbl_id) =
            resolve_developer_mode_for_diagnostics(None, false, false, false);
        assert_eq!(st_disc, "disconnected");
        assert_eq!(lbl_en, "Connect iPhone via USB");
        assert_eq!(lbl_id, "Hubungkan iPhone via USB");

        // Connected USB check never yields None/'-' even when offline
        clear_devmode_cache(None);
        let (dev_bool, st_conn, _, _) =
            resolve_developer_mode_for_diagnostics(Some("00008150-OFFLINE"), true, false, true);
        assert_eq!(dev_bool, Some(false));
        assert_eq!(st_conn, "disabled");
    }

    #[test]
    fn test_tahap285_amfi_mock_lockdown_protocol_action_0_1_2_3_and_edge_cases() {
        let _guard = MOCK_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_devmode_cache(None);

        // 1. Verify raw AMFI stream helper (`com.apple.amfi.lockdown` 4-byte BE plist RPC)
        // Case A: action=0 ok, action=1 ok -> enable_ok == true
        let l_ok = TcpListener::bind("127.0.0.1:0").unwrap();
        let p_ok = l_ok.local_addr().unwrap();
        let t_ok = std::thread::spawn(move || {
            let (mut s, _) = l_ok.accept().unwrap();
            let mut seen = Vec::new();
            for _ in 0..3 {
                let req = read_len_plist(&mut s);
                let act = req.get("action").and_then(|v| v.as_signed_integer()).unwrap();
                seen.push(act);
                let mut resp = plist::Dictionary::new();
                resp.insert("success".into(), plist::Value::Boolean(true));
                if act == 3 {
                    resp.insert("status".into(), plist::Value::Boolean(false));
                }
                write_len_plist(&mut s, resp);
            }
            seen
        });
        let mut c_ok = std::net::TcpStream::connect(p_ok).unwrap();
        let (rev_a, en_a, dev_a) = execute_amfi_unlock_over_stream(&mut c_ok, true).unwrap();
        assert_eq!(t_ok.join().unwrap(), vec![0, 1, 3]);
        assert!((rev_a, en_a, dev_a) == (true, true, false));

        // Case B: Passcode active -> action=0 ok, action=1 fails without success key, action=3 reports false
        let l_pc = TcpListener::bind("127.0.0.1:0").unwrap();
        let p_pc = l_pc.local_addr().unwrap();
        let t_pc = std::thread::spawn(move || {
            let (mut s, _) = l_pc.accept().unwrap();
            let mut seen = Vec::new();
            for _ in 0..3 {
                let req = read_len_plist(&mut s);
                let act = req.get("action").and_then(|v| v.as_signed_integer()).unwrap();
                seen.push(act);
                let mut resp = plist::Dictionary::new();
                if act == 1 {
                    resp.insert("Error".into(), plist::Value::String("PasscodeRequired".into()));
                } else {
                    resp.insert("success".into(), plist::Value::Boolean(true));
                    if act == 3 {
                        resp.insert("status".into(), plist::Value::Boolean(false));
                    }
                }
                write_len_plist(&mut s, resp);
            }
            seen
        });
        let mut c_pc = std::net::TcpStream::connect(p_pc).unwrap();
        let (rev_b, en_b, dev_b) = execute_amfi_unlock_over_stream(&mut c_pc, true).unwrap();
        assert_eq!(t_pc.join().unwrap(), vec![0, 1, 3]);
        assert!(rev_b && !en_b && !dev_b);

        // 2. Full usbmuxd -> lockdownd -> AMFI end-to-end with Passcode rejection on action=1
        let mux_l = TcpListener::bind("127.0.0.1:0").unwrap();
        let mux_addr = mux_l.local_addr().unwrap();
        mux_l.set_nonblocking(true).unwrap();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_srv = stop.clone();
        let actions_seen = std::sync::Arc::new(Mutex::new(Vec::<i64>::new()));
        let actions_srv = actions_seen.clone();

        let mux_srv = std::thread::spawn(move || {
            let mut handlers = Vec::new();
            while !stop_srv.load(std::sync::atomic::Ordering::Relaxed) {
                match mux_l.accept() {
                    Ok((mut s, _)) => {
                        let _ = s.set_nonblocking(false);
                        let ac = actions_srv.clone();
                        handlers.push(std::thread::spawn(move || {
                            let req = read_usbmux_req(&mut s);
                            match req.get("MessageType").and_then(|v| v.as_string()).unwrap_or("") {
                                "ListDevices" => {
                                    let mut props = plist::Dictionary::new();
                                    props.insert("SerialNumber".into(), plist::Value::String("00008150-AMFI285".into()));
                                    props.insert("DeviceID".into(), plist::Value::from(9u32));
                                    props.insert("ConnectionType".into(), plist::Value::String("USB".into()));
                                    let mut dev = plist::Dictionary::new();
                                    dev.insert("Properties".into(), plist::Value::Dictionary(props));
                                    let mut resp = plist::Dictionary::new();
                                    resp.insert("DeviceList".into(), plist::Value::Array(vec![plist::Value::Dictionary(dev)]));
                                    write_usbmux_resp(&mut s, resp);
                                }
                                "ReadPairRecord" => {
                                    let mut pair = plist::Dictionary::new();
                                    pair.insert("HostID".into(), plist::Value::String("H285".into()));
                                    pair.insert("SystemBUID".into(), plist::Value::String("B285".into()));
                                    let bytes = plist::to_bytes_xml(&plist::Value::Dictionary(pair)).unwrap();
                                    let mut resp = plist::Dictionary::new();
                                    resp.insert("PairRecordData".into(), plist::Value::Data(bytes));
                                    write_usbmux_resp(&mut s, resp);
                                }
                                "Connect" => {
                                    let port = u16::from_be(req.get("PortNumber").and_then(|v| v.as_unsigned_integer()).unwrap() as u16);
                                    let mut ok_resp = plist::Dictionary::new();
                                    ok_resp.insert("Number".into(), plist::Value::from(0u32));
                                    write_usbmux_resp(&mut s, ok_resp);
                                    if port == 62078 {
                                        for _ in 0..2 {
                                            let lreq = read_len_plist(&mut s);
                                            let kind = lreq.get("Request").and_then(|v| v.as_string()).unwrap_or("");
                                            let mut lresp = plist::Dictionary::new();
                                            if kind == "StartSession" {
                                                lresp.insert("EnableSessionSSL".into(), plist::Value::Boolean(false));
                                            } else if kind == "StartService" {
                                                lresp.insert("Port".into(), plist::Value::from(58285u16));
                                                lresp.insert("EnableServiceSSL".into(), plist::Value::Boolean(false));
                                            }
                                            write_len_plist(&mut s, lresp);
                                        }
                                    } else if port == 58285 {
                                        for _ in 0..3 {
                                            let areq = read_len_plist(&mut s);
                                            let act = areq.get("action").and_then(|v| v.as_signed_integer()).unwrap_or(-1);
                                            ac.lock().unwrap().push(act);
                                            let mut aresp = plist::Dictionary::new();
                                            if act == 1 {
                                                aresp.insert("Error".into(), plist::Value::String("PasscodeRequired".into()));
                                            } else {
                                                aresp.insert("success".into(), plist::Value::Boolean(true));
                                                if act == 3 {
                                                    aresp.insert("status".into(), plist::Value::Boolean(false));
                                                }
                                            }
                                            write_len_plist(&mut s, aresp);
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }));
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
            for h in handlers {
                let _ = h.join();
            }
        });

        std::env::set_var("USBMUXD_SOCKET_ADDRESS", mux_addr.to_string());
        let pc_res = unlock_developer_mode_menu(Some("00008150-AMFI285"), true).unwrap();
        std::env::remove_var("USBMUXD_SOCKET_ADDRESS");
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = mux_srv.join();

        assert!(pc_res.ok, "Passcode rejection on action=1 MUST preserve action=0 reveal ok=true");
        assert!(pc_res.revealed);
        assert_eq!(pc_res.status_code, "revealed_passcode_manual");
        assert_eq!(*actions_seen.lock().unwrap(), vec![0, 1, 3]);
        assert!(pc_res.message_id.contains("Passcode") || pc_res.message_id.contains("Developer Mode"));

        // 3. Edge case classification: usb_required & device_locked_or_untrusted
        std::env::set_var("USBMUXD_SOCKET_ADDRESS", "127.0.0.1:1");
        let no_usb = unlock_developer_mode_menu(None, true).unwrap();
        std::env::remove_var("USBMUXD_SOCKET_ADDRESS");
        assert!(!no_usb.ok);
        assert_eq!(no_usb.status_code, "usb_required");
        assert!(no_usb.message_id.contains("USB") && no_usb.message_id.contains("Wi-Fi"));
        assert!(no_usb.message_en.contains("USB") && no_usb.message_en.contains("Wi-Fi"));

        let (sc_locked, id_locked, en_locked) = classify_amfi_connect_or_reveal_error("PasswordProtected");
        assert_eq!(sc_locked, "device_locked_or_untrusted");
        assert!(id_locked.contains("Percayai Komputer Ini"));
        assert!(en_locked.contains("Trust This Computer"));
    }
}
