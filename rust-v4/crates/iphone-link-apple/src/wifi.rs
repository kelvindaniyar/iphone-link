//! Native Rust Wi-Fi Discovery, Wireless Lockdown Pairing, `wifi_state.json`, & `RemotePairing` TLS-PSK Tunnel.
//!
//! Replaces Python `handset/wifi.py` + `pymobiledevice3` SSL-PSK bridge with 100% pure Native Rust:
//! - RFC 4122 UUID v3 (`host_identifier`) & SipHash-2-4 `authTag` matching `pymobiledevice3`
//! - Read/write `remote_<udid>.plist` & `wifi_state.json` (100% compatible with `handset/wifi.py`)
//! - Strict local `/24` LAN prioritization & Apple Lockdown `QueryType` / mDNS verification
//! - Direct Unicast DNS-SD query (`< 10 ms`) + Parallel `/24` port `62078` subnet scanner
//! - `RemotePairingClient` + `connect_tls_psk_tunnel_native` + `TCP_NODELAY` & Windows `SIO_TCP_SET_ACK_FREQUENCY`

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use idevice::{
    lockdown::LockdownClient,
    provider::IdeviceProvider as _,
    remote_pairing::{
        connect_tls_psk_tunnel_native, RemotePairingClient, RemotePairingLockdownService,
        RpPairingFile, RpPairingSocket,
    },
    tcp::adapter::Adapter,
    usbmuxd::{Connection, UsbmuxdAddr},
    ReadWrite,
};
use serde_json::{json, Map, Value};
use tracing::{debug, info};

use crate::link::{block_on, usbmux};
use crate::wifi_discovery::{
    build_unicast_mdns_query, parse_srv_port_from_dns, LOCKDOWN_PORT, MDNS_PORT,
    REMOTEPAIRING_SERVICE,
};

pub use idevice::remote_pairing::{compute_auth_tag, compute_auth_tag_bytes, validate_auth_tag};

pub const APPLE_MOBDEV2_SERVICE: &str = "_apple-mobdev2._tcp.local.";

const NAMESPACE_DNS: [u8; 16] = [
    0x6b, 0xa7, 0xb8, 0x10, 0x9d, 0xad, 0x11, 0xd1, 0x80, 0xb4, 0x00, 0xc0, 0x4f, 0xd4, 0x30,
    0xc8,
];

#[cfg(windows)]
#[link(name = "ws2_32")]
extern "system" {
    fn setsockopt(
        s: usize,
        level: i32,
        optname: i32,
        optval: *const u8,
        optlen: i32,
    ) -> i32;
    fn WSAIoctl(
        s: usize,
        dwIoControlCode: u32,
        lpvInBuffer: *const std::ffi::c_void,
        cbInBuffer: u32,
        lpvOutBuffer: *mut std::ffi::c_void,
        cbOutBuffer: u32,
        lpcbBytesReturned: *mut u32,
        lpOverlapped: *mut std::ffi::c_void,
        lpCompletionRoutine: *mut std::ffi::c_void,
    ) -> i32;
}

/// Applies `TCP_NODELAY = true`, `SO_RCVBUF`/`SO_SNDBUF = 4 MB`, `SO_KEEPALIVE` +
/// Windows `SIO_KEEPALIVE_VALS (10s / 2s)`, and `SIO_TCP_SET_ACK_FREQUENCY = 1`.
#[cfg(windows)]
fn apply_win32_tcp_tuning(raw: usize) {
    const SOL_SOCKET: i32 = 0xFFFF;
    const SO_KEEPALIVE: i32 = 0x0008;
    const SO_SNDBUF: i32 = 0x1001;
    const SO_RCVBUF: i32 = 0x1002;
    const SIO_KEEPALIVE_VALS: u32 = 0x9800_0004;
    const SIO_TCP_SET_ACK_FREQUENCY: u32 = 0x9800_0017;

    #[repr(C)]
    struct TcpKeepalive {
        onoff: u32,
        keepalivetime: u32,
        keepaliveinterval: u32,
    }

    let buf_sz: i32 = 4 * 1024 * 1024;
    let keepalive_on: i32 = 1;
    let ka_vals = TcpKeepalive {
        onoff: 1,
        keepalivetime: 10_000,
        keepaliveinterval: 2_000,
    };
    let freq: u32 = 1;
    let mut bytes_returned: u32 = 0;
    let rc = unsafe {
        let _ = setsockopt(raw, SOL_SOCKET, SO_RCVBUF, (&buf_sz as *const i32).cast(), 4);
        let _ = setsockopt(raw, SOL_SOCKET, SO_SNDBUF, (&buf_sz as *const i32).cast(), 4);
        let _ = setsockopt(
            raw,
            SOL_SOCKET,
            SO_KEEPALIVE,
            (&keepalive_on as *const i32).cast(),
            4,
        );
        let _ = WSAIoctl(
            raw,
            SIO_KEEPALIVE_VALS,
            (&ka_vals as *const TcpKeepalive).cast(),
            std::mem::size_of::<TcpKeepalive>() as u32,
            std::ptr::null_mut(),
            0,
            &mut bytes_returned,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        WSAIoctl(
            raw,
            SIO_TCP_SET_ACK_FREQUENCY,
            (&freq as *const u32).cast(),
            std::mem::size_of::<u32>() as u32,
            std::ptr::null_mut(),
            0,
            &mut bytes_returned,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if rc != 0 {
        debug!("SIO_TCP_SET_ACK_FREQUENCY unavailable on socket");
    }
}

/// Applies `TCP_NODELAY = true`, `SO_RCVBUF`/`SO_SNDBUF = 4 MB`, `SO_KEEPALIVE` (`10s / 2s`), and Windows `SIO_TCP_SET_ACK_FREQUENCY = 1`.
#[cfg(windows)]
pub fn configure_low_latency_std_tcp(stream: &TcpStream) {
    use std::os::windows::io::AsRawSocket;
    let _ = stream.set_nodelay(true);
    apply_win32_tcp_tuning(stream.as_raw_socket() as usize);
}

#[cfg(not(windows))]
pub fn configure_low_latency_std_tcp(stream: &TcpStream) {
    let _ = stream.set_nodelay(true);
}

#[cfg(windows)]
pub fn configure_low_latency_tokio_tcp(stream: &tokio::net::TcpStream) {
    use std::os::windows::io::AsRawSocket;
    let _ = stream.set_nodelay(true);
    apply_win32_tcp_tuning(stream.as_raw_socket() as usize);
}

#[cfg(not(windows))]
pub fn configure_low_latency_tokio_tcp(stream: &tokio::net::TcpStream) {
    let _ = stream.set_nodelay(true);
}

fn md5(data: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_le_bytes());

    let (mut a0, mut b0, mut c0, mut d0) = (
        0x67452301u32,
        0xefcdab89u32,
        0x98badcfeu32,
        0x10325476u32,
    );
    for chunk in msg.chunks_exact(64) {
        let mut m = [0u32; 16];
        for i in 0..16 {
            m[i] = u32::from_le_bytes([
                chunk[4 * i],
                chunk[4 * i + 1],
                chunk[4 * i + 2],
                chunk[4 * i + 3],
            ]);
        }
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((b & c) | ((!b) & d), i),
                16..=31 => ((d & b) | ((!d) & c), (5 * i + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | (!d)), (7 * i) % 16),
            };
            let temp = d;
            d = c;
            c = b;
            let sum = a.wrapping_add(f).wrapping_add(K[i]).wrapping_add(m[g]);
            b = b.wrapping_add(sum.rotate_left(S[i]));
            a = temp;
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&a0.to_le_bytes());
    out[4..8].copy_from_slice(&b0.to_le_bytes());
    out[8..12].copy_from_slice(&c0.to_le_bytes());
    out[12..16].copy_from_slice(&d0.to_le_bytes());
    out
}

/// Computes `pymobiledevice3.pair_records.generate_host_id()` (`UUID v3(NAMESPACE_DNS, hostname)`).
pub fn host_identifier(hostname: &str) -> String {
    let mut buf = Vec::with_capacity(16 + hostname.len());
    buf.extend_from_slice(&NAMESPACE_DNS);
    buf.extend_from_slice(hostname.as_bytes());
    let mut digest = md5(&buf);
    digest[6] = (digest[6] & 0x0F) | 0x30;
    digest[8] = (digest[8] & 0x3F) | 0x80;
    format!(
        "{:02X}{:02X}{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
        digest[0],
        digest[1],
        digest[2],
        digest[3],
        digest[4],
        digest[5],
        digest[6],
        digest[7],
        digest[8],
        digest[9],
        digest[10],
        digest[11],
        digest[12],
        digest[13],
        digest[14],
        digest[15],
    )
}

pub fn hostname() -> String {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        extern "system" {
            fn GetComputerNameExW(
                name_type: i32,
                lp_buffer: *mut u16,
                n_size: *mut u32,
            ) -> i32;
        }
        // ComputerNameDnsHostname = 1 matches exact case returned by Python socket.gethostname() / gethostname()
        let mut buf = [0u16; 256];
        let mut len = buf.len() as u32;
        let ok = unsafe { GetComputerNameExW(1, buf.as_mut_ptr(), &mut len) };
        if ok != 0 && len > 0 {
            let s = String::from_utf16_lossy(&buf[..len as usize]).trim().to_string();
            if !s.is_empty() {
                return s;
            }
        }
    }
    if let Ok(v) = std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")) {
        let trimmed = v.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    "iPhoneLinkHost".to_string()
}

pub fn local_hostname() -> String {
    hostname()
}

pub fn auth_tag_matches(alt_irk: &[u8], identifier: &str, auth_tag: &str) -> bool {
    validate_auth_tag(alt_irk, identifier, auth_tag)
}

/// Stored `remote_<udid>.plist` record matching pymobiledevice3 / ZenLink layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub udid: String,
    pub peer_alt_irk: [u8; 16],
    pub private_key: Vec<u8>,
    pub public_key: Vec<u8>,
}

/// Candidate pairing directories (`IPHONE_LINK_PAIRING_DIR`, `~/.pymobiledevice3`, `%LOCALAPPDATA%\iPhoneLink\pairing`).
pub fn record_dirs() -> Vec<PathBuf> {
    if let Ok(override_dir) = std::env::var("IPHONE_LINK_PAIRING_DIR") {
        let trimmed = override_dir.trim();
        if !trimmed.is_empty() {
            return vec![PathBuf::from(trimmed)];
        }
    }
    let mut dirs = Vec::new();
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        dirs.push(PathBuf::from(home).join(".pymobiledevice3"));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(local).join("iPhoneLink").join("pairing"));
    }
    dirs
}

pub fn record_dir() -> Option<PathBuf> {
    record_dirs().into_iter().next()
}

pub fn records(dir: &Path) -> Vec<Record> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let Some(udid) = name
            .strip_prefix("remote_")
            .and_then(|s| s.strip_suffix(".plist"))
        else {
            continue;
        };
        let Ok(val) = plist::from_file(entry.path()) else {
            continue;
        };
        let Some(d) = val.into_dictionary() else {
            continue;
        };
        let Some(irk_bytes) = d
            .get("peer_alt_irk")
            .or_else(|| d.get("alt_irk"))
            .and_then(|v| v.as_data())
        else {
            continue;
        };
        let Ok(peer_alt_irk) = <[u8; 16]>::try_from(irk_bytes) else {
            continue;
        };
        let Some(private_key) = d.get("private_key").and_then(|v| v.as_data()).map(<[u8]>::to_vec)
        else {
            continue;
        };
        let Some(public_key) = d.get("public_key").and_then(|v| v.as_data()).map(<[u8]>::to_vec)
        else {
            continue;
        };
        out.push(Record {
            udid: udid.to_string(),
            peer_alt_irk,
            private_key,
            public_key,
        });
    }
    out
}

pub fn all_records() -> Vec<Record> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for dir in record_dirs() {
        for r in records(&dir) {
            if seen.insert(r.udid.clone()) {
                out.push(r);
            }
        }
    }
    out
}

pub fn find_record(udid: &str) -> Option<Record> {
    all_records().into_iter().find(|r| r.udid == udid)
}

pub fn save_remote_record(
    udid: &str,
    irk: &[u8; 16],
    private_key: &[u8],
    public_key: &[u8],
) -> Result<Vec<PathBuf>, String> {
    let mut d = plist::Dictionary::new();
    d.insert("peer_alt_irk".into(), plist::Value::Data(irk.to_vec()));
    d.insert("private_key".into(), plist::Value::Data(private_key.to_vec()));
    d.insert("public_key".into(), plist::Value::Data(public_key.to_vec()));
    d.insert("remote_unlock_host_key".into(), "".into());
    let val = plist::Value::Dictionary(d);

    let mut written = Vec::new();
    for dir in record_dirs() {
        fs::create_dir_all(&dir).map_err(|e| format!("create pairing dir {}: {e}", dir.display()))?;
        let path = dir.join(format!("remote_{udid}.plist"));
        plist::to_file_xml(&path, &val).map_err(|e| format!("write {}: {e}", path.display()))?;
        written.push(path);
    }
    Ok(written)
}

pub fn rp_file(r: &Record, identifier: &str) -> Result<RpPairingFile, String> {
    let mut d = plist::Dictionary::new();
    d.insert("public_key".into(), plist::Value::Data(r.public_key.clone()));
    d.insert("private_key".into(), plist::Value::Data(r.private_key.clone()));
    d.insert("identifier".into(), plist::Value::String(identifier.to_string()));
    d.insert("alt_irk".into(), plist::Value::Data(r.peer_alt_irk.to_vec()));
    let bytes = plist::to_bytes_xml(&plist::Value::Dictionary(d))
        .map_err(|e| format!("pair record xml: {e}"))?;
    RpPairingFile::from_bytes(&bytes).map_err(|e| format!("pair record parse: {e}"))
}

