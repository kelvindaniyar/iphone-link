//! Pure-Rust mDNS Advertiser & Query Responder (`224.0.0.251:5353`) for `_airplay._tcp.local.`
//! and `_raop._tcp.local.` without external Bonjour/mDNS crates.
//!
//! Uses Win32 `ws2_32` `SO_REUSEADDR` before `bind(0.0.0.0:5353)` so it coexists cleanly with
//! Windows `dnscache` / Apple Bonjour, responds to multicast/unicast queries from iOS Control Center,
//! and emits RFC 6762 Goodbye packets (`TTL = 0`) on shutdown.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::info::ReceiverInfo;

pub const MDNS_ADDR: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
pub const MDNS_PORT: u16 = 5353;

const TYPE_A: u16 = 1;
const TYPE_PTR: u16 = 12;
const TYPE_TXT: u16 = 16;
const TYPE_SRV: u16 = 33;
const CLASS_IN: u16 = 1;
const CLASS_IN_CACHE_FLUSH: u16 = 0x8001;

const TTL_HOST_SRV: u32 = 120;
const TTL_PTR_TXT: u32 = 4500;

/// Returns active non-loopback, non-APIPA local IPv4 addresses suitable for LAN mDNS advertisement.
pub fn collect_active_lan_ipv4s() -> Vec<Ipv4Addr> {
    if crate::connection::is_loopback_only_test_mode() {
        return vec![Ipv4Addr::LOCALHOST];
    }
    let mut ips = Vec::new();
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
                if let Ok(local) = sock.local_addr() {
                    if let IpAddr::V4(v4) = local.ip() {
                        let o = v4.octets();
                        let is_apipa = o[0] == 169 && o[1] == 254;
                        if !v4.is_loopback() && !v4.is_unspecified() && !is_apipa && !ips.contains(&v4)
                        {
                            ips.push(v4);
                        }
                    }
                }
            }
        }
    }
    if ips.is_empty() {
        for v4 in crate::wifi_discovery::get_host_local_ips() {
            let o = v4.octets();
            let is_apipa = o[0] == 169 && o[1] == 254;
            if !v4.is_loopback() && !v4.is_unspecified() && !is_apipa && !ips.contains(&v4) {
                ips.push(v4);
            }
        }
    }
    ips
}

fn push_dns_name(buf: &mut Vec<u8>, name: &str) {
    for label in name.trim_end_matches('.').split('.') {
        if label.is_empty() {
            continue;
        }
        let bytes = label.as_bytes();
        let len = bytes.len().min(63);
        buf.push(len as u8);
        buf.extend_from_slice(&bytes[..len]);
    }
    buf.push(0);
}

fn push_rr(buf: &mut Vec<u8>, name: &str, rr_type: u16, rr_class: u16, ttl: u32, rdata: &[u8]) {
    push_dns_name(buf, name);
    buf.extend_from_slice(&rr_type.to_be_bytes());
    buf.extend_from_slice(&rr_class.to_be_bytes());
    buf.extend_from_slice(&ttl.to_be_bytes());
    buf.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
    buf.extend_from_slice(rdata);
}

/// Builds a neutral `.local.` hostname derived purely from the receiver's ephemeral MAC (zero PII).
pub fn neutral_hostname(mac_compact: &str) -> String {
    let suffix = if mac_compact.len() >= 6 {
        &mac_compact[mac_compact.len() - 6..]
    } else {
        "000001"
    };
    format!("iphone-link-{}.local.", suffix.to_ascii_lowercase())
}

