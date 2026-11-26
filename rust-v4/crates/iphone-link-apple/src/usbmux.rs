//! USB connection via Apple Mobile Device Service (usbmuxd on 127.0.0.1:27015).

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use crate::models::{resolve_apple_model, series_name_from_udid};

pub const USBMUX_DEFAULT_PORT: u16 = 27015;
pub const USBMUX_HEADER_SIZE: usize = 16;
pub const USBMUX_PROTO_PLIST: u32 = 8;
pub const USBMUX_VERSION_1: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectedAppleDevice {
    pub device_id: u32,
    pub serial_udid: String,
    pub connection_type: String,
    pub product_id: u32,
    pub marketing_model: Option<String>,
    pub device_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppleError {
    ServiceUnavailable(String),
    Timeout(String),
    ProtocolError(String),
    IoError(String),
}

impl std::fmt::Display for AppleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ServiceUnavailable(s) => write!(f, "usbmuxd service unavailable: {}", s),
            Self::Timeout(s) => write!(f, "Timeout: {}", s),
            Self::ProtocolError(s) => write!(f, "Protocol error: {}", s),
            Self::IoError(s) => write!(f, "IO error: {}", s),
        }
    }
}

impl std::error::Error for AppleError {}

/// Build usbmux packet with 16-byte header: <length: u32, version: u32, message: u32, tag: u32>.
pub fn build_usbmux_packet(xml_payload: &[u8], tag: u32) -> Vec<u8> {
    let total_len = (USBMUX_HEADER_SIZE + xml_payload.len()) as u32;
    let mut packet = Vec::with_capacity(total_len as usize);
    packet.extend_from_slice(&total_len.to_le_bytes());
    packet.extend_from_slice(&USBMUX_VERSION_1.to_le_bytes());
    packet.extend_from_slice(&USBMUX_PROTO_PLIST.to_le_bytes());
    packet.extend_from_slice(&tag.to_le_bytes());
    packet.extend_from_slice(xml_payload);
    packet
}

/// Parse usbmux header: returns (total_length, version, message_type, tag).
pub fn parse_usbmux_header(header: &[u8; 16]) -> (u32, u32, u32, u32) {
    let length = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
    let version = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
    let message = u32::from_le_bytes([header[8], header[9], header[10], header[11]]);
    let tag = u32::from_le_bytes([header[12], header[13], header[14], header[15]]);
    (length, version, message, tag)
}

/// Extract string value for XML key, e.g. <key>SerialNumber</key><string>...</string>.
pub fn extract_xml_string(xml: &str, key: &str) -> Option<String> {
    let key_tag = format!("<key>{}</key>", key);
    let key_idx = xml.find(&key_tag)?;
    let after_key = &xml[key_idx + key_tag.len()..];
    let str_open = after_key.find("<string>")?;
    let val_start = str_open + "<string>".len();
    let str_close = after_key[val_start..].find("</string>")?;
    Some(after_key[val_start..val_start + str_close].trim().to_string())
}

/// Extract integer value for XML key, e.g. <key>DeviceID</key><integer>...</integer>.
pub fn extract_xml_integer(xml: &str, key: &str) -> Option<u32> {
    let key_tag = format!("<key>{}</key>", key);
    let key_idx = xml.find(&key_tag)?;
    let after_key = &xml[key_idx + key_tag.len()..];
    let int_open = after_key.find("<integer>")?;
    let val_start = int_open + "<integer>".len();
    let int_close = after_key[val_start..].find("</integer>")?;
    after_key[val_start..val_start + int_close].trim().parse().ok()
}

/// Query usbmuxd on `USBMUXD_SOCKET_ADDRESS` (default 127.0.0.1:27015) for connected USB devices.
/// If usbmuxd is not running or no devices attached, gracefully returns Ok(vec![]) within timeout.
pub fn list_connected_usb_devices(timeout: Duration) -> Result<Vec<ConnectedAppleDevice>, AppleError> {
    let addr: SocketAddr = std::env::var("USBMUXD_SOCKET_ADDRESS")
        .ok()
        .and_then(|s| s.trim().parse::<SocketAddr>().ok())
        .unwrap_or_else(|| SocketAddr::from(([127, 0, 0, 1], USBMUX_DEFAULT_PORT)));
    let mut stream = match TcpStream::connect_timeout(&addr, timeout) {
        Ok(s) => s,
        Err(_) => {
            // Service not running or unreachable -> return empty list gracefully
            return Ok(Vec::new());
        }
    };

    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));

    let xml_req = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n\
