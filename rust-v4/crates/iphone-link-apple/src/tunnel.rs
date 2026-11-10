//! CoreDevice RSD Tunnel connection manager and socket optimizer.

use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

pub const COREDEVICE_TUNNEL_PORT: u16 = 62078;
pub const DISPLAY_SERVICE: &str = "com.apple.coredevice.displayservice";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelKind {
    UsbCoreDeviceProxy,
    WifiTlsPsk,
}

#[derive(Debug)]
pub struct RsdTunnel {
    pub kind: TunnelKind,
    pub target: SocketAddr,
    pub stream: Option<TcpStream>,
    pub is_connected: bool,
}

impl RsdTunnel {
    pub fn new(kind: TunnelKind, target: SocketAddr) -> Self {
        Self {
            kind,
            target,
            stream: None,
            is_connected: false,
        }
    }

    /// Connect to RSD tunnel endpoint with TCP_NODELAY enabled for zero latency.
    pub fn connect(&mut self, timeout: Duration) -> Result<(), String> {
        let stream = TcpStream::connect_timeout(&self.target, timeout)
            .map_err(|e| format!("Failed to connect RSD tunnel to {}: {}", self.target, e))?;

        // Disable Nagle algorithm to guarantee instant packet transmission for Wi-Fi and USB
        stream
            .set_nodelay(true)
            .map_err(|e| format!("Failed to set TCP_NODELAY: {}", e))?;

        stream
            .set_read_timeout(Some(timeout))
            .map_err(|e| format!("Failed to set read timeout: {}", e))?;

        self.stream = Some(stream);
        self.is_connected = true;
        Ok(())
    }

    /// Close the tunnel and release socket resources.
    pub fn close(&mut self) {
        self.stream = None;
        self.is_connected = false;
    }
}

/// Handshake message representation for RSD services.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RsdServiceHandshake {
    pub service_name: String,
    pub protocol_version: u32,
    pub client_features: u64,
}

impl RsdServiceHandshake {
    pub fn display_service() -> Self {
        Self {
            service_name: DISPLAY_SERVICE.to_string(),
            protocol_version: 1,
            client_features: 140, // Standard CoreDevice display client feature mask
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rsd_handshake_creation() {
        let hs = RsdServiceHandshake::display_service();
        assert_eq!(hs.service_name, DISPLAY_SERVICE);
        assert_eq!(hs.client_features, 140);
        assert_eq!(hs.protocol_version, 1);
    }

    #[test]
    fn test_tunnel_lifecycle() {
        let addr = "127.0.0.1:62078".parse().unwrap();
        let mut tunnel = RsdTunnel::new(TunnelKind::UsbCoreDeviceProxy, addr);
        assert!(!tunnel.is_connected);
        assert_eq!(tunnel.kind, TunnelKind::UsbCoreDeviceProxy);
        tunnel.close();
        assert!(!tunnel.is_connected);
    }
}
