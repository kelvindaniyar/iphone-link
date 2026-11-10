//! Apple device models dictionary and screen profile detection.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenType {
    DynamicIsland,
    Notch,
    HomeButton,
    IPad,
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
    ("iPhone18,4", "iPhone Air"),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_models() {
        assert_eq!(resolve_apple_model("iPhone18,1"), Some("iPhone 17 Pro"));
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
            series_name_from_udid("00008150-000202D02299401C", "iPhone"),
            "iPhone 17 series"
        );
        assert_eq!(
            series_name_from_udid("00008030-001234567890ABCD", "iPhone"),
            "iPhone 11 series"
        );
    }
}
