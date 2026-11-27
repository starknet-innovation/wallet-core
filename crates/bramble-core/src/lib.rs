//! Bramble wallet core.
//!
//! The one library behind Bramble's browser extension (as a wasm32 module), iOS and Android. The
//! wallet logic lives in `wallet-core`; this crate holds what is Bramble's choice and exports it
//! through UniFFI:
//!
//! - which OpenZeppelin account version Bramble deploys, and the salt its `KmsPort` defaults to;
//! - which functions cross the FFI boundary (`raw-keys`);
//! - multisig coordination: the identifier, relays, page cap, sync window and who is a
//!   co-signer (`coordination`, plain Rust until the swap);
//! - its catalogs (`catalogs`: networks, token registry, account classes, multisig constants,
//!   embedded from `generated/`) and its portfolio, history and allowance choices
//!   (`PORTFOLIO_POLICY`, `HISTORY_POLICY`, `ALLOWANCE_POLICY`, `swap::SWAP_POLICY`,
//!   `staking::STAKING_POLICY`), plain Rust until the swap;
//! - its dapp provider budgets, advertised versions and own `mc_*` methods (`dapp`);
//! - its Ledger minimum versions and path schemes (`ledger`), plain Rust until the swap;
//! - the extension's v13 vault schema, which mobile writes (`vault_v13`, behind `mobile`).
//!
//! Functions keep the names and argument order the extension's `KmsPort` uses.

#![forbid(unsafe_code)]
// A panic in a wallet is a crash: library code returns errors (unit tests may unwrap: clippy.toml).
#![deny(clippy::unwrap_used, clippy::expect_used)]

// The mobile build returns no raw key and takes one, only to seal it (`import_private_key`; see
// AGENTS.md). Cargo unifies features, so one dependent that forgets `default-features = false`
// would silently add the `raw-keys` exports; refuse that build instead. Only the test suites,
// which run mobile's tests beside the extension's, combine the two.
#[cfg(all(feature = "mobile", feature = "raw-keys", not(feature = "testing")))]
compile_error!(
    "`mobile` with `raw-keys` exports raw private keys; build mobile with --no-default-features"
);

#[cfg(any(feature = "raw-keys", feature = "mobile"))]
pub(crate) use starknet_wallet_core::felt;
#[cfg(feature = "raw-keys")]
use starknet_wallet_core::SecretFelt;
#[cfg(feature = "extension")]
use starknet_wallet_core::Zeroizing;
pub use starknet_wallet_core::{NetworkId, NostrApplicationData, StarkSignature};

// The wasm2 player needs the allocator and panic hook this crate exports; nothing else
// references it, so the linker would drop it without this line.
#[cfg(target_arch = "wasm32")]
extern crate uniffi_runtime_wasm as _;

uniffi::setup_scaffolding!();

pub mod catalogs;
pub mod coordination;
pub mod dapp;
pub mod gasless;
pub mod ledger;
pub mod nft;
pub mod staking;
pub mod swap;

// The mobile wallet's surface, at the crate root where mobile had it.
#[cfg(feature = "mobile")]
include!("mobile.rs");

/// The OpenZeppelin account version Bramble deploys (`OZ_ACCOUNT_VERSION` in
/// `vault/src/accounts/account-contract.ts`). Pinned, never Krusty's `latest`: a manifest bump
/// would otherwise move every derived address.
pub const OZ_ACCOUNT_VERSION: &str = "3.0.0";

/// The margin starknet.js `toOverheadResourceBounds` adds to every estimated amount and price
/// before the extension signs them (its default overhead, which the extension does not change).
pub const FEE_MARGIN_PERCENT: u32 = 50;

/// `REVIEWED_FEE_HEADROOM`: a submit-time fee may reach 3/2 of the fee the person reviewed.
pub const REVIEWED_FEE_HEADROOM: starknet_wallet_core::send::fees::Headroom =
    starknet_wallet_core::send::fees::Headroom {
        numerator: 3,
        denominator: 2,
    };

/// The extension's `CONFIRMATION_DELAYS_MS`: when a broadcast transaction's status is read.
pub const CONFIRMATION_DELAYS_MS: [u32; 12] = [
    0, 250, 500, 900, 1_500, 2_500, 4_000, 5_000, 7_500, 10_000, 10_000, 10_000,
];

