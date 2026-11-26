//! Pure-std threaded AirPlay RTSP/1.0 Receiver (`GET /info`, `POST /pair-setup`,
//! `POST /pair-verify`, `POST /fp-setup`, `OPTIONS`, `SETUP`, `RECORD`, `GET_PARAMETER`,
//! `SET_PARAMETER`, `FLUSH`, `POST /feedback`, `TEARDOWN`).
//!
//! Tahap 253 Hardening for Click-#1 Stability (`iOS 16–27`):
//! - Per-stream `TEARDOWN` (tearing down audio `type=96` NEVER kills video `type=110` or closes RTSP!)
//! - Combined `SETUP` Stage 1 & Stage 2 handling (never drops `streams` when sent alongside `ekey`/`eiv`)
//! - Bi-directional UDP NTP `timingPort` (answers incoming `0xD2 -> 0xD3` AND queries peer clock every 3s)
//! - Multi-connection draining TCP `eventPort` (accepts & drains all connections without closing early)
//! - Exact Apple TV 3 `RECORD` headers (`Audio-Latency: 11025`, `Audio-Jack-Status: connected; type=analog`)
//! - Deterministic `stop: Arc<AtomicBool>` + `JoinHandle::join()` (`<= 150 ms` port release, 0 `AddrInUse`).

use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use plist::{Dictionary, Value};

use super::audio::spawn_airplay_audio_worker;
use super::fairplay::FairPlay;
use super::identity::AirPlayIdentity;
use super::info::ReceiverInfo;
use super::mdns::MdnsAdvertiser;
use super::mirror::run_mirror_listener;
use super::pair::PairVerify;
use super::rtsp::{RtspReader, RtspRequest, RtspResponse};
use super::{
    disable_udp_connreset, inc_trace_event_accept, inc_trace_event_rx, inc_trace_feedback,
    inc_trace_ntp_rx, inc_trace_ntp_tx, inc_trace_teardown, record_airplay_trace, session_key,
    set_trace_ntp_err, stream_key_iv, wait_udp_readable, AirPlaySink, AirPlayState,
};

const NTP_EPOCH_DELTA: u64 = 2_208_988_800;

/// Binds a TCP listener on `0.0.0.0:{port}` with `SO_REUSEADDR` so rapid restart never hits `AddrInUse`.
#[cfg(windows)]
fn bind_tcp_reuseaddr(port: u16) -> io::Result<TcpListener> {
    use std::os::windows::io::FromRawSocket;

    if crate::connection::is_loopback_only_test_mode() {
        return TcpListener::bind(crate::connection::bind_addr_v4(port));
    }

    let _wsa = UdpSocket::bind("127.0.0.1:0")?;

    const AF_INET: i32 = 2;
    const SOCK_STREAM: i32 = 1;
    const IPPROTO_TCP: i32 = 6;
    const SOL_SOCKET: i32 = 0xFFFF;
    const SO_REUSEADDR: i32 = 0x0004;
    const INVALID_SOCKET: usize = !0usize;

    #[repr(C)]
    struct SockAddrIn {
        sin_family: u16,
        sin_port: u16,
        sin_addr: u32,
        sin_zero: [u8; 8],
    }

    #[link(name = "ws2_32")]
    extern "system" {
        fn socket(af: i32, kind: i32, protocol: i32) -> usize;
        fn setsockopt(s: usize, level: i32, optname: i32, optval: *const u8, optlen: i32) -> i32;
        fn bind(s: usize, name: *const SockAddrIn, namelen: i32) -> i32;
        fn listen(s: usize, backlog: i32) -> i32;
        fn closesocket(s: usize) -> i32;
    }

    let raw = unsafe { socket(AF_INET, SOCK_STREAM, IPPROTO_TCP) };
    if raw == INVALID_SOCKET {
        return Err(io::Error::last_os_error());
    }
    let reuse: i32 = 1;
    unsafe {
        let _ = setsockopt(
            raw,
            SOL_SOCKET,
            SO_REUSEADDR,
            (&reuse as *const i32).cast::<u8>(),
            std::mem::size_of::<i32>() as i32,
        );
    }
    let addr = SockAddrIn {
        sin_family: AF_INET as u16,
        sin_port: port.to_be(),
        sin_addr: 0,
        sin_zero: [0; 8],
    };
    if unsafe { bind(raw, &addr, std::mem::size_of::<SockAddrIn>() as i32) } != 0
        || unsafe { listen(raw, 16) } != 0
    {
        let err = io::Error::last_os_error();
        unsafe {
            let _ = closesocket(raw);
        }
        return Err(err);
    }
    Ok(unsafe { TcpListener::from_raw_socket(raw as u64) })
}

#[cfg(not(windows))]
fn bind_tcp_reuseaddr(port: u16) -> io::Result<TcpListener> {
    TcpListener::bind(crate::connection::bind_addr_v4(port))
}

