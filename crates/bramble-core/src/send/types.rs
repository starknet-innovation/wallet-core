//! The send path's FFI vocabulary. The plan's own types, the deployment state and the activity
//! kind are `wallet_core`'s, exported here through `#[uniffi::remote]` with the shapes and docs
//! mobile's bindings already have; only the request and the plan-API errors are mobile's. Every
//! error and failure is a fixed variant: none carries a recipient, an amount, node text or key
//! material, so nothing a user typed or a node said can reach a log through an error.

pub use wallet_core::history::activity::ActivityKind;
pub use wallet_core::ledger::TransportFailure;
pub use wallet_core::send::call_decode::CallKind;
pub use wallet_core::send::calls::{ReviewFlag, SendAmountError};
pub use wallet_core::send::fees::{FeeAmount, FeeUnit};
pub use wallet_core::send::judge::TxStatus;
pub use wallet_core::send::types::{
    DeploymentState, FeeAffordability, RpcPurpose, SendFailure, SendFailureCode, SendStep,
    StepReply, TransferReview,
};

use crate::networks::NetworkId;

/// One ERC-20 transfer as the user entered it. Every field is untrusted and re-validated.
#[derive(Debug, Clone, uniffi::Record)]
pub struct TransferRequest {
    pub network: NetworkId,
    /// The vault record's id (`AccountRecord.id`), never an index.
    pub account_id: String,
    /// Must be one of the network's bundled tokens.
    pub token_address: String,
    /// Any spelling `validate_recipient` accepts; canonicalised in the core.
    pub recipient: String,
    /// Decimal text as typed.
    pub amount: String,
    /// STRK base units the host last read for this account, for the shortfall warning only.
    pub fee_balance: Option<String>,
    /// The person's tracked tokens on this network (`TokenPreferences::trusted_token_addresses`),
    /// for the token-contract-recipient warning only. Host-supplied and untrusted: an entry that
    /// is not a felt is ignored, and an extra one can only add a warning, never remove a refusal.
    #[uniffi(default = [])]
    pub tracked_tokens: Vec<String>,
}

/// The unit a fee is quoted in.
#[uniffi::remote(Enum)]
pub enum FeeUnit {
    Fri,
    Wei,
}

/// A fee as the spend its resource bounds authorise, with its display text.
#[uniffi::remote(Record)]
pub struct FeeAmount {
    /// Base-10 base units.
    pub max_spend: String,
    pub unit: FeeUnit,
    /// The extension's `formatFeeEstimate` text, e.g. "0.008053 STRK".
    pub display: String,
}

/// A reviewable risk the verifier raised.
#[uniffi::remote(Enum)]
pub enum ReviewFlag {
    SelfTransfer,
    /// The recipient is a token contract the wallet knows. A warning: it does not block Confirm.
    TokenContractRecipient,
    AddressPoisoning,
}

/// Whether the account's STRK covers the fee (and the amount, when STRK is sent).
#[uniffi::remote(Enum)]
pub enum FeeAffordability {
    Sufficient,
    Insufficient,
    Unknown,
}

/// What a `getClassHashAt` answer says about an account contract.
#[uniffi::remote(Enum)]
pub enum DeploymentState {
    Deployed,
    Undeployed,
    Unknown,
}

/// Everything the review screen shows, derived from the one stored call.
#[uniffi::remote(Record)]
pub struct TransferReview {
    /// The verifier's summary, e.g. "Send 1.5 STRK to 0x03fa…41a3", or "Deploy account contract".
    pub summary: String,
    pub recipient: String,
    pub recipient_short: String,
    pub amount_display: String,
    pub token_symbol: String,
    pub fee: Option<FeeAmount>,
    pub fee_estimate_failed: bool,
    pub deployment_fee: Option<FeeAmount>,
    pub deployment_fee_estimate_failed: bool,
    pub requires_deployment: bool,
    pub already_deployed: bool,
    pub affordability: FeeAffordability,
    pub flags: Vec<ReviewFlag>,
    pub can_confirm: bool,
}

