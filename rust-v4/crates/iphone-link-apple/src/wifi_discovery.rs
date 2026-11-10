//! Wi-Fi device discovery, Apple host verification, and Unicast mDNS query engine.

use std::fs;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream, UdpSocket};
use std::path::Path;
use std::time::Duration;

pub const LOCKDOWN_PORT: u16 = 62078;
pub const MDNS_PORT: u16 = 5353;
pub const REMOTEPAIRING_SERVICE: &str = "_remotepairing._tcp.local.";

/// Check if an IPv4 address belongs to a local /24 subnet of the host PC.
/// Ignores APIPA (169.254.x.x), loopback (127.x.x.x), and unspecified addresses.
pub fn is_ip_on_local_subnets(ip: IpAddr) -> bool {
    let target_v4 = match ip {
        IpAddr::V4(v4) => v4,
        IpAddr::V6(_) => return false,
    };

    if target_v4.is_loopback() || target_v4.is_unspecified() {
        return false;
    }

    // Check APIPA (169.254.x.x)
    let octets = target_v4.octets();
    if octets[0] == 169 && octets[1] == 254 {
        return false;
    }

    // Discover host local active IPv4 addresses by binding a dummy UDP socket
    let host_ips = get_host_local_ips();
    for host_ip in host_ips {
        let host_octets = host_ip.octets();
        // Match /24 subnet (first 3 octets)
        if host_octets[0] == octets[0] && host_octets[1] == octets[1] && host_octets[2] == octets[2] {
            return true;
        }
    }
    false
}

/// Retrieve local IPv4 addresses of this machine.
pub fn get_host_local_ips() -> Vec<Ipv4Addr> {
    let mut ips = Vec::new();
    // Use dummy outbound UDP sockets to query default routing interfaces
    let probe_targets = [
        "8.8.8.8:80",
        "1.1.1.1:80",
        "192.168.1.1:80",
        "192.168.100.1:80",
        "10.0.0.1:80",
    ];

    for target in probe_targets {
        if let Ok(socket) = UdpSocket::bind("0.0.0.0:0") {
            if socket.connect(target).is_ok() {
                if let Ok(local_addr) = socket.local_addr() {
                    if let IpAddr::V4(v4) = local_addr.ip() {
                        if !v4.is_loopback() && !ips.contains(&v4) {
                            ips.push(v4);
                        }
                    }
                }
            }
        }
    }

    // Fallback if no outbound route
    if ips.is_empty() {
        ips.push(Ipv4Addr::new(192, 168, 100, 8)); // Host default fallback
    }
    ips
}

/// Perform real TCP connect with TCP_NODELAY to Apple Lockdown port (62078).
pub fn probe_apple_lockdown_port(ip: IpAddr, timeout: Duration) -> bool {
    let socket_addr = SocketAddr::new(ip, LOCKDOWN_PORT);
    match TcpStream::connect_timeout(&socket_addr, timeout) {
        Ok(stream) => {
            let _ = stream.set_nodelay(true);
            true
        }
        Err(_) => false,
    }
}

/// Build a Unicast DNS-SD PTR/SRV query packet for _remotepairing._tcp.local.
/// Uses the Unicast Response (QU) bit in CLASS (0x8001) to request unicast UDP reply.
pub fn build_unicast_mdns_query(transaction_id: u16) -> Vec<u8> {
    let mut packet = Vec::with_capacity(64);
    // Header: ID, Flags (0x0000 = standard query), QDCOUNT = 1, ANCOUNT = 0, NSCOUNT = 0, ARCOUNT = 0
    packet.extend_from_slice(&transaction_id.to_be_bytes());
    packet.extend_from_slice(&[0x00, 0x00]); // Standard query
    packet.extend_from_slice(&[0x00, 0x01]); // 1 question
    packet.extend_from_slice(&[0x00, 0x00]); // 0 answers
    packet.extend_from_slice(&[0x00, 0x00]); // 0 authorities
    packet.extend_from_slice(&[0x00, 0x00]); // 0 additional

    // Question Name: \x0e_remotepairing\x04_tcp\x05local\x00
    let labels: &[(u8, &[u8])] = &[
        (14, b"_remotepairing"),
        (4, b"_tcp"),
        (5, b"local"),
    ];
    for (len, label) in labels {
        packet.push(*len);
        packet.extend_from_slice(label);
    }
    packet.push(0x00); // Root label

    // QTYPE: 0x000C (PTR, 12)
    packet.extend_from_slice(&[0x00, 0x0C]);
    // QCLASS: 0x8001 (IN with QU bit set for unicast response)
    packet.extend_from_slice(&[0x80, 0x01]);

    packet
}

/// Parse port number from an SRV record payload in DNS response.
pub fn parse_srv_port_from_dns(payload: &[u8]) -> Option<u16> {
    if payload.len() < 12 {
        return None;
    }
    // Scan for SRV type 0x0021 (33)
    let mut i = 12;
    while i + 10 <= payload.len() {
        if payload[i] == 0x00 && payload[i + 1] == 0x21 {
            // Found SRV type; skip TYPE (2), CLASS (2), TTL (4) -> offset +8 is RDLENGTH
            let rdlen_offset = i + 8;
            if rdlen_offset + 2 <= payload.len() {
                let rdlen = u16::from_be_bytes([payload[rdlen_offset], payload[rdlen_offset + 1]]) as usize;
                let rdata_start = rdlen_offset + 2;
                if rdata_start + rdlen <= payload.len() && rdlen >= 6 {
                    // SRV RDATA: Priority (2), Weight (2), Port (2)
                    let port = u16::from_be_bytes([payload[rdata_start + 4], payload[rdata_start + 5]]);
                    if port > 0 {
                        return Some(port);
                    }
                }
            }
        }
        i += 1;
    }
    None
}