// ============================================================================
// wifi_state.json Persistence (100% compatible with handset/wifi.py)
// ============================================================================

fn now_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn now_iso_string() -> String {
    now_epoch_secs().to_string()
}

pub fn wifi_state_path() -> PathBuf {
    if let Ok(p) = std::env::var("IPHONE_LINK_WIFI_STATE_PATH") {
        let trimmed = p.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("wifi_state.json");
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        let candidate = cwd.join("wifi_state.json");
        if candidate.is_file() {
            return candidate;
        }
    }
    std::env::var_os("LOCALAPPDATA")
        .map(|b| PathBuf::from(b).join("iPhoneLink").join("wifi_state.json"))
        .unwrap_or_else(|| PathBuf::from("wifi_state.json"))
}

pub fn load_wifi_state_value() -> Value {
    let path = wifi_state_path();
    let Ok(raw) = fs::read_to_string(&path) else {
        return json!({
            "devices": {},
            "enabled_udids": [],
            "last_ip": null,
            "mac_map": {},
            "updated_at": ""
        });
    };
    let Ok(mut val) = serde_json::from_str(&raw) else {
        return json!({
            "devices": {},
            "enabled_udids": [],
            "last_ip": null,
            "mac_map": {},
            "updated_at": ""
        });
    };
    if !val.get("devices").is_some_and(|v| v.as_object().is_some()) {
        if let Some(obj) = val.as_object_mut() {
            obj.insert("devices".into(), Value::Object(Map::new()));
        }
    }
    val
}

pub fn save_wifi_state_value(val: &Value) -> Result<(), String> {
    let path = wifi_state_path();
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            let _ = fs::create_dir_all(parent);
        }
    }
    let text = serde_json::to_string_pretty(val).map_err(|e| e.to_string())?;
    fs::write(&path, text).map_err(|e| format!("write {}: {e}", path.display()))
}

fn sync_top_level_wifi_fields(root: &mut Map<String, Value>, preferred_ip: Option<&str>) {
    let mut enabled_udids = Vec::new();
    let mut mac_map = Map::new();
    let mut latest_ip: Option<String> = preferred_ip
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    if let Some(devs) = root.get("devices").and_then(|v| v.as_object()) {
        for (u, entry) in devs {
            let Some(eobj) = entry.as_object() else {
                continue;
            };
            if eobj
                .get("wifi_enabled_on_device")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                enabled_udids.push(Value::String(u.clone()));
            }
            if let Some(mac) = eobj.get("mac_address").and_then(|v| v.as_str()) {
                if !mac.is_empty() {
                    mac_map.insert(u.clone(), Value::String(mac.to_string()));
                }
            }
            if latest_ip.is_none() {
                if let Some(ip) = eobj.get("last_ip").and_then(|v| v.as_str()) {
                    if !ip.is_empty() {
                        latest_ip = Some(ip.to_string());
                    }
                }
            }
        }
    }
    root.insert("enabled_udids".into(), Value::Array(enabled_udids));
    root.insert(
        "last_ip".into(),
        latest_ip.map(Value::String).unwrap_or(Value::Null),
    );
    root.insert("mac_map".into(), Value::Object(mac_map));
    root.insert("updated_at".into(), Value::String(now_iso_string()));
}

pub fn update_device_wifi_state(
    udid: &str,
    enabled: Option<bool>,
    ip: Option<&str>,
    clear_ip: bool,
    mac: Option<&str>,
    name: Option<&str>,
    last_seen_cable: bool,
    last_seen_wifi: bool,
    last_rp_port: Option<u16>,
) -> Value {
    if udid.is_empty() {
        return Value::Object(Map::new());
    }
    let mut root = load_wifi_state_value();
    let Some(root_obj) = root.as_object_mut() else {
        return Value::Object(Map::new());
    };
    if !root_obj.get("devices").is_some_and(|v| v.as_object().is_some()) {
        root_obj.insert("devices".into(), Value::Object(Map::new()));
    }
    let now = now_iso_string();
    let updated_entry = {
        let devices = root_obj
            .get_mut("devices")
            .and_then(|v| v.as_object_mut())
            .unwrap();
        let entry = devices
            .entry(udid.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if !entry.as_object().is_some() {
            *entry = Value::Object(Map::new());
        }
        let eobj = entry.as_object_mut().unwrap();
        if let Some(en) = enabled {
            eobj.insert("wifi_enabled_on_device".into(), Value::Bool(en));
        }
        if clear_ip {
            eobj.remove("last_ip");
            eobj.remove("lastIp");
        } else if let Some(ip_str) = ip.filter(|s| !s.is_empty()) {
            eobj.insert("last_ip".into(), Value::String(ip_str.to_string()));
            eobj.insert("lastIp".into(), Value::String(ip_str.to_string()));
        }
        if let Some(mac_str) = mac.filter(|s| !s.is_empty()) {
            eobj.insert(
                "mac_address".into(),
                Value::String(mac_str.to_lowercase().replace(':', "-")),
            );
        }
        if let Some(dev_name) = name.filter(|s| !s.is_empty()) {
            eobj.insert("device_name".into(), Value::String(dev_name.to_string()));
            eobj.insert("name".into(), Value::String(dev_name.to_string()));
        }
        let now_u64 = now_epoch_secs();
        if last_seen_cable {
            eobj.insert("last_seen_cable".into(), Value::String(now.clone()));
            eobj.insert("lastSeenAt".into(), Value::from(now_u64));
            eobj.insert("last_seen_at".into(), Value::from(now_u64));
        }
        if last_seen_wifi {
            eobj.insert("last_seen_wifi".into(), Value::String(now.clone()));
            eobj.insert("lastSeenAt".into(), Value::from(now_u64));
            eobj.insert("last_seen_at".into(), Value::from(now_u64));
            eobj.insert("reachable".into(), Value::Bool(true));
            set_cached_wifi_reachability(udid, true, ip, now_u64);
        }
        if let Some(port) = last_rp_port {
            eobj.insert("last_rp_port".into(), Value::from(port));
        }
        eobj.insert("updated_at".into(), Value::String(now));
        entry.clone()
    };
    sync_top_level_wifi_fields(root_obj, ip);
    let _ = save_wifi_state_value(&root);
    updated_entry
}

pub fn get_device_wifi_state(udid: &str) -> Map<String, Value> {
    if udid.is_empty() {
        return Map::new();
    }
    load_wifi_state_value()
        .get("devices")
        .and_then(|d| d.get(udid))
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default()
}

/// Rich per-UDID persisted device identity & Wi-Fi state (Tahap 247).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WifiDeviceMetadata {
    pub udid: String,
    pub name: String,
    pub product_type: String,
    pub model: String,
    pub ios_version: String,
    pub bluetooth_address: String,
    pub last_ip: Option<String>,
    pub last_seen_at: u64,
    pub wifi_enabled: bool,
    pub reachable: Option<bool>,
}

/// In-memory zero-I/O live reachability cache per UDID: `(reachable, ip, last_seen_at, updated_instant)`.
static WIFI_REACHABILITY_CACHE: OnceLock<Mutex<HashMap<String, (bool, Option<String>, u64, Instant)>>> =
    OnceLock::new();
/// Tahap 249 Pilar 3: Per-UDID post-clean-stop grace window (`20 s`) so immediately after
/// stopping an active Wi-Fi mirroring session, Launchpad & status pills keep `reachable: true`
/// while iOS `remoted` tears down the tunnel and re-advertises mDNS.
static WIFI_POST_STOP_GRACE_UNTIL: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();

fn wifi_reachability_cache() -> &'static Mutex<HashMap<String, (bool, Option<String>, u64, Instant)>> {
    WIFI_REACHABILITY_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn wifi_post_stop_grace_map() -> &'static Mutex<HashMap<String, Instant>> {
    WIFI_POST_STOP_GRACE_UNTIL.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn is_in_wifi_post_stop_grace(udid: &str) -> bool {
    if udid.is_empty() {
        return false;
    }
    let Ok(g) = wifi_post_stop_grace_map().lock() else {
        return false;
    };
    g.get(udid).is_some_and(|until| Instant::now() < *until)
}

/// Preserves Wi-Fi online reachability in `WIFI_REACHABILITY_CACHE` and opens a 20-second
/// post-clean-stop grace window so disconnecting a live Wi-Fi mirror session NEVER flashes
/// `iPhone Terputus` for 3–5 seconds while iOS `remoted` resets its mDNS announcement.
pub fn mark_wifi_device_online_after_clean_stop(udid: &str, ip: Option<&str>) {
    if udid.is_empty() {
        return;
    }
    let now_secs = now_epoch_secs();
    let existing_cached_ip = get_cached_wifi_reachability(udid).and_then(|(_, cip, _, _)| cip);
    let disk_ip = read_device_metadata_from_wifi_state(udid).last_ip;
    let effective_ip = ip
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or(existing_cached_ip)
        .or(disk_ip);
    set_cached_wifi_reachability(udid, true, effective_ip.as_deref(), now_secs);
    if let Ok(mut g) = wifi_post_stop_grace_map().lock() {
        g.insert(udid.to_string(), Instant::now() + Duration::from_secs(20));
    }
}

pub fn set_cached_wifi_reachability(udid: &str, reachable: bool, ip: Option<&str>, last_seen_at: u64) {
    if udid.is_empty() {
        return;
    }
    if let Ok(mut g) = wifi_reachability_cache().lock() {
        g.insert(
            udid.to_string(),
            (
                reachable,
                ip.filter(|s| !s.is_empty()).map(str::to_string),
                last_seen_at,
                Instant::now(),
            ),
        );
    }
}

pub fn get_cached_wifi_reachability(udid: &str) -> Option<(bool, Option<String>, u64, Duration)> {
    if udid.is_empty() {
        return None;
    }
    let g = wifi_reachability_cache().lock().ok()?;
    g.get(udid)
        .map(|(r, ip, ts_epoch, inst)| (*r, ip.clone(), *ts_epoch, inst.elapsed()))
}

pub fn clear_cached_wifi_reachability() {
    if let Ok(mut g) = wifi_reachability_cache().lock() {
        g.clear();
    }
    if let Ok(mut g) = wifi_post_stop_grace_map().lock() {
        g.clear();
    }
}

/// Reads structured per-UDID metadata from `wifi_state.json` (supporting both camelCase & snake_case).
pub fn read_device_metadata_from_wifi_state(udid: &str) -> WifiDeviceMetadata {
    let entry = get_device_wifi_state(udid);
    let get_str = |keys: &[&str]| -> String {
        for k in keys {
            if let Some(s) = entry.get(*k).and_then(|v| v.as_str()) {
                let t = s.trim();
                if !t.is_empty() {
                    return t.to_string();
                }
            }
        }
        String::new()
    };
    let name = get_str(&["name", "device_name"]);
    let product_type = get_str(&["productType", "product_type"]);
    let raw_model = get_str(&["model", "marketing_model"]);
    let model = if !product_type.is_empty() {
        crate::models::resolve_apple_model(&product_type)
            .map(str::to_string)
            .unwrap_or_else(|| raw_model.clone())
    } else {
        raw_model
    };
    let ios_version = get_str(&["iosVersion", "product_version"]);
    let bluetooth_address = get_str(&["bluetoothAddress", "bluetooth_address"]).to_ascii_uppercase();
    let last_ip_str = get_str(&["lastIp", "last_ip"]);
    let last_ip = if last_ip_str.is_empty() {
        None
    } else {
        Some(last_ip_str)
    };
    let last_seen_at = entry
        .get("lastSeenAt")
        .or_else(|| entry.get("last_seen_at"))
        .and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok())))
        .or_else(|| {
            entry
                .get("last_seen_wifi")
                .or_else(|| entry.get("last_seen_cable"))
                .or_else(|| entry.get("updated_at"))
                .and_then(|v| v.as_str().and_then(|s| s.parse::<u64>().ok()))
        })
        .unwrap_or(0);
    let wifi_enabled = entry
        .get("wifi_enabled_on_device")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let reachable_disk = entry.get("reachable").and_then(|v| v.as_bool());
    let (reachable, effective_ip, effective_seen) =
        if let Some((r, c_ip, c_seen, age)) = get_cached_wifi_reachability(udid) {
            if age < Duration::from_secs(600) {
                (Some(r), c_ip.or(last_ip), c_seen.max(last_seen_at))
            } else {
                (reachable_disk, last_ip, last_seen_at)
            }
        } else {
            (reachable_disk, last_ip, last_seen_at)
        };

    WifiDeviceMetadata {
        udid: udid.to_string(),
        name,
        product_type,
        model,
        ios_version,
        bluetooth_address,
        last_ip: effective_ip,
        last_seen_at: effective_seen,
        wifi_enabled,
        reachable,
    }
}