fn ntp_timestamp_bytes() -> [u8; 8] {
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = (dur.as_secs() + NTP_EPOCH_DELTA) as u32;
    let frac = (((dur.subsec_nanos() as u64) << 32) / 1_000_000_000) as u32;
    let mut out = [0u8; 8];
    out[..4].copy_from_slice(&secs.to_be_bytes());
    out[4..8].copy_from_slice(&frac.to_be_bytes());
    out
}

fn plist_u64(v: Option<&Value>) -> u64 {
    v.and_then(|val| {
        val.as_unsigned_integer()
            .or_else(|| val.as_signed_integer().map(|i| i as u64))
    })
    .unwrap_or(0)
}

fn plist_opt_u64(v: Option<&Value>) -> Option<u64> {
    v.and_then(|val| {
        val.as_unsigned_integer()
            .or_else(|| val.as_signed_integer().map(|i| i as u64))
    })
}

/// Normalizes a peer TCP address for UDP NTP replies: unwraps IPv4-mapped IPv6 (`::ffff:x.x.x.x`)
/// to plain IPv4 and rejects unspecified (`0.0.0.0` / `::`) addresses.
fn normalize_peer_ip(ip: IpAddr) -> Option<IpAddr> {
    let norm = match ip {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(v6),
        },
        v4 => v4,
    };
    if norm.is_unspecified() {
        None
    } else {
        Some(norm)
    }
}

/// Extracts strictly PII-free sorted ASCII key names from a plist dictionary (never logs values!).
fn safe_plist_keys(dict: &Dictionary) -> String {
    let mut keys: Vec<String> = dict
        .keys()
        .map(|k| {
            k.chars()
                .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
                .take(28)
                .collect::<String>()
        })
        .filter(|k| !k.is_empty())
        .collect();
    keys.sort();
    keys.truncate(16);
    keys.join(",")
}

/// Whitelists `timingProtocol` to strict non-PII protocol tokens (`NTP`, `PTP`, `none`).
fn safe_timing_protocol(dict: &Dictionary) -> &'static str {
    match dict.get("timingProtocol").and_then(|v| v.as_string()) {
        Some(s) if s.eq_ignore_ascii_case("NTP") => "NTP",
        Some(s) if s.eq_ignore_ascii_case("PTP") => "PTP",
        Some(_) => "other",
        None => "none",
    }
}