/// Send UDP unicast DNS-SD query for _remotepairing._tcp.local. directly to (ip, 5353).
/// Bypasses network routers (like Devani 5G) that block multicast traffic.
pub fn query_unicast_mdns_remotepairing(ip: IpAddr, timeout: Duration) -> Option<u16> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.set_read_timeout(Some(timeout)).ok()?;
    socket.set_write_timeout(Some(timeout)).ok()?;

    let target = SocketAddr::new(ip, MDNS_PORT);
    let query_packet = build_unicast_mdns_query(0x4150); // 'AP' Apple tag

    socket.send_to(&query_packet, target).ok()?;

    let mut buf = [0u8; 1500];
    let (len, _) = socket.recv_from(&mut buf).ok()?;
    parse_srv_port_from_dns(&buf[..len])
}

/// Verify if an IP belongs to a genuine Apple device on the local network.
/// Must be on local subnet AND (respond on lockdown port 62078 OR answer unicast mDNS).
pub fn verify_ip_is_apple_device(ip: IpAddr, timeout: Duration) -> bool {
    if !is_ip_on_local_subnets(ip) {
        return false;
    }

    // Fast check: probe lockdown port 62078
    if probe_apple_lockdown_port(ip, timeout) {
        return true;
    }

    // Fallback: check unicast mDNS remote pairing port
    query_unicast_mdns_remotepairing(ip, timeout).is_some()
}

/// Pairing state cache representation for wifi_state.json.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceWifiRecord {
    pub udid: String,
    pub last_ip: String,
    pub last_seen_cable: u64,
    pub wifi_enabled_on_device: bool,
}

/// Simple JSON loader and parser for wifi_state.json.
pub fn load_wifi_state(path: &Path) -> Vec<DeviceWifiRecord> {
    let Ok(content) = fs::read_to_string(path) else {
        return Vec::new();
    };

    let mut records = Vec::new();
    let mut search_from = 0;

    while let Some(dict_start) = content[search_from..].find('{') {
        let abs_start = search_from + dict_start;
        let dict_end = match content[abs_start..].find('}') {
            Some(e) => abs_start + e + 1,
            None => break,
        };
        let block = &content[abs_start..dict_end];

        let udid = extract_json_field(block, "udid");
        let last_ip = extract_json_field(block, "last_ip").unwrap_or_default();
        let wifi_enabled = block.contains("\"wifi_enabled_on_device\": true")
            || block.contains("\"wifi_enabled_on_device\":true");

        if let Some(u) = udid {
            records.push(DeviceWifiRecord {
                udid: u,
                last_ip,
                last_seen_cable: 0,
                wifi_enabled_on_device: wifi_enabled,
            });
        }
        search_from = dict_end;
    }

    records
}

fn extract_json_field(json: &str, field: &str) -> Option<String> {
    let key = format!("\"{}\"", field);
    let k_idx = json.find(&key)?;
    let after_k = &json[k_idx + key.len()..];
    let colon_idx = after_k.find(':')?;
    let after_colon = after_k[colon_idx + 1..].trim();

    if let Some(rest) = after_colon.strip_prefix('"') {
        let end_idx = rest.find('"')?;
        Some(rest[..end_idx].to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_local_subnet_check() {
        // Loopback must be rejected
        assert!(!is_ip_on_local_subnets(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))));
        // APIPA must be rejected
        assert!(!is_ip_on_local_subnets(IpAddr::V4(Ipv4Addr::new(169, 254, 1, 2))));
        // Foreign random subnet must be rejected
        assert!(!is_ip_on_local_subnets(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1))));

        // Valid host subnet check
        let host_ips = get_host_local_ips();
        if let Some(first) = host_ips.first() {
            let oct = first.octets();
            let neighbor = IpAddr::V4(Ipv4Addr::new(oct[0], oct[1], oct[2], 250));
            assert!(is_ip_on_local_subnets(neighbor));
        }
    }

    #[test]
    fn test_build_unicast_mdns_packet() {
        let packet = build_unicast_mdns_query(0x1234);
        assert_eq!(packet[0], 0x12);
        assert_eq!(packet[1], 0x34);
        assert_eq!(packet[2], 0x00);
        assert_eq!(packet[3], 0x00);
        assert!(packet.len() >= 36);
        // Verify QU bit in QCLASS: 0x80, 0x01
        assert_eq!(packet[packet.len() - 2], 0x80);
        assert_eq!(packet[packet.len() - 1], 0x01);
    }

    #[test]
    fn test_verify_ip_non_apple_rejected() {
        // Fake IP that does not exist or does not run lockdown
        let fake = IpAddr::V4(Ipv4Addr::new(192, 168, 100, 249));
        let verified = verify_ip_is_apple_device(fake, Duration::from_millis(50));
        assert!(!verified);
    }
}