/// `ARGENT_MULTISIG_RESOURCE_BOUNDS_OVERHEAD` (`rpc/multisig-invoke.ts`): a multisig proposal
/// waits hours or days for co-signers, so the two L1 price lanes tolerate a 64x move (6300%)
/// while amounts and the L2 price keep starknet.js's 50%.
pub const MULTISIG_FEE_OVERHEAD: starknet_wallet_core::send::fees::Overhead = {
    use starknet_wallet_core::send::fees::{LaneMargin, Overhead};
    let l1 = LaneMargin {
        amount_percent: 50,
        price_percent: 6_300,
    };
    Overhead {
        l1_gas: l1,
        l2_gas: LaneMargin {
            amount_percent: 50,
            price_percent: 50,
        },
        l1_data_gas: l1,
    }
};

/// `INVOKE_LIMITS.calls` (`wallet-protocol/src/transactions/invoke-limits.ts`): the most calls
/// the extension accepts in one invoke, the largest batch its review fits (Bramble 08f144034). It
/// bounds a dapp's request there; here every call batch. Argent multisig proposals keep their own
/// 16 (`send::multisig_invoke::MAX_CALLS`).
pub const MAX_INVOKE_CALLS: usize = 56;

/// `INVOKE_LIMITS.calldataFelts` (the same file): the most calldata felts in one call.
pub const MAX_CALL_CALLDATA_FELTS: usize = 4_096;

/// Bramble's send numbers, for `starknet_wallet_core::send::plan::Plan`.
pub const SEND_POLICY: starknet_wallet_core::send::plan::Policy =
    starknet_wallet_core::send::plan::Policy {
        fee_margin_percent: FEE_MARGIN_PERCENT,
        reviewed_fee_headroom: REVIEWED_FEE_HEADROOM,
        confirmation_delays_ms: &CONFIRMATION_DELAYS_MS,
        max_invoke_calls: MAX_INVOKE_CALLS,
        max_call_calldata_felts: MAX_CALL_CALLDATA_FELTS,
    };

/// Bramble's portfolio choices, for `starknet_wallet_core::portfolio`'s stores: the schema tag of
/// its saved portfolio documents, CoinGecko (`prices-feed.ts`), and fiat on mainnet only.
pub const PORTFOLIO_POLICY: starknet_wallet_core::portfolio::PortfolioPolicy =
    starknet_wallet_core::portfolio::PortfolioPolicy {
        schema: "bramble.mobile.portfolio.v1",
        price_api_base: "https://api.coingecko.com/api/v3",
        price_network: NetworkId::Mainnet,
        // `MAX_ADDED_TOKENS_PER_NETWORK`: a private balance reads at most 64 tokens.
        max_added_tokens: 50,
    };

/// Bramble's history choices, for `starknet_wallet_core::history`: `STARKSCAN_API_BASE`
/// (`history/starkscan.ts`), `matchingToken`'s untracked-token name, and the pre-confirmed
/// `PROVISIONAL_TX_HINT`.
pub const HISTORY_POLICY: starknet_wallet_core::history::HistoryPolicy =
    starknet_wallet_core::history::HistoryPolicy {
        explorer_api_base: "https://starkscan.co/v1",
        untracked_token_name: "Untracked token",
        preconfirmed_hint: "Pre-confirmed, not final yet. Bramble tells you if it doesn't settle.",
    };

/// Bramble's allowance numbers, for `starknet_wallet_core::allowances`: the discovery bounds of
/// `transactions/src/allowances/allowance-scan.ts` (`OWNER_EVENT_PAGES`, `LOG_EVENT_PAGES`,
/// `EVENT_CHUNK_SIZE`, `MAX_SPENDERS_PER_TOKEN`, `LEGACY_BLOCK_WINDOW`, `LEGACY_WINDOWS`,
/// `MAX_TRACKED_PAIRS`), and the retention caps and storage key of `allowance-store.ts`
/// (`ALLOWANCE_LIMITS`, `ALLOWANCE_STORE_KEY`), so the extension's stored log reads unchanged.
pub const ALLOWANCE_POLICY: starknet_wallet_core::allowances::AllowancePolicy =
    starknet_wallet_core::allowances::AllowancePolicy {
        owner_event_pages: 5,
        log_event_pages: 3,
        event_chunk_size: 100,
        max_spenders_per_token: 50,
        legacy_block_window: 200_000,
        legacy_windows: 3,
        max_tracked_pairs: 60,
        entries_per_scope: 50,
        entries_total: 200,
        store_key: "bramble:allowances",
    };

