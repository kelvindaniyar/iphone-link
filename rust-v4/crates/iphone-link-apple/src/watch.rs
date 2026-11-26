//! Hotplug USB watcher (`UsbmuxdConnection`) and SpringBoard orientation/lock watchers.

use std::collections::HashSet;
use std::time::Duration;

use idevice::{
    notification_proxy::NotificationProxyClient,
    rsd::RsdHandshake,
    springboardservices::{InterfaceOrientation, SpringBoardServicesClient},
    tcp::handle::AdapterHandle,
    usbmuxd::Connection,
};

use crate::link::{block_on, usbmux};

pub const LOCK_NOTIFICATION: &str = "com.apple.springboard.lockcomplete";

/// Clockwise rotation degrees to orient a portrait iOS stream upright.
pub fn rotation(o: &InterfaceOrientation) -> Option<u32> {
    match o {
        InterfaceOrientation::Portrait => Some(0),
        InterfaceOrientation::PortraitUpsideDown => Some(180),
        InterfaceOrientation::LandscapeRight => Some(270),
        InterfaceOrientation::LandscapeLeft => Some(90),
        InterfaceOrientation::Unknown => None,
    }
}

/// Polls SpringBoard interface orientation over an active RSD tunnel.
pub async fn orientation(
    mut adapter: AdapterHandle,
    mut rsd: RsdHandshake,
    mut on: impl FnMut(u32),
) {
    let mut last = None;
    loop {
        let mut sb = match SpringBoardServicesClient::connect_rsd(&mut adapter, &mut rsd).await {
            Ok(sb) => sb,
            Err(_) => {
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
        };
        while let Ok(o) = sb.get_interface_orientation().await {
            if let Some(r) = rotation(&o).filter(|r| Some(*r) != last) {
                last = Some(r);
                on(r);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// Observes `com.apple.springboard.lockcomplete` notifications over an active RSD tunnel.
pub async fn lock(mut adapter: AdapterHandle, mut rsd: RsdHandshake, mut on: impl FnMut()) {
    loop {
        if let Ok(mut np) = NotificationProxyClient::connect_rsd(&mut adapter, &mut rsd).await {
            if np.observe_notification(LOCK_NOTIFICATION).await.is_ok() {
                while let Ok(name) = np.receive_notification().await {
                    if name == LOCK_NOTIFICATION {
                        on();
                    }
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsbHotplugEvent {
    Connected(String),
    Disconnected(String),
}

/// Lightweight differential hotplug detector backed by `UsbmuxdConnection`.
#[derive(Debug, Default)]
pub struct UsbHotplugWatcher {
    known_usb_udids: HashSet<String>,
}

impl UsbHotplugWatcher {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn diff_udids(&mut self, current: Vec<String>) -> Vec<UsbHotplugEvent> {
        let cur_set: HashSet<String> = current.into_iter().collect();
        let mut events = Vec::new();
        for udid in &cur_set {
            if !self.known_usb_udids.contains(udid) {
                events.push(UsbHotplugEvent::Connected(udid.clone()));
            }
        }
        for old in &self.known_usb_udids {
            if !cur_set.contains(old) {
                events.push(UsbHotplugEvent::Disconnected(old.clone()));
            }
        }
        self.known_usb_udids = cur_set;
        events
    }

    pub fn poll_once(&mut self) -> Vec<UsbHotplugEvent> {
        let current = block_on(async {
            let Ok(mut mux) = usbmux().await else {
                return Vec::new();
            };
            mux.get_devices()
                .await
                .unwrap_or_default()
                .into_iter()
                .filter(|d| d.connection_type == Connection::Usb)
                .map(|d| d.udid)
                .collect()
        });
        self.diff_udids(current)
    }

    pub fn current_udids(&self) -> Vec<String> {
        let mut out: Vec<String> = self.known_usb_udids.iter().cloned().collect();
        out.sort();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_springboard_rotation_mapping() {
        assert_eq!(rotation(&InterfaceOrientation::Portrait), Some(0));
        assert_eq!(rotation(&InterfaceOrientation::LandscapeLeft), Some(90));
        assert_eq!(rotation(&InterfaceOrientation::PortraitUpsideDown), Some(180));
        assert_eq!(rotation(&InterfaceOrientation::LandscapeRight), Some(270));
        assert_eq!(rotation(&InterfaceOrientation::Unknown), None);
    }

    #[test]
    fn test_usb_hotplug_watcher_diff() {
        let mut w = UsbHotplugWatcher::new();
        let e1 = w.diff_udids(vec!["U1".into(), "U2".into()]);
        assert_eq!(e1.len(), 2);
        let e2 = w.diff_udids(vec!["U2".into(), "U3".into()]);
        assert!(e2.contains(&UsbHotplugEvent::Connected("U3".into())));
        assert!(e2.contains(&UsbHotplugEvent::Disconnected("U1".into())));
    }
}