/// What a JSON-RPC step is for; the host uses it only for progress copy and to refuse redirects
/// on writes.
#[uniffi::remote(Enum)]
pub enum RpcPurpose {
    ChainId,
    ClassHashAt,
    Nonce,
    EstimateFee,
    EstimateDeploymentFee,
    AddDeployAccount,
    AddInvoke,
    TransactionStatus,
    TransactionReceipt,
    /// Ready and Braavos: `starknet_blockHashAndNumber`, the block the account-state reads are
    /// pinned to.
    AcceptedBlock,
    /// Ready and Braavos: a read of the account's signer state at that block: `get_owner`,
    /// `get_owner_type`, `get_guardian`, `get_signers`, `get_deferred_remove_signers`, or the
    /// block's own timestamp.
    AccountSigner,
}

/// A transaction's chain status as the wallet reports it: the extension's `TxStatus`, the one
/// status for plans and activity rows alike.
#[uniffi::remote(Enum)]
pub enum TxStatus {
    Pending,
    Preconfirmed,
    Accepted,
    Rejected,
    Reverted,
}

/// The next thing the host must do for a plan.
#[uniffi::remote(Enum)]
pub enum SendStep {
    /// POST `body` to the plan's endpoint and reply with `StepReply::Rpc` (same `id`) or
    /// `StepReply::TransportFailure`. A write is sent exactly once and never redirected.
    Rpc {
        id: u64,
        body: String,
        purpose: RpcPurpose,
        is_write: bool,
    },
    /// Wait, then reply `WaitElapsed`.
    Wait { delay_ms: u32 },
    /// A Ledger account: send `apdu` to the account's device and reply `Device` with its answer,
    /// or `DeviceFailure` with what the transport reported.
    Device { apdu: Vec<u8> },
    /// The device refused (`reason` is the extension's rejection code: `device-locked`,
    /// `wrong-device`, ...). Nothing was signed. Reply `RetryDevice` once the person has fixed
    /// it, to sign the same transaction again, or close the plan.
    DeviceRetry { reason: String },
    /// Show `TransferPlan::current_review`; reply `Confirm` or `RetryEstimate`.
    Review,
    /// The node accepted the transaction; reply `Proceed` to confirm it on chain.
    Broadcast { hash: String },
    /// Terminal for now; `CheckAgain` re-runs the checkpoints from `Pending`.
    Finished {
        status: TxStatus,
        hash: String,
        explorer_url: Option<String>,
    },
    /// Terminal. The plan is closed.
    Failed { failure: SendFailure },
}

/// What the host reports back.
#[uniffi::remote(Enum)]
pub enum StepReply {
    Start,
    Rpc {
        id: u64,
        body: String,
        now_ms: u64,
    },
    TransportFailure {
        id: u64,
        now_ms: u64,
    },
    WaitElapsed,
    RetryEstimate,
    Confirm,
    Proceed,
    CheckAgain,
    /// The device's answer to the last `SendStep::Device`: its data, then the status word.
    Device {
        reply: Vec<u8>,
    },
    DeviceFailure {
        failure: TransportFailure,
    },
    /// After `SendStep::DeviceRetry`: sign on the device again.
    RetryDevice,
}

/// A failure the device's transport reported in place of an answer.
#[uniffi::remote(Enum)]
pub enum TransportFailure {
    /// The device is locked.
    Locked,
    /// The device disconnected.
    Disconnected,
    /// Another program holds the device.
    Busy,
    /// Anything else.
    Other,
}

/// Why an amount was refused.
#[uniffi::remote(Enum)]
pub enum SendAmountError {
    /// "Invalid transfer amount"
    Invalid,
    /// "Too many decimal places"
    TooManyDecimals,
    /// "Transfer amount must be greater than zero"
    NotPositive,
    /// At or above 2^256: no ERC-20 can carry it.
    TooLarge,
}