// `allowance-store-budget.test.ts` "the per-scope cap leaves room for several coexisting scopes",
// checked when the crate builds.
const _: () = assert!(ALLOWANCE_POLICY.entries_per_scope * 4 <= ALLOWANCE_POLICY.entries_total);

/// Every failure the core reports across the FFI boundary.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum CoreError {
    #[error("invalid input: {reason}")]
    InvalidInput { reason: String },
    #[error("key management failure during {operation}")]
    Kms { operation: String },
    #[error("wrong password")]
    WrongPassword,
    #[error("storage failure: {reason}")]
    Storage { reason: String },
    /// Biometric unlock is not set up (no slot, or the vault predates it).
    #[error("biometric unlock is not enabled")]
    BiometricUnavailable,
    /// The biometric slot no longer opens.
    #[error("biometric unlock was invalidated")]
    BiometricInvalidated,
    /// A node reply the core will not interpret.
    #[error("rpc failure: {reason}")]
    Rpc { reason: String },
    // Mobile's variants, after the extension's so its bindings keep their order.
    #[error("wallet is locked")]
    Locked,
    #[error("no wallet on this device")]
    NoWallet,
    #[error("a wallet already exists on this device")]
    WalletExists,
    /// Too many wrong passphrases: no attempt is allowed for `retry_after_ms`.
    #[error("too many attempts; retry in {retry_after_ms} ms")]
    Throttled { retry_after_ms: u64 },
    /// Nostr coordination failed; `reason` is a fixed operation name, never input text.
    #[error("coordination failure: {reason}")]
    Coordination { reason: String },
}

impl From<starknet_wallet_core::Error> for CoreError {
    fn from(error: starknet_wallet_core::Error) -> Self {
        match error {
            starknet_wallet_core::Error::InvalidInput { reason } => Self::InvalidInput { reason },
            starknet_wallet_core::Error::Kms { operation } => Self::Kms { operation },
            starknet_wallet_core::Error::WrongPassword => Self::WrongPassword,
            starknet_wallet_core::Error::Storage { reason } => Self::Storage { reason },
            starknet_wallet_core::Error::BiometricUnavailable => Self::BiometricUnavailable,
            starknet_wallet_core::Error::BiometricInvalidated => Self::BiometricInvalidated,
            starknet_wallet_core::Error::Rpc { reason } => Self::Rpc { reason },
        }
    }
}

// wallet-core's records and enums, exported as they are. Its keypairs hold `SecretFelt`; the
// `raw-keys` records below are where a key becomes text, and only for the FFI.

#[cfg_attr(feature = "mobile", doc = "A network the mobile wallet supports.")]
#[uniffi::remote(Enum)]
pub enum NetworkId {
    Mainnet,
    Sepolia,
}

#[uniffi::remote(Record)]
pub struct StarkSignature {
    pub public_key: String,
    pub r: String,
    pub s: String,
}

#[cfg(feature = "extension")]
#[uniffi::remote(Record)]
pub struct NostrApplicationData {
    pub content: String,
    pub identifier: String,
    pub sender_public_key: String,
}

/// Stark key material, as the extension's `StarkKeypair`.
#[cfg(feature = "raw-keys")]
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct StarkKeypair {
    pub private_key: String,
    pub public_key_x: String,
    pub public_key_y: String,
}

/// X-only Stark key material from Ready's ETH-seeded scheme (`StarkXOnlyKeypair`).
#[cfg(feature = "raw-keys")]
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct StarkXOnlyKeypair {
    pub private_key: String,
    pub public_key_x: String,
}

/// The extension's spelling of a key, for the hosts that still hold raw keys (`raw-keys`).
#[cfg(feature = "raw-keys")]
fn key_text(key: &SecretFelt) -> String {
    felt::canonical(key.expose_secret())
}

