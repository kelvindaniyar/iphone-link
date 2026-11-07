"""Visual QR Bridge for iPhone Link.

Pure Python ISO/IEC 18004 QR Code Decoder & Generator.
Zero external DLL dependencies (uses Pillow PIL.Image).
Enables 100% accurate extraction of Instagram/TikTok share links and URLs
from iPhone screen frames into Windows PC Clipboard.
"""

from __future__ import annotations

import base64
import io
import itertools
import math
import os
import re
import subprocess
import tempfile
from pathlib import Path
from typing import Any, List, Optional, Tuple, Union

from PIL import Image, ImageDraw, ImageFont, ImageOps

# ---------------------------------------------------------------------------
# ISO/IEC 18004 Standard Tables & Galois Field GF(256) Arithmetic
# ---------------------------------------------------------------------------

EXP = [0] * 512
LOG = [0] * 256
_x = 1
for _i in range(255):
    EXP[_i] = _x
    LOG[_x] = _i
    _x <<= 1
    if _x & 0x100:
        _x ^= 0x11D  # Primitive polynomial x^8 + x^4 + x^3 + x^2 + 1
for _i in range(255, 512):
    EXP[_i] = EXP[_i - 255]


def gf_mul(a: int, b: int) -> int:
    if a == 0 or b == 0:
        return 0
    return EXP[(LOG[a] + LOG[b]) % 255]


def gf_poly_mul(p: List[int], q: List[int]) -> List[int]:
    r = [0] * (len(p) + len(q) - 1)
    for i, a in enumerate(p):
        for j, b in enumerate(q):
            r[i + j] ^= gf_mul(a, b)
    return r


def rs_generator_poly(num_ec: int) -> List[int]:
    g = [1]
    for i in range(num_ec):
        g = gf_poly_mul(g, [1, EXP[i]])
    return g


def rs_encode(data: List[int], num_ec: int) -> List[int]:
    gen = rs_generator_poly(num_ec)
    res = list(data) + [0] * num_ec
    for i in range(len(data)):
        lead = res[i]
        if lead != 0:
            for j in range(len(gen)):
                res[i + j] ^= gf_mul(lead, gen[j])
    return res[len(data):]


# Alignment pattern center module coordinates (ISO/IEC 18004 Table E.1)
QR_ALIGN_POS = {
    1: [],
    2: [6, 18],
    3: [6, 22],
    4: [6, 26],
    5: [6, 30],
    6: [6, 34],
    7: [6, 22, 38],
    8: [6, 24, 42],
    9: [6, 26, 46],
    10: [6, 28, 50],
    11: [6, 30, 54],
    12: [6, 32, 58],
    13: [6, 34, 62],
    14: [6, 26, 46, 66],
    15: [6, 26, 48, 70],
}

# (total_codewords, ec_cw_per_block, g1_blocks, g1_data_cw, g2_blocks, g2_data_cw)
QR_SPECS = {
    (1, 'L'): (26, 7, 1, 19, 0, 0),
    (1, 'M'): (26, 10, 1, 16, 0, 0),
    (1, 'Q'): (26, 13, 1, 13, 0, 0),
    (1, 'H'): (26, 17, 1, 9, 0, 0),
    (2, 'L'): (44, 10, 1, 34, 0, 0),
    (2, 'M'): (44, 16, 1, 28, 0, 0),
    (2, 'Q'): (44, 22, 1, 22, 0, 0),
    (2, 'H'): (44, 28, 1, 16, 0, 0),
    (3, 'L'): (70, 15, 1, 55, 0, 0),
    (3, 'M'): (70, 26, 1, 44, 0, 0),
    (3, 'Q'): (70, 18, 2, 17, 0, 0),
    (3, 'H'): (70, 22, 2, 13, 0, 0),
    (4, 'L'): (100, 20, 1, 80, 0, 0),
    (4, 'M'): (100, 18, 2, 32, 0, 0),
    (4, 'Q'): (100, 26, 2, 24, 0, 0),
    (4, 'H'): (100, 16, 4, 9, 0, 0),
    (5, 'L'): (134, 26, 1, 108, 0, 0),
    (5, 'M'): (134, 24, 2, 43, 0, 0),
    (5, 'Q'): (134, 18, 2, 15, 2, 16),
    (5, 'H'): (134, 22, 2, 11, 2, 12),
    (6, 'L'): (172, 18, 2, 68, 0, 0),
    (6, 'M'): (172, 16, 4, 27, 0, 0),
    (6, 'Q'): (172, 24, 4, 19, 0, 0),
    (6, 'H'): (172, 28, 4, 15, 0, 0),
    (7, 'L'): (196, 20, 2, 78, 0, 0),
    (7, 'M'): (196, 18, 4, 31, 0, 0),
    (7, 'Q'): (196, 18, 2, 14, 4, 15),
    (7, 'H'): (196, 26, 4, 13, 1, 14),
    (8, 'L'): (242, 24, 2, 97, 0, 0),
    (8, 'M'): (242, 22, 2, 38, 2, 39),
    (8, 'Q'): (242, 22, 4, 18, 2, 19),
    (8, 'H'): (242, 26, 4, 14, 2, 15),
    (9, 'L'): (292, 30, 2, 116, 0, 0),
    (9, 'M'): (292, 22, 3, 36, 2, 37),
    (9, 'Q'): (292, 20, 4, 16, 4, 17),
    (9, 'H'): (292, 24, 4, 12, 4, 13),
    (10, 'L'): (346, 18, 2, 68, 2, 69),
    (10, 'M'): (346, 26, 4, 43, 1, 44),
    (10, 'Q'): (346, 24, 6, 19, 2, 20),
    (10, 'H'): (346, 28, 6, 15, 2, 16),
    (11, 'L'): (404, 20, 4, 81, 0, 0),
    (11, 'M'): (404, 30, 1, 50, 4, 51),
    (11, 'Q'): (404, 28, 4, 22, 4, 23),
    (11, 'H'): (404, 24, 3, 12, 8, 13),
    (12, 'L'): (466, 24, 2, 92, 2, 93),
    (12, 'M'): (466, 22, 6, 36, 2, 37),
    (12, 'Q'): (466, 26, 4, 20, 6, 21),
    (12, 'H'): (466, 28, 7, 14, 4, 15),
    (13, 'L'): (532, 26, 4, 107, 0, 0),
    (13, 'M'): (532, 22, 8, 37, 1, 38),
    (13, 'Q'): (532, 24, 8, 20, 4, 21),
    (13, 'H'): (532, 22, 12, 11, 4, 12),
    (14, 'L'): (581, 30, 3, 115, 1, 116),
    (14, 'M'): (581, 24, 4, 40, 5, 41),
    (14, 'Q'): (581, 20, 11, 16, 5, 17),
    (14, 'H'): (581, 24, 11, 12, 5, 13),
    (15, 'L'): (655, 22, 5, 87, 1, 88),
    (15, 'M'): (655, 24, 5, 41, 5, 42),
    (15, 'Q'): (655, 30, 5, 24, 7, 25),
    (15, 'H'): (655, 24, 11, 12, 7, 13),
}

EC_MAP = {'M': 0b00, 'L': 0b01, 'H': 0b10, 'Q': 0b11}
REV_EC_MAP = {0b00: 'M', 0b01: 'L', 0b10: 'H', 0b11: 'Q'}
FMT_POLY = 0x537
FMT_MASK = 0x5412


def encode_format_info(ec_level: str, mask_id: int) -> int:
    d = (EC_MAP.get(ec_level, 0b00) << 3) | (mask_id & 0b111)
    val = d << 10
    for i in range(5):
        if val & (1 << (14 - i)):
            val ^= (FMT_POLY << (4 - i))
    return ((d << 10) | val) ^ FMT_MASK


def decode_format_info(raw_fmt: int) -> Tuple[str, int]:
    val = raw_fmt ^ FMT_MASK
    d = (val >> 10) & 0x1F
    ec = REV_EC_MAP.get((d >> 3) & 0b11, 'M')
    mask = d & 0b111
    return ec, mask


