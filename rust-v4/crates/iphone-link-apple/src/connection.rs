//! Dual-Transport connection state machine and Apple service host verification.

pub const LOCKDOWN_PORT: u16 = 62078;
pub const MDNS_PORT: u16 = 5353;
pub const COREDEVICE_TUNNEL_PORT: u16 = 62078;
pub const LOCAL_STREAM_PORT: u16 = 8766;

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
        let mut usb = TransportEndpoint::new_cable("00008150-000202D02299401C");
        assert_eq!(usb.mode, TransportMode::CableUsb);
        assert_eq!(usb.port, 62078);
        assert_eq!(usb.stage, ConnectionStage::Disconnected);

        usb.advance_to(ConnectionStage::DetectingDevice);
        usb.advance_to(ConnectionStage::SecureHandshake);
        usb.advance_to(ConnectionStage::DeviceReady);
        usb.advance_to(ConnectionStage::ActiveStreaming);
        assert!(usb.is_streaming());

        let wifi = TransportEndpoint::new_wifi("00008150-000202D02299401C", "192.168.100.38");
        assert_eq!(wifi.mode, TransportMode::WifiCoreDevice);
        assert_eq!(wifi.host, "192.168.100.38");

        assert!(verify_transport_ready(TransportMode::CableUsb, "127.0.0.1", 62078).is_ok());
        assert!(verify_transport_ready(TransportMode::WifiCoreDevice, "192.168.100.38", 62078).is_ok());
        assert!(verify_transport_ready(TransportMode::CableUsb, "", 62078).is_err());
    }
}
