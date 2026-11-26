//! Legacy Apple TV 3 AirPlay Pair-Verify (`POST /pair-verify`):
//! X25519 ECDH + Ed25519 signature + Windows CNG `Aes128Ctr`.

use crate::rp_wifi_tls::{x25519, x25519_public};
use crate::win_cng::{fill_random, Aes128Ctr};

use super::identity::AirPlayIdentity;
use super::sha512_16;

#[derive(Default, Debug)]
pub struct PairVerify {
    ours_secret: Option<[u8; 32]>,
    theirs_x: [u8; 32],
    theirs_ed: [u8; 32],
    /// X25519 shared secret produced during Step 1.
    pub shared_secret: Option<[u8; 32]>,
}

impl PairVerify {
    fn cipher(secret: &[u8; 32]) -> Result<Aes128Ctr, String> {
        let key = sha512_16(&[b"Pair-Verify-AES-Key", secret]);
        let iv = sha512_16(&[b"Pair-Verify-AES-IV", secret]);
        Aes128Ctr::new(&key, &iv)
    }

    /// Handles a `POST /pair-verify` request body (68 bytes for Step 1 or Step 2).
    pub fn handle(&mut self, id: &AirPlayIdentity, body: &[u8]) -> Vec<u8> {
        let mut ephemeral = [0u8; 32];
        let _ = fill_random(&mut ephemeral);
        self.handle_with_ephemeral(id, body, ephemeral)
    }

    pub fn handle_with_ephemeral(
        &mut self,
        id: &AirPlayIdentity,
        body: &[u8],
        our_secret: [u8; 32],
    ) -> Vec<u8> {
        if body.len() == 68 && body[0] == 1 {
            self.theirs_x.copy_from_slice(&body[4..36]);
            self.theirs_ed.copy_from_slice(&body[36..68]);
            let ours_pub = x25519_public(&our_secret);
            let secret = x25519(&our_secret, &self.theirs_x);

            let mut msg = Vec::with_capacity(64);
            msg.extend_from_slice(&ours_pub);
            msg.extend_from_slice(&self.theirs_x);
            let mut sig = id.sign(&msg);
            if let Ok(mut ctr) = Self::cipher(&secret) {
                let _ = ctr.apply_keystream(&mut sig);
            }
            self.ours_secret = Some(our_secret);
            self.shared_secret = Some(secret);

            let mut out = Vec::with_capacity(96);
            out.extend_from_slice(&ours_pub);
            out.extend_from_slice(&sig);
            return out;
        }
        if body.len() == 68 && body[0] == 0 {
            if let Some(secret) = self.shared_secret {
                if let Ok(mut ctr) = Self::cipher(&secret) {
                    let mut skip = [0u8; 64];
                    let _ = ctr.apply_keystream(&mut skip);
                    let mut client_sig = [0u8; 64];
                    client_sig.copy_from_slice(&body[4..68]);
                    let _ = ctr.apply_keystream(&mut client_sig);
                }
            }
        }
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::airplay::hex_decode;

    #[test]
    fn test_pair_verify_matches_known_airplay_vector() {
        let seed: [u8; 32] = core::array::from_fn(|i| i as u8);
        let id = AirPlayIdentity::from_seed_and_mac(seed, "0A:11:22:33:44:55".into());
        assert_eq!(
            id.public_key.as_slice(),
            hex_decode("03a107bff3ce10be1d70dd18e74bc09967e4d6309ba50d5f1ddc8664125531b8").as_slice()
        );

        let mut pv = PairVerify::default();
        let mut req1 = vec![1u8, 0, 0, 0];
        req1.extend_from_slice(&hex_decode(
            "57db4b359f23ae5e146e4e2512056704722506348c150c14753d0c933d04d421",
        ));
        req1.extend_from_slice(&hex_decode(
            "6e7a1cdd29b0b78fd13af4c5598feff4ef2a97166e3ca6f2e4fbfccd80505bf1",
        ));
        let reply1 = pv.handle_with_ephemeral(&id, &req1, [7u8; 32]);
        assert_eq!(
            reply1,
            hex_decode("13be4feaeaf204c7fd3358fc9c00721881d174278128227ec674f37f7fe97b6dbd7006b5fbd3c8778c39f16495e20bb721bbf896df1a3c421d2609e0b9581791dd76f3bbdc72550fc5d07d5e3052ebdc4ac629412ec5b571a1db4ff9842fd45b")
        );
        assert_eq!(
            pv.shared_secret.unwrap().as_slice(),
            hex_decode("2ffe325b2a26611bf1911325d4a62b33749b8f350d0e88ba91e316acaae66554").as_slice()
        );
    }
}