/// Builds an RFC 6762 mDNS response packet containing `_airplay._tcp.local.` and `_raop._tcp.local.`
/// `PTR`, `SRV`, `TXT`, and `A` records. Pass `goodbye = true` to emit `TTL = 0` Goodbye records.
pub fn build_announcement_packet(
    info: &ReceiverInfo<'_>,
    port: u16,
    ips: &[Ipv4Addr],
    goodbye: bool,
) -> Vec<u8> {
    let mac_compact = info.id.mac_compact();
    let host_fqdn = neutral_hostname(&mac_compact);
    let airplay_service = "_airplay._tcp.local.";
    let raop_service = "_raop._tcp.local.";
    let airplay_instance = format!("{}.{airplay_service}", info.name);
    let raop_instance = format!("{mac_compact}@{}.{raop_service}", info.name);

    let ttl_ptr = if goodbye { 0 } else { TTL_PTR_TXT };
    let ttl_srv = if goodbye { 0 } else { TTL_HOST_SRV };

    let mut answers = Vec::with_capacity(768);
    let mut ancount: u16 = 0;

    // 1. Service discovery enumeration PTRs
    let mut push_ptr = |owner: &str, target: &str| {
        let mut rdata = Vec::new();
        push_dns_name(&mut rdata, target);
        push_rr(&mut answers, owner, TYPE_PTR, CLASS_IN, ttl_ptr, &rdata);
        ancount += 1;
    };

    push_ptr("_services._dns-sd._udp.local.", airplay_service);
    push_ptr("_services._dns-sd._udp.local.", raop_service);
    push_ptr(airplay_service, &airplay_instance);
    push_ptr(raop_service, &raop_instance);

    // 2. SRV records
    let mut srv_rdata = Vec::new();
    srv_rdata.extend_from_slice(&0u16.to_be_bytes()); // priority
    srv_rdata.extend_from_slice(&0u16.to_be_bytes()); // weight
    srv_rdata.extend_from_slice(&port.to_be_bytes());
    push_dns_name(&mut srv_rdata, &host_fqdn);

    push_rr(
        &mut answers,
        &airplay_instance,
        TYPE_SRV,
        CLASS_IN_CACHE_FLUSH,
        ttl_srv,
        &srv_rdata,
    );
    ancount += 1;
    push_rr(
        &mut answers,
        &raop_instance,
        TYPE_SRV,
        CLASS_IN_CACHE_FLUSH,
        ttl_srv,
        &srv_rdata,
    );
    ancount += 1;

    // 3. TXT records
    let airplay_txt = ReceiverInfo::encode_txt_wire(&info.txt_airplay());
    push_rr(
        &mut answers,
        &airplay_instance,
        TYPE_TXT,
        CLASS_IN_CACHE_FLUSH,
        ttl_ptr,
        &airplay_txt,
    );
    ancount += 1;

    let raop_txt = ReceiverInfo::encode_txt_wire(&info.txt_raop());
    push_rr(
        &mut answers,
        &raop_instance,
        TYPE_TXT,
        CLASS_IN_CACHE_FLUSH,
        ttl_ptr,
        &raop_txt,
    );
    ancount += 1;

    // 4. A records for all active LAN IPv4 addresses
    for ip in ips {
        push_rr(
            &mut answers,
            &host_fqdn,
            TYPE_A,
            CLASS_IN_CACHE_FLUSH,
            ttl_srv,
            &ip.octets(),
        );
        ancount += 1;
    }

    let mut pkt = Vec::with_capacity(12 + answers.len());
    pkt.extend_from_slice(&0u16.to_be_bytes()); // ID = 0 for mDNS
    pkt.extend_from_slice(&0x8400u16.to_be_bytes()); // QR=1, AA=1 (authoritative response)
    pkt.extend_from_slice(&0u16.to_be_bytes()); // QDCOUNT = 0
    pkt.extend_from_slice(&ancount.to_be_bytes()); // ANCOUNT
    pkt.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT = 0
    pkt.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT = 0
    pkt.extend_from_slice(&answers);
    pkt
}

/// Decodes DNS QNAMEs from an incoming mDNS query packet and returns `(should_reply, unicast_requested)`.
pub fn inspect_mdns_query(pkt: &[u8], receiver_name: &str, host_fqdn: &str) -> (bool, bool) {
    if pkt.len() < 12 {
        return (false, false);
    }
    let flags = u16::from_be_bytes([pkt[2], pkt[3]]);
    // QR bit (0x8000) == 1 means response, not query
    if (flags & 0x8000) != 0 {
        return (false, false);
    }
    let qdcount = u16::from_be_bytes([pkt[4], pkt[5]]) as usize;
    if qdcount == 0 {
        return (false, false);
    }

    let name_lower = receiver_name.to_ascii_lowercase();
    let host_lower = host_fqdn.trim_end_matches('.').to_ascii_lowercase();
    let mut offset = 12usize;
    let mut matched = false;
    let mut unicast = false;

    for _ in 0..qdcount.min(32) {
        let Some((qname, next_off)) = read_dns_name(pkt, offset, 0) else {
            break;
        };
        if next_off + 4 > pkt.len() {
            break;
        }
        let qclass = u16::from_be_bytes([pkt[next_off + 2], pkt[next_off + 3]]);
        offset = next_off + 4;

        let q = qname.to_ascii_lowercase();
        if q.contains("_airplay._tcp")
            || q.contains("_raop._tcp")
            || q.contains("_services._dns-sd._udp")
            || (!name_lower.is_empty() && q.contains(&name_lower))
            || (!host_lower.is_empty() && q == host_lower)
        {
            matched = true;
            if (qclass & 0x8000) != 0 {
                unicast = true;
            }
        }
    }
    (matched, unicast)
}