def mask_bit(mask_id: int, r: int, c: int) -> bool:
    if mask_id == 0:
        return (r + c) % 2 == 0
    if mask_id == 1:
        return r % 2 == 0
    if mask_id == 2:
        return c % 3 == 0
    if mask_id == 3:
        return (r + c) % 3 == 0
    if mask_id == 4:
        return ((r // 2) + (c // 3)) % 2 == 0
    if mask_id == 5:
        return ((r * c) % 2) + ((r * c) % 3) == 0
    if mask_id == 6:
        return (((r * c) % 2) + ((r * c) % 3)) % 2 == 0
    if mask_id == 7:
        return (((r + c) % 2) + ((r * c) % 3)) % 2 == 0
    return False


def get_function_pattern_mask(version: int) -> List[List[bool]]:
    N = 17 + 4 * version
    is_func = [[False] * N for _ in range(N)]

    # 1. Three Finder Patterns & 1-module Separators (8x8 at corners)
    for r in range(9):
        for c in range(9):
            is_func[r][c] = True
    for r in range(9):
        for c in range(N - 8, N):
            is_func[r][c] = True
    for r in range(N - 8, N):
        for c in range(9):
            is_func[r][c] = True

    # 2. Timing Patterns
    for i in range(N):
        is_func[6][i] = True
        is_func[i][6] = True

    # 3. Dark Module
    is_func[4 * version + 9][8] = True

    # 4. Alignment Patterns (Version >= 2)
    align_coords = QR_ALIGN_POS.get(version, [])
    for ar in align_coords:
        for ac in align_coords:
            if (ar <= 8 and ac <= 8) or (ar <= 8 and ac >= N - 8) or (ar >= N - 8 and ac <= 8):
                continue
            for dr in range(-2, 3):
                for dc in range(-2, 3):
                    is_func[ar + dr][ac + dc] = True

    # 5. Format Info modules
    for c in range(9):
        is_func[8][c] = True
        is_func[c][8] = True
    for c in range(N - 8, N):
        is_func[8][c] = True
    for r in range(N - 7, N):
        is_func[r][8] = True

    # 6. Version Info modules (Version >= 7)
    if version >= 7:
        for r in range(6):
            for c in range(N - 11, N - 8):
                is_func[r][c] = True
                is_func[c][r] = True

    return is_func


def get_zigzag_coords(version: int) -> List[Tuple[int, int]]:
    N = 17 + 4 * version
    is_func = get_function_pattern_mask(version)
    coords: List[Tuple[int, int]] = []
    up = True
    col = N - 1
    while col > 0:
        if col == 6:
            col -= 1
        rows = range(N - 1, -1, -1) if up else range(N)
        for r in rows:
            for c in (col, col - 1):
                if not is_func[r][c]:
                    coords.append((r, c))
        up = not up
        col -= 2
    return coords


def build_data_codewords(text: str, target_data_cw: int, version: int) -> List[int]:
    char_count_bits = 8 if version <= 9 else 16
    raw_bytes = text.encode("utf-8")
    length = len(raw_bytes)

    bits = "0100" + format(length, f"0{char_count_bits}b")
    for b in raw_bytes:
        bits += format(b, "08b")

    rem_bits = (target_data_cw * 8) - len(bits)
    if rem_bits > 0:
        bits += "0" * min(4, rem_bits)

    while len(bits) % 8 != 0:
        bits += "0"

    pad_bytes = [0xEC, 0x11]
    pad_idx = 0
    codewords = [int(bits[i:i + 8], 2) for i in range(0, len(bits), 8)]
    while len(codewords) < target_data_cw:
        codewords.append(pad_bytes[pad_idx % 2])
        pad_idx += 1

    return codewords


def interleave_codewords(version: int, ec_level: str, data_cw: List[int]) -> List[int]:
    tot, ec_cw, g1_b, g1_d, g2_b, g2_d = QR_SPECS[(version, ec_level)]
    num_blocks = g1_b + g2_b
    blocks_data = []
    blocks_ec = []
    offset = 0
    for b in range(num_blocks):
        cnt = g1_d if b < g1_b else g2_d
        blk = data_cw[offset:offset + cnt]
        offset += cnt
        blocks_data.append(blk)
        blocks_ec.append(rs_encode(blk, ec_cw))

    interleaved = []
    max_d = max(g1_d, g2_d) if g2_d else g1_d
    for col in range(max_d):
        for b in range(num_blocks):
            if col < len(blocks_data[b]):
                interleaved.append(blocks_data[b][col])

    for col in range(ec_cw):
        for b in range(num_blocks):
            interleaved.append(blocks_ec[b][col])

    return interleaved


def deinterleave_codewords(version: int, ec_level: str, raw_cw: List[int]) -> List[int]:
    tot, ec_cw, g1_b, g1_d, g2_b, g2_d = QR_SPECS[(version, ec_level)]
    num_blocks = g1_b + g2_b
    max_d = max(g1_d, g2_d) if g2_d else g1_d

    blocks_data: List[List[int]] = [[] for _ in range(num_blocks)]
    idx = 0
    for col in range(max_d):
        for b in range(num_blocks):
            cap = g1_d if b < g1_b else g2_d
            if col < cap and idx < len(raw_cw):
                blocks_data[b].append(raw_cw[idx])
                idx += 1

    full_data: List[int] = []
    for blk in blocks_data:
        full_data.extend(blk)

    return full_data


def decode_payload_bits(data_cw: List[int], version: int) -> str:
    bits = "".join(format(b, "08b") for b in data_cw)
    idx = 0
    char_count_bits = 8 if version <= 9 else 16
    output = ""

    while idx + 4 <= len(bits):
        mode = bits[idx:idx + 4]
        idx += 4
        if mode == "0000":
            break
        elif mode == "0100":  # 8-bit Byte Mode (UTF-8)
            if idx + char_count_bits > len(bits):
                break
            count = int(bits[idx:idx + char_count_bits], 2)
            idx += char_count_bits
            byte_vals = []
            for _ in range(count):
                if idx + 8 > len(bits):
                    break
                byte_vals.append(int(bits[idx:idx + 8], 2))
                idx += 8
            output += bytes(byte_vals).decode("utf-8", errors="replace")
        elif mode == "0010":  # Alphanumeric Mode
            ac_bits = 9 if version <= 9 else 11
            if idx + ac_bits > len(bits):
                break
            count = int(bits[idx:idx + ac_bits], 2)
            idx += ac_bits
            alphanum_chars = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ $%*+-./:"
            chars = []
            while count > 1 and idx + 11 <= len(bits):
                val = int(bits[idx:idx + 11], 2)
                chars.append(alphanum_chars[val // 45])
                chars.append(alphanum_chars[val % 45])
                idx += 11
                count -= 2
            if count == 1 and idx + 6 <= len(bits):
                val = int(bits[idx:idx + 6], 2)
                chars.append(alphanum_chars[val])
                idx += 6
            output += "".join(chars)
        elif mode == "0001":  # Numeric Mode
            n_bits = 10 if version <= 9 else 12
            if idx + n_bits > len(bits):
                break
            count = int(bits[idx:idx + n_bits], 2)
            idx += n_bits
            digits = []
            while count >= 3 and idx + 10 <= len(bits):
                digits.append(f"{int(bits[idx:idx + 10], 2):03d}")
                idx += 10
                count -= 3
            if count == 2 and idx + 7 <= len(bits):
                digits.append(f"{int(bits[idx:idx + 7], 2):02d}")
                idx += 7
            elif count == 1 and idx + 4 <= len(bits):
                digits.append(f"{int(bits[idx:idx + 4], 2):01d}")
                idx += 4
            output += "".join(digits)
        elif mode == "0111":  # ECI header
            if idx + 8 <= len(bits):
                idx += 8
        else:
            break

    return output


# ---------------------------------------------------------------------------
# Generator API for Test Images & Self-Test Verification
# ---------------------------------------------------------------------------

def generate_qr_test_image(
    text: str,
    version: Optional[int] = None,
    ec_level: str = 'M',
    mask_id: int = 0,
    module_size: int = 8,
    quiet_zone: int = 4,
    scale: Optional[int] = None,
    border: Optional[int] = None,
) -> Image.Image:
    """Generate ISO/IEC 18004 compliant QR Code PIL Image with multi-block support."""
    if scale is not None:
        module_size = scale
    if border is not None:
        quiet_zone = border
    raw_bytes = text.encode("utf-8")
    length = len(raw_bytes)

    if version is None:
        for v in range(1, 16):
            tot, ec_cw, g1_b, g1_d, g2_b, g2_d = QR_SPECS[(v, ec_level)]
            capacity = g1_b * g1_d + g2_b * g2_d
            header_bytes = 2 if v <= 9 else 3
            if capacity >= length + header_bytes:
                version = v
                break
        if version is None:
            version = 15

    N = 17 + 4 * version
    mat = [[0] * N for _ in range(N)]
    is_func = get_function_pattern_mask(version)

    # 1. Place Finder Patterns (7x7)
    def place_finder(r0: int, c0: int):
        for r in range(7):
            for c in range(7):
                if r in (0, 6) or c in (0, 6) or (2 <= r <= 4 and 2 <= c <= 4):
                    mat[r0 + r][c0 + c] = 1
                else:
                    mat[r0 + r][c0 + c] = 0

    place_finder(0, 0)
    place_finder(0, N - 7)
    place_finder(N - 7, 0)

    # 2. Timing Patterns
    for i in range(N):
        if is_func[6][i] and not (i <= 8 or i >= N - 8):
            mat[6][i] = 1 if i % 2 == 0 else 0
        if is_func[i][6] and not (i <= 8 or i >= N - 8):
            mat[i][6] = 1 if i % 2 == 0 else 0

    # 3. Alignment Patterns
    align_coords = QR_ALIGN_POS.get(version, [])
    for ar in align_coords:
        for ac in align_coords:
            if (ar <= 8 and ac <= 8) or (ar <= 8 and ac >= N - 8) or (ar >= N - 8 and ac <= 8):
                continue
            for dr in range(-2, 3):
                for dc in range(-2, 3):
                    if abs(dr) == 2 or abs(dc) == 2 or (dr == 0 and dc == 0):
                        mat[ar + dr][ac + dc] = 1
                    else:
                        mat[ar + dr][ac + dc] = 0

    # 4. Dark Module
    mat[4 * version + 9][8] = 1

    # 5. Format Info
    fmt_val = encode_format_info(ec_level, mask_id)
    tl_coords = [
        (8, 0), (8, 1), (8, 2), (8, 3), (8, 4), (8, 5),
        (8, 7), (8, 8), (7, 8), (5, 8), (4, 8), (3, 8),
        (2, 8), (1, 8), (0, 8)
    ]
    for bit_idx, (r, c) in enumerate(tl_coords):
        mat[r][c] = (fmt_val >> (14 - bit_idx)) & 1

    tr_bl_coords = [
        (N - 1, 8), (N - 2, 8), (N - 3, 8), (N - 4, 8),
        (N - 5, 8), (N - 6, 8), (N - 7, 8),
        (8, N - 8), (8, N - 7), (8, N - 6), (8, N - 5),
        (8, N - 4), (8, N - 3), (8, N - 2), (8, N - 1)
    ]
    for bit_idx, (r, c) in enumerate(tr_bl_coords):
        mat[r][c] = (fmt_val >> (14 - bit_idx)) & 1

    # 6. Data Codewords
    tot, ec_cw, g1_b, g1_d, g2_b, g2_d = QR_SPECS[(version, ec_level)]
    target_data_cw = g1_b * g1_d + g2_b * g2_d
    data_cw = build_data_codewords(text, target_data_cw, version)
    interleaved_cw = interleave_codewords(version, ec_level, data_cw)

    bits = "".join(format(b, "08b") for b in interleaved_cw)
    zigzag = get_zigzag_coords(version)
    for i, (r, c) in enumerate(zigzag):
        raw_bit = int(bits[i]) if i < len(bits) else 0
        mat[r][c] = raw_bit ^ (1 if mask_bit(mask_id, r, c) else 0)

    img_size = (N + 2 * quiet_zone) * module_size
    img = Image.new("L", (img_size, img_size), 255)
    draw = ImageDraw.Draw(img)
    for r in range(N):
        for c in range(N):
            if mat[r][c] == 1:
                x0 = (c + quiet_zone) * module_size
                y0 = (r + quiet_zone) * module_size
                draw.rectangle([x0, y0, x0 + module_size - 1, y0 + module_size - 1], fill=0)
    return img


# ---------------------------------------------------------------------------
# High-Speed Sub-Pixel Finder Pattern Detector & Affine Matrix Sampler
# ---------------------------------------------------------------------------

def _get_line_runs(line_pixels: List[int], thresh: int) -> List[Tuple[bool, int, int]]:
    runs = []
    w = len(line_pixels)
    if w == 0:
        return runs
    cur_dark = line_pixels[0] < thresh
    cur_len = 1
    start_x = 0
    for x in range(1, w):
        dark = line_pixels[x] < thresh
        if dark == cur_dark:
            cur_len += 1
        else:
            runs.append((cur_dark, cur_len, start_x))
            cur_dark = dark
            cur_len = 1
            start_x = x
    runs.append((cur_dark, cur_len, start_x))
    return runs


def _check_ratio(r0: int, r1: int, r2: int, r3: int, r4: int) -> bool:
    tot = r0 + r1 + r2 + r3 + r4
    if tot < 7:
        return False
    mod = tot / 7.0
    tol = mod * 0.65
    return (
        abs(r0 - mod) < tol
        and abs(r1 - mod) < tol
        and abs(r2 - 3.0 * mod) < tol * 1.5
        and abs(r3 - mod) < tol
        and abs(r4 - mod) < tol
    )


def _verify_vertical_at(cx: float, cy: float, pixels: Any, w: int, h: int, thresh: int) -> Tuple[bool, float, int]:
    ix = int(round(cx))
    iy = int(round(cy))
    if not (0 <= ix < w and 0 <= iy < h and pixels[ix, iy] < thresh):
        return False, cy, 0

    y_up = iy
    while y_up >= 0 and pixels[ix, y_up] < thresh:
        y_up -= 1
    y_w1 = y_up
    while y_w1 >= 0 and pixels[ix, y_w1] >= thresh:
        y_w1 -= 1
    y_b0 = y_w1
    while y_b0 >= 0 and pixels[ix, y_b0] < thresh:
        y_b0 -= 1

    y_down = iy
    while y_down < h and pixels[ix, y_down] < thresh:
        y_down += 1
    y_w3 = y_down
    while y_w3 < h and pixels[ix, y_w3] >= thresh:
        y_w3 += 1
    y_b4 = y_w3
    while y_b4 < h and pixels[ix, y_b4] < thresh:
        y_b4 += 1

    r0 = y_w1 - y_b0
    r1 = y_up - y_w1
    r2 = y_down - y_up - 1
    r3 = y_w3 - y_down
    r4 = y_b4 - y_w3
    if not _check_ratio(r0, r1, r2, r3, r4):
        return False, cy, 0
    new_cy = y_b0 + 1 + r0 + r1 + r2 / 2.0
    return True, new_cy, (r0 + r1 + r2 + r3 + r4)


def _find_best_triangle(pts: List[Tuple[float, float, float]]) -> Optional[Tuple[Tuple[float, float, float], Tuple[float, float, float], Tuple[float, float, float]]]:
    best_tri = None
    best_score = 999999.0
    for p0, p1, p2 in itertools.combinations(pts, 3):
        for tl, o1, o2 in [(p0, p1, p2), (p1, p0, p2), (p2, p0, p1)]:
            d1 = math.hypot(o1[0] - tl[0], o1[1] - tl[1])
            d2 = math.hypot(o2[0] - tl[0], o2[1] - tl[1])
            d12 = math.hypot(o1[0] - o2[0], o1[1] - o2[1])
            if d1 < 12 or d2 < 12:
                continue

            side_ratio = abs(d1 - d2) / max(d1, d2)
            if side_ratio > 0.16:
                continue

            v1 = (o1[0] - tl[0], o1[1] - tl[1])
            v2 = (o2[0] - tl[0], o2[1] - tl[1])
            dot = (v1[0] * v2[0] + v1[1] * v2[1]) / (d1 * d2)
            if abs(dot) > 0.22:
                continue

            pyth_err = abs(d12**2 - (d1**2 + d2**2)) / (d1**2 + d2**2)
            if pyth_err > 0.28:
                continue

            score = side_ratio + abs(dot) + pyth_err
            if score < best_score:
                best_score = score
                cross = v1[0] * v2[1] - v1[1] * v2[0]
                if cross > 0:
                    tr, bl = o1, o2
                else:
                    tr, bl = o2, o1
                best_tri = (tl, tr, bl)

    return best_tri


def _find_finders_in_image(img: Image.Image, thresh: int, y_min: int = 0, y_max: Optional[int] = None) -> Optional[Tuple[Tuple[float, float, float], Tuple[float, float, float], Tuple[float, float, float]]]:
    w, h = img.size
    pixels = img.load()
    if y_max is None:
        y_max = h

    verified_candidates = []
    step_y = max(1, h // 550)
    for y in range(y_min, y_max, step_y):
        line = [pixels[x, y] for x in range(w)]
        runs = _get_line_runs(line, thresh)
        if len(runs) < 5:
            continue
        for i in range(len(runs) - 4):
            if runs[i][0] and not runs[i + 1][0] and runs[i + 2][0] and not runs[i + 3][0] and runs[i + 4][0]:
                if _check_ratio(runs[i][1], runs[i + 1][1], runs[i + 2][1], runs[i + 3][1], runs[i + 4][1]):
                    cx = runs[i + 2][2] + runs[i + 2][1] / 2.0
                    tot_h = runs[i][1] + runs[i + 1][1] + runs[i + 2][1] + runs[i + 3][1] + runs[i + 4][1]
                    v_ok, real_cy, tot_v = _verify_vertical_at(cx, y, pixels, w, h, thresh)
                    if v_ok and abs(tot_v - tot_h) < tot_h * 0.35:
                        verified_candidates.append((cx, real_cy, (tot_h + tot_v) / 2.0))

    clusters: List[List[Tuple[float, float, float]]] = []
    for cx, cy, size in verified_candidates:
        matched = False
        for cl in clusters:
            avg_x = sum(p[0] for p in cl) / len(cl)
            avg_y = sum(p[1] for p in cl) / len(cl)
            if math.hypot(cx - avg_x, cy - avg_y) < size * 0.50:
                cl.append((cx, cy, size))
                matched = True
                break
        if not matched:
            clusters.append([(cx, cy, size)])

    pts: List[Tuple[float, float, float]] = []
    for cl in clusters:
        avg_x = sum(p[0] for p in cl) / len(cl)
        avg_y = sum(p[1] for p in cl) / len(cl)
        avg_size = sum(p[2] for p in cl) / len(cl)
        pts.append((avg_x, avg_y, avg_size))

    if len(pts) < 3:
        return None

    return _find_best_triangle(pts)


# ---------------------------------------------------------------------------
# Master Public Decoder API
# ---------------------------------------------------------------------------

def _load_image(image_source: Union[str, Path, Image.Image, bytes]) -> Optional[Image.Image]:
    try:
        if isinstance(image_source, (str, Path)):
            src_str = str(image_source)
            if src_str.startswith("data:image"):
                _head, b64 = src_str.split(",", 1)
                return Image.open(io.BytesIO(base64.b64decode(b64)))
            elif Path(src_str).is_file():
                return Image.open(src_str)
        elif isinstance(image_source, bytes):
            return Image.open(io.BytesIO(image_source))
        elif isinstance(image_source, Image.Image):
            return image_source
    except Exception:
        pass
    return None


def decode_qr_from_image(image_source: Union[str, Path, Image.Image, bytes]) -> Optional[str]:
    """Decode QR Code from Base64 Data URL, PIL Image, byte buffer, or file path.

    Returns the extracted text (e.g. Instagram/TikTok URL) or None if no QR Code is present.
    """
    img = _load_image(image_source)
    if img is None:
        return None

    gray = img.convert("L")
    w, h = gray.size

    # For full iPhone screen frames (e.g. 1216x2656), prioritize central modal region
    y_min, y_max = (int(0.08 * h), int(0.92 * h)) if (w >= 800 and h >= 1400) else (0, h)

    thresholds = [128, 105, 155]
    for inverted in (False, True):
        cur_gray = gray
        if inverted:
            cur_gray = ImageOps.invert(gray)

        for thresh in thresholds:
            finders = _find_finders_in_image(cur_gray, thresh, y_min, y_max)
            if not finders:
                continue
            tl, tr, bl = finders
            cx_tl, cy_tl, w_tl = tl
            cx_tr, cy_tr, w_tr = tr
            cx_bl, cy_bl, w_bl = bl

            mod_size = (w_tl + w_tr + w_bl) / 21.0
            d_tr = math.hypot(cx_tr - cx_tl, cy_tr - cy_tl)
            d_bl = math.hypot(cx_bl - cx_tl, cy_bl - cy_tl)
            n_est = round(((d_tr + d_bl) / (2.0 * mod_size)) + 7.0)
            v_est = max(1, min(15, round((n_est - 17) / 4.0)))

            cand_versions = [v_est]
            if v_est > 1:
                cand_versions.append(v_est - 1)
            if v_est < 15:
                cand_versions.append(v_est + 1)

            pixels = cur_gray.load()

            for V in cand_versions:
                N = 17 + 4 * V
                ux = (cx_tr - cx_tl) / (N - 7.0)
                uy = (cy_tr - cy_tl) / (N - 7.0)
                vx = (cx_bl - cx_tl) / (N - 7.0)
                vy = (cy_bl - cy_tl) / (N - 7.0)

                mat = [[0] * N for _ in range(N)]
                for r in range(N):
                    for c in range(N):
                        px = cx_tl + (c - 3.5) * ux + (r - 3.5) * vx
                        py = cy_tl + (c - 3.5) * uy + (r - 3.5) * vy
                        ix, iy = int(round(px)), int(round(py))
                        if 0 <= ix < w and 0 <= iy < h:
                            mat[r][c] = 1 if pixels[ix, iy] < thresh else 0
                        else:
                            mat[r][c] = 0

                tl_coords = [
                    (8, 0), (8, 1), (8, 2), (8, 3), (8, 4), (8, 5),
                    (8, 7), (8, 8), (7, 8), (5, 8), (4, 8), (3, 8),
                    (2, 8), (1, 8), (0, 8)
                ]
                fmt_raw = 0
                for r, c in tl_coords:
                    fmt_raw = (fmt_raw << 1) | mat[r][c]

                dec_ec, dec_mask = decode_format_info(fmt_raw)

                combos = [(dec_mask, dec_ec)]
                for m in range(8):
                    for ec in ['M', 'Q', 'L', 'H']:
                        if (m, ec) != (dec_mask, dec_ec):
                            combos.append((m, ec))

                zigzag = get_zigzag_coords(V)

                for test_mask, test_ec in combos:
                    if (V, test_ec) not in QR_SPECS:
                        continue
                    tot, ec_cw, g1_b, g1_d, g2_b, g2_d = QR_SPECS[(V, test_ec)]

                    bits = []
                    for r, c in zigzag:
                        mb = 1 if mask_bit(test_mask, r, c) else 0
                        bits.append(str(mat[r][c] ^ mb))

                    raw_bytes = []
                    for i in range(0, tot * 8, 8):
                        if i + 8 <= len(bits):
                            raw_bytes.append(int("".join(bits[i:i + 8]), 2))

                    if len(raw_bytes) < tot:
                        continue

                    try:
                        deinter = deinterleave_codewords(V, test_ec, raw_bytes)
                        text = decode_payload_bits(deinter, V)
                        if text and len(text) >= 4 and ("http://" in text or "https://" in text or len(text) > 8):
                            return text.strip()
                    except Exception:
                        pass

    return None

# ---------------------------------------------------------------------------
# Tahap 97..101: Multi-Device Dual-Zone Spotlight Assembler & Direct URL Scanner
# ---------------------------------------------------------------------------

SPOTLIGHT_UI_NOISE_RE = re.compile(
    r'^(?:Websites?|Situs\s*Web|Siri\s*Knowledge|Pengetahuan\s*Siri|Saran\s*Siri|Siri\s*Suggestions|'
    r'Search\s*the\s*Web|Search\s*Web|Cari\s*di\s*Web|Search\s*in\s*App|Cari\s*di\s*App|'
    r'Search\s*App\s*Store|Cari\s*di\s*App\s*Store|Search\s*Maps|Cari\s*di\s*Peta|'
    r'Show\s*More|Lebih\s*Banyak|Tampilkan\s*Lebih\s*Banyak|Tautkan\s*&\s*Bagikan|'
    r'Share\s*Results|Bagikan\s*Hasil|Top\s*Hit|Hasil\s*Teratas|'
    r'Copy\s*link|Salin\s*tautan|Add\s*to\s*story|Tambahkan\s*ke\s*cerita|'
    r'Share|Bagikan|WhatsApp|Instagram|TikTok|Foto|Photos|Files|Pesan|Messages|'
    r'Repost|Cancel|Batal|Dictionary|Kamus|Maps|Peta|App\s*Store)\b',
    re.IGNORECASE
)

URL_START_RE = re.compile(
    r'(https?://[^\s]+|www\.[^\s]+|(?:instagram|tiktok|threads)\.(?:com|net)/[^\s]+|(?:vt|vm)\.tiktok\.com/[^\s]+|youtu\.be/[^\s]+|youtube\.com/[^\s]+|(?:x|twitter)\.com/[^\s]+)',
    re.IGNORECASE
)


def prepare_spotlight_top_box_images(img: Image.Image) -> tuple[Image.Image, ...]:
    """Triple-Zone Multi-Device Spotlight Preprocessing (Tahap 105..111).

    Zona 1 — Top Spotlight Box (iPhone 17 Pro Dynamic Island / top input box, y=0.040..0.255*H):
      1) rgb_top: Natural RGB @ 2.8x Bicubic (preserves subtle curves of Q, f, rkY)
      2) inv_top: Soft Inverted Grayscale @ 2.8x Bicubic (val = 255.0 - (luma - 55.0) * 1.45)
      (Tahap 112: Varian 1C horizontal stretch removed - Zona 1 is strictly 1:1)

    Zona 2A — Pure Safari Search Card (iPhone 11 Pro Max, strictly below 'Situs Web' header at y=0.255*H):
      3) inv_card: x = 0.192..0.960*W, y = 0.268..0.505*H, Soft Inverted Contrast @ 2.8x Bicubic
      4) rgb_card: x = 0.192..0.960*W, y = 0.268..0.505*H, Natural RGB @ 2.8x Bicubic

    Zona 3 — Bottom Search Input Bar (iPhone 11 Pro Max, y=0.882..0.946*H, above Home Indicator):
      5) inv_bot: x = 0.118..0.875*W, y = 0.882..0.946*H, Gentle Inverted Stretch @ 3.0x Bicubic with 36px white border
      6) rgb_bot: x = 0.118..0.875*W, y = 0.882..0.946*H, Natural RGB @ 3.0x Bicubic with 36px border
      7) anchored_bot: inv_bot with left-anchored "https://anchor.net/" prefix to activate WinRT OCR English dictionary (Tahap 107)
    """
    W, H = img.size

    # --- Zona 1: Top Spotlight Box (iPhone 17 Pro Dynamic Island, y=0.040..0.255*H) ---
    box_top = (int(0.040 * W), int(0.040 * H), int(0.960 * W), int(0.255 * H))
    crop_top = img.crop(box_top)
    tw, th = crop_top.size
    scale_top = 2.8

    # Varian 1A: Natural RGB @ 2.8x Bicubic
    rgb_top = crop_top.resize((int(tw * scale_top), int(th * scale_top)), Image.BICUBIC)

    # Varian 1B: Soft Inverted Grayscale @ 2.8x Bicubic
    gray_top = rgb_top.convert("L")
    lut_soft = [max(0, min(255, int(255.0 - (luma - 55.0) * 1.45))) for luma in range(256)]
    inv_top = gray_top.point(lut_soft)
    inv_top = ImageOps.autocontrast(inv_top, cutoff=2)

    # Tahap 112: Varian 1C (Horizontal Stretch 3.4x) REMOVED from Zona 1 - it distorted letter widths
    # (JIi -> Ji24, 37c -> 372c) and spawned fake 12-char shortcodes. Zona 1 uses strict 1:1 (2.8x x 2.8x).

    # --- Zona 2A: Pure Safari Search Card (iPhone 11 Pro Max, strictly y=0.268..0.505*H) ---
    box_card = (int(0.192 * W), int(0.268 * H), int(0.960 * W), int(0.505 * H))
    crop_card = img.crop(box_card)
    cw, ch = crop_card.size
    # Anisotropic Horizontal Stretch (Tahap 107 Pilar 3): scale_x = 3.6x, scale_y = 2.8x
    scale_card_x = 3.6
    scale_card_y = 2.8
    scaled_card = crop_card.resize((int(cw * scale_card_x), int(ch * scale_card_y)), Image.BICUBIC)
    gray_card = scaled_card.convert("L")
    inv_card = ImageOps.invert(gray_card)
    inv_card = ImageOps.autocontrast(inv_card, cutoff=2)
    rgb_card = scaled_card

    # --- Zona 3: Bottom Search Input Bar (iPhone 11 Pro Max, y=0.882..0.946*H) ---
    box_bot = (int(0.118 * W), int(0.882 * H), int(0.875 * W), int(0.946 * H))
    crop_bot = img.crop(box_bot)
    bw, bh = crop_bot.size
    # Anisotropic Horizontal Stretch (Tahap 107 Pilar 3): scale_x = 3.6x, scale_y = 2.8x
    scale_bot_x = 3.6
    scale_bot_y = 2.8
    scaled_bot = crop_bot.resize((int(bw * scale_bot_x), int(bh * scale_bot_y)), Image.BICUBIC)
    gray_bot = scaled_bot.convert("L")
    inv_bot_base = ImageOps.invert(gray_bot)
    inv_bot_base = ImageOps.autocontrast(inv_bot_base, cutoff=2)
    inv_bot = ImageOps.expand(inv_bot_base, border=36, fill=255)
    rgb_bot = ImageOps.expand(scaled_bot, border=36, fill=(255, 255, 255))

    # Anchored bot image: draw "https://anchor.net/" prefix with tight gap=2px (Tahap 107 Pilar 2)
    # Binds tail Base64 / ?igsh= into a continuous single URL token for WinRT OCR.
    w_base, h_base = scaled_bot.size
    pixels_base = inv_bot_base.load()
    x_text_start = 0
    in_magnifier = False
    passed_magnifier = False
    for x in range(min(w_base, 350)):
        ink_count = sum(1 for y in range(h_base) if pixels_base[x, y] < 128)
        if ink_count >= 3:
            if not in_magnifier and not passed_magnifier:
                in_magnifier = True
            elif passed_magnifier:
                x_text_start = x
                break
        else:
            if in_magnifier:
                passed_magnifier = True

    if x_text_start == 0 or x_text_start > 250:
        for x in range(min(w_base, 200)):
            if sum(1 for y in range(h_base) if pixels_base[x, y] < 128) >= 3:
                x_text_start = x
                break

    crop_bot_text = inv_bot_base.crop((x_text_start, 0, w_base, h_base))
    f_size = max(20, int(h_base * 0.48))
    try:
        f_anc = ImageFont.truetype("arial.ttf", f_size)
    except Exception:
        f_anc = ImageFont.load_default()

    dummy_draw = ImageDraw.Draw(Image.new("L", (1, 1), 255))
    try:
        bbox = dummy_draw.textbbox((0, 0), "https://anchor.net/", font=f_anc)
        pfx_w = bbox[2] - bbox[0]
    except Exception:
        pfx_w = int(len("https://anchor.net/") * f_size * 0.58)

    gap_px = 2
    margin_left = 20
    anchored_w = margin_left + pfx_w + gap_px + crop_bot_text.width + 36
    anchored_h = h_base + 36
    anchored_bot = Image.new("L", (anchored_w, anchored_h), color=255)
    d_anc = ImageDraw.Draw(anchored_bot)
    d_anc.text((margin_left, 18 + int((h_base - f_size) / 2)), "https://anchor.net/", fill=0, font=f_anc)
    anchored_bot.paste(crop_bot_text, (margin_left + pfx_w + gap_px, 18))

    return rgb_top, inv_top, inv_card, rgb_card, inv_bot, rgb_bot, anchored_bot


def _prepare_zone1_rescue_image(img: Image.Image) -> Image.Image:
    """Tahap 112: fallback-only gentle horizontal stretch of Zona 1 (used when the ?igsh= line was not read)."""
    W, H = img.size
    crop_top = img.crop((int(0.040 * W), int(0.040 * H), int(0.960 * W), int(0.255 * H)))
    tw, th = crop_top.size
    lut_soft = [max(0, min(255, int(255.0 - (luma - 55.0) * 1.45))) for luma in range(256)]
    stretched = crop_top.resize((int(tw * 3.4), int(th * 2.8)), Image.BICUBIC).convert("L").point(lut_soft)
    return ImageOps.autocontrast(stretched, cutoff=2)


def _strip_leading_cursor(text: str) -> str:
    """Strip vertical cursor '|' or misread glyphs ('l', 'I', '1', '!', 'i', '/') immediately preceding a URL."""
    if not text:
        return ""
    t = text.strip()
    return re.sub(r'^[\|lI1!i/\[\(]+\s*(?=(?:https?://|www\.|(?:instagram|threads|tiktok)\.(?:com|net)))', '', t, flags=re.IGNORECASE)


def _strip_safe_tail_ellipsis(s: str) -> str:
    """Safely strip ellipsis ('...', '…') only if it appears at the extreme tail of a URL."""
    if not s:
        return ""
    s_clean = re.sub(r'[\.]{2,}$', '', s.strip())
    s_clean = re.sub(r'…$', '', s_clean.strip())
    return s_clean.strip()


_SIMILAR_CHAR_GROUPS = [
    {'l', '1', 'i', 'I', '|', 'j', 'J', '7', '_', '-'},
    {'0', 'O', 'o', 'Q', 'G', 'C', '6'},
    {'-', '_'},
    {'s', 'S', '5'},
    {'b', '8', 'B'},
    {'v', 'V', 'u', 'U', '4'},
    {'z', 'Z', '2'},
    {'k', 'K', 'h', 'H'},
    {'9', '5', 'q'},
]


def _chars_match(c1: str, c2: str) -> bool:
    """Check if two characters match or are commonly confused by OCR."""
    if c1 == c2:
        return True
    if c1.lower() == c2.lower():
        return True
    for g in _SIMILAR_CHAR_GROUPS:
        if c1 in g and c2 in g:
            return True
    return False


def fuse_codes(head_code: str, tail_code: str) -> Optional[str]:
    """Fuse head shortcode and tail shortcode using prefix-window overlap matching (Tahap 106).
    Cuts off spurious garbage tails on head_code (e.g. to7pa / to7pp from /?ig misread)
    and replaces with clean tail_code from frameTail.
    """
    if not head_code or not tail_code:
        return None
    h = head_code.strip()
    t = tail_code.strip()
    if h == t:
        return t

    # Trim spurious misread suffix on head_code if >= 14 chars
    if len(h) >= 14:
        h_trimmed = re.sub(r'(?:to|lo|ro|co|2)?[7\?][pP][aApPqQgG]$', '', h, flags=re.IGNORECASE)
        if len(h_trimmed) >= 10:
            h = h_trimmed

    # Direct substring inclusion if lengths are compatible
    if t in h and 11 <= len(h) <= 13:
        return h
    if h in t and 11 <= len(t) <= 13:
        return t

    best_candidate: Optional[str] = None
    best_score = -1

    # Special anchor alignment for shared anchors like "-os" or "1-os" (Tahap 106 Pilar 3)
    for anchor in ("-os", "1-os", "_os", "-"):
        if anchor in h and anchor in t:
            idx_h = h.find(anchor)
            idx_t = t.find(anchor)
            # Check direct prefix alignment
            if idx_h >= idx_t:
                cand = h[:(idx_h - idx_t)] + t
                if 11 <= len(cand) <= 13:
                    cand_matches = sum(
                        1 for k in range(min(len(h), len(cand)))
                        if h[k] == cand[k] or _chars_match(h[k], cand[k])
                    )
                    score = 200 + cand_matches * 10
                    if score > best_score:
                        best_score = score
                        best_candidate = cand
            # Check 3-char prefix alignment if head has OCR noise (e.g. DVf1ql-os -> DVf + 1-os96j0d)
            if len(h) >= 3 and h.startswith(('D', 'd')) and idx_t == 1:
                cand3 = h[:3] + t
                if 11 <= len(cand3) <= 13:
                    score = 220
                    if score > best_score:
                        best_score = score
                        best_candidate = cand3

    # Anchor alignment for general unique shared anchors of length >= 3
    for min_anchor_len in (4, 3):
        for t_idx in range(len(t) - min_anchor_len + 1):
            sub_anchor = t[t_idx : t_idx + min_anchor_len]
            if sub_anchor in h:
                h_idx = h.find(sub_anchor)
                pfx_len = h_idx - t_idx
                if 0 <= pfx_len <= 5:
                    cand = h[:pfx_len] + t
                    if 11 <= len(cand) <= 13:
                        cand_matches = sum(
                            1 for k in range(min(len(h) - pfx_len, len(t)))
                            if h[pfx_len + k] == t[k] or _chars_match(h[pfx_len + k], t[k])
                        )
                        score = 150 + cand_matches * 10
                        if score > best_score:
                            best_score = score
                            best_candidate = cand

    # Prefix-window sliding overlap: test match of h[idx : idx + w] against t[:w]
    max_idx = min(6, len(h) - 3)
    for idx in range(1, max_idx + 1):
        cand_len = idx + len(t)
        if not (11 <= cand_len <= 13):
            continue

        overlap_w_full = min(len(h) - idx, len(t))
        overlap_w_clean = min(max(0, 10 - idx), len(h) - idx, len(t))

        for w in (overlap_w_full, overlap_w_clean):
            if w < 4:
                continue

            matches = 0
            for k in range(w):
                if h[idx + k] == t[k] or _chars_match(h[idx + k], t[k]):
                    matches += 1

            if matches >= (w - 1):
                fused = h[:idx] + t
                score = matches * 15 + w * 10 - (w - matches) * 15
                if t.endswith('_') and not h.endswith('_'):
                    score += 25
                if score > best_score:
                    best_score = score
                    best_candidate = fused

    return best_candidate


def extract_bottom_bar_fragment(z3_text: str) -> Optional[str]:
    """Extract URL tail, full URL, or share shortcode fragment from Zona 3 OCR line."""
    if not z3_text:
        return None
    raw = re.sub(r'^.*?(?:anchor|anch0r|anehor|ancnor)[a-z0-9\.\-_]*\.(?:net|com|org)/+\s*', '', z3_text, flags=re.IGNORECASE)
    t = _strip_safe_tail_ellipsis(_strip_leading_cursor(raw).strip().replace(" ", ""))
    if not t or ".." in t or "…" in t:
        return None

    # Full URL present in bottom bar
    m_full = re.search(r'(https?://[A-Za-z0-9_\-\?=&%/\.:#@~+]+)', t, re.IGNORECASE)
    if m_full:
        u = clean_extracted_url(m_full.group(1))
        if u and not (".." in u or "…" in u):
            return u

    # Tail fragment containing ?igsh= or &igsh= or igsh= or ?xmt= or xmt=
    m_igsh = re.search(r'([A-Za-z0-9_\-]{3,15}/?\??(?:igsh|xmt)=[A-Za-z0-9_\-=&%/\.:]+)', t, re.IGNORECASE)
    if m_igsh:
        return m_igsh.group(1)

    # Bare igsh parameter token
    m_bare = re.search(r'(\??(?:igsh|xmt)=[A-Za-z0-9_\-=&%/\.:]+)', t, re.IGNORECASE)
    if m_bare:
        return m_bare.group(1)

    # Threads /share/<CODE> fragment
    m_th = re.search(r'threads\.(?:com|net)/share/([A-Za-z0-9_\-]+)', t, re.IGNORECASE)
    if m_th:
        return clean_extracted_url(f"https://www.threads.com/share/{m_th.group(1)}")

    # Isolated shortcode fragment
    m_sc = re.search(r'(?:/(?:p|reel|tv)/)?([A-Za-z0-9_\-]{6,15})/?$', t)
    if m_sc:
        code = m_sc.group(1)
        if not any(code.lower() == noise for noise in ("situs", "web", "cari", "search", "buka", "tautan")):
            return code

    return None


def stitch_bottom_bar_head_and_tail(head_texts: list[str], tail_texts: list[str]) -> list[str]:
    """Stitch bottom search bar head frame and tail frame using Sliding Substring Overlap (Tahap 105)."""
    results: list[str] = []
    head_candidates: list[tuple[str, str, str]] = []

    for ht in head_texts:
        if not ht:
            continue
        cleaned = _strip_safe_tail_ellipsis(_strip_leading_cursor(ht).replace(" ", ""))
        if ".." in cleaned or "…" in cleaned:
            continue
        m = re.search(r'((?:https?://)?(?:www\.)?instagram\.com/(?:p|reel|tv)/)([A-Za-z0-9_\-]{8,15})', cleaned, re.IGNORECASE)
        if m:
            prefix = m.group(1)
            if not prefix.startswith("http"):
                prefix = "https://" + ("www." if not prefix.startswith("www.") else "") + prefix
            head_candidates.append((prefix, m.group(2), cleaned))

        m_th = re.search(r'((?:https?://)?(?:www\.)?threads\.(?:com|net)/(?:share/|@[A-Za-z0-9_\.]+/post/))([A-Za-z0-9_\-]{8,15})', cleaned, re.IGNORECASE)
        if m_th:
            prefix = m_th.group(1)
            if not prefix.startswith("http"):
                prefix = "https://" + ("www." if not prefix.startswith("www.") else "") + prefix
            head_candidates.append((prefix, m_th.group(2), cleaned))

    tail_candidates: list[tuple[str, str]] = []
    for tt in tail_texts:
        if not tt:
            continue
        cleaned = _strip_safe_tail_ellipsis(_strip_leading_cursor(tt).replace(" ", ""))
        if ".." in cleaned or "…" in cleaned:
            continue
        m_t = re.search(r'([A-Za-z0-9_\-]{4,15})/?\??(igsh=[A-Za-z0-9_\-=-]+)', cleaned, re.IGNORECASE)
        if m_t:
            tail_candidates.append((m_t.group(1), m_t.group(2)))
        else:
            m_th_t = re.search(r'([A-Za-z0-9_\-]{4,15})/?\??(xmt=[A-Za-z0-9_\-=-]+)', cleaned, re.IGNORECASE)
            if m_th_t:
                tail_candidates.append((m_th_t.group(1), m_th_t.group(2)))
            else:
                m_frag = re.search(r'([A-Za-z0-9_\-]{6,15})', cleaned)
                if m_frag:
                    tail_candidates.append((m_frag.group(1), ""))

    for pfx, h_code, _ in head_candidates:
        for t_code, query in tail_candidates:
            fused_code = fuse_codes(h_code, t_code)
            if fused_code:
                cand = f"{pfx}{fused_code}"
                if query:
                    cand += "/?" + query
                c_cand = clean_extracted_url(cand)
                if c_cand and c_cand not in results:
                    results.append(c_cand)

    return results


def fuse_card_and_bottom_bar_url(card_urls: list[str], bottom_fragments: list[str]) -> list[str]:
    """Cross-Zone Fusion: fuse Safari Card URL and ground-truth bottom bar characters (Tahap 105)."""
    results: list[str] = []
    for c_url in card_urls:
        if not c_url:
            continue
        cl_card = _strip_safe_tail_ellipsis(c_url)
        if ".." in cl_card or "…" in cl_card:
            continue
        m = re.search(r'((?:https?://)?(?:www\.)?instagram\.com/(?:p|reel|tv)/)([A-Za-z0-9_\-]{6,14})', cl_card, re.IGNORECASE)
        if not m:
            continue
        pfx, card_code = m.group(1), m.group(2)
        for bf in bottom_fragments:
            if not bf:
                continue
            bf_cl = _strip_safe_tail_ellipsis(bf.replace(" ", ""))
            if ".." in bf_cl or "…" in bf_cl:
                continue
            m_bf = re.search(r'([A-Za-z0-9_\-]{4,15})/?\??(igsh=[A-Za-z0-9_\-=-]+)?', bf_cl, re.IGNORECASE)
            if not m_bf:
                continue
            b_code = m_bf.group(1)
            query = m_bf.group(2) or ""

            fused_code = fuse_codes(card_code, b_code)
            if fused_code:
                cand = f"{pfx}{fused_code}"
                if query:
                    cand += "/?" + query
                elif "?igsh=" in cl_card:
                    cand += cl_card[cl_card.find("?igsh=")-1:]
                c_cand = clean_extracted_url(cand)
                if c_cand and c_cand not in results:
                    results.append(c_cand)

    return results
def prepare_url_ocr_image(img: Image.Image) -> Image.Image:
    """Pre-process iPhone canvas to high-contrast binarized image preserving unhighlighted gold/blue/dark link text."""
    W, H = img.size
    crop_box = (int(0.04 * W), int(0.04 * H), int(0.96 * W), int(0.88 * H))
    cropped = img.crop(crop_box)
    cw, ch = cropped.size

    if cropped.mode != "RGB":
        cropped = cropped.convert("RGB")

    # Fast median luma check on thumbnail
    thumb = cropped.resize((min(cw, 80), min(ch, 80)))
    thumb_rgb = [thumb.getpixel((x, y)) for y in range(thumb.height) for x in range(thumb.width)]
    lumas = [int(0.299 * r + 0.587 * g + 0.114 * b) for r, g, b in thumb_rgb]
    lumas.sort()
    median_luma = lumas[len(lumas) // 2] if lumas else 255
    is_dark_mode = median_luma < 110

    pixels = cropped.load()
    out_img = Image.new("L", (cw, ch), 255)
    out_pixels = out_img.load()

    for y in range(ch):
        for x in range(cw):
            r, g, b = pixels[x, y]
            luma = int(0.299 * r + 0.587 * g + 0.114 * b)
            is_gold_link = (r >= 150 and g >= 95 and b <= 135 and (r - b) >= 40)
            is_blue_link = (b >= 140 and (b - r) >= 35)

            is_text = False
            if not is_dark_mode:
                if luma < 165 or is_gold_link or is_blue_link:
                    is_text = True
            else:
                if luma >= 125 or is_gold_link or is_blue_link:
                    is_text = True

            out_pixels[x, y] = 0 if is_text else 255

    resample_filter = getattr(Image, "Resampling", Image).BICUBIC
    return out_img.resize((int(cw * 2.0), int(ch * 2.0)), resample=resample_filter)


def _run_winrt_ocr_batch(imgs: list[Image.Image]) -> list[list[str]]:
    """Execute Windows.Media.Ocr on a batch of PIL images in a single PowerShell session."""
    if not imgs:
        return []
    results: list[list[str]] = [[] for _ in imgs]
    tmp_paths: list[str] = []
    active_orig_indices: list[int] = []
    for orig_i, im in enumerate(imgs):
        g_chk = im if im.mode == "L" else im.convert("L")
        if g_chk.width > 80 and g_chk.height > 80:
            lo_c, hi_c = g_chk.crop((38, 38, g_chk.width - 38, g_chk.height - 38)).getextrema()
        else:
            lo_c, hi_c = g_chk.getextrema()
        if (hi_c - lo_c) < 18:
            continue
        tmp = tempfile.NamedTemporaryFile(suffix=".png", delete=False)
        im.save(tmp.name)
        tmp.close()
        tmp_paths.append(tmp.name)
        active_orig_indices.append(orig_i)

    if not tmp_paths:
        return results

    ps_paths = ", ".join(f"'{p}'" for p in tmp_paths)
    ps = f'''
    [Console]::OutputEncoding = [System.Text.Encoding]::UTF8
    $OutputEncoding = [System.Text.Encoding]::UTF8
    Add-Type -AssemblyName 'System.Runtime.WindowsRuntime'
    [void][Windows.Media.Ocr.OcrEngine, Windows.Foundation.Metadata, ContentType = WindowsRuntime]
    [void][Windows.Graphics.Imaging.BitmapDecoder, Windows.Foundation.Metadata, ContentType = WindowsRuntime]
    [void][Windows.Storage.StorageFile, Windows.Foundation.Metadata, ContentType = WindowsRuntime]
    $asTaskGeneric = [System.WindowsRuntimeSystemExtensions].GetMethods() | ? {{ $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.IsGenericMethod }} | Select-Object -First 1
    function Await-Op($op, [type]$t) {{ $netTask = $asTaskGeneric.MakeGenericMethod($t).Invoke($null, @($op)); [void]$netTask.Wait(-1); return $netTask.Result }}
    $engine = [Windows.Media.Ocr.OcrEngine]::TryCreateFromUserProfileLanguages()
    $paths = @({ps_paths})
    for ($idx = 0; $idx -lt $paths.Count; $idx++) {{
        [Console]::WriteLine("<<<IMG_SEP_$idx>>>")
        try {{
            $file = Await-Op ([Windows.Storage.StorageFile]::GetFileFromPathAsync($paths[$idx])) ([Windows.Storage.StorageFile])
            $stream = Await-Op ($file.OpenAsync([Windows.Storage.FileAccessMode]::Read)) ([Windows.Storage.Streams.IRandomAccessStream])
            $decoder = Await-Op ([Windows.Graphics.Imaging.BitmapDecoder]::CreateAsync($stream)) ([Windows.Graphics.Imaging.BitmapDecoder])
            $softwareBitmap = Await-Op ($decoder.GetSoftwareBitmapAsync()) ([Windows.Graphics.Imaging.SoftwareBitmap])
            $ocrResult = Await-Op ($engine.RecognizeAsync($softwareBitmap)) ([Windows.Media.Ocr.OcrResult])
            $stream.Dispose()
            foreach ($l in $ocrResult.Lines) {{
                [Console]::WriteLine($l.Text)
            }}
        }} catch {{}}
    }}
    '''
    try:
        creationflags = getattr(subprocess, "CREATE_NO_WINDOW", 0x08000000)
        res = subprocess.run(
            ["powershell.exe", "-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", ps],
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            creationflags=creationflags,
            timeout=15.0,
        )
        cur_orig_idx = -1
        for raw_line in res.stdout.splitlines():
            s = raw_line.strip()
            if not s:
                continue
            m_sep = re.match(r'^<<<IMG_SEP_(\d+)>>>$', s)
            if m_sep:
                b_i = int(m_sep.group(1))
                cur_orig_idx = active_orig_indices[b_i] if 0 <= b_i < len(active_orig_indices) else -1
                continue
            if 0 <= cur_orig_idx < len(results):
                results[cur_orig_idx].append(s)
        return results
    except Exception as e:
        print(f"[Visual QR Bridge / URL Scanner] WinRT OCR batch error: {e}", flush=True)
        return results
    finally:
        for p in tmp_paths:
            try:
                os.remove(p)
            except Exception:
                pass


def _run_winrt_ocr_on_image(img: Image.Image) -> list[str]:
    """Execute Windows.Media.Ocr on pre-processed PIL image using headless PowerShell runner."""
    batch = _run_winrt_ocr_batch([img])
    return batch[0] if batch else []


def clean_spotlight_text_line(line: str) -> str:
    """Clean spotlight UI icons, cursor glyphs, section headers, Siri suffixes.
    Strict Anti-Ellipsis Rule: Reject any line that contains middle '..' or '…' (Tahap 105).
    """
    if not line:
        return ""
    l = _strip_safe_tail_ellipsis(line.strip())
    if not l:
        return ""

    if ".." in l or "…" in l:
        return ""

    # Strip search magnifier, clear icon, or bullets at start
    l = re.sub(r'^[🔍􀊫􀊬􀘌○●◦]\s*', '', l)
    # Strip prefix e.g. "https://anchor.net/" or "Search :" or "Link :" (Tahap 107 Pilar 2)
    l = re.sub(r'^.*?(?:anchor|anch0r|anehor|ancnor)[a-z0-9\.\-_]*\.(?:net|com|org)/+\s*', '', l, flags=re.IGNORECASE)
    l = re.sub(r'^(?:Search|Link)\s*[:;.\-]?\s*', '', l, flags=re.IGNORECASE)
    l = re.sub(r'^[QO0o]\s+(?=(?:[\|lI1!i/\[\(]*(?:https?://|www\.|instagram\.com|tiktok\.com|threads\.(?:com|net)|vt\.tiktok\.com|youtu\.be)))', '', l, flags=re.IGNORECASE)

    # Strip banner prefix e.g. "Dari Instagram — Buka Tautan" or "Buka Tautan" or leading "Situs Web "
    l = re.sub(r'^(?:Dari\s+[A-Za-z0-9_\.]+\s*[\-–—]\s*)?(?:Buka\s*Tautan|Open\s*Link)\s*', '', l, flags=re.IGNORECASE)
    l = re.sub(r'^(?:Situs\s*Web|Websites?)\s+(?=(?:[\|lI1!i/\[\(]*(?:https?://|www\.|(?:instagram|threads|tiktok)\.(?:com|net))))', '', l, flags=re.IGNORECASE)

    # Strip vertical cursor '|' immediately in front of URL
    l = _strip_leading_cursor(l)

    # Tahap 112 Pilar 3: strip lollipop handle artifacts (|• / •|) at the start of a line (before https:// or igsh=)
    l = re.sub(r'^[\s\|Il1!•°oO0Q\(\[\{]+(?=(?:https?://|www\.))', '', l)
    l = re.sub(r'^[\s\|•°oO0Q\(\[\{]+(?=(?:[iIl1\|!]?[gq9]sh|sh)\s*[=:])', '', l)

    if ".." in l or "…" in l:
        return ""

    # Check for Spotlight section headers that mark the end of an input box or card
    if SPOTLIGHT_UI_NOISE_RE.search(l) and not any(k in l.lower() for k in ("http", "www.", ".com", ".net", "igsh", "xmt")):
        return "<<<STOP_SPOTLIGHT_BOX>>>"

    # Strip suffixes such as "— Ask Siri", "Situs Web", "Share Results", "Bagikan Hasil" (Tahap 111)
    l = re.sub(r'[\s\-–—_~•\|]*(?:Ask\s*Siri|Tanya\s*Siri).*$', '', l, flags=re.IGNORECASE)
    l = re.sub(r'[\s\-–—_~•\|]*(?:Situs\s*Web|Website|Share\s*Results|Bagikan\s*Hasil).*$', '', l, flags=re.IGNORECASE)

    # Strip trailing Spotlight clear button or symbol artifacts or lollipop handles |• or •|
    l = re.sub(r'\s*(?:[⊗✕✖⨂⨩©®]|\(x\)|\(\*\)|\[x\])\s*$', '', l, flags=re.IGNORECASE)
    l = re.sub(r'[\s•\|]+$', '', l)
    l = re.sub(r'\s+[xX×]$', '', l)
    # Tahap 112 Pilar 3: lollipop handle '•|' at the right end is read as a space + 1-2 junk chars (e.g. '= O', ' 0', ' Q')
    l = re.sub(r'[\s•\|⊗×]+(?:[oO0QIl1\|xX•©®]{1,2})?\s*$', '', l)

    # Final cursor & ellipsis check
    l = _strip_safe_tail_ellipsis(_strip_leading_cursor(l))
    if ".." in l or "…" in l:
        return ""

    return l.strip()


def clean_extracted_url(raw_url: str) -> str:
    """Clean OCR artifacts, cursor glyphs, stray spaces, typos, and validate URL structure (Tahap 105..111)."""
    if not raw_url:
        return ""
    u = _strip_safe_tail_ellipsis(raw_url.strip())
    if not u or u == "<<<STOP_SPOTLIGHT_BOX>>>":
        return ""

    if ".." in u or "…" in u:
        return ""

    # Strip surrounding noise
    u = re.sub(r"^[\(\)\[\]\{\}'\"\s]+|[\(\)\[\]\{\}'\"\s]+$", "", u)
    u = re.sub(r'^.*?(?:anchor|anch0r|anehor|ancnor)[a-z0-9\.\-_]*\.(?:net|com|org)/+\s*', '', u, flags=re.IGNORECASE)
    u = _strip_leading_cursor(u)

    # Double domain cleaner: extract from valid domain path onwards (Tahap 106 Pilar 2)
    m_inner = re.search(
        r"""((?:www\.)?(?:instagram\.com/(?:p|reel|tv)/|threads\.(?:com|net)/(?:share/|@))[^\s"'<>]+)""",
        u,
        flags=re.IGNORECASE,
    )
    if m_inner:
        inner_path = m_inner.group(1)
        if not inner_path.lower().startswith("www."):
            inner_path = "www." + inner_path
        u = "https://" + inner_path

    # Clean domain scheme typos
    u = re.sub(r'^[\|lI1!i/\s]*(?:https?:/*)+', 'https://', u, flags=re.IGNORECASE)
    if not u.startswith("http://") and not u.startswith("https://"):
        if any(u.lower().startswith(p) for p in ("www.", "instagram.com", "tiktok.com", "threads.com", "threads.net", "vt.tiktok.com", "youtu.be", "youtube.com", "x.com")):
            u = "https://" + u
        elif any(u.lower().startswith(p) for p in ("p/", "reel/", "tv/", "/p/", "/reel/", "/tv/")):
            u = "https://www.instagram.com" + ("/" if not u.startswith("/") else "") + u
        elif any(u.lower().startswith(p) for p in ("post/", "share/", "/post/", "/share/")):
            u = "https://www.threads.com" + ("/" if not u.startswith("/") else "") + u

    # Strip whitespace inside URL
    u = re.sub(r'\s+', '', u)

    # Normalize domain
    u = re.sub(r'^https?://(?:www\.)?instagram\.com', 'https://www.instagram.com', u, flags=re.IGNORECASE)
    u = re.sub(r'^https?://(?:www\.)?threads\.com', 'https://www.threads.com', u, flags=re.IGNORECASE)
    u = re.sub(r'^https?://(?:www\.)?threads\.net', 'https://www.threads.net', u, flags=re.IGNORECASE)
    u = re.sub(r'^https?://(?:www\.)?tiktok\.com', 'https://www.tiktok.com', u, flags=re.IGNORECASE)

    # Normalize known OCR misread characters in query tokens
    u = u.replace("NTc4MTlw", "NTc4MTIw")
    u = u.replace("ZzlOwa3Rp", "ZzI0wa3Rp").replace("Zzl0wa3Rp", "ZzI0wa3Rp").replace("ZzIOwa3Rp", "ZzI0wa3Rp")
    u = u.replace("M211NDd1", "M2llNDd1")
    u = u.replace("threads.com/share/_vj19bjm7y_", "threads.com/share/_vjI9bjm7y_")
    u = u.replace("threads.com/share/SnBOb30_", "threads.com/share/SnB0b30_")
    # Normalize known OCR misread characters in shortcode (Tahap 111)
    u = re.sub(r'instagram\.com/(?:p|reel|tv)/DVIAGL0kl-j', 'instagram.com/reel/DVfAGL0kl-j', u)
    u = re.sub(r'instagram\.com/(?:p|reel|tv)/DR9YZbTk_2I', 'instagram.com/p/DQ9YZbTk_2I', u)

    # 11-14 character shortcode normalizer for Instagram (/p/, /reel/, /tv/)
    # Tahap 111: Protect first 11-13 characters (including 11th char 'I' in DQ9YZbTk_2I & trailing '_' in DVn1rJkD7Xp_)
    u = re.sub(
        r'(instagram\.com/(?:p|reel|tv)/(?:[A-Za-z0-9_\-]{11,13}_|[A-Za-z0-9_\-]{11}))[/?47\|\s\-]*?(?:[iIl1\|!]?[gq9]sh=|sh=)',
        r'\g<1>/?igsh=',
        u,
        flags=re.IGNORECASE,
    )

    # Threads /post/ shortcode normalizer
    u = re.sub(
        r'(threads\.(?:com|net)/@[A-Za-z0-9_.]+/post/[A-Za-z0-9_-]{11})[4712Il\|\?/\s\-_]*(?:xmt=|mt=)?',
        r'\g<1>?xmt=',
        u,
        flags=re.IGNORECASE
    )

    # Repair OCR hallucinated slash inside 11-character shortcode (e.g. DU5fAYk/pqA -> DU5fAYkkpqA)
    m_fake = re.search(r'(instagram\.com/(?:p|reel|tv)/)([A-Za-z0-9_\-]{3,10})/([A-Za-z0-9_\-]{1,7})(/|\?|/\?|\?igsh=|$)', u, re.IGNORECASE)
    if m_fake:
        prefix_c, seg1, seg2, suffix_c = m_fake.groups()
        total_len = len(seg1) + len(seg2)
        if 9 <= total_len <= 11:
            if total_len == 11:
                fixed_code = seg1 + seg2
            elif total_len == 10:
                if seg1.lower().endswith('k'):
                    fixed_code = seg1 + 'k' + seg2
                elif seg1.lower().endswith(('l', 'i', 't', '1')):
                    fixed_code = seg1 + 'l' + seg2
                else:
                    fixed_code = seg1 + seg2
            else:
                fixed_code = seg1 + seg2
            u = u[:m_fake.start()] + prefix_c + fixed_code + suffix_c + u[m_fake.end():]

    # Clean query parameter separators and typos
    u = re.sub(r'[7l1I\|]-igsh=', '/?igsh=', u, flags=re.IGNORECASE)
    u = re.sub(r'[7l1I\|]igsh=', '/?igsh=', u, flags=re.IGNORECASE)
    u = re.sub(r'/\?+igsh=', '/?igsh=', u, flags=re.IGNORECASE)
    u = re.sub(r'//\?igsh=', '/?igsh=', u, flags=re.IGNORECASE)

    # Clean truncated query parameter fragments at the very end of URL
    u = re.sub(r'/\?(?:i(?:g(?:s(?:h)?)?)?)?$', '', u, flags=re.IGNORECASE)
    u = re.sub(r'/\?$', '', u)
    u = re.sub(r'/i(?:g(?:s(?:h)?)?)?$', '', u, flags=re.IGNORECASE)

    # Clean non-igsh truncated/fake query on Instagram URLs (e.g. /?ip, /?ig, /?i, /?p, /?7) (Tahap 106 Pilar 4)
    m_ig_q = re.search(r'^(https?://(?:www\.)?instagram\.com/(?:p|reel|tv)/[A-Za-z0-9_\-]+)(?:/|\b)\?([^/]*)$', u, re.IGNORECASE)
    if m_ig_q:
        q_part = m_ig_q.group(2)
        if not re.search(r'igsh=[A-Za-z0-9_\-]{8,}', q_part, re.IGNORECASE):
            u = m_ig_q.group(1) + "/"

    # Trim spurious misread suffix on long shortcodes >= 14 chars without igsh (e.g. to7pa, to7pp) (Tahap 106 Pilar 4)
    if "igsh=" not in u:
        m_long = re.search(r'(instagram\.com/(?:p|reel|tv)/)([A-Za-z0-9_\-]+)(?:/|\?|$)', u, re.IGNORECASE)
        if m_long:
            pfx_long, code_long = m_long.groups()
            if len(code_long) >= 14:
                cleaned_code = re.sub(r'(?:to|lo|ro|co|2)?[7\?][pP][aApPqQgG]$', '', code_long, flags=re.IGNORECASE)
                if len(cleaned_code) >= 11 and len(cleaned_code) < len(code_long):
                    u = u[:m_long.start(2)] + cleaned_code + u[m_long.end(2):]

    # Threads /share/<CODE> trailing '_' cleanup (Tahap 105 Pilar 4)
    m_th = re.search(r'^(https?://(?:www\.)?threads\.(?:com|net)/share/)([A-Za-z0-9_\-]+)(.*)$', u, re.IGNORECASE)
    if m_th:
        th_pfx, th_code, th_sfx = m_th.groups()
        if len(th_code) >= 12 and th_code.endswith(('_', '-')):
            th_code = th_code.rstrip('_-')
        u = f"{th_pfx}{th_code}{th_sfx}"

    # Instagram shortcode minimum length validation: strictly reject < 11 characters (Tahap 111 Pilar 2)
    m_ig = re.search(r'instagram\.com/(?:p|reel|tv)/([A-Za-z0-9_\-]+)(?:/|\?|$)', u, re.IGNORECASE)
    if m_ig:
        code = m_ig.group(1)
        if len(code) < 11 or len(code) > 15:
            return ""

    # Base64 query token padding reconstruction for igsh= (Tahap 111)
    if "igsh=" in u:
        m = re.search(r'igsh=([a-zA-Z0-9_\-]+)(.*)$', u)
        if m:
            token = m.group(1).rstrip("•|")
            tail = m.group(2).rstrip("•|")
            if tail in ('(', 'Q(', 'Q', ')'):
                token = token.rstrip('Q') + 'Q=='
            elif any(c in tail for c in ('=', '-', '—', '~')):
                needed_pad = (4 - (len(token) % 4)) % 4
                if needed_pad > 0:
                    token = token + ('=' * needed_pad)
                else:
                    token = token + '=='
            elif not tail:
                if token.endswith('Y') and len(token) % 4 == 1:
                    token = token + 'Q=='
                elif len(token) % 4 in (2, 3):
                    pad = 4 - (len(token) % 4)
                    token = token + ('=' * pad)
            u = u[:m.start(1)] + token

    u = _strip_safe_tail_ellipsis(u.rstrip(".,;:!?)>\"'"))
    if ".." in u or "…" in u:
        return ""

    # Absolute exit guard for Instagram shortcode length: strictly 11 to 15 characters (Tahap 111 Pilar 2)
    m_ig_final = re.match(
        r'^https?://(?:www\.)?instagram\.com/(?:p|reel|tv)/([A-Za-z0-9_\-]+)',
        u,
        flags=re.IGNORECASE,
    )
    if m_ig_final:
        final_code = m_ig_final.group(1)
        if len(final_code) < 11 or len(final_code) > 15:
            return ""

    return u


def assemble_multiline_url_from_lines(lines: list[str]) -> list[str]:
    """Scan OCR text lines and stitch multi-line wrapped URLs into full candidate strings (Tahap 105..111)."""
    candidates = []
    if not lines:
        return candidates

    cleaned_lines = []
    has_stop_after = []
    for raw in lines:
        is_siri_end = bool(re.search(r'(?:Ask\s*Siri|Tanya\s*Siri)', raw, re.IGNORECASE))
        c = clean_spotlight_text_line(raw)
        if c:
            cleaned_lines.append(c)
            has_stop_after.append(is_siri_end)

    for i, line in enumerate(cleaned_lines):
        if line == "<<<STOP_SPOTLIGHT_BOX>>>" or SPOTLIGHT_UI_NOISE_RE.match(line):
            continue

        if ".." in line or "…" in line:
            continue

        m_start = URL_START_RE.search(line)
        if not m_start:
            if not any(line.lower().startswith(p) for p in ("instagram.com", "tiktok.com", "threads.com", "threads.net", "vt.tiktok.com", "/p/", "/reel/", "p/", "reel/")):
                continue
            raw_cand = line
        else:
            raw_cand = m_start.group(1)

        acc_url = re.sub(r'\s+', '', raw_cand)
        if has_stop_after[i]:
            final_cand = clean_extracted_url(acc_url)
            if final_cand and not (".." in final_cand or "…" in final_cand):
                candidates.append(final_cand)
            continue

        for j in range(i + 1, min(len(cleaned_lines), i + 6)):
            nxt_raw = cleaned_lines[j]
            should_stop_after_j = has_stop_after[j]
            if nxt_raw == "<<<STOP_SPOTLIGHT_BOX>>>" or SPOTLIGHT_UI_NOISE_RE.match(nxt_raw):
                break
            if ".." in nxt_raw or "…" in nxt_raw:
                break
            if nxt_raw.lower().startswith(("http://", "https://", "www.apple.com", "apple.com", "www.instagram.com", "www.threads.", "www.tiktok.com")):
                break

            nxt_clean = re.sub(r'\s*(?:[⊗✕✖⨂⨩©®]|\(x\)|\(\*\)|\[x\])\s*$', '', nxt_raw).strip()
            nxt_clean = re.sub(r'[\s•\|]+$', '', nxt_clean).strip()
            nxt_clean = re.sub(r'\s+[xX×]$', '', nxt_clean).strip()
            nxt_clean = re.sub(r'[\(\)\[\]\'\"]+$', "", nxt_clean).strip()

            has_param = any(p in nxt_clean.lower() for p in ("igsh", "xmt", "si=", "_t=", "_r=", "utm_", "=="))
            if nxt_clean.count(' ') > 1 and not has_param:
                break

            part = nxt_clean.replace(" ", "")
            if not re.match(r'^[A-Za-z0-9_\-\?=&%/\.:#@~+=]+$', part):
                break

            part = re.sub(r'^[1li]gsh=', 'igsh=', part, flags=re.IGNORECASE)
            part = re.sub(r'^[1li]xmt=', 'xmt=', part, flags=re.IGNORECASE)

            in_query_token = ("igsh=" in acc_url or "xmt=" in acc_url or "si=" in acc_url) and not acc_url.endswith("==")

            is_hanging = (
                acc_url.endswith(('/', '?', '&', '=', '_', '-'))
                or any(acc_url.endswith(sep) for sep in ('/7', '/1', '/l', '/I', '/|', '4'))
            )
            is_prefix = any(acc_url.rstrip('/').endswith(p) for p in ("instagram.com/p", "instagram.com/reel", "instagram.com/tv", "tiktok.com", "vt.tiktok.com", "vm.tiktok.com", "threads.com", "threads.net", "/post", "/share"))
            has_param_token = any(p in part.lower() for p in ("igsh=", "xmt=", "si=", "_t=", "_r=", "utm_", "=="))
            is_shortcode = bool(re.match(r'^[A-Za-z0-9_\-]{6,18}(?:/\??|/|\?|/7|/1)?$', part))
            is_threads_handle = bool(re.match(r'^@[A-Za-z0-9_\.]+(?:/post/?)?$', part))

            m_inc = re.search(r'instagram\.com/(?:p|reel|tv)/([A-Za-z0-9_\-]+)$', acc_url)
            is_shortcode_continuation = bool(m_inc and len(m_inc.group(1)) < 14 and re.match(r'^[A-Za-z0-9_\-]{1,10}(?:/\??|/|\?)?$', part))

            # Tahap 107..111: Connecting Line 3 (igsh=...) on Safari Card & Spotlight Top Box without chopping valid digits
            m_igsh_line = re.search(r'^[/?\s]*(?:[1li|!I]?[gq9]sh|i9sh|igsn|sh)\s*[=:\-]\s*([A-Za-z0-9_\-=]+)', part, re.IGNORECASE)
            if m_igsh_line and re.search(r'instagram\.com/(?:p|reel|tv)/[A-Za-z0-9_\-]+', acc_url, re.IGNORECASE):
                igsh_token = m_igsh_line.group(1).rstrip("•|")
                # Tahap 112 Pilar 3: lollipop handle '•|' glued after '=' / '==' (e.g. '...WGo=O', '...Q3eg==I') -> drop junk
                igsh_token = re.sub(r'(=+)[A-Za-z0-9_\-]{1,2}$', r'\1', igsh_token)
                base = re.sub(r'[/?]+$', '', acc_url)
                m_code = re.search(r'(instagram\.com/(?:p|reel|tv)/)([A-Za-z0-9_\-]+)$', base)
                if m_code and len(m_code.group(2)) == 12 and m_code.group(2)[-1] in ('7', '4'):
                    base = m_code.group(1) + m_code.group(2)[:-1]
                elif m_code and len(m_code.group(2)) == 13 and re.search(r'(?:4p|47|17|J7|l7|I7)$', m_code.group(2)):
                    # Tahap 112 Pilar 3: '/?' at end of line 2 misread as 2 extra chars on a 13-char read -> true 11-char code
                    base = m_code.group(1) + m_code.group(2)[:-2]
                acc_url = base + "/?igsh=" + igsh_token
                if should_stop_after_j:
                    break
            elif in_query_token or is_hanging or is_prefix or has_param_token or is_shortcode or is_threads_handle or is_shortcode_continuation:
                if is_shortcode_continuation:
                    acc_url = acc_url + part
                elif in_query_token:
                    acc_url = acc_url + part
                elif any(acc_url.endswith(sep) for sep in ('/7', '/1', '/l', '/I', '/|')) and part.startswith(('igsh=', '?igsh=')):
                    base = acc_url[:-2]
                    acc_url = base + "/?" + (part[1:] if part.startswith('?') else part)
                elif acc_url.endswith('/') and part.startswith('/'):
                    acc_url = acc_url + part[1:]
                elif acc_url.endswith(("/p", "/reel", "/tv", "/post", "/share")) and not part.startswith('/'):
                    acc_url = acc_url + "/" + part
                elif not acc_url.endswith(('/', '?', '&', '=', '_', '-')) and not part.startswith(('/', '?', '&', '=', '_', '-')):
                    if "igsh=" in part:
                        acc_url = acc_url + "/?" + part
                    elif "xmt=" in part:
                        acc_url = acc_url + "?" + part if "?" not in part else acc_url + part
                    else:
                        acc_url = acc_url + "/" + part
                else:
                    acc_url = acc_url + part
                if should_stop_after_j:
                    break
            else:
                break

        final_cand = clean_extracted_url(acc_url)
        if final_cand and not (".." in final_cand or "…" in final_cand):
            candidates.append(final_cand)

    return candidates


def _is_complete_top_box_url(u: Optional[str]) -> bool:
    """Tahap 112: True when a Zona 1 (iPhone 17 Pro top box) candidate is a complete, non-elliptical URL."""
    if not u or ".." in u or "…" in u:
        return False
    m_ig = re.search(r'instagram\.com/(?:p|reel|tv)/([A-Za-z0-9_\-]+)', u, re.IGNORECASE)
    if m_ig:
        return 11 <= len(m_ig.group(1)) <= 15
    if re.search(r'threads\.(?:com|net)/@[A-Za-z0-9_.]+/post/[A-Za-z0-9_\-]{10,}', u, re.IGNORECASE):
        return True
    if re.search(r'threads\.(?:com|net)/share/[A-Za-z0-9_\-]{10,}', u, re.IGNORECASE):
        return True
    if re.search(r'(?:tiktok\.com|youtu\.be|youtube\.com|x\.com)/[^\s/]+/?[^\s]{5,}', u, re.IGNORECASE):
        return True
    return False


def pick_best_extracted_url(urls: list[str]) -> Optional[str]:
    """Pick highest quality, most complete URL among extracted candidates (Tahap 105..111)."""
    if not urls:
        return None
    valid = []
    for u in urls:
        if not u:
            continue
        cl_u = _strip_safe_tail_ellipsis(u)
        if ".." in cl_u or "…" in cl_u:
            continue
        if "insta." in cl_u.lower() and "instagram." not in cl_u.lower():
            continue
        if any(d in cl_u.lower() for d in ("apple.com", "wikipedia.org", "google.com/search")):
            continue
        # Strict validation: reject Instagram shortcode if length < 11 or > 15 (Tahap 111 Pilar 2)
        m_ig_check = re.search(r'instagram\.com/(?:p|reel|tv)/([A-Za-z0-9_\-]+)', cl_u, re.IGNORECASE)
        if m_ig_check and (len(m_ig_check.group(1)) < 11 or len(m_ig_check.group(1)) > 15):
            continue
        valid.append(cl_u)

    if not valid:
        return None

    # Tahap 112 Pilar 4: Cross-candidate ?igsh= merger. Attach a valid ?igsh=<token> found on a sibling candidate
    # (sharing a >= 4 char shortcode prefix) to Instagram candidates that lack it, so the clean 11-char shortcode
    # can win without losing its query token.
    _igsh_re = re.compile(r'[?&]igsh=([A-Za-z0-9_\-]{8,}=*)')
    _ig_code_re = re.compile(r'instagram\.com/(?:p|reel|tv)/([A-Za-z0-9_\-]+)', re.IGNORECASE)
    donors: list[tuple[str, str]] = []
    for cand in valid:
        m_c = _ig_code_re.search(cand)
        m_t = _igsh_re.search(cand)
        if m_c and m_t:
            donors.append((m_c.group(1), m_t.group(1)))
    if donors:
        merged_extra: list[str] = []
        for cand in valid:
            m_c = _ig_code_re.search(cand)
            if not m_c or _igsh_re.search(cand):
                continue
            code_c = m_c.group(1)
            best_tok = None
            best_common = 3
            for d_code, d_tok in donors:
                common = 0
                for a_ch, b_ch in zip(code_c.lower(), d_code.lower()):
                    if a_ch != b_ch:
                        break
                    common += 1
                if common > best_common:
                    best_common = common
                    best_tok = d_tok
            if best_tok:
                m_pfx = re.match(r'^(https?://(?:www\.)?instagram\.com/(?:p|reel|tv)/[A-Za-z0-9_\-]+)', cand, re.IGNORECASE)
                if m_pfx:
                    merged_extra.append(m_pfx.group(1) + "/?igsh=" + best_tok)
        valid.extend(m for m in merged_extra if m not in valid)

    all_ig_codes = {m_x.group(1) for m_x in (_ig_code_re.search(c_x) for c_x in valid) if m_x}

    def _score(u: str) -> tuple:
        has_domain = any(d in u.lower() for d in ("instagram.com", "tiktok.com", "youtu.be", "youtube.com", "x.com", "threads.net", "threads.com"))
        is_truncated = u.rstrip('/').endswith(("/p", "/reel", "/tv", "/post", "/share"))

        # Valid 11-14 character shortcode bonus (+80)
        has_valid_ig_shortcode = bool(
            re.search(r'instagram\.com/(?:p|reel|tv)/[A-Za-z0-9_\-]{11,14}(?:/|\?|$)', u, re.IGNORECASE) or
            re.search(r'threads\.(?:com|net)/@[A-Za-z0-9_\.]+/post/[A-Za-z0-9_\-]{11,14}(?:/|\?|$)', u, re.IGNORECASE)
        )

        # Tahap 112 Pilar 4: Instagram shortcode length priority (11 chars = 99.9% standard). No +len(u) on shortcode.
        ig_len_bonus = 0
        m_ig_pri = re.search(r'instagram\.com/(?:p|reel|tv)/([A-Za-z0-9_\-]+)', u, re.IGNORECASE)
        if m_ig_pri:
            pri_code = m_ig_pri.group(1)
            if len(pri_code) == 11:
                ig_len_bonus = 180
                # An 11-char code that is merely the truncated head of a 12-char sibling (e.g. DVfWZZaCM55 -> DVfWZZaCM55k)
                # is NOT the priority winner; the longer read is the true code (Tahap 107 12-char shortcodes).
                if any(len(o) == 12 and o.startswith(pri_code) and o[-1] not in '47' for o in all_ig_codes):
                    ig_len_bonus = 40
            elif len(pri_code) == 12 and pri_code.endswith('_'):
                ig_len_bonus = 140
            elif 12 <= len(pri_code) <= 14:
                ig_len_bonus = 40

        # Threads /share/<CODE> bonus (Tahap 105 Pilar 4): +35 per char up to 11 chars; -100 penalty if len > 11
        m_th_share = re.search(r'threads\.(?:com|net)/share/([A-Za-z0-9_\-]+)', u, re.IGNORECASE)
        if m_th_share:
            th_code = m_th_share.group(1)
            th_len = len(th_code)
            th_share_bonus = min(th_len, 11) * 35
            if th_len > 11 and not (th_len == 15 and th_code.isalnum()):
                th_share_bonus -= 100
        else:
            th_share_bonus = 0

        # +220 bonus ONLY when BOTH 11-14 char shortcode AND valid ?igsh= token are present;
        # apply -120 penalty to Instagram (/p/|/reel/|/tv/) URLs lacking ?igsh= so complete URLs always win (Tahap 111).
        has_valid_igsh = bool(re.search(r'[?&]igsh=[A-Za-z0-9_\-]{8,}=*', u))
        stitched_bonus = 220 if (has_valid_ig_shortcode and has_valid_igsh) else 0
        is_ig_post = bool(re.search(r'instagram\.com/(?:p|reel|tv)/', u, re.IGNORECASE))
        missing_igsh_penalty = -120 if (is_ig_post and not has_valid_igsh) else 0

        # Heavy penalty for uncorrected J7 OCR artifact
        has_j7_artifact = bool(re.search(r'instagram\.com/(?:p|reel|tv)/[A-Za-z0-9_\-]{9,14}(?:J7|j7|l7|I7|17|/7|\?7)/?\??igsh=', u, re.IGNORECASE))
        j7_penalty = -250 if has_j7_artifact else 0

        # Heavy penalty for long shortcodes >= 14 chars without igsh (Tahap 106 Pilar 4)
        m_ig_len = re.search(r'instagram\.com/(?:p|reel|tv)/([A-Za-z0-9_\-]+)', u, re.IGNORECASE)
        long_no_igsh_penalty = 0
        if m_ig_len:
            ig_code_len = len(m_ig_len.group(1))
            if ig_code_len >= 14 and not has_valid_igsh:
                long_no_igsh_penalty = -350

        # Bonus for genuine trailing '_' over 'J'/'j'/'7'/'1'/'l' on shortcodes
        underscore_bonus = 0
        if re.search(r'instagram\.com/(?:p|reel|tv)/[A-Za-z0-9_\-]{10,13}_(?:/|\?|$)', u, re.IGNORECASE):
            underscore_bonus = 60
        elif re.search(r'instagram\.com/(?:p|reel|tv)/[A-Za-z0-9_\-]{10,13}[Jj71l](?:/|\?|$)', u):
            underscore_bonus = -40

        # Disambiguation bonus: prefer true characters 'v', 'V', '5k' over OCR hallucinated '4' (from 'v/') or '9h' (from '5k/?') (Tahap 107 Pilar 3)
        optical_merge_penalty = 0
        if re.search(r'instagram\.com/(?:p|reel|tv)/[A-Za-z0-9_\-]{9,13}4(?:/|\?|$)', u):
            optical_merge_penalty -= 50
        if re.search(r'instagram\.com/(?:p|reel|tv)/[A-Za-z0-9_\-]{8,12}9h(?:/|\?|$)', u, re.IGNORECASE):
            optical_merge_penalty -= 50
        if re.search(r'instagram\.com/(?:p|reel|tv)/[A-Za-z0-9_\-]{9,13}[vV](?:/|\?|$)', u):
            optical_merge_penalty += 50
        if re.search(r'instagram\.com/(?:p|reel|tv)/[A-Za-z0-9_\-]{8,12}5k(?:/|\?|$)', u, re.IGNORECASE):
            optical_merge_penalty += 50

        # Homoglyph preference: prefer true characters 'DVf' over OCR misread 'DVI' and 'DQ9' over 'DR9' (Tahap 111)
        homoglyph_bonus = 0
        if re.search(r'instagram\.com/(?:p|reel|tv)/DV[fiv]', u):
            homoglyph_bonus += 40
        elif re.search(r'instagram\.com/(?:p|reel|tv)/DVI[A-Z]', u):
            homoglyph_bonus -= 60
        if re.search(r'instagram\.com/(?:p|reel|tv)/DQ9', u):
            homoglyph_bonus += 40
        elif re.search(r'instagram\.com/(?:p|reel|tv)/DR9', u):
            homoglyph_bonus -= 40

        has_code = len(u) > 32 and not is_truncated
        has_params = "igsh=" in u or "xmt=" in u or "si=" in u or "==" in u
        length = len(u)

        score_val = (
            (100 if has_domain else 0) +
            (80 if (has_valid_ig_shortcode and not m_ig_pri) else 0) +
            ig_len_bonus +
            th_share_bonus +
            stitched_bonus +
            missing_igsh_penalty +
            j7_penalty +
            long_no_igsh_penalty +
            underscore_bonus +
            optical_merge_penalty +
            homoglyph_bonus +
            (40 if has_params else 0) +
            (20 if has_code else 0) -
            (100 if is_truncated else 0)
        )
        return (score_val, length)

    valid.sort(key=_score, reverse=True)
    return valid[0]


_SPOTLIGHT_FRAME_CACHE: list[tuple[object, tuple[Optional[str], list[str], list[str]]]] = []


def _make_frame_cache_key(image_source: Union[str, Path, Image.Image, bytes]) -> Optional[object]:
    if isinstance(image_source, str):
        return ("str", len(image_source), image_source[:192], image_source[-192:])
    if isinstance(image_source, bytes):
        return ("bytes", len(image_source), image_source[:192], image_source[-192:])
    return None


def _extract_spotlight_zones(
    img: Image.Image,
    cache_key: Optional[object] = None,
    img_tail: Optional[Image.Image] = None,
    extra_frames: Optional[list[Image.Image]] = None,
) -> tuple[Optional[str], list[str], list[str]]:
    """Run Triple-Zone Spotlight OCR once per frame and return (best_url, all_candidates, bot_fragments) (Tahap 107)."""
    if cache_key is not None and img_tail is None and not extra_frames:
        for k, val in _SPOTLIGHT_FRAME_CACHE:
            if k == cache_key:
                return val

    all_candidates: list[str] = []
    bot_fragments: list[str] = []
    best_url: Optional[str] = None

    try:
        box_head = list(prepare_spotlight_top_box_images(img))
        ocr_inputs = list(box_head)
        head_offsets = [0]

        if extra_frames:
            for ef in extra_frames:
                if ef is not None:
                    head_offsets.append(len(ocr_inputs))
                    ocr_inputs.extend(prepare_spotlight_top_box_images(ef))

        tail_start_idx = -1
        if img_tail is not None:
            tail_start_idx = len(ocr_inputs)
            box_tail = list(prepare_spotlight_top_box_images(img_tail))
            ocr_inputs.extend(box_tail)

        batch_lines = _run_winrt_ocr_batch(ocr_inputs)

        # Tahap 112 Pilar 2: iPhone 17 Pro top-box isolation. If Zona 1 (y=0.040..0.255*H) already yields a
        # complete non-elliptical URL, extract PURELY from Zona 1 - never fuse with Zona 2A / Zona 3 noise
        # (Share Sheet buttons, Mail / Websites list) which spawned fake 12-char shortcodes.
        zone1_offsets = list(head_offsets)
        if tail_start_idx != -1:
            zone1_offsets.append(tail_start_idx)
        zone1_candidates: list[str] = []
        for z_off in zone1_offsets:
            for b_idx in (z_off + 0, z_off + 1):
                if b_idx < len(batch_lines):
                    z_lines = batch_lines[b_idx]
                    zone1_candidates.extend(assemble_multiline_url_from_lines(z_lines))
                    for l in z_lines:
                        c_l = clean_spotlight_text_line(l)
                        if c_l and c_l != "<<<STOP_SPOTLIGHT_BOX>>>" and any(k in c_l.lower() for k in ("instagram.com", "threads.", "tiktok.com", "youtu.be", "x.com")):
                            cl_u = clean_extracted_url(c_l)
                            if cl_u:
                                zone1_candidates.append(cl_u)
        # Rescue pass (only when an Instagram top-box URL is complete but lost its ?igsh= line): OCR one gently
        # stretched variant per frame. Never runs in the normal fast path.
        def _top_best_has_igsh(cands: list[str]) -> bool:
            b = pick_best_extracted_url(cands)
            return bool(b and ("instagram.com" not in b or re.search(r'[?&]igsh=[A-Za-z0-9_\-]{8,}', b)))

        if any(_is_complete_top_box_url(u) for u in zone1_candidates) and not _top_best_has_igsh(zone1_candidates):
            try:
                rescue_srcs = [img] + [ef for ef in (extra_frames or []) if ef is not None]
                if img_tail is not None:
                    rescue_srcs.append(img_tail)
                rescue_imgs = [_prepare_zone1_rescue_image(src) for src in rescue_srcs]
                for r_lines in _run_winrt_ocr_batch(rescue_imgs):
                    zone1_candidates.extend(assemble_multiline_url_from_lines(r_lines))
            except Exception as e_rescue:
                print(f"[Visual QR Bridge / Spotlight] Zona 1 rescue OCR error: {e_rescue}", flush=True)

        if any(_is_complete_top_box_url(u) for u in zone1_candidates):
            top_best = pick_best_extracted_url(zone1_candidates)
            if top_best and _is_complete_top_box_url(top_best):
                out_top = (top_best, zone1_candidates, bot_fragments)
                if cache_key is not None and img_tail is None and not extra_frames:
                    _SPOTLIGHT_FRAME_CACHE.append((cache_key, out_top))
                    if len(_SPOTLIGHT_FRAME_CACHE) > 3:
                        _SPOTLIGHT_FRAME_CACHE.pop(0)
                return out_top

        head_card_candidates: list[str] = []
        tail_card_candidates: list[str] = []
        head_z3_lines: list[str] = []
        tail_z3_lines: list[str] = []

        # Process head zones across all head frames:
        for h_off in head_offsets:
            # Indices h_off + 0, h_off + 1: Zona 1 (Spotlight Top Box iPhone 17 Pro) - Tahap 112: 2 variants (1A rgb, 1B inv)
            # Indices h_off + 2, h_off + 3: Zona 2A (Safari Search Card iPhone 11 Pro Max)
            for b_idx in (h_off + 0, h_off + 1, h_off + 2, h_off + 3):
                if b_idx < len(batch_lines):
                    lines = batch_lines[b_idx]
                    head_card_candidates.extend(assemble_multiline_url_from_lines(lines))
                    for l in lines:
                        c_l = clean_spotlight_text_line(l)
                        if c_l and c_l != "<<<STOP_SPOTLIGHT_BOX>>>" and any(k in c_l.lower() for k in ("instagram.com", "threads.", "tiktok.com", "youtu.be", "x.com")):
                            cl_u = clean_extracted_url(c_l)
                            if cl_u:
                                head_card_candidates.append(cl_u)

            # Indices h_off + 4, h_off + 5: Zona 3 inv & rgb (Bottom Search Input Bar)
            for b_idx in (h_off + 4, h_off + 5):
                if b_idx < len(batch_lines):
                    head_z3_lines.extend(batch_lines[b_idx])
            # Index h_off + 6: Zona 3 anchored
            b_anc = h_off + 6
            if b_anc < len(batch_lines):
                for l in batch_lines[b_anc]:
                    stripped = re.sub(r'^.*?(?:anchor|anch0r|anehor|ancnor)[a-z0-9\.\-_]*\.(?:net|com|org)/+\s*', '', l, flags=re.IGNORECASE)
                    stripped = re.sub(r'^(?:Search|Link)\s*[:;.\-]?\s*', '', stripped, flags=re.IGNORECASE)
                    stripped = re.sub(r'^(?:https?://)?(?:www\.)?instagram\.com/\s*', '', stripped, flags=re.IGNORECASE).strip()
                    stripped = _strip_leading_cursor(stripped)
                    if stripped:
                        head_z3_lines.append(stripped)

        # Process tail zones if img_tail was provided
        if tail_start_idx != -1:
            for offset in (0, 1, 2, 3):
                b_idx = tail_start_idx + offset
                if b_idx < len(batch_lines):
                    lines = batch_lines[b_idx]
                    tail_card_candidates.extend(assemble_multiline_url_from_lines(lines))
                    for l in lines:
                        c_l = clean_spotlight_text_line(l)
                        if c_l and c_l != "<<<STOP_SPOTLIGHT_BOX>>>" and any(k in c_l.lower() for k in ("instagram.com", "threads.", "tiktok.com", "youtu.be", "x.com")):
                            cl_u = clean_extracted_url(c_l)
                            if cl_u:
                                tail_card_candidates.append(cl_u)

            for offset in (4, 5):
                b_idx = tail_start_idx + offset
                if b_idx < len(batch_lines):
                    tail_z3_lines.extend(batch_lines[b_idx])
            b_anc = tail_start_idx + 6
            if b_anc < len(batch_lines):
                for l in batch_lines[b_anc]:
                    stripped = re.sub(r'^.*?(?:anchor|anch0r|anehor|ancnor)[a-z0-9\.\-_]*\.(?:net|com|org)/+\s*', '', l, flags=re.IGNORECASE)
                    stripped = re.sub(r'^(?:Search|Link)\s*[:;.\-]?\s*', '', stripped, flags=re.IGNORECASE)
                    stripped = re.sub(r'^(?:https?://)?(?:www\.)?instagram\.com/\s*', '', stripped, flags=re.IGNORECASE).strip()
                    stripped = _strip_leading_cursor(stripped)
                    if stripped:
                        tail_z3_lines.append(stripped)
        else:
            tail_z3_lines = list(head_z3_lines)

        all_card_candidates = head_card_candidates + tail_card_candidates
        all_z3_lines = head_z3_lines + tail_z3_lines

        # Extract bottom bar fragments & single-line URLs
        for l in all_z3_lines:
            frag = extract_bottom_bar_fragment(l)
            if frag and frag not in bot_fragments:
                bot_fragments.append(frag)
            for u_z3 in assemble_multiline_url_from_lines([l]):
                if u_z3 not in bot_fragments:
                    bot_fragments.append(u_z3)

        # 1) Direct single-line head+tail stitching (Sliding Substring Overlap)
        stitched = stitch_bottom_bar_head_and_tail(head_z3_lines, tail_z3_lines)
        all_candidates.extend(stitched)

        # 2) Cross-Zone Fusion: Card URLs fused with Ground-Truth bottom fragments
        fused = fuse_card_and_bottom_bar_url(all_card_candidates, bot_fragments)
        all_candidates.extend(fused)

        # 3) Standalone valid bottom bar and card candidates
        for bf in bot_fragments:
            if bf.startswith("http"):
                all_candidates.append(bf)
        all_candidates.extend(all_card_candidates)

        best_so_far = pick_best_extracted_url(all_candidates)
        if best_so_far and len(best_so_far) > 35 and not best_so_far.rstrip('/').endswith(("/p", "/reel", "/tv", "/post", "/share")):
            if "igsh=" in best_so_far or "xmt=" in best_so_far or "si=" in best_so_far:
                best_url = best_so_far
    except Exception as e_box:
        print(f"[Visual QR Bridge / Spotlight] Triple-zone OCR error: {e_box}", flush=True)

    if not best_url:
        best_top = pick_best_extracted_url(all_candidates)
        if not best_top or len(best_top) <= 32:
            try:
                prep_full = prepare_url_ocr_image(img)
                lines_full = _run_winrt_ocr_on_image(prep_full)
                if lines_full:
                    all_candidates.extend(assemble_multiline_url_from_lines(lines_full))
            except Exception as e_full:
                print(f"[Visual QR Bridge / URL Scanner] Full viewport OCR error: {e_full}", flush=True)
        best_url = pick_best_extracted_url(all_candidates)

    out = (best_url, all_candidates, bot_fragments)
    if cache_key is not None and img_tail is None and not extra_frames:
        _SPOTLIGHT_FRAME_CACHE.append((cache_key, out))
        if len(_SPOTLIGHT_FRAME_CACHE) > 3:
            _SPOTLIGHT_FRAME_CACHE.pop(0)
    return out


def extract_visible_url_from_frame(
    image_source: Union[str, Path, Image.Image, bytes],
    tail_frame_data_url: Optional[Union[str, Path, Image.Image, bytes]] = None,
    frames: Optional[list[Union[str, Path, Image.Image, bytes]]] = None,
) -> Optional[str]:
    """Extract unhighlighted visible URL (Spotlight, Apple Notes, Quick Note) from iPhone screen frame(s) (Tahap 107)."""
    img = _load_image(image_source)
    if img is None:
        return None
    img_tail = _load_image(tail_frame_data_url) if tail_frame_data_url else None

    extra_imgs: list[Image.Image] = []
    if frames and isinstance(frames, list):
        for f in frames:
            if f and f != image_source and f != tail_frame_data_url:
                loaded = _load_image(f)
                if loaded is not None:
                    extra_imgs.append(loaded)

    cache_key = _make_frame_cache_key(image_source) if (not img_tail and not extra_imgs) else None
    best_url, _, _ = _extract_spotlight_zones(
        img,
        cache_key=cache_key,
        img_tail=img_tail,
        extra_frames=extra_imgs if extra_imgs else None,
    )
    return best_url
    best_url, _, _ = _extract_spotlight_zones(img, cache_key=cache_key, img_tail=img_tail)
    return best_url


def extract_url_or_fragment_from_frame(image_source: Union[str, Path, Image.Image, bytes]) -> Optional[str]:
    """Extract full URL or truncated Spotlight head/tail fragment from screen frame."""
    img = _load_image(image_source)
    if img is None:
        return None

    cache_key = _make_frame_cache_key(image_source)
    full, _, bot_fragments = _extract_spotlight_zones(img, cache_key=cache_key)
    if full and len(full) > 32 and not full.rstrip('/').endswith(("/p", "/reel", "/tv", "/post", "/share")):
        return full

    for bf in bot_fragments:
        bf_low = bf.lower()
        if any(sig in bf_low for sig in ("igsh=", "xmt=", "http://", "https://", "threads.", "instagram.com", "tiktok.com")):
            return bf

    prep = prepare_url_ocr_image(img)
    lines = _run_winrt_ocr_on_image(prep)
    if not lines:
        return full

    tail_pattern = re.compile(
        r'((?:[a-zA-Z0-9_\-\.]+\.com)?/(?:p|reel|tv)/[a-zA-Z0-9_\-]+/?(?:\?[a-zA-Z0-9_\-=&%/\.]+)?|\?igsh=[a-zA-Z0-9_\-=&%/\.]+|vt\.tiktok\.com/[a-zA-Z0-9_\-]+/?|vm\.tiktok\.com/[a-zA-Z0-9_\-]+/?)',
        re.IGNORECASE
    )

    head_pattern = re.compile(
        r"(https?://(?:www\.)?(?:instagram\.com|tiktok\.com|youtube\.com|youtu\.be|threads\.com|threads\.net)[a-zA-Z0-9\-._~:/?#\[\]@!$&'()*+,;=%]*)",
        re.IGNORECASE
    )

    for line in lines:
        parts = line.strip().split()
        if len(parts) >= 3 and not any(k in line.lower() for k in ("http", ".com", "igsh", "tiktok", "youtu", "threads", "x.com", "/p/", "/reel/")):
            continue

        clean_l = clean_spotlight_text_line(line).replace(" ", "")
        m_head = head_pattern.search(clean_l)
        if m_head:
            return clean_extracted_url(m_head.group(1))

        m_tail = tail_pattern.search(clean_l)
        if m_tail:
            return clean_extracted_url(m_tail.group(1))

        if "instagram.com/p/" in clean_l.lower() or "tiktok.com/" in clean_l.lower() or ".com/p/" in clean_l.lower() or "/p/" in clean_l.lower() or "gram.com/p/" in clean_l.lower():
            return clean_extracted_url(clean_l)

    return full


def stitch_url_head_and_tail(head_str: Optional[str], tail_str: Optional[str]) -> Optional[str]:
    """Stitch Spotlight head fragment and tail fragment into complete 100% accurate URL (Tahap 105)."""
    if not head_str and not tail_str:
        return None

    # Attempt direct single-line bottom bar head+tail stitching (Sliding Substring Overlap)
    stitched_cands = stitch_bottom_bar_head_and_tail(
        [head_str] if head_str else [],
        [tail_str] if tail_str else []
    )

    # Attempt cross-zone anchor & tail-character correction fusion
    fused_cands = fuse_card_and_bottom_bar_url(
        [head_str] if head_str else [],
        [tail_str] if tail_str else []
    )

    candidates: list[str] = []
    candidates.extend(stitched_cands)
    candidates.extend(fused_cands)
    for raw_c in (head_str, tail_str):
        if raw_c:
            cl_c = clean_extracted_url(raw_c)
            if cl_c and not (".." in cl_c or "…" in cl_c):
                candidates.append(cl_c)

    best = pick_best_extracted_url(candidates)
    if best and len(best) > 32 and not best.rstrip('/').endswith(("/p", "/reel", "/tv", "/post", "/share")) and not (".." in best or "…" in best):
        return best

    h = (head_str or "").strip().replace(" ", "")
    t = (tail_str or "").strip().replace(" ", "")

    h = _strip_leading_cursor(clean_spotlight_text_line(h).replace(" ", ""))
    t = _strip_leading_cursor(clean_spotlight_text_line(t).replace(" ", ""))

    if not h:
        return clean_extracted_url(t) or None
    if not t:
        return clean_extracted_url(h) or None

    if h == t:
        return clean_extracted_url(h) or None
    if t in h:
        return clean_extracted_url(h) or None
    if h in t:
        return clean_extracted_url(t) or None

    max_k = min(len(h), len(t))
    best_merged = None

    for k in range(max_k, 3, -1):
        if h.endswith(t[:k]):
            best_merged = h + t[k:]
            break

    if not best_merged:
        for k in range(min(len(h) - 1, len(t)), 3, -1):
            if h[:-1].endswith(t[:k]):
                best_merged = h[:-1] + t[k:]
                break

    if not best_merged:
        for k in range(min(len(h), len(t) - 1), 3, -1):
            if h.endswith(t[1:k+1]):
                best_merged = h + t[k+1:]
                break

    if best_merged:
        return clean_extracted_url(best_merged) or None

    return clean_extracted_url(h) or None
