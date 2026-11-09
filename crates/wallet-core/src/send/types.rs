//! The plan's vocabulary. Every error and failure is a fixed variant: none carries a recipient,
//! an amount, node text or key material, so nothing a user typed or a node said can reach a log
//! through an error.

use super::calls::ReviewFlag;
use super::fees::{FeeAmount, Headroom};
use super::judge::TxStatus;
use super::sign::Deployment;
use super::u256::U256;
use crate::kms::NetworkId;
use crate::Felt;

/// Whether the account's STRK covers the fee (and the amount, when STRK is sent).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeeAffordability {
    Sufficient,
    Insufficient,
    Unknown,
}

/// Everything the review screen shows, derived from the one stored call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferReview {
    /// The verifier's summary, e.g. "Send 1.5 STRK to 0x3fa7…41a3", or "Deploy account contract".
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    /// Ready and Braavos: a read of the account's signer state at that block, or the block's
    /// own timestamp.
    AccountSigner,
}

/// The next thing the host must do for a plan.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// Show `Plan::current_review`; reply `Confirm` or `RetryEstimate`.
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepReply {
    Start,
    Rpc { id: u64, body: String, now_ms: u64 },
    TransportFailure { id: u64, now_ms: u64 },
    WaitElapsed,
    RetryEstimate,
    Confirm,
    Proceed,
    CheckAgain,
}

/// Refusals raised synchronously by `Plan::advance`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SendError {
    #[error("reply out of order")]
    OutOfOrder,
    #[error("plan closed")]
    PlanClosed,
}

/// Why a plan ended without a confirmed transaction. A wallet maps each code to its copy (the
/// extension's is `extension-transactions-copy.json`, `failures`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    /// The host failed to sign, or signed under a key that is not the account's.
    SigningFailed,
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
    /// Braavos: an undeployed account deploys only through its own reviewed deployment, never
    /// inside a send.
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendFailure {
    pub code: SendFailureCode,
    pub hash: Option<String>,
    /// The class label an `AccountClassUnsupported` refusal names (the extension's `{label}`,
    /// e.g. "argent 0.3.1"); catalog text, never node text.
    pub detail: Option<String>,
}

/// A wallet's send numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    /// Added to every estimated amount and price before signing (starknet.js's overhead).
    pub fee_margin_percent: u32,
    /// How far a submit-time fee may exceed the reviewed one.
    pub reviewed_fee_headroom: Headroom,
    /// When to read a broadcast transaction's status, as delays between reads.
    pub confirmation_delays_ms: &'static [u32],
}

/// One Argent class the wallet knows, from its catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArgentClass {
    pub class_hash: Felt,
    /// Diagnostic label, e.g. "argent 0.3.1", named by `AccountClassUnsupported`.
    pub label: String,
    /// Whether its authorization is the signer array this wallet produces.
    pub signer_array: bool,
}

/// How the account authorises a transaction (`authorizationShape` in the extension's
/// `account-families.ts`), with the catalog data its account-state reads check against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Authorization {
    /// OpenZeppelin: a bare `[r, s]`; estimates skip validation.
    StarkPair,
    /// Ready (Argent 0.4): one `Signer::Starknet` in a signer array, `[1, 0, public_key, r, s]`;
    /// estimates validate a query signature of the same shape, and the account's class and signer
    /// are read at one accepted block before anything is estimated or signed. A deployed account
    /// must run one of `classes` whose authorization is the signer array.
    ArgentSignerArray { classes: Vec<ArgentClass> },
    /// Braavos: a bare `[r, s]`, but the account's class (one of `signing_classes`) and signers
    /// are read at one accepted block first, and its deployment carries the extended
    /// authorization installing `implementation`.
    Braavos {
        signing_classes: Vec<Felt>,
        implementation: Felt,
    },
}

/// The acting account, as the wallet stored it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub network: NetworkId,
    pub address: Felt,
    pub public_key: Felt,
    pub authorization: Authorization,
    /// The DEPLOY_ACCOUNT the address commits to (`Deployment::address` must reproduce it), or
    /// `None` when the wallet does not deploy the account's class: such an account can send once
    /// deployed, but is never deployed from here.
    pub deployment: Option<Deployment>,
}

impl Account {
    pub(crate) fn is_argent(&self) -> bool {
        matches!(self.authorization, Authorization::ArgentSignerArray { .. })
    }

    pub(crate) fn is_braavos(&self) -> bool {
        matches!(self.authorization, Authorization::Braavos { .. })
    }

    /// The signature an invoke estimate carries: none for OpenZeppelin and Braavos (validation
    /// skipped), the extension's `argentQueryAccount` shape `(r, s) = (1, 1)` for Ready.
    pub(crate) fn query_signature(&self) -> Vec<Felt> {
        if self.is_argent() {
            self.signature(Felt::ONE, Felt::ONE)
        } else {
            Vec::new()
        }
    }

    /// `serializeAccountSignature` for this account's family.
    pub(crate) fn signature(&self, r: Felt, s: Felt) -> Vec<Felt> {
        if self.is_argent() {
            // One signer, `Signer::Starknet` (variant 0), its public key, then `(r, s)`.
            vec![Felt::ONE, Felt::ZERO, self.public_key, r, s]
        } else {
            vec![r, s]
        }
    }
}

/// The one ERC-20 call every review, hash and record is derived from, already validated by the
/// wallet (`calls::validate_recipient`, `parse_token_amount`, `transfer_flags`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transfer {
    pub symbol: String,
    pub token_address: Felt,
    pub is_strk: bool,
    pub recipient: Felt,
    pub amount: U256,
    /// The amount as typed (trimmed), for the review and the activity record.
    pub amount_text: String,
    pub calldata: Vec<Felt>,
    pub summary: String,
    pub flags: Vec<ReviewFlag>,
    /// STRK base units the host last read, for the shortfall warning only.
    pub fee_balance: Option<U256>,
}

/// What the plan does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Transfer(Box<Transfer>),
    /// The standalone Deploy account flow.
    Deployment,
    /// Reads one recorded transaction's status and settles its record; never signs.
    Refresh {
        hash: Felt,
    },
}