fn read_dns_name(pkt: &[u8], mut off: usize, depth: u8) -> Option<(String, usize)> {
    if depth > 8 || off >= pkt.len() {
        return None;
    }
    let mut labels = Vec::new();
    let end_off;

    loop {
        if off >= pkt.len() {
            return None;
        }
        let len = pkt[off];
        if len == 0 {
            end_off = off + 1;
            break;
        }
        if (len & 0xC0) == 0xC0 {
            if off + 1 >= pkt.len() {
                return None;
            }
            let ptr = (((len & 0x3F) as usize) << 8) | (pkt[off + 1] as usize);
            end_off = off + 2;
            let (suffix, _) = read_dns_name(pkt, ptr, depth + 1)?;
            if !suffix.is_empty() {
                labels.push(suffix);
            }
            break;
        }
        let l = len as usize;
        off += 1;
        if off + l > pkt.len() {
            return None;
        }
        labels.push(String::from_utf8_lossy(&pkt[off..off + l]).into_owned());
        off += l;
    }

    Some((labels.join("."), end_off))
}

#[cfg(windows)]
fn bind_mdns_socket(ips: &[Ipv4Addr]) -> std::io::Result<UdpSocket> {
    use std::os::windows::io::FromRawSocket;

    // Ensure Winsock2 is initialized via stdlib before raw ws2_32 calls.
    let _wsa_init = UdpSocket::bind("127.0.0.1:0")?;

    const AF_INET: i32 = 2;
    const SOCK_DGRAM: i32 = 2;
    const IPPROTO_UDP: i32 = 17;
    const SOL_SOCKET: i32 = 0xFFFF;
    const SO_REUSEADDR: i32 = 0x0004;
    const INVALID_SOCKET: usize = !0usize;

    #[repr(C)]
    struct SockAddrIn {
        sin_family: u16,
        sin_port: u16,
        sin_addr: u32,
        sin_zero: [u8; 8],
    }

    #[link(name = "ws2_32")]
    extern "system" {
        fn socket(af: i32, kind: i32, protocol: i32) -> usize;
        fn setsockopt(s: usize, level: i32, optname: i32, optval: *const u8, optlen: i32) -> i32;
        fn bind(s: usize, name: *const SockAddrIn, namelen: i32) -> i32;
        fn closesocket(s: usize) -> i32;
    }

    let raw = unsafe { socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP) };
    if raw == INVALID_SOCKET {
        return Err(std::io::Error::last_os_error());
    }

    let reuse: i32 = 1;
    unsafe {
        let _ = setsockopt(
            raw,
            SOL_SOCKET,
            SO_REUSEADDR,
            (&reuse as *const i32).cast::<u8>(),
            std::mem::size_of::<i32>() as i32,
        );
    }

    let addr = SockAddrIn {
        sin_family: AF_INET as u16,
        sin_port: MDNS_PORT.to_be(),
        sin_addr: 0, // INADDR_ANY
        sin_zero: [0; 8],
    };
    let rc = unsafe { bind(raw, &addr, std::mem::size_of::<SockAddrIn>() as i32) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        unsafe {
            let _ = closesocket(raw);
        }
        return Err(err);
    }

    let sock = unsafe { UdpSocket::from_raw_socket(raw as u64) };
    let _ = super::disable_udp_connreset(&sock);
    let _ = sock.set_multicast_ttl_v4(255);
    let _ = sock.set_multicast_loop_v4(true);
    let _ = sock.join_multicast_v4(&MDNS_ADDR, &Ipv4Addr::UNSPECIFIED);
    for ip in ips {
        let _ = sock.join_multicast_v4(&MDNS_ADDR, ip);
    }
    Ok(sock)
}

#[cfg(not(windows))]
fn bind_mdns_socket(ips: &[Ipv4Addr]) -> std::io::Result<UdpSocket> {
    let sock = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, MDNS_PORT))?;
    let _ = sock.set_multicast_ttl_v4(255);
    let _ = sock.join_multicast_v4(&MDNS_ADDR, &Ipv4Addr::UNSPECIFIED);
    for ip in ips {
        let _ = sock.join_multicast_v4(&MDNS_ADDR, ip);
    }
    Ok(sock)
}

