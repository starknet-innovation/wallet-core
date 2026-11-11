// Bramble's mobile wallet (`--features mobile`), ported from Bramble's `mobile/core/src/lib.rs`
// (docs/PLAN.md, 2.7): included by `lib.rs` so its modules keep the crate-root paths
// (`crate::wallet`, `crate::kms`) and UniFFI names they had in mobile.
//
// The Rust middleware shared by the iOS and Android wallets. It is the mobile counterpart of the
// extension's background service: Krusty KMS key custody (`kms`), the encrypted vault
// (`vault`, byte-compatible with the extension envelope), the bundled network catalog
// (`networks`), and sans-IO Starknet RPC (`wallet_core::rpc`). The native apps render UI and move
// bytes; they never see a private key.
//
// Errors carry an operation name or a fixed reason, never input text, so nothing a user typed
// can reach a log through an error message.

// Mobile's code names wallet-core `wallet_core`, as its dependency rename did.
extern crate starknet_wallet_core as wallet_core;

// Bramble's catalogs and coordination policy, under the names mobile's modules use.
use catalogs::{accounts as account_catalog, multisig as multisig_constants, networks};

mod account_address;
mod account_names;
mod argent_multisig;
mod avatar;
mod class_declared;
mod container;
mod files;
mod groups;
pub(crate) use wallet_core::js;
mod kms;
pub use kms::Ladder;
mod multisig_rpc;
mod multisig_store;
mod nostr;
mod portfolio;
mod public_store;
mod records;
pub(crate) use wallet_core::rpc;
mod rpc_request;
mod ui;
mod vault;

pub use account_names::{
    account_display_name, account_label, account_preview_label, friendly_account_name,
    is_account_label,
};
pub use argent_multisig::{ArgentMultisigConfig, ArgentMultisigPrediction};
pub use avatar::{AccountAvatar, AvatarPaint, AvatarPart, AvatarShape};
pub use class_declared::ClassDeclaredRead;
pub use groups::AccountGroup;
pub use multisig_rpc::{MultisigAuthorityRead, MultisigAuthorityStatus};
pub use rpc_request::{RpcRequest, RpcResponse};
pub use ui::{PassphraseLevel, PassphraseStrength};

pub use portfolio::fiat::{
    compact_amount, fiat_value_text, format_fiat, portfolio_amount, portfolio_hero, token_row_fiat,
    token_sparkline, ChangeBadge, FiatDelta, PortfolioHero, Sparkline, SparklinePoint,
    SparklineTrend,
};
pub use portfolio::model::{DisplayCurrency, TokenBalance, TokenConfig};
pub use portfolio::prices::{
    ChartPlan, PriceBook, PriceOutcome, PricePlan, PricePoint, PriceRequest, PriceView,
};
pub use portfolio::snapshots::{
    balance_failure_copy, friendly_network_error, network_data_issue, BalanceHealth, BalancePlan,
    BalanceSnapshots, BalanceView,
};
pub use portfolio::token_meta::{
    balance_judge, balance_requests, discovery_judge, token_add_address, token_metadata_judge,
    token_metadata_requests, token_network_error, BalanceOutcome, DiscoveryOutcome,
    TokenMetaOutcome,
};
pub use portfolio::tokens::{
    token_config_id, token_glyph, token_spam_reasons, AddTokenOutcome, HomeRows, SpamReason,
    TokenGlyph, TokenPreferences, TrackedToken,
};
pub use portfolio::wire::{
    rpc_limits, HttpHeader, HttpMethod, HttpOutcome, HttpRequest, RpcLimits, RpcReply,
};
