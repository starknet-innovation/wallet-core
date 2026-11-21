//! The revoke (`allowance-revoke.ts`'s `allowanceTarget` and the background's
//! `handleRevokeAllowance`, `isRevokableErc20` and `revokeOutcomes` in `allowance-ops.ts`): which
//! allowance an id names for the active account, the checks before anything is sent, the
//! `approve(spender, 0)` call as a reviewed `send::invoke::Invoke`, and the outcomes that settle a
//! submitted revoke. Nothing here signs or submits: the plan does, and the wallet files the batch
//! with `calls::record_submitted` once it has a hash, which marks a tracked grant `revoking` under
//! that hash (`store`). The next listing settles it (`AllowanceLog::settle_revocations`) on a zero
//! reading or on the outcome `revoke_outcomes` reads from the wallet's own activity rows.
//!
//! The revoke is ERC-20 only. `approve(spender, 0)` on an ERC-721 approves item #0 rather than
//! revoking anything, so it is built only for a contract the wallet can call fungible: one this
//! account trusts as a token, or one the chain answered `allowance(owner, spender)` for.

use std::collections::HashSet;

use starknet_types_core::felt::Felt;

use super::calls::{approved_asset_kind, AllowanceOwner, AssetKind};
use super::discovery::AllowanceReading;
use super::record::AllowanceRecord;
use super::store::{RevokeOutcome, RevokeStatus};
use super::tracked::TrackedAllowancePair;
use crate::felt;
use crate::history::activity::{ActivityEntry, ActivityOrigin};
use crate::js::felt::canonical_felt;
use crate::js::num::utf16_len;
use crate::send::call_decode::Known;
use crate::send::calls::Call;
use crate::send::invoke::{Invoke, InvokeRefusal};
use crate::send::judge::TxStatus;
use crate::send::multisig_invoke::StarkPrimitives;
use crate::send::types::{Account, Policy};
use crate::NetworkId;

/// `readRevokeAllowanceInput`'s `MAX_ID_LENGTH`: a bound of the id grammar (a full-length id is
/// about 208 characters), checked before any work on the id.
const MAX_ID_LENGTH: usize = 240;

/// Why a revoke was refused. `Display` is the extension's sentence; none carries an id, an
/// address or an amount.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RevokeRefusal {
    #[error("invalid revoke request")]
    InvalidRequest,
    /// The id names a record another scope owns (another account, or a multisig this account
    /// signs for, whose grants need a reviewed multisig intent).
    #[error("switch to the account that granted this allowance")]
    OtherAccount,
    #[error("allowance not found")]
    NotFound,
    #[error("this contract is an NFT collection, not a token; revoke it from the dapp")]
    Collection,
    #[error(
        "this wallet could not confirm that contract is a token, so it will not send a revoke \
         that could approve an NFT instead; revoke it from the dapp"
    )]
    UnconfirmedToken,
    /// The batch review refused the call.
    #[error("{0}")]
    Invoke(#[from] InvokeRefusal),
}

/// `allowanceTarget`: the token and spender an id names, only when it is exactly
/// `<account_id>:<token>:<spender>` with both non-zero felts, so another account's id or a
/// malformed one names nothing. Lets a revoke reach an allowance found on chain that the log
/// never held.
pub fn allowance_target(id: &str, account_id: &str) -> Option<(Felt, Felt)> {
    let rest = id.strip_prefix(account_id)?.strip_prefix(':')?;
    // A third part leaves a `:` in the spender, which no felt spelling admits.
    let (token, spender) = rest.split_once(':')?;
    let felt = |part: &str| felt::parse_non_zero(&canonical_felt(part)?);
    Some((felt(token)?, felt(spender)?))
}

/// One allowance the active account may revoke, bound to that account and its network. Built
/// only by `resolve`; `invoke` holds the account it is given to the same binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevokeTarget {
    owner: AllowanceOwner,
    address: Felt,
    id: String,
    token: Felt,
    spender: Felt,
}