/// Active mDNS advertiser handle. Dropping or calling `stop()` sends Goodbye (`TTL=0`) packets
/// and joins the background responder thread deterministically (`<= 80 ms`).
pub struct MdnsAdvertiser {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl MdnsAdvertiser {
    pub fn start(info: &ReceiverInfo<'_>, port: u16) -> Result<Self, String> {
        if crate::connection::is_loopback_only_test_mode() {
            return Ok(Self {
                stop: Arc::new(AtomicBool::new(true)),
                thread: None,
            });
        }
        let ips = collect_active_lan_ipv4s();
        let announce_pkt = build_announcement_packet(info, port, &ips, false);
        let goodbye_pkt = build_announcement_packet(info, port, &ips, true);
        let receiver_name = info.name.to_string();
        let host_fqdn = neutral_hostname(&info.id.mac_compact());

        let sock = bind_mdns_socket(&ips).or_else(|_| UdpSocket::bind("0.0.0.0:0"))
            .map_err(|e| format!("mDNS socket bind failed: {e}"))?;
        let _ = sock.set_read_timeout(Some(Duration::from_millis(50)));

        let stop = Arc::new(AtomicBool::new(false));
        let stop_bg = stop.clone();

        let thread = std::thread::Builder::new()
            .name("airplay-mdns".into())
            .spawn(move || {
                let mcast_target = SocketAddr::V4(SocketAddrV4::new(MDNS_ADDR, MDNS_PORT));
                let _ = sock.send_to(&announce_pkt, mcast_target);

                let mut buf = [0u8; 2048];
                let mut burst_sent = 1u8;
                let mut last_announce = Instant::now();

                while !stop_bg.load(Ordering::Relaxed) {
                    if burst_sent < 3 && last_announce.elapsed() >= Duration::from_millis(300) {
                        let _ = sock.send_to(&announce_pkt, mcast_target);
                        burst_sent += 1;
                        last_announce = Instant::now();
                    } else if last_announce.elapsed() >= Duration::from_secs(25) {
                        let _ = sock.send_to(&announce_pkt, mcast_target);
                        last_announce = Instant::now();
                    }

                    match sock.recv_from(&mut buf) {
                        Ok((n, src)) if n >= 12 => {
                            let (should_reply, unicast) =
                                inspect_mdns_query(&buf[..n], &receiver_name, &host_fqdn);
                            if should_reply {
                                let _ = sock.send_to(&announce_pkt, mcast_target);
                                if unicast || src.port() != MDNS_PORT {
                                    let _ = sock.send_to(&announce_pkt, src);
                                }
                            }
                        }
                        _ => {}
                    }
                }

                // RFC 6762 Goodbye packets (TTL=0)
                let _ = sock.send_to(&goodbye_pkt, mcast_target);
                std::thread::sleep(Duration::from_millis(20));
                let _ = sock.send_to(&goodbye_pkt, mcast_target);
            })
            .map_err(|e| format!("mDNS thread spawn failed: {e}"))?;

        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for MdnsAdvertiser {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::airplay::identity::AirPlayIdentity;

    #[test]
    fn test_mdns_announcement_and_query_matching() {
        let id = AirPlayIdentity::from_seed_and_mac([3u8; 32], "02:AA:BB:CC:DD:EE".into());
        let info = ReceiverInfo {
            id: &id,
            name: "iPhone Link",
            hevc: false,
        };
        let ips = [Ipv4Addr::new(192, 0, 2, 10)];
        let announce = build_announcement_packet(&info, 7100, &ips, false);
        assert!(announce.len() > 200);
        // Header: QR=1, AA=1 (0x8400), ANCOUNT >= 9
        assert_eq!(&announce[2..4], &[0x84, 0x00]);
        let ancount = u16::from_be_bytes([announce[6], announce[7]]);
        assert_eq!(ancount, 9);

        // Goodbye packet must zero all TTLs cleanly and remain same size
        let goodbye = build_announcement_packet(&info, 7100, &ips, true);
        assert_eq!(goodbye.len(), announce.len());

        // Construct a synthetic _airplay._tcp.local. PTR query with QU bit set
        let mut query = vec![0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0];
        push_dns_name(&mut query, "_airplay._tcp.local.");
        query.extend_from_slice(&TYPE_PTR.to_be_bytes());
        query.extend_from_slice(&0x8001u16.to_be_bytes()); // QU=1
        let (matched, unicast) =
            inspect_mdns_query(&query, "iPhone Link", &neutral_hostname(&id.mac_compact()));
        assert!(matched);
        assert!(unicast);
    }
}