/// Refusals raised synchronously by the plan API.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum SendError {
    #[error("invalid recipient")]
    InvalidRecipient,
    #[error("invalid amount")]
    InvalidAmount { reason: SendAmountError },
    #[error("unknown token")]
    UnknownToken,
    #[error("wallet is locked")]
    Locked,
    #[error("unknown account")]
    UnknownAccount,
    #[error("reply out of order")]
    OutOfOrder,
    #[error("plan closed")]
    PlanClosed,
    #[error("unknown transaction")]
    UnknownTransaction,
}

impl From<wallet_core::send::types::SendError> for SendError {
    fn from(error: wallet_core::send::types::SendError) -> Self {
        match error {
            wallet_core::send::types::SendError::OutOfOrder => Self::OutOfOrder,
            wallet_core::send::types::SendError::PlanClosed => Self::PlanClosed,
        }
    }
}

/// Why a plan ended without a confirmed transaction. The host maps each code to the extension's
/// copy (`extension-transactions-*.json`, `copy`).
#[uniffi::remote(Enum)]
pub enum SendFailureCode {
    ReviewedFeeExceeded,
    ReviewedFeeRequired,
    ReviewedDeploymentFeeExceeded,
    ReviewedDeploymentFeeRequired,
    PoisonedRecipient,
    WrongChain,
    PreparationUnavailable,
    HashMismatch,
    SubmissionIndeterminate,
    DeploymentStatusUnknown,
    DeploymentUnconfirmed,
    DeploymentThenInvokeUnconfirmed,
    AutoDeploymentFailed,
    AlreadyDeployed,
    BoundsOutOfRange,
    UnrecognisedFeeUnit,
    NodeError,
    Locked,
    /// Krusty failed to sign, or signed under a key that is not the account's.
    SigningFailed,
    /// A Ledger account's device returned a signature that did not verify against the account's
    /// key (`detail`: `signature-verify-failed`). Nothing was written.
    DeviceRefused,
    /// Ready: the account runs an Argent class this wallet does not recognise.
    AccountClassUnrecognised,
    /// Ready: the account runs an Argent class whose authorization this wallet does not produce
    /// (an un-upgraded 0.3.x account); `detail` names the class.
    AccountClassUnsupported,
    /// Ready: the account is not deployed and its class is not the release this wallet deploys.
    AccountNotDeployable,
    /// Ready: the deployed account's owner is not this wallet's signer.
    AccountOwnerMismatch,
    /// Ready: the deployed account's owner is not a Starknet signer.
    AccountOwnerTypeUnsupported,
    /// Ready: the deployed account has a guardian, a second approver the phrase cannot supply.
    AccountGuardianUnsupported,
    /// Braavos: an undeployed account deploys only through its own reviewed deployment
    /// (Settings → Deploy account), never inside a send.
    BraavosDeployFirst,
    /// Braavos: the account is not deployed and its class is not the one this wallet deploys.
    BraavosNotDeployable,
    /// Braavos: the account runs a class that is not an implementation this wallet signs for.
    BraavosClassUnrecognised,
    /// Braavos: this wallet's key is not the account's first Stark signer.
    BraavosOwnerMismatch,
    /// Braavos: `get_signers` did not parse as three length-prefixed signer lists.
    BraavosSignerListMalformed,
    /// Braavos: a 2FA signer is active (its removal, if requested, has not expired).
    BraavosStrongSigner,
}

/// A failure, with the hash of anything already accepted (a deployment, or an ambiguous write).
#[uniffi::remote(Record)]
pub struct SendFailure {
    pub code: SendFailureCode,
    pub hash: Option<String>,
    /// The class label an `AccountClassUnsupported` refusal names (the extension's `{label}`,
    /// e.g. "argent 0.3.1"); public catalog text, never node text.
    #[uniffi(default = None)]
    pub detail: Option<String>,
}