/// Bi-directional AirPlay NTP Timing UDP socket (Tahap 256/257 Hardening):
/// - Disables WinSock2 `SIO_UDP_CONNRESET` (`0x9800_000C`) so ICMP Port Unreachable NEVER poisons
///   subsequent `recv_from()` calls on Windows!
/// - Non-blocking `WSAPoll` loop (zero `SO_RCVTIMEO` IRP hazards).
/// - Sends initial `0x80 0xD2` NTP clock query immediately (`t = 0 ms`, 1:1 `zenlink-airplay`),
///   backup retry at `t = 250 ms` if no `0xD3` arrived, then every `3 s`.
/// - Immediately replies (`0x80 0xD3`, epoch 1900) to every incoming `0xD2` timing request from iOS.
fn spawn_timing_udp(
    peer_ip: Option<IpAddr>,
    their_timing_port: u16,
    stop: Arc<AtomicBool>,
) -> io::Result<(u16, JoinHandle<()>)> {
    let sock = UdpSocket::bind(crate::connection::bind_addr_v4(0))?;
    let port = sock.local_addr()?.port();
    let _ = disable_udp_connreset(&sock);
    let _ = sock.set_nonblocking(true);
    let handle = std::thread::Builder::new()
        .name("airplay-timing".into())
        .spawn(move || {
            let mut buf = [0u8; 64];
            let peer_target = peer_ip
                .and_then(normalize_peer_ip)
                .filter(|_| their_timing_port > 0)
                .map(|ip| SocketAddr::new(ip, their_timing_port));
            let mut last_query: Option<Instant> = None;
            let mut probe_count: u32 = 0;
            let mut got_d3_reply = false;
            let mut ntp_tx_logged: u32 = 0;
            let mut ntp_rx_logged: u32 = 0;

            while !stop.load(Ordering::Relaxed) {
                if let Some(target) = peer_target {
                    let should_probe = match last_query {
                        None => true, // Immediate probe #1 (1:1 zenlink-airplay/src/server.rs:483)
                        Some(last) => {
                            if probe_count == 1 && !got_d3_reply {
                                last.elapsed() >= Duration::from_millis(250)
                            } else {
                                last.elapsed() >= Duration::from_secs(3)
                            }
                        }
                    };
                    if should_probe {
                        last_query = Some(Instant::now());
                        probe_count = probe_count.saturating_add(1);
                        let mut q = [0u8; 32];
                        q[0] = 0x80;
                        q[1] = 0xD2; // Timing Request
                        q[2] = 0x00;
                        q[3] = 0x07;
                        q[24..32].copy_from_slice(&ntp_timestamp_bytes());
                        match sock.send_to(&q, target) {
                            Ok(_) => {
                                inc_trace_ntp_tx();
                                if ntp_tx_logged < 4 {
                                    ntp_tx_logged += 1;
                                    record_airplay_trace(
                                        "ntp_tx",
                                        format!("type=0xD2 probe=#{probe_count}"),
                                    );
                                }
                            }
                            Err(e) => {
                                set_trace_ntp_err(format!("{:?}", e.kind()));
                                if ntp_tx_logged < 6 {
                                    ntp_tx_logged += 1;
                                    record_airplay_trace(
                                        "ntp_tx",
                                        format!("type=0xD2 err={:?}", e.kind()),
                                    );
                                }
                            }
                        }
                    }
                }

                let mut drained_any = false;
                loop {
                    match sock.recv_from(&mut buf) {
                        Ok((n, src)) => {
                            drained_any = true;
                            if n >= 32 && (buf[1] == 0xD2 || (buf[1] & 0x7F) == 0x52) {
                                inc_trace_ntp_rx();
                                let rx_ts = ntp_timestamp_bytes();
                                if ntp_rx_logged < 4 {
                                    ntp_rx_logged += 1;
                                    record_airplay_trace(
                                        "ntp_rx",
                                        format!("type=0xD2 len={n}"),
                                    );
                                }
                                let mut reply = [0u8; 32];
                                reply[0] = 0x80;
                                reply[1] = 0xD3; // Timing Response
                                reply[2..4].copy_from_slice(&buf[2..4]);
                                reply[8..16].copy_from_slice(&buf[24..32]); // originate = client send ts
                                reply[16..24].copy_from_slice(&rx_ts); // receive ts
                                reply[24..32].copy_from_slice(&ntp_timestamp_bytes()); // transmit ts
                                match sock.send_to(&reply, src) {
                                    Ok(_) => {
                                        inc_trace_ntp_tx();
                                        if ntp_tx_logged < 4 {
                                            ntp_tx_logged += 1;
                                            record_airplay_trace("ntp_tx", "type=0xD3 reply");
                                        }
                                    }
                                    Err(e) => {
                                        set_trace_ntp_err(format!("{:?}", e.kind()));
                                        if ntp_tx_logged < 6 {
                                            ntp_tx_logged += 1;
                                            record_airplay_trace(
                                                "ntp_tx",
                                                format!("type=0xD3 err={:?}", e.kind()),
                                            );
                                        }
                                    }
                                }
                            } else if n >= 32 && (buf[1] == 0xD3 || (buf[1] & 0x7F) == 0x53) {
                                inc_trace_ntp_rx();
                                got_d3_reply = true;
                                if ntp_rx_logged < 4 {
                                    ntp_rx_logged += 1;
                                    record_airplay_trace(
                                        "ntp_rx",
                                        format!("type=0xD3 len={n}"),
                                    );
                                }
                            }
                        }
                        Err(e)
                            if e.kind() == io::ErrorKind::WouldBlock
                                || e.kind() == io::ErrorKind::Interrupted
                                || e.kind() == io::ErrorKind::ConnectionReset =>
                        {
                            break;
                        }
                        Err(_) => break,
                    }
                }
                if !drained_any {
                    let _ = wait_udp_readable(&sock, 20);
                }
            }
        })?;
    Ok((port, handle))
}

fn try_process_event_buffer(stream: &mut TcpStream, buf: &mut Vec<u8>) {
    loop {
        while buf.starts_with(b"\r\n") {
            buf.drain(..2);
        }
        let Some(hdr_end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
            if buf.len() > 64 * 1024 {
                buf.clear();
            }
            break;
        };
        let hdr_str = String::from_utf8_lossy(&buf[..hdr_end]).into_owned();
        let mut lines = hdr_str.split("\r\n");
        let first = lines.next().unwrap_or("");
        let is_rtsp = first.contains("RTSP/");
        let is_http = first.contains("HTTP/");
        let mut clen: usize = 0;
        let mut cseq: Option<String> = None;
        for line in lines {
            if let Some((k, v)) = line.split_once(':') {
                let kl = k.trim().to_ascii_lowercase();
                if kl == "content-length" {
                    clen = v.trim().parse::<usize>().unwrap_or(0);
                } else if kl == "cseq" {
                    cseq = Some(v.trim().to_string());
                }
            }
        }
        let total = hdr_end + 4 + clen;
        if buf.len() < total {
            break;
        }
        let method = first.split_whitespace().next().unwrap_or("RAW");
        buf.drain(..total);
        if is_rtsp {
            record_airplay_trace("event_rx", format!("proto=RTSP method={method} body_len={clen}"));
            let seq = cseq.as_deref().unwrap_or("0");
            let reply = format!(
                "RTSP/1.0 200 OK\r\nCSeq: {seq}\r\nServer: AirTunes/220.68\r\nContent-Length: 0\r\n\r\n"
            );
            let _ = stream.write_all(reply.as_bytes());
            let _ = stream.flush();
        } else if is_http {
            record_airplay_trace("event_rx", format!("proto=HTTP method={method} body_len={clen}"));
            let reply = "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
            let _ = stream.write_all(reply.as_bytes());
            let _ = stream.flush();
        }
    }
}

