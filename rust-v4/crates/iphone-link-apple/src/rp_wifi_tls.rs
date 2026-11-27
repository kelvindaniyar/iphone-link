//! Pure-Rust Apple RemotePairing Wi-Fi Client (`_remotepairing._tcp.local.`)
//! and TLS 1.2 PSK (`0x00A8 TLS_PSK_WITH_AES_128_GCM_SHA256`) CDTunnel transport.
//!
//! Uses Windows CNG (`win_cng` -> `bcrypt.dll`) for hardware-accelerated SHA-256,
//! SHA-512, HMAC, HKDF, CSPRNG, and AES-128-GCM, plus original pure-Rust
//! Curve25519 (X25519 & Ed25519) and ChaCha20-Poly1305 for Apple Pair-Verify.

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::pin::Pin;
use std::sync::OnceLock;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use idevice::tunnel::CdTunnel;
use idevice::ReadWrite;
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::TcpStream;

use crate::win_cng::{
    fill_random, hkdf_sha512_32, sha256, sha512, sha512_chunks, tls12_prf_sha256, AesGcmKey,
};

// ============================================================================
// 1. GF(2^255 - 19) Arithmetic, X25519 & Ed25519 Sign
// ============================================================================

const MASK51: u64 = (1u64 << 51) - 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Fe([u64; 5]);

impl Fe {
    const ZERO: Self = Self([0, 0, 0, 0, 0]);
    const ONE: Self = Self([1, 0, 0, 0, 0]);

    fn from_u64(v: u64) -> Self {
        Self([v & MASK51, v >> 51, 0, 0, 0])
    }

    fn from_bytes(b: &[u8; 32]) -> Self {
        let load8 = |off: usize| -> u64 {
            u64::from_le_bytes([
                b[off],
                b[off + 1],
                b[off + 2],
                b[off + 3],
                b[off + 4],
                b[off + 5],
                b[off + 6],
                b[off + 7],
            ])
        };
        let l0 = load8(0) & MASK51;
        let l1 = (load8(6) >> 3) & MASK51;
        let l2 = (load8(12) >> 6) & MASK51;
        let l3 = (load8(19) >> 1) & MASK51;
        let l4 = (load8(24) >> 12) & MASK51;
        Self([l0, l1, l2, l3, l4])
    }

    fn reduce_weak(mut h: [u128; 5]) -> Self {
        for _ in 0..2 {
            let c0 = h[0] >> 51;
            h[0] &= MASK51 as u128;
            h[1] += c0;
            let c1 = h[1] >> 51;
            h[1] &= MASK51 as u128;
            h[2] += c1;
            let c2 = h[2] >> 51;
            h[2] &= MASK51 as u128;
            h[3] += c2;
            let c3 = h[3] >> 51;
            h[3] &= MASK51 as u128;
            h[4] += c3;
            let c4 = h[4] >> 51;
            h[4] &= MASK51 as u128;
            h[0] += c4 * 19;
        }
        Self([
            h[0] as u64,
            h[1] as u64,
            h[2] as u64,
            h[3] as u64,
            h[4] as u64,
        ])
    }

    fn add(self, o: Self) -> Self {
        Self::reduce_weak([
            self.0[0] as u128 + o.0[0] as u128,
            self.0[1] as u128 + o.0[1] as u128,
            self.0[2] as u128 + o.0[2] as u128,
            self.0[3] as u128 + o.0[3] as u128,
            self.0[4] as u128 + o.0[4] as u128,
        ])
    }

    fn sub(self, o: Self) -> Self {
        // Add 2*p so every limb stays non-negative before reduction
        const TWO_P0: u128 = ((1u128 << 51) - 19) * 2;
        const TWO_PN: u128 = ((1u128 << 51) - 1) * 2;
        Self::reduce_weak([
            self.0[0] as u128 + TWO_P0 - o.0[0] as u128,
            self.0[1] as u128 + TWO_PN - o.0[1] as u128,
            self.0[2] as u128 + TWO_PN - o.0[2] as u128,
            self.0[3] as u128 + TWO_PN - o.0[3] as u128,
            self.0[4] as u128 + TWO_PN - o.0[4] as u128,
        ])
    }

    fn neg(self) -> Self {
        Self::ZERO.sub(self)
    }

    fn mul(self, o: Self) -> Self {
        let mut t = [0u128; 5];
        for i in 0..5 {
            let ai = self.0[i] as u128;
            for j in 0..5 {
                let prod = ai * (o.0[j] as u128);
                if i + j < 5 {
                    t[i + j] += prod;
                } else {
                    t[i + j - 5] += prod * 19;
                }
            }
        }
        Self::reduce_weak(t)
    }

    fn sq(self) -> Self {
        self.mul(self)
    }

    fn pow_bytes_le(self, exp: &[u8; 32]) -> Self {
        let mut acc = Self::ONE;
        let mut base = self;
        for byte in exp.iter() {
            for bit in 0..8 {
                if (byte >> bit) & 1 == 1 {
                    acc = acc.mul(base);
                }
                base = base.sq();
            }
        }
        acc
    }

    fn inv(self) -> Self {
        // p - 2 = 2^255 - 21
        let mut exp = [0xFFu8; 32];
        exp[0] = 0xEB; // 256 - 21 = 235 = 0xEB
        exp[31] = 0x7F;
        self.pow_bytes_le(&exp)
    }

    fn to_bytes(self) -> [u8; 32] {
        let r = Self::reduce_weak([
            self.0[0] as u128,
            self.0[1] as u128,
            self.0[2] as u128,
            self.0[3] as u128,
            self.0[4] as u128,
        ]);
        // Canonical reduction mod p = 2^255 - 19
        let mut t = r.0;
        let mut q = (t[0] + 19) >> 51;
        q = (t[1] + q) >> 51;
        q = (t[2] + q) >> 51;
        q = (t[3] + q) >> 51;
        q = (t[4] + q) >> 51;
        t[0] += 19 * q;
        let mut carry = t[0] >> 51;
        t[0] &= MASK51;
        for limb in &mut t[1..5] {
            *limb += carry;
            carry = *limb >> 51;
            *limb &= MASK51;
        }
        let mut out = [0u8; 32];
        // Pack 5x51-bit limbs into 32 little-endian bytes
        for bit_idx in 0..255 {
            let limb = bit_idx / 51;
            let off = bit_idx % 51;
            if (t[limb] >> off) & 1 == 1 {
                out[bit_idx / 8] |= 1u8 << (bit_idx % 8);
            }
        }
        out
    }

    fn sqrt_ed(self) -> Option<Self> {
        // (p + 3) / 8 = 2^252 - 2
        let mut exp_p3_8 = [0xFFu8; 32];
        exp_p3_8[0] = 0xFE;
        exp_p3_8[31] = 0x0F;
        let cand = self.pow_bytes_le(&exp_p3_8);
        if cand.sq().to_bytes() == self.to_bytes() {
            return Some(cand);
        }
        // Multiply by sqrt(-1) = 2^((p - 1) / 4)
        let mut exp_i = [0xFFu8; 32];
        exp_i[0] = 0xFB;
        exp_i[31] = 0x1F;
        let sqrt_m1 = Fe::from_u64(2).pow_bytes_le(&exp_i);
        let cand2 = cand.mul(sqrt_m1);
        if cand2.sq().to_bytes() == self.to_bytes() {
            Some(cand2)
        } else {
            None
        }
    }
}

pub fn x25519(scalar_in: &[u8; 32], u_in: &[u8; 32]) -> [u8; 32] {
    let mut k = *scalar_in;
    k[0] &= 248;
    k[31] &= 127;
    k[31] |= 64;

    let mut u_masked = *u_in;
    u_masked[31] &= 127;
    let x_1 = Fe::from_bytes(&u_masked);
    let mut x_2 = Fe::ONE;
    let mut z_2 = Fe::ZERO;
    let mut x_3 = x_1;
    let mut z_3 = Fe::ONE;
    let a24 = Fe::from_u64(121_665);

    let mut swap = 0u64;
    for t in (0..255).rev() {
        let kt = u64::from((k[t / 8] >> (t % 8)) & 1);
        swap ^= kt;
        if swap != 0 {
            std::mem::swap(&mut x_2, &mut x_3);
            std::mem::swap(&mut z_2, &mut z_3);
        }
        swap = kt;

        let a = x_2.add(z_2);
        let aa = a.sq();
        let b = x_2.sub(z_2);
        let bb = b.sq();
        let e = aa.sub(bb);
        let c = x_3.add(z_3);
        let d = x_3.sub(z_3);
        let da = d.mul(a);
        let cb = c.mul(b);
        x_3 = da.add(cb).sq();
        z_3 = x_1.mul(da.sub(cb).sq());
        x_2 = aa.mul(bb);
        z_2 = e.mul(aa.add(a24.mul(e)));
    }
    if swap != 0 {
        std::mem::swap(&mut x_2, &mut x_3);
        std::mem::swap(&mut z_2, &mut z_3);
    }
    x_2.mul(z_2.inv()).to_bytes()
}

pub fn x25519_public(priv_key: &[u8; 32]) -> [u8; 32] {
    let mut base = [0u8; 32];
    base[0] = 9;
    x25519(priv_key, &base)
}

#[derive(Clone, Copy)]
struct EdPoint {
    x: Fe,
    y: Fe,
    z: Fe,
    t: Fe,
}

struct EdParams {
    d2: Fe,
    base: EdPoint,
}

fn ed_params() -> &'static EdParams {
    static PARAMS: OnceLock<EdParams> = OnceLock::new();
    PARAMS.get_or_init(|| {
        let d = Fe::from_u64(121_665).neg().mul(Fe::from_u64(121_666).inv());
        let d2 = d.add(d);
        let by = Fe::from_u64(4).mul(Fe::from_u64(5).inv());
        let by2 = by.sq();
        let num = by2.sub(Fe::ONE);
        let den = d.mul(by2).add(Fe::ONE);
        let mut bx = num.mul(den.inv()).sqrt_ed().expect("Ed25519 base x");
        if (bx.to_bytes()[0] & 1) == 1 {
            bx = bx.neg();
        }
        EdParams {
            d2,
            base: EdPoint {
                x: bx,
                y: by,
                z: Fe::ONE,
                t: bx.mul(by),
            },
        }
    })
}

impl EdPoint {
    const IDENTITY: Self = Self {
        x: Fe::ZERO,
        y: Fe::ONE,
        z: Fe::ONE,
        t: Fe::ZERO,
    };

    fn add(self, o: Self) -> Self {
        let d2 = ed_params().d2;
        let a = self.y.sub(self.x).mul(o.y.sub(o.x));
        let b = self.y.add(self.x).mul(o.y.add(o.x));
        let c = self.t.mul(d2).mul(o.t);
        let d = self.z.mul(o.z);
        let d = d.add(d);
        let e = b.sub(a);
        let f = d.sub(c);
        let g = d.add(c);
        let h = b.add(a);
        Self {
            x: e.mul(f),
            y: g.mul(h),
            z: f.mul(g),
            t: e.mul(h),
        }
    }

    fn double(self) -> Self {
        self.add(self)
    }

    fn encode(self) -> [u8; 32] {
        let zi = self.z.inv();
        let x = self.x.mul(zi).to_bytes();
        let mut y = self.y.mul(zi).to_bytes();
        y[31] |= (x[0] & 1) << 7;
        y
    }
}

fn ed_scalar_mul_base(scalar_le: &[u8; 32]) -> EdPoint {
    let mut acc = EdPoint::IDENTITY;
    let mut cur = ed_params().base;
    for byte in scalar_le.iter() {
        for bit in 0..8 {
            if (byte >> bit) & 1 == 1 {
                acc = acc.add(cur);
            }
            cur = cur.double();
        }
    }
    acc
}

// Ed25519 group order L = 2^252 + 27742317777372353535851937790883648493 (little-endian u64 limbs)
const ED_L: [u64; 4] = [
    0x5812_631A_5CF5_D3ED,
    0x14DE_F9DE_A2F7_9CD6,
    0x0000_0000_0000_0000,
    0x1000_0000_0000_0000,
];

fn u256_cmp(a: &[u64; 4], b: &[u64; 4]) -> std::cmp::Ordering {
    for i in (0..4).rev() {
        match a[i].cmp(&b[i]) {
            std::cmp::Ordering::Equal => continue,
            ord => return ord,
        }
    }
    std::cmp::Ordering::Equal
}

fn u256_sub_assign(a: &mut [u64; 4], b: &[u64; 4]) {
    let mut borrow = 0u128;
    for i in 0..4 {
        let ai = a[i] as u128;
        let bi = b[i] as u128 + borrow;
        if ai >= bi {
            a[i] = (ai - bi) as u64;
            borrow = 0;
        } else {
            a[i] = ((1u128 << 64) + ai - bi) as u64;
            borrow = 1;
        }
    }
}

