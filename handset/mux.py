"""usbmuxd plist client for the Apple mobile-device service on Windows.

The service listens on 127.0.0.1:27015 after Apple Devices or iTunes is installed.
Each packet is a little-endian length, then version, message type, tag, and an XML plist.
"""

from __future__ import annotations

import plistlib
import socket
import struct
import threading
from contextlib import contextmanager

USB_HOST = ("127.0.0.1", 27015)
LOCKDOWN_PORT = 62078
PLIST_VERSION = 1
PLIST_MESSAGE = 8
HEADER = struct.Struct("<III")
LENGTH = struct.Struct("<I")
BE_LENGTH = struct.Struct(">I")


class MuxError(ConnectionError):
    pass


def list_devices(address: tuple[str, int] = USB_HOST) -> list[dict]:
    with _session(address) as sock:
        response = _exchange(sock, {"MessageType": "ListDevices"}, tag=1)
    devices = []
    for item in response.get("DeviceList") or []:
        if item.get("MessageType") != "Attached":
            continue
        props = item.get("Properties") or {}
        devices.append(
            {
                "udid": props.get("SerialNumber") or "",
                "connection": props.get("ConnectionType") or "USB",
                "deviceId": item.get("DeviceID"),
            }
        )
    return [device for device in devices if device["udid"] and device["deviceId"] is not None]


@contextmanager
def open_lockdown(device_id: int, address: tuple[str, int] = USB_HOST):
    with _session(address) as sock:
        port = socket.htons(LOCKDOWN_PORT)
        result = _exchange(
            sock,
            {"MessageType": "Connect", "DeviceID": device_id, "PortNumber": port},
            tag=1,
        )
        if result.get("MessageType") != "Result" or result.get("Number") not in (0, None):
            number = result.get("Number")
            raise MuxError(_result_message(number))
        yield sock


def lockdown_request(sock: socket.socket, request: str, **options) -> dict:
    message = {"Label": "handset", "Request": request, "ProtocolVersion": "2"}
    message.update(options)
    payload = plistlib.dumps(message)
    sock.sendall(BE_LENGTH.pack(len(payload)) + payload)
    size = BE_LENGTH.unpack(_recv_exact(sock, 4))[0]
    if size > 8_000_000:
        raise MuxError("The phone sent an unexpectedly large reply.")
    parsed = plistlib.loads(_recv_exact(sock, size))
    if not isinstance(parsed, dict):
        raise MuxError("The phone sent a reply this window cannot read.")
    return parsed


def _exchange(sock: socket.socket, fields: dict, tag: int) -> dict:
    request = {
        "ClientVersionString": "handset",
        "ProgName": "handset",
        "kLibUSBMuxVersion": 3,
    }
    request.update(fields)
    payload = plistlib.dumps(request)
    body = HEADER.pack(PLIST_VERSION, PLIST_MESSAGE, tag) + payload
    sock.sendall(LENGTH.pack(4 + len(body)) + body)
    version, message, reply_tag, data = _recv_packet(sock)
    if version != PLIST_VERSION or message != PLIST_MESSAGE:
        raise MuxError("The Apple device service speaks a protocol this window does not.")
    if reply_tag not in (tag, 0):
        raise MuxError("The Apple device service reply did not match the request.")
    parsed = plistlib.loads(data)
    if not isinstance(parsed, dict):
        raise MuxError("The Apple device service sent an unexpected reply.")
    return parsed


def _recv_packet(sock: socket.socket) -> tuple[int, int, int, bytes]:
    length = LENGTH.unpack(_recv_exact(sock, 4))[0]
    if length < 16:
        raise MuxError("The Apple device service sent a truncated packet.")
    rest = _recv_exact(sock, length - 4)
    version, message, tag = HEADER.unpack(rest[:12])
    return version, message, tag, rest[12:]


def _recv_exact(sock: socket.socket, size: int) -> bytes:
    chunks = []
    remaining = size
    while remaining:
        chunk = sock.recv(remaining)
        if not chunk:
            raise MuxError("The Apple device service closed the connection.")
        chunks.append(chunk)
        remaining -= len(chunk)
    return b"".join(chunks)


@contextmanager
def _session(address: tuple[str, int]):
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.settimeout(4)
    try:
        try:
            sock.connect(address)
        except OSError as error:
            raise MuxError(
                "Apple Mobile Device Service is not running. Install Apple Devices from the Microsoft Store, then reconnect the iPhone."
            ) from error
        yield sock
    finally:
        sock.close()


def _result_message(number: int | None) -> str:
    reasons = {
        2: "That iPhone is no longer connected.",
        3: "The phone refused the connection. Unlock it and keep it on the cable.",
        4: "The requested phone service is not available.",
    }
    return reasons.get(number, "The phone did not accept the connection.")


def self_check() -> None:
    """Speak the list request to a local stand-in and require one device back."""
    ready = threading.Event()
    holder: dict[str, int] = {}

    def serve() -> None:
        server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        server.bind(("127.0.0.1", 0))
        holder["port"] = server.getsockname()[1]
        server.listen(1)
        ready.set()
        conn, _ = server.accept()
        with conn:
            version, message, tag, data = _recv_packet(conn)
            assert version == PLIST_VERSION and message == PLIST_MESSAGE and tag == 1
            request = plistlib.loads(data)
            assert request["MessageType"] == "ListDevices"
            reply = plistlib.dumps(
                {
                    "DeviceList": [
                        {
                            "MessageType": "Attached",
                            "DeviceID": 7,
                            "Properties": {"SerialNumber": "TESTUDID", "ConnectionType": "USB"},
                        }
                    ]
                }
            )
            body = HEADER.pack(PLIST_VERSION, PLIST_MESSAGE, tag) + reply
            conn.sendall(LENGTH.pack(4 + len(body)) + body)
        server.close()

    thread = threading.Thread(target=serve)
    thread.start()
    ready.wait(2)
    found = list_devices(("127.0.0.1", holder["port"]))
    thread.join(2)
    if found != [{"udid": "TESTUDID", "connection": "USB", "deviceId": 7}]:
        raise AssertionError(found)
