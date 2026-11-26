//! Native Win32 FFI wrapper for `playfair.dll` (`zl_playfair_decrypt`) and AirPlay FairPlay v3
//! `POST /fp-setup` Stage 1 (`16 B -> 142 B`) & Stage 2 (`164 B -> 32 B`) handshake.
//!
//! Zero external crates (`LoadLibraryW` + `GetProcAddress` directly via `kernel32.dll`).
//! Searches only neutral relative locations (never hardcoded developer paths).

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use super::hex_decode;

/// Fixed FairPlay v3 Stage 1 reply payloads (UxPlay / shairplay protocol, 142 bytes each).
const REPLY_MESSAGES: [&str; 4] = [
    "46504c59030102000000008202000f9f3f9e0a2521dbdf312ab2bfb29e8d232b6376a8c818701d22ae93d82737feaf9db4fdf41c2dba9d1f49caaabf6591ac1f7bc6f7e0663d21afe01565953eab81f418ceed095adb7c3d0e254909a79831d49c3982973434facb42c63a1cd911a6fe941a8a6d4a743b46c3a7649e44c78955e49d8155009549c4e2f7a3f6d5ba",
    "46504c5903010200000000820201cf32a25714b2524f8aa0ad7af164e37bcf4424e200047efc0ad67afcd95ded1c2730bb591b962ed63a9c4ded88ba8fc78de64d91ccfd5c7b56da88e31f5cceafc7431995a01665a54e1939d25b94db64b9e45d8d063e1e6af07e9656162b0efa404275ea5a44d9591c7256b9fbe6513898b80227721988571650942ad946688a",
    "46504c5903010200000000820202c169a352eeed35b18cdd9c58d64f16c1519a89eb5317bd0d4336cd68f638ff9d016a5b52b7fa9216b2b65482c78444118121a2c7fed83db7119e9182aad7d18c7063e2a457555910af9e0efc76347d164043807f581ee4fbe42ca9dedc1b5eb2a3aa3d2ecd59e7eee70b3629f22afd161d877353ddb99adc8e07006e56f850ce",
    "46504c59030102000000008202039001e1727e0f57f9f5880db104a6257a23f5cfff1abbe1e93045251afb97eb9fc0011ebe0f3a81df5b691d76acb2f7a5c708e3d328f56bb39dbde5f29c8a17f481487e3ae863c678325422e6f78e166d18aa7fd636258bce28726f661f738893ce44311e4be6c0535193e5ef72e8686233729c227d820c999445d89246c8c359",
];
const FP_HEADER: &str = "46504c590301040000000014";

pub const ENV_PLAYFAIR_DLL: &str = "IPHONE_LINK_PLAYFAIR_DLL";

type PlayfairDecryptFn = unsafe extern "C" fn(*const u8, *const u8, *mut u8);

#[cfg(windows)]
mod win_dl {
    use super::*;

    #[link(name = "kernel32")]
    extern "system" {
        fn LoadLibraryW(lp_lib_file_name: *const u16) -> *mut c_void;
        fn GetProcAddress(h_module: *mut c_void, lp_proc_name: *const u8) -> *const c_void;
        fn FreeLibrary(h_lib_module: *mut c_void) -> i32;
        fn GetLastError() -> u32;
    }

    pub struct LoadedPlayfair {
        _module: usize,
        pub decrypt: PlayfairDecryptFn,
    }

    impl LoadedPlayfair {
        pub fn load(path: &Path) -> Result<Self, String> {
            let wide: Vec<u16> = path
                .as_os_str()
                .to_string_lossy()
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            let hmod = unsafe { LoadLibraryW(wide.as_ptr()) };
            if hmod.is_null() {
                let err = unsafe { GetLastError() };
                return Err(format!(
                    "LoadLibraryW({}) gagal (Win32={err})",
                    path.display()
                ));
            }
            let sym = unsafe { GetProcAddress(hmod, b"zl_playfair_decrypt\0".as_ptr()) };
            if sym.is_null() {
                let err = unsafe { GetLastError() };
                unsafe {
                    let _ = FreeLibrary(hmod);
                }
                return Err(format!(
                    "Simbol zl_playfair_decrypt tidak ditemukan di {} (Win32={err})",
                    path.display()
                ));
            }
            let decrypt: PlayfairDecryptFn = unsafe { std::mem::transmute(sym) };
            Ok(Self {
                _module: hmod as usize,
                decrypt,
            })
        }
    }
}

#[cfg(windows)]
static LOADED_DLL: OnceLock<win_dl::LoadedPlayfair> = OnceLock::new();

