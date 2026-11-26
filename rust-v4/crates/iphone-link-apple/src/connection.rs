//! Dual-Transport connection state machine and Apple service host verification.

use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::atomic::{AtomicBool, Ordering};

pub const LOCKDOWN_PORT: u16 = 62078;
pub const MDNS_PORT: u16 = 5353;
pub const COREDEVICE_TUNNEL_PORT: u16 = 62078;
pub const LOCAL_STREAM_PORT: u16 = 8766;

static LOOPBACK_ONLY_TEST_MODE: AtomicBool = AtomicBool::new(false);

/// Enables or disables in-process loopback-only test binding (`127.0.0.1` instead of `0.0.0.0`,
/// skipping LAN multicast mDNS) so automated self-tests never trigger Windows Defender Firewall popups.
pub fn set_loopback_only_test_mode(enabled: bool) {
    LOOPBACK_ONLY_TEST_MODE.store(enabled, Ordering::SeqCst);
}

/// Returns `true` during automated self-tests (`--self-test` / `--native-ui-selftest` /
/// `IPHONE_LINK_TEST_LOOPBACK_ONLY=1|true|yes`). Normal user launches ALWAYS return `false`.
pub fn is_loopback_only_test_mode() -> bool {
    if LOOPBACK_ONLY_TEST_MODE.load(Ordering::Relaxed) {
        return true;
    }
    std::env::var("IPHONE_LINK_TEST_LOOPBACK_ONLY")
        .map(|v| {
            let trimmed = v.trim();
            trimmed == "1"
                || trimmed.eq_ignore_ascii_case("true")
                || trimmed.eq_ignore_ascii_case("yes")
        })
        .unwrap_or(false)
}

/// Pure helper resolving `"127.0.0.1"` when `loopback_only == true` and `"0.0.0.0"` otherwise.
pub fn resolve_bind_host_for_mode(loopback_only: bool) -> &'static str {
    if loopback_only {
        "127.0.0.1"
    } else {
        "0.0.0.0"
    }
}

/// Returns `"127.0.0.1"` in loopback-only test mode or `"0.0.0.0"` in normal runtime mode.
pub fn bind_host_ipv4() -> &'static str {
    resolve_bind_host_for_mode(is_loopback_only_test_mode())
}

/// Pure helper resolving `SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)` when
/// `loopback_only == true` and `SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port)` otherwise.
pub fn resolve_bind_addr_v4_for_mode(port: u16, loopback_only: bool) -> SocketAddrV4 {
    let ip = if loopback_only {
        Ipv4Addr::LOCALHOST
    } else {
        Ipv4Addr::UNSPECIFIED
    };
    SocketAddrV4::new(ip, port)
}

/// Returns `SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)` in loopback-only test mode,
/// or `SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port)` in normal runtime mode.
pub fn bind_addr_v4(port: u16) -> SocketAddrV4 {
    resolve_bind_addr_v4_for_mode(port, is_loopback_only_test_mode())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportMode {
    CableUsb,
    WifiCoreDevice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStage {
    Disconnected,
    DetectingDevice,
    SecureHandshake,
    DeviceReady,
    ActiveStreaming,
}

#[derive(Debug, Clone)]
pub struct TransportEndpoint {
    pub mode: TransportMode,
    pub host: String,
    pub port: u16,
    pub udid: String,
    pub stage: ConnectionStage,
}

impl TransportEndpoint {
    pub fn new_cable(udid: &str) -> Self {
        Self {
            mode: TransportMode::CableUsb,
            host: "127.0.0.1".to_string(),
            port: LOCKDOWN_PORT,
            udid: udid.to_string(),
            stage: ConnectionStage::Disconnected,
        }
    }

    pub fn new_wifi(udid: &str, ip: &str) -> Self {
        Self {
            mode: TransportMode::WifiCoreDevice,
            host: ip.to_string(),
            port: COREDEVICE_TUNNEL_PORT,
            udid: udid.to_string(),
            stage: ConnectionStage::Disconnected,
        }
    }

    pub fn advance_to(&mut self, next: ConnectionStage) {
        self.stage = next;
    }

    pub fn is_streaming(&self) -> bool {
        self.stage == ConnectionStage::ActiveStreaming
    }
}

pub fn verify_transport_ready(mode: TransportMode, host: &str, port: u16) -> Result<String, String> {
    if host.is_empty() {
        return Err("Host address cannot be empty".to_string());
    }
    if port == 0 {
        return Err("Port must be non-zero".to_string());
    }
    match mode {
        TransportMode::CableUsb => {
            Ok(format!("Cable USB endpoint valid: {}:{}", host, port))
        }
        TransportMode::WifiCoreDevice => {
            Ok(format!("WiFi CoreDevice endpoint valid: {}:{}", host, port))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transport_endpoints() {
        let mut usb = TransportEndpoint::new_cable("00008101-0000000000000004");
        assert_eq!(usb.mode, TransportMode::CableUsb);
        assert_eq!(usb.port, 62078);
        assert_eq!(usb.stage, ConnectionStage::Disconnected);

        usb.advance_to(ConnectionStage::DetectingDevice);
        usb.advance_to(ConnectionStage::SecureHandshake);
        usb.advance_to(ConnectionStage::DeviceReady);
        usb.advance_to(ConnectionStage::ActiveStreaming);
        assert!(usb.is_streaming());

        let wifi = TransportEndpoint::new_wifi("00008101-0000000000000004", "192.0.2.38");
        assert_eq!(wifi.mode, TransportMode::WifiCoreDevice);
        assert_eq!(wifi.host, "192.0.2.38");

        assert!(verify_transport_ready(TransportMode::CableUsb, "127.0.0.1", 62078).is_ok());
        assert!(verify_transport_ready(TransportMode::WifiCoreDevice, "192.0.2.38", 62078).is_ok());
        assert!(verify_transport_ready(TransportMode::CableUsb, "", 62078).is_err());
    }
}
