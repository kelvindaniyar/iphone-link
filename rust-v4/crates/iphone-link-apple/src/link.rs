//! Native CoreDevice USB Userspace Software Tunnel, RSD Handshake, and DDI Mounter.
//!
//! Uses `idevice` (`UsbmuxdConnection`, `CoreDeviceProxy`, `create_software_tunnel`, `RsdHandshake`,
//! `cryptexd`, `ImageMounter`, `AmfiClient`) without requiring `wintun.dll` or Python.

use std::sync::OnceLock;
use std::time::Duration;

use idevice::{
    amfi::AmfiClient,
    core_device_proxy::CoreDeviceProxy,
    cryptexd::{install_ddi, installed_ddi, Cryptex1Assets},
    lockdown::LockdownClient,
    mobile_image_mounter::ImageMounter,
    provider::{IdeviceProvider as _, UsbmuxdProvider},
    remote_pairing::RemotePairingClient,
    rsd::RsdHandshake,
    tcp::handle::AdapterHandle,
    usbmuxd::{Connection, UsbmuxdAddr, UsbmuxdConnection},
    ReadWrite,
};

use crate::ddi::{self, Kind};
use crate::wifi;

pub const DISPLAY_SERVICE: &str = "com.apple.coredevice.displayservice";
pub const SERVICE_ERR: &str = "Apple Mobile Device Service:";
pub const DEV_MODE_OFF: &str = "Developer Mode is off";
pub const DEV_MODE_OFF_BILINGUAL: &str = "Developer Mode belum aktif di iPhone Anda. Buka Settings -> Privacy & Security -> Developer Mode -> Nyalakan -> Restart iPhone, lalu coba lagi. / Developer Mode is off on your iPhone. Open Settings -> Privacy & Security -> Developer Mode -> Turn On -> Restart iPhone, then retry.";
pub const WIFI_DDI_POST_REBOOT_BILINGUAL: &str = "iPhone baru saja restart sehingga Developer Disk Image (DDI) belum terpasang. Hubungkan iPhone dengan kabel USB sekali saja agar DDI terpasang otomatis, lalu Wi-Fi mirroring dapat digunakan kembali. / Your iPhone recently restarted and needs the Developer Disk Image (DDI) mounted. Connect via USB cable once to mount automatically, then Wi-Fi mirroring will work again.";

/// Result of `Link::ensure_ddi()` or `ensure_ddi_on_tunnel()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DdiMountOutcome {
    AlreadyMounted,
    MountedNow,
    NotRequiredForIosVersion,
}

/// Structured progress stage emitted during automatic DDI download/mount/RSD-reconnect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DdiStage {
    DownloadingDdi,
    MountingDdi,
    ReconnectingRsd,
}

impl DdiStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DownloadingDdi => "downloading_ddi",
            Self::MountingDdi => "mounting_ddi",
            Self::ReconnectingRsd => "reconnecting_rsd",
        }
    }
}

/// Routing tier selected from iOS `ProductVersion` for multi-iOS mirroring compatibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IosMirrorRoute {
    /// iOS <= 17.3: AirPlay standalone H.264 mirroring.
    AirPlayStandalone,
    /// iOS 17.4 ..= 26.x: Hybrid AirPlay H.264 video + CoreDevice/RSD HID touch/keyboard.
    HybridAirPlayCoreDeviceHid,
    /// iOS >= 27.0 (or unknown fallback): Native CoreDevice DisplayService HEVC + HID.
    CoreDeviceNativeHevc,
}

/// Resolves the optimal mirroring transport route from an iOS `ProductVersion` string
/// (e.g. `"16.7.10"`, `"17.2.1"`, `"17.4.1"`, `"18.5"`, `"26.2"`, `"27.2"`).
/// Empty or unparseable versions safely default to `CoreDeviceNativeHevc`.
pub fn resolve_ios_mirror_route(product_version: &str) -> IosMirrorRoute {
    let trimmed = product_version.trim();
    if trimmed.is_empty() {
        return IosMirrorRoute::CoreDeviceNativeHevc;
    }
    let mut parts = trimmed.split('.');
    let Ok(major) = parts.next().unwrap_or("").trim().parse::<u32>() else {
        return IosMirrorRoute::CoreDeviceNativeHevc;
    };
    let minor = parts
        .next()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(0);
    if major < 17 || (major == 17 && minor <= 3) {
        IosMirrorRoute::AirPlayStandalone
    } else if (major == 17 && minor >= 4) || (18..=26).contains(&major) {
        IosMirrorRoute::HybridAirPlayCoreDeviceHid
    } else {
        IosMirrorRoute::CoreDeviceNativeHevc
    }
}

static TOKIO_RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

pub fn runtime() -> &'static tokio::runtime::Runtime {
    TOKIO_RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("iphone-link-apple")
            .enable_all()
            .build()
            .expect("tokio runtime for iphone-link-apple")
    })
}

/// Runs an async `idevice` future synchronously on the shared runtime.
pub fn block_on<F: std::future::Future>(f: F) -> F::Output {
    runtime().block_on(f)
}

fn err(what: &str) -> impl Fn(idevice::IdeviceError) -> String + '_ {
    move |e| format!("{what}: {e:?}")
}

/// Basic device info fetched via Lockdown.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Info {
    pub name: String,
    pub product_type: String,
    pub version: String,
    pub build_version: String,
    pub device_class: String,
    pub wifi_address: String,
    pub bluetooth_address: String,
}

/// Open RSD link over a CoreDevice software tunnel (without Wintun/admin).
#[derive(Debug)]
pub struct Link {
    pub adapter: AdapterHandle,
    pub rsd: RsdHandshake,
    pub rsd_port: u16,
    pub wifi: bool,
    pub provider: Option<UsbmuxdProvider>,
    pub _control: Option<RemotePairingClient<Box<dyn ReadWrite>>>,
}

pub async fn usbmux() -> Result<UsbmuxdConnection, String> {
    tokio::time::timeout(Duration::from_millis(800), UsbmuxdConnection::default())
        .await
        .map_err(|_| format!("{SERVICE_ERR} connection timed out"))?
        .map_err(|e| format!("{SERVICE_ERR} {e:?}"))
}

pub(crate) async fn check_developer_mode_enabled(provider: &UsbmuxdProvider) -> bool {
    // Primary check via Lockdown (`com.apple.security.mac.amfi` -> `DeveloperModeStatus`)
    if let Ok(mut lockdown) = LockdownClient::connect(provider).await {
        let pairing_opt = provider.get_pairing_file().await.ok();
        if let Some(ref pf) = pairing_opt {
            let _ = lockdown.start_session(pf).await;
        }
        if let Ok(status) = lockdown.get_developer_mode_status().await {
            if !status {
                if let Ok(mut amfi) = AmfiClient::connect(provider).await {
                    let _ = amfi.reveal_developer_mode_option_in_ui().await;
                }
            }
            return status;
        }
    }
    // Secondary check via AMFI Lockdown service (`com.apple.amfi.lockdown`, action=3)
    let Ok(mut amfi) = AmfiClient::connect(provider).await else {
        return true;
    };
    match amfi.get_developer_mode_status().await {
        Ok(false) => {
            let _ = amfi.reveal_developer_mode_option_in_ui().await;
            false
        }
        _ => true,
    }
}

/// Reads `UniqueChipID` (ECID) and optional `ProductVersion` from Lockdown over USB.
pub(crate) async fn read_ecid_and_version(
    provider: &UsbmuxdProvider,
) -> Result<(u64, String), String> {
    let mut lockdown = LockdownClient::connect(provider)
        .await
        .map_err(err("lockdown"))?;
    let pairing_opt = provider.get_pairing_file().await.ok();
    if let Some(ref pf) = pairing_opt {
        let _ = lockdown.start_session(pf).await;
    }
    let ecid = lockdown
        .get_value(Some("UniqueChipID"), None)
        .await
        .ok()
        .and_then(|v| v.as_unsigned_integer())
        .ok_or_else(|| "Gagal membaca UniqueChipID (ECID) dari iPhone.".to_string())?;
    let version = lockdown
        .get_value(Some("ProductVersion"), None)
        .await
        .ok()
        .and_then(|v| v.as_string().map(str::to_owned))
        .unwrap_or_default();
    Ok((ecid, version))
}

/// Core shared DDI installer used by BOTH `Link::ensure_ddi()` AND `physical_stream.rs`
/// whenever `RsdHandshake` does not offer `com.apple.coredevice.displayservice` (`disp_port == 0`).
pub async fn ensure_ddi_with_rsd<P: idevice::provider::RsdProvider>(
    rsd_provider: &mut P,
    rsd: &mut RsdHandshake,
    usb_provider: Option<&UsbmuxdProvider>,
    product_version: &str,
    mut on_stage: impl FnMut(DdiStage, u8, &str),
) -> Result<DdiMountOutcome, String> {
    if rsd.services.contains_key(DISPLAY_SERVICE) {
        return Ok(DdiMountOutcome::AlreadyMounted);
    }

    let resolved_version = if !product_version.trim().is_empty() {
        product_version.trim().to_string()
    } else if let Some(p) = usb_provider {
        read_ecid_and_version(p)
            .await
            .map(|(_, v)| v)
            .unwrap_or_default()
    } else {
        String::new()
    };

    let kind = if resolved_version.is_empty() {
        if rsd
            .services
            .contains_key(idevice::cryptexd::CRYPTEXD_SERVICE_NAME)
        {
            Kind::Cryptex
        } else {
            Kind::Personalized
        }
    } else {
        match Kind::for_version(&resolved_version) {
            Some(k) => k,
            None => return Ok(DdiMountOutcome::NotRequiredForIosVersion),
        }
    };

    // Fast path: for iOS 27+ Cryptex1, check if DDI cryptex is already installed.
    if kind == Kind::Cryptex
        && rsd
            .services
            .contains_key(idevice::cryptexd::CRYPTEXD_SERVICE_NAME)
    {
        if let Ok(Some(_)) = installed_ddi(rsd_provider, rsd).await {
            return Ok(DdiMountOutcome::AlreadyMounted);
        }
    }

    // Check Developer Mode over USB AMFI before any download or TSS request.
    if let Some(provider) = usb_provider {
        if !check_developer_mode_enabled(provider).await {
            return Err(DEV_MODE_OFF_BILINGUAL.into());
        }
    } else if kind == Kind::Personalized {
        return Err(WIFI_DDI_POST_REBOOT_BILINGUAL.into());
    } else if !rsd
        .services
        .contains_key(idevice::cryptexd::CRYPTEXD_SERVICE_NAME)
    {
        return Err(WIFI_DDI_POST_REBOOT_BILINGUAL.into());
    }

    // Step 1: Ensure DDI assets exist locally (auto-download 1x via pure WinHTTP if missing).
    on_stage(
        DdiStage::DownloadingDdi,
        10,
        "Menyiapkan komponen iPhone (pertama kali)...",
    );
    let dir = ddi::ensure_ddi_files_downloaded(kind, |pct, msg| {
        on_stage(DdiStage::DownloadingDdi, pct, msg);
    })?;

    // Step 2: Read DDI assets and perform real Apple TSS personalization + device mount.
    on_stage(
        DdiStage::MountingDdi,
        75,
        "Menandatangani & memasang Developer Disk Image ke iPhone...",
    );
    let files = ddi::read(kind, &dir).map_err(|e| format!("Gagal membaca berkas DDI: {e}"))?;

    match kind {
        Kind::Cryptex => {
            let manifest_val = plist::from_bytes(&files.build_manifest)
                .map_err(|e| format!("BuildManifest.plist tidak valid: {e}"))?;
            let manifest = manifest_val
                .into_dictionary()
                .ok_or_else(|| "Root BuildManifest.plist bukan dictionary".to_string())?;
            let identity = idevice::tss::select_cryptex_build_identity(&manifest)
                .map_err(err("build identity"))?
                .clone();
            let assets = Cryptex1Assets::from_parts(
                files.image,
                files.trustcache,
                files.cryptex_info,
                files.root_hash,
                identity,
            );
            install_ddi(rsd_provider, rsd, &assets)
                .await
                .map_err(|e| {
                    let s = format!("{e}");
                    if usb_provider.is_none() {
                        format!("{WIFI_DDI_POST_REBOOT_BILINGUAL} ({s})")
                    } else if s.to_ascii_lowercase().contains("developer mode") {
                        DEV_MODE_OFF_BILINGUAL.to_string()
                    } else {
                        format!("Gagal memasang Cryptex1 DDI: {s}")
                    }
                })?;
        }
        Kind::Personalized => {
            let Some(provider) = usb_provider else {
                return Err(WIFI_DDI_POST_REBOOT_BILINGUAL.into());
            };
            let mut mounter = ImageMounter::connect(provider)
                .await
                .map_err(err("mobile_image_mounter"))?;
            let (ecid, _) = read_ecid_and_version(provider).await?;
            mounter
                .mount_personalized(
                    provider,
                    files.image,
                    files.trustcache,
                    &files.build_manifest,
                    None,
                    ecid,
                )
                .await
                .map_err(|e| {
                    let s = format!("{e}");
                    if s.to_ascii_lowercase().contains("developer mode") {
                        DEV_MODE_OFF_BILINGUAL.to_string()
                    } else {
                        format!("Gagal memasang Personalized DDI: {s}")
                    }
                })?;
        }
    }

    on_stage(
        DdiStage::ReconnectingRsd,
        92,
        "Menyambungkan ulang layanan tampilan iPhone...",
    );
    Ok(DdiMountOutcome::MountedNow)
}