fn scalar_reduce_512_mod_l(bytes_le: &[u8; 64]) -> [u64; 4] {
    let mut rem = [0u64; 4];
    for bit_idx in (0..512).rev() {
        let bit = u64::from((bytes_le[bit_idx / 8] >> (bit_idx % 8)) & 1);
        let mut carry = bit;
        for limb in &mut rem {
            let next_c = *limb >> 63;
            *limb = (*limb << 1) | carry;
            carry = next_c;
        }
        if carry != 0 || u256_cmp(&rem, &ED_L) != std::cmp::Ordering::Less {
            u256_sub_assign(&mut rem, &ED_L);
        }
    }
    rem
}

fn u256_to_le_bytes(v: &[u64; 4]) -> [u8; 32] {
    let mut out = [0u8; 32];
    for (i, &w) in v.iter().enumerate() {
        out[i * 8..(i + 1) * 8].copy_from_slice(&w.to_le_bytes());
    }
    out
}

fn scalar_mul_add_mod_l(r: &[u64; 4], k: &[u64; 4], a: &[u64; 4]) -> [u8; 32] {
    // Tahap 253 FIX: Knuth Algorithm M schoolbook multiply-accumulate with
    // per-step carry propagation into 8x u64 limbs (512 bits).
    // Previously `prod[i + j] += (k[i] as u128) * (a[j] as u128)` accumulated
    // up to 4 full u128 products without propagating carries, overflowing
    // u128::MAX (~50%+ of random Ed25519 Pair-Verify handshakes) — panicking
    // in debug builds and silently wrapping modulo 2^128 in release builds,
    // which produced invalid Ed25519 signatures and caused iPhone to drop
    // AirPlay / RemotePairing on Click #1!
    // Invariant: ki*aj + wide[i+j] + carry <= (2^64-1)^2 + 2*(2^64-1) == u128::MAX.
    let mut wide_u64 = [0u64; 8];
    wide_u64[..4].copy_from_slice(r);
    for i in 0..4 {
        let mut carry = 0u128;
        let ki = k[i] as u128;
        for j in 0..4 {
            let acc = ki * (a[j] as u128) + (wide_u64[i + j] as u128) + carry;
            wide_u64[i + j] = acc as u64;
            carry = acc >> 64;
        }
        let mut idx = i + 4;
        while carry != 0 && idx < 8 {
            let sum = (wide_u64[idx] as u128) + carry;
            wide_u64[idx] = sum as u64;
            carry = sum >> 64;
            idx += 1;
        }
    }
    let mut wide_bytes = [0u8; 64];
    for (i, &w) in wide_u64.iter().enumerate() {
        wide_bytes[i * 8..(i + 1) * 8].copy_from_slice(&w.to_le_bytes());
    }
    let reduced = scalar_reduce_512_mod_l(&wide_bytes);
    u256_to_le_bytes(&reduced)
}

pub fn ed25519_public_from_seed(seed: &[u8; 32]) -> [u8; 32] {
    let digest = sha512(seed);
    let mut a = [0u8; 32];
    a.copy_from_slice(&digest[..32]);
    a[0] &= 248;
    a[31] &= 63;
    a[31] |= 64;
    ed_scalar_mul_base(&a).encode()
}

pub fn ed25519_sign(seed: &[u8; 32], message: &[u8]) -> [u8; 64] {
    let digest = sha512(seed);
    let mut a_bytes = [0u8; 32];
    a_bytes.copy_from_slice(&digest[..32]);
    a_bytes[0] &= 248;
    a_bytes[31] &= 63;
    a_bytes[31] |= 64;
    let prefix = &digest[32..64];

    let pub_key = ed_scalar_mul_base(&a_bytes).encode();

    let r_hash = sha512_chunks(&[prefix, message]);
    let r_lim = scalar_reduce_512_mod_l(&r_hash);
    let r_bytes = u256_to_le_bytes(&r_lim);
    let cap_r = ed_scalar_mul_base(&r_bytes).encode();

    let k_hash = sha512_chunks(&[&cap_r, &pub_key, message]);
    let k_lim = scalar_reduce_512_mod_l(&k_hash);

    let mut a_lim = [0u64; 4];
    for i in 0..4 {
        a_lim[i] = u64::from_le_bytes(a_bytes[i * 8..(i + 1) * 8].try_into().unwrap());
    }
    let s_bytes = scalar_mul_add_mod_l(&r_lim, &k_lim, &a_lim);

    let mut sig = [0u8; 64];
    sig[..32].copy_from_slice(&cap_r);
    sig[32..64].copy_from_slice(&s_bytes);
    sig
}

// ============================================================================
// 2. ChaCha20-Poly1305 AEAD (RFC 8439)
// ============================================================================

#[inline]
fn cc_qr(s: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    s[a] = s[a].wrapping_add(s[b]);
    s[d] ^= s[a];
    s[d] = s[d].rotate_left(16);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] ^= s[c];
    s[b] = s[b].rotate_left(12);
    s[a] = s[a].wrapping_add(s[b]);
    s[d] ^= s[a];
    s[d] = s[d].rotate_left(8);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] ^= s[c];
    s[b] = s[b].rotate_left(7);
}

fn chacha20_block(key: &[u8; 32], counter: u32, nonce: &[u8; 12]) -> [u8; 64] {
    let mut init = [0u32; 16];
    init[0] = u32::from_le_bytes(*b"expa");
    init[1] = u32::from_le_bytes(*b"nd 3");
    init[2] = u32::from_le_bytes(*b"2-by");
    init[3] = u32::from_le_bytes(*b"te k");
    for i in 0..8 {
        init[4 + i] = u32::from_le_bytes(key[i * 4..(i + 1) * 4].try_into().unwrap());
    }
    init[12] = counter;
    init[13] = u32::from_le_bytes(nonce[0..4].try_into().unwrap());
    init[14] = u32::from_le_bytes(nonce[4..8].try_into().unwrap());
    init[15] = u32::from_le_bytes(nonce[8..12].try_into().unwrap());

    let mut st = init;
    for _ in 0..10 {
        cc_qr(&mut st, 0, 4, 8, 12);
        cc_qr(&mut st, 1, 5, 9, 13);
        cc_qr(&mut st, 2, 6, 10, 14);
        cc_qr(&mut st, 3, 7, 11, 15);
        cc_qr(&mut st, 0, 5, 10, 15);
        cc_qr(&mut st, 1, 6, 11, 12);
        cc_qr(&mut st, 2, 7, 8, 13);
        cc_qr(&mut st, 3, 4, 9, 14);
    }
    let mut out = [0u8; 64];
    for i in 0..16 {
        let w = st[i].wrapping_add(init[i]);
        out[i * 4..(i + 1) * 4].copy_from_slice(&w.to_le_bytes());
    }
    out
}

fn chacha20_xor(key: &[u8; 32], start_counter: u32, nonce: &[u8; 12], input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    for (idx, chunk) in input.chunks(64).enumerate() {
        let ks = chacha20_block(key, start_counter.wrapping_add(idx as u32), nonce);
        for (b, k) in chunk.iter().zip(ks.iter()) {
            out.push(b ^ k);
        }
    }
    out
}

fn poly1305_mac(otk: &[u8; 32], msg: &[u8]) -> [u8; 16] {
    let mut r_bytes = [0u8; 16];
    r_bytes.copy_from_slice(&otk[..16]);
    r_bytes[3] &= 15;
    r_bytes[7] &= 15;
    r_bytes[11] &= 15;
    r_bytes[15] &= 15;
    r_bytes[4] &= 252;
    r_bytes[8] &= 252;
    r_bytes[12] &= 252;

    let r0 = (u32::from_le_bytes(r_bytes[0..4].try_into().unwrap()) & 0x03FF_FFFF) as u64;
    let r1 = ((u32::from_le_bytes(r_bytes[3..7].try_into().unwrap()) >> 2) & 0x03FF_FF03) as u64;
    let r2 = ((u32::from_le_bytes(r_bytes[6..10].try_into().unwrap()) >> 4) & 0x03FF_C0FF) as u64;
    let r3 = ((u32::from_le_bytes(r_bytes[9..13].try_into().unwrap()) >> 6) & 0x03F0_3FFF) as u64;
    let r4 = ((u32::from_le_bytes(r_bytes[12..16].try_into().unwrap()) >> 8) & 0x000F_FFFF) as u64;

    let s1 = r1 * 5;
    let s2 = r2 * 5;
    let s3 = r3 * 5;
    let s4 = r4 * 5;

    let (mut h0, mut h1, mut h2, mut h3, mut h4) = (0u64, 0u64, 0u64, 0u64, 0u64);

    for chunk in msg.chunks(16) {
        let mut blk = [0u8; 17];
        blk[..chunk.len()].copy_from_slice(chunk);
        blk[chunk.len()] = 1;

        h0 += (u32::from_le_bytes(blk[0..4].try_into().unwrap()) & 0x03FF_FFFF) as u64;
        h1 += ((u32::from_le_bytes(blk[3..7].try_into().unwrap()) >> 2) & 0x03FF_FFFF) as u64;
        h2 += ((u32::from_le_bytes(blk[6..10].try_into().unwrap()) >> 4) & 0x03FF_FFFF) as u64;
        h3 += ((u32::from_le_bytes(blk[9..13].try_into().unwrap()) >> 6) & 0x03FF_FFFF) as u64;
        h4 += ((u32::from_le_bytes(blk[12..16].try_into().unwrap()) >> 8) as u64)
            | ((blk[16] as u64) << 24);

        let d0 = (h0 as u128) * (r0 as u128)
            + (h1 as u128) * (s4 as u128)
            + (h2 as u128) * (s3 as u128)
            + (h3 as u128) * (s2 as u128)
            + (h4 as u128) * (s1 as u128);
        let mut d1 = (h0 as u128) * (r1 as u128)
            + (h1 as u128) * (r0 as u128)
            + (h2 as u128) * (s4 as u128)
            + (h3 as u128) * (s3 as u128)
            + (h4 as u128) * (s2 as u128);
        let mut d2 = (h0 as u128) * (r2 as u128)
            + (h1 as u128) * (r1 as u128)
            + (h2 as u128) * (r0 as u128)
            + (h3 as u128) * (s4 as u128)
            + (h4 as u128) * (s3 as u128);
        let mut d3 = (h0 as u128) * (r3 as u128)
            + (h1 as u128) * (r2 as u128)
            + (h2 as u128) * (r1 as u128)
            + (h3 as u128) * (r0 as u128)
            + (h4 as u128) * (s4 as u128);
        let mut d4 = (h0 as u128) * (r4 as u128)
            + (h1 as u128) * (r3 as u128)
            + (h2 as u128) * (r2 as u128)
            + (h3 as u128) * (r1 as u128)
            + (h4 as u128) * (r0 as u128);

        let c = d0 >> 26;
        h0 = (d0 as u64) & 0x03FF_FFFF;
        d1 += c;
        let c = d1 >> 26;
        h1 = (d1 as u64) & 0x03FF_FFFF;
        d2 += c;
        let c = d2 >> 26;
        h2 = (d2 as u64) & 0x03FF_FFFF;
        d3 += c;
        let c = d3 >> 26;
        h3 = (d3 as u64) & 0x03FF_FFFF;
        d4 += c;
        let c = d4 >> 26;
        h4 = (d4 as u64) & 0x03FF_FFFF;
        h0 += (c as u64) * 5;
        let c = h0 >> 26;
        h0 &= 0x03FF_FFFF;
        h1 += c;
    }

    let c = h1 >> 26;
    h1 &= 0x03FF_FFFF;
    h2 += c;
    let c = h2 >> 26;
    h2 &= 0x03FF_FFFF;
    h3 += c;
    let c = h3 >> 26;
    h3 &= 0x03FF_FFFF;
    h4 += c;
    let c = h4 >> 26;
    h4 &= 0x03FF_FFFF;
    h0 += c * 5;
    let c = h0 >> 26;
    h0 &= 0x03FF_FFFF;
    h1 += c;

    let mut g0 = h0.wrapping_add(5);
    let c = g0 >> 26;
    g0 &= 0x03FF_FFFF;
    let mut g1 = h1.wrapping_add(c);
    let c = g1 >> 26;
    g1 &= 0x03FF_FFFF;
    let mut g2 = h2.wrapping_add(c);
    let c = g2 >> 26;
    g2 &= 0x03FF_FFFF;
    let mut g3 = h3.wrapping_add(c);
    let c = g3 >> 26;
    g3 &= 0x03FF_FFFF;
    let g4 = h4.wrapping_add(c).wrapping_sub(1 << 26);

    let mask = (g4 >> 63).wrapping_sub(1);
    g0 &= mask;
    g1 &= mask;
    g2 &= mask;
    g3 &= mask;
    let g4_m = g4 & mask;
    let nmask = !mask;
    h0 = (h0 & nmask) | g0;
    h1 = (h1 & nmask) | g1;
    h2 = (h2 & nmask) | g2;
    h3 = (h3 & nmask) | g3;
    h4 = (h4 & nmask) | g4_m;

    let f0 = ((h0) | (h1 << 26)) as u32;
    let f1 = ((h1 >> 6) | (h2 << 20)) as u32;
    let f2 = ((h2 >> 12) | (h3 << 14)) as u32;
    let f3 = ((h3 >> 18) | (h4 << 8)) as u32;

    let mut acc: u64 = f0 as u64 + u32::from_le_bytes(otk[16..20].try_into().unwrap()) as u64;
    let w0 = acc as u32;
    acc = (acc >> 32) + f1 as u64 + u32::from_le_bytes(otk[20..24].try_into().unwrap()) as u64;
    let w1 = acc as u32;
    acc = (acc >> 32) + f2 as u64 + u32::from_le_bytes(otk[24..28].try_into().unwrap()) as u64;
    let w2 = acc as u32;
    acc = (acc >> 32) + f3 as u64 + u32::from_le_bytes(otk[28..32].try_into().unwrap()) as u64;
    let w3 = acc as u32;

    let mut tag = [0u8; 16];
    tag[0..4].copy_from_slice(&w0.to_le_bytes());
    tag[4..8].copy_from_slice(&w1.to_le_bytes());
    tag[8..12].copy_from_slice(&w2.to_le_bytes());
    tag[12..16].copy_from_slice(&w3.to_le_bytes());
    tag
}