/// Candidate neutral search paths for `playfair.dll` (no hardcoded drive/developer paths).
pub fn candidate_dll_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(custom) = std::env::var_os(ENV_PLAYFAIR_DLL) {
        let s = custom.to_string_lossy();
        if !s.trim().is_empty() {
            out.push(PathBuf::from(s.trim()));
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            out.push(dir.join("playfair.dll"));
            out.push(dir.join("bin").join("playfair.dll"));
            // Neutral parent traversal (e.g. target/debug/deps -> rust-v4/bins/playfair.dll)
            let mut cur = dir;
            for _ in 0..4 {
                if let Some(p) = cur.parent() {
                    out.push(p.join("bins").join("playfair.dll"));
                    cur = p;
                } else {
                    break;
                }
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        out.push(cwd.join("playfair.dll"));
        out.push(cwd.join("bins").join("playfair.dll"));
        out.push(cwd.join("rust-v4").join("bins").join("playfair.dll"));
        out.push(cwd.join("..").join("..").join("bins").join("playfair.dll"));
        out.push(cwd.join("handset").join("bin").join("playfair.dll"));
    }
    #[cfg(test)]
    {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        out.push(manifest_dir.join("..").join("..").join("bins").join("playfair.dll"));
        out.push(
            manifest_dir
                .join("..")
                .join("..")
                .join("..")
                .join("handset")
                .join("bin")
                .join("playfair.dll"),
        );
    }
    out
}

/// Ensures `playfair.dll` is loaded from the first valid candidate path.
#[cfg(windows)]
pub fn ensure_loaded() -> Result<(), String> {
    if LOADED_DLL.get().is_some() {
        return Ok(());
    }
    let mut last_err = "playfair.dll tidak ditemukan di folder aplikasi.".to_string();
    for path in candidate_dll_paths() {
        if !path.is_file() {
            continue;
        }
        match win_dl::LoadedPlayfair::load(&path) {
            Ok(loaded) => {
                let _ = LOADED_DLL.set(loaded);
                return Ok(());
            }
            Err(e) => last_err = e,
        }
    }
    Err(last_err)
}

#[cfg(not(windows))]
pub fn ensure_loaded() -> Result<(), String> {
    Err("playfair.dll is only supported on Windows".into())
}

/// Per-RTSP-connection FairPlay v3 session state.
#[derive(Default, Debug, Clone)]
pub struct FairPlay {
    keymsg: Option<Vec<u8>>,
}

impl FairPlay {
    /// Handles a `POST /fp-setup` payload:
    /// - Stage 1 (`16 bytes`, `req[4] == 0x03`) -> `142 bytes`
    /// - Stage 2 (`164 bytes`, `req[4] == 0x03`) -> `32 bytes` (`FP_HEADER || req[144..164]`)
    pub fn handle(&mut self, req: &[u8]) -> Option<Vec<u8>> {
        if req.len() < 5 || req[4] != 0x03 {
            return None;
        }
        match req.len() {
            16 => {
                self.keymsg = None;
                let idx = req[14] as usize;
                REPLY_MESSAGES.get(idx).map(|m| hex_decode(m))
            }
            164 => {
                self.keymsg = Some(req.to_vec());
                let mut out = hex_decode(FP_HEADER);
                out.extend_from_slice(&req[144..164]);
                Some(out)
            }
            _ => None,
        }
    }

    /// Decrypts the 72-byte `ekey` from AirPlay `SETUP` into a 16-byte FairPlay AES key
    /// using `playfair.dll` (`zl_playfair_decrypt`).
    #[cfg(windows)]
    pub fn decrypt(&self, ekey: &[u8]) -> Result<[u8; 16], String> {
        let keymsg = self
            .keymsg
            .as_ref()
            .filter(|k| k.len() == 164)
            .ok_or_else(|| "FairPlay handshake belum selesai (keymsg 164 B hilang)".to_string())?;
        if ekey.len() != 72 {
            return Err(format!("FairPlay ekey harus 72 byte (diterima {} byte)", ekey.len()));
        }
        ensure_loaded()?;
        let dll = LOADED_DLL
            .get()
            .ok_or_else(|| "playfair.dll gagal dimuat".to_string())?;
        let mut out = [0u8; 16];
        unsafe {
            (dll.decrypt)(keymsg.as_ptr(), ekey.as_ptr(), out.as_mut_ptr());
        }
        Ok(out)
    }

    #[cfg(not(windows))]
    pub fn decrypt(&self, _ekey: &[u8]) -> Result<[u8; 16], String> {
        Err("playfair.dll only supported on Windows".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tahap252_playfair_dll_ffi_stage1_handshake() {
        ensure_loaded().expect("playfair.dll must load from rust-v4/bins or handset/bin");
        let mut fp = FairPlay::default();

        // Stage 1: 16 bytes -> 142 bytes
        let stage1_req = [
            0x46, 0x50, 0x4C, 0x59, 0x03, 0x01, 0x01, 0x00,
            0x00, 0x00, 0x00, 0x04, 0x02, 0x00, 0x02, 0xBB,
        ];
        let stage1_resp = fp.handle(&stage1_req).expect("Stage 1 must return 142-byte reply");
        assert_eq!(stage1_resp.len(), 142);
        assert_eq!(stage1_resp, hex_decode(REPLY_MESSAGES[2]));

        // Stage 2: 164 bytes -> 32 bytes (12-byte header + last 20 bytes)
        let mut stage2_req = vec![0x46, 0x50, 0x4C, 0x59, 0x03];
        stage2_req.resize(164, 0);
        let stage2_resp = fp.handle(&stage2_req).expect("Stage 2 must return 32-byte reply");
        assert_eq!(stage2_resp.len(), 32);
        assert_eq!(&stage2_resp[..12], hex_decode(FP_HEADER).as_slice());

        // Real zl_playfair_decrypt FFI check against known deterministic vector
        let ekey: Vec<u8> = (0u8..72u8).collect();
        let decrypted = fp.decrypt(&ekey).expect("zl_playfair_decrypt must succeed");
        assert_eq!(
            decrypted.as_slice(),
            hex_decode("299464065e68f7ed2125fcdd9a06f983").as_slice()
        );

        // Invalid FairPlay version rejected cleanly
        assert!(fp.handle(&[0u8; 16]).is_none());
    }
}
