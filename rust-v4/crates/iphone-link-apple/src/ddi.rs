//! Developer Disk Image (DDI) & Cryptex1 finder, validator, reader, and 1x auto-downloader
//! for iOS 17–27+.
//!
//! Layout matching Apple CoreDevice / `doronz88/DeveloperDiskImage`:
//! - iOS 27 and later: `Kind::Cryptex` (`Xcode_iOS_DDI_Cryptex`) installed over RemoteXPC `cryptexd`.
//! - iOS 17 to 26: `Kind::Personalized` (`Xcode_iOS_DDI_Personalized`) mounted over `mobile_image_mounter`.
//!
//! Cache directory (neutral & permanent): `%LOCALAPPDATA%\iPhoneLink\ddi\<folder>`.

use std::path::{Path, PathBuf};

/// Upstream repository base URL for Apple Developer Disk Images (`doronz88/DeveloperDiskImage`).
/// Can be overridden at runtime or in offline tests via `IPHONE_LINK_DDI_BASE_URL`.
pub const DEFAULT_DDI_BASE_URL: &str =
    "https://raw.githubusercontent.com/doronz88/DeveloperDiskImage/main";

/// Environment variable overriding the local DDI directory (used for custom caches & offline tests).
pub const ENV_DDI_DIR: &str = "IPHONE_LINK_DDI_DIR";

/// Environment variable overriding the DDI download base URL (used for offline Mock M-3 tests).
pub const ENV_DDI_BASE_URL: &str = "IPHONE_LINK_DDI_BASE_URL";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Personalized,
    Cryptex,
}

impl Kind {
    /// Returns `None` below iOS 17 (legacy signed DMG not used by CoreDevice).
    pub fn for_version(product_version: &str) -> Option<Self> {
        let major: u32 = product_version.split('.').next()?.trim().parse().ok()?;
        match major {
            0..=16 => None,
            17..=26 => Some(Kind::Personalized),
            _ => Some(Kind::Cryptex),
        }
    }

    pub fn folder(self) -> &'static str {
        match self {
            Kind::Personalized => "Xcode_iOS_DDI_Personalized",
            Kind::Cryptex => "Xcode_iOS_DDI_Cryptex",
        }
    }

    pub fn files(self) -> &'static [&'static str] {
        match self {
            Kind::Personalized => &["BuildManifest.plist", "Image.dmg", "Image.dmg.trustcache"],
            Kind::Cryptex => &[
                "BuildManifest.plist",
                "Image.dmg",
                "Image.dmg.trustcache",
                "Image.dmg.cryptex_info",
                "Image.dmg.root_hash",
            ],
        }
    }

    pub fn label_id(self) -> &'static str {
        match self {
            Kind::Personalized => "Developer Disk Image (iOS 17–26)",
            Kind::Cryptex => "Developer Disk Image Cryptex (iOS 27+)",
        }
    }
}

/// Primary writable cache directory for downloaded DDI assets:
/// `IPHONE_LINK_DDI_DIR\<folder>` if override is set, otherwise `%LOCALAPPDATA%\iPhoneLink\ddi\<folder>`.
pub fn cache_dir(kind: Kind) -> Result<PathBuf, String> {
    if let Some(override_dir) = std::env::var_os(ENV_DDI_DIR) {
        let s = override_dir.to_string_lossy();
        if !s.trim().is_empty() {
            return Ok(PathBuf::from(s.trim()).join(kind.folder()));
        }
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        return Ok(PathBuf::from(local)
            .join("iPhoneLink")
            .join("ddi")
            .join(kind.folder()));
    }
    if let Some(home) = std::env::var_os("USERPROFILE") {
        return Ok(PathBuf::from(home)
            .join("AppData")
            .join("Local")
            .join("iPhoneLink")
            .join("ddi")
            .join(kind.folder()));
    }
    Err("LOCALAPPDATA / USERPROFILE tidak tersedia untuk menyimpan cache DDI.".into())
}

/// Candidate directories checked for DDI assets in strict priority order (first match wins):
/// 1. `IPHONE_LINK_DDI_DIR` override (`<dir>/<folder>` and `<dir>`)
/// 2. `%LOCALAPPDATA%\iPhoneLink\ddi\<folder>` (neutral permanent cache)
/// 3. `%USERPROFILE%\.pymobiledevice3\<folder>` (read-only fallback if user already has it)
pub fn search_dirs(kind: Kind) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let has_override = if let Some(override_dir) = std::env::var_os(ENV_DDI_DIR) {
        let s = override_dir.to_string_lossy();
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            let base = PathBuf::from(trimmed);
            dirs.push(base.join(kind.folder()));
            dirs.push(base);
            true
        } else {
            false
        }
    } else {
        false
    };

    if !has_override {
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            dirs.push(
                PathBuf::from(local)
                    .join("iPhoneLink")
                    .join("ddi")
                    .join(kind.folder()),
            );
        }
        if let Some(home) = std::env::var_os("USERPROFILE") {
            dirs.push(
                PathBuf::from(home)
                    .join(".pymobiledevice3")
                    .join(kind.folder()),
            );
        }
    }
    dirs
}