/// Lists USB-connected Apple devices paired/unpaired status (`(udid, paired)`).
pub async fn usb_devices() -> Result<Vec<(String, bool)>, String> {
    let mut mux = usbmux().await?;
    let list = mux.get_devices().await.map_err(err("device list"))?;
    let mut out = Vec::new();
    for d in list
        .into_iter()
        .filter(|d| d.connection_type == Connection::Usb)
    {
        let paired = mux.get_pair_record(&d.udid).await.is_ok();
        out.push((d.udid, paired));
    }
    Ok(out)
}

pub(crate) async fn read_info(lockdown: &mut LockdownClient) -> Info {
    let mut get = async |k: &str| -> String {
        lockdown
            .get_value(Some(k), None)
            .await
            .ok()
            .and_then(|v| v.as_string().map(str::to_owned))
            .unwrap_or_default()
    };
    Info {
        name: get("DeviceName").await,
        product_type: get("ProductType").await,
        version: get("ProductVersion").await,
        build_version: get("BuildVersion").await,
        device_class: get("DeviceClass").await,
        wifi_address: get("WiFiAddress").await,
        bluetooth_address: get("BluetoothAddress").await,
    }
}

/// Fetches device metadata from Lockdown over USB usbmuxd.
pub async fn info(udid: &str) -> Result<Info, String> {
    let provider = provider(udid).await?;
    let mut lockdown = LockdownClient::connect(&provider)
        .await
        .map_err(err("lockdown"))?;
    let pre = read_info(&mut lockdown).await;
    let pairing_opt = provider
        .get_pairing_file()
        .await
        .ok()
        .or_else(|| idevice::provider::PairingFile::load_from_lockdown_dir(udid));
    if let Some(pairing) = pairing_opt {
        if lockdown.start_session(&pairing).await.is_ok() {
            let post = read_info(&mut lockdown).await;
            return Ok(Info {
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
            });
        }
    }
    Ok(pre)
}

pub(crate) async fn provider(udid: &str) -> Result<UsbmuxdProvider, String> {
    let mut mux = usbmux().await?;
    let dev = mux.get_device(udid).await.map_err(err("device"))?;
    let addr = UsbmuxdAddr::from_env_var().map_err(|e| format!("usbmuxd address: {e}"))?;
    Ok(dev.to_provider(addr, "iphone-link"))
}

impl Link {
    /// Opens a USB CoreDevice userspace software tunnel and performs `RsdHandshake`.
    pub async fn usb(udid: &str) -> Result<Self, String> {
        let provider = provider(udid).await?;
        let proxy = CoreDeviceProxy::connect(&provider)
            .await
            .map_err(err("CoreDeviceProxy (iOS 17.4 or later?)"))?;
        let rsd_port = proxy.tunnel_info().server_rsd_port;
        let adapter = proxy
            .create_software_tunnel()
            .map_err(err("tunnel"))?
            .to_async_handle();
        Self::finish(adapter, rsd_port, false, Some(provider), None).await
    }

    /// Opens a wireless Wi-Fi RemotePairing TLS-PSK tunnel to an explicit `addr` & `Record`.
    pub async fn wifi_with_record(
        addr: std::net::SocketAddr,
        record: &wifi::Record,
    ) -> Result<Self, String> {
        let t = wifi::connect(addr, record).await?;
        Self::finish(
            t.adapter.to_async_handle(),
            t.rsd_port,
            true,
            None,
            Some(t.control),
        )
        .await
    }

    /// Opens a wireless Wi-Fi CoreDevice RemotePairing TLS-PSK tunnel for `udid`.
    pub async fn wifi(udid: &str, ip_hint: Option<&str>) -> Result<Self, String> {
        let record = wifi::find_record(udid).ok_or_else(|| {
            "Wi-Fi isn't set up for this device yet: connect USB cable once and run Set up Wi-Fi"
                .to_string()
        })?;
        let cached = wifi::get_device_wifi_state(udid);
        let ip_str = ip_hint
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string)
            .or_else(|| {
                cached
                    .get("last_ip")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            })
            .or_else(|| wifi::discover_iphone_ip_on_lan(Some(udid), 250))
            .ok_or_else(|| "iPhone LAN IP not found on Wi-Fi".to_string())?;
        let ip_addr: std::net::IpAddr = ip_str
            .parse()
            .map_err(|e| format!("invalid iPhone LAN IP '{ip_str}': {e}"))?;

        let mut ports = Vec::new();
        if let Some(p) = cached.get("last_rp_port").and_then(|v| v.as_u64()) {
            if p > 0 && p <= u16::MAX as u64 {
                ports.push(p as u16);
            }
        }
        for p in wifi::query_unicast_dns_sd(&ip_str, 10) {
            if !ports.contains(&p) {
                ports.push(p);
            }
        }
        for fallback_p in [49152u16, 58767, 49153] {
            if !ports.contains(&fallback_p) {
                ports.push(fallback_p);
            }
        }