/// Persists full per-UDID device metadata (`name`, `productType`, `model`, `iosVersion`,
/// `bluetoothAddress`, `lastIp`, `lastSeenAt`, `reachable`) into `wifi_state.json`.
#[allow(clippy::too_many_arguments)]
pub fn update_device_full_metadata(
    udid: &str,
    name: Option<&str>,
    product_type: Option<&str>,
    model: Option<&str>,
    ios_version: Option<&str>,
    bluetooth_address: Option<&str>,
    last_ip: Option<&str>,
    last_seen_at: Option<u64>,
    reachable: Option<bool>,
) {
    if udid.is_empty() {
        return;
    }
    let clean_name = name.map(str::trim).filter(|s| !s.is_empty());
    let clean_ptype = product_type.map(str::trim).filter(|s| !s.is_empty());
    let derived_model: Option<String> = clean_ptype
        .and_then(|pt| crate::models::resolve_apple_model(pt).map(str::to_string))
        .or_else(|| model.map(str::trim).filter(|s| !s.is_empty()).map(str::to_string));
    let clean_ios = ios_version.map(str::trim).filter(|s| !s.is_empty());
    let clean_bt = bluetooth_address.map(str::trim).filter(|s| !s.is_empty());
    let clean_ip = last_ip.map(str::trim).filter(|s| !s.is_empty());

    if clean_name.is_none()
        && clean_ptype.is_none()
        && derived_model.is_none()
        && clean_ios.is_none()
        && clean_bt.is_none()
        && clean_ip.is_none()
        && last_seen_at.is_none()
        && reachable.is_none()
    {
        return;
    }

    let mut root = load_wifi_state_value();
    let Some(root_obj) = root.as_object_mut() else {
        return;
    };
    if !root_obj.get("devices").is_some_and(|v| v.as_object().is_some()) {
        root_obj.insert("devices".into(), Value::Object(Map::new()));
    }
    let mut changed = false;
    if let Some(devices) = root_obj.get_mut("devices").and_then(|v| v.as_object_mut()) {
        let entry = devices
            .entry(udid.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if entry.as_object().is_none() {
            *entry = Value::Object(Map::new());
        }
        if let Some(eobj) = entry.as_object_mut() {
            if let Some(n) = clean_name {
                let prev1 = eobj.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let prev2 = eobj.get("device_name").and_then(|v| v.as_str()).unwrap_or("");
                if prev1 != n || prev2 != n {
                    eobj.insert("name".into(), Value::String(n.to_string()));
                    eobj.insert("device_name".into(), Value::String(n.to_string()));
                    changed = true;
                }
            }
            if let Some(pt) = clean_ptype {
                let prev1 = eobj.get("productType").and_then(|v| v.as_str()).unwrap_or("");
                let prev2 = eobj.get("product_type").and_then(|v| v.as_str()).unwrap_or("");
                if prev1 != pt || prev2 != pt {
                    eobj.insert("productType".into(), Value::String(pt.to_string()));
                    eobj.insert("product_type".into(), Value::String(pt.to_string()));
                    changed = true;
                }
            }
            if let Some(ref m) = derived_model {
                let prev1 = eobj.get("model").and_then(|v| v.as_str()).unwrap_or("");
                let prev2 = eobj
                    .get("marketing_model")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if prev1 != m.as_str() || prev2 != m.as_str() {
                    eobj.insert("model".into(), Value::String(m.clone()));
                    eobj.insert("marketing_model".into(), Value::String(m.clone()));
                    changed = true;
                }
            }
            if let Some(ios) = clean_ios {
                let prev1 = eobj.get("iosVersion").and_then(|v| v.as_str()).unwrap_or("");
                let prev2 = eobj
                    .get("product_version")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if prev1 != ios || prev2 != ios {
                    eobj.insert("iosVersion".into(), Value::String(ios.to_string()));
                    eobj.insert("product_version".into(), Value::String(ios.to_string()));
                    changed = true;
                }
            }
            if let Some(bt) = clean_bt {
                let norm = bt.to_ascii_uppercase();
                let prev1 = eobj
                    .get("bluetoothAddress")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let prev2 = eobj
                    .get("bluetooth_address")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if prev1 != norm || prev2 != norm {
                    eobj.insert("bluetoothAddress".into(), Value::String(norm.clone()));
                    eobj.insert("bluetooth_address".into(), Value::String(norm));
                    changed = true;
                }
            }
            if let Some(ip_str) = clean_ip {
                let prev1 = eobj.get("lastIp").and_then(|v| v.as_str()).unwrap_or("");
                let prev2 = eobj.get("last_ip").and_then(|v| v.as_str()).unwrap_or("");
                if prev1 != ip_str || prev2 != ip_str {
                    eobj.insert("lastIp".into(), Value::String(ip_str.to_string()));
                    eobj.insert("last_ip".into(), Value::String(ip_str.to_string()));
                    changed = true;
                }
            }
            if let Some(ts) = last_seen_at {
                if ts > 0 {
                    let prev = eobj.get("lastSeenAt").and_then(|v| v.as_u64()).unwrap_or(0);
                    if prev != ts {
                        eobj.insert("lastSeenAt".into(), Value::from(ts));
                        eobj.insert("last_seen_at".into(), Value::from(ts));
                        changed = true;
                    }
                }
            }
            if let Some(r) = reachable {
                let prev = eobj.get("reachable").and_then(|v| v.as_bool());
                if prev != Some(r) {
                    eobj.insert("reachable".into(), Value::Bool(r));
                    changed = true;
                }
                let eff_ts = last_seen_at.unwrap_or_else(|| if r { now_epoch_secs() } else { 0 });
                let eff_ip = clean_ip.or_else(|| eobj.get("last_ip").and_then(|v| v.as_str()));
                set_cached_wifi_reachability(udid, r, eff_ip, eff_ts);
            }
        }
    }
    if changed {
        sync_top_level_wifi_fields(root_obj, clean_ip);
        let _ = save_wifi_state_value(&root);
    }
}

/// Persists non-empty device identity hints (`device_name`, `marketing_model`, `bluetooth_address`)
/// inside `wifi_state.json` for accurate Multi-iPhone Audio PC targeting across USB & Wi-Fi sessions.
pub fn update_device_identity_cache(
    udid: &str,
    device_name: Option<&str>,
    marketing_model: Option<&str>,
    bluetooth_address: Option<&str>,
) {
    update_device_full_metadata(
        udid,
        device_name,
        None,
        marketing_model,
        None,
        bluetooth_address,
        None,
        None,
        None,
    );
}

pub fn has_remote_record(udid: &str) -> bool {
    if udid.is_empty() {
        return false;
    }
    if find_record(udid).is_some() {
        return true;
    }
    record_dirs().into_iter().any(|dir| {
        let p = dir.join(format!("remote_{udid}.plist"));
        p.is_file() && p.metadata().map(|m| m.len() > 0).unwrap_or(false)
    }) || get_device_wifi_state(udid)
        .get("wifi_enabled_on_device")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

fn plist_file_exists_for(udid: &str) -> bool {
    record_dirs().into_iter().any(|dir| {
        let p = dir.join(format!("remote_{udid}.plist"));
        p.is_file() && p.metadata().map(|m| m.len() > 0).unwrap_or(false)
    })
}

pub fn wifi_paired_udids() -> Vec<String> {
    let state = load_wifi_state_value();
    let mut found = HashSet::new();
    for r in all_records() {
        found.insert(r.udid);
    }
    let devices_obj = state.get("devices").and_then(|v| v.as_object());
    if let Some(devs) = devices_obj {
        for (u, entry) in devs {
            if entry
                .get("wifi_enabled_on_device")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                found.insert(u.clone());
            }
        }
    }
    let mut list: Vec<String> = found.into_iter().collect();
    list.sort_by(|a, b| {
        let key = |u: &String| -> (u8, std::cmp::Reverse<u64>, u8, String) {
            let meta = read_device_metadata_from_wifi_state(u);
            let online_prio = match meta.reachable {
                Some(true) => 0,
                None if meta.last_ip.is_some() => 1,
                _ => 2,
            };
            let has_p = plist_file_exists_for(u);
            let has_w = devices_obj
                .and_then(|m| m.get(u))
                .and_then(|e| e.get("wifi_enabled_on_device"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let rec_prio = match (has_p, has_w) {
                (true, true) => 0,
                (true, false) | (false, true) => 1,
                (false, false) => 2,
            };
            (online_prio, std::cmp::Reverse(meta.last_seen_at), rec_prio, u.clone())
        };
        key(a).cmp(&key(b))
    });
    list
}

// ============================================================================
// Multi-Stage Sub-Second LAN Discovery
// ============================================================================

pub fn is_private_lan_ip(ip: &str) -> bool {
    let Ok(v4) = ip.trim().parse::<Ipv4Addr>() else {
        return false;
    };
    let o = v4.octets();
    o[0] == 10 || (o[0] == 192 && o[1] == 168) || (o[0] == 172 && (16..=31).contains(&o[1]))
}

pub fn is_apipa_ip(ip: &str) -> bool {
    let Ok(v4) = ip.trim().parse::<Ipv4Addr>() else {
        return false;
    };
    let o = v4.octets();
    o[0] == 169 && o[1] == 254
}

/// Prioritizes real private LAN IPv4 addresses first, other non-loopback non-APIPA second, and APIPA (`169.254.*`) last.
pub fn sort_local_ipv4_candidates(ips: Vec<String>) -> Vec<String> {
    let mut lan = Vec::new();
    let mut other = Vec::new();
    let mut apipa = Vec::new();
    let mut seen = HashSet::new();

    for raw in ips {
        let trimmed = raw.trim().to_string();
        let Ok(v4) = trimmed.parse::<Ipv4Addr>() else {
            continue;
        };
        if v4.is_loopback() || v4.is_unspecified() || !seen.insert(trimmed.clone()) {
            continue;
        }
        if is_private_lan_ip(&trimmed) {
            lan.push(trimmed);
        } else if is_apipa_ip(&trimmed) {
            apipa.push(trimmed);
        } else {
            other.push(trimmed);
        }
    }
    lan.extend(other);
    lan.extend(apipa);
    lan
}

pub fn local_ipv4_addresses() -> Vec<String> {
    if crate::connection::is_loopback_only_test_mode() {
        return vec!["127.0.0.1".to_string()];
    }
    let mut raw = Vec::new();
    let probe_targets = [
        "8.8.8.8:80",
        "1.1.1.1:80",
        "192.168.1.1:80",
        "192.168.0.1:80",
        "10.0.0.1:80",
        "172.16.0.1:80",
    ];
    for target in probe_targets {
        if let Ok(sock) = UdpSocket::bind("0.0.0.0:0") {
            if sock.connect(target).is_ok() {
                if let Ok(addr) = sock.local_addr() {
                    if let IpAddr::V4(v4) = addr.ip() {
                        raw.push(v4.to_string());
                    }
                }
            }
        }
    }
    let host = hostname();
    if let Ok(addrs) = format!("{host}:0").to_socket_addrs() {
        for sa in addrs {
            if let IpAddr::V4(v4) = sa.ip() {
                raw.push(v4.to_string());
            }
        }
    }
    sort_local_ipv4_candidates(raw)
}

/// Checks whether `ip` matches a host private `/24` LAN subnet while rejecting loopback, APIPA, and foreign subnets.
pub fn is_ip_on_local_subnets(ip: &str) -> bool {
    let Ok(v4) = ip.trim().parse::<Ipv4Addr>() else {
        return false;
    };
    if v4.is_loopback() || v4.is_unspecified() {
        return false;
    }
    let o = v4.octets();
    if o[0] == 169 && o[1] == 254 {
        return false;
    }
    if !is_private_lan_ip(ip) {
        return false;
    }
    for local_ip in local_ipv4_addresses() {
        if !is_private_lan_ip(&local_ip) {
            continue;
        }
        if let Ok(lv4) = local_ip.parse::<Ipv4Addr>() {
            let lo = lv4.octets();
            if lo[0] == o[0] && lo[1] == o[1] && lo[2] == o[2] {
                return true;
            }
        }
    }
    false
}

/// Builds a Unicast DNS-SD PTR query (`QU` bit `0x8001`) for any `.local.` service name.
pub fn build_dnssd_ptr_query(service_name: &str, transaction_id: u16) -> Vec<u8> {
    if service_name == REMOTEPAIRING_SERVICE {
        return build_unicast_mdns_query(transaction_id);
    }
    let mut pkt = Vec::with_capacity(64);
    pkt.extend_from_slice(&transaction_id.to_be_bytes());
    pkt.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    for label in service_name.trim_end_matches('.').split('.') {
        if !label.is_empty() && label.len() < 64 {
            pkt.push(label.len() as u8);
            pkt.extend_from_slice(label.as_bytes());
        }
    }
    pkt.push(0x00);
    pkt.extend_from_slice(&[0x00, 0x0C, 0x80, 0x01]);
    pkt
}

/// Sends direct Unicast DNS-SD PTR queries to `(ip, port)` (`< 10 ms` default or custom timeout)
/// for `_remotepairing._tcp.local.` and `_apple-mobdev2._tcp.local.`.
pub fn query_unicast_dns_sd_to_port(ip: &str, port: u16, timeout_ms: u64) -> Vec<u16> {
    let Ok(parsed_ip) = ip.trim().parse::<IpAddr>() else {
        return Vec::new();
    };
    if crate::connection::is_loopback_only_test_mode() && !parsed_ip.is_loopback() {
        return Vec::new();
    }
    let Ok(sock) = UdpSocket::bind(crate::connection::bind_addr_v4(0)) else {
        return Vec::new();
    };
    let dur = Duration::from_millis(timeout_ms.clamp(5, 1000));
    let _ = sock.set_read_timeout(Some(dur));
    let _ = sock.set_write_timeout(Some(dur));

    let target = SocketAddr::new(parsed_ip, port);
    for (idx, svc) in [REMOTEPAIRING_SERVICE, APPLE_MOBDEV2_SERVICE]
        .iter()
        .enumerate()
    {
        let q = build_dnssd_ptr_query(svc, 0x4150 + idx as u16);
        let _ = sock.send_to(&q, target);
    }

    let mut ports = Vec::new();
    let mut buf = [0u8; 1500];
    let deadline = std::time::Instant::now() + dur;
    while std::time::Instant::now() < deadline {
        match sock.recv_from(&mut buf) {
            Ok((len, _)) => {
                if let Some(p) = parse_srv_port_from_dns(&buf[..len]) {
                    if !ports.contains(&p) {
                        ports.push(p);
                    }
                }
            }
            Err(_) => break,
        }
    }
    ports
}

/// Sends direct Unicast DNS-SD PTR queries to `(ip, 5353)`.
pub fn query_unicast_dns_sd(ip: &str, timeout_ms: u64) -> Vec<u16> {
    query_unicast_dns_sd_to_port(ip, MDNS_PORT, timeout_ms)
}

/// Performs a genuine Apple Lockdown `QueryType` plist handshake on TCP `(addr)` and verifies
/// `Type == "com.apple.mobile.lockdown"` (plus `UniqueDeviceID == expected_udid` if both present).
pub fn verify_lockdown_query_type(
    addr: SocketAddr,
    expected_udid: Option<&str>,
    timeout_ms: u64,
) -> bool {
    let dur = Duration::from_millis(timeout_ms.clamp(40, 2000));
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, dur) else {
        return false;
    };
    configure_low_latency_std_tcp(&stream);
    let _ = stream.set_read_timeout(Some(dur));
    let _ = stream.set_write_timeout(Some(dur));

    let mut req = plist::Dictionary::new();
    req.insert("Label".into(), plist::Value::String("iphone-link".into()));
    req.insert(
        "ProtocolVersion".into(),
        plist::Value::String("2".into()),
    );
    req.insert("Request".into(), plist::Value::String("QueryType".into()));

    let Ok(body) = plist::to_bytes_xml(&plist::Value::Dictionary(req)) else {
        return false;
    };
    let len = body.len() as u32;
    if stream.write_all(&len.to_be_bytes()).is_err() || stream.write_all(&body).is_err() {
        return false;
    }
    let _ = stream.flush();

    let mut hdr = [0u8; 4];
    if stream.read_exact(&mut hdr).is_err() {
        return false;
    }
    let resp_len = u32::from_be_bytes(hdr) as usize;
    if resp_len == 0 || resp_len > 131_072 {
        return false;
    }
    let mut resp_buf = vec![0u8; resp_len];
    if stream.read_exact(&mut resp_buf).is_err() {
        return false;
    }
    let Ok(val) = plist::from_bytes(&resp_buf) else {
        return false;
    };
    let Some(dict) = val.as_dictionary() else {
        return false;
    };
    if dict.get("Type").and_then(|v| v.as_string()) != Some("com.apple.mobile.lockdown") {
        return false;
    }
    if let (Some(exp), Some(got)) = (
        expected_udid.filter(|s| !s.is_empty()),
        dict.get("UniqueDeviceID").and_then(|v| v.as_string()),
    ) {
        return got == exp;
    }
    true
}

/// Strictly verifies whether `ip` on `port` is a genuine Apple iOS device (not an HTTP/TCP sinkhole).
pub fn verify_ip_is_apple_device_with_port(
    ip: &str,
    port: u16,
    expected_udid: Option<&str>,
    timeout_ms: u64,
    allow_loopback_for_test: bool,
) -> bool {
    let trimmed = ip.trim();
    if trimmed.is_empty() {
        return false;
    }
    let Ok(parsed) = trimmed.parse::<IpAddr>() else {
        return false;
    };
    let on_local = is_ip_on_local_subnets(trimmed);
    if !on_local && !(allow_loopback_for_test && parsed.is_loopback()) {
        return false;
    }

    if verify_lockdown_query_type(SocketAddr::new(parsed, port), expected_udid, timeout_ms) {
        return true;
    }

    if port == LOCKDOWN_PORT && !query_unicast_dns_sd(trimmed, timeout_ms.min(60)).is_empty() {
        return true;
    }

    false
}

/// Strict Apple IP guard matching `handset/wifi.py::_verify_ip_is_apple_device`.
pub fn verify_ip_is_apple_device(
    ip: &str,
    expected_udid: Option<&str>,
    timeout_ms: u64,
) -> bool {
    let port = std::env::var("IPHONE_LINK_LOCKDOWN_PORT")
        .ok()
        .and_then(|v| v.trim().parse::<u16>().ok())
        .unwrap_or(LOCKDOWN_PORT);
    let allow_loop = port != LOCKDOWN_PORT;
    verify_ip_is_apple_device_with_port(ip, port, expected_udid, timeout_ms, allow_loop)
}

/// Parallel `/24` TCP port `62078` subnet scanner (`120-180 ms` per host across 64 concurrent workers).
pub fn scan_subnet_port_62078_parallel(
    subnet_prefix: &str,
    exclude_ip: Option<&str>,
    port: u16,
    timeout_ms: u64,
) -> Vec<String> {
    let prefix = subnet_prefix.trim_end_matches('.');
    let mut candidates = Vec::with_capacity(254);
    for host in 1..=254u8 {
        let ip = format!("{prefix}.{host}");
        if exclude_ip.is_some_and(|ex| ex == ip) {
            continue;
        }
        candidates.push(ip);
    }

    let queue = Arc::new(Mutex::new(candidates));
    let found = Arc::new(Mutex::new(Vec::<String>::new()));
    let workers = 64usize;
    let probe_timeout = timeout_ms.clamp(100, 180);

    let mut handles = Vec::with_capacity(workers);
    for _ in 0..workers {
        let q = Arc::clone(&queue);
        let f = Arc::clone(&found);
        handles.push(std::thread::spawn(move || loop {
            let next_ip = q.lock().ok().and_then(|mut g| g.pop());
            let Some(ip) = next_ip else {
                break;
            };
            if verify_ip_is_apple_device_with_port(&ip, port, None, probe_timeout, false) {
                if let Ok(mut out) = f.lock() {
                    out.push(ip);
                }
            }
        }));
    }
    for h in handles {
        let _ = h.join();
    }

    let mut result = found.lock().map(|g| g.clone()).unwrap_or_default();
    result.sort();
    result
}

fn arp_private_lan_ips() -> Vec<String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW_FLAG: u32 = 0x0800_0000;
        let mut cmd = std::process::Command::new("arp");
        cmd.arg("-a").creation_flags(CREATE_NO_WINDOW_FLAG);
        let Ok(out) = cmd.output() else {
            return Vec::new();
        };
        let txt = String::from_utf8_lossy(&out.stdout);
        let mut ips = Vec::new();
        for token in txt.split_whitespace() {
            let clean = token.trim_matches(|c: char| !c.is_ascii_digit() && c != '.');
            if is_private_lan_ip(clean) && is_ip_on_local_subnets(clean) && !ips.contains(&clean.to_string()) {
                ips.push(clean.to_string());
            }
        }
        ips
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

/// Discovers an iPhone LAN IP across usbmux Network -> cached `last_ip` -> ARP -> Unicast DNS-SD -> Parallel `/24` scan.
pub fn discover_iphone_ip_on_lan(udid: Option<&str>, timeout_ms: u64) -> Option<String> {
    // 0. Check usbmuxd Network entries first (< 10ms)
    let mux_ip = block_on(async {
        let Ok(mut mux) = usbmux().await else {
            return None;
        };
        let Ok(devs) = mux.get_devices().await else {
            return None;
        };
        for dev in devs {
            if let Connection::Network(IpAddr::V4(v4)) = dev.connection_type {
                if udid.is_none_or(|u| u.is_empty() || dev.udid == u) {
                    let ip = v4.to_string();
                    if is_ip_on_local_subnets(&ip) {
                        return Some((dev.udid, ip));
                    }
                }
            }
        }
        None
    });
    if let Some((dev_udid, ip)) = mux_ip {
        let target_u = udid.filter(|s| !s.is_empty()).unwrap_or(&dev_udid);
        update_device_wifi_state(target_u, None, Some(&ip), false, None, None, false, true, None);
        return Some(ip);
    }

    // 1. Cached last_ip verification (purges stale/fake IP on failure)
    let target_udid = udid
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| wifi_paired_udids().into_iter().next());
    let mut cached_ip_opt: Option<String> = None;
    if let Some(ref u) = target_udid {
        let st = get_device_wifi_state(u);
        if let Some(last_ip) = st.get("last_ip").and_then(|v| v.as_str()) {
            if !last_ip.is_empty() {
                if verify_ip_is_apple_device(last_ip, Some(u), timeout_ms.min(350)) {
                    update_device_wifi_state(u, None, Some(last_ip), false, None, None, false, true, None);
                    return Some(last_ip.to_string());
                } else {
                    cached_ip_opt = Some(last_ip.to_string());
                    update_device_wifi_state(u, None, None, true, None, None, false, false, None);
                }
            }
        }
    }

    // 2. ARP candidates verified strictly as Apple devices
    let arp_ips = arp_private_lan_ips();
    for ip in &arp_ips {
        if verify_ip_is_apple_device(ip, target_udid.as_deref(), timeout_ms.min(250)) {
            if let Some(ref u) = target_udid {
                update_device_wifi_state(u, None, Some(ip), false, None, None, false, true, None);
            }
            return Some(ip.clone());
        }
    }

    // 3. Direct Unicast DNS-SD wake-up probe (< 10ms)
    for ip in arp_ips.iter().take(6).chain(cached_ip_opt.iter()) {
        if !query_unicast_dns_sd(ip, 10).is_empty()
            && verify_ip_is_apple_device(ip, target_udid.as_deref(), timeout_ms.min(250))
        {
            if let Some(ref u) = target_udid {
                update_device_wifi_state(u, None, Some(ip), false, None, None, false, true, None);
            }
            return Some(ip.clone());
        }
    }

    // 4. Parallel /24 subnet scan on port 62078
    if let Some(primary_lan) = local_ipv4_addresses()
        .into_iter()
        .find(|ip| is_private_lan_ip(ip))
    {
        if let Some(last_dot) = primary_lan.rfind('.') {
            let prefix = &primary_lan[..last_dot];
            let port = std::env::var("IPHONE_LINK_LOCKDOWN_PORT")
                .ok()
                .and_then(|v| v.trim().parse::<u16>().ok())
                .unwrap_or(LOCKDOWN_PORT);
            let found = scan_subnet_port_62078_parallel(
                prefix,
                Some(&primary_lan),
                port,
                timeout_ms.clamp(120, 180),
            );
            if let Some(ip) = found.into_iter().next() {
                if let Some(ref u) = target_udid {
                    update_device_wifi_state(u, None, Some(&ip), false, None, None, false, true, None);
                }
                return Some(ip);
            }
        }
    }

    None
}

// ============================================================================
// High-Level WifiStatusPayload & Probe Results (Parity with handset/wifi.py)
// ============================================================================

#[derive(Debug, Clone)]
pub struct WifiProbeResult {
    pub udid: String,
    pub reachable: bool,
    pub ip: Option<String>,
    pub method: String,
    pub wifi_enabled: bool,
    pub paired: bool,
    pub status_text: String,
    pub status_tone: String,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct WifiStatusPayload {
    pub paired: Vec<String>,
    pub enabled: bool,
    pub reachable: bool,
    pub ip: Option<String>,
    pub method: String,
    pub status_text: String,
    pub status_tone: String,
    pub note: String,
    pub udid: Option<String>,
    pub devices: BTreeMap<String, Value>,
}

impl WifiStatusPayload {
    pub fn to_json_value(&self) -> Value {
        let mut devs_obj = Map::new();
        for (k, v) in &self.devices {
            devs_obj.insert(k.clone(), v.clone());
        }
        // Tahap 247: Compact summary array of paired Wi-Fi devices with real name, model, ip, reachable
        let mut devices_arr = Vec::with_capacity(self.paired.len());
        for u in &self.paired {
            let meta = read_device_metadata_from_wifi_state(u);
            let is_reach = meta.reachable.unwrap_or_else(|| meta.last_ip.is_some());
            let mut item = Map::new();
            item.insert("udid".into(), Value::String(u.clone()));
            item.insert("name".into(), Value::String(meta.name));
            item.insert("model".into(), Value::String(meta.model));
            item.insert("productType".into(), Value::String(meta.product_type));
            item.insert("iosVersion".into(), Value::String(meta.ios_version));
            item.insert("ip".into(), Value::from(meta.last_ip.clone()));
            item.insert("lastIp".into(), Value::from(meta.last_ip));
            item.insert("reachable".into(), Value::Bool(is_reach));
            item.insert("lastSeenAt".into(), Value::from(meta.last_seen_at));
            devices_arr.push(Value::Object(item));
        }
        let paired_arr: Vec<Value> = self.paired.iter().cloned().map(Value::String).collect();
        let mut obj = Map::new();
        obj.insert("paired".into(), Value::Array(paired_arr.clone()));
        obj.insert("pairedUdids".into(), Value::Array(paired_arr));
        obj.insert("enabled".into(), Value::Bool(self.enabled));
        obj.insert("wirelessLockdownEnabled".into(), Value::Bool(self.enabled));
        obj.insert("reachable".into(), Value::Bool(self.reachable));
        obj.insert("ip".into(), Value::from(self.ip.clone()));
        obj.insert("lastIp".into(), Value::from(self.ip.clone()));
        obj.insert("method".into(), Value::String(self.method.clone()));
        obj.insert("statusText".into(), Value::String(self.status_text.clone()));
        obj.insert("stateLabel".into(), Value::String(self.status_text.clone()));
        obj.insert("statusTone".into(), Value::String(self.status_tone.clone()));
        obj.insert("note".into(), Value::String(self.note.clone()));
        obj.insert("udid".into(), Value::from(self.udid.clone()));
        obj.insert("service".into(), Value::String(REMOTEPAIRING_SERVICE.to_string()));
        obj.insert("devices".into(), Value::Array(devices_arr));
        obj.insert("deviceMap".into(), Value::Object(devs_obj));
        Value::Object(obj)
    }

    pub fn to_json_string(&self) -> String {
        self.to_json_value().to_json_string()
    }

    /// Full `/api/wifi` HTTP JSON response with top-level & nested `"wifi"` parity fields.
    pub fn to_api_json_string(&self) -> String {
        let mut root = match self.to_json_value() {
            Value::Object(m) => m,
            _ => Map::new(),
        };
        root.insert("ok".into(), Value::Bool(true));
        root.insert("native".into(), Value::Bool(true));
        root.insert("wifi".into(), self.to_json_value());
        Value::Object(root).to_json_string()
    }
}

/// Lightweight bounded TCP Lockdown `:62078` plaintext metadata query (`<= 350 ms`).
/// Enriches missing `DeviceName`, `ProductType`, `ProductVersion`, `BluetoothAddress` for Wi-Fi devices.
fn enrich_wifi_device_metadata_over_lockdown(ip: &str, udid: &str, timeout_ms: u64) {
    let existing = read_device_metadata_from_wifi_state(udid);
    if !existing.name.is_empty()
        && !existing.product_type.is_empty()
        && !existing.ios_version.is_empty()
    {
        return;
    }
    let Ok(parsed_ip) = ip.trim().parse::<IpAddr>() else {
        return;
    };
    if parsed_ip.is_loopback() {
        return;
    }
    let port = std::env::var("IPHONE_LINK_LOCKDOWN_PORT")
        .ok()
        .and_then(|v| v.trim().parse::<u16>().ok())
        .unwrap_or(LOCKDOWN_PORT);
    let addr = SocketAddr::new(parsed_ip, port);
    let timeout = Duration::from_millis(timeout_ms.clamp(120, 350));
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, timeout) else {
        return;
    };
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));

    let query_key = |s: &mut TcpStream, key: &str| -> Option<String> {
        let mut req = plist::Dictionary::new();
        req.insert("Label".into(), plist::Value::String("iphone-link".into()));
        req.insert("ProtocolVersion".into(), plist::Value::String("2".into()));
        req.insert("Request".into(), plist::Value::String("GetValue".into()));
        req.insert("Key".into(), plist::Value::String(key.into()));
        let body = plist::to_bytes_xml(&plist::Value::Dictionary(req)).ok()?;
        let len = body.len() as u32;
        s.write_all(&len.to_be_bytes()).ok()?;
        s.write_all(&body).ok()?;
        let _ = s.flush();
        let mut hdr = [0u8; 4];
        s.read_exact(&mut hdr).ok()?;
        let rlen = u32::from_be_bytes(hdr) as usize;
        if rlen == 0 || rlen > 65_536 {
            return None;
        }
        let mut rbuf = vec![0u8; rlen];
        s.read_exact(&mut rbuf).ok()?;
        let val = plist::from_bytes(&rbuf).ok()?;
        val.as_dictionary()?
            .get("Value")
            .and_then(|v| v.as_string())
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
    };

    let ptype = query_key(&mut stream, "ProductType")
        .filter(|s| s.starts_with("iPhone") || s.starts_with("iPad"));
    let dname = query_key(&mut stream, "DeviceName");
    let ios_v = query_key(&mut stream, "ProductVersion").filter(|s| s.contains('.'));
    let bt_mac = query_key(&mut stream, "BluetoothAddress").filter(|s| s.contains(':'));
    let model = ptype
        .as_deref()
        .and_then(crate::models::resolve_apple_model)
        .map(str::to_string);

    if ptype.is_some() || dname.is_some() || ios_v.is_some() || bt_mac.is_some() {
        update_device_full_metadata(
            udid,
            dname.as_deref(),
            ptype.as_deref(),
            model.as_deref(),
            ios_v.as_deref(),
            bt_mac.as_deref(),
            Some(ip),
            Some(now_epoch_secs()),
            Some(true),
        );
    }
}

