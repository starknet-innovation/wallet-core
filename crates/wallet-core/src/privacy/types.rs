//! The privacy vocabulary. Every failure is a fixed variant: none carries service text, a note,
//! a cursor or key material. The one field that may hold the viewing key is the opaque
//! `PrivacyStep::Post::body`, a zeroizing buffer whose `Debug` is redacted.

use std::fmt;

use zeroize::Zeroizing;

use crate::portfolio::model::TokenBalance;
use crate::NetworkId;

/// Whether the account has a viewing key registered with the pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivacyRegistration {
    Registered,
    Unregistered,
}

/// One bundled token's private total.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivateBalance {
    pub token_address: String,
    pub symbol: String,
    pub decimals: u8,
    /// Base units, decimal text.
    pub balance: String,
    /// The extension's `formatTokenAmount` (full precision).
    pub formatted: String,
}

/// A settled private-balance reading for one account on one network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivacyState {
    pub network: NetworkId,
    /// The vault record id the reading belongs to (never an account index).
    pub account_id: String,
    pub registration: PrivacyRegistration,
    /// Every bundled token in catalog order while registered; empty otherwise.
    pub balances: Vec<PrivateBalance>,
    /// Canonical `0x` hex, `0 <= fee < 2^128`.
    pub pool_fee_fri: String,
    /// Public; for a future review's technical details.
    pub pool_address: String,
}

/// What a wallet on this core can submit. All false until an upstream privacy client exists
/// (Bramble's `mobile/docs/privacy.md` §9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrivacyCapabilities {
    pub can_register: bool,
    pub can_shield: bool,
    pub can_send_privately: bool,
    pub can_unshield: bool,
}

/// What a step is for; progress copy only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivacyPurpose {
    ChainId,
    PreflightCheck,
    IncomingState,
    PoolFee,
}

/// The next thing the host must do for a read plan.
#[derive(Clone, PartialEq, Eq)]
pub enum PrivacyStep {
    /// POST `body` to `url` with `Content-Type: application/json`; no redirects, no cache, read at
    /// most `MAX_RESPONSE_BYTES`; reply `Http` (same id) or `TransportFailure`.
    /// `body` MAY CONTAIN THE VIEWING KEY: never log, persist, display or inspect it. It is wiped
    /// when the step is dropped; a wallet that must copy it out (across FFI) makes that one copy.
    Post {
        id: u64,
        url: String,
        body: Zeroizing<Vec<u8>>,
        purpose: PrivacyPurpose,
    },
    /// JSON-RPC to the network's chain-proven endpoint (public data only).
    Rpc {
        id: u64,
        body: String,
        purpose: PrivacyPurpose,
    },
    /// Sleep, then reply `WaitElapsed`.
    Wait { delay_ms: u32 },
    /// Terminal. The plan is closed.
    Finished { state: PrivacyState },
    /// Terminal. The plan is closed.
    Failed { failure: PrivacyFailure },
}

impl fmt::Debug for PrivacyStep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Post {
                id,
                url,
                body,
                purpose,
            } => f
                .debug_struct("Post")
                .field("id", id)
                .field("url", url)
                .field("body", &format_args!("[redacted {} bytes]", body.len()))
                .field("purpose", purpose)
                .finish(),
            Self::Rpc { id, body, purpose } => f
                .debug_struct("Rpc")
                .field("id", id)
                .field("body", body)
                .field("purpose", purpose)
                .finish(),
            Self::Wait { delay_ms } => f.debug_struct("Wait").field("delay_ms", delay_ms).finish(),
            Self::Finished { state } => f.debug_struct("Finished").field("state", state).finish(),
            Self::Failed { failure } => f.debug_struct("Failed").field("failure", failure).finish(),
        }
    }
}

/// What the host reports back. Every reply carries the host clock; the core never reads one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrivacyReply {
    Start {
        now_ms: u64,
    },
    Http {
        id: u64,
        status: u16,
        body: Vec<u8>,
        now_ms: u64,
    },
    Rpc {
        id: u64,
        body: String,
        now_ms: u64,
    },
    TransportFailure {
        id: u64,
        now_ms: u64,
    },
    WaitElapsed {
        now_ms: u64,
    },
}

/// Why a read ended without a state. The UI shows each as "Unavailable" with "Try again".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivacyFailure {
    NotConfigured,
    WrongChain,
    RpcUnavailable,
    ServiceUnavailable,
    RateLimited,
    Reorg,
    BudgetExceeded,
    InvalidResponse,
    PoolReadFailed,
    Locked,
}