        let mut last_err = "RemotePairing connect failed".to_string();
        for rp_port in ports {
            match Self::wifi_with_record(std::net::SocketAddr::new(ip_addr, rp_port), &record).await
            {
                Ok(link) => {
                    wifi::update_device_wifi_state(
                        udid,
                        Some(true),
                        Some(&ip_str),
                        false,
                        None,
                        None,
                        false,
                        true,
                        Some(rp_port),
                    );
                    return Ok(link);
                }
                Err(e) => last_err = e,
            }
        }
        Err(last_err)
    }

    async fn finish(
        mut adapter: AdapterHandle,
        rsd_port: u16,
        wifi: bool,
        provider: Option<UsbmuxdProvider>,
        control: Option<RemotePairingClient<Box<dyn ReadWrite>>>,
    ) -> Result<Self, String> {
        let s = adapter
            .connect(rsd_port)
            .await
            .map_err(|e| format!("RSD connect: {e:?}"))?;
        let rsd = RsdHandshake::new(s).await.map_err(err("RSD handshake"))?;
        Ok(Self {
            adapter,
            rsd,
            rsd_port,
            wifi,
            provider,
            _control: control,
        })
    }

    pub async fn info(&mut self) -> Result<Info, String> {
        let mut lockdown = LockdownClient::connect_rsd(&mut self.adapter, &mut self.rsd)
            .await
            .map_err(err("lockdown over RSD"))?;
        Ok(read_info(&mut lockdown).await)
    }

    pub async fn screen(&mut self) -> Option<(u32, u32)> {
        let mut lockdown = LockdownClient::connect_rsd(&mut self.adapter, &mut self.rsd)
            .await
            .ok()?;
        let domain = Some("com.apple.mobile.iTunes");
        let num = |v: Result<plist::Value, _>| {
            let v = v.ok()?;
            v.as_unsigned_integer()
                .or_else(|| v.as_signed_integer().map(|i| i as u64))
        };
        let w = num(lockdown.get_value(Some("ScreenWidth"), domain).await)?;
        let h = num(lockdown.get_value(Some("ScreenHeight"), domain).await)?;
        (w > 0 && h > 0).then_some((w as u32, h as u32))
    }

    pub fn display_offered(&self) -> bool {
        self.rsd.services.contains_key(DISPLAY_SERVICE)
    }

    /// Ensures the Developer Disk Image (`iOS 17–26`) or Cryptex1 DDI (`iOS 27+`) is mounted
    /// on the device, auto-downloading DDI files 1x if absent, then reconnecting RSD up to 3x @ 400 ms.
    pub async fn ensure_ddi(
        &mut self,
        version: &str,
        mut progress: impl FnMut(&str),
    ) -> Result<DdiMountOutcome, String> {
        if self.display_offered() {
            return Ok(DdiMountOutcome::AlreadyMounted);
        }

        let outcome = ensure_ddi_with_rsd(
            &mut self.adapter,
            &mut self.rsd,
            self.provider.as_ref(),
            version,
            |_stage, _pct, msg| progress(msg),
        )
        .await?;

        if outcome == DdiMountOutcome::NotRequiredForIosVersion {
            return Ok(outcome);
        }

        // Reconnect RSD up to 3x @ 400 ms until DisplayService is advertised by remoted.
        let mut last_rsd_err = String::new();
        for attempt in 0..3u8 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(400)).await;
            }
            match self.adapter.connect(self.rsd_port).await {
                Ok(s) => match RsdHandshake::new(s).await {
                    Ok(new_rsd) => {
                        self.rsd = new_rsd;
                        if self.display_offered() {
                            return Ok(outcome);
                        }
                        last_rsd_err =
                            "com.apple.coredevice.displayservice belum muncul pasca mount DDI"
                                .to_string();
                    }
                    Err(e) => {
                        last_rsd_err = format!("RSD handshake ulang gagal: {e:?}");
                    }
                },
                Err(e) => {
                    last_rsd_err = format!("RSD reconnect gagal: {e:?}");
                }
            }
        }

        // On iOS 17–26 Personalized DDI, DisplayService itself may not exist (Hybrid AirPlay + HID),
        // so treat MountedNow as success if Personalized DDI mounted cleanly.
        if Kind::for_version(version) == Some(Kind::Personalized)
            && outcome == DdiMountOutcome::MountedNow
        {
            return Ok(outcome);
        }

        Err(format!(
            "Layanan layar iPhone belum aktif setelah pemasangan Developer Disk Image ({last_rsd_err})."
        ))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Mutex;

    pub(crate) static MOCK_ENV_LOCK: Mutex<()> = Mutex::new(());

    fn read_usbmux_req(s: &mut std::net::TcpStream) -> plist::Dictionary {
        let mut hdr = [0u8; 16];
        s.read_exact(&mut hdr).unwrap();
        let len = u32::from_le_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]) as usize;
        let mut body = vec![0u8; len - 16];
        s.read_exact(&mut body).unwrap();
        plist::from_bytes(&body).unwrap().into_dictionary().unwrap()
    }

    fn write_usbmux_resp(s: &mut std::net::TcpStream, dict: plist::Dictionary) {
        let body = plist::to_bytes_xml(&plist::Value::Dictionary(dict)).unwrap();
        let total = (16 + body.len()) as u32;
        let mut hdr = [0u8; 16];
        hdr[0..4].copy_from_slice(&total.to_le_bytes());
        hdr[4..8].copy_from_slice(&1u32.to_le_bytes());
        hdr[8..12].copy_from_slice(&8u32.to_le_bytes());
        hdr[12..16].copy_from_slice(&1u32.to_le_bytes());
        s.write_all(&hdr).unwrap();
        s.write_all(&body).unwrap();
        s.flush().unwrap();
    }

    fn read_len_plist(s: &mut std::net::TcpStream) -> plist::Dictionary {
        let mut lb = [0u8; 4];
        s.read_exact(&mut lb).unwrap();
        let len = u32::from_be_bytes(lb) as usize;
        let mut body = vec![0u8; len];
        s.read_exact(&mut body).unwrap();
        plist::from_bytes(&body).unwrap().into_dictionary().unwrap()
    }

    fn write_len_plist(s: &mut std::net::TcpStream, dict: plist::Dictionary) {
        let body = plist::to_bytes_xml(&plist::Value::Dictionary(dict)).unwrap();
        let len = body.len() as u32;
        s.write_all(&len.to_be_bytes()).unwrap();
        s.write_all(&body).unwrap();
        s.flush().unwrap();
    }

    #[test]
    fn test_coredevice_proxy_software_tunnel_and_rsd_handshake() {
        let _guard = MOCK_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mux_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mux_addr = mux_listener.local_addr().unwrap();
        let rsd_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let rsd_port = rsd_listener.local_addr().unwrap().port();

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_srv = stop.clone();
        mux_listener.set_nonblocking(true).unwrap();

        let server_handle = std::thread::spawn(move || {
            let mut handlers = Vec::new();
            while !stop_srv.load(std::sync::atomic::Ordering::Relaxed) {
                match mux_listener.accept() {
                    Ok((mut s, _)) => {
                        let _ = s.set_nonblocking(false);
                        let _ = s.set_read_timeout(Some(Duration::from_secs(3)));
                        handlers.push(std::thread::spawn(move || {
                            let req = read_usbmux_req(&mut s);
                            let msg = req.get("MessageType").and_then(|v| v.as_string()).unwrap_or("");
                            match msg {
                                "ListDevices" => {
                                    let mut props = plist::Dictionary::new();
                                    props.insert("SerialNumber".into(), plist::Value::String("00008150-MOCKTUNNEL".into()));
                                    props.insert("DeviceID".into(), plist::Value::from(9u32));
                                    props.insert("ConnectionType".into(), plist::Value::String("USB".into()));
                                    let mut dev = plist::Dictionary::new();
                                    dev.insert("Properties".into(), plist::Value::Dictionary(props));
                                    let mut resp = plist::Dictionary::new();
                                    resp.insert("DeviceList".into(), plist::Value::Array(vec![plist::Value::Dictionary(dev)]));
                                    write_usbmux_resp(&mut s, resp);
                                }
                                "ReadPairRecord" => {
                                    let mut pair = plist::Dictionary::new();
                                    pair.insert("HostID".into(), plist::Value::String("HOST-1".into()));
                                    pair.insert("SystemBUID".into(), plist::Value::String("BUID-1".into()));
                                    let pair_bytes = plist::to_bytes_xml(&plist::Value::Dictionary(pair)).unwrap();
                                    let mut resp = plist::Dictionary::new();
                                    resp.insert("PairRecordData".into(), plist::Value::Data(pair_bytes));
                                    write_usbmux_resp(&mut s, resp);
                                }
                                "Connect" => {
                                    let port_be = req.get("PortNumber").and_then(|v| v.as_unsigned_integer()).unwrap() as u16;
                                    let port = u16::from_be(port_be);
                                    let mut ok_resp = plist::Dictionary::new();
                                    ok_resp.insert("Number".into(), plist::Value::from(0u32));
                                    write_usbmux_resp(&mut s, ok_resp);

                                    if port == 62078 {
                                        for _ in 0..3 {
                                            let lreq = read_len_plist(&mut s);
                                            let kind = lreq.get("Request").and_then(|v| v.as_string()).unwrap_or("");
                                            let mut lresp = plist::Dictionary::new();
                                            match kind {
                                                "StartSession" => {
                                                    lresp.insert("EnableSessionSSL".into(), plist::Value::Boolean(false));
                                                    lresp.insert("SessionID".into(), plist::Value::String("SESS-1".into()));
                                                }
                                                "GetValue" => {
                                                    lresp.insert("Value".into(), plist::Value::String("00008150-MOCKTUNNEL".into()));
                                                }
                                                "StartService" => {
                                                    lresp.insert("Port".into(), plist::Value::from(59001u16));
                                                    lresp.insert("EnableServiceSSL".into(), plist::Value::Boolean(false));
                                                }
                                                _ => {}
                                            }
                                            write_len_plist(&mut s, lresp);
                                        }
                                    } else if port == 59001 {
                                        let mut magic = [0u8; 8];
                                        s.read_exact(&mut magic).unwrap();
                                        assert_eq!(&magic, b"CDTunnel");
                                        let mut lbuf = [0u8; 2];
                                        s.read_exact(&mut lbuf).unwrap();
                                        let rlen = u16::from_be_bytes(lbuf) as usize;
                                        let mut rbuf = vec![0u8; rlen];
                                        s.read_exact(&mut rbuf).unwrap();

                                        let reply = format!(
                                            r#"{{"clientParameters":{{"address":"fd00::1","netmask":"ffff:ffff::","mtu":16000}},"serverAddress":"fd00::2","serverRSDPort":{},"loopbackRsdPort":{}}}"#,
                                            rsd_port, rsd_port
                                        );
                                        s.write_all(b"CDTunnel").unwrap();
                                        s.write_all(&(reply.len() as u16).to_be_bytes()).unwrap();
                                        s.write_all(reply.as_bytes()).unwrap();
                                        s.flush().unwrap();
                                    }
                                }
                                _ => {}
                            }
                        }));
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
            for h in handlers {
                let _ = h.join();
            }
        });

        let rsd_handle = std::thread::spawn(move || {
            let (mut rs, _) = rsd_listener.accept().unwrap();
            let mut lb = [0u8; 4];
            rs.read_exact(&mut lb).unwrap();
            let plen = u32::from_be_bytes(lb) as usize;
            let mut pbuf = vec![0u8; plen];
            rs.read_exact(&mut pbuf).unwrap();

            let mut disp = plist::Dictionary::new();
            disp.insert("Entitlement".into(), plist::Value::String("com.apple.coredevice.display".into()));
            disp.insert("Port".into(), plist::Value::String("60001".into()));
            let mut svcs = plist::Dictionary::new();
            svcs.insert(DISPLAY_SERVICE.into(), plist::Value::Dictionary(disp));
            let mut root = plist::Dictionary::new();
            root.insert("UUID".into(), plist::Value::String("RSD-TEST-UUID".into()));
            root.insert("Services".into(), plist::Value::Dictionary(svcs));
            write_len_plist(&mut rs, root);
        });

        std::env::set_var("USBMUXD_SOCKET_ADDRESS", mux_addr.to_string());
        let res = block_on(Link::usb("00008150-MOCKTUNNEL"));
        std::env::remove_var("USBMUXD_SOCKET_ADDRESS");

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let link = res.expect("Link::usb should establish software tunnel and RSD handshake");
        assert!(link.display_offered());
        assert_eq!(link.rsd_port, rsd_port);
        assert_eq!(link.adapter.host_ip().to_string(), "fd00::1");

        let _ = server_handle.join();
        let _ = rsd_handle.join();
    }

    #[test]
    fn test_wifi_remote_pairing_tls_psk_tunnel_and_rsd_handshake() {
        let ctrl_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let ctrl_addr = ctrl_listener.local_addr().unwrap();
        let data_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let data_port = data_listener.local_addr().unwrap().port();
        let rsd_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let rsd_port = rsd_listener.local_addr().unwrap().port();

        let irk: [u8; 16] = core::array::from_fn(|i| (i as u8).wrapping_add(11));
        let enc_key = vec![0xA5u8; 32];
        let enc_key_ctrl = enc_key.clone();
        let enc_key_data = enc_key.clone();

        let record = crate::wifi::Record {
            udid: "00008150-WIFILINK".to_string(),
            peer_alt_irk: irk,
            private_key: vec![1u8; 32],
            public_key: vec![2u8; 32],
        };

        let ctrl_handle = std::thread::spawn(move || {
            let (mut cs, _) = ctrl_listener.accept().unwrap();
            let req1 = read_len_plist(&mut cs);
            assert_eq!(
                req1.get("type").and_then(|v| v.as_string()),
                Some("attemptPairVerify")
            );
            let mut r1 = plist::Dictionary::new();
            r1.insert("verified".into(), plist::Value::Boolean(true));
            write_len_plist(&mut cs, r1);

            let req2 = read_len_plist(&mut cs);
            assert_eq!(
                req2.get("type").and_then(|v| v.as_string()),
                Some("validatePairing")
            );
            let ident = req2.get("identifier").and_then(|v| v.as_string()).unwrap();
            let auth_tag = req2.get("authTag").and_then(|v| v.as_string()).unwrap();
            assert!(crate::wifi::auth_tag_matches(&irk, ident, auth_tag));
            let mut r2 = plist::Dictionary::new();
            r2.insert("verified".into(), plist::Value::Boolean(true));
            r2.insert("encryptionKey".into(), plist::Value::Data(enc_key_ctrl));
            write_len_plist(&mut cs, r2);

            let req3 = read_len_plist(&mut cs);
            assert_eq!(
                req3.get("type").and_then(|v| v.as_string()),
                Some("createTcpListener")
            );
            let mut r3 = plist::Dictionary::new();
            r3.insert("port".into(), plist::Value::from(data_port));
            write_len_plist(&mut cs, r3);
            std::thread::sleep(Duration::from_millis(120));
        });

        let data_handle = std::thread::spawn(move || {
            let (mut ds, _) = data_listener.accept().unwrap();
            let mut psk_hello = [0u8; 12];
            ds.read_exact(&mut psk_hello).unwrap();
            assert_eq!(&psk_hello[0..8], b"TLSPSK1\0");
            assert_eq!(
                &psk_hello[8..12],
                &idevice::remote_pairing::psk_key_tag(&enc_key_data)
            );
            ds.write_all(b"TLSPSKOK").unwrap();
            ds.flush().unwrap();

            let mut magic = [0u8; 8];
            ds.read_exact(&mut magic).unwrap();
            assert_eq!(&magic, b"CDTunnel");
            let mut lbuf = [0u8; 2];
            ds.read_exact(&mut lbuf).unwrap();
            let rlen = u16::from_be_bytes(lbuf) as usize;
            let mut rbuf = vec![0u8; rlen];
            ds.read_exact(&mut rbuf).unwrap();

            let reply = format!(
                r#"{{"clientParameters":{{"address":"fd00::10","netmask":"ffff:ffff::","mtu":1500}},"serverAddress":"fd00::20","serverRSDPort":{},"loopbackRsdPort":{}}}"#,
                rsd_port, rsd_port
            );
            ds.write_all(b"CDTunnel").unwrap();
            ds.write_all(&(reply.len() as u16).to_be_bytes()).unwrap();
            ds.write_all(reply.as_bytes()).unwrap();
            ds.flush().unwrap();
        });

        let rsd_handle = std::thread::spawn(move || {
            let (mut rs, _) = rsd_listener.accept().unwrap();
            let mut lb = [0u8; 4];
            rs.read_exact(&mut lb).unwrap();
            let plen = u32::from_be_bytes(lb) as usize;
            let mut pbuf = vec![0u8; plen];
            rs.read_exact(&mut pbuf).unwrap();

            let mut disp = plist::Dictionary::new();
            disp.insert(
                "Entitlement".into(),
                plist::Value::String("com.apple.coredevice.display".into()),
            );
            disp.insert("Port".into(), plist::Value::String("60001".into()));
            let mut svcs = plist::Dictionary::new();
            svcs.insert(DISPLAY_SERVICE.into(), plist::Value::Dictionary(disp));
            let mut root = plist::Dictionary::new();
            root.insert("UUID".into(), plist::Value::String("WIFI-RSD-UUID".into()));
            root.insert("Services".into(), plist::Value::Dictionary(svcs));
            write_len_plist(&mut rs, root);
        });

        let link = block_on(Link::wifi_with_record(ctrl_addr, &record))
            .expect("Link::wifi_with_record should complete TLS-PSK tunnel and RsdHandshake");
        assert!(link.wifi);
        assert!(link.display_offered());
        assert_eq!(link.rsd_port, rsd_port);
        assert!(link._control.is_some());
        assert_eq!(link.adapter.host_ip().to_string(), "fd00::10");
        assert_eq!(link.adapter.mss(), 1440);

        let _ = ctrl_handle.join();
        let _ = data_handle.join();
        let _ = rsd_handle.join();
    }

    fn read_http_request(stream: &mut std::net::TcpStream) -> (String, Vec<u8>) {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        let header_end = loop {
            let n = stream.read(&mut tmp).unwrap_or(0);
            if n == 0 {
                break buf.len();
            }
            buf.extend_from_slice(&tmp[..n]);
            if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break pos + 4;
            }
        };
        let hdr_str = String::from_utf8_lossy(&buf[..header_end]).to_string();
        let content_len = hdr_str
            .lines()
            .find_map(|l| {
                let (k, v) = l.split_once(':')?;
                if k.trim().eq_ignore_ascii_case("content-length") {
                    v.trim().parse::<usize>().ok()
                } else {
                    None
                }
            })
            .unwrap_or(0);
        let mut body = buf[header_end..].to_vec();
        while body.len() < content_len {
            let n = stream.read(&mut tmp).unwrap_or(0);
            if n == 0 {
                break;
            }
            body.extend_from_slice(&tmp[..n]);
        }
        body.truncate(content_len);
        let first_line = hdr_str.lines().next().unwrap_or("").to_string();
        (first_line, body)
    }

    fn write_http_response(stream: &mut std::net::TcpStream, status: &str, body: &[u8]) {
        let hdr = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(hdr.as_bytes());
        let _ = stream.write_all(body);
        let _ = stream.flush();
    }

    fn read_h2_frame_sync(s: &mut std::net::TcpStream) -> std::io::Result<(u8, u8, u32, Vec<u8>)> {
        let mut hdr = [0u8; 9];
        s.read_exact(&mut hdr)?;
        let len = ((hdr[0] as usize) << 16) | ((hdr[1] as usize) << 8) | (hdr[2] as usize);
        let ftype = hdr[3];
        let flags = hdr[4];
        let stream_id = u32::from_be_bytes([hdr[5], hdr[6], hdr[7], hdr[8]]) & 0x7FFF_FFFF;
        let mut body = vec![0u8; len];
        if len > 0 {
            s.read_exact(&mut body)?;
        }
        Ok((ftype, flags, stream_id, body))
    }

    #[test]
    fn test_tahap251_m3_ios27_fresh_reboot_missing_ddi_triggers_ensure_ddi_and_rsd_reconnect() {
        let _guard = MOCK_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp_ddi_root = std::env::temp_dir().join(format!(
            "ipl-m3-ios27-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&temp_ddi_root);

        // Synthetic Cryptex1 DDI assets (RFC 5737 / fully synthetic)
        let img_bytes = b"SYNTHETIC-IOS27-CRYPTEX-DMG-PAYLOAD".to_vec();
        let tc_bytes = b"SYNTHETIC-IOS27-TRUSTCACHE".to_vec();
        let info_bytes = b"<plist version=\"1.0\"><dict><key>CFBundleIdentifier</key><string>com.apple.MobileAsset.DDI</string></dict></plist>".to_vec();
        let hash_bytes = b"SYNTHETIC-IOS27-ROOTHASH".to_vec();
        let ticket_bytes = b"SYNTHETIC-CRYPTEX1-IM4M-TICKET-251".to_vec();
        let expected_nonce: Vec<u8> = (0u8..48u8).map(|i| i.wrapping_add(0x21)).collect();
        let mut wrapped_nonce = Vec::with_capacity(56);
        wrapped_nonce.extend_from_slice(&[0x00, 0x01]);
        wrapped_nonce.extend_from_slice(&expected_nonce);
        wrapped_nonce.extend_from_slice(&[0x00, 0x00]);
        wrapped_nonce.extend_from_slice(&48u32.to_le_bytes());

        let mut manifest_dict = plist::Dictionary::new();
        let mut comp_tc = plist::Dictionary::new();
        comp_tc.insert("Digest".into(), plist::Value::Data(vec![0xAB; 48]));
        let mut comp_tc_info = plist::Dictionary::new();
        comp_tc_info.insert("Personalize".into(), plist::Value::Boolean(true));
        comp_tc.insert("Info".into(), plist::Value::Dictionary(comp_tc_info));
        manifest_dict.insert("Cryptex1,GenericTrustCache".into(), plist::Value::Dictionary(comp_tc));
        let mut comp_dmg = plist::Dictionary::new();
        comp_dmg.insert("Digest".into(), plist::Value::Data(vec![0xCD; 48]));
        let mut comp_dmg_info = plist::Dictionary::new();
        comp_dmg_info.insert("Personalize".into(), plist::Value::Boolean(false));
        comp_dmg.insert("Info".into(), plist::Value::Dictionary(comp_dmg_info));
        manifest_dict.insert("Cryptex1,GenericDmg".into(), plist::Value::Dictionary(comp_dmg));

        let mut id_info = plist::Dictionary::new();
        id_info.insert("Variant".into(), plist::Value::String("iOS Developer Disk Image Cryptex".into()));
        let mut identity = plist::Dictionary::new();
        identity.insert("Info".into(), plist::Value::Dictionary(id_info));
        identity.insert("Cryptex1,ChipID".into(), plist::Value::String("0x20000".into()));
        identity.insert("Cryptex1,Type".into(), plist::Value::String("Cryptex1".into()));
        identity.insert("Cryptex1,SubType".into(), plist::Value::from(1u64));
        identity.insert("Cryptex1,ProductClass".into(), plist::Value::from(2u64));
        identity.insert("Cryptex1,UseProductClass".into(), plist::Value::Boolean(true));
        identity.insert("Cryptex1,NonceDomain".into(), plist::Value::from(4u64));
        identity.insert("Cryptex1,Version".into(), plist::Value::String("27.2".into()));
        identity.insert("Cryptex1,PreauthorizationVersion".into(), plist::Value::String("1.0".into()));
        identity.insert("Manifest".into(), plist::Value::Dictionary(manifest_dict));
        let mut bm_root = plist::Dictionary::new();
        bm_root.insert("BuildIdentities".into(), plist::Value::Array(vec![plist::Value::Dictionary(identity)]));
        let bm_bytes = plist::to_bytes_xml(&plist::Value::Dictionary(bm_root)).unwrap();

        // 1) Local Mock HTTP DDI Asset Server (serves all 5 Cryptex1 files offline)
        let ddi_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let ddi_port = ddi_listener.local_addr().unwrap().port();
        let ddi_stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ddi_stop_srv = ddi_stop.clone();
        ddi_listener.set_nonblocking(true).unwrap();
        let (bm_c, img_c, tc_c, info_c, hash_c) = (
            bm_bytes.clone(),
            img_bytes.clone(),
            tc_bytes.clone(),
            info_bytes.clone(),
            hash_bytes.clone(),
        );
        let ddi_srv = std::thread::spawn(move || {
            while !ddi_stop_srv.load(std::sync::atomic::Ordering::Relaxed) {
                match ddi_listener.accept() {
                    Ok((mut s, _)) => {
                        let _ = s.set_nonblocking(false);
                        let (req_line, _) = read_http_request(&mut s);
                        if req_line.contains("/Xcode_iOS_DDI_Cryptex/BuildManifest.plist") {
                            write_http_response(&mut s, "200 OK", &bm_c);
                        } else if req_line.contains("/Xcode_iOS_DDI_Cryptex/Image.dmg.trustcache") {
                            write_http_response(&mut s, "200 OK", &tc_c);
                        } else if req_line.contains("/Xcode_iOS_DDI_Cryptex/Image.dmg.cryptex_info") {
                            write_http_response(&mut s, "200 OK", &info_c);
                        } else if req_line.contains("/Xcode_iOS_DDI_Cryptex/Image.dmg.root_hash") {
                            write_http_response(&mut s, "200 OK", &hash_c);
                        } else if req_line.contains("/Xcode_iOS_DDI_Cryptex/Image.dmg") {
                            write_http_response(&mut s, "200 OK", &img_c);
                        } else {
                            write_http_response(&mut s, "404 Not Found", b"not found");
                        }
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });

        // 2) Local Mock Apple TSS Server (validates Cryptex1 TSS request tags & returns Cryptex1,Ticket)
        let tss_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let tss_port = tss_listener.local_addr().unwrap().port();
        let tss_verified = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let tss_verified_srv = tss_verified.clone();
        let ticket_for_tss = ticket_bytes.clone();
        let nonce_for_tss = expected_nonce.clone();
        let tss_srv = std::thread::spawn(move || {
            let (mut s, _) = tss_listener.accept().unwrap();
            let (req_line, body) = read_http_request(&mut s);
            assert!(req_line.starts_with("POST /TSS/controller?action=2"));
            let req_plist = plist::from_bytes(&body).unwrap().into_dictionary().unwrap();
            assert_eq!(req_plist.get("@Cryptex1,Ticket").and_then(|v| v.as_boolean()), Some(true));
            assert_eq!(req_plist.get("Cryptex1,Nonce").and_then(|v| v.as_data()), Some(nonce_for_tss.as_slice()));
            let udid_bytes = req_plist.get("Cryptex1,UDID").and_then(|v| v.as_data()).unwrap();
            assert_eq!(udid_bytes.len(), 16);
            assert_eq!(&udid_bytes[0..8], &0x8150u64.to_be_bytes());
            assert_eq!(&udid_bytes[8..16], &0x0001_0002_0003_0004u64.to_be_bytes());
            assert!(req_plist.contains_key("Cryptex1,GenericTrustCache"));
            assert!(!req_plist.contains_key("Cryptex1,GenericDmg"));
            tss_verified_srv.store(true, std::sync::atomic::Ordering::SeqCst);

            let mut resp_dict = plist::Dictionary::new();
            resp_dict.insert("Cryptex1,Ticket".into(), plist::Value::Data(ticket_for_tss));
            let xml = plist::to_bytes_xml(&plist::Value::Dictionary(resp_dict)).unwrap();
            let mut full = b"STATUS=0&MESSAGE=SUCCESS&REQUEST_STRING=".to_vec();
            full.extend_from_slice(&xml);
            write_http_response(&mut s, "200 OK", &full);
        });

        // 3) Local Mock RemoteXPC HTTP/2 cryptexd server
        let ddi_installed_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ddi_installed_cxd = ddi_installed_flag.clone();
        let transferred_files = std::sync::Arc::new(Mutex::new(Vec::<(u64, Vec<u8>)>::new()));
        let transferred_files_cxd = transferred_files.clone();
        let cxd_listener2 = TcpListener::bind("127.0.0.1:0").unwrap();
        let cxd_port2 = cxd_listener2.local_addr().unwrap().port();
        let cxd_stop2 = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cxd_stop2_srv = cxd_stop2.clone();
        cxd_listener2.set_nonblocking(true).unwrap();
        let wrapped_nonce_cxd = wrapped_nonce.clone();
        let cxd_srv2 = std::thread::spawn(move || {
            use idevice::remote_xpc::*;
            while !cxd_stop2_srv.load(std::sync::atomic::Ordering::Relaxed) {
                match cxd_listener2.accept() {
                    Ok((mut s, _)) => {
                        let _ = s.set_nonblocking(false);
                        let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
                        let mut magic = [0u8; 24];
                        if s.read_exact(&mut magic).is_err() {
                            continue;
                        }
                        let mut srv_settings = Vec::new();
                        srv_settings.extend_from_slice(&4u16.to_be_bytes());
                        srv_settings.extend_from_slice(&(1024u32 * 1024).to_be_bytes());
                        let _ = s.write_all(&build_h2_frame(4, 0, 0, &srv_settings));
                        let _ = s.write_all(&build_h2_frame(8, 0, 0, &(1024u32 * 1024).to_be_bytes()));
                        let _ = s.flush();

                        let mut root_acc = Vec::new();
                        let mut decoded_rpc: Option<XpcObject> = None;
                        while decoded_rpc.is_none() {
                            let Ok((ft, _fl, sid, body)) = read_h2_frame_sync(&mut s) else {
                                break;
                            };
                            if ft == 0 && sid == 1 && !body.is_empty() {
                                root_acc.extend_from_slice(&body);
                                while root_acc.len() >= 24 {
                                    let sz = u64::from_le_bytes([
                                        root_acc[8], root_acc[9], root_acc[10], root_acc[11],
                                        root_acc[12], root_acc[13], root_acc[14], root_acc[15],
                                    ]) as usize;
                                    if root_acc.len() < 24 + sz {
                                        break;
                                    }
                                    let frame = root_acc[..24 + sz].to_vec();
                                    root_acc.drain(..24 + sz);
                                    if sz >= 12 {
                                        let mut off = 32usize;
                                        if let Some(obj) = decode_xpc_object(&frame, &mut off) {
                                            if obj.dict_get("routine").is_some() {
                                                decoded_rpc = Some(obj);
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        let Some(rpc) = decoded_rpc else {
                            continue;
                        };
                        let routine = rpc.dict_get("routine").and_then(XpcObject::as_str).unwrap_or("");
                        let argv = rpc.dict_get("argv");
                        let reply = match routine {
                            "copy-installed" => {
                                let arr = if ddi_installed_cxd.load(std::sync::atomic::Ordering::SeqCst) {
                                    vec![XpcObject::Dictionary(vec![
                                        ("remote-cryptex-identifier".into(), XpcObject::String("com.apple.MobileAsset.DDI".into())),
                                        ("remote-cryptex-version".into(), XpcObject::String("27.2".into())),
                                    ])]
                                } else {
                                    Vec::new()
                                };
                                XpcObject::Dictionary(vec![
                                    ("error".into(), XpcObject::Int64(0)),
                                    ("argv".into(), XpcObject::Dictionary(vec![
                                        ("remote-cryptex-array".into(), XpcObject::Array(arr)),
                                    ])),
                                ])
                            }
                            "read-personalization-id" => XpcObject::Dictionary(vec![
                                ("error".into(), XpcObject::Int64(0)),
                                ("argv".into(), XpcObject::Dictionary(vec![
                                    ("img4_chip_chip".into(), XpcObject::Uint64(0x8150)),
                                    ("img4_chip_ecid".into(), XpcObject::Uint64(0x0001_0002_0003_0004)),
                                    ("img4_chip_bord".into(), XpcObject::Uint64(0x0A)),
                                    ("img4_chip_cpro".into(), XpcObject::Uint64(1)),
                                    ("img4_chip_csec".into(), XpcObject::Uint64(1)),
                                    ("img4_chip_sdom".into(), XpcObject::Uint64(1)),
                                ])),
                            ]),
                            "get-nonce" => {
                                let handle = argv
                                    .and_then(|a| a.dict_get("nonce-domain-handle"))
                                    .and_then(XpcObject::as_u64);
                                assert_eq!(handle, Some(4), "cryptexd get-nonce MUST pass nonce-domain-handle=4");
                                XpcObject::Dictionary(vec![
                                    ("error".into(), XpcObject::Int64(0)),
                                    ("argv".into(), XpcObject::Dictionary(vec![
                                        ("nonce".into(), XpcObject::Data(wrapped_nonce_cxd.clone())),
                                    ])),
                                ])
                            }
                            "install" => {
                                let a = argv.unwrap();
                                assert_eq!(a.dict_get("client-version").and_then(XpcObject::as_u64), Some(3));
                                assert_eq!(a.dict_get("image-type-index").and_then(XpcObject::as_i64), Some(10));
                                assert_eq!(a.dict_get("persistence").and_then(XpcObject::as_u64), Some(2));
                                assert_eq!(a.dict_get("nonce-persistence").and_then(XpcObject::as_u64), Some(1));
                                let props = a.dict_get("cryptex1-properties").unwrap();
                                assert_eq!(props.dict_get("Cryptex1,NonceDomain"), Some(&XpcObject::Uint64(4)));
                                assert_eq!(props.dict_get("Cryptex1,SubType"), Some(&XpcObject::Uint64(1)));
                                assert_eq!(props.dict_get("Cryptex1,PreauthVersion").and_then(XpcObject::as_str), Some("1.0"));

                                // Receive all 5 FileTransfer streams (streams 5, 7, 9, 11, 13)
                                let mut stream_bufs: std::collections::HashMap<u32, Vec<u8>> = std::collections::HashMap::new();
                                let mut ended_streams = 0usize;
                                while ended_streams < 5 {
                                    let Ok((ft, fl, sid, body)) = read_h2_frame_sync(&mut s) else {
                                        break;
                                    };
                                    if ft == 0 && sid >= 5 {
                                        if !body.is_empty() {
                                            stream_bufs.entry(sid).or_default().extend_from_slice(&body);
                                        }
                                        if (fl & 0x01) != 0 {
                                            ended_streams += 1;
                                        }
                                    }
                                }
                                let mut out_tx = transferred_files_cxd.lock().unwrap();
                                for sid in [5u32, 7, 9, 11, 13] {
                                    let raw = stream_bufs.remove(&sid).unwrap_or_default();
                                    assert!(raw.len() >= 24, "FileTransfer stream {sid} missing preamble");
                                    let tx_id = u64::from_le_bytes([
                                        raw[16], raw[17], raw[18], raw[19],
                                        raw[20], raw[21], raw[22], raw[23],
                                    ]);
                                    out_tx.push((tx_id, raw[24..].to_vec()));
                                }
                                ddi_installed_cxd.store(true, std::sync::atomic::Ordering::SeqCst);
                                XpcObject::Dictionary(vec![
                                    ("error".into(), XpcObject::Int64(0)),
                                ])
                            }
                            _ => XpcObject::Dictionary(vec![("error".into(), XpcObject::Int64(22))]),
                        };
                        let resp_wrap = build_xpc_wrapper(
                            XPC_FLAG_ALWAYS_SET | XPC_FLAG_DATA_PRESENT | XPC_FLAG_REPLY,
                            1,
                            Some(&reply),
                        );
                        let _ = s.write_all(&build_h2_frame(0, 0, 3, &resp_wrap));
                        let _ = s.flush();
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });

        // 4) Local Mock RSD server: Handshake #1 omits DisplayService; Handshake #2 (after install) advertises DisplayService!
        let rsd_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let rsd_port = rsd_listener.local_addr().unwrap().port();
        let ddi_installed_rsd = ddi_installed_flag.clone();
        let rsd_srv = std::thread::spawn(move || {
            for _ in 0..2 {
                let (mut rs, _) = rsd_listener.accept().unwrap();
                let mut lb = [0u8; 4];
                rs.read_exact(&mut lb).unwrap();
                let plen = u32::from_be_bytes(lb) as usize;
                let mut pbuf = vec![0u8; plen];
                rs.read_exact(&mut pbuf).unwrap();

                let mut svcs = plist::Dictionary::new();
                let mut cxd_svc = plist::Dictionary::new();
                cxd_svc.insert("Entitlement".into(), plist::Value::String("com.apple.security.cryptexd.remote".into()));
                cxd_svc.insert("Port".into(), plist::Value::String(cxd_port2.to_string()));
                svcs.insert("com.apple.security.cryptexd.remote".into(), plist::Value::Dictionary(cxd_svc));

                if ddi_installed_rsd.load(std::sync::atomic::Ordering::SeqCst) {
                    let mut disp = plist::Dictionary::new();
                    disp.insert("Entitlement".into(), plist::Value::String("com.apple.coredevice.display".into()));
                    disp.insert("Port".into(), plist::Value::String("60001".into()));
                    svcs.insert(DISPLAY_SERVICE.into(), plist::Value::Dictionary(disp));
                }

                let mut root = plist::Dictionary::new();
                root.insert("UUID".into(), plist::Value::String("M3-IOS27-RSD-UUID".into()));
                root.insert("Services".into(), plist::Value::Dictionary(svcs));
                write_len_plist(&mut rs, root);
            }
        });

        // 5) Local Mock usbmuxd + Lockdown (DeveloperModeStatus=true) + CoreDeviceProxy
        let mux_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mux_addr = mux_listener.local_addr().unwrap();
        let mux_stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mux_stop_srv = mux_stop.clone();
        mux_listener.set_nonblocking(true).unwrap();
        let mux_srv = std::thread::spawn(move || {
            let mut handlers = Vec::new();
            while !mux_stop_srv.load(std::sync::atomic::Ordering::Relaxed) {
                match mux_listener.accept() {
                    Ok((mut s, _)) => {
                        let _ = s.set_nonblocking(false);
                        let _ = s.set_read_timeout(Some(Duration::from_secs(3)));
                        handlers.push(std::thread::spawn(move || {
                            let req = read_usbmux_req(&mut s);
                            match req.get("MessageType").and_then(|v| v.as_string()).unwrap_or("") {
                                "ListDevices" => {
                                    let mut props = plist::Dictionary::new();
                                    props.insert("SerialNumber".into(), plist::Value::String("00008150-0001000200030004".into()));
                                    props.insert("DeviceID".into(), plist::Value::from(11u32));
                                    props.insert("ConnectionType".into(), plist::Value::String("USB".into()));
                                    let mut dev = plist::Dictionary::new();
                                    dev.insert("Properties".into(), plist::Value::Dictionary(props));
                                    let mut resp = plist::Dictionary::new();
                                    resp.insert("DeviceList".into(), plist::Value::Array(vec![plist::Value::Dictionary(dev)]));
                                    write_usbmux_resp(&mut s, resp);
                                }
                                "ReadPairRecord" => {
                                    let mut pair = plist::Dictionary::new();
                                    pair.insert("HostID".into(), plist::Value::String("M3-HOST".into()));
                                    pair.insert("SystemBUID".into(), plist::Value::String("M3-BUID".into()));
                                    let pair_bytes = plist::to_bytes_xml(&plist::Value::Dictionary(pair)).unwrap();
                                    let mut resp = plist::Dictionary::new();
                                    resp.insert("PairRecordData".into(), plist::Value::Data(pair_bytes));
                                    write_usbmux_resp(&mut s, resp);
                                }
                                "Connect" => {
                                    let port = u16::from_be(req.get("PortNumber").and_then(|v| v.as_unsigned_integer()).unwrap() as u16);
                                    let mut ok_resp = plist::Dictionary::new();
                                    ok_resp.insert("Number".into(), plist::Value::from(0u32));
                                    write_usbmux_resp(&mut s, ok_resp);

                                    if port == 62078 {
                                        for _ in 0..4 {
                                            let mut lb = [0u8; 4];
                                            if s.read_exact(&mut lb).is_err() {
                                                break;
                                            }
                                            let len = u32::from_be_bytes(lb) as usize;
                                            let mut body = vec![0u8; len];
                                            if s.read_exact(&mut body).is_err() {
                                                break;
                                            }
                                            let lreq = plist::from_bytes(&body).unwrap().into_dictionary().unwrap();
                                            let kind = lreq.get("Request").and_then(|v| v.as_string()).unwrap_or("");
                                            let mut lresp = plist::Dictionary::new();
                                            match kind {
                                                "StartSession" => {
                                                    lresp.insert("EnableSessionSSL".into(), plist::Value::Boolean(false));
                                                    lresp.insert("SessionID".into(), plist::Value::String("M3-SESS".into()));
                                                }
                                                "GetValue" => {
                                                    let key = lreq.get("Key").and_then(|v| v.as_string()).unwrap_or("");
                                                    match key {
                                                        "DeveloperModeStatus" => {
                                                            lresp.insert("Value".into(), plist::Value::Boolean(true));
                                                        }
                                                        "ProductVersion" => {
                                                            lresp.insert("Value".into(), plist::Value::String("27.2".into()));
                                                        }
                                                        _ => {
                                                            lresp.insert("Value".into(), plist::Value::String("00008150-0001000200030004".into()));
                                                        }
                                                    }
                                                }
                                                "StartService" => {
                                                    lresp.insert("Port".into(), plist::Value::from(59111u16));
                                                    lresp.insert("EnableServiceSSL".into(), plist::Value::Boolean(false));
                                                }
                                                _ => {}
                                            }
                                            write_len_plist(&mut s, lresp);
                                        }
                                    } else if port == 59111 {
                                        let mut magic = [0u8; 8];
                                        s.read_exact(&mut magic).unwrap();
                                        let mut lbuf = [0u8; 2];
                                        s.read_exact(&mut lbuf).unwrap();
                                        let rlen = u16::from_be_bytes(lbuf) as usize;
                                        let mut rbuf = vec![0u8; rlen];
                                        s.read_exact(&mut rbuf).unwrap();
                                        let reply = format!(
                                            r#"{{"clientParameters":{{"address":"fd00::1","netmask":"ffff:ffff::","mtu":16000}},"serverAddress":"fd00::2","serverRSDPort":{},"loopbackRsdPort":{}}}"#,
                                            rsd_port, rsd_port
                                        );
                                        s.write_all(b"CDTunnel").unwrap();
                                        s.write_all(&(reply.len() as u16).to_be_bytes()).unwrap();
                                        s.write_all(reply.as_bytes()).unwrap();
                                        s.flush().unwrap();
                                    }
                                }
                                _ => {}
                            }
                        }));
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
            for h in handlers {
                let _ = h.join();
            }
        });

        std::env::set_var("USBMUXD_SOCKET_ADDRESS", mux_addr.to_string());
        std::env::set_var(ddi::ENV_DDI_DIR, temp_ddi_root.to_string_lossy().to_string());
        std::env::set_var(ddi::ENV_DDI_BASE_URL, format!("http://127.0.0.1:{ddi_port}"));
        std::env::set_var(
            idevice::tss::ENV_TSS_URL,
            format!("http://127.0.0.1:{tss_port}/TSS/controller?action=2"),
        );

        let mut link = block_on(Link::usb("00008150-0001000200030004"))
            .expect("Link::usb should establish RSD tunnel");
        assert!(
            !link.display_offered(),
            "Fresh-reboot iPhone must NOT offer DisplayService before DDI mount"
        );

        let mut progress_msgs = Vec::new();
        let outcome = block_on(link.ensure_ddi("27.2", |msg| {
            progress_msgs.push(msg.to_string());
        }))
        .expect("ensure_ddi should auto-download, TSS-sign, install Cryptex1, and reconnect RSD");

        // Second call must short-circuit as AlreadyMounted with zero network calls!
        let outcome_second = block_on(link.ensure_ddi("27.2", |_| {}))
            .expect("second ensure_ddi call must succeed immediately");

        std::env::remove_var("USBMUXD_SOCKET_ADDRESS");
        std::env::remove_var(ddi::ENV_DDI_DIR);
        std::env::remove_var(ddi::ENV_DDI_BASE_URL);
        std::env::remove_var(idevice::tss::ENV_TSS_URL);

        mux_stop.store(true, std::sync::atomic::Ordering::Relaxed);
        ddi_stop.store(true, std::sync::atomic::Ordering::Relaxed);
        cxd_stop2.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = mux_srv.join();
        let _ = rsd_srv.join();
        let _ = ddi_srv.join();
        let _ = tss_srv.join();
        let _ = cxd_srv2.join();

        assert_eq!(outcome, DdiMountOutcome::MountedNow);
        assert_eq!(outcome_second, DdiMountOutcome::AlreadyMounted);
        assert!(
            link.display_offered(),
            "DisplayService MUST appear on RSD after Cryptex1 DDI install + RSD reconnect"
        );
        assert!(tss_verified.load(std::sync::atomic::Ordering::SeqCst));
        assert!(!progress_msgs.is_empty());

        let tx_list = transferred_files.lock().unwrap();
        assert_eq!(tx_list.len(), 5, "All 5 Cryptex1 FileTransfer streams must be delivered");
        assert_eq!(tx_list[0], (1, img_bytes));
        assert_eq!(tx_list[1], (2, tc_bytes));
        assert_eq!(tx_list[2], (3, ticket_bytes));
        assert_eq!(tx_list[3], (4, info_bytes));
        assert_eq!(tx_list[4], (5, hash_bytes));

        // Verify zero .part leftovers and all 5 cached files exist
        let cached_folder = temp_ddi_root.join("Xcode_iOS_DDI_Cryptex");
        assert!(ddi::is_valid_ddi_dir(Kind::Cryptex, &cached_folder));
        for entry in std::fs::read_dir(&cached_folder).unwrap().flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            assert!(!name.ends_with(".part"), "Unexpected .part leftover: {name}");
        }
        let _ = std::fs::remove_dir_all(&temp_ddi_root);
    }

    #[test]
    fn test_tahap251_m3_ios18_personalized_ddi_lookup_nonce_tss_receive_bytes_mount() {
        let _guard = MOCK_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp_ddi_root = std::env::temp_dir().join(format!(
            "ipl-m3-ios18-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&temp_ddi_root);

        let img_bytes = b"SYNTHETIC-IOS18-PERSONALIZED-DMG-PAYLOAD-1234567890".to_vec();
        let tc_bytes = b"SYNTHETIC-IOS18-TRUSTCACHE-PAYLOAD".to_vec();
        let ticket_bytes = b"SYNTHETIC-AP-IMG4-TICKET-IOS18".to_vec();
        let device_nonce = vec![0x5Au8; 48];

        // Build synthetic iOS 18 Personalized BuildManifest.plist with RestoreRequestRules
        let mut cond = plist::Dictionary::new();
        cond.insert("ApRawProductionMode".into(), plist::Value::Boolean(true));
        let mut act = plist::Dictionary::new();
        act.insert("EPRO".into(), plist::Value::Boolean(true));
        let mut rule = plist::Dictionary::new();
        rule.insert("Conditions".into(), plist::Value::Dictionary(cond));
        rule.insert("Actions".into(), plist::Value::Dictionary(act));

        let mut tc_info = plist::Dictionary::new();
        tc_info.insert(
            "RestoreRequestRules".into(),
            plist::Value::Array(vec![plist::Value::Dictionary(rule)]),
        );
        let mut tc_entry = plist::Dictionary::new();
        tc_entry.insert("Digest".into(), plist::Value::Data(vec![0x77; 48]));
        tc_entry.insert("Trusted".into(), plist::Value::Boolean(true));
        tc_entry.insert("Info".into(), plist::Value::Dictionary(tc_info));

        let mut manifest_dict = plist::Dictionary::new();
        manifest_dict.insert("LoadableTrustCache".into(), plist::Value::Dictionary(tc_entry));
        let mut identity = plist::Dictionary::new();
        identity.insert("ApBoardID".into(), plist::Value::String("0x0C".into()));
        identity.insert("ApChipID".into(), plist::Value::String("0x8120".into()));
        identity.insert("Manifest".into(), plist::Value::Dictionary(manifest_dict));
        let mut bm_root = plist::Dictionary::new();
        bm_root.insert("BuildIdentities".into(), plist::Value::Array(vec![plist::Value::Dictionary(identity)]));
        let bm_bytes = plist::to_bytes_xml(&plist::Value::Dictionary(bm_root)).unwrap();

        // 1) Local Mock HTTP DDI Asset Server (3 Personalized files)
        let ddi_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let ddi_port = ddi_listener.local_addr().unwrap().port();
        let ddi_stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ddi_stop_srv = ddi_stop.clone();
        ddi_listener.set_nonblocking(true).unwrap();
        let (bm_c, img_c, tc_c) = (bm_bytes.clone(), img_bytes.clone(), tc_bytes.clone());
        let ddi_srv = std::thread::spawn(move || {
            while !ddi_stop_srv.load(std::sync::atomic::Ordering::Relaxed) {
                match ddi_listener.accept() {
                    Ok((mut s, _)) => {
                        let _ = s.set_nonblocking(false);
                        let (req_line, _) = read_http_request(&mut s);
                        if req_line.contains("/Xcode_iOS_DDI_Personalized/BuildManifest.plist") {
                            write_http_response(&mut s, "200 OK", &bm_c);
                        } else if req_line.contains("/Xcode_iOS_DDI_Personalized/Image.dmg.trustcache") {
                            write_http_response(&mut s, "200 OK", &tc_c);
                        } else if req_line.contains("/Xcode_iOS_DDI_Personalized/Image.dmg") {
                            write_http_response(&mut s, "200 OK", &img_c);
                        } else {
                            write_http_response(&mut s, "404 Not Found", b"not found");
                        }
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });

        // 2) Local Mock Apple TSS Server (validates Personalized tags + RestoreRequestRules)
        let tss_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let tss_port = tss_listener.local_addr().unwrap().port();
        let ticket_tss = ticket_bytes.clone();
        let nonce_tss = device_nonce.clone();
        let tss_srv = std::thread::spawn(move || {
            let (mut s, _) = tss_listener.accept().unwrap();
            let (_line, body) = read_http_request(&mut s);
            let req = plist::from_bytes(&body).unwrap().into_dictionary().unwrap();
            assert_eq!(req.get("@ApImg4Ticket").and_then(|v| v.as_boolean()), Some(true));
            assert_eq!(req.get("ApBoardID").and_then(|v| v.as_unsigned_integer()), Some(0x0C));
            assert_eq!(req.get("ApChipID").and_then(|v| v.as_unsigned_integer()), Some(0x8120));
            assert_eq!(req.get("ApECID").and_then(|v| v.as_unsigned_integer()), Some(9988776655u64));
            assert_eq!(req.get("ApNonce").and_then(|v| v.as_data()), Some(nonce_tss.as_slice()));
            assert_eq!(req.get("Ap,ProductType").and_then(|v| v.as_string()), Some("iPhone15,2"));
            let ltc = req.get("LoadableTrustCache").and_then(|v| v.as_dictionary()).unwrap();
            assert_eq!(ltc.get("EPRO").and_then(|v| v.as_boolean()), Some(true));

            let mut resp = plist::Dictionary::new();
            resp.insert("ApImg4Ticket".into(), plist::Value::Data(ticket_tss));
            let xml = plist::to_bytes_xml(&plist::Value::Dictionary(resp)).unwrap();
            let mut full = b"STATUS=0&MESSAGE=SUCCESS&REQUEST_STRING=".to_vec();
            full.extend_from_slice(&xml);
            write_http_response(&mut s, "200 OK", &full);
        });

        // 3) Local Mock usbmuxd + Lockdown + mobile_image_mounter
        let steps_seen = std::sync::Arc::new(Mutex::new(Vec::<String>::new()));
        let steps_srv = steps_seen.clone();
        let mux_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mux_addr = mux_listener.local_addr().unwrap();
        let mux_stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mux_stop_srv = mux_stop.clone();
        mux_listener.set_nonblocking(true).unwrap();
        let img_expected = img_bytes.clone();
        let tc_expected = tc_bytes.clone();
        let ticket_expected = ticket_bytes.clone();
        let nonce_dev = device_nonce.clone();
        let mux_srv = std::thread::spawn(move || {
            let mut handlers = Vec::new();
            while !mux_stop_srv.load(std::sync::atomic::Ordering::Relaxed) {
                match mux_listener.accept() {
                    Ok((mut s, _)) => {
                        let _ = s.set_nonblocking(false);
                        let _ = s.set_read_timeout(Some(Duration::from_secs(4)));
                        let steps_c = steps_srv.clone();
                        let img_e = img_expected.clone();
                        let tc_e = tc_expected.clone();
                        let tick_e = ticket_expected.clone();
                        let nonce_e = nonce_dev.clone();
                        handlers.push(std::thread::spawn(move || {
                            let req = read_usbmux_req(&mut s);
                            match req.get("MessageType").and_then(|v| v.as_string()).unwrap_or("") {
                                "ListDevices" => {
                                    let mut props = plist::Dictionary::new();
                                    props.insert("SerialNumber".into(), plist::Value::String("00008120-IOS18MOCK".into()));
                                    props.insert("DeviceID".into(), plist::Value::from(12u32));
                                    props.insert("ConnectionType".into(), plist::Value::String("USB".into()));
                                    let mut dev = plist::Dictionary::new();
                                    dev.insert("Properties".into(), plist::Value::Dictionary(props));
                                    let mut resp = plist::Dictionary::new();
                                    resp.insert("DeviceList".into(), plist::Value::Array(vec![plist::Value::Dictionary(dev)]));
                                    write_usbmux_resp(&mut s, resp);
                                }
                                "ReadPairRecord" => {
                                    let mut pair = plist::Dictionary::new();
                                    pair.insert("HostID".into(), plist::Value::String("M3-H18".into()));
                                    pair.insert("SystemBUID".into(), plist::Value::String("M3-B18".into()));
                                    let pb = plist::to_bytes_xml(&plist::Value::Dictionary(pair)).unwrap();
                                    let mut resp = plist::Dictionary::new();
                                    resp.insert("PairRecordData".into(), plist::Value::Data(pb));
                                    write_usbmux_resp(&mut s, resp);
                                }
                                "Connect" => {
                                    let port = u16::from_be(req.get("PortNumber").and_then(|v| v.as_unsigned_integer()).unwrap() as u16);
                                    let mut ok_resp = plist::Dictionary::new();
                                    ok_resp.insert("Number".into(), plist::Value::from(0u32));
                                    write_usbmux_resp(&mut s, ok_resp);
                                    if port == 62078 {
                                        for _ in 0..5 {
                                            let mut lb = [0u8; 4];
                                            if s.read_exact(&mut lb).is_err() {
                                                break;
                                            }
                                            let len = u32::from_be_bytes(lb) as usize;
                                            let mut body = vec![0u8; len];
                                            if s.read_exact(&mut body).is_err() {
                                                break;
                                            }
                                            let lreq = plist::from_bytes(&body).unwrap().into_dictionary().unwrap();
                                            let kind = lreq.get("Request").and_then(|v| v.as_string()).unwrap_or("");
                                            let mut lresp = plist::Dictionary::new();
                                            match kind {
                                                "StartSession" => {
                                                    lresp.insert("EnableSessionSSL".into(), plist::Value::Boolean(false));
                                                }
                                                "GetValue" => {
                                                    let key = lreq.get("Key").and_then(|v| v.as_string()).unwrap_or("");
                                                    match key {
                                                        "UniqueChipID" => {
                                                            lresp.insert("Value".into(), plist::Value::from(9988776655u64));
                                                        }
                                                        "DeveloperModeStatus" => {
                                                            lresp.insert("Value".into(), plist::Value::Boolean(true));
                                                        }
                                                        "ProductVersion" => {
                                                            lresp.insert("Value".into(), plist::Value::String("18.5".into()));
                                                        }
                                                        _ => {
                                                            lresp.insert("Value".into(), plist::Value::String("00008120-IOS18MOCK".into()));
                                                        }
                                                    }
                                                }
                                                "StartService" => {
                                                    lresp.insert("Port".into(), plist::Value::from(59222u16));
                                                    lresp.insert("EnableServiceSSL".into(), plist::Value::Boolean(false));
                                                }
                                                _ => {}
                                            }
                                            write_len_plist(&mut s, lresp);
                                        }
                                    } else if port == 59222 {
                                        loop {
                                            let mut lb = [0u8; 4];
                                            if s.read_exact(&mut lb).is_err() {
                                                break;
                                            }
                                            let len = u32::from_be_bytes(lb) as usize;
                                            let mut body = vec![0u8; len];
                                            if s.read_exact(&mut body).is_err() {
                                                break;
                                            }
                                            let mreq = plist::from_bytes(&body).unwrap().into_dictionary().unwrap();
                                            let cmd = mreq.get("Command").and_then(|v| v.as_string()).unwrap_or("").to_string();
                                            steps_c.lock().unwrap().push(cmd.clone());
                                            let mut mresp = plist::Dictionary::new();
                                            match cmd.as_str() {
                                                "LookupImage" => {
                                                    mresp.insert("ImagePresent".into(), plist::Value::Boolean(false));
                                                    write_len_plist(&mut s, mresp);
                                                }
                                                "CopyDevices" => {
                                                    mresp.insert("EntryList".into(), plist::Value::Array(Vec::new()));
                                                    write_len_plist(&mut s, mresp);
                                                }
                                                "QueryPersonalizationManifest" => {
                                                    mresp.insert("Error".into(), plist::Value::String("MissingManifest".into()));
                                                    write_len_plist(&mut s, mresp);
                                                    break; // Simulate real iOS closing connection after missing manifest!
                                                }
                                                "QueryPersonalizationIdentifiers" => {
                                                    let mut ids = plist::Dictionary::new();
                                                    ids.insert("BoardId".into(), plist::Value::from(0x0Cu64));
                                                    ids.insert("ChipID".into(), plist::Value::from(0x8120u64));
                                                    ids.insert("Ap,ProductType".into(), plist::Value::String("iPhone15,2".into()));
                                                    mresp.insert("PersonalizationIdentifiers".into(), plist::Value::Dictionary(ids));
                                                    write_len_plist(&mut s, mresp);
                                                }
                                                "QueryNonce" => {
                                                    assert_eq!(mreq.get("PersonalizedImageType").and_then(|v| v.as_string()), Some("DeveloperDiskImage"));
                                                    mresp.insert("PersonalizationNonce".into(), plist::Value::Data(nonce_e.clone()));
                                                    write_len_plist(&mut s, mresp);
                                                }
                                                "ReceiveBytes" => {
                                                    let sz = mreq.get("ImageSize").and_then(|v| v.as_unsigned_integer()).unwrap() as usize;
                                                    assert_eq!(sz, img_e.len());
                                                    mresp.insert("Status".into(), plist::Value::String("ReceiveBytesAck".into()));
                                                    write_len_plist(&mut s, mresp);
                                                    let mut raw = vec![0u8; sz];
                                                    s.read_exact(&mut raw).unwrap();
                                                    assert_eq!(raw, img_e);
                                                    let mut done = plist::Dictionary::new();
                                                    done.insert("Status".into(), plist::Value::String("Complete".into()));
                                                    write_len_plist(&mut s, done);
                                                }
                                                "MountImage" => {
                                                    assert_eq!(mreq.get("ImageSignature").and_then(|v| v.as_data()), Some(tick_e.as_slice()));
                                                    assert_eq!(mreq.get("ImageTrustCache").and_then(|v| v.as_data()), Some(tc_e.as_slice()));
                                                    mresp.insert("Status".into(), plist::Value::String("Complete".into()));
                                                    write_len_plist(&mut s, mresp);
                                                }
                                                _ => break,
                                            }
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }));
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
            for h in handlers {
                let _ = h.join();
            }
        });

        std::env::set_var("USBMUXD_SOCKET_ADDRESS", mux_addr.to_string());
        std::env::set_var(ddi::ENV_DDI_DIR, temp_ddi_root.to_string_lossy().to_string());
        std::env::set_var(ddi::ENV_DDI_BASE_URL, format!("http://127.0.0.1:{ddi_port}"));
        std::env::set_var(
            idevice::tss::ENV_TSS_URL,
            format!("http://127.0.0.1:{tss_port}/TSS/controller?action=2"),
        );

        let prov = block_on(provider("00008120-IOS18MOCK")).unwrap();
        let mut dummy_adapter = idevice::tcp::adapter::Adapter::new(
            Box::new(std::io::Cursor::new(Vec::new())),
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        )
        .to_async_handle();
        let mut rsd = RsdHandshake {
            services: std::collections::HashMap::new(),
            protocol_version: 1,
            uuid: "M3-IOS18-UUID".into(),
        };

        let outcome = block_on(ensure_ddi_with_rsd(
            &mut dummy_adapter,
            &mut rsd,
            Some(&prov),
            "18.5",
            |_stage, _pct, _detail| {},
        ))
        .expect("iOS 18.5 Personalized DDI mount should succeed end-to-end");

        std::env::remove_var("USBMUXD_SOCKET_ADDRESS");
        std::env::remove_var(ddi::ENV_DDI_DIR);
        std::env::remove_var(ddi::ENV_DDI_BASE_URL);
        std::env::remove_var(idevice::tss::ENV_TSS_URL);

        mux_stop.store(true, std::sync::atomic::Ordering::Relaxed);
        ddi_stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = mux_srv.join();
        let _ = ddi_srv.join();
        let _ = tss_srv.join();

        assert_eq!(outcome, DdiMountOutcome::MountedNow);
        let steps = steps_seen.lock().unwrap();
        assert_eq!(
            steps.as_slice(),
            &[
                "LookupImage",
                "CopyDevices",
                "QueryPersonalizationManifest",
                "QueryPersonalizationIdentifiers",
                "QueryNonce",
                "ReceiveBytes",
                "MountImage",
            ]
        );
        let cached_folder = temp_ddi_root.join("Xcode_iOS_DDI_Personalized");
        assert!(ddi::is_valid_ddi_dir(Kind::Personalized, &cached_folder));
        for entry in std::fs::read_dir(&cached_folder).unwrap().flatten() {
            assert!(!entry.file_name().to_string_lossy().ends_with(".part"));
        }
        let _ = std::fs::remove_dir_all(&temp_ddi_root);
    }

    #[test]
    fn test_tahap251_ddi_offline_and_devmode_off_graceful_errors() {
        let _guard = MOCK_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        // Subtest A: Offline DDI download fails gracefully with bilingual guidance & zero .part files
        let temp_offline = std::env::temp_dir().join(format!(
            "ipl-m3-offline-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp_offline);
        let dead_port = {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        std::env::set_var(ddi::ENV_DDI_DIR, temp_offline.to_string_lossy().to_string());
        std::env::set_var(ddi::ENV_DDI_BASE_URL, format!("http://127.0.0.1:{dead_port}"));

        let offline_err = ddi::ensure_ddi_files_downloaded(Kind::Cryptex, |_, _| {})
            .expect_err("Offline DDI download must return clear error");
        assert!(
            offline_err.contains("internet") && offline_err.contains("Failed to download"),
            "Offline error must be bilingual: {offline_err}"
        );
        let target_dir = temp_offline.join("Xcode_iOS_DDI_Cryptex");
        if target_dir.exists() {
            for entry in std::fs::read_dir(&target_dir).unwrap().flatten() {
                assert!(
                    !entry.file_name().to_string_lossy().ends_with(".part"),
                    "Offline failure must leave zero .part files"
                );
            }
        }
        std::env::remove_var(ddi::ENV_DDI_DIR);
        std::env::remove_var(ddi::ENV_DDI_BASE_URL);
        let _ = std::fs::remove_dir_all(&temp_offline);

        // Subtest B: Developer Mode OFF aborts immediately before download/TSS and reveals AMFI option
        let amfi_revealed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let amfi_revealed_srv = amfi_revealed.clone();
        let mux_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mux_addr = mux_listener.local_addr().unwrap();
        let mux_stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mux_stop_srv = mux_stop.clone();
        mux_listener.set_nonblocking(true).unwrap();
        let mux_srv = std::thread::spawn(move || {
            let mut handlers = Vec::new();
            while !mux_stop_srv.load(std::sync::atomic::Ordering::Relaxed) {
                match mux_listener.accept() {
                    Ok((mut s, _)) => {
                        let _ = s.set_nonblocking(false);
                        let _ = s.set_read_timeout(Some(Duration::from_secs(3)));
                        let rev_c = amfi_revealed_srv.clone();
                        handlers.push(std::thread::spawn(move || {
                            let req = read_usbmux_req(&mut s);
                            match req.get("MessageType").and_then(|v| v.as_string()).unwrap_or("") {
                                "ListDevices" => {
                                    let mut props = plist::Dictionary::new();
                                    props.insert("SerialNumber".into(), plist::Value::String("00008150-DEVMODEOFF".into()));
                                    props.insert("DeviceID".into(), plist::Value::from(15u32));
                                    props.insert("ConnectionType".into(), plist::Value::String("USB".into()));
                                    let mut dev = plist::Dictionary::new();
                                    dev.insert("Properties".into(), plist::Value::Dictionary(props));
                                    let mut resp = plist::Dictionary::new();
                                    resp.insert("DeviceList".into(), plist::Value::Array(vec![plist::Value::Dictionary(dev)]));
                                    write_usbmux_resp(&mut s, resp);
                                }
                                "ReadPairRecord" => {
                                    let mut pair = plist::Dictionary::new();
                                    pair.insert("HostID".into(), plist::Value::String("H-OFF".into()));
                                    pair.insert("SystemBUID".into(), plist::Value::String("B-OFF".into()));
                                    let pb = plist::to_bytes_xml(&plist::Value::Dictionary(pair)).unwrap();
                                    let mut resp = plist::Dictionary::new();
                                    resp.insert("PairRecordData".into(), plist::Value::Data(pb));
                                    write_usbmux_resp(&mut s, resp);
                                }
                                "Connect" => {
                                    let port = u16::from_be(req.get("PortNumber").and_then(|v| v.as_unsigned_integer()).unwrap() as u16);
                                    let mut ok_resp = plist::Dictionary::new();
                                    ok_resp.insert("Number".into(), plist::Value::from(0u32));
                                    write_usbmux_resp(&mut s, ok_resp);
                                    if port == 62078 {
                                        for _ in 0..3 {
                                            let mut lb = [0u8; 4];
                                            if s.read_exact(&mut lb).is_err() {
                                                break;
                                            }
                                            let len = u32::from_be_bytes(lb) as usize;
                                            let mut body = vec![0u8; len];
                                            if s.read_exact(&mut body).is_err() {
                                                break;
                                            }
                                            let lreq = plist::from_bytes(&body).unwrap().into_dictionary().unwrap();
                                            let kind = lreq.get("Request").and_then(|v| v.as_string()).unwrap_or("");
                                            let mut lresp = plist::Dictionary::new();
                                            match kind {
                                                "StartSession" => {
                                                    lresp.insert("EnableSessionSSL".into(), plist::Value::Boolean(false));
                                                }
                                                "GetValue" => {
                                                    lresp.insert("Value".into(), plist::Value::Boolean(false));
                                                }
                                                "StartService" => {
                                                    lresp.insert("Port".into(), plist::Value::from(59333u16));
                                                    lresp.insert("EnableServiceSSL".into(), plist::Value::Boolean(false));
                                                }
                                                _ => {}
                                            }
                                            write_len_plist(&mut s, lresp);
                                        }
                                    } else if port == 59333 {
                                        let areq = read_len_plist(&mut s);
                                        if areq.get("action").and_then(|v| v.as_signed_integer()) == Some(0) {
                                            rev_c.store(true, std::sync::atomic::Ordering::SeqCst);
                                        }
                                        let mut aresp = plist::Dictionary::new();
                                        aresp.insert("success".into(), plist::Value::Boolean(true));
                                        write_len_plist(&mut s, aresp);
                                    }
                                }
                                _ => {}
                            }
                        }));
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
            for h in handlers {
                let _ = h.join();
            }
        });

        std::env::set_var("USBMUXD_SOCKET_ADDRESS", mux_addr.to_string());
        let prov = block_on(provider("00008150-DEVMODEOFF")).unwrap();
        let mut dummy_adapter = idevice::tcp::adapter::Adapter::new(
            Box::new(std::io::Cursor::new(Vec::new())),
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        )
        .to_async_handle();
        let mut rsd = RsdHandshake {
            services: std::collections::HashMap::new(),
            protocol_version: 1,
            uuid: "M3-DEVOFF-UUID".into(),
        };

        let devoff_err = block_on(ensure_ddi_with_rsd(
            &mut dummy_adapter,
            &mut rsd,
            Some(&prov),
            "27.2",
            |_, _, _| {},
        ))
        .expect_err("Developer Mode off must return clear error before DDI download");
        std::env::remove_var("USBMUXD_SOCKET_ADDRESS");
        mux_stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = mux_srv.join();

        assert!(
            devoff_err.contains("Developer Mode") && devoff_err.contains("Settings -> Privacy & Security"),
            "Developer Mode error must give clear instructions: {devoff_err}"
        );
        assert!(
            amfi_revealed.load(std::sync::atomic::Ordering::SeqCst),
            "AMFI reveal_developer_mode_option_in_ui must be triggered when Developer Mode is off"
        );

        // Subtest C: Wi-Fi post-reboot without USB cable returns clear bilingual USB-1x guidance
        let wifi_err = block_on(ensure_ddi_with_rsd(
            &mut dummy_adapter,
            &mut rsd,
            None,
            "27.2",
            |_, _, _| {},
        ))
        .expect_err("Wi-Fi missing DDI without USB must return clear USB-1x guidance");
        assert!(
            wifi_err.contains("kabel USB") && wifi_err.contains("USB cable"),
            "Wi-Fi post-reboot error must be bilingual: {wifi_err}"
        );
    }
}
