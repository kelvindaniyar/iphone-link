"""AirPlay Screen Mirroring & Audio Receiver (Wi-Fi) for iPhone Link v3.0.

Implements native Apple AirPlay protocol (UxPlay / shairplay architecture):
  - mDNS advertisement: Zeroconf (_airplay._tcp + _raop._tcp) + direct unicast
  - RTSP server on port 7100: /info, /pair-setup, /pair-verify, /fp-setup, SETUP, RECORD, TEARDOWN
  - FairPlay fp-setup stages 1 & 2 + AES session key decryption via native playfair.dll
  - TCP Video stream: 128-byte headers, stateful AES-128-CTR decryption, AVCC -> Annex-B framing @ 60 FPS
  - UDP Audio stream: RTP AAC-ELD 44.1 kHz stereo, AES-128-CBC decryption, zlaac.dll decoding to 48 kHz PCM
  - Local HTTP Server on 127.0.0.1:8766 compatible with mirror.js and UsbCoreDeviceAudioEngine
  - Anti-Errno 10048 singleton receiver architecture + orphan port killer
"""

from __future__ import annotations

import asyncio
import base64
import contextlib
import ctypes
import hashlib
import json
import logging
import os
import plistlib
import secrets
import socket
import struct
import subprocess
import sys
import threading
import time
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional, Set, Tuple

from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey, Ed25519PublicKey
from cryptography.hazmat.primitives.asymmetric.x25519 import X25519PrivateKey, X25519PublicKey
from Crypto.Cipher import AES

from handset import get_app_root
from handset.aac import AacDecoder

logger = logging.getLogger(__name__)

AIRPLAY_PORT = 7100
AIRPLAY_HTTP_PORT = 8766
MODEL = "AppleTV3,2"
SOURCE_VERSION = "220.68"
PI = "2e388006-13ba-4041-9a67-25dd4a43d536"
AAC_ELD_44K = bytes.fromhex("f8e85000")

# UxPlay / ShairPlay fixed responses for fp-setup stage 1
REPLY_MESSAGES = (
    bytes.fromhex(
        "46504c59030102000000008202000f9f3f9e0a2521dbdf312ab2bfb29e8d232b6376a8c818701d22ae93d82737feaf9db4fdf41c2dba9d1f49caaabf6591ac1f7bc6f7e0663d21afe01565953eab81f418ceed095adb7c3d0e254909a79831d49c3982973434facb42c63a1cd911a6fe941a8a6d4a743b46c3a7649e44c78955e49d8155009549c4e2f7a3f6d5ba"
    ),
    bytes.fromhex(
        "46504c5903010200000000820201cf32a25714b2524f8aa0ad7af164e37bcf4424e200047efc0ad67afcd95ded1c2730bb591b962ed63a9c4ded88ba8fc78de64d91ccfd5c7b56da88e31f5cceafc7431995a01665a54e1939d25b94db64b9e45d8d063e1e6af07e9656162b0efa404275ea5a44d9591c7256b9fbe6513898b80227721988571650942ad946688a"
    ),
    bytes.fromhex(
        "46504c5903010200000000820202c169a352eeed35b18cdd9c58d64f16c1519a89eb5317bd0d4336cd68f638ff9d016a5b52b7fa9216b2b65482c78444118121a2c7fed83db7119e9182aad7d18c7063e2a457555910af9e0efc76347d164043807f581ee4fbe42ca9dedc1b5eb2a3aa3d2ecd59e7eee70b3629f22afd161d877353ddb99adc8e07006e56f850ce"
    ),
    bytes.fromhex(
        "46504c59030102000000008202039001e1727e0f57f9f5880db104a6257a23f5cfff1abbe1e93045251afb97eb9fc0011ebe0f3a81df5b691d76acb2f7a5c708e3d328f56bb39dbde5f29c8a17f481487e3ae863c678325422e6f78e166d18aa7fd636258bce28726f661f738893ce44311e4be6c0535193e5ef72e8686233729c227d820c999445d89246c8c359"
    ),
)
FP_HEADER = bytes.fromhex("46504c590301040000000014")

_PLAYFAIR_DLL: Optional[ctypes.CDLL] = None


def release_port_if_held_by_other_process(port: int) -> bool:
    """Tahap 131: Kill any orphan process holding `port` if its PID is not the current process."""
    my_pid = os.getpid()
    freed = False
    try:
        startupinfo = None
        creationflags = 0
        if sys.platform == "win32":
            startupinfo = subprocess.STARTUPINFO()
            startupinfo.dwFlags |= subprocess.STARTF_USESHOWWINDOW
            startupinfo.wShowWindow = 0
            creationflags = getattr(subprocess, "CREATE_NO_WINDOW", 0x08000000)

        cmd = ["netstat", "-ano", "-p", "tcp"]
        proc = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            startupinfo=startupinfo,
            creationflags=creationflags,
            timeout=5,
        )
        pids_to_kill: Set[int] = set()
        for line in proc.stdout.splitlines():
            line = line.strip()
            if f":{port} " in line and "LISTENING" in line:
                parts = line.split()
                if len(parts) >= 5:
                    with contextlib.suppress(ValueError):
                        pid = int(parts[-1])
                        if pid > 0 and pid != my_pid:
                            pids_to_kill.add(pid)

        for pid in pids_to_kill:
            logger.warning("Releasing port %d by terminating orphan PID %d", port, pid)
            subprocess.run(
                ["taskkill", "/F", "/PID", str(pid)],
                capture_output=True,
                startupinfo=startupinfo,
                creationflags=creationflags,
                timeout=5,
            )
            freed = True

        if freed:
            time.sleep(0.15)
    except Exception as e:
        logger.debug("Port release check for port %d note: %s", port, e)
    return freed


