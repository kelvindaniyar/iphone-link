"""Stand-in for the compiled lzss module.

pyimg4 imports it at load time, but the developer disk image mount never
compresses or expands LZSS firmware payloads.
"""


def compress(data: bytes) -> bytes:
    raise RuntimeError("LZSS firmware compression is not available in Handset.")


def decompress(data: bytes) -> bytes:
    raise RuntimeError("LZSS firmware decompression is not available in Handset.")