fn probe_single_wifi_device_with_mdns_cache(
    udid: &str,
    shared_mdns: Option<&[crate::rp_wifi_tls::MdnsRpEndpoint]>,
    allow_subnet_scan: bool,
) -> WifiProbeResult {
    let paired_list = wifi_paired_udids();
    let paired = paired_list.iter().any(|u| u == udid);
    let meta = read_device_metadata_from_wifi_state(udid);
    let last_ip = meta.last_ip.clone();
    let rec_opt = find_record(udid);
    let alt_irk = rec_opt.as_ref().map(|r| r.peer_alt_irk);

    // 0. For targeted single-device probe without pre-collected mDNS, fast-path cached last_ip TCP :62078 first
    if shared_mdns.is_none() {
        if let Some(ref ip) = last_ip {
            if verify_ip_is_apple_device(ip, Some(udid), 250) {
                update_device_wifi_state(udid, None, Some(ip), false, None, None, false, true, None);
                enrich_wifi_device_metadata_over_lockdown(ip, udid, 250);
                return WifiProbeResult {
                    udid: udid.to_string(),
                    reachable: true,
                    ip: Some(ip.clone()),
                    method: "tcp_probe".into(),
                    wifi_enabled: true,
                    paired,
                    status_text: format!("✓ Wi-Fi Ready (Connected on LAN: {ip})"),
                    status_tone: "ok".into(),
                    message: format!("iPhone terdeteksi aktif di IP {ip}. Siap mirroring tanpa kabel."),
                };
            }
        }
    }

    // 1. Check fast shared/live mDNS _remotepairing._tcp.local. match (handles DHCP IP changes!)
    let owned_mdns;
    let mdns_slice: &[crate::rp_wifi_tls::MdnsRpEndpoint] = match shared_mdns {
        Some(eps) => eps,
        None => {
            owned_mdns = crate::rp_wifi_tls::discover_remotepairing_mdns(
                last_ip.as_deref(),
                alt_irk.as_ref(),
                450,
            );
            &owned_mdns
        }
    };
    let matched_ep = mdns_slice.iter().find(|ep| {
        if let Some(ref irk) = alt_irk {
            !ep.identifier.is_empty()
                && !ep.auth_tag.is_empty()
                && validate_auth_tag(irk, &ep.identifier, &ep.auth_tag)
        } else {
            false
        }
    });
    if let Some(ep) = matched_ep {
        let found_ip = ep.ip.to_string();
        let host_name_hint = ep
            .host_name
            .trim()
            .trim_end_matches('.')
            .strip_suffix(".local")
            .map(str::trim)
            .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("localhost"));
        let fallback_name = if meta.name.is_empty() {
            host_name_hint
        } else {
            None
        };
        update_device_wifi_state(
            udid,
            Some(true),
            Some(&found_ip),
            false,
            None,
            fallback_name,
            false,
            true,
            Some(ep.port),
        );
        enrich_wifi_device_metadata_over_lockdown(&found_ip, udid, 250);
        return WifiProbeResult {
            udid: udid.to_string(),
            reachable: true,
            ip: Some(found_ip.clone()),
            method: "mdns_remotepairing".into(),
            wifi_enabled: true,
            paired: paired || rec_opt.is_some(),
            status_text: format!("✓ Wi-Fi Ready (RemotePairing LAN: {found_ip}:{})", ep.port),
            status_tone: "ok".into(),
            message: format!(
                "iPhone terdeteksi via _remotepairing._tcp.local. di {found_ip}:{}. Siap mirroring Wi-Fi tanpa kabel.",
                ep.port
            ),
        };
    }

    // 2. Direct TCP Lockdown :62078 probe on cached last_ip (<= 250 ms)
    if let Some(ref ip) = last_ip {
        if verify_ip_is_apple_device(ip, Some(udid), 250) {
            update_device_wifi_state(udid, None, Some(ip), false, None, None, false, true, None);
            enrich_wifi_device_metadata_over_lockdown(ip, udid, 250);
            return WifiProbeResult {
                udid: udid.to_string(),
                reachable: true,
                ip: Some(ip.clone()),
                method: "tcp_probe".into(),
                wifi_enabled: true,
                paired,
                status_text: format!("✓ Wi-Fi Ready (Connected on LAN: {ip})"),
                status_tone: "ok".into(),
                message: format!("iPhone terdeteksi aktif di IP {ip}. Siap mirroring tanpa kabel."),
            };
        }
    }

    // 3. Optional LAN subnet scan fallback (only for single-device targeted scan)
    if allow_subnet_scan {
        if let Some(found_ip) = discover_iphone_ip_on_lan(Some(udid), 160) {
            update_device_wifi_state(udid, None, Some(&found_ip), false, None, None, false, true, None);
            enrich_wifi_device_metadata_over_lockdown(&found_ip, udid, 250);
            return WifiProbeResult {
                udid: udid.to_string(),
                reachable: true,
                ip: Some(found_ip.clone()),
                method: "subnet_scan".into(),
                wifi_enabled: true,
                paired,
                status_text: format!("✓ Wi-Fi Ready (Connected on LAN: {found_ip})"),
                status_tone: "ok".into(),
                message: format!("iPhone terdeteksi aktif di IP {found_ip}. Siap mirroring tanpa kabel."),
            };
        }
    }

    if paired || rec_opt.is_some() {
        // Tahap 249 Pilar 3: During the 20s post-clean-stop grace window after stopping an active
        // Wi-Fi mirroring session, keep the device online instead of downgrading to offline while
        // iOS `remoted` finishes closing the tunnel and re-advertises mDNS.
        if is_in_wifi_post_stop_grace(udid) {
            let grace_ip = get_cached_wifi_reachability(udid)
                .and_then(|(_, cip, _, _)| cip)
                .or(last_ip);
            set_cached_wifi_reachability(udid, true, grace_ip.as_deref(), now_epoch_secs());
            let ok_label = match grace_ip {
                Some(ref ip) => format!("✓ Wi-Fi Ready (Connected on LAN: {ip})"),
                None => "✓ Wi-Fi Ready".to_string(),
            };
            return WifiProbeResult {
                udid: udid.to_string(),
                reachable: true,
                ip: grace_ip,
                method: "post_stop_grace".into(),
                wifi_enabled: true,
                paired: true,
                status_text: ok_label,
                status_tone: "ok".into(),
                message: "iPhone siap di jaringan Wi-Fi.".into(),
            };
        }
        update_device_full_metadata(udid, None, None, None, None, None, None, None, Some(false));
        let offline_label = match last_ip {
            Some(ref ip) => format!("⚠ Wi-Fi Paired (Offline - IP terakhir: {ip})"),
            None => "⚠ Wi-Fi Paired (Offline)".to_string(),
        };
        return WifiProbeResult {
            udid: udid.to_string(),
            reachable: false,
            ip: last_ip,
            method: "none".into(),
            wifi_enabled: true,
            paired: true,
            status_text: offline_label,
            status_tone: "warn".into(),
            message: "Perangkat ter-pairing untuk Wi-Fi tetapi tidak terdeteksi di LAN saat ini. Pastikan iPhone menyala, Wi-Fi aktif, dan terhubung ke jaringan yang sama.".into(),
        };
    }

    WifiProbeResult {
        udid: udid.to_string(),
        reachable: false,
        ip: None,
        method: "none".into(),
        wifi_enabled: false,
        paired: false,
        status_text: "Wi-Fi Not Configured".into(),
        status_tone: "muted".into(),
        message: "Wi-Fi belum dikonfigurasi. Hubungkan iPhone via kabel lalu klik 'Set up Wi-Fi'.".into(),
    }
}

