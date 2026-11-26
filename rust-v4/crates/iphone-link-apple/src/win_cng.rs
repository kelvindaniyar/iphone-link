//! Windows CNG (`bcrypt.dll`) hardware-accelerated cryptographic primitives
//! for Apple RemotePairing Wi-Fi (`SHA-256`, `SHA-512`, `HMAC`, `HKDF`, `AES-128-GCM`, CSPRNG).

use std::ffi::c_void;
use std::ptr;

#[cfg(windows)]
mod native {
    use super::*;

    const BCRYPT_ALG_HANDLE_HMAC_FLAG: u32 = 0x0000_0008;
    const BCRYPT_USE_SYSTEM_PREFERRED_RNG: u32 = 0x0000_0002;
    const BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO_VERSION: u32 = 1;

    #[repr(C)]
    struct BcryptAuthModeInfo {
        cb_size: u32,
        dw_info_version: u32,
        pb_nonce: *mut u8,
        cb_nonce: u32,
        pb_auth_data: *mut u8,
        cb_auth_data: u32,
        pb_tag: *mut u8,
        cb_tag: u32,
        pb_mac_context: *mut u8,
        cb_mac_context: u32,
        cb_aad: u32,
        cb_data: u64,
        dw_flags: u32,
    }

    #[link(name = "bcrypt")]
    extern "system" {
        fn BCryptOpenAlgorithmProvider(
            ph_algorithm: *mut *mut c_void,
            psz_alg_id: *const u16,
            psz_implementation: *const u16,
            dw_flags: u32,
        ) -> i32;
        fn BCryptCloseAlgorithmProvider(h_algorithm: *mut c_void, dw_flags: u32) -> i32;
        fn BCryptSetProperty(
            h_object: *mut c_void,
            psz_property: *const u16,
            pb_input: *const u8,
            cb_input: u32,
            dw_flags: u32,
        ) -> i32;
        fn BCryptCreateHash(
            h_algorithm: *mut c_void,
            ph_hash: *mut *mut c_void,
            pb_hash_object: *mut u8,
            cb_hash_object: u32,
            pb_secret: *const u8,
            cb_secret: u32,
            dw_flags: u32,
        ) -> i32;
        fn BCryptHashData(
            h_hash: *mut c_void,
            pb_input: *const u8,
            cb_input: u32,
            dw_flags: u32,
        ) -> i32;
        fn BCryptFinishHash(
            h_hash: *mut c_void,
            pb_output: *mut u8,
            cb_output: u32,
            dw_flags: u32,
        ) -> i32;
        fn BCryptDestroyHash(h_hash: *mut c_void) -> i32;
        fn BCryptGenerateSymmetricKey(
            h_algorithm: *mut c_void,
            ph_key: *mut *mut c_void,
            pb_key_object: *mut u8,
            cb_key_object: u32,
            pb_secret: *const u8,
            cb_secret: u32,
            dw_flags: u32,
        ) -> i32;
        fn BCryptDestroyKey(h_key: *mut c_void) -> i32;
        fn BCryptEncrypt(
            h_key: *mut c_void,
            pb_input: *const u8,
            cb_input: u32,
            p_padding_info: *mut c_void,
            pb_iv: *mut u8,
            cb_iv: u32,
            pb_output: *mut u8,
            cb_output: u32,
            pcb_result: *mut u32,
            dw_flags: u32,
        ) -> i32;
        fn BCryptDecrypt(
            h_key: *mut c_void,
            pb_input: *const u8,
            cb_input: u32,
            p_padding_info: *mut c_void,
            pb_iv: *mut u8,
            cb_iv: u32,
            pb_output: *mut u8,
            cb_output: u32,
            pcb_result: *mut u32,
            dw_flags: u32,
        ) -> i32;
        fn BCryptGenRandom(
            h_algorithm: *mut c_void,
            pb_buffer: *mut u8,
            cb_buffer: u32,
            dw_flags: u32,
        ) -> i32;
    }