/// Multi-connection AirPlay `eventPort` TCP server (Tahap 256 Hardening):
/// Accepts every incoming connection immediately (`<= 10 ms`), logs `event_accept` / `event_eof`,
/// replies `200 OK` to any framed RTSP/HTTP request arriving on the channel while keeping every
/// connection alive (NEVER closing on `WouldBlock` or audio `TEARDOWN type=96`).
fn spawn_event_tcp(stop: Arc<AtomicBool>) -> io::Result<(u16, JoinHandle<()>)> {
    let listener = TcpListener::bind(crate::connection::bind_addr_v4(0))?;
    let port = listener.local_addr()?.port();
    let _ = listener.set_nonblocking(true);
    let handle = std::thread::Builder::new()
        .name("airplay-event".into())
        .spawn(move || {
            let mut conns: Vec<(u64, TcpStream, Vec<u8>)> = Vec::new();
            let mut next_conn_id: u64 = 0;
            let mut drain_buf = [0u8; 2048];
            while !stop.load(Ordering::Relaxed) {
                while let Ok((s, _)) = listener.accept() {
                    let _ = s.set_nodelay(true);
                    let _ = s.set_nonblocking(true);
                    next_conn_id = next_conn_id.saturating_add(1);
                    inc_trace_event_accept();
                    record_airplay_trace("event_accept", format!("conn=#{next_conn_id}"));
                    conns.push((next_conn_id, s, Vec::with_capacity(512)));
                    if conns.len() > 8 {
                        conns.remove(0);
                    }
                }
                conns.retain_mut(|(cid, c, rbuf)| loop {
                    match c.read(&mut drain_buf) {
                        Ok(0) => {
                            record_airplay_trace("event_eof", format!("conn=#{cid}"));
                            return false;
                        }
                        Ok(n) => {
                            inc_trace_event_rx();
                            rbuf.extend_from_slice(&drain_buf[..n]);
                            try_process_event_buffer(c, rbuf);
                        }
                        Err(e)
                            if e.kind() == io::ErrorKind::WouldBlock
                                || e.kind() == io::ErrorKind::Interrupted =>
                        {
                            return true;
                        }
                        Err(_) => {
                            record_airplay_trace("event_eof", format!("conn=#{cid}"));
                            return false;
                        }
                    }
                });
                std::thread::sleep(Duration::from_millis(10));
            }
            conns.clear();
        })?;
    Ok((port, handle))
}

struct ManagedWorker {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl ManagedWorker {
    fn new(stop: Arc<AtomicBool>, handle: JoinHandle<()>) -> Self {
        Self {
            stop,
            handle: Some(handle),
        }
    }