/// What an activity row's transaction did: the extension's `ActivityEntry.kind`. The one kind
/// for local and on-chain rows alike (`history::activity::ActivityEntry`).
#[uniffi::remote(Enum)]
pub enum ActivityKind {
    Declare,
    Deploy,
    Invoke,
    Transfer,
}

/// What one call of a batch does (`DecodedCallKind`).
#[uniffi::remote(Enum)]
pub enum CallKind {
    /// Grants, raises or (at zero) revokes an ERC-20 allowance.
    Approve,
    NftApproval,
    OperatorApproval,
    Swap,
    Transfer,
    /// A call the wallet cannot describe.
    Unknown,
}

/// A batch review's annotation (`RiskFlagCode`); the transfer's are `ReviewFlag`. None blocks
/// Confirm: a payee that mimics a trusted address refuses the batch before a plan exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum RiskFlagCode {
    UnknownContract,
    UnlimitedApproval,
    OperatorApproval,
    SelfTransfer,
    TokenContractRecipient,
    ForeignBeneficiary,
    UndecodedSwap,
    /// Only on a queued dapp card (an outside execution); refused before anything is signed.
    AddressPoisoning,
}

/// One placeholder of a review sentence and the display text it takes.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ReviewFact {
    pub name: String,
    pub value: String,
}

/// A review sentence as the extension's catalog key and facts (`ReviewText`), for a host that
/// renders it in the person's language; `message` is the same sentence in English.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ReviewText {
    /// e.g. `review.flag.unknownContract`.
    pub key: String,
    pub facts: Vec<ReviewFact>,
}

/// One reviewable risk with the extension's sentence (`RiskFlag`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RiskFlag {
    pub code: RiskFlagCode,
    pub message: String,
    pub text: ReviewText,
}

impl From<&wallet_core::send::review_flags::RiskFlag> for RiskFlag {
    fn from(flag: &wallet_core::send::review_flags::RiskFlag) -> Self {
        use wallet_core::send::review_flags::RiskFlag as Core;
        let code = match flag {
            Core::UnknownContract { .. } => RiskFlagCode::UnknownContract,
            Core::UnlimitedApproval => RiskFlagCode::UnlimitedApproval,
            Core::OperatorApproval => RiskFlagCode::OperatorApproval,
            Core::SelfTransfer => RiskFlagCode::SelfTransfer,
            Core::TokenContractRecipient => RiskFlagCode::TokenContractRecipient,
            Core::ForeignBeneficiary { .. } => RiskFlagCode::ForeignBeneficiary,
            Core::UndecodedSwap => RiskFlagCode::UndecodedSwap,
            Core::AddressPoisoning => RiskFlagCode::AddressPoisoning,
        };
        let text = flag.text();
        Self {
            code,
            message: flag.message(),
            text: ReviewText {
                key: text.key.into(),
                facts: text
                    .facts
                    .into_iter()
                    .map(|(name, value)| ReviewFact {
                        name: name.into(),
                        value,
                    })
                    .collect(),
            },
        }
    }
}

/// One call of a batch as its review shows it (`DecodedCall`), with the flags raised on it.
/// Output only: no export takes a call.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ReviewedCall {
    pub contract_address: String,
    pub entrypoint: String,
    pub kind: CallKind,
    pub summary: String,
    pub flags: Vec<RiskFlag>,
}

impl ReviewedCall {
    /// `call` beside its review.
    pub(crate) fn new(
        call: &wallet_core::send::calls::Call,
        reviewed: &wallet_core::send::invoke::ReviewedCall,
    ) -> Self {
        Self {
            contract_address: crate::felt::canonical(&call.contract_address),
            entrypoint: call.entrypoint.clone(),
            kind: reviewed.kind,
            summary: reviewed.summary.clone(),
            flags: reviewed.flags.iter().map(RiskFlag::from).collect(),
        }
    }
}