pub fn probe_wifi_device(udid: &str) -> WifiProbeResult {
    probe_single_wifi_device_with_mdns_cache(udid, None, true)
}

/// Tahap 247 Pilar 1: Probes ALL paired Wi-Fi devices in parallel (bounded <= 800 ms total wall time).
/// One sleeping/offline device NEVER blocks discovering another online device on the same LAN.
pub fn probe_all_paired_wifi_devices_parallel(timeout_ms: u64) -> Vec<WifiProbeResult> {
    let paired = wifi_paired_udids();
    if paired.is_empty() {
        return Vec::new();
    }
    let mdns_budget = timeout_ms.clamp(220, 450);
    let shared_mdns = Arc::new(crate::rp_wifi_tls::discover_remotepairing_mdns(
        None,
        None,
        mdns_budget,
    ));

    let mut handles = Vec::with_capacity(paired.len());
    for udid in paired {
        let mdns_ref = Arc::clone(&shared_mdns);
        handles.push(std::thread::spawn(move || {
            probe_single_wifi_device_with_mdns_cache(&udid, Some(&mdns_ref), false)
        }));
    }

    let mut results = Vec::with_capacity(handles.len());
    for h in handles {
        if let Ok(res) = h.join() {
            results.push(res);
        }
    }
    // Sort online results first, then by udid
    results.sort_by(|a, b| b.reachable.cmp(&a.reachable).then_with(|| a.udid.cmp(&b.udid)));
    results
}