fn is_non_empty_file(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.len() > 0)
        .unwrap_or(false)
}

/// Returns `true` if `dir` contains every required non-empty file for `kind`
/// AND `BuildManifest.plist` parses cleanly as a plist dictionary.
pub fn is_valid_ddi_dir(kind: Kind, dir: &Path) -> bool {
    if !kind.files().iter().all(|f| is_non_empty_file(&dir.join(f))) {
        return false;
    }
    plist::from_file(dir.join("BuildManifest.plist"))
        .ok()
        .and_then(|v| v.into_dictionary())
        .is_some()
}

/// Returns the first directory in `dirs` containing every required non-empty file for `kind`.
pub fn find(kind: Kind, dirs: &[PathBuf]) -> Option<PathBuf> {
    dirs.iter()
        .find(|d| kind.files().iter().all(|f| is_non_empty_file(&d.join(f))))
        .cloned()
}

/// Returns the first directory in `dirs` containing a complete and valid DDI bundle
/// (every file non-empty + valid `BuildManifest.plist`).
pub fn find_valid(kind: Kind, dirs: &[PathBuf]) -> Option<PathBuf> {
    dirs.iter().find(|d| is_valid_ddi_dir(kind, d)).cloned()
}

/// Resolves the DDI base download URL (`IPHONE_LINK_DDI_BASE_URL` or `DEFAULT_DDI_BASE_URL`).
pub fn ddi_base_url() -> String {
    if let Ok(custom) = std::env::var(ENV_DDI_BASE_URL) {
        let trimmed = custom.trim().trim_end_matches('/');
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    DEFAULT_DDI_BASE_URL.to_string()
}

/// Ensures all required DDI files for `kind` exist locally and are valid.
///
/// - If already present in `search_dirs(kind)` (valid `BuildManifest.plist` + non-empty assets),
///   returns that directory immediately without any network activity.
/// - Otherwise downloads missing/invalid files atomically (`.part` -> `fs::rename`) via pure
///   Win32 WinHTTP (`winhttp.dll`) into `cache_dir(kind)` (`%LOCALAPPDATA%\iPhoneLink\ddi\<folder>`).
/// - Progress callback receives `(percent: 10..=70, status_text)`.
/// - On failure (e.g., offline), leaves zero `.part` files behind and returns a clear bilingual error.
pub fn ensure_ddi_files_downloaded<F>(kind: Kind, mut on_progress: F) -> Result<PathBuf, String>
where
    F: FnMut(u8, &str),
{
    let dirs = search_dirs(kind);
    if let Some(existing) = find_valid(kind, &dirs) {
        on_progress(70, "Komponen Developer Disk Image sudah tersedia secara lokal.");
        return Ok(existing);
    }

    let target_dir = cache_dir(kind)?;
    std::fs::create_dir_all(&target_dir).map_err(|e| {
        format!(
            "Gagal membuat folder cache DDI ({}): {e}",
            target_dir.display()
        )
    })?;

    let base_url = ddi_base_url();
    let files = kind.files();
    let total_files = files.len().max(1);

    for (idx, filename) in files.iter().enumerate() {
        let dest = target_dir.join(filename);
        let already_ok = if *filename == "BuildManifest.plist" {
            is_non_empty_file(&dest)
                && plist::from_file(&dest)
                    .ok()
                    .and_then(|v| v.into_dictionary())
                    .is_some()
        } else {
            is_non_empty_file(&dest)
        };
        if already_ok {
            let pct = 10u8 + (((idx + 1) * 60) / total_files) as u8;
            on_progress(
                pct.min(70),
                &format!("Memverifikasi komponen {} ({}/{})...", filename, idx + 1, total_files),
            );
            continue;
        }

        let url = format!("{base_url}/PersonalizedImages/{}/{filename}", kind.folder());
        let slice_start = 10u8 + ((idx * 60) / total_files) as u8;
        let slice_span = (60 / total_files).max(1) as u8;
        on_progress(
            slice_start,
            &format!(
                "Menyiapkan iPhone (pertama kali) — mengunduh {} ({}/{})...",
                filename,
                idx + 1,
                total_files
            ),
        );

        let mut last_reported_bytes: u64 = 0;
        let dl_res = crate::win_http::http_download_to_file_atomic(
            &url,
            &dest,
            90_000,
            |downloaded, total_opt| {
                if downloaded.saturating_sub(last_reported_bytes) < 256 * 1024
                    && total_opt.map(|t| downloaded < t).unwrap_or(true)
                {
                    return;
                }
                last_reported_bytes = downloaded;
                let sub_pct = match total_opt {
                    Some(total) if total > 0 => {
                        let ratio = (downloaded.min(total) * u64::from(slice_span)) / total;
                        slice_start.saturating_add(ratio as u8).min(70)
                    }
                    _ => slice_start.saturating_add(slice_span / 2).min(70),
                };
                let mb = downloaded as f64 / (1024.0 * 1024.0);
                let msg = if let Some(total) = total_opt.filter(|&t| t > 0) {
                    let tot_mb = total as f64 / (1024.0 * 1024.0);
                    format!(
                        "Mengunduh {} ({:.1}/{:.1} MB)...",
                        filename, mb, tot_mb
                    )
                } else {
                    format!("Mengunduh {} ({:.1} MB)...", filename, mb)
                };
                on_progress(sub_pct, &msg);
            },
        );

        if let Err(err) = dl_res {
            return Err(format!(
                "Gagal mengunduh {} (butuh internet 1x untuk penyiapan pertama iPhone). Pastikan PC terhubung ke internet lalu coba lagi. / Failed to download {} (1x internet connection required for first-time setup). Detail: {}",
                kind.label_id(),
                filename,
                err
            ));
        }

        if *filename == "BuildManifest.plist" {
            let valid_manifest = plist::from_file(&dest)
                .ok()
                .and_then(|v| v.into_dictionary())
                .is_some();
            if !valid_manifest {
                let _ = std::fs::remove_file(&dest);
                return Err(format!(
                    "Berkas BuildManifest.plist yang diunduh dari {url} tidak valid."
                ));
            }
        }
    }

    if !is_valid_ddi_dir(kind, &target_dir) {
        return Err(format!(
            "Verifikasi akhir cache DDI gagal di {}.",
            target_dir.display()
        ));
    }

    on_progress(70, "Komponen Developer Disk Image siap dipasang ke iPhone.");
    Ok(target_dir)
}

/// Raw bytes of a discovered DDI folder.
#[derive(Debug, Clone)]
pub struct Files {
    pub build_manifest: Vec<u8>,
    pub image: Vec<u8>,
    pub trustcache: Vec<u8>,
    pub cryptex_info: Vec<u8>,
    pub root_hash: Vec<u8>,
}

pub fn read(kind: Kind, dir: &Path) -> std::io::Result<Files> {
    let read_one = |name: &str| std::fs::read(dir.join(name));
    let cryptex = kind == Kind::Cryptex;
    Ok(Files {
        build_manifest: read_one("BuildManifest.plist")?,
        image: read_one("Image.dmg")?,
        trustcache: read_one("Image.dmg.trustcache")?,
        cryptex_info: if cryptex {
            read_one("Image.dmg.cryptex_info")?
        } else {
            Vec::new()
        },
        root_hash: if cryptex {
            read_one("Image.dmg.root_hash")?
        } else {
            Vec::new()
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ddi_kind_mapping_ios16_to_ios28() {
        assert_eq!(Kind::for_version("16.7.10"), None);
        assert_eq!(Kind::for_version("17.0"), Some(Kind::Personalized));
        assert_eq!(Kind::for_version("26.4.1"), Some(Kind::Personalized));
        assert_eq!(Kind::for_version("27.0"), Some(Kind::Cryptex));
        assert_eq!(Kind::for_version("27.0.1"), Some(Kind::Cryptex));
        assert_eq!(Kind::for_version("28.1"), Some(Kind::Cryptex));
        assert_eq!(Kind::for_version(""), None);
    }

    #[test]
    fn test_ddi_find_and_read_complete_folder() {
        let base = std::env::temp_dir().join(format!("ipl-ddi-test-{}", std::process::id()));
        let full = base.join("full");
        let partial = base.join("part");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&full).unwrap();
        std::fs::create_dir_all(&partial).unwrap();

        for f in Kind::Cryptex.files() {
            if *f == "BuildManifest.plist" {
                std::fs::write(full.join(f), b"<plist version=\"1.0\"><dict/></plist>").unwrap();
            } else {
                std::fs::write(full.join(f), b"ddi-payload").unwrap();
            }
        }
        std::fs::write(partial.join("Image.dmg"), b"incomplete").unwrap();

        assert_eq!(
            find(Kind::Cryptex, &[partial.clone(), full.clone()]),
            Some(full.clone())
        );
        assert_eq!(
            find_valid(Kind::Cryptex, &[partial.clone(), full.clone()]),
            Some(full.clone())
        );
        assert_eq!(find(Kind::Cryptex, &[partial]), None);

        let files = read(Kind::Cryptex, &full).unwrap();
        assert_eq!(files.root_hash, b"ddi-payload");
        assert_eq!(files.cryptex_info, b"ddi-payload");
        let _ = std::fs::remove_dir_all(&base);
    }
}