/// A raw key from the host: the extension's felt grammar, else `reason`. The range is
/// wallet-core's to check.
#[cfg(feature = "raw-keys")]
fn key_from_text(text: String, reason: &str) -> Result<SecretFelt, CoreError> {
    let text = Zeroizing::new(text);
    felt::parse(&text)
        .map(SecretFelt::new)
        .ok_or_else(|| CoreError::InvalidInput {
            reason: reason.into(),
        })
}

/// Versions compiled into this core, for About screens, bug reports and the host's pin check.
#[derive(Debug, Clone, uniffi::Record)]
pub struct CoreInfo {
    pub core_version: String,
    pub krusty_version: String,
}

/// Build information.
#[uniffi::export]
pub fn core_info() -> CoreInfo {
    CoreInfo {
        core_version: env!("CARGO_PKG_VERSION").into(),
        krusty_version: starknet_wallet_core::KRUSTY_VERSION.into(),
    }
}

/// Generates a fresh phrase from OS entropy.
#[cfg(feature = "extension")]
#[uniffi::export]
pub fn generate_mnemonic(word_count: u8) -> Result<String, CoreError> {
    // The copy that crosses the boundary is the host's to wipe; this one is wiped on drop.
    Ok(starknet_wallet_core::generate_mnemonic(word_count)?.to_string())
}

/// Whether text is a checksummed BIP-39 English phrase of a supported length.
#[cfg(feature = "extension")]
#[uniffi::export]
pub fn validate_mnemonic(mnemonic: String) -> bool {
    starknet_wallet_core::validate_mnemonic(&Zeroizing::new(mnemonic))
}

/// The Starknet keypair at `m/44'/9004'/account_index'/0/address_index`.
#[cfg(feature = "raw-keys")]
#[uniffi::export]
pub fn derive_starknet_keypair(
    mnemonic: String,
    address_index: u32,
    account_index: u32,
    passphrase: Option<String>,
) -> Result<StarkKeypair, CoreError> {
    let (mnemonic, passphrase) = (Zeroizing::new(mnemonic), passphrase.map(Zeroizing::new));
    let pair = starknet_wallet_core::derive_starknet_keypair(
        &mnemonic,
        address_index,
        account_index,
        passphrase.as_deref().map(String::as_str),
    )?;
    Ok(StarkKeypair {
        private_key: key_text(&pair.private_key),
        public_key_x: pair.public_key_x,
        public_key_y: pair.public_key_y,
    })
}

/// Ready (Argent X) legacy derivation.
#[cfg(feature = "raw-keys")]
#[uniffi::export]
pub fn derive_argent_legacy_keypair(
    mnemonic: String,
    address_index: u32,
) -> Result<StarkXOnlyKeypair, CoreError> {
    let pair = starknet_wallet_core::derive_argent_legacy_keypair(
        &Zeroizing::new(mnemonic),
        address_index,
    )?;
    Ok(StarkXOnlyKeypair {
        private_key: key_text(&pair.private_key),
        public_key_x: pair.public_key_x,
    })
}

/// The counterfactual OpenZeppelin account address. An omitted salt is the public key, as
/// Bramble's `KmsPort` (Krusty's default) has it; Bramble's own OZ accounts pass `0x0`
/// explicitly (`account-contract.ts`).
#[cfg(feature = "extension")]
#[uniffi::export]
pub fn derive_oz_account_address(
    public_key_x: String,
    class_hash: String,
    salt: Option<String>,
) -> Result<String, CoreError> {
    let salt = salt.unwrap_or_else(|| public_key_x.clone());
    Ok(starknet_wallet_core::derive_oz_account_address(
        public_key_x,
        class_hash,
        salt,
    )?)
}

/// The OpenZeppelin account class Bramble deploys on `network`.
#[cfg(feature = "extension")]
#[uniffi::export]
pub fn oz_account_class_hash(network: NetworkId) -> Result<String, CoreError> {
    Ok(starknet_wallet_core::oz_account_class_hash(
        network,
        OZ_ACCOUNT_VERSION,
    )?)
}

