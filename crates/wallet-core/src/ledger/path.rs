//! An EIP-2645 derivation path as the Starknet app takes it: `serializePath`
//! (`hw-app-starknet` 3.0.2, `lib/esm/helper.js:4-21`), from components rather than text. Which
//! paths a wallet uses (Bramble's `LedgerW` and Ready's `argentx` schemes) is the wallet's.

use std::fmt;

/// The hardened bit of a BIP-32 child number.
const HARDENED: u32 = 0x8000_0000;
/// EIP-2645's purpose, the first level of every path.
const PURPOSE: u32 = 2645;

/// One level of a path: a child index below 2^31, hardened or not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathComponent {
    pub index: u32,
    pub hardened: bool,
}

impl PathComponent {
    /// `<index>'`.
    pub const fn hardened(index: u32) -> Self {
        Self {
            index,
            hardened: true,
        }
    }

    /// `<index>`.
    pub const fn plain(index: u32) -> Self {
        Self {
            index,
            hardened: false,
        }
    }
}

/// A six-level EIP-2645 path, `m/2645'/...`: the shape every path the Starknet app derives at
/// has, and the only one the extension's schemes produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LedgerPath([PathComponent; 6]);

impl LedgerPath {
    /// The path, or `None` when it does not start at `2645'` or an index is 2^31 or more (the
    /// library throws on both, "Path should start with 'm/2645'" and "Incorrect child value").
    pub fn new(components: [PathComponent; 6]) -> Option<Self> {
        let rooted = components[0] == PathComponent::hardened(PURPOSE);
        let in_range = components.iter().all(|c| c.index < HARDENED);
        (rooted && in_range).then_some(Self(components))
    }

    /// The APDU data: each level's child number (the hardened bit added) as a big-endian `u32`.
    pub fn to_bytes(&self) -> [u8; 24] {
        let mut bytes = [0; 24];
        for (chunk, component) in bytes.chunks_exact_mut(4).zip(&self.0) {
            let child = component.index | if component.hardened { HARDENED } else { 0 };
            chunk.copy_from_slice(&child.to_be_bytes());
        }
        bytes
    }
}

/// The path's text, `m/2645'/1195502025'/...`, as the extension stores and the library parses it.
impl fmt::Display for LedgerPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("m")?;
        for component in &self.0 {
            let mark = if component.hardened { "'" } else { "" };
            write!(f, "/{}{mark}", component.index)?;
        }
        Ok(())
    }
}