/// Consolidated Wi-Fi status.
/// Zero-polling invariant: when `probe_network == false`, executes ZERO network I/O (`< 0.2 ms`).
pub fn get_wifi_status(selected_udid: Option<&str>, probe_network: bool) -> WifiStatusPayload {
    let paired_list = wifi_paired_udids();
    let state_val = load_wifi_state_value();
    let mut devices_map = BTreeMap::new();
    if let Some(devs) = state_val.get("devices").and_then(|v| v.as_object()) {
        for (k, v) in devs {
            devices_map.insert(k.clone(), v.clone());
        }
    }

    let target = selected_udid
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| paired_list.first().cloned());

    let Some(target_udid) = target else {
        return WifiStatusPayload {
            paired: Vec::new(),
            enabled: false,
            reachable: false,
            ip: None,
            method: "none".into(),
            status_text: "Wi‑Fi not set up".into(),
            status_tone: String::new(),
            note: "Plug in the cable once to set up Wi‑Fi.".into(),
            udid: None,
            devices: devices_map,
        };
    };

    if !probe_network {
        let meta = read_device_metadata_from_wifi_state(&target_udid);
        let last_ip = meta.last_ip.clone();
        let is_paired = paired_list.iter().any(|u| u == &target_udid) || has_remote_record(&target_udid);
        if is_paired {
            let is_reachable = meta.reachable.unwrap_or_else(|| last_ip.is_some());
            if !is_reachable {
                let offline_label = match last_ip {
                    Some(ref ip) => format!("⚠ Wi-Fi Paired (Offline - IP terakhir: {ip})"),
                    None => "⚠ Wi-Fi Paired (Offline)".to_string(),
                };
                return WifiStatusPayload {
                    paired: paired_list,
                    enabled: true,
                    reachable: false,
                    ip: last_ip,
                    method: "cached".into(),
                    status_text: offline_label,
                    status_tone: "warn".into(),
                    note: "Perangkat ter-pairing untuk Wi-Fi tetapi saat ini offline/terkunci.".into(),
                    udid: Some(target_udid),
                    devices: devices_map,
                };
            }
            let ip_note = last_ip
                .as_ref()
                .map(|ip| format!(" (IP: {ip})"))
                .unwrap_or_default();
            let status_text = if last_ip.is_some() {
                "✓ Wi‑Fi siap (Terhubung di LAN)".to_string()
            } else {
                "✓ Wi‑Fi siap".to_string()
            };
            return WifiStatusPayload {
                paired: paired_list,
                enabled: true,
                reachable: true,
                ip: last_ip,
                method: "cached".into(),
                status_text,
                status_tone: "ok".into(),
                note: format!("Mirroring CoreDevice via Wi‑Fi tanpa kabel siap{ip_note}."),
                udid: Some(target_udid),
                devices: devices_map,
            };
        }
        return WifiStatusPayload {
            paired: paired_list,
            enabled: false,
            reachable: false,
            ip: last_ip,
            method: "cached".into(),
            status_text: "🔌 Colok kabel USB 1x untuk aktifkan mode Wi‑Fi".into(),
            status_tone: "bad".into(),
            note: "iPhone belum pernah di-setup untuk Wi-Fi. Colokkan kabel USB dan klik 'Set up Wi-Fi'.".into(),
            udid: Some(target_udid),
            devices: devices_map,
        };
    }

    let probe = if paired_list.len() >= 2 {
        let all_probes = probe_all_paired_wifi_devices_parallel(550);
        all_probes
            .iter()
            .find(|p| p.udid.eq_ignore_ascii_case(&target_udid))
            .cloned()
            .unwrap_or_else(|| probe_wifi_device(&target_udid))
    } else {
        probe_wifi_device(&target_udid)
    };
    // Reload state map in case probe discovered/updated last_ip
    let updated_state = load_wifi_state_value();
    devices_map.clear();
    if let Some(devs) = updated_state.get("devices").and_then(|v| v.as_object()) {
        for (k, v) in devs {
            devices_map.insert(k.clone(), v.clone());
        }
    }
    WifiStatusPayload {
        paired: wifi_paired_udids(),
        enabled: probe.wifi_enabled,
        reachable: probe.reachable,
        ip: probe.ip,
        method: probe.method,
        status_text: probe.status_text,
        status_tone: probe.status_tone,
        note: probe.message,
        udid: Some(target_udid),
        devices: devices_map,
    }
}

// ============================================================================
// 1-Time USB Setup (EnableWifiConnections + EnableWifiDebugging + RemotePairing PIN 000000)
// ============================================================================

#[derive(Debug, Clone)]
pub struct WifiPairResult {
    pub ok: bool,
    pub udid: String,
    pub paired: Vec<String>,
    pub wifi_enabled: bool,
    pub wireless_lockdown_enabled: bool,
    pub ip: Option<String>,
    pub mac: Option<String>,
    pub name: Option<String>,
    pub message: String,
    pub message_id: String,
    pub message_en: String,
}

impl WifiPairResult {
    pub fn to_json_value(&self) -> Value {
        let paired_arr: Vec<Value> = self.paired.iter().cloned().map(Value::String).collect();
        let mut obj = Map::new();
        obj.insert("ok".into(), Value::Bool(self.ok));
        obj.insert("native".into(), Value::Bool(true));
        obj.insert("udid".into(), Value::String(self.udid.clone()));
        obj.insert("paired".into(), Value::Array(paired_arr));
        obj.insert("wifi_enabled".into(), Value::Bool(self.wifi_enabled));
        obj.insert(
            "wirelessLockdownEnabled".into(),
            Value::Bool(self.wireless_lockdown_enabled),
        );
        obj.insert("ip".into(), Value::from(self.ip.clone()));
        obj.insert("mac".into(), Value::from(self.mac.clone()));
        obj.insert("name".into(), Value::from(self.name.clone()));
        obj.insert("message".into(), Value::String(self.message.clone()));
        obj.insert("message_id".into(), Value::String(self.message_id.clone()));
        obj.insert("message_en".into(), Value::String(self.message_en.clone()));
        Value::Object(obj)
    }
}

pub fn prepare_wifi_over_usb(udid: Option<&str>) -> Result<WifiPairResult, String> {
    let clean_req = udid
        .map(crate::physical_stream::sanitize_udid_label)
        .filter(|s| !s.is_empty());
    let (target_udid, mac, name, product_type, ios_version, bt_mac, wireless_ok) = block_on(async {
        let mut mux = usbmux().await?;
        let devs = mux
            .get_devices()
            .await
            .map_err(|e| format!("usbmuxd list: {e}"))?;
        let usb_devs: Vec<_> = devs
            .into_iter()
            .filter(|d| d.connection_type == Connection::Usb && !d.udid.is_empty())
            .collect();
        let usb_dev = usb_devs
            .iter()
            .find(|d| {
                clean_req
                    .as_deref()
                    .is_none_or(|req| d.udid.eq_ignore_ascii_case(req))
            })
            .or_else(|| usb_devs.first())
            .cloned()
            .ok_or_else(|| "Plug in the iPhone first.".to_string())?;

        let addr = UsbmuxdAddr::from_env_var().map_err(|e| format!("usbmuxd addr: {e}"))?;
        let provider = usb_dev.to_provider(addr, "iphone-link");
        let mut lockdown = LockdownClient::connect(&provider)
            .await
            .map_err(|e| format!("lockdown connect: {e}"))?;
        let pairing = provider
            .get_pairing_file()
            .await
            .map_err(|e| format!("pair record (Trust this computer): {e}"))?;
        lockdown
            .start_session(&pairing)
            .await
            .map_err(|e| format!("lockdown session: {e}"))?;

        lockdown
            .set_value(
                "EnableWifiConnections",
                true.into(),
                Some("com.apple.mobile.wireless_lockdown"),
            )
            .await
            .map_err(|e| format!("EnableWifiConnections: {e}"))?;
        let _ = lockdown
            .set_value(
                "EnableWifiDebugging",
                true.into(),
                Some("com.apple.mobile.wireless_lockdown"),
            )
            .await;

        let dev_name = lockdown
            .get_value(Some("DeviceName"), None)
            .await
            .ok()
            .and_then(|v| v.as_string().map(str::to_owned));
        let wifi_mac = lockdown
            .get_value(Some("WiFiAddress"), None)
            .await
            .ok()
            .and_then(|v| v.as_string().map(str::to_owned));
        let dev_ptype = lockdown
            .get_value(Some("ProductType"), None)
            .await
            .ok()
            .and_then(|v| v.as_string().map(str::to_owned))
            .filter(|s| s.starts_with("iPhone") || s.starts_with("iPad"));
        let dev_ios = lockdown
            .get_value(Some("ProductVersion"), None)
            .await
            .ok()
            .and_then(|v| v.as_string().map(str::to_owned))
            .filter(|s| s.contains('.'));
        let dev_bt = lockdown
            .get_value(Some("BluetoothAddress"), None)
            .await
            .ok()
            .and_then(|v| v.as_string().map(str::to_owned))
            .filter(|s| s.contains(':'));
        let resolved_udid = lockdown
            .get_value(Some("UniqueDeviceID"), None)
            .await
            .ok()
            .and_then(|v| v.as_string().map(str::to_owned))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| usb_dev.udid.clone());

        let host = hostname();
        let host_id = host_identifier(&host);
        let old = find_record(&resolved_udid);
        let mut rp_pf = match &old {
            Some(r) => rp_file(r, &host_id)?,
            None => {
                let mut f = RpPairingFile::generate(&host);
                f.identifier = host_id;
                f
            }
        };

        let rp_res = async {
            let service = RemotePairingLockdownService::connect(&provider)
                .await
                .map_err(|e| format!("{e}"))?;
            let mut client = service.into_client(&host).map_err(|e| format!("{e}"))?;
            client
                .connect(&mut rp_pf, || async { "000000".to_string() })
                .await
                .map_err(|e| format!("{e}"))
        }
        .await;

        match rp_res {
            Ok(()) => {
                if !old
                    .as_ref()
                    .is_some_and(|r| r.public_key == rp_pf.public_key_bytes() && r.peer_alt_irk == *rp_pf.alt_irk().unwrap_or(&r.peer_alt_irk))
                {
                    if let Some(&irk) = rp_pf.alt_irk() {
                        let _ = save_remote_record(
                            &resolved_udid,
                            &irk,
                            &rp_pf.private_key_bytes(),
                            &rp_pf.public_key_bytes(),
                        );
                    }
                }
            }
            Err(e) => {
                info!("wifi: RemotePairing service fallback notice: {e}");
            }
        }

        Ok::<_, String>((
            resolved_udid,
            wifi_mac,
            dev_name,
            dev_ptype,
            dev_ios,
            dev_bt,
            true,
        ))
    })?;

    let cached_ip = get_device_wifi_state(&target_udid)
        .get("last_ip")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .filter(|ip| verify_ip_is_apple_device(ip, Some(&target_udid), 250));

    let discovered_ip = cached_ip.or_else(|| {
        arp_private_lan_ips()
            .into_iter()
            .find(|ip| verify_ip_is_apple_device(ip, Some(&target_udid), 180))
    });

    update_device_wifi_state(
        &target_udid,
        Some(true),
        discovered_ip.as_deref(),
        false,
        mac.as_deref(),
        name.as_deref(),
        true,
        discovered_ip.is_some(),
        None,
    );
    let resolved_model = product_type
        .as_deref()
        .and_then(crate::models::resolve_apple_model);
    update_device_full_metadata(
        &target_udid,
        name.as_deref(),
        product_type.as_deref(),
        resolved_model,
        ios_version.as_deref(),
        bt_mac.as_deref(),
        discovered_ip.as_deref(),
        Some(now_epoch_secs()),
        if discovered_ip.is_some() { Some(true) } else { None },
    );

    let paired_list = wifi_paired_udids();
    let msg_id = match discovered_ip {
        Some(ref ip) => format!("✓ Wi-Fi siap (Terhubung di LAN: {ip}). Sekarang bisa tanpa kabel."),
        None => "✓ Wi-Fi berhasil dikonfigurasi di iPhone. Klik Scan saat kabel dilepas untuk menemukan IP LAN.".to_string(),
    };
    let msg_en = match discovered_ip {
        Some(ref ip) => format!("✓ Wi-Fi ready (Connected on LAN: {ip}). You can unplug the cable now."),
        None => "✓ Wi-Fi pairing configured on iPhone. Click Scan after unplugging to resolve LAN IP.".to_string(),
    };

    Ok(WifiPairResult {
        ok: true,
        udid: target_udid,
        paired: paired_list,
        wifi_enabled: true,
        wireless_lockdown_enabled: wireless_ok,
        ip: discovered_ip.clone(),
        mac,
        name,
        message: discovered_ip
            .as_ref()
            .map(|ip| format!("✓ Wi-Fi Ready (Connected on LAN: {ip})"))
            .unwrap_or_else(|| "✓ Wi-Fi Paired".to_string()),
        message_id: msg_id,
        message_en: msg_en,
    })
}

/// Runs active parallel multi-device LAN discovery (`probe_all_paired_wifi_devices_parallel` + fallback)
/// and returns `/api/wifi/scan` JSON.
pub fn scan_wifi_json(udid: Option<&str>, timeout_ms: u64) -> String {
    let explicit_udid = udid
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let paired_all = wifi_paired_udids();
    let (target_hint, probe_res, found_ip) = if paired_all.len() >= 2 && explicit_udid.is_none() {
        let all_probes = probe_all_paired_wifi_devices_parallel(timeout_ms.clamp(250, 700));
        let best = all_probes
            .iter()
            .find(|p| p.reachable)
            .or_else(|| all_probes.first())
            .cloned();
        let ip = best.as_ref().filter(|p| p.reachable).and_then(|p| p.ip.clone());
        let tgt = best.as_ref().map(|p| p.udid.clone());
        (tgt, best, ip)
    } else if paired_all.len() >= 2 {
        let all_probes = probe_all_paired_wifi_devices_parallel(timeout_ms.clamp(250, 700));
        let chosen = explicit_udid
            .as_deref()
            .and_then(|req| all_probes.iter().find(|p| p.udid.eq_ignore_ascii_case(req)))
            .or_else(|| all_probes.iter().find(|p| p.reachable))
            .or_else(|| all_probes.first())
            .cloned();
        let ip = chosen.as_ref().filter(|p| p.reachable).and_then(|p| p.ip.clone());
        let tgt = chosen.as_ref().map(|p| p.udid.clone()).or(explicit_udid);
        (tgt, chosen, ip)
    } else {
        let tgt = explicit_udid
            .or_else(|| paired_all.into_iter().next())
            .or_else(|| all_records().into_iter().next().map(|r| r.udid));
        let p = tgt.as_deref().map(probe_wifi_device);
        let ip = p
            .as_ref()
            .filter(|x| x.reachable)
            .and_then(|x| x.ip.clone())
            .or_else(|| discover_iphone_ip_on_lan(tgt.as_deref(), timeout_ms));
        (tgt, p, ip)
    };
    let wifi_st = get_wifi_status(target_hint.as_deref(), false);
    let resolved_ip = found_ip.clone().or_else(|| wifi_st.ip.clone());
    let reachable = found_ip.is_some()
        || probe_res.as_ref().map(|p| p.reachable).unwrap_or(false)
        || wifi_st.reachable;
    let found = resolved_ip.is_some() && reachable;
    let target_udid = target_hint.or_else(|| wifi_st.udid.clone());

    let (msg_id, msg_en) = if let Some(ref ip) = resolved_ip {
        if reachable {
            (
                format!("iPhone ditemukan di IP LAN {ip}. Siap mirroring via Wi-Fi."),
                format!("iPhone found at LAN IP {ip}. Ready for Wi-Fi mirroring."),
            )
        } else {
            (
                "iPhone belum merespons di LAN. Pastikan Wi-Fi aktif dan iPhone dalam keadaan terbuka.".to_string(),
                "iPhone did not respond on LAN yet. Ensure Wi-Fi is active and iPhone is unlocked.".to_string(),
            )
        }
    } else {
        (
            "iPhone belum ditemukan di jaringan lokal. Pastikan PC & iPhone berada di Wi-Fi yang sama.".to_string(),
            "iPhone not found on local network yet. Ensure PC & iPhone are on the same Wi-Fi.".to_string(),
        )
    };

    let mut obj = Map::new();
    obj.insert("ok".into(), Value::Bool(true));
    obj.insert("native".into(), Value::Bool(true));
    obj.insert("found".into(), Value::Bool(found));
    obj.insert("ip".into(), Value::from(resolved_ip));
    obj.insert("udid".into(), Value::from(target_udid));
    obj.insert("reachable".into(), Value::Bool(reachable));
    obj.insert("wifi".into(), wifi_st.to_json_value());
    obj.insert("message_id".into(), Value::String(msg_id));
    obj.insert("message_en".into(), Value::String(msg_en));
    Value::Object(obj).to_json_string()
}