    fn wide_nul(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn fill_random(buf: &mut [u8]) -> Result<(), String> {
        let st = unsafe {
            BCryptGenRandom(
                ptr::null_mut(),
                buf.as_mut_ptr(),
                buf.len() as u32,
                BCRYPT_USE_SYSTEM_PREFERRED_RNG,
            )
        };
        if st == 0 {
            Ok(())
        } else {
            Err(format!("BCryptGenRandom failed: 0x{:08x}", st as u32))
        }
    }

    fn hash_one_shot(alg_name: &str, key: Option<&[u8]>, chunks: &[&[u8]], out: &mut [u8]) -> Result<(), String> {
        let alg_w = wide_nul(alg_name);
        let flags = if key.is_some() { BCRYPT_ALG_HANDLE_HMAC_FLAG } else { 0 };
        let mut h_alg: *mut c_void = ptr::null_mut();
        let st_open = unsafe { BCryptOpenAlgorithmProvider(&mut h_alg, alg_w.as_ptr(), ptr::null(), flags) };
        if st_open != 0 || h_alg.is_null() {
            return Err(format!("BCryptOpenAlgorithmProvider({alg_name}) failed: 0x{:08x}", st_open as u32));
        }
        let mut h_hash: *mut c_void = ptr::null_mut();
        let (k_ptr, k_len) = match key {
            Some(k) if !k.is_empty() => (k.as_ptr(), k.len() as u32),
            Some(_) => (b"".as_ptr(), 0),
            None => (ptr::null(), 0),
        };
        let st_create = unsafe {
            BCryptCreateHash(h_alg, &mut h_hash, ptr::null_mut(), 0, k_ptr, k_len, 0)
        };
        if st_create != 0 || h_hash.is_null() {
            unsafe { BCryptCloseAlgorithmProvider(h_alg, 0) };
            return Err(format!("BCryptCreateHash({alg_name}) failed: 0x{:08x}", st_create as u32));
        }
        for c in chunks {
            if !c.is_empty() {
                let st_data = unsafe { BCryptHashData(h_hash, c.as_ptr(), c.len() as u32, 0) };
                if st_data != 0 {
                    unsafe {
                        BCryptDestroyHash(h_hash);
                        BCryptCloseAlgorithmProvider(h_alg, 0);
                    }
                    return Err(format!("BCryptHashData failed: 0x{:08x}", st_data as u32));
                }
            }
        }
        let st_fin = unsafe { BCryptFinishHash(h_hash, out.as_mut_ptr(), out.len() as u32, 0) };
        unsafe {
            BCryptDestroyHash(h_hash);
            BCryptCloseAlgorithmProvider(h_alg, 0);
        }
        if st_fin != 0 {
            Err(format!("BCryptFinishHash failed: 0x{:08x}", st_fin as u32))
        } else {
            Ok(())
        }
    }

    pub fn sha256_chunks(chunks: &[&[u8]]) -> [u8; 32] {
        let mut out = [0u8; 32];
        let _ = hash_one_shot("SHA256", None, chunks, &mut out);
        out
    }

    pub fn sha512_chunks(chunks: &[&[u8]]) -> [u8; 64] {
        let mut out = [0u8; 64];
        let _ = hash_one_shot("SHA512", None, chunks, &mut out);
        out
    }

    pub fn hmac_sha256_chunks(key: &[u8], chunks: &[&[u8]]) -> [u8; 32] {
        let mut out = [0u8; 32];
        let _ = hash_one_shot("SHA256", Some(key), chunks, &mut out);
        out
    }

    pub fn hmac_sha512_chunks(key: &[u8], chunks: &[&[u8]]) -> [u8; 64] {
        let mut out = [0u8; 64];
        let _ = hash_one_shot("SHA512", Some(key), chunks, &mut out);
        out
    }

    pub struct AesGcmKey {
        h_alg: *mut c_void,
        h_key: *mut c_void,
    }

    unsafe impl Send for AesGcmKey {}
    unsafe impl Sync for AesGcmKey {}

    impl Drop for AesGcmKey {
        fn drop(&mut self) {
            unsafe {
                if !self.h_key.is_null() {
                    BCryptDestroyKey(self.h_key);
                }
                if !self.h_alg.is_null() {
                    BCryptCloseAlgorithmProvider(self.h_alg, 0);
                }
            }
        }
    }

    impl AesGcmKey {
        pub fn new(raw_key: &[u8]) -> Result<Self, String> {
            let alg_w = wide_nul("AES");
            let prop_w = wide_nul("ChainingMode");
            let mode_w = wide_nul("ChainingModeGCM");
            let mut h_alg: *mut c_void = ptr::null_mut();
            let st_open = unsafe { BCryptOpenAlgorithmProvider(&mut h_alg, alg_w.as_ptr(), ptr::null(), 0) };
            if st_open != 0 || h_alg.is_null() {
                return Err(format!("BCryptOpenAlgorithmProvider(AES) failed: 0x{:08x}", st_open as u32));
            }
            let mode_bytes = unsafe {
                std::slice::from_raw_parts(mode_w.as_ptr() as *const u8, mode_w.len() * 2)
            };
            let st_prop = unsafe {
                BCryptSetProperty(h_alg, prop_w.as_ptr(), mode_bytes.as_ptr(), mode_bytes.len() as u32, 0)
            };
            if st_prop != 0 {
                unsafe { BCryptCloseAlgorithmProvider(h_alg, 0) };
                return Err(format!("BCryptSetProperty(GCM) failed: 0x{:08x}", st_prop as u32));
            }
            let mut h_key: *mut c_void = ptr::null_mut();
            let st_key = unsafe {
                BCryptGenerateSymmetricKey(
                    h_alg,
                    &mut h_key,
                    ptr::null_mut(),
                    0,
                    raw_key.as_ptr(),
                    raw_key.len() as u32,
                    0,
                )
            };
            if st_key != 0 || h_key.is_null() {
                unsafe { BCryptCloseAlgorithmProvider(h_alg, 0) };
                return Err(format!("BCryptGenerateSymmetricKey(AES) failed: 0x{:08x}", st_key as u32));
            }
            Ok(Self { h_alg, h_key })
        }

        pub fn encrypt(&self, nonce: &[u8; 12], aad: &[u8], plaintext: &[u8]) -> Result<(Vec<u8>, [u8; 16]), String> {
            let mut nonce_copy = *nonce;
            let mut aad_copy = aad.to_vec();
            let mut tag = [0u8; 16];
            let mut out = vec![0u8; plaintext.len()];
            let mut info = BcryptAuthModeInfo {
                cb_size: std::mem::size_of::<BcryptAuthModeInfo>() as u32,
                dw_info_version: BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO_VERSION,
                pb_nonce: nonce_copy.as_mut_ptr(),
                cb_nonce: 12,
                pb_auth_data: if aad_copy.is_empty() { ptr::null_mut() } else { aad_copy.as_mut_ptr() },
                cb_auth_data: aad_copy.len() as u32,
                pb_tag: tag.as_mut_ptr(),
                cb_tag: 16,
                pb_mac_context: ptr::null_mut(),
                cb_mac_context: 0,
                cb_aad: 0,
                cb_data: 0,
                dw_flags: 0,
            };
            let mut written: u32 = 0;
            let st = unsafe {
                BCryptEncrypt(
                    self.h_key,
                    if plaintext.is_empty() { ptr::null() } else { plaintext.as_ptr() },
                    plaintext.len() as u32,
                    (&mut info as *mut BcryptAuthModeInfo).cast::<c_void>(),
                    ptr::null_mut(),
                    0,
                    if out.is_empty() { ptr::null_mut() } else { out.as_mut_ptr() },
                    out.len() as u32,
                    &mut written,
                    0,
                )
            };
            if st != 0 {
                return Err(format!("BCryptEncrypt(AES-GCM) failed: 0x{:08x}", st as u32));
            }
            out.truncate(written as usize);
            Ok((out, tag))
        }

        pub fn decrypt_into(
            &self,
            nonce: &[u8; 12],
            aad: &[u8],
            ciphertext: &[u8],
            tag: &[u8; 16],
            out: &mut Vec<u8>,
        ) -> Result<usize, String> {
            let mut nonce_copy = *nonce;
            let mut tag_copy = *tag;
            let mut aad_stack = [0u8; 16];
            let mut aad_heap = if aad.len() > aad_stack.len() {
                aad.to_vec()
            } else {
                Vec::new()
            };
            let (pb_auth_data, cb_auth_data) = if aad.is_empty() {
                (ptr::null_mut(), 0u32)
            } else if aad.len() <= aad_stack.len() {
                aad_stack[..aad.len()].copy_from_slice(aad);
                (aad_stack.as_mut_ptr(), aad.len() as u32)
            } else {
                (aad_heap.as_mut_ptr(), aad_heap.len() as u32)
            };
            let mut info = BcryptAuthModeInfo {
                cb_size: std::mem::size_of::<BcryptAuthModeInfo>() as u32,
                dw_info_version: BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO_VERSION,
                pb_nonce: nonce_copy.as_mut_ptr(),
                cb_nonce: 12,
                pb_auth_data,
                cb_auth_data,
                pb_tag: tag_copy.as_mut_ptr(),
                cb_tag: 16,
                pb_mac_context: ptr::null_mut(),
                cb_mac_context: 0,
                cb_aad: 0,
                cb_data: 0,
                dw_flags: 0,
            };
            let mut written: u32 = 0;
            if ciphertext.is_empty() {
                let st = unsafe {
                    BCryptDecrypt(
                        self.h_key,
                        ptr::null(),
                        0,
                        (&mut info as *mut BcryptAuthModeInfo).cast::<c_void>(),
                        ptr::null_mut(),
                        0,
                        ptr::null_mut(),
                        0,
                        &mut written,
                        0,
                    )
                };
                if st != 0 {
                    return Err(format!("BCryptDecrypt(AES-GCM) auth failed: 0x{:08x}", st as u32));
                }
                return Ok(0);
            }

            let old_len = out.len();
            out.resize(old_len + ciphertext.len(), 0);
            let st = unsafe {
                BCryptDecrypt(
                    self.h_key,
                    ciphertext.as_ptr(),
                    ciphertext.len() as u32,
                    (&mut info as *mut BcryptAuthModeInfo).cast::<c_void>(),
                    ptr::null_mut(),
                    0,
                    out[old_len..].as_mut_ptr(),
                    ciphertext.len() as u32,
                    &mut written,
                    0,
                )
            };
            if st != 0 {
                out.truncate(old_len);
                return Err(format!("BCryptDecrypt(AES-GCM) auth failed: 0x{:08x}", st as u32));
            }
            let written_usize = (written as usize).min(ciphertext.len());
            out.truncate(old_len + written_usize);
            Ok(written_usize)
        }

        pub fn decrypt(&self, nonce: &[u8; 12], aad: &[u8], ciphertext: &[u8], tag: &[u8; 16]) -> Result<Vec<u8>, String> {
            let mut out = Vec::with_capacity(ciphertext.len());
            self.decrypt_into(nonce, aad, ciphertext, tag, &mut out)?;
            Ok(out)
        }
    }

    /// Hardware-accelerated AES-128-CTR (128-bit big-endian counter, continuous keystream across chunks)
    /// backed by Windows CNG `bcrypt.dll` (`ChainingModeECB`).
    pub struct Aes128Ctr {
        h_alg: *mut c_void,
        h_key: *mut c_void,
        counter: u128,
        keystream_block: [u8; 16],
        keystream_pos: usize,
    }

    unsafe impl Send for Aes128Ctr {}
    unsafe impl Sync for Aes128Ctr {}

    impl Drop for Aes128Ctr {
        fn drop(&mut self) {
            unsafe {
                if !self.h_key.is_null() {
                    BCryptDestroyKey(self.h_key);
                }
                if !self.h_alg.is_null() {
                    BCryptCloseAlgorithmProvider(self.h_alg, 0);
                }
            }
        }
    }

    impl Aes128Ctr {
        pub fn new(key: &[u8; 16], iv: &[u8; 16]) -> Result<Self, String> {
            let alg_w = wide_nul("AES");
            let prop_w = wide_nul("ChainingMode");
            let mode_w = wide_nul("ChainingModeECB");
            let mut h_alg: *mut c_void = ptr::null_mut();
            let st_open = unsafe {
                BCryptOpenAlgorithmProvider(&mut h_alg, alg_w.as_ptr(), ptr::null(), 0)
            };
            if st_open != 0 || h_alg.is_null() {
                return Err(format!(
                    "BCryptOpenAlgorithmProvider(AES-ECB) failed: 0x{:08x}",
                    st_open as u32
                ));
            }
            let mode_bytes = unsafe {
                std::slice::from_raw_parts(mode_w.as_ptr() as *const u8, mode_w.len() * 2)
            };
            let st_prop = unsafe {
                BCryptSetProperty(
                    h_alg,
                    prop_w.as_ptr(),
                    mode_bytes.as_ptr(),
                    mode_bytes.len() as u32,
                    0,
                )
            };
            if st_prop != 0 {
                unsafe { BCryptCloseAlgorithmProvider(h_alg, 0) };
                return Err(format!(
                    "BCryptSetProperty(ECB) failed: 0x{:08x}",
                    st_prop as u32
                ));
            }
            let mut h_key: *mut c_void = ptr::null_mut();
            let st_key = unsafe {
                BCryptGenerateSymmetricKey(
                    h_alg,
                    &mut h_key,
                    ptr::null_mut(),
                    0,
                    key.as_ptr(),
                    key.len() as u32,
                    0,
                )
            };
            if st_key != 0 || h_key.is_null() {
                unsafe { BCryptCloseAlgorithmProvider(h_alg, 0) };
                return Err(format!(
                    "BCryptGenerateSymmetricKey(AES-128) failed: 0x{:08x}",
                    st_key as u32
                ));
            }
            Ok(Self {
                h_alg,
                h_key,
                counter: u128::from_be_bytes(*iv),
                keystream_block: [0u8; 16],
                keystream_pos: 16,
            })
        }

        fn encrypt_ecb_blocks(&self, input: &[u8], output: &mut [u8]) -> Result<(), String> {
            debug_assert_eq!(input.len(), output.len());
            debug_assert_eq!(input.len() % 16, 0);
            let mut written: u32 = 0;
            let st = unsafe {
                BCryptEncrypt(
                    self.h_key,
                    input.as_ptr(),
                    input.len() as u32,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    0,
                    output.as_mut_ptr(),
                    output.len() as u32,
                    &mut written,
                    0,
                )
            };
            if st != 0 || written as usize != output.len() {
                return Err(format!(
                    "BCryptEncrypt(AES-ECB) failed: 0x{:08x} (written={written})",
                    st as u32
                ));
            }
            Ok(())
        }

        fn refill_single_block(&mut self) -> Result<(), String> {
            let ctr_bytes = self.counter.to_be_bytes();
            self.counter = self.counter.wrapping_add(1);
            let mut block = [0u8; 16];
            self.encrypt_ecb_blocks(&ctr_bytes, &mut block)?;
            self.keystream_block = block;
            self.keystream_pos = 0;
            Ok(())
        }

        pub fn snapshot(&self) -> (u128, [u8; 16], usize) {
            (self.counter, self.keystream_block, self.keystream_pos)
        }

        pub fn restore(&mut self, snap: (u128, [u8; 16], usize)) {
            self.counter = snap.0;
            self.keystream_block = snap.1;
            self.keystream_pos = snap.2;
        }

        /// Applies the continuous AES-128-CTR keystream in-place (encrypt or decrypt).
        pub fn apply_keystream(&mut self, data: &mut [u8]) -> Result<(), String> {
            if data.is_empty() {
                return Ok(());
            }
            let mut offset = 0usize;
            // 1. Drain any remaining bytes from current partial 16-byte keystream block.
            if self.keystream_pos < 16 {
                let avail = 16 - self.keystream_pos;
                let take = avail.min(data.len());
                for i in 0..take {
                    data[i] ^= self.keystream_block[self.keystream_pos + i];
                }
                self.keystream_pos += take;
                offset += take;
            }

            // 2. Fast batched path: encrypt up to 256 counter blocks (4096 B) per CNG call.
            const BATCH_BLOCKS: usize = 256;
            let remaining_full_blocks = (data.len() - offset) / 16;
            if remaining_full_blocks > 0 {
                let mut ctr_in = vec![0u8; remaining_full_blocks.min(BATCH_BLOCKS) * 16];
                let mut ks_out = vec![0u8; ctr_in.len()];
                let mut blocks_left = remaining_full_blocks;
                while blocks_left > 0 {
                    let batch = blocks_left.min(BATCH_BLOCKS);
                    let byte_len = batch * 16;
                    for b in 0..batch {
                        ctr_in[b * 16..(b + 1) * 16].copy_from_slice(&self.counter.to_be_bytes());
                        self.counter = self.counter.wrapping_add(1);
                    }
                    self.encrypt_ecb_blocks(&ctr_in[..byte_len], &mut ks_out[..byte_len])?;
                    for i in 0..byte_len {
                        data[offset + i] ^= ks_out[i];
                    }
                    offset += byte_len;
                    blocks_left -= batch;
                }
            }

            // 3. Tail (< 16 bytes) leaves remainder buffered for continuous next call.
            if offset < data.len() {
                self.refill_single_block()?;
                let tail = data.len() - offset;
                for i in 0..tail {
                    data[offset + i] ^= self.keystream_block[i];
                }
                self.keystream_pos = tail;
            }

            Ok(())
        }
    }

    /// Hardware-accelerated AES-128-CBC (no PKCS7 padding, whole 16-byte blocks only,
    /// trailing `< 16` bytes untouched, IV reset per packet) backed by Windows CNG `bcrypt.dll`
    /// for AirPlay `type=96` RTP audio decryption (1:1 `zenlink-airplay/src/audio.rs:48-56`).
    pub struct Aes128CbcKey {
        h_alg: *mut c_void,
        h_key: *mut c_void,
    }

    unsafe impl Send for Aes128CbcKey {}
    unsafe impl Sync for Aes128CbcKey {}

    impl Drop for Aes128CbcKey {
        fn drop(&mut self) {
            unsafe {
                if !self.h_key.is_null() {
                    BCryptDestroyKey(self.h_key);
                }
                if !self.h_alg.is_null() {
                    BCryptCloseAlgorithmProvider(self.h_alg, 0);
                }
            }
        }
    }

    impl Aes128CbcKey {
        pub fn new(key: &[u8; 16]) -> Result<Self, String> {
            let alg_w = wide_nul("AES");
            let prop_w = wide_nul("ChainingMode");
            let mode_w = wide_nul("ChainingModeCBC");
            let mut h_alg: *mut c_void = ptr::null_mut();
            let st_open = unsafe {
                BCryptOpenAlgorithmProvider(&mut h_alg, alg_w.as_ptr(), ptr::null(), 0)
            };
            if st_open != 0 || h_alg.is_null() {
                return Err(format!(
                    "BCryptOpenAlgorithmProvider(AES-CBC) failed: 0x{:08x}",
                    st_open as u32
                ));
            }
            let mode_bytes = unsafe {
                std::slice::from_raw_parts(mode_w.as_ptr() as *const u8, mode_w.len() * 2)
            };
            let st_prop = unsafe {
                BCryptSetProperty(
                    h_alg,
                    prop_w.as_ptr(),
                    mode_bytes.as_ptr(),
                    mode_bytes.len() as u32,
                    0,
                )
            };
            if st_prop != 0 {
                unsafe { BCryptCloseAlgorithmProvider(h_alg, 0) };
                return Err(format!(
                    "BCryptSetProperty(CBC) failed: 0x{:08x}",
                    st_prop as u32
                ));
            }
            let mut h_key: *mut c_void = ptr::null_mut();
            let st_key = unsafe {
                BCryptGenerateSymmetricKey(
                    h_alg,
                    &mut h_key,
                    ptr::null_mut(),
                    0,
                    key.as_ptr(),
                    key.len() as u32,
                    0,
                )
            };
            if st_key != 0 || h_key.is_null() {
                unsafe { BCryptCloseAlgorithmProvider(h_alg, 0) };
                return Err(format!(
                    "BCryptGenerateSymmetricKey(AES-CBC) failed: 0x{:08x}",
                    st_key as u32
                ));
            }
            Ok(Self { h_alg, h_key })
        }

        /// Decrypts the whole 16-byte blocks of `data` in place with `iv` (reset per call, `dwFlags = 0`
        /// no padding); leaves trailing `data.len() % 16` bytes untouched (AirPlay audio RTP convention).
        pub fn decrypt_in_place(&self, iv: &[u8; 16], data: &mut [u8]) -> Result<(), String> {
            let full_blocks = (data.len() / 16) * 16;
            if full_blocks == 0 {
                return Ok(());
            }
            let mut iv_copy = *iv;
            let mut out = vec![0u8; full_blocks];
            let mut written: u32 = 0;
            let st = unsafe {
                BCryptDecrypt(
                    self.h_key,
                    data.as_ptr(),
                    full_blocks as u32,
                    ptr::null_mut(),
                    iv_copy.as_mut_ptr(),
                    16,
                    out.as_mut_ptr(),
                    full_blocks as u32,
                    &mut written,
                    0,
                )
            };
            if st != 0 || written as usize != full_blocks {
                return Err(format!(
                    "BCryptDecrypt(AES-CBC) failed: 0x{:08x} written={written}/{full_blocks}",
                    st as u32
                ));
            }
            data[..full_blocks].copy_from_slice(&out);
            Ok(())
        }

        /// Encrypts the whole 16-byte blocks of `data` in place with `iv` (`dwFlags = 0`, no padding),
        /// leaving trailing `data.len() % 16` bytes untouched (matches AirPlay sender / test vectors).
        pub fn encrypt_in_place(&self, iv: &[u8; 16], data: &mut [u8]) -> Result<(), String> {
            let full_blocks = (data.len() / 16) * 16;
            if full_blocks == 0 {
                return Ok(());
            }
            let mut iv_copy = *iv;
            let mut out = vec![0u8; full_blocks];
            let mut written: u32 = 0;
            let st = unsafe {
                BCryptEncrypt(
                    self.h_key,
                    data.as_ptr(),
                    full_blocks as u32,
                    ptr::null_mut(),
                    iv_copy.as_mut_ptr(),
                    16,
                    out.as_mut_ptr(),
                    full_blocks as u32,
                    &mut written,
                    0,
                )
            };
            if st != 0 || written as usize != full_blocks {
                return Err(format!(
                    "BCryptEncrypt(AES-CBC) failed: 0x{:08x} written={written}/{full_blocks}",
                    st as u32
                ));
            }
            data[..full_blocks].copy_from_slice(&out);
            Ok(())
        }
    }

    pub fn aes128_cbc_decrypt_in_place(
        key: &[u8; 16],
        iv: &[u8; 16],
        data: &mut [u8],
    ) -> Result<(), String> {
        let cbc = Aes128CbcKey::new(key)?;
        cbc.decrypt_in_place(iv, data)
    }
}

#[cfg(windows)]
pub use native::{
    aes128_cbc_decrypt_in_place, fill_random, hmac_sha256_chunks, hmac_sha512_chunks,
    sha256_chunks, sha512_chunks, Aes128CbcKey, Aes128Ctr, AesGcmKey,
};

pub fn sha256(data: &[u8]) -> [u8; 32] {
    sha256_chunks(&[data])
}

pub fn sha512(data: &[u8]) -> [u8; 64] {
    sha512_chunks(&[data])
}

pub fn hkdf_sha512_32(ikm: &[u8], salt: &[u8], info: &[u8]) -> [u8; 32] {
    let real_salt: &[u8] = if salt.is_empty() { &[0u8; 64] } else { salt };
    let prk = hmac_sha512_chunks(real_salt, &[ikm]);
    let okm = hmac_sha512_chunks(&prk, &[info, &[1u8]]);
    let mut out = [0u8; 32];
    out.copy_from_slice(&okm[..32]);
    out
}

pub fn tls12_prf_sha256(secret: &[u8], label: &[u8], seed: &[u8], out_len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(out_len);
    let mut a = hmac_sha256_chunks(secret, &[label, seed]);
    while out.len() < out_len {
        let p = hmac_sha256_chunks(secret, &[&a, label, seed]);
        let need = (out_len - out.len()).min(32);
        out.extend_from_slice(&p[..need]);
        a = hmac_sha256_chunks(secret, &[&a]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_win_cng_sha_hmac_hkdf_and_aes_gcm_roundtrip() {
        let h256 = sha256(b"abc");
        assert_eq!(h256[0], 0xBA);
        assert_eq!(h256[31], 0xAD);

        let h512 = sha512(b"abc");
        assert_eq!(h512[0], 0xDD);
        assert_eq!(h512[63], 0x9F);

        let key = AesGcmKey::new(&[0x11u8; 16]).expect("AES-128-GCM key");
        let nonce = [0x22u8; 12];
        let aad = b"tls-record-hdr";
        let msg = b"Hello iPhone 17 Pro Wi-Fi TLS-PSK!";
        let (ct, tag) = key.encrypt(&nonce, aad, msg).expect("encrypt");
        assert_ne!(ct.as_slice(), msg.as_slice());
        let pt = key.decrypt(&nonce, aad, &ct, &tag).expect("decrypt");
        assert_eq!(pt.as_slice(), msg.as_slice());
        let mut bad_tag = tag;
        bad_tag[0] ^= 1;
        assert!(key.decrypt(&nonce, aad, &ct, &bad_tag).is_err());
    }

    #[test]
    fn test_tahap252_aes128_ctr_nist_vector_and_continuous_keystream() {
        fn unhex(s: &str) -> Vec<u8> {
            (0..s.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
                .collect()
        }
        // Official NIST SP 800-38A Section F.5.1 (CTR-AES128.Encrypt)
        let key: [u8; 16] = unhex("2b7e151628aed2a6abf7158809cf4f3c").try_into().unwrap();
        let iv: [u8; 16] = unhex("f0f1f2f3f4f5f6f7f8f9fafbfcfdfeff").try_into().unwrap();
        let pt = unhex(concat!(
            "6bc1bee22e409f96e93d7e117393172a",
            "ae2d8a571e03ac9c9eb76fac45af8e51",
            "30c81c46a35ce411e5fbc1191a0a52ef",
            "f69f2445df4f9b17ad2b417be66c3710"
        ));
        let expected_ct = unhex(concat!(
            "874d6191b620e3261bef6864990db6ce",
            "9806f66b7970fdff8617187bb9fffdff",
            "5ae4df3edbd5d35e5b4f09020db03eab",
            "1e031dda2fbe03d1792170a0f3009cee"
        ));

        // 1. One-shot encryption
        let mut buf = pt.clone();
        let mut enc = Aes128Ctr::new(&key, &iv).unwrap();
        enc.apply_keystream(&mut buf).unwrap();
        assert_eq!(buf, expected_ct, "NIST SP 800-38A F.5.1 AES-128-CTR mismatch");

        // 2. Continuous keystream across arbitrary unaligned chunk boundaries (3, 14, 1, 29, 17 bytes)
        let mut dec = Aes128Ctr::new(&key, &iv).unwrap();
        let mut split_buf = expected_ct.clone();
        let splits = [3usize, 14, 1, 29, 17];
        let mut pos = 0usize;
        for s in splits {
            dec.apply_keystream(&mut split_buf[pos..pos + s]).unwrap();
            pos += s;
        }
        assert_eq!(pos, pt.len());
        assert_eq!(split_buf, pt, "Unaligned continuous keystream decryption must match plaintext");
    }

    #[test]
    fn test_tahap254_aes128_ctr_odd_length_chunks_match_continuous_stream() {
        let key: [u8; 16] = [
            0x0F, 0x1E, 0x2D, 0x3C, 0x4B, 0x5A, 0x69, 0x78,
            0x87, 0x96, 0xA5, 0xB4, 0xC3, 0xD2, 0xE1, 0xF0,
        ];
        // IV near wraparound boundary in low 64 bits to verify 128-bit big-endian carry!
        let iv: [u8; 16] = [
            0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0,
            0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFE,
        ];
        let chunk_lens: [usize; 50] = [
            1, 3, 7, 15, 17, 31, 33, 47, 129, 513,
            4099, 2, 5, 11, 13, 19, 23, 29, 37, 41,
            53, 61, 67, 73, 77, 89, 97, 131, 193, 211,
            255, 257, 317, 509, 769, 1021, 1031, 2053, 4093, 4097,
            8191, 8209, 16381, 32771, 65537, 15, 1, 16, 17, 99,
        ];
        let total_len: usize = chunk_lens.iter().sum();
        let plaintext: Vec<u8> = (0..total_len)
            .map(|i| ((i.wrapping_mul(31).wrapping_add(17)) & 0xFF) as u8)
            .collect();

        // 1. Single-buffer continuous reference encryption
        let mut ref_ct = plaintext.clone();
        let mut ref_ctr = Aes128Ctr::new(&key, &iv).expect("ref ctr");
        ref_ctr.apply_keystream(&mut ref_ct).expect("ref apply");

        // 2. Chunked odd-length encryption across all 50 misaligned sizes (plus 0-len no-op)
        let mut chunked_ct = plaintext.clone();
        let mut enc_ctr = Aes128Ctr::new(&key, &iv).expect("enc ctr");
        let mut off = 0usize;
        for &clen in &chunk_lens {
            enc_ctr.apply_keystream(&mut []).expect("empty no-op");
            enc_ctr
                .apply_keystream(&mut chunked_ct[off..off + clen])
                .expect("chunk enc");
            off += clen;
        }
        assert_eq!(off, total_len);
        assert_eq!(
            chunked_ct, ref_ct,
            "50 odd-length chunked Aes128Ctr encryptions must match single continuous stream byte-for-byte!"
        );

        // 3. Decrypt back using a DIFFERENT unaligned partition (reverse chunk lengths)
        let mut recovered = chunked_ct.clone();
        let mut dec_ctr = Aes128Ctr::new(&key, &iv).expect("dec ctr");
        let mut roff = 0usize;
        for &clen in chunk_lens.iter().rev() {
            dec_ctr
                .apply_keystream(&mut recovered[roff..roff + clen])
                .expect("chunk dec");
            roff += clen;
        }
        assert_eq!(roff, total_len);
        assert_eq!(
            recovered, plaintext,
            "Reverse-partitioned odd-length Aes128Ctr decryption must recover exact plaintext!"
        );
    }

    #[test]
    fn test_tahap258_aes128_cbc_decrypt_matches_reference_and_preserves_unencrypted_tail() {
        fn unhex(s: &str) -> Vec<u8> {
            (0..s.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
                .collect()
        }

        // 1. Exact AirPlay audio CBC vector (zenlink-airplay/src/audio.rs:109):
        //    key = 0..16, iv = 16..32, plaintext = [0x8C, 1..40] (32B CBC + 8B untouched tail!)
        let ref_key: [u8; 16] = std::array::from_fn(|i| i as u8);
        let ref_iv: [u8; 16] = std::array::from_fn(|i| 16 + i as u8);
        let mut ref_ct = unhex(
            "3efbe32a44b2dff306a188b949a98be94a290ca281882feeeff354b2ce5bcef72021222324252627",
        );
        aes128_cbc_decrypt_in_place(&ref_key, &ref_iv, &mut ref_ct)
            .expect("CBC decrypt reference vector");
        let expected: Vec<u8> = std::iter::once(0x8C).chain(1..40).collect();
        assert_eq!(
            ref_ct, expected,
            "CNG AES-128-CBC must match reference vector & leave 8B tail untouched!"
        );

        // 2. Roundtrip across multiple buffer sizes (16, 32, 37, 141, 485 bytes + <16B short buffer)
        let key: [u8; 16] = [
            0x2B, 0x7E, 0x15, 0x16, 0x28, 0xAE, 0xD2, 0xA6,
            0xAB, 0xF7, 0x15, 0x88, 0x09, 0xCF, 0x4F, 0x3C,
        ];
        let iv: [u8; 16] = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07,
            0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F,
        ];
        let cbc = Aes128CbcKey::new(&key).expect("Aes128CbcKey::new");
        for &len in &[7usize, 16, 32, 37, 141, 485] {
            let pt: Vec<u8> = (0..len).map(|i| ((i * 29 + 13) & 0xFF) as u8).collect();
            let mut buf = pt.clone();
            let full = (len / 16) * 16;
            cbc.encrypt_in_place(&iv, &mut buf).expect("CBC encrypt");
            if full > 0 {
                assert_ne!(&buf[..full], &pt[..full], "len={len} blocks must be encrypted");
            }
            assert_eq!(
                &buf[full..],
                &pt[full..],
                "len={len} trailing {} bytes must stay unencrypted!",
                len - full
            );
            cbc.decrypt_in_place(&iv, &mut buf).expect("CBC decrypt");
            assert_eq!(buf, pt, "len={len} CBC decrypt must recover exact plaintext!");
        }
    }

    #[test]
    fn test_tahap281_aes_gcm_decrypt_into_zero_alloc_roundtrip() {
        let key = AesGcmKey::new(&[0x3Au8; 16]).expect("AES-128-GCM key");
        let records: [([u8; 12], [u8; 13], Vec<u8>); 4] = [
            (
                [0x01u8; 12],
                [0, 0, 0, 0, 0, 0, 0, 1, 0x17, 0x03, 0x03, 0, 0],
                Vec::new(),
            ),
            (
                [0x02u8; 12],
                [0, 0, 0, 0, 0, 0, 0, 2, 0x17, 0x03, 0x03, 0, 17],
                (0..17u8).map(|i| i.wrapping_mul(11).wrapping_add(5)).collect(),
            ),
            (
                [0x03u8; 12],
                [0, 0, 0, 0, 0, 0, 0, 3, 0x17, 0x03, 0x03, 0x04, 0x00],
                (0..1024usize).map(|i| ((i * 29 + 7) & 0xFF) as u8).collect(),
            ),
            (
                [0x04u8; 12],
                [0, 0, 0, 0, 0, 0, 0, 4, 0x17, 0x03, 0x03, 0x40, 0x00],
                (0..16384usize).map(|i| ((i * 53 + 19) & 0xFF) as u8).collect(),
            ),
        ];

        let mut encrypted = Vec::with_capacity(records.len());
        for (nonce, aad, pt) in &records {
            let (ct, tag) = key.encrypt(nonce, aad, pt).expect("encrypt");
            encrypted.push((*nonce, *aad, pt.clone(), ct, tag));
        }

        let mut out = Vec::with_capacity(32768);
        let initial_cap = out.capacity();
        let mut expected = Vec::new();
        for (nonce, aad, pt, ct, tag) in &encrypted {
            let written = key
                .decrypt_into(nonce, aad, ct, tag, &mut out)
                .expect("decrypt_into");
            assert_eq!(written, pt.len(), "written byte count mismatch");
            expected.extend_from_slice(pt);
        }
        assert_eq!(out, expected, "multi-record decrypt_into plaintext mismatch");
        assert_eq!(
            out.capacity(),
            initial_cap,
            "decrypt_into must reuse pre-allocated output capacity without reallocating"
        );

        // Tampered tag MUST return Err and roll back out.len() cleanly!
        let len_before = out.len();
        let (nonce, aad, _, ct, tag) = &encrypted[2];
        let mut bad_tag = *tag;
        bad_tag[15] ^= 0x80;
        let err = key.decrypt_into(nonce, aad, ct, &bad_tag, &mut out);
        assert!(err.is_err(), "tampered GCM tag must fail authentication");
        assert_eq!(
            out.len(),
            len_before,
            "failed decrypt_into must truncate out back to old_len without dirty trailing bytes"
        );
        assert_eq!(out, expected);
    }
}
