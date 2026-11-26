//! Apple device models dictionary, screen profile detection, and native thread-safe metadata cache.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use idevice::{
    lockdown::LockdownClient,
    provider::IdeviceProvider as _,
    usbmuxd::{Connection, UsbmuxdAddr},
};

use crate::link::{block_on, read_info, usbmux};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenType {
    DynamicIsland,
    Notch,
    HomeButton,
    IPad,
}

impl ScreenType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DynamicIsland => "DynamicIsland",
            Self::Notch => "Notch",
            Self::HomeButton => "HomeButton",
            Self::IPad => "iPad",
        }
    }
}

const PRODUCT_TYPES: &[(&str, &str)] = &[
    ("iPhone10,3", "iPhone X"),
    ("iPhone10,6", "iPhone X"),
    ("iPhone11,2", "iPhone XS"),
    ("iPhone11,4", "iPhone XS Max"),
    ("iPhone11,6", "iPhone XS Max"),
    ("iPhone11,8", "iPhone XR"),
    ("iPhone12,1", "iPhone 11"),
    ("iPhone12,3", "iPhone 11 Pro"),
    ("iPhone12,5", "iPhone 11 Pro Max"),
    ("iPhone12,8", "iPhone SE (2nd gen)"),
    ("iPhone13,1", "iPhone 12 mini"),
    ("iPhone13,2", "iPhone 12"),
    ("iPhone13,3", "iPhone 12 Pro"),
    ("iPhone13,4", "iPhone 12 Pro Max"),
    ("iPhone14,2", "iPhone 13 Pro"),
    ("iPhone14,3", "iPhone 13 Pro Max"),
    ("iPhone14,4", "iPhone 13 mini"),
    ("iPhone14,5", "iPhone 13"),
    ("iPhone14,6", "iPhone SE (3rd gen)"),
    ("iPhone14,7", "iPhone 14"),
    ("iPhone14,8", "iPhone 14 Plus"),
    ("iPhone15,2", "iPhone 14 Pro"),
    ("iPhone15,3", "iPhone 14 Pro Max"),
    ("iPhone15,4", "iPhone 15"),
    ("iPhone15,5", "iPhone 15 Plus"),
    ("iPhone16,1", "iPhone 15 Pro"),
    ("iPhone16,2", "iPhone 15 Pro Max"),
    ("iPhone17,1", "iPhone 16 Pro"),
    ("iPhone17,2", "iPhone 16 Pro Max"),
    ("iPhone17,3", "iPhone 16"),
    ("iPhone17,4", "iPhone 16 Plus"),
    ("iPhone17,5", "iPhone 16e"),
    ("iPhone18,1", "iPhone 17 Pro"),
    ("iPhone18,2", "iPhone 17 Pro Max"),
    ("iPhone18,3", "iPhone 17"),
    ("iPhone18,4", "iPhone 17 Air"),
    ("iPad13,18", "iPad (10th gen)"),
    ("iPad13,19", "iPad (10th gen)"),
    ("iPad15,7", "iPad (11th gen)"),
    ("iPad15,8", "iPad (11th gen)"),
    ("iPad14,1", "iPad mini (6th gen)"),
    ("iPad14,2", "iPad mini (6th gen)"),
    ("iPad16,1", "iPad mini (A17 Pro)"),
    ("iPad16,2", "iPad mini (A17 Pro)"),
    ("iPad14,8", "iPad Air 11-inch (M2)"),
    ("iPad14,9", "iPad Air 11-inch (M2)"),
    ("iPad14,10", "iPad Air 13-inch (M2)"),
    ("iPad14,11", "iPad Air 13-inch (M2)"),
    ("iPad15,3", "iPad Air 11-inch (M3)"),
    ("iPad15,4", "iPad Air 11-inch (M3)"),
    ("iPad15,5", "iPad Air 13-inch (M3)"),
    ("iPad15,6", "iPad Air 13-inch (M3)"),
    ("iPad16,3", "iPad Pro 11-inch (M4)"),
    ("iPad16,4", "iPad Pro 11-inch (M4)"),
    ("iPad16,5", "iPad Pro 13-inch (M4)"),
    ("iPad16,6", "iPad Pro 13-inch (M4)"),
];