/// Runs 1-time USB Wi-Fi Lockdown & RemotePairing setup and returns `/api/wifi/pair` JSON.
pub fn pair_wifi_json(udid: Option<&str>) -> String {
    match prepare_wifi_over_usb(udid) {
        Ok(res) => {
            let wifi_st = get_wifi_status(Some(&res.udid), false);
            let mut obj = match res.to_json_value() {
                Value::Object(m) => m,
                _ => Map::new(),
            };
            obj.insert("wifi".into(), wifi_st.to_json_value());
            Value::Object(obj).to_json_string()
        }
        Err(err) => {
            let wifi_st = get_wifi_status(udid, false);
            let mut obj = Map::new();
            obj.insert("ok".into(), Value::Bool(false));
            obj.insert("native".into(), Value::Bool(true));
            obj.insert("udid".into(), Value::from(udid.map(str::to_string)));
            obj.insert("wifi_enabled".into(), Value::Bool(false));
            obj.insert("error".into(), Value::String(err));
            obj.insert("wifi".into(), wifi_st.to_json_value());
            obj.insert(
                "message_id".into(),
                Value::String("Hubungkan iPhone menggunakan kabel USB terlebih dahulu, buka kunci layar, dan ketuk Trust This Computer.".into()),
            );
            obj.insert(
                "message_en".into(),
                Value::String("Plug in the iPhone first via USB cable, unlock the screen, and tap Trust This Computer.".into()),
            );
            Value::Object(obj).to_json_string()
        }
    }
}

// ============================================================================
// Wireless Tunnel (`RemotePairingClient` -> `TLS-PSK` -> `Adapter` -> `RsdHandshake`)
// ============================================================================

pub struct Tunnel {
    pub adapter: Adapter,
    pub rsd_port: u16,
    pub control: RemotePairingClient<Box<dyn ReadWrite>>,
}

/// Raw unlocked CoreDevice IPv6 tunnel stream + control channel returned over Wi-Fi `RemotePairing`.
pub struct RawWifiCdTunnel {
    pub stream: Box<dyn ReadWrite>,
    pub client_ip: std::net::Ipv6Addr,
    pub server_ip: std::net::Ipv6Addr,
    pub rsd_port: u16,
    pub control: RemotePairingClient<Box<dyn ReadWrite>>,
    pub mtu: usize,
    pub loopback_target: Option<SocketAddr>,
}

async fn connect_raw_inner(addr: SocketAddr, record: &Record) -> Result<RawWifiCdTunnel, String> {
    let host = hostname();
    let mut file = rp_file(record, &host_identifier(&host))?;
    let control_stream = tokio::time::timeout(
        Duration::from_secs(4),
        tokio::net::TcpStream::connect(addr),
    )
    .await
    .map_err(|_| format!("connect {addr}: timeout"))?
    .map_err(|e| format!("connect {addr}: {e}"))?;
    configure_low_latency_tokio_tcp(&control_stream);
    let boxed_ctrl: Box<dyn ReadWrite> = Box::new(control_stream);
    let mut control = RemotePairingClient::new(RpPairingSocket::new(boxed_ctrl), &host);
    control
        .attempt_pair_verify()
        .await
        .map_err(|e| format!("pair-verify start: {e}"))?;
    control
        .validate_pairing(&mut file)
        .await
        .map_err(|e| format!("pair-verify (pair over USB again?): {e}"))?;
    let data_port = control
        .create_tcp_listener()
        .await
        .map_err(|e| format!("tunnel listener: {e}"))?;

    let data_addr = SocketAddr::new(addr.ip(), data_port);
    let data = tokio::time::timeout(
        Duration::from_secs(4),
        tokio::net::TcpStream::connect(data_addr),
    )
    .await
    .map_err(|_| format!("connect tunnel port {data_port}: timeout"))?
    .map_err(|e| format!("connect tunnel port {data_port}: {e}"))?;
    configure_low_latency_tokio_tcp(&data);

    let tunnel = connect_tls_psk_tunnel_native(data, control.encryption_key())
        .await
        .map_err(|e| format!("TLS-PSK tunnel: {e}"))?;
    let info = tunnel.info.clone();
    let ours: IpAddr = info
        .client_address
        .parse()
        .map_err(|e| format!("tunnel client address: {e}"))?;
    let theirs: IpAddr = info
        .server_address
        .parse()
        .map_err(|e| format!("tunnel server address: {e}"))?;
    let to_v6 = |ip: IpAddr| match ip {
        IpAddr::V6(v6) => v6,
        IpAddr::V4(v4) => v4.to_ipv6_mapped(),
    };
    let loopback_target = info.rsd_loopback_port.map(|p| SocketAddr::new(addr.ip(), p));
    Ok(RawWifiCdTunnel {
        stream: tunnel.into_inner(),
        client_ip: to_v6(ours),
        server_ip: to_v6(theirs),
        rsd_port: info.server_rsd_port,
        control,
        mtu: info.mtu as usize,
        loopback_target,
    })
}

/// Pair-verifies using a stored `remote_<udid>.plist`, opens the `TLS-PSK` data tunnel, and wraps into `Adapter`.
pub async fn connect(addr: SocketAddr, record: &Record) -> Result<Tunnel, String> {
    let raw = connect_raw_inner(addr, record).await?;
    let mut adapter = Adapter::new(
        raw.stream,
        IpAddr::V6(raw.client_ip),
        IpAddr::V6(raw.server_ip),
    )
    .with_loopback_target(raw.loopback_target);
    adapter.set_mss(raw.mtu.saturating_sub(60));
    Ok(Tunnel {
        adapter,
        rsd_port: raw.rsd_port,
        control: raw.control,
    })
}

