//! Pure-std RTSP/1.0 request framer and response writer for AirPlay Mirroring.
//!
//! Uses short socket read timeouts (`50 ms`) polling `stop: &AtomicBool` so shutdown tears down
//! every active RTSP connection cleanly in `<= 60 ms` without leaked sockets.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::wait_tcp_readable;

const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;
const SERVER_HEADER: &str = "AirTunes/220.68";

#[derive(Debug, Clone)]
pub struct RtspRequest {
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

impl RtspRequest {
    pub fn header(&self, key: &str) -> Option<&str> {
        self.headers.get(&key.to_ascii_lowercase()).map(String::as_str)
    }

    pub fn cseq(&self) -> Option<&str> {
        self.header("cseq")
    }

    pub fn content_type(&self) -> Option<&str> {
        self.header("content-type")
    }
}

/// Incremental buffered RTSP/1.0 reader over a `TcpStream` that respects `stop: &AtomicBool`.
pub struct RtspReader {
    buf: Vec<u8>,
}

impl RtspReader {
    pub fn new() -> Self {
        Self {
            buf: Vec::with_capacity(4096),
        }
    }

    fn fill_until(
        &mut self,
        stream: &mut TcpStream,
        stop: &AtomicBool,
        mut done: impl FnMut(&[u8]) -> Option<usize>,
    ) -> io::Result<Option<usize>> {
        let mut tmp = [0u8; 4096];
        loop {
            if let Some(pos) = done(&self.buf) {
                return Ok(Some(pos));
            }
            if stop.load(Ordering::Relaxed) {
                return Ok(None);
            }
            match stream.read(&mut tmp) {
                Ok(0) => return Ok(None),
                Ok(n) => self.buf.extend_from_slice(&tmp[..n]),
                Err(e)
                    if e.kind() == io::ErrorKind::WouldBlock
                        || e.kind() == io::ErrorKind::TimedOut
                        || e.kind() == io::ErrorKind::Interrupted =>
                {
                    let _ = wait_tcp_readable(stream, 50);
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
    }

    pub fn read_request(
        &mut self,
        stream: &mut TcpStream,
        stop: &AtomicBool,
    ) -> io::Result<Option<RtspRequest>> {
        let _ = stream.set_nonblocking(true);

        // Tahap 256 Pilar 1 (1:1 zenlink-airplay/src/rtsp.rs:33-35):
        // Strip stray leading blank lines (\r\n) between pipelined RTSP requests instead of closing!
        let header_end = loop {
            while self.buf.starts_with(b"\r\n") {
                self.buf.drain(..2);
            }
            let Some(pos) = self.fill_until(stream, stop, |b| {
                if b.len() > MAX_HEADER_BYTES {
                    return Some(usize::MAX);
                }
                if b.starts_with(b"\r\n") {
                    return Some(0);
                }
                b.windows(4).position(|w| w == b"\r\n\r\n")
            })? else {
                return Ok(None);
            };
            if pos == 0 && self.buf.starts_with(b"\r\n") {
                self.buf.drain(..2);
                continue;
            }
            break pos;
        };
        if header_end == usize::MAX {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "RTSP headers too large",
            ));
        }

        let header_bytes: Vec<u8> = self.buf.drain(..header_end + 4).collect();
        let header_str = String::from_utf8_lossy(&header_bytes[..header_end]);
        let mut lines = header_str.split("\r\n");
        let first = lines.next().unwrap_or("").trim();
        if first.is_empty() {
            return Ok(None);
        }
        let mut parts = first.split_whitespace();
        let method = parts.next().unwrap_or("").to_string();
        let path = parts.next().unwrap_or("").to_string();

        let mut headers = HashMap::new();
        for line in lines {
            if let Some((k, v)) = line.split_once(':') {
                headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
            }
        }

        let content_len = headers
            .get("content-length")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(0);
        if content_len > MAX_BODY_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "RTSP body too large",
            ));
        }

        let body = if content_len > 0 {
            let Some(_) = self.fill_until(stream, stop, |b| {
                if b.len() >= content_len {
                    Some(content_len)
                } else {
                    None
                }
            })? else {
                return Ok(None);
            };
            self.buf.drain(..content_len).collect()
        } else {
            Vec::new()
        };

        Ok(Some(RtspRequest {
            method,
            path,
            headers,
            body,
        }))
    }
}

pub struct RtspResponse {
    status_code: u16,
    reason: &'static str,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl RtspResponse {
    pub fn ok() -> Self {
        Self::status(200, "OK")
    }

    pub fn status(status_code: u16, reason: &'static str) -> Self {
        Self {
            status_code,
            reason,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    pub fn header(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.headers.push((k.into(), v.into()));
        self
    }

    pub fn bplist(self, body: Vec<u8>) -> Self {
        self.header("Content-Type", "application/x-apple-binary-plist")
            .body(body)
    }

    pub fn octet_stream(self, body: Vec<u8>) -> Self {
        self.header("Content-Type", "application/octet-stream")
            .body(body)
    }

    pub fn body(mut self, body: Vec<u8>) -> Self {
        self.body = body;
        self
    }

    pub fn status_code(&self) -> u16 {
        self.status_code
    }

    pub fn body_len(&self) -> usize {
        self.body.len()
    }

    pub fn encode(&self, cseq: Option<&str>) -> Vec<u8> {
        // Tahap 256 Pilar 1 (1:1 zenlink-airplay/src/rtsp.rs:66-73):
        // Status line -> CSeq -> Server: AirTunes/220.68 -> extra headers -> Content-Length.
        let cseq_val = cseq.unwrap_or("0");
        let mut head = format!(
            "RTSP/1.0 {} {}\r\nCSeq: {cseq_val}\r\nServer: {SERVER_HEADER}\r\n",
            self.status_code, self.reason
        );
        for (k, v) in &self.headers {
            head.push_str(&format!("{k}: {v}\r\n"));
        }
        head.push_str(&format!("Content-Length: {}\r\n\r\n", self.body.len()));

        let mut out = Vec::with_capacity(head.len() + self.body.len());
        out.extend_from_slice(head.as_bytes());
        out.extend_from_slice(&self.body);
        out
    }

    pub fn write_to(&self, stream: &mut TcpStream, cseq: Option<&str>) -> io::Result<()> {
        let wire = self.encode(cseq);
        let mut written = 0usize;
        let deadline = Instant::now() + Duration::from_secs(3);
        while written < wire.len() {
            match stream.write(&wire[written..]) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "RTSP write returned 0",
                    ));
                }
                Ok(n) => written += n,
                Err(e)
                    if e.kind() == io::ErrorKind::WouldBlock
                        || e.kind() == io::ErrorKind::Interrupted =>
                {
                    if Instant::now() >= deadline {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "RTSP non-blocking write timed out",
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(e) => return Err(e),
            }
        }
        let _ = stream.flush();
        Ok(())
    }
}
