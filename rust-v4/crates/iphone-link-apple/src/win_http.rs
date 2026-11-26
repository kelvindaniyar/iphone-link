//! Pure Native Win32 WinHTTP (`winhttp.dll`) HTTP/HTTPS client re-exported for
//! Apple TSS (`gs.apple.com`) and 1x Developer Disk Image auto-downloader.
//!
//! Zero external crates, zero PowerShell, zero `curl.exe`, zero `python.exe`.

pub use idevice::win_http::{
    http_download_to_file_atomic, http_get_bytes, http_post_xml, parse_http_url, HttpResponse,
    ParsedUrl,
};