/// Resolves stored `remote_<udid>.plist` + live/cached `_remotepairing._tcp.local.` port (`49152` fallback)
/// and opens the raw IPv6 `TLS-PSK` CoreDevice tunnel stream over Wi-Fi.
pub async fn connect_raw_cd_tunnel(udid: &str, ip: &str) -> Result<RawWifiCdTunnel, String> {
    let parsed_ip: IpAddr = ip
        .trim()
        .parse()
        .map_err(|e| format!("Alamat IP Wi-Fi tidak valid ({ip}): {e}"))?;
    let record = find_record(udid)
        .or_else(|| all_records().into_iter().next())
        .ok_or_else(|| {
            "Record RemotePairing Wi-Fi (remote_<udid>.plist) belum ditemukan; colokkan USB sekali untuk pairing Wi-Fi.".to_string()
        })?;

    let mut ports = Vec::new();
    for p in query_unicast_dns_sd(ip, 150) {
        if p > 0 && !ports.contains(&p) {
            ports.push(p);
        }
    }
    if let Some(cached_port) = get_device_wifi_state(&record.udid)
        .get("last_rp_port")
        .and_then(|v| v.as_u64())
        .map(|p| p as u16)
    {
        if cached_port > 0 && !ports.contains(&cached_port) {
            ports.push(cached_port);
        }
    }
    if !ports.contains(&49152) {
        ports.push(49152);
    }

    let mut last_err = String::from("RemotePairing Wi-Fi port tidak merespons.");
    for port in ports {
        let addr = SocketAddr::new(parsed_ip, port);
        match connect_raw_inner(addr, &record).await {
            Ok(tun) => return Ok(tun),
            Err(e) => last_err = format!("{addr}: {e}"),
        }
    }
    Err(last_err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link::tests::MOCK_ENV_LOCK;
    use std::net::TcpListener;

    #[test]
    fn test_host_identifier_and_auth_tag_match_pymobiledevice3() {
        assert_eq!(
            host_identifier("ZenvyChan"),
            "25C323A2-B6C8-3641-8C3D-C99D5184F57E"
        );
        let irk: [u8; 16] = core::array::from_fn(|i| i as u8);
        let ident = "D45C330B-B1B7-4E60-842C-C3ED8C81C91B";
        let tag = compute_auth_tag(&irk, ident);
        assert_eq!(tag, "MxMIjju5");
        assert!(auth_tag_matches(&irk, ident, &tag));
        assert!(!auth_tag_matches(&[0u8; 16], ident, &tag));
        assert!(!auth_tag_matches(&irk, ident, "invalid!!"));
    }

    #[test]
    fn test_local_ipv4_priority_and_subnet_guard() {
        let sorted = sort_local_ipv4_candidates(vec![
            "127.0.0.1".into(),
            "169.254.12.34".into(),
            "203.0.113.5".into(),
            "192.168.1.10".into(),
            "10.0.0.4".into(),
            "172.20.10.2".into(),
        ]);
        assert_eq!(sorted[0], "192.168.1.10");
        assert_eq!(sorted[1], "10.0.0.4");
        assert_eq!(sorted[2], "172.20.10.2");
        assert_eq!(sorted.last().map(String::as_str), Some("169.254.12.34"));
        assert!(!sorted.contains(&"127.0.0.1".to_string()));

        assert!(!is_ip_on_local_subnets("127.0.0.1"));
        assert!(!is_ip_on_local_subnets("169.254.1.2"));
        assert!(!is_ip_on_local_subnets("192.0.2.1"));
        if let Some(first_lan) = local_ipv4_addresses()
            .into_iter()
            .find(|ip| is_private_lan_ip(ip))
        {
            if let Some(dot) = first_lan.rfind('.') {
                let peer = format!("{}.247", &first_lan[..dot]);
                assert!(is_ip_on_local_subnets(&peer));
            }
        }
    }

    #[test]
    fn test_verify_ip_is_apple_device_accepts_lockdown_and_rejects_http_sinkhole() {
        // 1. Mock genuine Apple Lockdown server returning Type = com.apple.mobile.lockdown
        let apple_srv = TcpListener::bind("127.0.0.1:0").unwrap();
        let apple_port = apple_srv.local_addr().unwrap().port();
        let apple_handle = std::thread::spawn(move || {
            if let Ok((mut s, _)) = apple_srv.accept() {
                let mut lb = [0u8; 4];
                if s.read_exact(&mut lb).is_ok() {
                    let len = u32::from_be_bytes(lb) as usize;
                    let mut req_buf = vec![0u8; len];
                    let _ = s.read_exact(&mut req_buf);
                    let mut resp = plist::Dictionary::new();
                    resp.insert(
                        "Type".into(),
                        plist::Value::String("com.apple.mobile.lockdown".into()),
                    );
                    resp.insert(
                        "UniqueDeviceID".into(),
                        plist::Value::String("00008150-TESTWIFI".into()),
                    );
                    let rb = plist::to_bytes_xml(&plist::Value::Dictionary(resp)).unwrap();
                    let _ = s.write_all(&(rb.len() as u32).to_be_bytes());
                    let _ = s.write_all(&rb);
                    let _ = s.flush();
                }
            }
        });

        assert!(verify_ip_is_apple_device_with_port(
            "127.0.0.1",
            apple_port,
            Some("00008150-TESTWIFI"),
            400,
            true,
        ));
        let _ = apple_handle.join();

        // 2. Fake HTTP / sinkhole TCP server must be strictly rejected
        let fake_srv = TcpListener::bind("127.0.0.1:0").unwrap();
        let fake_port = fake_srv.local_addr().unwrap().port();
        let fake_handle = std::thread::spawn(move || {
            if let Ok((mut s, _)) = fake_srv.accept() {
                let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK");
                let _ = s.flush();
            }
        });
        assert!(!verify_ip_is_apple_device_with_port(
            "127.0.0.1",
            fake_port,
            None,
            300,
            true,
        ));
        let _ = fake_handle.join();
    }

    #[test]
    fn test_wifi_state_json_and_remote_plist_persistence_and_unicast_packet() {
        let _guard = MOCK_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp_root = std::env::temp_dir().join("iphone_link_tahap227_wifi_state_test");
        let _ = fs::remove_dir_all(&tmp_root);
        fs::create_dir_all(&tmp_root).unwrap();
        let state_file = tmp_root.join("wifi_state.json");
        let pairing_dir = tmp_root.join("pairing");

        std::env::set_var("IPHONE_LINK_WIFI_STATE_PATH", &state_file);
        std::env::set_var("IPHONE_LINK_PAIRING_DIR", &pairing_dir);

        let irk = [7u8; 16];
        let paths = save_remote_record("00008150-WIFIPAIR", &irk, b"privkey", b"pubkey").unwrap();
        assert_eq!(paths.len(), 1);
        assert!(paths[0].is_file());
        let loaded = all_records();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].udid, "00008150-WIFIPAIR");
        assert_eq!(loaded[0].peer_alt_irk, irk);

        update_device_wifi_state(
            "00008150-WIFIPAIR",
            Some(true),
            Some("192.0.2.41"),
            false,
            Some("AA:BB:CC:DD:EE:FF"),
            Some("Test iPhone"),
            true,
            true,
            Some(58767),
        );

        let state = load_wifi_state_value();
        assert_eq!(
            state.get("last_ip").and_then(|v| v.as_str()),
            Some("192.0.2.41")
        );
        let enabled_arr = state.get("enabled_udids").and_then(|v| v.as_array()).unwrap();
        assert!(enabled_arr.iter().any(|v| v.as_str() == Some("00008150-WIFIPAIR")));
        assert_eq!(
            state
                .pointer("/mac_map/00008150-WIFIPAIR")
                .and_then(|v| v.as_str()),
            Some("aa-bb-cc-dd-ee-ff")
        );

        let st_payload = get_wifi_status(Some("00008150-WIFIPAIR"), false);
        assert!(st_payload.enabled);
        assert!(st_payload.reachable);
        assert_eq!(st_payload.ip.as_deref(), Some("192.0.2.41"));

        let q = build_dnssd_ptr_query(APPLE_MOBDEV2_SERVICE, 0x4151);
        assert_eq!(&q[0..2], &[0x41, 0x51]);
        assert_eq!(&q[q.len() - 2..], &[0x80, 0x01]);

        std::env::remove_var("IPHONE_LINK_WIFI_STATE_PATH");
        std::env::remove_var("IPHONE_LINK_PAIRING_DIR");
        let _ = fs::remove_dir_all(&tmp_root);
    }

    #[test]
    fn test_prepare_wifi_over_usb_and_unicast_dns_sd_responder() {
        let _guard = MOCK_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        // 1. Live UDP Unicast DNS-SD SRV responder (< 10ms query)
        let udp_srv = UdpSocket::bind("127.0.0.1:0").unwrap();
        let udp_port = udp_srv.local_addr().unwrap().port();
        let _ = udp_srv.set_read_timeout(Some(Duration::from_millis(400)));
        let udp_handle = std::thread::spawn(move || {
            let mut buf = [0u8; 512];
            if let Ok((_n, peer)) = udp_srv.recv_from(&mut buf) {
                // DNS response with SRV RR (TYPE=0x0021) pointing to port 58767
                let mut pkt = vec![
                    0x41, 0x50, 0x84, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00,
                ];
                pkt.extend_from_slice(&[0x00, 0x21, 0x00, 0x01, 0x00, 0x00, 0x00, 0x78]);
                pkt.extend_from_slice(&(7u16).to_be_bytes()); // RDLENGTH: Priority(2)+Weight(2)+Port(2)+Root(1)
                pkt.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // Priority, Weight
                pkt.extend_from_slice(&(58767u16).to_be_bytes()); // Port 58767
                pkt.extend_from_slice(&[0x00]); // Target root
                let _ = udp_srv.send_to(&pkt, peer);
            }
        });

        let ports = query_unicast_dns_sd_to_port("127.0.0.1", udp_port, 50);
        let _ = udp_handle.join();
        assert_eq!(ports, vec![58767]);

        // 2. Mock USBMUXD + Lockdown EnableWifiConnections/Debugging + RemotePairing PIN 000000
        let tmp_root = std::env::temp_dir().join("iphone_link_tahap227_pair_usb_test");
        let _ = fs::remove_dir_all(&tmp_root);
        fs::create_dir_all(&tmp_root).unwrap();
        let state_file = tmp_root.join("wifi_state.json");
        let pairing_dir = tmp_root.join("pairing");
        std::env::set_var("IPHONE_LINK_WIFI_STATE_PATH", &state_file);
        std::env::set_var("IPHONE_LINK_PAIRING_DIR", &pairing_dir);

        let mux_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mux_addr = mux_listener.local_addr().unwrap();
        mux_listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_srv = stop.clone();

        fn read_mux(s: &mut TcpStream) -> plist::Dictionary {
            let mut hdr = [0u8; 16];
            s.read_exact(&mut hdr).unwrap();
            let len = u32::from_le_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]) as usize;
            let mut body = vec![0u8; len - 16];
            s.read_exact(&mut body).unwrap();
            plist::from_bytes(&body).unwrap().into_dictionary().unwrap()
        }
        fn write_mux(s: &mut TcpStream, d: plist::Dictionary) {
            let b = plist::to_bytes_xml(&plist::Value::Dictionary(d)).unwrap();
            let total = (16 + b.len()) as u32;
            let mut hdr = [0u8; 16];
            hdr[0..4].copy_from_slice(&total.to_le_bytes());
            hdr[4..8].copy_from_slice(&1u32.to_le_bytes());
            hdr[8..12].copy_from_slice(&8u32.to_le_bytes());
            hdr[12..16].copy_from_slice(&1u32.to_le_bytes());
            s.write_all(&hdr).unwrap();
            s.write_all(&b).unwrap();
            s.flush().unwrap();
        }
        fn read_lp(s: &mut TcpStream) -> Option<plist::Dictionary> {
            let mut lb = [0u8; 4];
            s.read_exact(&mut lb).ok()?;
            let len = u32::from_be_bytes(lb) as usize;
            let mut b = vec![0u8; len];
            s.read_exact(&mut b).ok()?;
            plist::from_bytes(&b).ok()?.into_dictionary()
        }
        fn write_lp(s: &mut TcpStream, d: plist::Dictionary) {
            let b = plist::to_bytes_xml(&plist::Value::Dictionary(d)).unwrap();
            s.write_all(&(b.len() as u32).to_be_bytes()).unwrap();
            s.write_all(&b).unwrap();
            s.flush().unwrap();
        }

        let seen_set_keys = Arc::new(Mutex::new(Vec::<String>::new()));
        let seen_clone = Arc::clone(&seen_set_keys);
        let irk: [u8; 16] = core::array::from_fn(|i| (i as u8) + 21);

        let srv_handle = std::thread::spawn(move || {
            let mut handlers = Vec::new();
            while !stop_srv.load(std::sync::atomic::Ordering::Relaxed) {
                match mux_listener.accept() {
                    Ok((mut s, _)) => {
                        let _ = s.set_nonblocking(false);
                        let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
                        let sc = Arc::clone(&seen_clone);
                        handlers.push(std::thread::spawn(move || {
                            let req = read_mux(&mut s);
                            match req.get("MessageType").and_then(|v| v.as_string()).unwrap_or("") {
                                "ListDevices" => {
                                    let mut p = plist::Dictionary::new();
                                    p.insert("SerialNumber".into(), plist::Value::String("00008150-USBPAIRWIFI".into()));
                                    p.insert("DeviceID".into(), plist::Value::from(12u32));
                                    p.insert("ConnectionType".into(), plist::Value::String("USB".into()));
                                    let mut dev = plist::Dictionary::new();
                                    dev.insert("Properties".into(), plist::Value::Dictionary(p));
                                    let mut resp = plist::Dictionary::new();
                                    resp.insert("DeviceList".into(), plist::Value::Array(vec![plist::Value::Dictionary(dev)]));
                                    write_mux(&mut s, resp);
                                }
                                "ReadPairRecord" => {
                                    let mut pair = plist::Dictionary::new();
                                    pair.insert("HostID".into(), plist::Value::String("H1".into()));
                                    let mut resp = plist::Dictionary::new();
                                    resp.insert("PairRecordData".into(), plist::Value::Data(plist::to_bytes_xml(&plist::Value::Dictionary(pair)).unwrap()));
                                    write_mux(&mut s, resp);
                                }
                                "Connect" => {
                                    let port = u16::from_be(req.get("PortNumber").and_then(|v| v.as_unsigned_integer()).unwrap() as u16);
                                    let mut ok = plist::Dictionary::new();
                                    ok.insert("Number".into(), plist::Value::from(0u32));
                                    write_mux(&mut s, ok);
                                    if port == LOCKDOWN_PORT {
                                        while let Some(lr) = read_lp(&mut s) {
                                            let rtype = lr.get("Request").and_then(|v| v.as_string()).unwrap_or("");
                                            let mut lresp = plist::Dictionary::new();
                                            match rtype {
                                                "StartSession" => {
                                                    lresp.insert("EnableSessionSSL".into(), plist::Value::Boolean(false));
                                                }
                                                "SetValue" => {
                                                    if let Some(k) = lr.get("Key").and_then(|v| v.as_string()) {
                                                        sc.lock().unwrap().push(k.to_string());
                                                    }
                                                }
                                                "GetValue" => {
                                                    let key = lr.get("Key").and_then(|v| v.as_string()).unwrap_or("");
                                                    let val = match key {
                                                        "DeviceName" => "iPhone 17 Pro Test",
                                                        "WiFiAddress" => "aa:bb:cc:11:22:33",
                                                        _ => "00008150-USBPAIRWIFI",
                                                    };
                                                    lresp.insert("Value".into(), plist::Value::String(val.into()));
                                                }
                                                "StartService" => {
                                                    lresp.insert("Port".into(), plist::Value::from(59999u16));
                                                    lresp.insert("EnableServiceSSL".into(), plist::Value::Boolean(false));
                                                }
                                                _ => {}
                                            }
                                            write_lp(&mut s, lresp);
                                        }
                                    } else if port == 59999 {
                                        if let Some(pr) = read_lp(&mut s) {
                                            assert_eq!(pr.get("PIN").and_then(|v| v.as_string()), Some("000000"));
                                            let mut presp = plist::Dictionary::new();
                                            presp.insert("alt_irk".into(), plist::Value::Data(irk.to_vec()));
                                            presp.insert("encryptionKey".into(), plist::Value::Data(vec![0x5Au8; 32]));
                                            write_lp(&mut s, presp);
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
        let res = prepare_wifi_over_usb(Some("00008150-USBPAIRWIFI"));
        std::env::remove_var("USBMUXD_SOCKET_ADDRESS");
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = srv_handle.join();

        let pair_res = res.expect("prepare_wifi_over_usb should succeed over mock USB");
        assert!(pair_res.ok);
        assert!(pair_res.wifi_enabled);
        assert!(pair_res.wireless_lockdown_enabled);
        assert!(pair_res.paired.contains(&"00008150-USBPAIRWIFI".to_string()));
        let set_keys = seen_set_keys.lock().unwrap();
        assert!(set_keys.contains(&"EnableWifiConnections".to_string()));
        assert!(set_keys.contains(&"EnableWifiDebugging".to_string()));
        assert!(pairing_dir.join("remote_00008150-USBPAIRWIFI.plist").is_file());
        assert!(state_file.is_file());

        std::env::remove_var("IPHONE_LINK_WIFI_STATE_PATH");
        std::env::remove_var("IPHONE_LINK_PAIRING_DIR");
        let _ = fs::remove_dir_all(&tmp_root);
    }

    #[test]
    fn test_tahap247_wifi_state_metadata_roundtrip() {
        let _guard = crate::hid::HID_TEST_SERIAL_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        clear_cached_wifi_reachability();

        let tmp_root = std::env::temp_dir().join(format!(
            "iphone_link_t247_meta_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&tmp_root);
        let pairing_dir = tmp_root.join("pairing");
        fs::create_dir_all(&pairing_dir).unwrap();
        let state_file = tmp_root.join("wifi_state.json");

        std::env::set_var("IPHONE_LINK_WIFI_STATE_PATH", &state_file);
        std::env::set_var("IPHONE_LINK_PAIRING_DIR", &pairing_dir);

        update_device_wifi_state(
            "00008150-TEST-DEV-A",
            Some(true),
            Some("198.51.100.49"),
            false,
            Some("AA:BB:CC:11:22:33"),
            Some("Alpha Phone"),
            true,
            true,
            Some(49152),
        );
        update_device_full_metadata(
            "00008150-TEST-DEV-A",
            Some("Alpha Phone"),
            Some("iPhone18,1"),
            None, // Must derive "iPhone 17 Pro" from official ProductType map
            Some("27.2"),
            Some("AA:BB:CC:DD:EE:01"),
            Some("198.51.100.49"),
            Some(1_763_600_123),
            Some(true),
        );

        // Clear in-memory reachability cache to verify full disk roundtrip from wifi_state.json
        clear_cached_wifi_reachability();
        let loaded = read_device_metadata_from_wifi_state("00008150-TEST-DEV-A");
        assert_eq!(loaded.name, "Alpha Phone");
        assert_eq!(loaded.product_type, "iPhone18,1");
        assert_eq!(loaded.model, "iPhone 17 Pro");
        assert_eq!(loaded.ios_version, "27.2");
        assert_eq!(loaded.bluetooth_address, "AA:BB:CC:DD:EE:01");
        assert_eq!(loaded.last_ip.as_deref(), Some("198.51.100.49"));
        assert_eq!(loaded.last_seen_at, 1_763_600_123);
        assert_eq!(loaded.reachable, Some(true));

        let payload = get_wifi_status(Some("00008150-TEST-DEV-A"), false);
        let pjson = payload.to_json_string();
        assert!(pjson.contains(r#""Alpha Phone""#), "missing name in wifi payload: {pjson}");
        assert!(pjson.contains(r#""iPhone 17 Pro""#), "missing model in wifi payload: {pjson}");
        assert!(pjson.contains(r#""198.51.100.49""#), "missing ip in wifi payload: {pjson}");

        clear_cached_wifi_reachability();
        std::env::remove_var("IPHONE_LINK_WIFI_STATE_PATH");
        std::env::remove_var("IPHONE_LINK_PAIRING_DIR");
        let _ = fs::remove_dir_all(&tmp_root);
    }
}