fn poly1305_aead_input(aad: &[u8], ct: &[u8]) -> Vec<u8> {
    let pad16 = |n: usize| (16 - (n % 16)) % 16;
    let mut mac_data =
        Vec::with_capacity(aad.len() + pad16(aad.len()) + ct.len() + pad16(ct.len()) + 16);
    mac_data.extend_from_slice(aad);
    mac_data.resize(mac_data.len() + pad16(aad.len()), 0);
    mac_data.extend_from_slice(ct);
    mac_data.resize(mac_data.len() + pad16(ct.len()), 0);
    mac_data.extend_from_slice(&(aad.len() as u64).to_le_bytes());
    mac_data.extend_from_slice(&(ct.len() as u64).to_le_bytes());
    mac_data
}

pub fn chacha20_poly1305_encrypt(
    key: &[u8; 32],
    nonce: &[u8; 12],
    plaintext: &[u8],
    aad: &[u8],
) -> Vec<u8> {
    let block0 = chacha20_block(key, 0, nonce);
    let mut otk = [0u8; 32];
    otk.copy_from_slice(&block0[..32]);
    let mut ct = chacha20_xor(key, 1, nonce, plaintext);
    let mac_in = poly1305_aead_input(aad, &ct);
    let tag = poly1305_mac(&otk, &mac_in);
    ct.extend_from_slice(&tag);
    ct
}

pub fn chacha20_poly1305_decrypt(
    key: &[u8; 32],
    nonce: &[u8; 12],
    ciphertext_and_tag: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>, String> {
    if ciphertext_and_tag.len() < 16 {
        return Err("ChaCha20-Poly1305 ciphertext too short".into());
    }
    let split = ciphertext_and_tag.len() - 16;
    let ct = &ciphertext_and_tag[..split];
    let tag = &ciphertext_and_tag[split..];
    let block0 = chacha20_block(key, 0, nonce);
    let mut otk = [0u8; 32];
    otk.copy_from_slice(&block0[..32]);
    let mac_in = poly1305_aead_input(aad, ct);
    let expected = poly1305_mac(&otk, &mac_in);
    let mut diff = 0u8;
    for (a, b) in expected.iter().zip(tag.iter()) {
        diff |= a ^ b;
    }
    if diff != 0 {
        return Err("ChaCha20-Poly1305 authentication tag mismatch".into());
    }
    Ok(chacha20_xor(key, 1, nonce, ct))
}

// ============================================================================
// 3. TLS 1.2 PSK (`0x00A8 = TLS_PSK_WITH_AES_128_GCM_SHA256`) Stream
// ============================================================================

const TLS_RECORD_CHANGE_CIPHER: u8 = 0x14;
const TLS_RECORD_ALERT: u8 = 0x15;
const TLS_RECORD_HANDSHAKE: u8 = 0x16;
const TLS_RECORD_APPDATA: u8 = 0x17;

pub struct TlsPskStream<S = TcpStream> {
    inner: S,
    tx_key: AesGcmKey,
    rx_key: AesGcmKey,
    tx_iv: [u8; 4],
    rx_iv: [u8; 4],
    tx_seq: u64,
    rx_seq: u64,
    rx_cipher: Vec<u8>,
    rx_cipher_pos: usize,
    rx_plain: Vec<u8>,
    rx_plain_pos: usize,
    tx_cipher: Vec<u8>,
    tx_cipher_pos: usize,
    eof: bool,
}

#[inline]
fn compact_cursor_vec(buf: &mut Vec<u8>, pos: &mut usize) {
    if *pos >= buf.len() {
        buf.clear();
        *pos = 0;
    } else if *pos >= 32768 {
        let rem = buf.len() - *pos;
        buf.copy_within(*pos.., 0);
        buf.truncate(rem);
        *pos = 0;
    }
}

fn tls_record_aad(seq: u64, rec_type: u8, plain_len: usize) -> [u8; 13] {
    let mut aad = [0u8; 13];
    aad[0..8].copy_from_slice(&seq.to_be_bytes());
    aad[8] = rec_type;
    aad[9] = 0x03;
    aad[10] = 0x03;
    aad[11..13].copy_from_slice(&(plain_len as u16).to_be_bytes());
    aad
}

fn tls_encrypt_record(
    key: &AesGcmKey,
    iv4: &[u8; 4],
    seq: u64,
    rec_type: u8,
    plain: &[u8],
) -> Result<Vec<u8>, String> {
    let explicit_nonce = seq.to_be_bytes();
    let mut nonce = [0u8; 12];
    nonce[0..4].copy_from_slice(iv4);
    nonce[4..12].copy_from_slice(&explicit_nonce);
    let aad = tls_record_aad(seq, rec_type, plain.len());
    let (ct, tag) = key.encrypt(&nonce, &aad, plain)?;
    let body_len = 8 + ct.len() + 16;
    let mut rec = Vec::with_capacity(5 + body_len);
    rec.push(rec_type);
    rec.push(0x03);
    rec.push(0x03);
    rec.extend_from_slice(&(body_len as u16).to_be_bytes());
    rec.extend_from_slice(&explicit_nonce);
    rec.extend_from_slice(&ct);
    rec.extend_from_slice(&tag);
    Ok(rec)
}

fn tls_decrypt_record_into(
    key: &AesGcmKey,
    iv4: &[u8; 4],
    seq: u64,
    rec_type: u8,
    body: &[u8],
    out: &mut Vec<u8>,
) -> Result<usize, String> {
    if body.len() < 24 {
        return Err("TLS GCM record body too short".into());
    }
    let explicit_nonce = &body[0..8];
    let ct = &body[8..body.len() - 16];
    let mut tag = [0u8; 16];
    tag.copy_from_slice(&body[body.len() - 16..]);
    let mut nonce = [0u8; 12];
    nonce[0..4].copy_from_slice(iv4);
    nonce[4..12].copy_from_slice(explicit_nonce);
    let aad = tls_record_aad(seq, rec_type, ct.len());
    key.decrypt_into(&nonce, &aad, ct, &tag, out)
}

fn tls_decrypt_record(
    key: &AesGcmKey,
    iv4: &[u8; 4],
    seq: u64,
    rec_type: u8,
    body: &[u8],
) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(body.len().saturating_sub(24));
    tls_decrypt_record_into(key, iv4, seq, rec_type, body, &mut out)?;
    Ok(out)
}

async fn read_tls_record(stream: &mut TcpStream) -> Result<(u8, Vec<u8>), String> {
    let mut hdr = [0u8; 5];
    stream
        .read_exact(&mut hdr)
        .await
        .map_err(|e| format!("TLS record header read: {e}"))?;
    let rtype = hdr[0];
    let rlen = u16::from_be_bytes([hdr[3], hdr[4]]) as usize;
    if rlen > 18432 {
        return Err(format!("TLS record too large: {rlen}"));
    }
    let mut body = vec![0u8; rlen];
    stream
        .read_exact(&mut body)
        .await
        .map_err(|e| format!("TLS record body read: {e}"))?;
    Ok((rtype, body))
}

