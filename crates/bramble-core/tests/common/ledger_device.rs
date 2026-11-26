//! A scripted Starknet Ledger app holding one key, for the suites that sign on a device (the
//! multisig co-signer's and the dapp typed-data ones). Krusty signs what the device is sent, so
//! its signatures are the ones a software key makes.

use starknet_wallet_core::{felt, sign_stark_hash, Felt, SecretFelt};

pub const OK: [u8; 2] = [0x90, 0x00];
const USER_REJECTED: [u8; 2] = [0x6e, 0x04];

/// A scripted Starknet app holding one key.
pub struct Device {
    pub key: SecretFelt,
    /// Answer `signHash` with the person's rejection, this many times.
    pub reject: usize,
    /// Sign another hash than the one sent.
    pub tamper: bool,
    pub apdus: usize,
}

impl Device {
    pub fn holding(key: SecretFelt) -> Self {
        Self {
            key,
            reject: 0,
            tamper: false,
            apdus: 0,
        }
    }

    pub fn answer(&mut self, apdu: &[u8]) -> Vec<u8> {
        self.apdus += 1;
        let f = |hex: &str| Felt::from_hex(hex).unwrap();
        match (apdu[1], apdu[2]) {
            (0x00, _) => [&[2, 4, 0][..], &OK].concat(),
            (0x01, _) => {
                let x = f(&sign_stark_hash(&self.key, "0x1").unwrap().public_key);
                [&[0x04][..], &x.to_bytes_be(), &[0x22; 32], &OK].concat()
            }
            (0x02, 0) => OK.to_vec(),
            (0x02, 1) if self.reject > 0 => {
                self.reject -= 1;
                USER_REJECTED.to_vec()
            }
            (0x02, 1) => {
                let mut hash = Felt::from_bytes_be_slice(&apdu[5..37]);
                if self.tamper {
                    hash += Felt::ONE;
                }
                let signature = sign_stark_hash(&self.key, &felt::canonical(&hash)).unwrap();
                let (r, s) = (f(&signature.r), f(&signature.s));
                [
                    &[0x41][..],
                    &r.to_bytes_be(),
                    &s.to_bytes_be(),
                    &[0x00],
                    &OK,
                ]
                .concat()
            }
            other => panic!("unexpected APDU {other:?}"),
        }
    }
}