/// Refusals raised synchronously by the plan API.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PrivacyError {
    #[error("privacy is not configured for this network")]
    NotConfigured,
    #[error("reply out of order")]
    OutOfOrder,
    #[error("plan closed")]
    PlanClosed,
}

/// The extension's `PrivacyDisclosureState`, reduced to what the gates read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivacyReading {
    /// Not read yet (the extension's `value === null`, not loading, no error).
    Idle,
    Loading,
    /// The read failed (a service error).
    Failed,
    /// No routing for this network (the extension's `network` blocker, reported as an error).
    Unconfigured,
    Settled {
        registration: PrivacyRegistration,
    },
}

/// The Home Shield tile (`wallet-shell.ts` `shieldTile`). Every enabled variant is labelled
/// "Shield"; the variant picks the accessible name, which carries the reason the label does not
/// spell out, and the destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShieldTile {
    /// Disabled: no STRK token is tracked on this network.
    NoStrkToken,
    /// Undeployed and the loaded STRK balance is zero. Opens the privacy journey, which raises
    /// the `Fund` blocker.
    FundFirst,
    /// Undeployed (STRK funded or not yet loaded). Opens the privacy journey, which raises the
    /// `Deploy` blocker.
    DeployFirst,
    /// Deployed or deployment unknown, and not read as registered. Opens the privacy journey.
    Activate,
    /// Registered and deployed. Opens Send in Shield mode (public to private).
    Ready,
}

/// A token row's private line (`token-row.ts` `privateBreakdownLine`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrivateLine {
    Unavailable,
    /// Undeployed, funded or not: a "Shield" control that opens the privacy journey, which names
    /// the prerequisite in the way (deploy or fund).
    Shield,
    Discovering,
    /// A control that opens the privacy journey.
    RegisterToStart,
    /// `compactAmount(formatted, 2)` and the symbol.
    Amount {
        text: String,
    },
}

/// Receive's privacy-pool notice (`receive-view.ts` `privacyPoolNotice`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolNotice {
    Hidden,
    FundFirst,
    DeployFirst,
    Register,
}

/// The extension's blocker vocabulary plus `Unsupported` (this core cannot submit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivacyBlocker {
    Deploy,
    Fund,
    Ledger,
    Network,
    Pending,
    Service,
    Unsupported,
}

/// Where a privacy entry point leads (`privacy-ui.ts` `startJourney`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivacyEntry {
    Blocked {
        blocker: PrivacyBlocker,
    },
    /// Read the deployment, then ask again (a failed read is the `service` blocker).
    CheckDeployment,
    /// Show the preparing screen until the reading settles, then ask again.
    Discovering,
}

/// Token detail's private summary line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetailSummary {
    None,
    Retry,
    Amount { text: String },
    RegisterToStart,
}

/// Token detail's privacy action (`token-flows.ts` `privacyActions`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailAction {
    None,
    /// Undeployed, funded or not: one "Shield" control that opens the privacy journey, which
    /// names the prerequisite in the way (deploy or fund).
    Shield,
    /// Registered: "Shield" and "Unshield", each opening Send in its privacy mode.
    ShieldUnshield,
    /// Deployed and read as unregistered: one "Shield" (`detailShieldFirst`). The extension's
    /// first shield registers in the same transaction (789) and opens Send; this core cannot shield
    /// yet, so hosts show it disabled with the Unsupported note, as for `ShieldUnshield`.
    ShieldFirst,
    /// Deployment unknown, or a reading not yet settled: "Go private" (`detailGoPrivate`),
    /// opening the privacy journey.
    GoPrivate,
}

/// Both halves of token detail's privacy section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenDetailPrivacy {
    pub summary: DetailSummary,
    pub action: DetailAction,
}

/// The header's private-data freshness (`network-status.ts` `privacyDataIssue`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivacyStatus {
    Current,
    Refreshing,
    Unavailable,
}

/// One Home row with public and private summed (`home-tabs.ts` `unifiedHomeBalances`): the
/// portfolio's `TokenBalance` with its balance and formatted text covering both halves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnifiedBalance {
    /// The row: the portfolio's token config, balance (base units) and `formatTokenAmount` of the sum.
    pub balance: TokenBalance,
    /// The public half as the breakdown shows it, `compactAmount(public, 2)`.
    pub public_compact: String,
    /// True when the token holds only a private balance (appended by the extension).
    pub private_only: bool,
}
