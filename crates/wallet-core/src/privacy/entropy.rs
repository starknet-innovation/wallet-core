//! The privacy SDK's randomness (`utils/crypto.ts`), over a byte source the caller picks, so a
//! test can replay the stream the extension's SDK read and get the same values.
//!
//! The SDK is pinned to starknet.js 10.5.0, whose `@scure/starknet` 1.1.0 runs on
//! `@noble/curves` 1.7.0. Its `randomPrivateKey()` reads `getMinHashLength(n)` = 48 bytes in one
//! `crypto.getRandomValues` call and passes them to `mapHashToField(bytes, n)`, which (with its
//! default `isLE = false`) reads them **little-endian**, then returns `x mod (n - 1) + 1` as 32
//! big-endian bytes; `encode.uint8ArrayToBigInt` reads those back big-endian.

use zeroize::Zeroizing;

use crate::{envelope, Error, Felt};

/// Stark curve order `n` minus one: `mapHashToField` reduces modulo it, then adds one.
const CURVE_ORDER_LESS_ONE: Felt =
    Felt::from_hex_unchecked("0x0800000000000010ffffffffffffffffb781126dcae7b2321e66a241adc64d2e");

/// A source of random bytes.
pub trait Entropy {
    /// Fills all of `bytes`, or fails.
    fn fill(&mut self, bytes: &mut [u8]) -> Result<(), Error>;
}

/// The OS CSPRNG, through the vault's randomness boundary.
pub struct OsEntropy;

impl Entropy for OsEntropy {
    fn fill(&mut self, bytes: &mut [u8]) -> Result<(), Error> {
        envelope::fill(bytes)
    }
}

/// `generateRandom()`: `uint8ArrayToBigInt(starkCurve.utils.randomPrivateKey())`, a value in
/// `[1, n)`.
pub fn generate_random(entropy: &mut impl Entropy) -> Result<Felt, Error> {
    let mut bytes = Zeroizing::new([0u8; 48]);
    entropy.fill(bytes.as_mut())?;
    // The 384-bit little-endian integer, rebuilt exactly from 128-bit limbs (each below the
    // field prime), most significant first.
    let value = bytes
        .chunks_exact(16)
        .rev()
        .fold(Felt::ZERO.to_biguint(), |value, limb| {
            (value << 128u32) + Felt::from_bytes_le_slice(limb).to_biguint()
        });
    let reduced = value % CURVE_ORDER_LESS_ONE.to_biguint();
    Ok(Felt::from(&reduced) + Felt::ONE)
}

/// `generateRandom120()`: 15 bytes from one `crypto.getRandomValues` call, read big-endian.
pub fn generate_random120(entropy: &mut impl Entropy) -> Result<u128, Error> {
    let mut bytes = Zeroizing::new([0u8; 16]);
    entropy.fill(&mut bytes[1..])?;
    Ok(u128::from_be_bytes(*bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hands out a fixed stream and records each read's length.
    struct Fixed(Vec<u8>, Vec<usize>);

    impl Entropy for Fixed {
        fn fill(&mut self, bytes: &mut [u8]) -> Result<(), Error> {
            if self.0.len() < bytes.len() {
                return Err(Error::storage("entropy unavailable"));
            }
            bytes.copy_from_slice(&self.0.drain(..bytes.len()).collect::<Vec<_>>());
            self.1.push(bytes.len());
            Ok(())
        }
    }

    #[test]
    fn random_reads_48_bytes_little_endian_and_lands_in_one_to_n() {
        let mut zeros = Fixed(vec![0; 48], vec![]);
        assert_eq!(generate_random(&mut zeros).unwrap(), Felt::ONE);
        assert_eq!(zeros.1, [48]);
        // Little-endian: a leading 2 is the value 2.
        let mut two = vec![0; 48];
        two[0] = 2;
        assert_eq!(
            generate_random(&mut Fixed(two, vec![])).unwrap(),
            Felt::THREE
        );
        // 2^384 - 1 mod (n - 1), plus one, stays below n.
        let top = generate_random(&mut Fixed(vec![0xff; 48], vec![])).unwrap();
        assert!(top < CURVE_ORDER_LESS_ONE + Felt::ONE);
        assert!(generate_random(&mut Fixed(vec![0; 47], vec![])).is_err());
    }

    #[test]
    fn random120_reads_15_bytes_big_endian() {
        let mut stream = Fixed((1..=15).collect(), vec![]);
        assert_eq!(
            generate_random120(&mut stream).unwrap(),
            0x0102030405060708090a0b0c0d0e0f
        );
        assert_eq!(stream.1, [15]);
    }

    #[test]
    fn os_entropy_fills() {
        assert!(generate_random(&mut OsEntropy).unwrap() != Felt::ZERO);
    }
}