impl RevokeTarget {
    /// The checks `handleRevokeAllowance` makes before reading anything: the id's grammar, a
    /// stored record with that id owned by `account` on its network, or else an id naming
    /// `account`'s own pair, and a contract that is not a watched collection. `stored` is the
    /// whole log; `known` is what the account trusts on its network.
    pub fn resolve(
        id: &str,
        account: &Account,
        stored: &[AllowanceRecord],
        known: &Known,
    ) -> Result<Self, RevokeRefusal> {
        if id.is_empty() || utf16_len(id) > MAX_ID_LENGTH {
            return Err(RevokeRefusal::InvalidRequest);
        }
        let owner =
            AllowanceOwner::of(account.network, &account.address).ok_or(RevokeRefusal::NotFound)?;
        let (token, spender) = match stored.iter().find(|record| record.id == id) {
            Some(record) => {
                if record.account_id != owner.account_id || record.network != owner.network {
                    return Err(RevokeRefusal::OtherAccount);
                }
                let token = felt::parse_non_zero(&record.token).ok_or(RevokeRefusal::NotFound)?;
                let spender =
                    felt::parse_non_zero(&record.spender).ok_or(RevokeRefusal::NotFound)?;
                (token, spender)
            }
            None => allowance_target(id, &owner.account_id).ok_or(RevokeRefusal::NotFound)?,
        };
        let target = Self {
            owner,
            address: account.address,
            id: id.to_string(),
            token,
            spender,
        };
        target.refuse_collection(known)?;
        Ok(target)
    }

    fn refuse_collection(&self, known: &Known) -> Result<(), RevokeRefusal> {
        match approved_asset_kind(known, &self.token) {
            AssetKind::Collection => Err(RevokeRefusal::Collection),
            _ => Ok(()),
        }
    }

    /// The id the revoke was asked for.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The read that confirms the contract is fungible (`isRevokableErc20`), for a
    /// `tracked::TrackedAllowanceRead` over this one pair; `None` when `known` already trusts the
    /// contract as a token and no read is needed.
    pub fn confirmation_pair(&self, known: &Known) -> Option<TrackedAllowancePair> {
        (approved_asset_kind(known, &self.token) != AssetKind::Fungible).then(|| {
            TrackedAllowancePair {
                id: self.id.clone(),
                owner: felt::canonical(&self.address),
                token: felt::canonical(&self.token),
                spender: felt::canonical(&self.spender),
            }
        })
    }

    /// `buildErc20ApproveCall(token, spender, 0)`: `approve(spender, 0)`, the uint256 as two
    /// zero limbs.
    pub fn call(&self) -> Call {
        Call {
            contract_address: self.token,
            entrypoint: "approve".into(),
            calldata: vec![self.spender, Felt::ZERO, Felt::ZERO],
        }
    }

    /// The revoke as a reviewed batch for the plan, from the same `account` it was resolved for.
    /// The contract must be one `known` trusts as a token, or one `readings` (the confirmation
    /// read's) answered for this pair, whatever amount it reported; a watched collection is
    /// refused again here. `policy`, `trusted` and `primitives` are `Invoke::new`'s.
    pub fn invoke(
        &self,
        account: &Account,
        known: &Known,
        readings: &[AllowanceReading],
        policy: &Policy,
        trusted: &[Felt],
        primitives: &dyn StarkPrimitives,
    ) -> Result<Invoke, RevokeRefusal> {
        if AllowanceOwner::of(account.network, &account.address).as_ref() != Some(&self.owner)
            || account.address != self.address
        {
            return Err(RevokeRefusal::OtherAccount);
        }
        self.refuse_collection(known)?;
        let fungible = approved_asset_kind(known, &self.token) == AssetKind::Fungible;
        if !fungible && !readings.iter().any(|reading| reading.id == self.id) {
            return Err(RevokeRefusal::UnconfirmedToken);
        }
        Ok(Invoke::new(
            account,
            vec![self.call()],
            policy,
            known,
            trusted,
            primitives,
        )?)
    }
}

/// `revokeOutcomes`: how each pending revoke in `records` settled, from the wallet's own activity
/// rows on `network`. Only a row this wallet recorded for the transaction it submitted, in the
/// revoking record's own scope, counts; a row still pending or preconfirmed is no outcome yet.
/// Conflicting rows are resolved by `settle_revocations`, failed over accepted.
pub fn revoke_outcomes(
    records: &[AllowanceRecord],
    network: NetworkId,
    entries: &[ActivityEntry],
) -> Vec<RevokeOutcome> {
    let pending: HashSet<(&str, &str)> = records
        .iter()
        .filter_map(|r| Some((r.account_id.as_str(), r.revoke_hash.as_deref()?)))
        .collect();
    if pending.is_empty() {
        return Vec::new();
    }
    entries
        .iter()
        .filter(|e| e.origin == Some(ActivityOrigin::Wallet) && e.network == network)
        .filter_map(|e| {
            let hash = canonical_felt(&e.hash)?;
            if !pending.contains(&(e.account_id.as_str(), hash.as_str())) {
                return None;
            }
            let status = match e.status {
                TxStatus::Accepted => RevokeStatus::Accepted,
                TxStatus::Rejected | TxStatus::Reverted => RevokeStatus::Failed,
                TxStatus::Pending | TxStatus::Preconfirmed => return None,
            };
            Some(RevokeOutcome { hash, status })
        })
        .collect()
}