const IPHONE_CPID: &[(&str, &str)] = &[
    ("8020", "iPhone XS / XR"),
    ("8030", "iPhone 11 series"),
    ("8101", "iPhone 12 series"),
    ("8110", "iPhone 13 / 14"),
    ("8120", "iPhone 15 / 14 Pro"),
    ("8130", "iPhone 15 Pro"),
    ("8140", "iPhone 16 series"),
    ("8150", "iPhone 17 series"),
];

const IPAD_CPID: &[(&str, &str)] = &[
    ("8020", "iPad (8th gen)"),
    ("8030", "iPad (9th gen)"),
    ("8101", "iPad (10th gen)"),
    ("8103", "iPad Pro / Air (M1)"),
    ("8110", "iPad mini (6th gen)"),
    ("8112", "iPad Pro / Air (M2)"),
    ("8120", "iPad (11th gen)"),
    ("8122", "iPad Air (M3)"),
    ("8130", "iPad mini (A17 Pro)"),
    ("8132", "iPad Pro (M4)"),
];

/// Resolve marketing name of Apple ProductType (e.g. "iPhone18,1" -> "iPhone 17 Pro").
pub fn resolve_apple_model(product_type: &str) -> Option<&'static str> {
    PRODUCT_TYPES
        .iter()
        .find(|(k, _)| *k == product_type)
        .map(|(_, v)| *v)
}

/// Classify screen cutout / aspect type from ProductType.
pub fn screen_type_for_model(product_type: &str) -> ScreenType {
    if product_type.starts_with("iPad") {
        return ScreenType::IPad;
    }
    match product_type {
        "iPhone14,6" | "iPhone12,8" | "iPhone8,1" | "iPhone8,2" | "iPhone8,4" => {
            ScreenType::HomeButton
        }
        "iPhone15,2" | "iPhone15,3" => ScreenType::DynamicIsland,
        _ if product_type.starts_with("iPhone16,")
            || product_type.starts_with("iPhone17,")
            || product_type.starts_with("iPhone18,") =>
        {
            ScreenType::DynamicIsland
        }
        _ => ScreenType::Notch,
    }
}