    fn stop_and_join(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for ManagedWorker {
    fn drop(&mut self) {
        self.stop_and_join();
    }
}

struct RtspSession {
    peer_ip: Option<IpAddr>,
    session_id: u64,
    active_session_id: Arc<AtomicU64>,
    pair: PairVerify,
    fairplay: FairPlay,
    aeskey: Option<[u8; 16]>,
    eiv: Option<[u8; 16]>,
    had_mirror_stream: bool,
    audio_flush_epoch: Arc<AtomicU64>,
    mirror_worker: Option<ManagedWorker>,
    audio_worker: Option<ManagedWorker>,
    timing_worker: Option<ManagedWorker>,
    event_worker: Option<ManagedWorker>,
}

impl RtspSession {
    fn new(peer_ip: Option<IpAddr>, active_session_id: Arc<AtomicU64>) -> Self {
        Self {
            peer_ip,
            session_id: 0,
            active_session_id,
            pair: PairVerify::default(),
            fairplay: FairPlay::default(),
            aeskey: None,
            eiv: None,
            had_mirror_stream: false,
            audio_flush_epoch: Arc::new(AtomicU64::new(0)),
            mirror_worker: None,
            audio_worker: None,
            timing_worker: None,
            event_worker: None,
        }
    }

    fn claim_active_owner(&mut self) {
        let next = self
            .active_session_id
            .fetch_add(1, Ordering::SeqCst)
            .saturating_add(1);
        self.session_id = next;
    }

    fn is_active_owner(&self) -> bool {
        self.session_id != 0 && self.active_session_id.load(Ordering::SeqCst) == self.session_id
    }

    fn stop_mirror(&mut self) {
        if let Some(mut w) = self.mirror_worker.take() {
            w.stop_and_join();
        }
    }

    fn stop_audio(&mut self) {
        if let Some(mut w) = self.audio_worker.take() {
            w.stop_and_join();
        }
    }

    fn stop_all(&mut self) {
        self.stop_mirror();
        self.stop_audio();
        if let Some(mut w) = self.timing_worker.take() {
            w.stop_and_join();
        }
        if let Some(mut w) = self.event_worker.take() {
            w.stop_and_join();
        }
    }

    fn has_active_media(&self) -> bool {
        self.mirror_worker.is_some() || self.audio_worker.is_some()
    }
}

impl Drop for RtspSession {
    fn drop(&mut self) {
        self.stop_all();
    }
}

fn handle_client(
    mut stream: TcpStream,
    id: AirPlayIdentity,
    name: String,
    sink: Arc<dyn AirPlaySink>,
    server_stop: Arc<AtomicBool>,
    active_session_id: Arc<AtomicU64>,
) {
    let _ = stream.set_nodelay(true);
    let peer_ip = stream.peer_addr().ok().and_then(|a| normalize_peer_ip(a.ip()));
    let mut reader = RtspReader::new();
    let mut sess = RtspSession::new(peer_ip, active_session_id);

    let mut feedback_resp_logged: u32 = 0;

    while !server_stop.load(Ordering::Relaxed) {
        let req = match reader.read_request(&mut stream, &server_stop) {
            Ok(Some(r)) => r,
            _ => break,
        };

        let cseq = req.cseq().map(str::to_string);
        let is_feedback = req.method == "POST" && req.path.ends_with("/feedback");
        if !is_feedback {
            let safe_path = if req.path.starts_with("rtsp://") {
                "rtsp://session"
            } else {
                req.path.as_str()
            };
            record_airplay_trace(
                "rtsp_req",
                format!("{} {} len={}", req.method, safe_path, req.body.len()),
            );
        }
        let (resp, should_close) = dispatch_rtsp(&req, &id, &name, &sink, &mut sess);
        let should_log_resp = if is_feedback {
            if resp.status_code() != 200 || feedback_resp_logged < 3 {
                feedback_resp_logged += 1;
                true
            } else {
                false
            }
        } else {
            matches!(
                req.method.as_str(),
                "SETUP" | "RECORD" | "TEARDOWN" | "GET"
            ) || resp.status_code() != 200
        };
        if should_log_resp {
            let label = if is_feedback {
                "POST /feedback"
            } else if req.method == "GET" && req.path.ends_with("/info") {
                "GET /info"
            } else {
                req.method.as_str()
            };
            record_airplay_trace(
                "rtsp_resp",
                format!(
                    "{label} status={} body_len={}",
                    resp.status_code(),
                    resp.body_len()
                ),
            );
        }
        if resp.write_to(&mut stream, cseq.as_deref()).is_err() {
            break;
        }
        if should_close {
            break;
        }
    }

    let owned_mirror = sess.had_mirror_stream && sess.is_active_owner();
    sess.stop_all();
    if owned_mirror && !server_stop.load(Ordering::Relaxed) {
        sink.on_state(AirPlayState::Waiting);
    }
}

fn dispatch_rtsp(
    req: &RtspRequest,
    id: &AirPlayIdentity,
    name: &str,
    sink: &Arc<dyn AirPlaySink>,
    sess: &mut RtspSession,
) -> (RtspResponse, bool) {
    let info = ReceiverInfo {
        id,
        name,
        hevc: false, // Explicitly H.264 (avc1) for universal iOS 16–27 & non-HEVC Windows compatibility
    };

    match (req.method.as_str(), req.path.as_str()) {
        ("GET", p) if p == "/info" || p.ends_with("/info") => {
            let bp = info.build_info_bplist(req.content_type(), &req.body);
            (RtspResponse::ok().bplist(bp), false)
        }
        ("POST", p) if p == "/pair-setup" || p.ends_with("/pair-setup") => {
            (RtspResponse::ok().octet_stream(id.public_key.to_vec()), false)
        }
        ("POST", p) if p == "/pair-verify" || p.ends_with("/pair-verify") => {
            let reply = sess.pair.handle(id, &req.body);
            (RtspResponse::ok().octet_stream(reply), false)
        }
        ("POST", p) if p == "/fp-setup" || p.ends_with("/fp-setup") => {
            match sess.fairplay.handle(&req.body) {
                Some(reply) => (RtspResponse::ok().octet_stream(reply), false),
                None => (RtspResponse::status(400, "Bad Request"), false),
            }
        }
        ("OPTIONS", _) => (
            RtspResponse::ok().header(
                "Public",
                "ANNOUNCE, SETUP, RECORD, PAUSE, FLUSH, TEARDOWN, OPTIONS, GET_PARAMETER, SET_PARAMETER, POST, GET",
            ),
            false,
        ),
        ("SETUP", _) => (handle_setup(&req.body, sink, sess), false),
        ("RECORD", _) => (
            RtspResponse::ok()
                .header("Audio-Latency", "11025")
                .header("Audio-Jack-Status", "connected; type=analog"),
            false,
        ),
        ("GET_PARAMETER", _) => {
            let body_text = String::from_utf8_lossy(&req.body);
            if body_text.contains("volume") {
                (
                    RtspResponse::ok()
                        .header("Content-Type", "text/parameters")
                        .body(b"volume: 0.000000\r\n".to_vec()),
                    false,
                )
            } else {
                (RtspResponse::ok(), false)
            }
        }
        ("SET_PARAMETER", _) => (RtspResponse::ok(), false),
        ("FLUSH", _) => {
            sess.audio_flush_epoch.fetch_add(1, Ordering::SeqCst);
            record_airplay_trace("audio_flush", "rtsp_flush");
            (RtspResponse::ok(), false)
        }
        ("POST", p)
            if p == "/command"
                || p.ends_with("/command")
                || p == "/audioMode"
                || p.ends_with("/audioMode")
                || p == "/info"
                || p.ends_with("/info") =>
        {
            (RtspResponse::ok(), false)
        }
        ("POST", p) if p == "/feedback" || p.ends_with("/feedback") => {
            inc_trace_feedback();
            (RtspResponse::ok(), false)
        }
        ("TEARDOWN", _) => {
            // Tahap 253/256/257 Pilar 1: Per-stream TEARDOWN (1:1 zenlink-airplay/src/server.rs:289-315).
            // iOS frequently sends TEARDOWN for audio (type=96) during first-click startup while
            // keeping mirror video (type=110), eventPort, timingPort, and RTSP alive!
            inc_trace_teardown();
            let parsed_dict = plist::from_bytes(&req.body)
                .ok()
                .and_then(|v| v.as_dictionary().cloned());
            let td_keys = parsed_dict
                .as_ref()
                .map(safe_plist_keys)
                .unwrap_or_default();
            let types: Vec<u64> = parsed_dict
                .as_ref()
                .and_then(|d| {
                    d.get("streams")?.as_array().map(|arr| {
                        arr.iter()
                            .filter_map(|item| Some(plist_u64(item.as_dictionary()?.get("type"))))
                            .collect()
                    })
                })
                .unwrap_or_default();

            let is_full = types.is_empty();
            if is_full || types.contains(&110) {
                sess.stop_mirror();
            }
            if is_full || types.contains(&96) {
                sess.stop_audio();
            }
            let active_media = sess.has_active_media();
            let types_str = types
                .iter()
                .map(|t| t.to_string())
                .collect::<Vec<_>>()
                .join(",");
            record_airplay_trace(
                "teardown_parsed",
                format!(
                    "len={} types=[{types_str}] keys=[{td_keys}] full={is_full} active_media={active_media}",
                    req.body.len()
                ),
            );
            let should_close = is_full || !active_media;
            if should_close && sess.had_mirror_stream && sess.is_active_owner() {
                sink.on_state(AirPlayState::Waiting);
            }
            (RtspResponse::ok(), should_close)
        }
        _ => {
            let safe_path = if req.path.starts_with("rtsp://") {
                "rtsp://session"
            } else {
                req.path.as_str()
            };
            record_airplay_trace(
                "rtsp_method_unhandled",
                format!("{} {} len={}", req.method, safe_path, req.body.len()),
            );
            (RtspResponse::ok(), false)
        }
    }
}

fn handle_setup(
    body: &[u8],
    sink: &Arc<dyn AirPlaySink>,
    sess: &mut RtspSession,
) -> RtspResponse {
    let val = match plist::from_bytes(body) {
        Ok(v) => v,
        Err(_) => {
            record_airplay_trace("setup_parse_err", format!("bplist_len={}", body.len()));
            return RtspResponse::status(400, "Bad Request");
        }
    };
    let Some(dict) = val.as_dictionary() else {
        record_airplay_trace("setup_parse_err", "not_dict");
        return RtspResponse::status(400, "Bad Request");
    };

    let top_keys = safe_plist_keys(dict);
    let mut resp = Dictionary::new();

    // Stage 1 (or combined Stage 1+2): FairPlay `ekey` + `eiv` + timingPort + eventPort
    let ekey_opt = dict.get("ekey").and_then(|v| v.as_data()).or_else(|| {
        dict.get("streams")
            .and_then(|v| v.as_array())
            .and_then(|arr| {
                arr.iter()
                    .find_map(|item| item.as_dictionary()?.get("ekey")?.as_data())
            })
    });
    if let Some(ekey) = ekey_opt {
        let Ok(fp_key) = sess.fairplay.decrypt(ekey) else {
            return RtspResponse::status(500, "FairPlay Decrypt Failed");
        };
        let has_eiv = if let Some(eiv) = dict.get("eiv").and_then(|v| v.as_data()) {
            let Ok(eiv_arr) = <[u8; 16]>::try_from(eiv) else {
                return RtspResponse::status(400, "Invalid eiv");
            };
            sess.eiv = Some(eiv_arr);
            true
        } else {
            false
        };
        let aes = session_key(&fp_key, sess.pair.shared_secret.as_ref());
        sess.aeskey = Some(aes);
        sess.claim_active_owner();

        // Replace any stale timing/event workers from a previous Stage-1 SETUP cleanly
        if let Some(mut old_t) = sess.timing_worker.take() {
            old_t.stop_and_join();
        }
        if let Some(mut old_e) = sess.event_worker.take() {
            old_e.stop_and_join();
        }

        let their_timing_port = plist_u64(dict.get("timingPort")) as u16;
        let timing_proto = safe_timing_protocol(dict);
        let t_stop = Arc::new(AtomicBool::new(false));
        let timing_port = match spawn_timing_udp(sess.peer_ip, their_timing_port, t_stop.clone()) {
            Ok((p, h)) => {
                sess.timing_worker = Some(ManagedWorker::new(t_stop, h));
                p
            }
            Err(_) => 0,
        };
        let e_stop = Arc::new(AtomicBool::new(false));
        let event_port = match spawn_event_tcp(e_stop.clone()) {
            Ok((p, h)) => {
                sess.event_worker = Some(ManagedWorker::new(e_stop, h));
                p
            }
            Err(_) => 0,
        };

        resp.insert("timingPort".into(), Value::from(timing_port as u64));
        resp.insert("eventPort".into(), Value::from(event_port as u64));
        record_airplay_trace(
            "setup_s1",
            format!("timing={timing_port} their_timing={their_timing_port} event={event_port}"),
        );
        record_airplay_trace(
            "setup_s1_parsed",
            format!(
                "proto={timing_proto} theirTiming={their_timing_port} hasEkey=true hasEiv={has_eiv} ourTiming={timing_port} ourEvent={event_port} keys=[{top_keys}]"
            ),
        );
        sink.on_state(AirPlayState::Connected);
    }

    // Stage 2 (or combined): `streams` array (type=110 mirror video, type=96 audio)
    if let Some(streams) = dict.get("streams").and_then(|v| v.as_array()) {
        let mut resp_streams = Vec::new();
        let mut parsed_types: Vec<String> = Vec::new();
        let mut parsed_ct: Option<u64> = None;
        let mut parsed_sr: Option<u64> = None;
        let mut parsed_their_ctrl: Option<u64> = None;
        let mut parsed_our_data: u16 = 0;
        let mut parsed_our_ctrl: Option<u16> = None;
        let mut stream_keys_summary = String::new();

        for item in streams {
            let Some(sd) = item.as_dictionary() else {
                continue;
            };
            let stype = plist_u64(sd.get("type"));
            parsed_types.push(stype.to_string());
            if stream_keys_summary.is_empty() {
                stream_keys_summary = safe_plist_keys(sd);
            }
            if parsed_ct.is_none() {
                parsed_ct = plist_opt_u64(sd.get("ct"));
            }
            if parsed_sr.is_none() {
                parsed_sr = plist_opt_u64(sd.get("sr"));
            }
            if parsed_their_ctrl.is_none() {
                parsed_their_ctrl = plist_opt_u64(sd.get("controlPort"));
            }

            if stype == 110 {
                let conn_id = plist_u64(sd.get("streamConnectionID"));
                let Some(aeskey) = sess.aeskey else {
                    return RtspResponse::status(400, "Missing session key");
                };
                let (skey, siv) = stream_key_iv(&aeskey, conn_id);
                let Ok(listener) = TcpListener::bind(crate::connection::bind_addr_v4(0)) else {
                    return RtspResponse::status(500, "Mirror bind failed");
                };
                let Ok(local_addr) = listener.local_addr() else {
                    return RtspResponse::status(500, "Mirror addr failed");
                };
                let data_port = local_addr.port();

                // Cleanly replace any prior mirror listener on this RTSP session
                sess.stop_mirror();
                sess.had_mirror_stream = true;
                if sess.session_id == 0 {
                    sess.claim_active_owner();
                }

                let sink_clone = sink.clone();
                let mirror_stop = Arc::new(AtomicBool::new(false));
                let m_stop_bg = mirror_stop.clone();
                if let Ok(h) = std::thread::Builder::new()
                    .name("airplay-mirror-rx".into())
                    .spawn(move || {
                        run_mirror_listener(listener, skey, siv, sink_clone, m_stop_bg);
                    })
                {
                    sess.mirror_worker = Some(ManagedWorker::new(mirror_stop, h));
                }

                parsed_our_data = data_port;
                let mut out = Dictionary::new();
                out.insert("type".into(), Value::from(110u64));
                out.insert("dataPort".into(), Value::from(data_port as u64));
                record_airplay_trace("setup_s2", format!("type=110 dataPort={data_port}"));
                resp_streams.push(Value::Dictionary(out));
            } else if stype == 96 {
                sess.stop_audio();
                let audio_stop = Arc::new(AtomicBool::new(false));
                let ct = plist_opt_u64(sd.get("ct")).unwrap_or(8);
                let sr = plist_opt_u64(sd.get("sr")).unwrap_or(44_100) as u32;
                let spf = plist_opt_u64(sd.get("spf")).unwrap_or(480) as usize;
                let audio_key = sess.aeskey.unwrap_or([0u8; 16]);
                let audio_iv = sess.eiv.unwrap_or([0u8; 16]);
                if let Ok((dp, cp, h)) = spawn_airplay_audio_worker(
                    audio_key,
                    audio_iv,
                    ct,
                    sr,
                    spf,
                    sess.audio_flush_epoch.clone(),
                    audio_stop.clone(),
                ) {
                    sess.audio_worker = Some(ManagedWorker::new(audio_stop, h));
                    parsed_our_data = dp;
                    parsed_our_ctrl = Some(cp);
                    let mut out = Dictionary::new();
                    out.insert("type".into(), Value::from(96u64));
                    out.insert("dataPort".into(), Value::from(dp as u64));
                    out.insert("controlPort".into(), Value::from(cp as u64));
                    record_airplay_trace(
                        "setup_s2_audio",
                        format!("type=96 ct={ct} sr={sr} dataPort={dp} controlPort={cp}"),
                    );
                    resp_streams.push(Value::Dictionary(out));
                }
            }
        }
        record_airplay_trace(
            "setup_s2_parsed",
            format!(
                "types=[{}] ct={} sr={} theirCtrl={} ourData={} ourCtrl={} keys=[{}]",
                parsed_types.join(","),
                parsed_ct.map(|v| v.to_string()).unwrap_or_else(|| "none".into()),
                parsed_sr.map(|v| v.to_string()).unwrap_or_else(|| "none".into()),
                parsed_their_ctrl.map(|v| v.to_string()).unwrap_or_else(|| "none".into()),
                parsed_our_data,
                parsed_our_ctrl.map(|v| v.to_string()).unwrap_or_else(|| "none".into()),
                stream_keys_summary,
            ),
        );
        if !resp_streams.is_empty() {
            resp.insert("streams".into(), Value::Array(resp_streams));
        }
    }

    let bp = plist::to_bytes_bplist00(&Value::Dictionary(resp)).unwrap_or_default();
    RtspResponse::ok().bplist(bp)
}

/// Running native AirPlay receiver instance (`RTSP TCP` + optional `mDNS` advertiser).
pub struct AirPlayServer {
    port: u16,
    name: String,
    stop: Arc<AtomicBool>,
    mdns: Option<MdnsAdvertiser>,
    accept_thread: Option<JoinHandle<()>>,
    client_threads: Arc<Mutex<Vec<JoinHandle<()>>>>,
}

impl AirPlayServer {
    /// Starts the AirPlay receiver on `port` (`0` picks an OS ephemeral port; `7100` is the standard
    /// AirPlay port). When `advertise_mdns == true`, announces `_airplay._tcp` & `_raop._tcp` on LAN.
    pub fn start(
        name: impl Into<String>,
        port: u16,
        id: AirPlayIdentity,
        sink: Arc<dyn AirPlaySink>,
        advertise_mdns: bool,
    ) -> Result<Self, String> {
        let name = name.into();
        let listener = bind_tcp_reuseaddr(port)
            .map_err(|e| format!("AirPlay RTSP bind port {port} gagal: {e}"))?;
        let actual_port = listener
            .local_addr()
            .map_err(|e| format!("AirPlay local_addr gagal: {e}"))?
            .port();
        let _ = listener.set_nonblocking(true);

        let mdns = if advertise_mdns {
            let info = ReceiverInfo {
                id: &id,
                name: &name,
                hevc: false,
            };
            MdnsAdvertiser::start(&info, actual_port).ok()
        } else {
            None
        };

        sink.on_state(AirPlayState::Waiting);

        let stop = Arc::new(AtomicBool::new(false));
        let stop_bg = stop.clone();
        let active_session_id = Arc::new(AtomicU64::new(0));
        let client_threads: Arc<Mutex<Vec<JoinHandle<()>>>> = Arc::new(Mutex::new(Vec::new()));
        let clients_bg = client_threads.clone();
        let name_bg = name.clone();

        let accept_thread = std::thread::Builder::new()
            .name("airplay-rtsp-accept".into())
            .spawn(move || {
                while !stop_bg.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let id_c = id.clone();
                            let name_c = name_bg.clone();
                            let sink_c = sink.clone();
                            let stop_c = stop_bg.clone();
                            let owner_c = active_session_id.clone();
                            if let Ok(h) = std::thread::Builder::new()
                                .name("airplay-rtsp-client".into())
                                .spawn(move || {
                                    handle_client(stream, id_c, name_c, sink_c, stop_c, owner_c);
                                })
                            {
                                if let Ok(mut list) = clients_bg.lock() {
                                    list.retain(|w| !w.is_finished());
                                    list.push(h);
                                }
                            }
                        }
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(25));
                        }
                        Err(_) => break,
                    }
                }
                drop(listener);
            })
            .map_err(|e| format!("AirPlay accept thread spawn failed: {e}"))?;

        Ok(Self {
            port: actual_port,
            name,
            stop,
            mdns,
            accept_thread: Some(accept_thread),
            client_threads,
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        record_airplay_trace("session_stop", "receiver_stop");
        if let Some(mut mdns) = self.mdns.take() {
            mdns.stop();
        }
        if let Some(h) = self.accept_thread.take() {
            let _ = h.join();
        }
        if let Ok(mut list) = self.client_threads.lock() {
            for h in list.drain(..) {
                let _ = h.join();
            }
        }
    }
}

impl Drop for AirPlayServer {
    fn drop(&mut self) {
        self.stop();
    }
}