def _load_playfair() -> ctypes.CDLL:
    global _PLAYFAIR_DLL
    if _PLAYFAIR_DLL is not None:
        return _PLAYFAIR_DLL

    candidates = [
        get_app_root() / "handset" / "bin" / "playfair.dll",
        get_app_root() / "bin" / "playfair.dll",
        Path(__file__).parent / "bin" / "playfair.dll",
        Path(sys.executable).parent / "_internal" / "handset" / "bin" / "playfair.dll",
        Path(sys.executable).parent / "playfair.dll",
    ]
    if hasattr(sys, "_MEIPASS"):
        candidates.insert(0, Path(sys._MEIPASS) / "handset" / "bin" / "playfair.dll")
        candidates.insert(1, Path(sys._MEIPASS) / "playfair.dll")

    for cand in candidates:
        if cand.is_file():
            try:
                dll = ctypes.CDLL(str(cand))
                dll.zl_playfair_decrypt.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_char_p]
                dll.zl_playfair_decrypt.restype = None
                _PLAYFAIR_DLL = dll
                logger.info("Loaded playfair.dll from %s", cand)
                return dll
            except Exception as e:
                logger.warning("Failed loading playfair.dll from %s: %s", cand, e)

    raise OSError(f"playfair.dll not found in search paths: {[str(c) for c in candidates]}")


class FairPlay:
    """FairPlay (fp-setup) session key decryption wrapper for playfair.dll."""

    def __init__(self) -> None:
        self.keymsg: Optional[bytes] = None

    def setup(self, req: bytes) -> bytes:
        if len(req) != 16 or req[4] != 3:
            raise ValueError(f"Unsupported FairPlay version/length: len={len(req)}")
        self.keymsg = None
        idx = req[14]
        if idx >= len(REPLY_MESSAGES):
            idx = 0
        return REPLY_MESSAGES[idx]

    def handshake(self, req: bytes) -> bytes:
        if len(req) != 164 or req[4] != 3:
            raise ValueError(f"Unsupported FairPlay version/length: len={len(req)}")
        self.keymsg = bytes(req)
        return FP_HEADER + req[144:164]

    def decrypt(self, ekey: bytes) -> bytes:
        if self.keymsg is None or len(ekey) != 72:
            raise ValueError("FairPlay handshake not completed or invalid ekey length")
        out = ctypes.create_string_buffer(16)
        dll = _load_playfair()
        dll.zl_playfair_decrypt(self.keymsg, bytes(ekey), out)
        return bytes(out.raw)


def _sha512_16(*parts: bytes) -> bytes:
    h = hashlib.sha512()
    for p in parts:
        h.update(p)
    return h.digest()[:16]


def _get_identity() -> Tuple[Ed25519PrivateKey, bytes, str]:
    """Return persistent Ed25519 private key, public key bytes, and MAC address."""
    state_file = Path(os.environ.get("LOCALAPPDATA", ".")) / "iPhoneLink" / "airplay_identity.json"
    data = None
    if state_file.is_file():
        try:
            data = json.loads(state_file.read_text(encoding="utf-8"))
        except Exception:
            data = None

    seed = None
    mac = None
    if data and "seed" in data and "mac" in data:
        try:
            seed_bytes = bytes.fromhex(data["seed"])
            if len(seed_bytes) == 32:
                seed = seed_bytes
                mac = str(data["mac"])
        except Exception:
            seed = None

    if seed is None or mac is None:
        seed = secrets.token_bytes(32)
        mac_raw = bytearray(secrets.token_bytes(6))
        mac_raw[0] = (mac_raw[0] | 0x02) & 0xFE  # locally administered unicast MAC
        mac = ":".join(f"{b:02X}" for b in mac_raw)
        try:
            state_file.parent.mkdir(parents=True, exist_ok=True)
            state_file.write_text(json.dumps({"seed": seed.hex(), "mac": mac}), encoding="utf-8")
        except Exception:
            pass

    priv = Ed25519PrivateKey.from_private_bytes(seed)
    pub = priv.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw)
    return priv, pub, mac


def _get_lan_ipv4() -> List[str]:
    """Get active non-virtual IPv4 addresses."""
    out: List[str] = []
    try:
        host = socket.gethostname()
        for ip in socket.gethostbyname_ex(host)[2]:
            if not ip.startswith(("127.", "169.254.")):
                out.append(ip)
    except Exception:
        pass
    if not out:
        out.append("127.0.0.1")
    return out


def _valid_au(data: bytes) -> bool:
    """Validate that data is a sequence of 4-byte length-prefixed NALUs."""
    pos = 0
    n = len(data)
    if n < 5:
        return False
    while pos + 4 <= n:
        size = int.from_bytes(data[pos : pos + 4], "big")
        if size <= 0 or pos + 4 + size > n:
            return False
        # Forbidden zero bit in NAL header must be 0
        if data[pos + 4] & 0x80:
            return False
        pos += 4 + size
    return pos == n


def is_keyframe(avcc: bytes, hevc: bool = False) -> bool:
    """Detect if an AVCC access unit contains an IDR/IRAP keyframe."""
    pos = 0
    n = len(avcc)
    while pos + 5 <= n:
        size = int.from_bytes(avcc[pos : pos + 4], "big")
        header = avcc[pos + 4]
        if hevc:
            nal_type = (header >> 1) & 63
            if 16 <= nal_type <= 21:
                return True
        else:
            nal_type = header & 31
            if nal_type in (5, 7):  # IDR or SPS
                return True
        if size <= 0:
            break
        pos += 4 + size
    return False