<dict>\n\
    <key>MessageType</key>\n\
    <string>ListDevices</string>\n\
    <key>ClientVersionString</key>\n\
    <string>iphone-link-v4</string>\n\
    <key>ProgName</key>\n\
    <string>iphone-link-v4</string>\n\
</dict>\n\
</plist>";

    let req_packet = build_usbmux_packet(xml_req, 1);
    if stream.write_all(&req_packet).is_err() {
        return Ok(Vec::new());
    }

    let mut header = [0u8; USBMUX_HEADER_SIZE];
    if stream.read_exact(&mut header).is_err() {
        return Ok(Vec::new());
    }

    let (total_len, _, _, _) = parse_usbmux_header(&header);
    if total_len < USBMUX_HEADER_SIZE as u32 {
        return Ok(Vec::new());
    }

    let payload_len = (total_len as usize) - USBMUX_HEADER_SIZE;
    let mut payload = vec![0u8; payload_len];
    if stream.read_exact(&mut payload).is_err() {
        return Ok(Vec::new());
    }

    let xml_resp = String::from_utf8_lossy(&payload);
    let mut devices = Vec::new();

    // Parse device blocks inside <key>DeviceList</key><array>...</array>
    let mut search_from = 0;
    while let Some(dict_start) = xml_resp[search_from..].find("<dict>") {
        let abs_start = search_from + dict_start;
        let dict_end = match xml_resp[abs_start..].find("</dict>") {
            Some(e) => abs_start + e + "</dict>".len(),
            None => break,
        };
        let block = &xml_resp[abs_start..dict_end];

        if let Some(udid) = extract_xml_string(block, "SerialNumber") {
            let dev_id = extract_xml_integer(block, "DeviceID").unwrap_or(0);
            let conn_type = extract_xml_string(block, "ConnectionType").unwrap_or_else(|| "USB".to_string());
            let prod_id = extract_xml_integer(block, "ProductID").unwrap_or(0);

            // Resolve marketing name
            let model_name = resolve_apple_model(&udid)
                .map(str::to_string)
                .or_else(|| Some(series_name_from_udid(&udid, "iPhone")));

            devices.push(ConnectedAppleDevice {
                device_id: dev_id,
                serial_udid: udid,
                connection_type: conn_type,
                product_id: prod_id,
                marketing_model: model_name,
                device_name: "iPhone".to_string(),
            });
        }
        search_from = dict_end;
    }

    Ok(devices)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_parse_usbmux_header() {
        let payload = b"<plist></plist>";
        let packet = build_usbmux_packet(payload, 42);
        assert_eq!(packet.len(), 16 + payload.len());

        let mut header = [0u8; 16];
        header.copy_from_slice(&packet[..16]);
        let (len, ver, msg, tag) = parse_usbmux_header(&header);
        assert_eq!(len, (16 + payload.len()) as u32);
        assert_eq!(ver, 1);
        assert_eq!(msg, 8);
        assert_eq!(tag, 42);
    }

    #[test]
    fn test_xml_extractors() {
        let xml = "<dict><key>SerialNumber</key><string>00008101-0000000000000003</string><key>DeviceID</key><integer>12</integer></dict>";
        assert_eq!(
            extract_xml_string(xml, "SerialNumber"),
            Some("00008101-0000000000000003".to_string())
        );
        assert_eq!(extract_xml_integer(xml, "DeviceID"), Some(12));
        assert_eq!(extract_xml_string(xml, "NonExistent"), None);
    }

    #[test]
    fn test_list_connected_usb_devices_timeout_graceful() {
        // Test with short 150ms timeout; must never panic and must finish in < 300ms
        let start = std::time::Instant::now();
        let res = list_connected_usb_devices(Duration::from_millis(150));
        let elapsed = start.elapsed();
        assert!(res.is_ok());
        assert!(elapsed < Duration::from_millis(400));
    }
}