impl TlsPskStream {
    pub async fn connect(mut stream: TcpStream, psk: &[u8]) -> Result<Self, String> {
        let mut client_random = [0u8; 32];
        fill_random(&mut client_random)?;

        // 1. Send ClientHello offering TLS_PSK_WITH_AES_128_GCM_SHA256 (0x00A8)
        let mut ch_body = Vec::with_capacity(48);
        ch_body.extend_from_slice(&[0x03, 0x03]); // TLS 1.2
        ch_body.extend_from_slice(&client_random);
        ch_body.push(0x00); // session_id len
        ch_body.extend_from_slice(&2u16.to_be_bytes()); // cipher_suites len
        ch_body.extend_from_slice(&0x00A8u16.to_be_bytes()); // TLS_PSK_WITH_AES_128_GCM_SHA256
        ch_body.extend_from_slice(&[0x01, 0x00]); // compression methods: null

        let mut ch_hs = Vec::with_capacity(4 + ch_body.len());
        ch_hs.push(0x01); // ClientHello
        let ch_len = ch_body.len() as u32;
        ch_hs.extend_from_slice(&[(ch_len >> 16) as u8, (ch_len >> 8) as u8, ch_len as u8]);
        ch_hs.extend_from_slice(&ch_body);

        let mut transcript = Vec::with_capacity(512);
        transcript.extend_from_slice(&ch_hs);

        let mut ch_rec = Vec::with_capacity(5 + ch_hs.len());
        ch_rec.extend_from_slice(&[TLS_RECORD_HANDSHAKE, 0x03, 0x01]);
        ch_rec.extend_from_slice(&(ch_hs.len() as u16).to_be_bytes());
        ch_rec.extend_from_slice(&ch_hs);
        stream
            .write_all(&ch_rec)
            .await
            .map_err(|e| format!("TLS ClientHello write: {e}"))?;
        stream
            .flush()
            .await
            .map_err(|e| format!("TLS ClientHello flush: {e}"))?;

        // 2. Read ServerHello .. ServerHelloDone
        let mut server_random = [0u8; 32];
        let mut got_server_hello = false;
        let mut got_hello_done = false;
        let mut hs_acc = Vec::new();
        while !got_hello_done {
            let (rtype, body) = read_tls_record(&mut stream).await?;
            if rtype == TLS_RECORD_ALERT {
                return Err(format!("TLS Alert during handshake: {body:02X?}"));
            }
            if rtype != TLS_RECORD_HANDSHAKE {
                return Err(format!("Unexpected TLS record type {rtype} during handshake"));
            }
            hs_acc.extend_from_slice(&body);
            while hs_acc.len() >= 4 {
                let htype = hs_acc[0];
                let hlen = ((hs_acc[1] as usize) << 16)
                    | ((hs_acc[2] as usize) << 8)
                    | (hs_acc[3] as usize);
                if hs_acc.len() < 4 + hlen {
                    break;
                }
                let msg = hs_acc[..4 + hlen].to_vec();
                hs_acc.drain(..4 + hlen);
                transcript.extend_from_slice(&msg);
                match htype {
                    0x02 => {
                        // ServerHello
                        if msg.len() < 42 {
                            return Err("ServerHello too short".into());
                        }
                        server_random.copy_from_slice(&msg[6..38]);
                        let sid_len = msg[38] as usize;
                        if msg.len() < 41 + sid_len {
                            return Err("ServerHello truncated cipher".into());
                        }
                        let cs = u16::from_be_bytes([msg[39 + sid_len], msg[40 + sid_len]]);
                        if cs != 0x00A8 {
                            return Err(format!("Unsupported TLS-PSK cipher 0x{cs:04X}"));
                        }
                        got_server_hello = true;
                    }
                    0x0E => {
                        // ServerHelloDone
                        got_hello_done = true;
                    }
                    _ => {} // ServerKeyExchange (PSK hint) etc.
                }
            }
        }
        if !got_server_hello {
            return Err("Missing ServerHello".into());
        }

        // 3. RFC 4279 Pre-Master Secret & RFC 5246 Key Expansion
        let n = psk.len();
        let mut pms = Vec::with_capacity(4 + 2 * n);
        pms.extend_from_slice(&(n as u16).to_be_bytes());
        pms.resize(pms.len() + n, 0);
        pms.extend_from_slice(&(n as u16).to_be_bytes());
        pms.extend_from_slice(psk);

        let mut cr_sr = [0u8; 64];
        cr_sr[0..32].copy_from_slice(&client_random);
        cr_sr[32..64].copy_from_slice(&server_random);
        let master_secret = tls12_prf_sha256(&pms, b"master secret", &cr_sr, 48);

        let mut sr_cr = [0u8; 64];
        sr_cr[0..32].copy_from_slice(&server_random);
        sr_cr[32..64].copy_from_slice(&client_random);
        let key_block = tls12_prf_sha256(&master_secret, b"key expansion", &sr_cr, 40);

        let tx_key = AesGcmKey::new(&key_block[0..16])?;
        let rx_key = AesGcmKey::new(&key_block[16..32])?;
        let mut tx_iv = [0u8; 4];
        let mut rx_iv = [0u8; 4];
        tx_iv.copy_from_slice(&key_block[32..36]);
        rx_iv.copy_from_slice(&key_block[36..40]);

        // 4. Send ClientKeyExchange (empty PSK identity) + ChangeCipherSpec + Client Finished
        let cke_hs: [u8; 6] = [0x10, 0x00, 0x00, 0x02, 0x00, 0x00];
        transcript.extend_from_slice(&cke_hs);

        let tx_hash = sha256(&transcript);
        let verify_data = tls12_prf_sha256(&master_secret, b"client finished", &tx_hash, 12);
        let mut fin_hs = Vec::with_capacity(16);
        fin_hs.extend_from_slice(&[0x14, 0x00, 0x00, 0x0C]);
        fin_hs.extend_from_slice(&verify_data);
        transcript.extend_from_slice(&fin_hs);

        let fin_rec = tls_encrypt_record(&tx_key, &tx_iv, 0, TLS_RECORD_HANDSHAKE, &fin_hs)?;
        let mut flight = Vec::with_capacity(11 + 6 + fin_rec.len());
        flight.extend_from_slice(&[TLS_RECORD_HANDSHAKE, 0x03, 0x03, 0x00, 0x06]);
        flight.extend_from_slice(&cke_hs);
        flight.extend_from_slice(&[TLS_RECORD_CHANGE_CIPHER, 0x03, 0x03, 0x00, 0x01, 0x01]);
        flight.extend_from_slice(&fin_rec);
        stream
            .write_all(&flight)
            .await
            .map_err(|e| format!("TLS ClientFinished write: {e}"))?;
        stream
            .flush()
            .await
            .map_err(|e| format!("TLS ClientFinished flush: {e}"))?;

        // 5. Read Server ChangeCipherSpec + Server Finished
        let mut got_ccs = false;
        let mut rx_seq = 0u64;
        loop {
            let (rtype, body) = read_tls_record(&mut stream).await?;
            match rtype {
                TLS_RECORD_CHANGE_CIPHER => {
                    got_ccs = true;
                }
                TLS_RECORD_HANDSHAKE if got_ccs => {
                    let plain = tls_decrypt_record(&rx_key, &rx_iv, rx_seq, rtype, &body)?;
                    rx_seq += 1;
                    let srv_hash = sha256(&transcript);
                    let expected_srv =
                        tls12_prf_sha256(&master_secret, b"server finished", &srv_hash, 12);
                    if plain.len() < 16 || plain[0] != 0x14 || &plain[4..16] != expected_srv.as_slice()
                    {
                        return Err("TLS ServerFinished verify_data mismatch".into());
                    }
                    break;
                }
                TLS_RECORD_ALERT => {
                    return Err(format!("TLS Alert after Finished: {body:02X?}"));
                }
                _ => {}
            }
        }

        Ok(Self {
            inner: stream,
            tx_key,
            rx_key,
            tx_iv,
            rx_iv,
            tx_seq: 1,
            rx_seq,
            rx_cipher: Vec::with_capacity(65536),
            rx_cipher_pos: 0,
            rx_plain: Vec::with_capacity(65536),
            rx_plain_pos: 0,
            tx_cipher: Vec::with_capacity(32768),
            tx_cipher_pos: 0,
            eof: false,
        })
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> TlsPskStream<S> {
    fn flush_tx_cipher(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while self.tx_cipher_pos < self.tx_cipher.len() {
            match Pin::new(&mut self.inner).poll_write(cx, &self.tx_cipher[self.tx_cipher_pos..]) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(e))
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) =>
                {
                    return Poll::Pending;
                }
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Ready(Ok(0)) => {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "TLS-PSK socket closed during write",
                    )));
                }
                Poll::Ready(Ok(n)) => self.tx_cipher_pos += n,
            }
        }
        self.tx_cipher.clear();
        self.tx_cipher_pos = 0;
        Poll::Ready(Ok(()))
    }

    fn decrypt_buffered_records(&mut self) -> io::Result<()> {
        while self.rx_cipher.len().saturating_sub(self.rx_cipher_pos) >= 5 {
            let p = self.rx_cipher_pos;
            let rtype = self.rx_cipher[p];
            let rlen = u16::from_be_bytes([self.rx_cipher[p + 3], self.rx_cipher[p + 4]]) as usize;
            if rlen > 18432 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("TLS-PSK record length overflow: {rlen}"),
                ));
            }
            if self.rx_cipher.len().saturating_sub(p) < 5 + rlen {
                // Atomic partial record framing: keep untouched until remaining ciphertext arrives.
                break;
            }
            let rec_start = p + 5;
            let rec_end = rec_start + rlen;
            let seq = self.rx_seq;
            self.rx_cipher_pos = rec_end;
            self.rx_seq = seq.wrapping_add(1);
            match rtype {
                TLS_RECORD_APPDATA => {
                    tls_decrypt_record_into(
                        &self.rx_key,
                        &self.rx_iv,
                        seq,
                        rtype,
                        &self.rx_cipher[rec_start..rec_end],
                        &mut self.rx_plain,
                    )
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                }
                TLS_RECORD_ALERT => {
                    let plain = tls_decrypt_record(
                        &self.rx_key,
                        &self.rx_iv,
                        seq,
                        rtype,
                        &self.rx_cipher[rec_start..rec_end],
                    )
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                    if plain.len() >= 2 && plain[1] == 0 {
                        // close_notify (warning or fatal): mark clean EOF
                        self.eof = true;
                        break;
                    }
                    if plain.len() >= 2 && plain[0] == 1 {
                        // Non-fatal TLS warning alert: ignore cleanly without tearing down tunnel
                        continue;
                    }
                    return Err(io::Error::new(
                        io::ErrorKind::ConnectionReset,
                        format!("TLS-PSK fatal peer alert: {plain:02X?}"),
                    ));
                }
                _ => {
                    let _ = tls_decrypt_record(
                        &self.rx_key,
                        &self.rx_iv,
                        seq,
                        rtype,
                        &self.rx_cipher[rec_start..rec_end],
                    )
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                }
            }
        }
        compact_cursor_vec(&mut self.rx_cipher, &mut self.rx_cipher_pos);
        Ok(())
    }
}

impl<S> std::fmt::Debug for TlsPskStream<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TlsPskStream")
            .field("rx_plain_len", &self.rx_plain.len().saturating_sub(self.rx_plain_pos))
            .field("rx_cipher_len", &self.rx_cipher.len().saturating_sub(self.rx_cipher_pos))
            .field("tx_cipher_pending", &self.tx_cipher.len().saturating_sub(self.tx_cipher_pos))
            .field("eof", &self.eof)
            .finish()
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> AsyncRead for TlsPskStream<S> {
    fn set_stream_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.inner.set_stream_nonblocking(nonblocking)
    }

    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = &mut *self;
        loop {
            if this.rx_plain_pos < this.rx_plain.len() {
                let avail = this.rx_plain.len() - this.rx_plain_pos;
                let take = buf.unfilled_mut().len().min(avail);
                if take > 0 {
                    buf.put_slice(&this.rx_plain[this.rx_plain_pos..this.rx_plain_pos + take]);
                    this.rx_plain_pos += take;
                    compact_cursor_vec(&mut this.rx_plain, &mut this.rx_plain_pos);
                }
                return Poll::Ready(Ok(()));
            }
            if this.eof {
                return Poll::Ready(Ok(()));
            }
            if this.rx_cipher.len().saturating_sub(this.rx_cipher_pos) >= 5 {
                if let Err(e) = this.decrypt_buffered_records() {
                    return Poll::Ready(Err(e));
                }
                if this.rx_plain_pos < this.rx_plain.len() || this.eof {
                    continue;
                }
            }
            let mut raw = [0u8; 65536];
            let mut rb = ReadBuf::new(&mut raw);
            match Pin::new(&mut this.inner).poll_read(cx, &mut rb) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(e))
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) =>
                {
                    return Poll::Pending;
                }
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Ready(Ok(())) => {
                    let n = rb.filled().len();
                    if n == 0 {
                        this.eof = true;
                        return Poll::Ready(Ok(()));
                    }
                    this.rx_cipher.extend_from_slice(rb.filled());
                    if let Err(e) = this.decrypt_buffered_records() {
                        return Poll::Ready(Err(e));
                    }
                }
            }
        }
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> AsyncWrite for TlsPskStream<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = &mut *self;
        if let Poll::Pending = this.flush_tx_cipher(cx)? {
            return Poll::Pending;
        }
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let take = buf.len().min(16384);
        let rec = tls_encrypt_record(
            &this.tx_key,
            &this.tx_iv,
            this.tx_seq,
            TLS_RECORD_APPDATA,
            &buf[..take],
        )
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        this.tx_seq = this.tx_seq.wrapping_add(1);
        this.tx_cipher = rec;
        this.tx_cipher_pos = 0;
        match this.flush_tx_cipher(cx) {
            Poll::Ready(Ok(())) | Poll::Pending => Poll::Ready(Ok(take)),
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = &mut *self;
        match this.flush_tx_cipher(cx) {
            Poll::Ready(Ok(())) => Pin::new(&mut this.inner).poll_flush(cx),
            other => other,
        }
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = &mut *self;
        match this.flush_tx_cipher(cx) {
            Poll::Ready(Ok(())) => Pin::new(&mut this.inner).poll_shutdown(cx),
            other => other,
        }
    }
}

/// Non-blockingly drains incoming keepalive/event bytes from the Wi-Fi `RPPairing` control TCP
/// socket so its kernel receive window stays open and the socket stays alive for hours.
pub fn drain_rp_control_nonblocking(ctrl: &mut TcpStream) -> Result<usize, String> {
    use std::io::Read;
    let _ = ctrl.0.set_nonblocking(true);
    let mut total = 0usize;
    let mut buf = [0u8; 4096];
    for _ in 0..8 {
        match ctrl.0.read(&mut buf) {
            Ok(0) => return Err("RPPairing control TCP peer closed (EOF)".into()),
            Ok(n) => total += n,
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) =>
            {
                break;
            }
            Err(e) => return Err(format!("RPPairing control TCP error: {e}")),
        }
    }
    Ok(total)
}

// ============================================================================
// 4. Apple RemotePairing Control Protocol (`RPPairing`) + mDNS Discovery
// ============================================================================

const RP_MAGIC: &[u8; 9] = b"RPPairing";

fn b64_encode(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(((input.len() + 2) / 3) * 4);
    let mut i = 0;
    while i + 3 <= input.len() {
        let (a, b, c) = (input[i] as usize, input[i + 1] as usize, input[i + 2] as usize);
        out.push(T[a >> 2]);
        out.push(T[((a & 3) << 4) | (b >> 4)]);
        out.push(T[((b & 15) << 2) | (c >> 6)]);
        out.push(T[c & 63]);
        i += 3;
    }
    if i + 1 == input.len() {
        let a = input[i] as usize;
        out.push(T[a >> 2]);
        out.push(T[(a & 3) << 4]);
        out.push(b'=');
        out.push(b'=');
    } else if i + 2 == input.len() {
        let (a, b) = (input[i] as usize, input[i + 1] as usize);
        out.push(T[a >> 2]);
        out.push(T[((a & 3) << 4) | (b >> 4)]);
        out.push(T[(b & 15) << 2]);
        out.push(b'=');
    }
    String::from_utf8(out).unwrap_or_default()
}

fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let mut clean: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    while clean.len() % 4 != 0 {
        clean.push(b'=');
    }
    let val = |b: u8| -> Option<u8> {
        match b {
            b'A'..=b'Z' => Some(b - b'A'),
            b'a'..=b'z' => Some(b - b'a' + 26),
            b'0'..=b'9' => Some(b - b'0' + 52),
            b'+' | b'-' => Some(62),
            b'/' | b'_' => Some(63),
            _ => None,
        }
    };
    let mut out = Vec::with_capacity((clean.len() / 4) * 3);
    for ch in clean.chunks_exact(4) {
        let a = val(ch[0])?;
        let b = val(ch[1])?;
        out.push((a << 2) | (b >> 4));
        if ch[2] != b'=' {
            let c = val(ch[2])?;
            out.push(((b & 15) << 4) | (c >> 2));
            if ch[3] != b'=' {
                let d = val(ch[3])?;
                out.push(((c & 3) << 6) | d);
            }
        }
    }
    Some(out)
}

