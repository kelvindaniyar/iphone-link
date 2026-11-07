"""Stand-in for the compiled lzfse module.

pyimg4 imports it at load time on Windows, but the developer disk image mount
never compresses or expands LZFSE firmware payloads.
"""


def compress(data: bytes) -> bytes:
    raise RuntimeError("LZFSE firmware compression is not available in Handset.")


def decompress(data: bytes) -> bytes:
    raise RuntimeError("LZFSE firmware decompression is not available in Handset.")
