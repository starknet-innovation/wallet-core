//! Sending one NFT (`nft-ops.ts`'s `prepareNftTransfer`, `nft-inputs.ts`'s transfer reader,
//! `collectibles/nft.ts`'s call builders, `nft-transfer-submit.ts`'s activity row).
//!
//! The send is a one-call batch under P9's review: built here, then reviewed and frozen by
//! `send::invoke::Invoke::reviewed` (so the poisoned-payee refusal, the flags and the fee review
//! are every batch's), and handed to `send::plan::Plan` as `Kind::Invoke`. The collection is the
//! batch's one pinned contract, as the extension drops `unknown-contract` from this review: the
//! host proved on chain that the account owns the token before it asked. The row is the review's
//! line with the recipient as its payee, and names no amount or token, as the extension's does.

use super::NftStandard;
use crate::send::calls::Call;
use crate::send::invoke::{Invoke, InvokeRefusal};
use crate::send::review_flags::ReviewContext;
use crate::send::types::{Account, Policy};
use crate::send::u256::U256;
use crate::{felt, Felt, NetworkId};

/// `MAX_BASE_UNITS_DIGITS`: the longest token id text `isTokenId` accepts.
const MAX_TOKEN_ID_DIGITS: usize = 78;

/// Why an NFT send was refused before any plan exists, in the extension's words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum NftTransferRefusal {
    /// `readTransferNftInput` returned null: a zero collection or a recipient outside the
    /// non-zero lowercase-hex felt grammar.
    #[error("invalid transfer nft request")]
    InvalidRequest,
    /// The NFT is on another network than the active account.
    #[error("switch to the NFT's network first")]
    WrongNetwork,
    /// The host's ownership read found no balance of the token for this account.
    #[error("this account does not own that NFT")]
    NotOwned,
    /// `buildErc721TransferCall` / `buildErc1155TransferCall`: a zero contract, sender or
    /// recipient.
    #[error("Invalid NFT transfer address")]
    InvalidAddress,
    /// The batch review refused the call (a poisoned recipient, a call to the account itself).
    #[error(transparent)]
    Invoke(#[from] InvokeRefusal),
}

/// `isTokenId`: a base-units integer (`0`, or no leading zero), at most 78 digits, that fits a
/// u256. The extension's `cairo.uint256` throws past 2^256 - 1; the core refuses it here.
pub fn parse_token_id(text: &str) -> Option<U256> {
    (text.len() <= MAX_TOKEN_ID_DIGITS && (text == "0" || !text.starts_with('0')))
        .then(|| U256::from_decimal(text))
        .flatten()
}

/// What the person asked to send: the NFT (`NftTarget`) and the recipient as typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NftTransferRequest {
    pub network: NetworkId,
    pub contract: Felt,
    pub token_id: U256,
    /// Held to `isNonZeroStarkFieldElement` as typed: lowercase hex, never trimmed or
    /// normalised (the extension's reader refuses what `validate_recipient` would repair).
    pub recipient: String,
}

/// `buildErc721TransferCall` (`transfer_from(from, to, token_id)`) or `buildErc1155TransferCall`
/// (`safe_transfer_from(from, to, token_id, 1, [])`: one edition, empty data). Token id 0 is
/// allowed; every address must be non-zero.
pub fn transfer_call(
    standard: NftStandard,
    contract: Felt,
    from: Felt,
    to: Felt,
    token_id: U256,
) -> Result<Call, NftTransferRefusal> {
    if [contract, from, to].contains(&Felt::ZERO) {
        return Err(NftTransferRefusal::InvalidAddress);
    }
    let mut calldata = vec![
        from,
        to,
        Felt::from(token_id.low),
        Felt::from(token_id.high),
    ];
    let entrypoint = match standard {
        NftStandard::Erc721 => "transfer_from",
        NftStandard::Erc1155 => {
            calldata.extend([Felt::ONE, Felt::ZERO, Felt::ZERO]);
            "safe_transfer_from"
        }
    };
    Ok(Call {
        contract_address: contract,
        entrypoint: entrypoint.into(),
        calldata,
    })
}

/// `prepareNftTransfer`: the reviewed send of `request` from `account`, for its review and its
/// submission alike. `owned` is the host's ownership read for this account made just now
/// (`ownedNftStandard`: `owner_of`, or ERC-1155 `balance_of` above zero), never the gallery's
/// state: `None` refuses, and the standard it found picks the call. `review` is the account's
/// network as `Invoke::reviewed` takes it; its `known_contracts` are replaced by the collection.
pub fn prepare_transfer(
    account: &Account,
    request: &NftTransferRequest,
    owned: Option<NftStandard>,
    policy: &Policy,
    review: &ReviewContext,
) -> Result<Invoke, NftTransferRefusal> {
    let recipient =
        felt::parse_non_zero(&request.recipient).ok_or(NftTransferRefusal::InvalidRequest)?;
    if request.contract == Felt::ZERO {
        return Err(NftTransferRefusal::InvalidRequest);
    }
    if request.network != account.network {
        return Err(NftTransferRefusal::WrongNetwork);
    }
    let standard = owned.ok_or(NftTransferRefusal::NotOwned)?;
    let call = transfer_call(
        standard,
        request.contract,
        account.address,
        recipient,
        request.token_id,
    )?;
    let review = ReviewContext {
        known_contracts: std::slice::from_ref(&request.contract),
        ..*review
    };
    Ok(Invoke::reviewed(account, vec![call], policy, &review)?
        .with_activity_counterparty(recipient))
}

#[cfg(test)]
#[path = "transfer_tests.rs"]
mod tests;