/// Deterministic RFC-6979 Stark ECDSA over `message_hash`.
#[cfg(feature = "raw-keys")]
#[uniffi::export]
pub fn sign_stark_hash(
    private_key: String,
    message_hash: String,
) -> Result<StarkSignature, CoreError> {
    let key = key_from_text(private_key, "invalid signing input")?;
    Ok(starknet_wallet_core::sign_stark_hash(&key, &message_hash)?)
}

/// The STRK20 viewing key scoped to `"<chain_id>:<pool_address>"`.
#[cfg(feature = "raw-keys")]
#[uniffi::export]
pub fn derive_strk20_viewing_key(
    private_key: String,
    chain_id: String,
    pool_address: String,
) -> Result<String, CoreError> {
    let key = key_from_text(private_key, "invalid viewing-key input")?;
    let viewing_key =
        starknet_wallet_core::derive_strk20_viewing_key(&key, &chain_id, &pool_address)?;
    Ok(key_text(&viewing_key))
}

/// The wallet's coordination public key: BIP-340 x-only, 64 lowercase hex digits.
#[cfg(feature = "extension")]
#[uniffi::export]
pub fn derive_nostr_public_key(
    mnemonic: String,
    address_index: u32,
    account_index: u32,
    passphrase: Option<String>,
) -> Result<String, CoreError> {
    let (mnemonic, passphrase) = (Zeroizing::new(mnemonic), passphrase.map(Zeroizing::new));
    Ok(starknet_wallet_core::derive_nostr_public_key(
        &mnemonic,
        address_index,
        account_index,
        passphrase.as_deref().map(String::as_str),
    )?)
}

/// Wraps application data for one recipient at the host's clock (`created_at`, Unix seconds).
#[cfg(feature = "extension")]
#[uniffi::export]
#[allow(clippy::too_many_arguments)]
pub fn wrap_nostr_application_data(
    mnemonic: String,
    address_index: u32,
    account_index: u32,
    recipient_public_key: String,
    identifier: String,
    content: String,
    created_at: u64,
    passphrase: Option<String>,
) -> Result<String, CoreError> {
    let (mnemonic, passphrase) = (Zeroizing::new(mnemonic), passphrase.map(Zeroizing::new));
    Ok(starknet_wallet_core::wrap_nostr_application_data(
        &mnemonic,
        address_index,
        account_index,
        &recipient_public_key,
        &identifier,
        &content,
        created_at,
        passphrase.as_deref().map(String::as_str),
    )?)
}

/// Opens one gift wrap addressed to this wallet.
#[cfg(feature = "extension")]
#[uniffi::export]
pub fn open_nostr_application_data(
    mnemonic: String,
    address_index: u32,
    account_index: u32,
    event_json: String,
    passphrase: Option<String>,
) -> Result<NostrApplicationData, CoreError> {
    let (mnemonic, passphrase) = (Zeroizing::new(mnemonic), passphrase.map(Zeroizing::new));
    Ok(starknet_wallet_core::open_nostr_application_data(
        &mnemonic,
        address_index,
        account_index,
        &event_json,
        passphrase.as_deref().map(String::as_str),
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_info_names_the_compiled_krusty() {
        assert_eq!(
            core_info().krusty_version,
            starknet_wallet_core::KRUSTY_VERSION
        );
        assert_eq!(core_info().core_version, env!("CARGO_PKG_VERSION"));
    }

    #[cfg(feature = "extension")]
    #[test]
    fn an_omitted_salt_is_the_public_key() {
        let public_key = "0xb7ff7bf7404503b6df17fa28ea02dd529e58ddd0cfb3232db83ef27630f6aa";
        let class_hash = oz_account_class_hash(NetworkId::Mainnet).unwrap();
        assert_eq!(
            derive_oz_account_address(public_key.into(), class_hash.clone(), None).unwrap(),
            derive_oz_account_address(
                public_key.into(),
                class_hash.clone(),
                Some(public_key.into())
            )
            .unwrap()
        );
        assert_ne!(
            derive_oz_account_address(public_key.into(), class_hash.clone(), None).unwrap(),
            derive_oz_account_address(public_key.into(), class_hash, Some("0x0".into())).unwrap()
        );
    }
}