/// Derive series name from UDID and device family.
pub fn series_name_from_udid(udid: &str, family: &str) -> String {
    let id = udid.replace('-', "");
    let cpid = if id.len() >= 8 && id.starts_with("0000") {
        id[4..8].to_uppercase()
    } else {
        String::new()
    };
    let table = match family {
        "iPad" | "ipad" => IPAD_CPID,
        "iPhone" | "iphone" => IPHONE_CPID,
        _ => &[],
    };
    table
        .iter()
        .find(|(k, _)| *k == cpid)
        .map(|(_, v)| v.to_string())
        .unwrap_or_else(|| family.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceMetadata {
    pub udid: String,
    pub device_id: u32,
    pub connection_type: String,
    pub device_name: String,
    pub product_type: String,
    pub marketing_model: String,
    pub product_version: String,
    pub build_version: String,
    pub device_class: String,
    pub wifi_address: String,
    pub bluetooth_address: String,
    pub screen_type: ScreenType,
    pub paired: bool,
}

const METADATA_CACHE_TTL: Duration = Duration::from_millis(3000);
static METADATA_CACHE: OnceLock<Mutex<HashMap<String, (DeviceMetadata, Instant)>>> = OnceLock::new();

fn metadata_cache() -> &'static Mutex<HashMap<String, (DeviceMetadata, Instant)>> {
    METADATA_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn clear_metadata_cache(udid: Option<&str>) {
    if let Ok(mut g) = metadata_cache().lock() {
        match udid {
            Some(u) if !u.is_empty() => {
                g.remove(u);
            }
            _ => g.clear(),
        }
    }
}

pub fn cache_device_metadata(meta: DeviceMetadata) {
    if let Ok(mut g) = metadata_cache().lock() {
        g.insert(meta.udid.clone(), (meta, Instant::now()));
    }
}

pub fn get_cached_device_metadata(udid: &str) -> Option<DeviceMetadata> {
    let g = metadata_cache().lock().ok()?;
    g.get(udid).and_then(|(m, ts)| {
        if ts.elapsed() < METADATA_CACHE_TTL {
            Some(m.clone())
        } else {
            None
        }
    })
}

/// Zero-I/O RAM snapshot of discovered devices for Zero-Polling Guard when streaming/audio is active.
pub fn snapshot_cached_devices_metadata_zero_io() -> Vec<DeviceMetadata> {
    let Ok(g) = metadata_cache().lock() else {
        return Vec::new();
    };
    g.values().map(|(m, _)| m.clone()).collect()
}

/// Discovers connected USB/Network Apple devices using `idevice::usbmuxd::UsbmuxdConnection`
/// and enriches them with Lockdown metadata (cached in memory for `< 0.2 ms` repeat reads).
pub fn discover_devices_with_metadata(force_refresh: bool) -> Vec<DeviceMetadata> {
    if force_refresh {
        clear_metadata_cache(None);
    }

    block_on(async move {
        let Ok(mut mux) = usbmux().await else {
            return Vec::new();
        };
        let Ok(devs) = mux.get_devices().await else {
            return Vec::new();
        };
        let addr = UsbmuxdAddr::from_env_var().unwrap_or_default();
        let mut results = Vec::new();

        for d in devs {
            let conn_str = match &d.connection_type {
                Connection::Usb => "USB".to_string(),
                Connection::Network(_) => "Network".to_string(),
                Connection::Unknown(s) => s.clone(),
            };

            if !force_refresh {
                if let Some(mut cached) = get_cached_device_metadata(&d.udid) {
                    cached.device_id = d.device_id;
                    cached.connection_type = conn_str;
                    results.push(cached);
                    continue;
                }
            }

            let paired = mux.get_pair_record(&d.udid).await.is_ok()
                || idevice::provider::PairingFile::load_from_lockdown_dir(&d.udid).is_some();
            let provider = d.to_provider(addr.clone(), "iphone-link");
            let ld_info = tokio::time::timeout(Duration::from_millis(2000), async {
                let Ok(mut ld) = LockdownClient::connect(&provider).await else {
                    return crate::link::Info::default();
                };
                // Read baseline metadata before StartSession (ProductType is always readable in plaintext)
                let pre = read_info(&mut ld).await;
                let pairing_opt = provider
                    .get_pairing_file()
                    .await
                    .ok()
                    .or_else(|| idevice::provider::PairingFile::load_from_lockdown_dir(&d.udid));
                if let Some(pf) = pairing_opt {
                    if ld.start_session(&pf).await.is_ok() {
                        let post = read_info(&mut ld).await;
                        return crate::link::Info {
                            name: if post.name.is_empty() { pre.name } else { post.name },
                            product_type: if post.product_type.is_empty() {
                                pre.product_type
                            } else {
                                post.product_type
                            },
                            version: if post.version.is_empty() {
                                pre.version
                            } else {
                                post.version
                            },
                            build_version: if post.build_version.is_empty() {
                                pre.build_version
                            } else {
                                post.build_version
                            },
                            device_class: if post.device_class.is_empty() {
                                pre.device_class
                            } else {
                                post.device_class
                            },
                            wifi_address: if post.wifi_address.is_empty() {
                                pre.wifi_address
                            } else {
                                post.wifi_address
                            },
                            bluetooth_address: if post.bluetooth_address.is_empty() {
                                pre.bluetooth_address
                            } else {
                                post.bluetooth_address
                            },
                        };
                    }
                }
                pre
            })
            .await
            .unwrap_or_default();

            let family = if ld_info.device_class.eq_ignore_ascii_case("iPad") {
                "iPad"
            } else {
                "iPhone"
            };
            let marketing_model = resolve_apple_model(&ld_info.product_type)
                .map(str::to_string)
                .unwrap_or_else(|| series_name_from_udid(&d.udid, family));
            let screen_type = if !ld_info.product_type.is_empty() {
                screen_type_for_model(&ld_info.product_type)
            } else if family == "iPad" {
                ScreenType::IPad
            } else if d.udid.contains("8150") {
                ScreenType::DynamicIsland
            } else {
                ScreenType::Notch
            };

            let meta = DeviceMetadata {
                udid: d.udid.clone(),
                device_id: d.device_id,
                connection_type: conn_str,
                device_name: if ld_info.name.is_empty() {
                    marketing_model.clone()
                } else {
                    ld_info.name
                },
                product_type: ld_info.product_type,
                marketing_model,
                product_version: ld_info.version,
                build_version: ld_info.build_version,
                device_class: if ld_info.device_class.is_empty() {
                    family.to_string()
                } else {
                    ld_info.device_class
                },
                wifi_address: ld_info.wifi_address,
                bluetooth_address: ld_info.bluetooth_address,
                screen_type,
                paired,
            };
            if !meta.device_name.is_empty()
                || !meta.product_type.is_empty()
                || !meta.bluetooth_address.is_empty()
            {
                let now_ts = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                crate::wifi::update_device_full_metadata(
                    &meta.udid,
                    Some(meta.device_name.as_str()),
                    Some(meta.product_type.as_str()),
                    Some(meta.marketing_model.as_str()),
                    Some(meta.product_version.as_str()),
                    Some(meta.bluetooth_address.as_str()),
                    None,
                    Some(now_ts),
                    None,
                );
            }
            cache_device_metadata(meta.clone());
            results.push(meta);
        }

        results
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_models() {
        assert_eq!(resolve_apple_model("iPhone18,1"), Some("iPhone 17 Pro"));
        assert_eq!(resolve_apple_model("iPhone18,2"), Some("iPhone 17 Pro Max"));
        assert_eq!(resolve_apple_model("iPhone18,3"), Some("iPhone 17"));
        assert_eq!(resolve_apple_model("iPhone18,4"), Some("iPhone 17 Air"));
        assert_eq!(resolve_apple_model("iPhone12,5"), Some("iPhone 11 Pro Max"));
        assert_eq!(resolve_apple_model("iPhone15,2"), Some("iPhone 14 Pro"));
        assert_eq!(resolve_apple_model("iPad15,7"), Some("iPad (11th gen)"));
        assert_eq!(resolve_apple_model("Unknown99,1"), None);
    }

    #[test]
    fn test_screen_types() {
        assert_eq!(screen_type_for_model("iPhone18,1"), ScreenType::DynamicIsland);
        assert_eq!(screen_type_for_model("iPhone12,5"), ScreenType::Notch);
        assert_eq!(screen_type_for_model("iPhone14,6"), ScreenType::HomeButton);
        assert_eq!(screen_type_for_model("iPad16,3"), ScreenType::IPad);
    }

    #[test]
    fn test_series_from_udid() {
        assert_eq!(
            series_name_from_udid("00008150-0000000000000002", "iPhone"),
            "iPhone 17 series"
        );
        assert_eq!(
            series_name_from_udid("00008030-001234567890ABCD", "iPhone"),
            "iPhone 11 series"
        );
    }

    #[test]
    fn test_metadata_cache_submillisecond_read() {
        clear_metadata_cache(None);
        let sample = DeviceMetadata {
            udid: "00008150-CACHEUNIT".into(),
            device_id: 1,
            connection_type: "USB".into(),
            device_name: "iPhone Test Device".into(),
            product_type: "iPhone18,1".into(),
            marketing_model: "iPhone 17 Pro".into(),
            product_version: "27.0.1".into(),
            build_version: "24A335".into(),
            device_class: "iPhone".into(),
            wifi_address: "AA:BB:CC:DD:EE:FF".into(),
            bluetooth_address: "02:00:00:00:00:02".into(),
            screen_type: ScreenType::DynamicIsland,
            paired: true,
        };
        cache_device_metadata(sample.clone());
        let t0 = Instant::now();
        let hit = get_cached_device_metadata("00008150-CACHEUNIT");
        let elapsed = t0.elapsed();
        assert_eq!(hit, Some(sample));
        assert!(elapsed < Duration::from_millis(2), "cache hit took {:?}", elapsed);
    }
}
