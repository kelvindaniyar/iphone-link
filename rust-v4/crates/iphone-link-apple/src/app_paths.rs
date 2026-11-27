//! Unified Writable AppData Path Resolver & Legacy Migration (Tahap 293).
//!
//! Ensures installed binaries under `C:\Program Files\iPhone Link\` NEVER fail on read-only
//! disk writes and never lock onto Admin-owned stale files inside `Program Files`, while
//! keeping Portable ZIP folders outside protected OS directories 100% self-contained.

use std::fs;
use std::path::{Path, PathBuf};

/// Normalizes a directory path for deterministic comparisons (strips Windows `\\?\` prefix).
pub fn normalize_dir_path(dir: &Path) -> PathBuf {
    let raw = dir.to_string_lossy();
    let stripped = raw.strip_prefix(r"\\?\").unwrap_or(&raw);
    PathBuf::from(stripped)
}

fn normalized_cmp_str(dir: &Path) -> String {
    normalize_dir_path(dir)
        .to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

/// Returns `true` when `dir` resides inside Windows protected read-only OS directories
/// (`Program Files`, `Program Files (x86)`, `Windows`, or `%ProgramW6432%`).
pub fn is_path_under_program_files(dir: &Path) -> bool {
    let norm = normalized_cmp_str(dir);
    let mut roots = vec![
        "c:/program files".to_string(),
        "c:/program files (x86)".to_string(),
        "c:/windows".to_string(),
    ];
    for var in ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432", "SystemRoot"] {
        if let Ok(val) = std::env::var(var) {
            let trimmed = val.trim();
            if !trimmed.is_empty() {
                roots.push(normalized_cmp_str(Path::new(trimmed)));
            }
        }
    }
    roots.into_iter().any(|root| {
        !root.is_empty() && (norm == root || norm.starts_with(&format!("{root}/")))
    })
}

/// Probes whether `dir` is writable by the current non-elevated user (always `false` in Program Files).
pub fn is_dir_writable(dir: &Path) -> bool {
    if is_path_under_program_files(dir) {
        return false;
    }
    let probe = dir.join(format!(".ipl_write_probe_{}", std::process::id()));
    match fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&probe)
    {
        Ok(f) => {
            drop(f);
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Canonical unified `%LOCALAPPDATA%\iPhoneLink` directory.
pub fn unified_localappdata_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .filter(|v| !v.is_empty())
        .map(|v| PathBuf::from(v).join("iPhoneLink"))
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .filter(|v| !v.is_empty())
                .map(|h| PathBuf::from(h).join("AppData").join("Local").join("iPhoneLink"))
        })
}

/// Legacy v4/v5 settings folder `%LOCALAPPDATA%\iPhoneLink_NativeRust_v4` for backward migration.
pub fn legacy_localappdata_v4_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .filter(|v| !v.is_empty())
        .map(|v| PathBuf::from(v).join("iPhoneLink_NativeRust_v4"))
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .filter(|v| !v.is_empty())
                .map(|h| {
                    PathBuf::from(h)
                        .join("AppData")
                        .join("Local")
                        .join("iPhoneLink_NativeRust_v4")
                })
        })
}

/// Resolves the active writable data directory for a given executable directory:
/// - If `exe_dir` is writable and NOT under `Program Files` -> returns `exe_dir` (Portable mode).
/// - Otherwise returns `%LOCALAPPDATA%\iPhoneLink\` (Installed / read-only mode).
pub fn resolve_app_data_dir_for_exe_dir(exe_dir: &Path) -> PathBuf {
    let clean_exe = normalize_dir_path(exe_dir);
    if !is_path_under_program_files(&clean_exe) && is_dir_writable(&clean_exe) {
        return clean_exe;
    }
    if let Some(unified) = unified_localappdata_dir() {
        let _ = fs::create_dir_all(&unified);
        return unified;
    }
    clean_exe
}

/// Returns the current executable parent directory (normalized without `\\?\`).
pub fn current_exe_dir_clean() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            return normalize_dir_path(parent);
        }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Resolves the active writable data directory for the running process.
pub fn resolve_app_data_dir() -> PathBuf {
    resolve_app_data_dir_for_exe_dir(&current_exe_dir_clean())
}

/// Non-destructively copies `legacy` -> `target` if `target` does not exist yet and `legacy` is a valid file.
pub fn migrate_legacy_file_if_needed(target: &Path, legacy: &Path) -> bool {
    if target == legacy || target.is_file() || !legacy.is_file() {
        return false;
    }
    if let Some(parent) = target.parent() {
        if !parent.as_os_str().is_empty() {
            let _ = fs::create_dir_all(parent);
        }
    }
    fs::copy(legacy, target).is_ok()
}

/// Returns `"ProgramFiles"` when running from a read-only/installed folder, else `"Portable"`.
pub fn install_mode_label() -> &'static str {
    let exe_dir = current_exe_dir_clean();
    if is_path_under_program_files(&exe_dir) || !is_dir_writable(&exe_dir) {
        "ProgramFiles"
    } else {
        "Portable"
    }
}