fn tlv8_encode(items: &[(u8, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    for &(ty, data) in items {
        if data.is_empty() {
            out.push(ty);
            out.push(0);
        } else {
            for chunk in data.chunks(255) {
                out.push(ty);
                out.push(chunk.len() as u8);
                out.extend_from_slice(chunk);
            }
        }
    }
    out
}

fn tlv8_decode(mut data: &[u8]) -> std::collections::HashMap<u8, Vec<u8>> {
    let mut map: std::collections::HashMap<u8, Vec<u8>> = std::collections::HashMap::new();
    while data.len() >= 2 {
        let ty = data[0];
        let len = data[1] as usize;
        if data.len() < 2 + len {
            break;
        }
        map.entry(ty).or_default().extend_from_slice(&data[2..2 + len]);
        data = &data[2 + len..];
    }
    map
}

#[derive(Debug, Clone)]
pub struct MdnsRpEndpoint {
    pub ip: Ipv4Addr,
    pub port: u16,
    pub identifier: String,
    pub auth_tag: String,
    pub host_name: String,
}

fn read_dns_name(buf: &[u8], mut off: usize) -> String {
    let mut labels: Vec<String> = Vec::new();
    let mut hops = 0usize;
    while off < buf.len() && hops < 64 {
        let l = buf[off];
        if l == 0 {
            break;
        }
        if (l & 0xC0) == 0xC0 {
            if off + 1 >= buf.len() {
                break;
            }
            let ptr = (((l & 0x3F) as usize) << 8) | (buf[off + 1] as usize);
            off = ptr;
            hops += 1;
            continue;
        }
        let len = l as usize;
        off += 1;
        if off + len > buf.len() {
            break;
        }
        labels.push(String::from_utf8_lossy(&buf[off..off + len]).into_owned());
        off += len;
        hops += 1;
    }
    let full = labels.join(".");
    let stripped = full
        .trim_end_matches('.')
        .strip_suffix(".local")
        .unwrap_or(full.trim_end_matches('.'));
    stripped.to_string()
}

fn parse_mdns_rp_packet(buf: &[u8], fallback_ip: Ipv4Addr) -> Option<MdnsRpEndpoint> {
    if buf.len() < 12 {
        return None;
    }
    let qd = u16::from_be_bytes([buf[4], buf[5]]) as usize;
    let an = u16::from_be_bytes([buf[6], buf[7]]) as usize;
    let ns = u16::from_be_bytes([buf[8], buf[9]]) as usize;
    let ar = u16::from_be_bytes([buf[10], buf[11]]) as usize;

    let skip_name = |mut off: usize| -> Option<usize> {
        let mut hops = 0;
        while off < buf.len() && hops < 64 {
            let l = buf[off];
            if l == 0 {
                return Some(off + 1);
            }
            if (l & 0xC0) == 0xC0 {
                return Some(off + 2);
            }
            off = off.checked_add(1 + l as usize)?;
            hops += 1;
        }
        None
    };

    let mut off = 12usize;
    for _ in 0..qd {
        off = skip_name(off)?.checked_add(4)?;
    }

    let mut srv_port = 0u16;
    let mut a_ip = fallback_ip;
    let mut ident = String::new();
    let mut auth_tag = String::new();
    let mut host_name = String::new();

    for _ in 0..(an + ns + ar) {
        if off >= buf.len() {
            break;
        }
        off = skip_name(off)?;
        if off + 10 > buf.len() {
            break;
        }
        let rtype = u16::from_be_bytes([buf[off], buf[off + 1]]);
        let rdlen = u16::from_be_bytes([buf[off + 8], buf[off + 9]]) as usize;
        off += 10;
        if off + rdlen > buf.len() {
            break;
        }
        let rdata_start = off;
        let rdata = &buf[off..off + rdlen];
        off += rdlen;
        match rtype {
            33 if rdlen >= 6 => {
                srv_port = u16::from_be_bytes([rdata[4], rdata[5]]);
                if rdlen > 6 && host_name.is_empty() {
                    host_name = read_dns_name(buf, rdata_start + 6);
                }
            }
            1 if rdlen == 4 => {
                a_ip = Ipv4Addr::new(rdata[0], rdata[1], rdata[2], rdata[3]);
            }
            16 => {
                let mut p = 0usize;
                while p < rdata.len() {
                    let tl = rdata[p] as usize;
                    p += 1;
                    if p + tl > rdata.len() {
                        break;
                    }
                    let kv = String::from_utf8_lossy(&rdata[p..p + tl]);
                    if let Some((k, v)) = kv.split_once('=') {
                        if k == "identifier" {
                            ident = v.to_string();
                        } else if k == "authTag" {
                            auth_tag = v.to_string();
                        }
                    }
                    p += tl;
                }
            }
            _ => {}
        }
    }
    if srv_port > 0 {
        Some(MdnsRpEndpoint {
            ip: a_ip,
            port: srv_port,
            identifier: ident,
            auth_tag,
            host_name,
        })
    } else {
        None
    }
}

use std::sync::atomic::{AtomicU64, Ordering};

static LAST_WIFI_SESSION_END_MS: AtomicU64 = AtomicU64::new(0);

fn now_epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Tahap 250 Pilar 2: Bounded TCP connect timeout for stale/cached/fallback ephemeral ports (`<= 400 ms`).
pub const FAST_WIFI_CACHED_PORT_TCP_TIMEOUT_MS: u64 = 400;
/// Tahap 250 Pilar 2: TCP connect timeout for fresh `alt_irk`-verified mDNS targets (`1800 ms`).
pub const FAST_WIFI_VERIFIED_MDNS_TCP_TIMEOUT_MS: u64 = 1_800;
/// Tahap 250 Pilar 2: Window after a previous Wi-Fi session ended (`12 s`) during which stale
/// `cached_port` MUST be skipped in favor of fresh mDNS discovery.
pub const FAST_WIFI_POST_STOP_SKIP_CACHED_PORT_WINDOW_MS: u64 = 12_000;

/// Tahap 250 Pilar 2: Pure mDNS early-break policy.
/// - When `has_target_alt_irk == true`: break IMMEDIATELY ONLY when an `alt_irk`-verified endpoint
///   matches (never cut discovery short on unverified responses from another iPhone on the LAN!).
/// - When `has_target_alt_irk == false`: allow early-break after second burst (`>= 240 ms`).
pub fn should_early_break_mdns_discovery(
    has_target_alt_irk: bool,
    verified_match_found: bool,
    unverified_count: usize,
    elapsed_ms: u64,
) -> bool {
    if has_target_alt_irk {
        verified_match_found
    } else {
        unverified_count > 0 && elapsed_ms >= 240
    }
}

/// Tahap 250 Pilar 2: Determines whether Round 1 should try a saved `cached_port`.
/// Skips `cached_port` whenever:
/// - `round > 1`, OR
/// - no `cached_port` exists, OR
/// - a fresh `alt_irk`-verified mDNS target was already discovered, OR
/// - the previous Wi-Fi session ended `< 12_000 ms` ago (iOS `remoted` rotates its ephemeral port!).
pub fn should_try_cached_rp_port(
    round: u8,
    cached_port: Option<u16>,
    elapsed_since_last_wifi_end_ms: Option<u64>,
    has_verified_mdns_target: bool,
) -> bool {
    if round != 1 || cached_port.is_none() || has_verified_mdns_target {
        return false;
    }
    if let Some(el) = elapsed_since_last_wifi_end_ms {
        if el < FAST_WIFI_POST_STOP_SKIP_CACHED_PORT_WINDOW_MS {
            return false;
        }
    }
    true
}

/// Tahap 250 Pilar 2: Returns bounded TCP connect timeout (`400 ms` for cached/fallback ports,
/// `1800 ms` for fresh `alt_irk`-verified mDNS targets).
pub fn rp_tcp_connect_timeout_ms(is_cached_or_fallback_port: bool) -> u64 {
    if is_cached_or_fallback_port {
        FAST_WIFI_CACHED_PORT_TCP_TIMEOUT_MS
    } else {
        FAST_WIFI_VERIFIED_MDNS_TCP_TIMEOUT_MS
    }
}

/// Records when a physical USB/Wi-Fi session ended so a back-to-back 1-click Wi-Fi reconnect
/// gives iOS `remoted` a brief settle window (`200 ms`) before opening a fresh tunnel.
pub fn mark_wifi_session_ended() {
    LAST_WIFI_SESSION_END_MS.store(now_epoch_ms(), Ordering::SeqCst);
}

/// Discovers live `_remotepairing._tcp.local.` endpoints across ALL non-loopback non-APIPA IPv4 LAN
/// interfaces via 2x multicast (`224.0.0.251:5353`) + direct unicast (`hint_ip:5353`) bursts spaced
/// `150 ms` apart, prioritizing endpoints whose `TXT` `authTag` validates against `alt_irk`.
pub fn discover_remotepairing_mdns(
    hint_ip: Option<&str>,
    alt_irk: Option<&[u8; 16]>,
    timeout_ms: u64,
) -> Vec<MdnsRpEndpoint> {
    if crate::connection::is_loopback_only_test_mode() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut sockets: Vec<UdpSocket> = Vec::new();

    // Bind one non-blocking UDP socket per active LAN IPv4 adapter so Windows multi-adapter setups
    // (Wi-Fi + Ethernet + WSL/VPN) always transmit & receive mDNS on the real Wi-Fi LAN!
    for ip_str in crate::wifi::local_ipv4_addresses() {
        if let Ok(local_v4) = ip_str.parse::<Ipv4Addr>() {
            if local_v4.is_loopback() || (local_v4.octets()[0] == 169 && local_v4.octets()[1] == 254) {
                continue;
            }
            if let Ok(s) = UdpSocket::bind(SocketAddr::new(IpAddr::V4(local_v4), 0)) {
                let _ = s.set_nonblocking(true);
                sockets.push(s);
            }
        }
    }
    if let Ok(any_sock) = UdpSocket::bind("0.0.0.0:0") {
        let _ = any_sock.set_nonblocking(true);
        sockets.push(any_sock);
    }
    if sockets.is_empty() {
        return out;
    }

    let mut q = vec![0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
    for label in [b"_remotepairing".as_slice(), b"_tcp".as_slice(), b"local".as_slice()] {
        q.push(label.len() as u8);
        q.extend_from_slice(label);
    }
    q.extend_from_slice(&[0x00, 0x00, 0x0C, 0x80, 0x01]); // PTR, QU=1

    let mcast_dst = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(224, 0, 0, 251)), 5353);
    let unicast_dst = hint_ip
        .and_then(|s| s.trim().parse::<Ipv4Addr>().ok())
        .map(|ip4| SocketAddr::new(IpAddr::V4(ip4), 5353));

    let send_burst = |socks: &[UdpSocket]| {
        for s in socks {
            if let Some(u_dst) = unicast_dst {
                let _ = s.send_to(&q, u_dst);
            }
            let _ = s.send_to(&q, mcast_dst);
        }
    };

    // Tahap 250 Pilar 2: Aggressive 3-burst cadence (0 ms, 120 ms, 360 ms) + instant early-break
    // as soon as 1 alt_irk-verified target endpoint arrives (cuts average discovery to < 150 ms!).
    send_burst(&sockets);
    let started = Instant::now();
    let deadline = started + Duration::from_millis(timeout_ms.max(250));
    let mut sent_second_burst = false;
    let mut sent_third_burst = false;
    let mut verified_found = false;
    let mut buf = [0u8; 4096];

    while Instant::now() < deadline {
        if !sent_second_burst && started.elapsed() >= Duration::from_millis(120) {
            send_burst(&sockets);
            sent_second_burst = true;
        }
        if !sent_third_burst
            && timeout_ms >= 600
            && !verified_found
            && started.elapsed() >= Duration::from_millis(360)
        {
            send_burst(&sockets);
            sent_third_burst = true;
        }
        let mut got_any_pkt = false;
        for s in &sockets {
            loop {
                match s.recv_from(&mut buf) {
                    Ok((n, src)) => {
                        got_any_pkt = true;
                        let fallback_ip = match src.ip() {
                            IpAddr::V4(v4) => v4,
                            _ => Ipv4Addr::UNSPECIFIED,
                        };
                        if let Some(ep) = parse_mdns_rp_packet(&buf[..n], fallback_ip) {
                            let matched_irk = alt_irk.is_some_and(|irk| {
                                !ep.identifier.is_empty()
                                    && !ep.auth_tag.is_empty()
                                    && crate::wifi::auth_tag_matches(irk, &ep.identifier, &ep.auth_tag)
                            });
                            if matched_irk {
                                out.retain(|e: &MdnsRpEndpoint| !(e.ip == ep.ip && e.port == ep.port));
                                out.insert(0, ep);
                                verified_found = true;
                                break;
                            }
                            if !out.iter().any(|e: &MdnsRpEndpoint| e.ip == ep.ip && e.port == ep.port) {
                                out.push(ep);
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            if verified_found {
                break;
            }
        }
        if should_early_break_mdns_discovery(
            alt_irk.is_some(),
            verified_found,
            out.len(),
            started.elapsed().as_millis() as u64,
        ) {
            break;
        }
        if !got_any_pkt {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    out
}

async fn send_rp_json(stream: &mut TcpStream, val: &Value) -> Result<(), String> {
    let body = serde_json::to_vec(val).map_err(|e| format!("RP json encode: {e}"))?;
    if body.len() > u16::MAX as usize {
        return Err("RP json body too large".into());
    }
    let mut pkt = Vec::with_capacity(RP_MAGIC.len() + 2 + body.len());
    pkt.extend_from_slice(RP_MAGIC);
    pkt.extend_from_slice(&(body.len() as u16).to_be_bytes());
    pkt.extend_from_slice(&body);
    stream
        .write_all(&pkt)
        .await
        .map_err(|e| format!("RP send write: {e}"))?;
    stream.flush().await.map_err(|e| format!("RP send flush: {e}"))
}

async fn recv_rp_json(stream: &mut TcpStream) -> Result<Value, String> {
    let mut hdr = [0u8; 11];
    stream
        .read_exact(&mut hdr)
        .await
        .map_err(|e| format!("RP recv header: {e}"))?;
    if &hdr[0..9] != RP_MAGIC {
        return Err("Invalid RPPairing magic".into());
    }
    let len = u16::from_be_bytes([hdr[9], hdr[10]]) as usize;
    let mut body = vec![0u8; len];
    stream
        .read_exact(&mut body)
        .await
        .map_err(|e| format!("RP recv body: {e}"))?;
    serde_json::from_slice(&body).map_err(|e| format!("RP json decode: {e}"))
}

pub struct RealWifiCdTunnel {
    pub stream: Box<dyn ReadWrite>,
    pub raw_data_socket: Option<usize>,
    pub client_ip: Ipv6Addr,
    pub server_ip: Ipv6Addr,
    pub rsd_port: u16,
    pub mtu: usize,
    pub control: TcpStream,
    pub resolved_ip: String,
    pub resolved_port: u16,
}

/// Connects 100% natively over Wi-Fi to an iOS 17–27.2 device via real Apple RemotePairing
/// (`_remotepairing._tcp.local.` Pair-Verify + `createListener` + TLS 1.2 PSK `0x00A8` + `CDTunnel`).
/// Tahap 241: Clean previous-session settle guard (`200 ms`), fresh multi-interface 2x-burst mDNS
/// discovery BEFORE every connect attempt, and 3-round resilient auto-retry loop (`450 ms` backoff).
pub async fn connect_real_apple_rp_wifi_tunnel(
    udid: &str,
    hint_ip: &str,
) -> Result<RealWifiCdTunnel, String> {
    let record = crate::wifi::find_record(udid)
        .or_else(|| crate::wifi::all_records().into_iter().next())
        .ok_or_else(|| {
            "Record RemotePairing Wi-Fi (remote_<udid>.plist) belum ditemukan; colokkan kabel USB sekali lalu klik 'Atur Wi-Fi'.".to_string()
        })?;
    if record.private_key.len() != 32 {
        return Err("Record private_key Ed25519 tidak valid (bukan 32 byte).".into());
    }
    let mut ed_seed = [0u8; 32];
    ed_seed.copy_from_slice(&record.private_key);

    // Step 1: Clean Previous Socket Guard — let iOS `remoted` finish tearing down prior tunnel
    let last_end = LAST_WIFI_SESSION_END_MS.load(Ordering::SeqCst);
    let elapsed_since_last_end_ms = if last_end > 0 {
        Some(now_epoch_ms().saturating_sub(last_end))
    } else {
        None
    };
    if elapsed_since_last_end_ms.is_some_and(|el| el < 350) {
        tokio::time::sleep(Duration::from_millis(180)).await;
    }

    let host_name = crate::wifi::local_hostname();
    let host_id = crate::wifi::host_identifier(&host_name);
    let cached_state = crate::wifi::get_device_wifi_state(&record.udid);
    let cached_ip = cached_state
        .get("last_ip")
        .or_else(|| cached_state.get("lastIp"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let cached_port = cached_state
        .get("last_rp_port")
        .and_then(|v| v.as_u64())
        .map(|p| p as u16)
        .filter(|&p| p > 0);
    let mut effective_hint = if !hint_ip.trim().is_empty() {
        Some(hint_ip.trim().to_string())
    } else {
        cached_ip
    };

    let mut last_err = String::from("Perangkat iPhone tidak ditemukan di Wi-Fi (_remotepairing._tcp.local.).");

    // Step 2 & 3 (Tahap 244/250 Pilar 2): Fast `< 2.5s` 3-Round Resilient Connect Loop —
    // fresh 0/120/360 ms mDNS with instant `alt_irk` early-break, skip stale `cached_port`
    // within 12s post-stop, and bounded 400 ms TCP timeout on any cached/fallback ephemeral port!
    for round in 1..=3u8 {
        let mut targets: Vec<(SocketAddr, bool)> = Vec::new();
        let mdns_eps = discover_remotepairing_mdns(
            effective_hint.as_deref(),
            Some(&record.peer_alt_irk),
            if round == 1 { 650 } else { 900 },
        );
        let mut verified_eps: Vec<&MdnsRpEndpoint> = Vec::new();
        for ep in &mdns_eps {
            let auth_ok = !ep.identifier.is_empty()
                && !ep.auth_tag.is_empty()
                && crate::wifi::validate_auth_tag(
                    &record.peer_alt_irk,
                    &ep.identifier,
                    &ep.auth_tag,
                );
            if auth_ok {
                verified_eps.push(ep);
            }
        }
        if let Some(first_ep) = verified_eps.first() {
            let fresh_ip = first_ep.ip.to_string();
            effective_hint = Some(fresh_ip.clone());
            let clean_host = first_ep.host_name.trim();
            let host_opt = if !clean_host.is_empty() {
                Some(clean_host)
            } else {
                None
            };
            crate::wifi::update_device_wifi_state(
                &record.udid,
                Some(true),
                Some(&fresh_ip),
                false,
                None,
                None,
                false,
                true,
                Some(first_ep.port),
            );
            if let Some(hname) = host_opt {
                let existing_name = cached_state
                    .get("device_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if existing_name.is_empty() {
                    crate::wifi::update_device_identity_cache(
                        &record.udid,
                        Some(hname),
                        None,
                        None,
                    );
                }
                crate::physical_stream::set_active_session_device_identity(
                    &record.udid,
                    Some(hname),
                    None,
                    None,
                );
            }
            for ep in &verified_eps {
                let sa = SocketAddr::new(IpAddr::V4(ep.ip), ep.port);
                if !targets.iter().any(|(a, _)| *a == sa) {
                    targets.push((sa, false));
                }
            }
        } else {
            for ep in &mdns_eps {
                let sa = SocketAddr::new(IpAddr::V4(ep.ip), ep.port);
                if !targets.iter().any(|(a, _)| *a == sa) {
                    targets.push((sa, false));
                }
            }
        }
        // Fallback to cached port ONLY when policy allows (round 1, no verified mDNS, NOT < 12s post-stop)
        if let Some(ref hip) = effective_hint {
            if let Ok(parsed_ip) = hip.parse::<IpAddr>() {
                if should_try_cached_rp_port(
                    round,
                    cached_port,
                    elapsed_since_last_end_ms,
                    !verified_eps.is_empty(),
                ) {
                    if let Some(cp) = cached_port {
                        let sa = SocketAddr::new(parsed_ip, cp);
                        if !targets.iter().any(|(a, _)| *a == sa) {
                            targets.push((sa, true));
                        }
                    }
                }
                if verified_eps.is_empty() {
                    let fallback_sa = SocketAddr::new(parsed_ip, 49152);
                    if !targets.iter().any(|(a, _)| *a == fallback_sa) {
                        targets.push((fallback_sa, true));
                    }
                }
            }
        }

        for (addr, is_cached_or_fallback) in targets {
            let tcp_timeout_ms = rp_tcp_connect_timeout_ms(is_cached_or_fallback);
            match connect_real_rp_single(addr, &ed_seed, &host_id, tcp_timeout_ms).await {
                Ok(tun) => {
                    crate::wifi::update_device_wifi_state(
                        &record.udid,
                        Some(true),
                        Some(&tun.resolved_ip),
                        false,
                        None,
                        None,
                        false,
                        true,
                        Some(tun.resolved_port),
                    );
                    return Ok(tun);
                }
                Err(e) => {
                    last_err = format!("round {round} ({addr}): {e}");
                }
            }
        }
        if round < 3 {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
    Err(last_err)
}

async fn connect_real_rp_single(
    addr: SocketAddr,
    ed_seed: &[u8; 32],
    host_id: &str,
    tcp_timeout_ms: u64,
) -> Result<RealWifiCdTunnel, String> {
    let mut ctrl = tokio::time::timeout(
        Duration::from_millis(tcp_timeout_ms),
        TcpStream::connect(addr),
    )
    .await
    .map_err(|_| "TCP connect timeout".to_string())?
    .map_err(|e| format!("TCP connect: {e}"))?;
    crate::wifi::configure_low_latency_tokio_tcp(&ctrl);

    let mut seq_num: u64 = 0;

    // Step 1: attemptPairVerify handshake
    let hs_req = json!({
        "message": {
            "plain": {
                "_0": {
                    "request": {
                        "_0": {
                            "handshake": {
                                "_0": {
                                    "hostOptions": { "attemptPairVerify": true },
                                    "wireProtocolVersion": 19
                                }
                            }
                        }
                    }
                }
            }
        },
        "originatedBy": "host",
        "sequenceNumber": seq_num
    });
    seq_num += 1;
    send_rp_json(&mut ctrl, &hs_req).await?;
    let _hs_resp = tokio::time::timeout(Duration::from_millis(2500), recv_rp_json(&mut ctrl))
        .await
        .map_err(|_| "attemptPairVerify timeout".to_string())??;

    // Step 2: Pair-Verify M1 (ephemeral X25519 public key)
    let mut x_priv = [0u8; 32];
    fill_random(&mut x_priv)?;
    let x_pub = x25519_public(&x_priv);
    let m1_tlv = tlv8_encode(&[(0x06, &[0x01]), (0x03, &x_pub)]);
    let m1_b64 = b64_encode(&m1_tlv);
    let m1_req = json!({
        "message": {
            "plain": {
                "_0": {
                    "event": {
                        "_0": {
                            "pairingData": {
                                "_0": {
                                    "data": m1_b64,
                                    "kind": "verifyManualPairing",
                                    "startNewSession": true
                                }
                            }
                        }
                    }
                }
            }
        },
        "originatedBy": "host",
        "sequenceNumber": seq_num
    });
    seq_num += 1;
    send_rp_json(&mut ctrl, &m1_req).await?;

    let m2_resp = tokio::time::timeout(Duration::from_millis(2500), recv_rp_json(&mut ctrl))
        .await
        .map_err(|_| "pair-verify M2 timeout".to_string())??;
    let m2_b64 = m2_resp
        .pointer("/message/plain/_0/event/_0/pairingData/_0/data")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("M2 missing pairingData: {m2_resp:?}"))?;
    let m2_raw = b64_decode(m2_b64).ok_or("M2 base64 invalid")?;
    let m2_tlv = tlv8_decode(&m2_raw);
    if let Some(err_b) = m2_tlv.get(&0x07) {
        return Err(format!("Pair-Verify M2 error code: {err_b:02X?}"));
    }
    let peer_pub_vec = m2_tlv
        .get(&0x03)
        .filter(|v| v.len() == 32)
        .ok_or("M2 missing 32-byte peer X25519 public key")?;
    let mut peer_pub = [0u8; 32];
    peer_pub.copy_from_slice(peer_pub_vec);

    // Step 3: Shared secret + Ed25519 signature + ChaCha20-Poly1305 M3
    let encryption_key = x25519(&x_priv, &peer_pub);
    let pv_enc_key = hkdf_sha512_32(
        &encryption_key,
        b"Pair-Verify-Encrypt-Salt",
        b"Pair-Verify-Encrypt-Info",
    );

    let mut sign_buf = Vec::with_capacity(64 + host_id.len());
    sign_buf.extend_from_slice(&x_pub);
    sign_buf.extend_from_slice(host_id.as_bytes());
    sign_buf.extend_from_slice(&peer_pub);
    let signature = ed25519_sign(ed_seed, &sign_buf);

    let inner_m3 = tlv8_encode(&[(0x01, host_id.as_bytes()), (0x0A, &signature)]);
    let nonce_m3: [u8; 12] = *b"\x00\x00\x00\x00PV-Msg03";
    let enc_m3 = chacha20_poly1305_encrypt(&pv_enc_key, &nonce_m3, &inner_m3, &[]);
    let m3_tlv = tlv8_encode(&[(0x06, &[0x03]), (0x05, &enc_m3)]);
    let m3_b64 = b64_encode(&m3_tlv);

    let m3_req = json!({
        "message": {
            "plain": {
                "_0": {
                    "event": {
                        "_0": {
                            "pairingData": {
                                "_0": {
                                    "data": m3_b64,
                                    "kind": "verifyManualPairing",
                                    "startNewSession": false
                                }
                            }
                        }
                    }
                }
            }
        },
        "originatedBy": "host",
        "sequenceNumber": seq_num
    });
    seq_num += 1;
    send_rp_json(&mut ctrl, &m3_req).await?;

    let m4_resp = tokio::time::timeout(Duration::from_millis(2500), recv_rp_json(&mut ctrl))
        .await
        .map_err(|_| "pair-verify M4 timeout".to_string())??;
    let m4_b64 = m4_resp
        .pointer("/message/plain/_0/event/_0/pairingData/_0/data")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("M4 missing pairingData: {m4_resp:?}"))?;
    let m4_raw = b64_decode(m4_b64).ok_or("M4 base64 invalid")?;
    let m4_tlv = tlv8_decode(&m4_raw);
    if let Some(err_b) = m4_tlv.get(&0x07) {
        return Err(format!("Pair-Verify M4 rejected (code {err_b:02X?}); jalankan 'Atur Wi-Fi' sekali via USB."));
    }

    // Step 4: Encrypted `createListener` (`transportProtocolType = "tcp"`)
    let client_key = hkdf_sha512_32(&encryption_key, &[], b"ClientEncrypt-main");
    let server_key = hkdf_sha512_32(&encryption_key, &[], b"ServerEncrypt-main");
    let enc_key_b64 = b64_encode(&encryption_key);
    let own_pid = std::process::id();

    let create_req_inner = json!({
        "request": {
            "_0": {
                "createListener": {
                    "key": enc_key_b64,
                    "peerConnectionsInfo": [{
                        "owningPID": own_pid,
                        "owningProcessName": "CoreDeviceService"
                    }],
                    "transportProtocolType": "tcp"
                }
            }
        }
    });
    let create_plain = serde_json::to_vec(&create_req_inner).map_err(|e| e.to_string())?;
    let nonce0 = [0u8; 12]; // little-endian u64(0) || 4 zero bytes
    let create_enc = chacha20_poly1305_encrypt(&client_key, &nonce0, &create_plain, &[]);
    let create_enc_b64 = b64_encode(&create_enc);
    let create_env = json!({
        "message": {
            "streamEncrypted": {
                "_0": create_enc_b64
            }
        },
        "originatedBy": "host",
        "sequenceNumber": seq_num
    });
    send_rp_json(&mut ctrl, &create_env).await?;

    let cl_resp = tokio::time::timeout(Duration::from_millis(2500), recv_rp_json(&mut ctrl))
        .await
        .map_err(|_| "createListener timeout".to_string())??;
    let cl_b64 = cl_resp
        .pointer("/message/streamEncrypted/_0")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("createListener missing streamEncrypted: {cl_resp:?}"))?;
    let cl_ct = b64_decode(cl_b64).ok_or("createListener base64 invalid")?;
    let cl_plain = chacha20_poly1305_decrypt(&server_key, &nonce0, &cl_ct, &[])?;
    let cl_json: Value = serde_json::from_slice(&cl_plain)
        .map_err(|e| format!("createListener json parse: {e}"))?;
    let data_port = cl_json
        .pointer("/response/_1/createListener/port")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| format!("createListener missing port: {cl_json:?}"))? as u16;

    // Step 5: Connect TLS 1.2 PSK (`0x00A8`) to `<ip>:<data_port>` + CDTunnel handshake
    let data_addr = SocketAddr::new(addr.ip(), data_port);
    let data_sock = tokio::time::timeout(Duration::from_millis(2500), TcpStream::connect(data_addr))
        .await
        .map_err(|_| format!("data TCP connect timeout ({data_addr})"))?
        .map_err(|e| format!("data TCP connect ({data_addr}): {e}"))?;
    crate::wifi::configure_low_latency_tokio_tcp(&data_sock);
    #[cfg(windows)]
    let raw_data_socket = {
        use std::os::windows::io::AsRawSocket;
        Some(data_sock.as_raw_socket() as usize)
    };
    #[cfg(not(windows))]
    let raw_data_socket: Option<usize> = None;

    let tls_stream = tokio::time::timeout(
        Duration::from_millis(3000),
        TlsPskStream::connect(data_sock, &encryption_key),
    )
    .await
    .map_err(|_| "TLS 1.2 PSK handshake timeout".to_string())??;

    let boxed: Box<dyn ReadWrite> = Box::new(tls_stream);
    let cd = tokio::time::timeout(Duration::from_millis(3000), CdTunnel::handshake(boxed))
        .await
        .map_err(|_| "CDTunnel Wi-Fi handshake timeout".to_string())?
        .map_err(|e| format!("CDTunnel Wi-Fi handshake: {e}"))?;

    let info = cd.info.clone();
    let client_ip: Ipv6Addr = info
        .client_address
        .parse()
        .map_err(|_| format!("invalid Wi-Fi client IPv6: {}", info.client_address))?;
    let server_ip: Ipv6Addr = info
        .server_address
        .parse()
        .map_err(|_| format!("invalid Wi-Fi server IPv6: {}", info.server_address))?;

    Ok(RealWifiCdTunnel {
        stream: cd.into_inner(),
        raw_data_socket,
        client_ip,
        server_ip,
        rsd_port: info.server_rsd_port,
        mtu: info.mtu as usize,
        control: ctrl,
        resolved_ip: addr.ip().to_string(),
        resolved_port: addr.port(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    fn hex_to_vec(s: &str) -> Vec<u8> {
        let clean: String = s.chars().filter(|c| c.is_ascii_hexdigit()).collect();
        (0..clean.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&clean[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn test_tahap240_x25519_ed25519_chacha20poly1305_rfc_vectors() {
        // RFC 7748 Section 6.1 X25519 vector
        let alice_priv: [u8; 32] =
            hex_to_vec("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a")
                .try_into()
                .unwrap();
        let alice_pub = x25519_public(&alice_priv);
        assert_eq!(
            alice_pub.as_slice(),
            hex_to_vec("8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a").as_slice()
        );
        let bob_priv: [u8; 32] =
            hex_to_vec("5dab087e624a8a4b79e17f8b83800ee66f3bb1292618b6fd1c2f8b27ff88e0eb")
                .try_into()
                .unwrap();
        let bob_pub = x25519_public(&bob_priv);
        assert_eq!(x25519(&alice_priv, &bob_pub), x25519(&bob_priv, &alice_pub));

        // RFC 8032 Section 7.1 Ed25519 Test Vector 1 (empty message)
        let seed1: [u8; 32] =
            hex_to_vec("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60")
                .try_into()
                .unwrap();
        let pub1 = ed25519_public_from_seed(&seed1);
        assert_eq!(
            pub1.as_slice(),
            hex_to_vec("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a").as_slice()
        );
        let sig1 = ed25519_sign(&seed1, b"");
        assert_eq!(
            sig1.as_slice(),
            hex_to_vec(
                "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e06522490155\
                 5fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b"
            )
            .as_slice()
        );

        // RFC 8439 ChaCha20-Poly1305 roundtrip
        let key: [u8; 32] =
            hex_to_vec("808182838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9f")
                .try_into()
                .unwrap();
        let nonce: [u8; 12] = hex_to_vec("070000004041424344454647").try_into().unwrap();
        let aad = hex_to_vec("50515253c0c1c2c3c4c5c6c7");
        let pt = b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.";
        let ct = chacha20_poly1305_encrypt(&key, &nonce, pt, &aad);
        let dec = chacha20_poly1305_decrypt(&key, &nonce, &ct, &aad).expect("decrypt");
        assert_eq!(dec.as_slice(), pt.as_slice());
        assert_eq!(
            &ct[ct.len() - 16..],
            hex_to_vec("1ae10b594f09e26a7e902ecbd0600691").as_slice()
        );
    }

    #[test]
    fn test_tahap242_tls_psk_atomic_1byte_3byte_wouldblock_framing() {
        use std::task::{RawWaker, RawWakerVTable, Waker};

        fn noop_waker() -> Waker {
            fn clone(_: *const ()) -> RawWaker {
                RawWaker::new(std::ptr::null(), &VT)
            }
            fn no_op(_: *const ()) {}
            static VT: RawWakerVTable = RawWakerVTable::new(clone, no_op, no_op, no_op);
            unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VT)) }
        }

        // Mock stream that delivers 1-byte / 3-byte chunks interleaved with WouldBlock
        // on BOTH read and write paths to prove 100% atomic TLS-PSK AES-128-GCM framing.
        struct FragmentedDuplex {
            rx_wire: VecDeque<u8>,
            tx_wire: Vec<u8>,
            read_step: usize,
            write_step: usize,
        }
        impl AsyncRead for FragmentedDuplex {
            fn poll_read(
                mut self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
                buf: &mut ReadBuf<'_>,
            ) -> Poll<io::Result<()>> {
                self.read_step += 1;
                if self.read_step % 2 == 1 {
                    return Poll::Pending; // alternating WouldBlock!
                }
                if self.rx_wire.is_empty() {
                    return Poll::Pending;
                }
                let chunk = if self.read_step % 4 == 0 { 3 } else { 1 };
                let take = chunk.min(self.rx_wire.len()).min(buf.unfilled_mut().len());
                let mut tmp = Vec::with_capacity(take);
                for _ in 0..take {
                    if let Some(b) = self.rx_wire.pop_front() {
                        tmp.push(b);
                    }
                }
                buf.put_slice(&tmp);
                Poll::Ready(Ok(()))
            }
        }
        impl AsyncWrite for FragmentedDuplex {
            fn poll_write(
                mut self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
                buf: &[u8],
            ) -> Poll<io::Result<usize>> {
                self.write_step += 1;
                if self.write_step % 2 == 1 {
                    return Poll::Pending; // alternating WouldBlock mid-record!
                }
                let chunk = if self.write_step % 4 == 0 { 3 } else { 1 };
                let take = chunk.min(buf.len());
                self.tx_wire.extend_from_slice(&buf[..take]);
                Poll::Ready(Ok(take))
            }
            fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
                Poll::Ready(Ok(()))
            }
            fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
                Poll::Ready(Ok(()))
            }
        }

        let k_bytes = [0x42u8; 16];
        let iv_bytes = [0x11u8, 0x22, 0x33, 0x44];
        let mut sender = TlsPskStream {
            inner: FragmentedDuplex {
                rx_wire: VecDeque::new(),
                tx_wire: Vec::new(),
                read_step: 0,
                write_step: 0,
            },
            tx_key: AesGcmKey::new(&k_bytes).unwrap(),
            rx_key: AesGcmKey::new(&k_bytes).unwrap(),
            tx_iv: iv_bytes,
            rx_iv: iv_bytes,
            tx_seq: 1,
            rx_seq: 1,
            rx_cipher: Vec::new(),
            rx_cipher_pos: 0,
            rx_plain: Vec::new(),
            rx_plain_pos: 0,
            tx_cipher: Vec::new(),
            tx_cipher_pos: 0,
            eof: false,
        };

        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        let msg1 = b"Tahap242-Packet-Alpha-CDTunnel-IPv6-Header-Payload-0001";
        let msg2 = b"Tahap242-Packet-Beta-RTP-RTCP-Keepalive-Endurance-0002!";

        for msg in [msg1.as_slice(), msg2.as_slice()] {
            let mut off = 0usize;
            while off < msg.len() {
                match Pin::new(&mut sender).poll_write(&mut cx, &msg[off..]) {
                    Poll::Ready(Ok(n)) => off += n,
                    Poll::Pending => continue,
                    Poll::Ready(Err(e)) => panic!("write failed: {e}"),
                }
            }
            loop {
                match Pin::new(&mut sender).poll_flush(&mut cx) {
                    Poll::Ready(Ok(())) => break,
                    Poll::Pending => continue,
                    Poll::Ready(Err(e)) => panic!("flush failed: {e}"),
                }
            }
        }

        // Also inject a benign TLS warning alert (level=1, desc=100) + close_notify (level=1, desc=0)
        let warn_alert = tls_encrypt_record(
            &AesGcmKey::new(&k_bytes).unwrap(),
            &iv_bytes,
            3,
            TLS_RECORD_ALERT,
            &[1, 100],
        )
        .unwrap();
        let close_alert = tls_encrypt_record(
            &AesGcmKey::new(&k_bytes).unwrap(),
            &iv_bytes,
            4,
            TLS_RECORD_ALERT,
            &[1, 0],
        )
        .unwrap();

        let mut wire = VecDeque::from(sender.inner.tx_wire);
        wire.extend(warn_alert);
        wire.extend(close_alert);

        let mut receiver = TlsPskStream {
            inner: FragmentedDuplex {
                rx_wire: wire,
                tx_wire: Vec::new(),
                read_step: 0,
                write_step: 0,
            },
            tx_key: AesGcmKey::new(&k_bytes).unwrap(),
            rx_key: AesGcmKey::new(&k_bytes).unwrap(),
            tx_iv: iv_bytes,
            rx_iv: iv_bytes,
            tx_seq: 1,
            rx_seq: 1,
            rx_cipher: Vec::new(),
            rx_cipher_pos: 0,
            rx_plain: Vec::new(),
            rx_plain_pos: 0,
            tx_cipher: Vec::new(),
            tx_cipher_pos: 0,
            eof: false,
        };

        let mut recovered = Vec::new();
        let mut tmp = [0u8; 7];
        for _ in 0..5000 {
            let mut rb = ReadBuf::new(&mut tmp);
            match Pin::new(&mut receiver).poll_read(&mut cx, &mut rb) {
                Poll::Pending => continue,
                Poll::Ready(Ok(())) => {
                    if rb.filled().is_empty() {
                        break; // clean close_notify EOF
                    }
                    recovered.extend_from_slice(rb.filled());
                }
                Poll::Ready(Err(e)) => panic!("fragmented TLS-PSK read failed: {e}"),
            }
        }
        let mut expected = Vec::new();
        expected.extend_from_slice(msg1);
        expected.extend_from_slice(msg2);
        assert_eq!(recovered, expected);
        assert!(receiver.eof, "close_notify must mark clean EOF after full payload");
    }

    #[test]
    fn test_tahap250_fast_wifi_cached_port_timeout_and_post_stop_fresh_mdns_policy() {
        // 1. Cached/fallback ephemeral port TCP timeout strictly <= 450 ms (prevents 4s dead hangs!)
        let cached_ms = rp_tcp_connect_timeout_ms(true);
        let verified_ms = rp_tcp_connect_timeout_ms(false);
        assert!(
            cached_ms <= 450 && cached_ms >= 200,
            "cached_port TCP timeout must be fast (<= 450ms), got {cached_ms}ms"
        );
        assert!(
            verified_ms >= 1000 && verified_ms <= 2500,
            "verified mDNS TCP timeout out of range: {verified_ms}ms"
        );

        // 2. Post-stop (< 12s) MUST skip stale cached_port and go straight to fresh mDNS
        assert!(
            !should_try_cached_rp_port(1, Some(51234), Some(3_000), false),
            "Within 12s post-stop, stale cached_port MUST be skipped!"
        );
        assert!(
            !should_try_cached_rp_port(1, Some(51234), Some(11_999), false),
            "At 11.999s post-stop, stale cached_port MUST still be skipped!"
        );
        // After >= 12s (or cold start None) with no verified mDNS yet on Round 1 -> allowed
        assert!(should_try_cached_rp_port(1, Some(51234), Some(15_000), false));
        assert!(should_try_cached_rp_port(1, Some(51234), None, false));
        // Round >= 2 or verified mDNS already found -> skip cached_port
        assert!(!should_try_cached_rp_port(2, Some(51234), None, false));
        assert!(!should_try_cached_rp_port(1, Some(51234), None, true));

        // 3. Instant early-break mDNS policy:
        //    - With target alt_irk: ONLY break when alt_irk_verified == true (NEVER break on unverified!)
        assert!(
            !should_early_break_mdns_discovery(true, false, 3, 500),
            "Must NEVER early-break on unverified mDNS responses when target alt_irk is set!"
        );
        assert!(
            should_early_break_mdns_discovery(true, true, 0, 15),
            "Must IMMEDIATELY early-break (even at 15ms!) when alt_irk-verified match arrives!"
        );
        //    - Without target alt_irk: wait >= 240 ms before early-break
        assert!(!should_early_break_mdns_discovery(false, false, 1, 100));
        assert!(should_early_break_mdns_discovery(false, false, 1, 250));
    }

    #[test]
    fn test_tahap253_ed25519_scalar_mul_add_overflow_proof_and_multi_seed_verification() {
        fn ed_scalar_mul_point(point: EdPoint, scalar_le: &[u8; 32]) -> EdPoint {
            let mut acc = EdPoint::IDENTITY;
            let mut cur = point;
            for byte in scalar_le.iter() {
                for bit in 0..8 {
                    if (byte >> bit) & 1 == 1 {
                        acc = acc.add(cur);
                    }
                    cur = cur.double();
                }
            }
            acc
        }

        // 1. Worst-case u64::MAX limbs must never overflow u128!
        let max_limbs = [u64::MAX; 4];
        let s_extreme = scalar_mul_add_mod_l(&max_limbs, &max_limbs, &max_limbs);
        let mut s_ext_limbs = [0u64; 4];
        for i in 0..4 {
            s_ext_limbs[i] = u64::from_le_bytes(s_extreme[i * 8..(i + 1) * 8].try_into().unwrap());
        }
        assert_eq!(
            u256_cmp(&s_ext_limbs, &ED_L),
            std::cmp::Ordering::Less,
            "scalar_mul_add_mod_l output must always be canonical (< L)"
        );

        // 2. Cryptographic verification S * B == R + k * A across 64 distinct
        //    seeds & 64-byte AirPlay Pair-Verify-like payloads (old code failed >50%!).
        for idx in 0u8..64u8 {
            let mut seed = [0u8; 32];
            for (b_i, slot) in seed.iter_mut().enumerate() {
                *slot = idx.wrapping_mul(37).wrapping_add((b_i as u8).wrapping_mul(13)).wrapping_add(0xA5);
            }
            let mut msg = [0u8; 64];
            for (b_i, slot) in msg.iter_mut().enumerate() {
                *slot = idx.wrapping_mul(53).wrapping_add((b_i as u8).wrapping_mul(7)).wrapping_add(0x5A);
            }

            let sig = ed25519_sign(&seed, &msg);
            let pub_key = ed25519_public_from_seed(&seed);

            // Recompute A = a*B, R = r*B, k = H(R || A || msg) mod L, and verify S*B == R + k*A!
            let digest = sha512(&seed);
            let mut a_bytes = [0u8; 32];
            a_bytes.copy_from_slice(&digest[..32]);
            a_bytes[0] &= 248;
            a_bytes[31] &= 63;
            a_bytes[31] |= 64;
            let prefix = &digest[32..64];
            let r_lim = scalar_reduce_512_mod_l(&sha512_chunks(&[prefix, &msg]));
            let r_bytes = u256_to_le_bytes(&r_lim);
            let cap_r = ed_scalar_mul_base(&r_bytes);
            assert_eq!(&sig[..32], cap_r.encode().as_slice(), "R mismatch at seed {idx}");

            let k_lim = scalar_reduce_512_mod_l(&sha512_chunks(&[&sig[..32], &pub_key, &msg]));
            let k_bytes = u256_to_le_bytes(&k_lim);
            let mut s_bytes = [0u8; 32];
            s_bytes.copy_from_slice(&sig[32..64]);

            let lhs = ed_scalar_mul_base(&s_bytes).encode();
            let cap_a = ed_scalar_mul_base(&a_bytes);
            let rhs = cap_r.add(ed_scalar_mul_point(cap_a, &k_bytes)).encode();
            assert_eq!(
                lhs, rhs,
                "Ed25519 verification S*B == R + k*A FAILED at seed {idx}!"
            );
        }
    }

    #[test]
    fn test_tahap281_tls_psk_contiguous_buffer_and_burst_loopback() {
        use std::io::Write;
        use std::net::TcpListener;

        const TOTAL: usize = 512 * 1024;
        let payload: Vec<u8> = (0..TOTAL)
            .map(|i| (i.wrapping_mul(131).wrapping_add(i >> 8) & 0xFF) as u8)
            .collect();
        let key_bytes = [0x7Bu8; 16];
        let iv_bytes = [0x21u8, 0x43, 0x65, 0x87];

        // Build TLS 1.2 AES-128-GCM wire stream with varied record lengths (0..16384 B) + close_notify
        let tx_key = AesGcmKey::new(&key_bytes).unwrap();
        let rec_sizes = [0usize, 1, 37, 255, 511, 1300, 4096, 16384];
        let mut wire = Vec::with_capacity(TOTAL + 32768);
        let mut off = 0usize;
        let mut seq = 1u64;
        let mut rs_idx = 0usize;
        while off < TOTAL {
            let want = rec_sizes[rs_idx % rec_sizes.len()].min(TOTAL - off);
            rs_idx += 1;
            let rec = tls_encrypt_record(
                &tx_key,
                &iv_bytes,
                seq,
                TLS_RECORD_APPDATA,
                &payload[off..off + want],
            )
            .expect("tls_encrypt_record");
            wire.extend_from_slice(&rec);
            off += want;
            seq += 1;
        }
        let close_rec = tls_encrypt_record(&tx_key, &iv_bytes, seq, TLS_RECORD_ALERT, &[1, 0])
            .expect("close_notify");
        wire.extend_from_slice(&close_rec);

        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
        let addr = listener.local_addr().unwrap();
        let srv_handle = std::thread::spawn(move || {
            let (mut srv, _) = listener.accept().expect("accept");
            let _ = srv.set_nodelay(true);
            let wire_chunks = [3usize, 19, 2000, 7777, 1, 513, 16384, 77];
            let mut pos = 0usize;
            let mut wi = 0usize;
            while pos < wire.len() {
                let take = wire_chunks[wi % wire_chunks.len()].min(wire.len() - pos);
                wi += 1;
                srv.write_all(&wire[pos..pos + take]).expect("write chunk");
                pos += take;
            }
            let _ = srv.flush();
        });

        let cli_std = std::net::TcpStream::connect(addr).expect("connect");
        let _ = cli_std.set_nodelay(true);
        let cli_tokio = TcpStream::from_std(cli_std).expect("TcpStream::from_std");
        let mut stream = TlsPskStream {
            inner: cli_tokio,
            tx_key: AesGcmKey::new(&key_bytes).unwrap(),
            rx_key: AesGcmKey::new(&key_bytes).unwrap(),
            tx_iv: iv_bytes,
            rx_iv: iv_bytes,
            tx_seq: 1,
            rx_seq: 1,
            rx_cipher: Vec::with_capacity(65536),
            rx_cipher_pos: 0,
            rx_plain: Vec::with_capacity(65536),
            rx_plain_pos: 0,
            tx_cipher: Vec::with_capacity(32768),
            tx_cipher_pos: 0,
            eof: false,
        };

        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime");
        let started = Instant::now();
        let recovered = rt.block_on(async {
            let mut out = Vec::with_capacity(TOTAL);
            let mut big_buf = vec![0u8; 65536];
            let read_caps = [65536usize, 777, 4096, 13];
            let mut ri = 0usize;
            loop {
                let cap = read_caps[ri % read_caps.len()];
                ri += 1;
                let n = stream.read(&mut big_buf[..cap]).await.expect("tls read");
                if n == 0 {
                    break;
                }
                out.extend_from_slice(&big_buf[..n]);
            }
            out
        });
        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
        let _ = srv_handle.join();

        assert_eq!(recovered.len(), TOTAL, "512 KB burst byte count mismatch");
        assert_eq!(recovered, payload, "512 KB TLS-PSK contiguous buffer corruption!");
        assert!(stream.eof, "close_notify EOF expected after full 512 KB burst");
        assert!(
            stream.rx_plain.is_empty() && stream.rx_plain_pos == 0,
            "rx_plain cursor must compact cleanly to 0 after drain"
        );
        assert!(
            stream.rx_cipher.is_empty() && stream.rx_cipher_pos == 0,
            "rx_cipher cursor must compact cleanly to 0 after drain"
        );
        eprintln!(
            "TAHAP281_TLS_BURST_MS={:.3} bytes={}",
            elapsed_ms,
            recovered.len()
        );
    }
}