class _VideoDecryptor:
    """AES-128-CTR video decryptor with automatic re-synchronization guard."""

    def __init__(self, key: bytes, iv: bytes) -> None:
        self.key = key
        self.iv = iv
        self.cipher = self._fresh()
        self.bad = 0

    def _fresh(self) -> Any:
        return AES.new(self.key, AES.MODE_CTR, nonce=b"", initial_value=self.iv)

    def decrypt(self, payload: bytes) -> Optional[bytes]:
        cand = self.cipher.decrypt(payload)
        if _valid_au(cand):
            self.bad = 0
            return cand
        fresh = self._fresh()
        retry = fresh.decrypt(payload)
        if _valid_au(retry):
            logger.info("AirPlay video keystream restarted by iOS; re-synchronized successfully.")
            self.cipher = fresh
            self.bad = 0
            return retry
        self.bad += 1
        return None


class _AudioDecoder:
    """RTP UDP AES-128-CBC decryptor + AAC-ELD to 48 kHz stereo s16le PCM decoder."""

    def __init__(self, key: bytes, iv: bytes, ct: int = 8) -> None:
        self._key = key
        self._iv = iv
        self._ok = (ct == 8)  # ct == 8 is AAC-ELD 44.1 kHz stereo
        self._dec: Optional[AacDecoder] = None
        if self._ok:
            try:
                self._dec = AacDecoder(bytes(AAC_ELD_44K), channels=2, out_rate=48000)
            except Exception as e:
                logger.warning("Failed initializing AAC decoder: %s", e)
                self._dec = None
        self._last_seq: Optional[int] = None
        self.decoded = 0
        self.failed = 0

    def decode(self, packet: bytes) -> bytes:
        if not self._ok or self._dec is None or len(packet) < 12:
            return b""
        payload = packet[12:]
        n = (len(payload) // 16) * 16
        if n:
            dec = AES.new(self._key, AES.MODE_CBC, self._iv)
            decrypted = dec.decrypt(payload[:n]) + payload[n:]
        else:
            decrypted = payload

        try:
            pcm = self._dec.decode(decrypted)
            if pcm:
                self.decoded += 1
                return pcm
        except Exception:
            self.failed += 1
        return b""

    def close(self) -> None:
        if self._dec is not None:
            with contextlib.suppress(Exception):
                self._dec.close()
            self._dec = None


class _RtspConn:
    def __init__(self, peer: str) -> None:
        self.peer = peer
        self.fairplay = FairPlay()
        self.ecdh_secret: Optional[bytes] = None
        self.ecdh_ours: Optional[X25519PrivateKey] = None
        self.ecdh_theirs: bytes = b""
        self.ed_theirs: bytes = b""
        self.aeskey: Optional[bytes] = None
        self.aesiv: Optional[bytes] = None


class AirPlayMirrorServer:
    """Native AirPlay Screen Mirroring & Audio Receiver for iPhone Link v3.0."""

    def __init__(
        self,
        name: str = "iPhone Link",
        airplay_port: int = AIRPLAY_PORT,
        http_port: int = AIRPLAY_HTTP_PORT,
        on_state: Optional[Callable[[str, Optional[str]], None]] = None,
    ) -> None:
        self.name = name
        self.airplay_port = airplay_port
        self.http_port = http_port
        self.on_state = on_state

        self._ed, self._pk, self.mac = _get_identity()
        self.features = (1518337766, 0)  # Features H.264
        self.display = (1920, 1080)

        self._zc = None
        self._infos: List[Any] = []
        self._stopped = False

        self._rtsp_server: Optional[asyncio.Server] = None
        self._http_server: Optional[asyncio.Server] = None
        self._mirror_server: Optional[asyncio.Server] = None
        self._audio_transport: Optional[asyncio.DatagramTransport] = None
        self._timing_transport: Optional[asyncio.DatagramTransport] = None

        self._tasks: Set[asyncio.Task] = set()
        self._video_subscribers: Set[asyncio.Queue] = set()
        self._audio_subscribers: Set[asyncio.Queue] = set()

        self.client_name: Optional[str] = None
        self.codec_string: str = "avc1.640028"
        self.sps_pps_config: bytes = b""
        self._sps: bytes = b""
        self._pps: bytes = b""
        self._sps_annexb: bytes = b""
        self._pps_annexb: bytes = b""
        self._first_frame_sent: bool = False
        self._codec_ready = asyncio.Event()

        self.width: int = 1080
        self.height: int = 1920
        self.is_streaming: bool = False
        self._last_good_au_t: float = 0.0
        self._loop: Optional[asyncio.AbstractEventLoop] = None

    def broadcast_video_chunk(self, chunk: bytes) -> None:
        """Thread-safe broadcast of an Annex-B framed chunk to all HTTP video subscribers."""
        for q in list(self._video_subscribers):
            try:
                if self._loop and self._loop.is_running():
                    self._loop.call_soon_threadsafe(q.put_nowait, chunk)
                else:
                    q.put_nowait(chunk)
            except Exception:
                pass

    def _spawn(self, coro) -> asyncio.Task:
        task = asyncio.create_task(coro)
        self._tasks.add(task)
        task.add_done_callback(self._tasks.discard)
        return task

    def _txt_airplay(self) -> Dict[str, str]:
        return {
            "deviceid": self.mac,
            "features": f"0x{self.features[0]:X},0x{self.features[1]:X}",
            "flags": "0x4",
            "model": MODEL,
            "pk": self._pk.hex(),
            "pi": PI,
            "srcvers": SOURCE_VERSION,
            "vv": "2",
        }

    def _txt_raop(self) -> Dict[str, str]:
        mac_no_colons = self.mac.replace(":", "")
        return {
            "ch": "2",
            "cn": "0,1,2,3",
            "da": "true",
            "et": "0,3,5",
            "vv": "2",
            "ft": f"0x{self.features[0]:X},0x{self.features[1]:X}",
            "am": MODEL,
            "md": "0,1,2",
            "rhd": "5.6.0.0",
            "pw": "false",
            "sf": "0x4",
            "sr": "44100",
            "ss": "16",
            "sv": "false",
            "tp": "UDP",
            "txtvers": "1",
            "vs": SOURCE_VERSION,
            "vn": "65537",
            "pk": self._pk.hex(),
        }

    def _advertise_sync(self) -> None:
        try:
            from zeroconf import IPVersion, ServiceInfo, Zeroconf

            ips = _get_lan_ipv4()
            addresses = [socket.inet_aton(ip) for ip in ips]
            host = f"iPhoneLink-{self.mac.replace(':', '')[-6:]}.local."

            self._zc = Zeroconf(ip_version=IPVersion.V4Only)

            raop_name = f"{self.mac.replace(':', '')}@{self.name}"

            airplay_info = ServiceInfo(
                "_airplay._tcp.local.",
                f"{self.name}._airplay._tcp.local.",
                addresses=addresses,
                port=self.airplay_port,
                properties=self._txt_airplay(),
                server=host,
            )
            raop_info = ServiceInfo(
                "_raop._tcp.local.",
                f"{raop_name}._raop._tcp.local.",
                addresses=addresses,
                port=self.airplay_port,
                properties=self._txt_raop(),
                server=host,
            )

            self._infos = [airplay_info, raop_info]
            for info in self._infos:
                self._zc.register_service(info, allow_name_change=True)
            logger.info("AirPlay mDNS service '%s' registered on port %d", self.name, self.airplay_port)
        except Exception as e:
            logger.warning("mDNS Zeroconf registration warning: %s", e)

        # Fallback Direct Unicast DNS-SD Announcement to last known paired iPhone IP
        self._send_direct_unicast_announcement()

    async def _advertise(self) -> None:
        await asyncio.to_thread(self._advertise_sync)

    def _send_direct_unicast_announcement(self) -> None:
        """Send direct UDP 5353 DNS-SD announcement to paired iPhone IP if router blocks multicast."""
        try:
            wifi_state_file = Path(os.environ.get("LOCALAPPDATA", ".")) / "iPhoneLink" / "wifi_state.json"
            if not wifi_state_file.is_file():
                return
            state = json.loads(wifi_state_file.read_text(encoding="utf-8"))
            last_ip = state.get("last_ip")
            if not last_ip:
                for dev in state.get("devices", {}).values():
                    if dev.get("last_ip"):
                        last_ip = dev["last_ip"]
                        break
            if last_ip:
                sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
                sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
                pkt = b"\x00\x00\x84\x00\x00\x00\x00\x01\x00\x00\x00\x00\x08_airplay\x04_tcp\x05local\x00\x00\x0c\x00\x01\x00\x00\x11\x94"
                sock.sendto(pkt, (last_ip, 5353))
                sock.close()
                logger.info("Sent direct unicast DNS-SD announcement to %s:5353", last_ip)
        except Exception as e:
            logger.debug("Direct unicast announcement non-fatal note: %s", e)

    def _unadvertise_sync(self) -> None:
        if self._zc is not None:
            for info in self._infos:
                with contextlib.suppress(Exception):
                    self._zc.unregister_service(info)
            with contextlib.suppress(Exception):
                self._zc.close()
            self._zc = None
            self._infos.clear()

    async def _unadvertise(self) -> None:
        await asyncio.to_thread(self._unadvertise_sync)

    async def _handle_rtsp(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        peer = writer.get_extra_info("peername")[0]
        conn = _RtspConn(peer)
        logger.info("AirPlay RTSP connection from %s", peer)

        try:
            while not self._stopped:
                head = await reader.readuntil(b"\r\n\r\n")
                lines = head.decode("latin-1").split("\r\n")
                parts = lines[0].split(" ") + ["", ""]
                method, url, _proto = parts[0], parts[1], parts[2]

                headers: Dict[str, str] = {}
                for line in lines[1:]:
                    if ":" in line:
                        k, v = line.split(":", 1)
                        headers[k.strip().lower()] = v.strip()

                content_len = int(headers.get("content-length", "0") or 0)
                body = await reader.readexactly(content_len) if content_len > 0 else b""

                status, extra, data = await self._dispatch_rtsp(conn, method, url, headers, body)

                resp_lines = [f"RTSP/1.0 {status}"]
                if "cseq" in headers:
                    resp_lines.append(f"CSeq: {headers['cseq']}")
                resp_lines.append(f"Server: AirTunes/{SOURCE_VERSION}")
                for k, v in extra.items():
                    resp_lines.append(f"{k}: {v}")
                resp_lines.append(f"Content-Length: {len(data)}")
                resp_lines.append("\r\n")

                writer.write("\r\n".join(resp_lines).encode("latin-1") + data)
                await writer.drain()

                if method == "TEARDOWN":
                    break
        except (asyncio.IncompleteReadError, ConnectionError):
            pass
        except Exception as e:
            logger.warning("RTSP handler exception: %s", e)
        finally:
            with contextlib.suppress(Exception):
                writer.close()
            logger.info("AirPlay RTSP disconnected: %s", peer)

    async def _dispatch_rtsp(
        self, conn: _RtspConn, method: str, url: str, headers: Dict[str, str], body: bytes
    ) -> Tuple[str, Dict[str, str], bytes]:
        ok = "200 OK"
        bplist = {"Content-Type": "application/x-apple-binary-plist"}
        octet = {"Content-Type": "application/octet-stream"}

        if method == "GET" and url == "/info":
            return ok, bplist, self._info(headers, body)

        if method == "POST" and url == "/pair-setup":
            return ok, octet, self._pk

        if method == "POST" and url == "/pair-verify":
            res = self._pair_verify(conn, body)
            if self.on_state:
                self.on_state("connected", self.client_name)
            return ok, octet, res

        if method == "POST" and url == "/fp-setup":
            if len(body) == 16:
                return ok, octet, conn.fairplay.setup(body)
            res = conn.fairplay.handshake(body)
            if self.on_state:
                self.on_state("connected", self.client_name)
            return ok, octet, res

        if method == "OPTIONS":
            return ok, {"Public": "SETUP, RECORD, FLUSH, TEARDOWN, OPTIONS, GET_PARAMETER, SET_PARAMETER"}, b""

        if method == "SETUP":
            res = await self._setup(conn, body)
            return ok, bplist, res

        if method == "GET_PARAMETER":
            if b"volume" in body:
                return ok, {"Content-Type": "text/parameters"}, b"volume: 0.000000\r\n"
            return ok, {}, b""

        if method == "RECORD":
            if self.on_state:
                self.on_state("connected", self.client_name)
            return ok, {"Audio-Latency": "11025", "Audio-Jack-Status": "connected; type=analog"}, b""

        if method == "TEARDOWN":
            self._close_streams()
            if self.on_state:
                self.on_state("stopped", None)
            return ok, {}, b""

        return ok, {}, b""

    def _info(self, headers: Dict[str, str], body: bytes) -> bytes:
        w, h = self.display
        res: Dict[str, Any] = {
            "deviceID": self.mac,
            "macAddress": self.mac,
            "pk": self._pk,
            "features": (self.features[1] << 32) | self.features[0],
            "name": self.name,
            "pi": PI,
            "vv": 2,
            "statusFlags": 68,
            "keepAliveLowPower": True,
            "sourceVersion": SOURCE_VERSION,
            "keepAliveSendStatsAsBody": True,
            "model": MODEL,
            "initialVolume": 0.0,
            "displays": [
                {
                    "uuid": "e0ff8a27-6738-3d56-8a16-cc53aacee925",
                    "widthPhysical": w,
                    "heightPhysical": h,
                    "width": w,
                    "height": h,
                    "widthPixels": w,
                    "heightPixels": h,
                    "rotation": 0,
                    "refreshRate": 0.016666666666666666,
                    "maxFPS": 60,
                    "overscanned": False,
                    "features": 14,
                }
            ],
        }
        return plistlib.dumps(res, fmt=plistlib.FMT_BINARY)

    def _pair_verify(self, conn: _RtspConn, body: bytes) -> bytes:
        if len(body) < 4:
            return b""
        flag = body[0]
        if flag == 1 and len(body) == 68:
            conn.ecdh_theirs = body[4:36]
            conn.ed_theirs = body[36:68]
            conn.ecdh_ours = X25519PrivateKey.generate()
            ours = conn.ecdh_ours.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw)
            conn.ecdh_secret = conn.ecdh_ours.exchange(X25519PublicKey.from_public_bytes(conn.ecdh_theirs))

            sig = self._ed.sign(ours + conn.ecdh_theirs)
            key = _sha512_16(b"Pair-Verify-AES-Key", conn.ecdh_secret)
            iv = _sha512_16(b"Pair-Verify-AES-IV", conn.ecdh_secret)
            enc = AES.new(key, AES.MODE_CTR, nonce=b"", initial_value=iv).encrypt(sig)
            return ours + enc

        if flag == 0 and len(body) == 68 and conn.ecdh_secret is not None:
            key = _sha512_16(b"Pair-Verify-AES-Key", conn.ecdh_secret)
            iv = _sha512_16(b"Pair-Verify-AES-IV", conn.ecdh_secret)
            ctr = AES.new(key, AES.MODE_CTR, nonce=b"", initial_value=iv)
            ctr.encrypt(bytes(64))  # discard keystream
            sig = ctr.encrypt(body[4:])
            ours = conn.ecdh_ours.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw)
            try:
                Ed25519PublicKey.from_public_bytes(conn.ed_theirs).verify(sig, conn.ecdh_theirs + ours)
            except Exception as e:
                logger.warning("airplay: pair-verify signature warning (continuing): %s", e)
            return b""

        return b""

    async def _setup(self, conn: _RtspConn, body: bytes) -> bytes:
        req = plistlib.loads(body) if body else {}
        res: Dict[str, Any] = {}

        # Derivasi master session key (ekey + ecdh_secret)
        ekey = req.get("ekey")
        if not ekey and isinstance(req.get("streams"), list):
            for s in req["streams"]:
                if s.get("ekey"):
                    ekey = s["ekey"]
                    break

        if ekey:
            aeskey = conn.fairplay.decrypt(bytes(ekey))
            if conn.ecdh_secret:
                aeskey = _sha512_16(aeskey, conn.ecdh_secret)
            conn.aeskey = aeskey
            if "eiv" in req:
                conn.aesiv = bytes(req["eiv"])

        self.client_name = req.get("name") or req.get("model") or "iPhone/iPad"
        logger.info("AirPlay client connected: %s", self.client_name)
        if self.on_state:
            self.on_state("connected", self.client_name)

        # Setup timing transport
        loop = asyncio.get_running_loop()
        timing_transport, _ = await loop.create_datagram_endpoint(
            asyncio.DatagramProtocol, local_addr=("0.0.0.0", 0)
        )
        self._timing_transport = timing_transport
        res["timingPort"] = timing_transport.get_extra_info("sockname")[1]
        res["eventPort"] = 0

        streams: List[Dict[str, Any]] = []
        for s in req.get("streams", []):
            stype = s.get("type")
            if stype == 110 and conn.aeskey:  # Video stream
                stream_id = int(s.get("streamConnectionID", 0)) & 0xFFFFFFFFFFFFFFFF
                vport = await self._start_video_stream(conn, stream_id)
                streams.append({"type": 110, "dataPort": vport})
            elif stype == 96 and conn.aeskey:  # Audio stream
                aport, cport = await self._start_audio_stream(conn, s)
                streams.append({"type": 96, "dataPort": aport, "controlPort": cport})

        if streams:
            res["streams"] = streams

        return plistlib.dumps(res, fmt=plistlib.FMT_BINARY)

    async def _start_video_stream(self, conn: _RtspConn, stream_id: int) -> int:
        key = _sha512_16(f"AirPlayStreamKey{stream_id}".encode("ascii"), conn.aeskey)
        iv = _sha512_16(f"AirPlayStreamIV{stream_id}".encode("ascii"), conn.aeskey)

        if self._mirror_server is not None:
            with contextlib.suppress(Exception):
                self._mirror_server.close()
                await self._mirror_server.wait_closed()
            self._mirror_server = None

        async def handle(reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
            peer = writer.get_extra_info("peername")
            logger.info("AirPlay video TCP connected from %s", peer)
            cipher = _VideoDecryptor(key, iv)
            try:
                await self._video_stream_loop(reader, cipher)
            except Exception as e:
                logger.debug("Video stream loop ended: %s", e)
            finally:
                with contextlib.suppress(Exception):
                    writer.close()

        self._mirror_server = await asyncio.start_server(handle, "0.0.0.0", 0, reuse_address=True)
        port = self._mirror_server.sockets[0].getsockname()[1]
        logger.info("AirPlay video receiver listening on port %d", port)
        return port

    @staticmethod
    def _extract_sps_pps(data: bytes) -> Tuple[bytes, bytes, str]:
        """Extract SPS, PPS, and AVC codec string from AVCDecoderConfigurationRecord or NAL stream."""
        sps = b""
        pps = b""
        codec_str = "avc1.640028"

        # Check if AVCDecoderConfigurationRecord (starts with 0x01)
        if len(data) >= 7 and data[0] == 1:
            try:
                profile_idc = data[1]
                profile_compat = data[2]
                level_idc = data[3]
                codec_str = f"avc1.{profile_idc:02x}{profile_compat:02x}{level_idc:02x}"
                num_sps = data[5] & 0x1F
                pos = 6
                for _ in range(num_sps):
                    if pos + 2 > len(data):
                        break
                    sps_len = int.from_bytes(data[pos : pos + 2], "big")
                    pos += 2
                    if pos + sps_len > len(data):
                        break
                    if not sps:
                        sps = data[pos : pos + sps_len]
                    pos += sps_len
                if pos < len(data):
                    num_pps = data[pos]
                    pos += 1
                    for _ in range(num_pps):
                        if pos + 2 > len(data):
                            break
                        pps_len = int.from_bytes(data[pos : pos + 2], "big")
                        pos += 2
                        if pos + pps_len > len(data):
                            break
                        if not pps:
                            pps = data[pos : pos + pps_len]
                        pos += pps_len
                return sps, pps, codec_str
            except Exception:
                pass

        # Check if Annex-B format (00 00 00 01)
        if b"\x00\x00\x00\x01" in data:
            parts = data.split(b"\x00\x00\x00\x01")
            for p in parts:
                if not p:
                    continue
                nal_type = p[0] & 0x1F
                if nal_type == 7 and not sps:
                    sps = p
                    if len(p) >= 4:
                        codec_str = f"avc1.{p[1:4].hex()}"
                elif nal_type == 8 and not pps:
                    pps = p

        # Check if AVCC format (4-byte length prefixes)
        if not sps:
            pos = 0
            n = len(data)
            while pos + 4 < n:
                sz = int.from_bytes(data[pos : pos + 4], "big")
                if sz <= 0 or pos + 4 + sz > n:
                    break
                nal = data[pos + 4 : pos + 4 + sz]
                nal_type = nal[0] & 0x1F
                if nal_type == 7 and not sps:
                    sps = nal
                    if len(nal) >= 4:
                        codec_str = f"avc1.{nal[1:4].hex()}"
                elif nal_type == 8 and not pps:
                    pps = nal
                pos += 4 + sz

        return sps, pps, codec_str

    async def _video_stream_loop(self, reader: asyncio.StreamReader, cipher: _VideoDecryptor) -> None:
        """Process 128-byte AirPlay video TCP header and payload stream."""
        while not self._stopped:
            header = await reader.readexactly(128)
            payload_size = int.from_bytes(header[0:4], "little")
            payload_type = int.from_bytes(header[4:6], "little")

            payload = await reader.readexactly(payload_size) if payload_size > 0 else b""

            if payload_type == 1 and payload:  # Codec configuration (avcC / SPS / PPS) — NOT ENCRYPTED
                w, h = 0.0, 0.0
                with contextlib.suppress(Exception):
                    w, h = struct.unpack_from("<ff", header, 56)
                if w <= 0 or h <= 0:
                    with contextlib.suppress(Exception):
                        w, h = struct.unpack_from("<ff", header, 40)
                if w > 0 and h > 0:
                    self.width = int(w)
                    self.height = int(h)

                sps, pps, codec_str = self._extract_sps_pps(payload)
                if sps:
                    self._sps = sps
                    self._sps_annexb = b"\x00\x00\x00\x01" + sps
                if pps:
                    self._pps = pps
                    self._pps_annexb = b"\x00\x00\x00\x01" + pps
                self.sps_pps_config = bytes(payload)
                self.codec_string = codec_str
                self._codec_ready.set()
                logger.info("AirPlay video parameters: %dx%d codec=%s (sps=%d B, pps=%d B)", self.width, self.height, self.codec_string, len(sps), len(pps))
                if self.on_state:
                    self.on_state("connected", self.client_name)

            elif payload_type == 0 and payload:  # Encrypted video access unit
                decrypted = cipher.decrypt(payload)
                if decrypted is None:
                    continue

                pos = 0
                n = len(decrypted)
                annexb_parts: List[bytes] = []
                has_idr = False

                while pos + 4 <= n:
                    sz = int.from_bytes(decrypted[pos : pos + 4], "big")
                    if sz <= 0 or pos + 4 + sz > n:
                        break
                    nal = decrypted[pos + 4 : pos + 4 + sz]
                    nal_type = nal[0] & 0x1F
                    if nal_type in (5, 7):
                        has_idr = True
                    annexb_parts.append(b"\x00\x00\x00\x01" + nal)
                    pos += 4 + sz

                if not annexb_parts:
                    continue

                # Prepend SPS + PPS Annex-B to IDR frames or initial frame to ensure instant keyframe decoding
                if (has_idr or not self._first_frame_sent) and self._sps_annexb and self._pps_annexb:
                    first_nal_type = annexb_parts[0][4] & 0x1F if len(annexb_parts[0]) > 4 else 0
                    if first_nal_type != 7:
                        annexb_parts.insert(0, self._pps_annexb)
                        annexb_parts.insert(0, self._sps_annexb)
                    self._first_frame_sent = True

                annexb_frame = b"".join(annexb_parts)
                type_byte = b"\x00" if (has_idr or not self.is_streaming) else b"\x01"
                chunk = struct.pack(">I", len(annexb_frame) + 1) + type_byte + annexb_frame

                if not self.is_streaming:
                    self.is_streaming = True
                    logger.info("AirPlay video stream broadcasting to browser (/live)")
                    if self.on_state:
                        self.on_state("streaming", None)

                self._last_good_au_t = time.monotonic()
                for q in list(self._video_subscribers):
                    if q.full():
                        with contextlib.suppress(Exception):
                            q.get_nowait()
                    with contextlib.suppress(Exception):
                        q.put_nowait(chunk)

    async def _start_audio_stream(self, conn: _RtspConn, stream_info: Dict[str, Any]) -> Tuple[int, int]:
        ct = stream_info.get("ct", 8)
        audio_key = _sha512_16(b"AirPlayStreamKey", conn.aeskey)
        audio_iv = _sha512_16(b"AirPlayStreamIV", conn.aeskey)

        decoder = _AudioDecoder(audio_key, audio_iv, ct)

        loop = asyncio.get_running_loop()

        class AudioProtocol(asyncio.DatagramProtocol):
            def __init__(self, parent: AirPlayMirrorServer, dec: _AudioDecoder) -> None:
                self.parent = parent
                self.dec = dec

            def datagram_received(self, data: bytes, addr) -> None:
                pcm = self.dec.decode(data)
                if pcm:
                    # Format as [uint32_be len][pcm_s16le]
                    packet = struct.pack(">I", len(pcm)) + pcm
                    for q in list(self.parent._audio_subscribers):
                        if q.full():
                            with contextlib.suppress(Exception):
                                q.get_nowait()
                        with contextlib.suppress(Exception):
                            q.put_nowait(packet)

        transport, _ = await loop.create_datagram_endpoint(
            lambda: AudioProtocol(self, decoder), local_addr=("0.0.0.0", 0)
        )
        self._audio_transport = transport
        aport = transport.get_extra_info("sockname")[1]

        # Control transport (empty UDP for RTCP/keepalive)
        control_transport, _ = await loop.create_datagram_endpoint(
            asyncio.DatagramProtocol, local_addr=("0.0.0.0", 0)
        )
        cport = control_transport.get_extra_info("sockname")[1]

        logger.info("AirPlay audio receiver listening on UDP %d (ctrl: %d)", aport, cport)
        return aport, cport

    def reset_stream_state(self) -> None:
        """Reset video and audio stream state without shutting down main RTSP / HTTP server."""
        self._close_streams()
        self.is_streaming = False
        self._first_frame_sent = False

    def _close_streams(self) -> None:
        self.is_streaming = False
        if self._mirror_server is not None:
            with contextlib.suppress(Exception):
                self._mirror_server.close()
            self._mirror_server = None
        if self._audio_transport is not None:
            with contextlib.suppress(Exception):
                self._audio_transport.close()
            self._audio_transport = None
        if self._timing_transport is not None:
            with contextlib.suppress(Exception):
                self._timing_transport.close()
            self._timing_transport = None

    async def _handle_http(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        """Handle 127.0.0.1:8766 HTTP API endpoints (/codec, /stream.bin, /live, /audio.bin, /touch, /pli)."""
        sock = writer.get_extra_info("socket")
        if sock is not None:
            with contextlib.suppress(Exception):
                sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)

        try:
            line = await reader.readline()
            if not line:
                writer.close()
                return
            parts = line.decode("latin-1", errors="ignore").split()
            if not parts:
                writer.close()
                return
            method = parts[0]
            path = parts[1].split("?", 1)[0] if len(parts) > 1 else "/"

            content_length = 0
            while True:
                hdr = await reader.readline()
                if not hdr or hdr in (b"\r\n", b"\n"):
                    break
                if hdr.lower().startswith(b"content-length:"):
                    with contextlib.suppress(Exception):
                        content_length = int(hdr.split(b":", 1)[1].strip())

            if content_length > 0:
                await reader.readexactly(content_length)

            if method == "POST" and path in ("/touch", "/button", "/key", "/restart", "/pli"):
                self._first_frame_sent = False
                resp = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 15\r\nConnection: close\r\n\r\n{\"ok\":true}\r\n"
                writer.write(resp)
                await writer.drain()
                writer.close()
                return

            if path == "/codec":
                # Wait briefly if SPS/PPS is still arriving
                if not self.sps_pps_config:
                    with contextlib.suppress(Exception):
                        await asyncio.wait_for(self._codec_ready.wait(), timeout=2.5)

                desc_b64 = base64.b64encode(self.sps_pps_config).decode("ascii") if self.sps_pps_config else ""
                payload = json.dumps({
                    "codec": self.codec_string,
                    "description": desc_b64,
                    "width": self.width,
                    "height": self.height,
                    "fps": 60,
                    "is_airplay": True,
                }).encode("utf-8")
                writer.write(
                    f"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {len(payload)}\r\nConnection: close\r\n\r\n".encode("latin-1")
                    + payload
                )
                await writer.drain()
                writer.close()
                return

            if path in ("/stream.bin", "/live"):
                writer.write(b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n")
                await writer.drain()

                q = asyncio.Queue(maxsize=120)
                self._video_subscribers.add(q)
                try:
                    while not self._stopped:
                        frame = await q.get()
                        writer.write(frame)
                        await writer.drain()
                finally:
                    self._video_subscribers.discard(q)
                return

            if path == "/audio.bin":
                writer.write(b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n")
                await writer.drain()

                q = asyncio.Queue(maxsize=60)
                self._audio_subscribers.add(q)
                try:
                    while not self._stopped:
                        audio_chunk = await q.get()
                        writer.write(audio_chunk)
                        await writer.drain()
                finally:
                    self._audio_subscribers.discard(q)
                return

            # Default 404
            writer.write(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            await writer.drain()
        except (asyncio.IncompleteReadError, ConnectionError):
            pass
        except Exception as e:
            logger.debug("HTTP handler note: %s", e)
        finally:
            with contextlib.suppress(Exception):
                writer.close()

    async def serve(self) -> None:
        """Run the RTSP, mDNS, and HTTP servers until cancelled with singleton socket reuse."""
        self._stopped = False
        self._loop = asyncio.get_running_loop()
        release_port_if_held_by_other_process(self.airplay_port)
        release_port_if_held_by_other_process(self.http_port)

        # Reuse existing server sockets if already bound and listening
        if self._rtsp_server is None or not self._rtsp_server.is_serving():
            self._rtsp_server = await asyncio.start_server(
                self._handle_rtsp, "0.0.0.0", self.airplay_port, reuse_address=True
            )
        if self._http_server is None or not self._http_server.is_serving():
            self._http_server = await asyncio.start_server(
                self._handle_http, "127.0.0.1", self.http_port, reuse_address=True
            )

        logger.info(
            "AirPlayMirrorServer ready: RTSP on port %d, HTTP API on port %d",
            self.airplay_port,
            self.http_port,
        )

        try:
            await self._advertise()
            while not self._stopped:
                await asyncio.sleep(0.5)
        except asyncio.CancelledError:
            pass
        finally:
            await self.close()

    async def close(self) -> None:
        self._stopped = True
        await self._unadvertise()
        self._close_streams()

        if self._rtsp_server is not None:
            with contextlib.suppress(Exception):
                self._rtsp_server.close()
                await self._rtsp_server.wait_closed()
            self._rtsp_server = None

        if self._http_server is not None:
            with contextlib.suppress(Exception):
                self._http_server.close()
                await self._http_server.wait_closed()
            self._http_server = None

        for task in list(self._tasks):
            task.cancel()
        self._tasks.clear()
        logger.info("AirPlayMirrorServer closed cleanly.")

    def stop_and_release_ports(self) -> None:
        """Synchronously close RTSP, HTTP and mDNS servers and release all listening ports."""
        self._stopped = True
        if self._loop and self._loop.is_running():
            try:
                fut = asyncio.run_coroutine_threadsafe(self.close(), self._loop)
                fut.result(timeout=2.0)
            except Exception:
                pass
        else:
            if self._rtsp_server is not None:
                with contextlib.suppress(Exception):
                    self._rtsp_server.close()
                self._rtsp_server = None
            if self._http_server is not None:
                with contextlib.suppress(Exception):
                    self._http_server.close()
                self._http_server = None


_GLOBAL_AIRPLAY_SERVER: Optional[AirPlayMirrorServer] = None
_GLOBAL_AIRPLAY_LOCK = threading.Lock()


def get_global_airplay_server() -> Optional[AirPlayMirrorServer]:
    global _GLOBAL_AIRPLAY_SERVER
    return _GLOBAL_AIRPLAY_SERVER


def get_or_create_airplay_server(
    name: str = "iPhone Link",
    airplay_port: int = AIRPLAY_PORT,
    http_port: int = AIRPLAY_HTTP_PORT,
    on_state: Optional[Callable[[str, Optional[str]], None]] = None,
) -> AirPlayMirrorServer:
    """Return the global AirPlayMirrorServer singleton, creating it if needed."""
    global _GLOBAL_AIRPLAY_SERVER
    with _GLOBAL_AIRPLAY_LOCK:
        if _GLOBAL_AIRPLAY_SERVER is None or _GLOBAL_AIRPLAY_SERVER._stopped:
            _GLOBAL_AIRPLAY_SERVER = AirPlayMirrorServer(
                name=name,
                airplay_port=airplay_port,
                http_port=http_port,
                on_state=on_state,
            )
        else:
            if on_state is not None:
                _GLOBAL_AIRPLAY_SERVER.on_state = on_state
        return _GLOBAL_AIRPLAY_SERVER
