//! A frozen Argent Multisig 0.2 INVOKE v3, mirroring the extension's
//! `packages/platform/starknet/src/rpc/multisig-invoke{,-prepared,-types}.ts` and the signature
//! rules of `accounts/account-contracts.ts`: the calls a proposal freezes, the prepared
//! transaction every co-signer signs (`PreparedInvoke`, field for field the extension's
//! `PreparedArgentMultisigInvokeV3`), its validator and maximum fee, and the threshold signature
//! a submission carries. The RPC sequence that prepares and submits one is
//! `multisig_invoke_plan`; the shared intent around it is `multisig_intent`.
//!
//! Two primitives the path needs are not in Krusty 3.1.0: `starknet_keccak` of an entrypoint
//! name (a call's selector) and Stark ECDSA verification. The caller lends them as
//! [`StarkPrimitives`]; nothing here implements either, so without them nothing validates.

use serde::{Deserialize, Serialize};
use starknet_types_core::felt::Felt;

use super::fees::{max_spend, Bound, Bounds, FeeQuote, FeeUnit};
use super::sign::invoke_hash;
use crate::accounts::multisig::{self, ArgentMultisigClass, ArgentMultisigConfig};
use crate::felt;
use crate::js::felt::canonical_felt;
use crate::kms::NetworkId;

/// `ARGENT_MULTISIG_INVOKE_LIMITS`: storage and request budgets for one frozen transaction
/// (bounds on hostile input, kept here like the Nostr bounds).
pub const MAX_CALLS: usize = 16;
pub const MAX_CALL_CALLDATA_FELTS: usize = 256;
pub const MAX_TOTAL_CALLDATA_FELTS: usize = 512;
pub const MAX_EXECUTE_CALLDATA_FELTS: usize = 561;

/// The two primitives Krusty 3.1.0 lacks, lent by the caller until it has them.
pub trait StarkPrimitives {
    /// `hash.getSelectorFromName`: `starknet_keccak(entrypoint)`.
    fn selector(&self, entrypoint: &str) -> Option<Felt>;
    /// `verifyStarkSignature`: `(r, s)` over `hash` under either point with x `public_key`.
    fn verify(&self, hash: &Felt, public_key: &Felt, r: &Felt, s: &Felt) -> bool;
}

/// Why a multisig transaction input was refused. Fixed variants: no value, key or node text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MultisigInvokeError {
    #[error("account is not the pinned Argent multisig class")]
    NotPinnedMultisig,
    #[error("invalid Argent multisig address")]
    InvalidAddress,
    #[error("invalid Argent multisig configuration")]
    InvalidConfig,
    #[error("invalid Argent multisig calls")]
    InvalidCalls,
    #[error("invalid Argent multisig calldata length")]
    CalldataLength,
    #[error("invalid prepared Argent multisig transaction")]
    InvalidPrepared,
    #[error("wrong Argent multisig signature count")]
    SignatureCount,
    #[error("invalid Argent multisig signature")]
    InvalidSignature,
    #[error("Argent multisig signature is not from a configured signer")]
    NotASigner,
    #[error("Argent multisig signatures must use distinct signers")]
    DuplicateSigner,
    #[error("Argent multisig signing intent is not ready to submit")]
    InvalidIntent,
    /// `require_safe_calls` refused; `review_calls` names the reason.
    #[error("unsafe Argent multisig calls")]
    UnsafeCalls,
}

/// `PreparedArgentMultisigCall`: one canonical Cairo 1 call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MultisigCall {
    pub calldata: Vec<String>,
    pub contract_address: String,
    pub entrypoint: String,
}

/// `PreparedResourceBound`, canonical hex.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreparedBound {
    pub max_amount: String,
    pub max_price_per_unit: String,
}

/// `PreparedResourceBounds`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreparedBounds {
    pub l1_data_gas: PreparedBound,
    pub l1_gas: PreparedBound,
    pub l2_gas: PreparedBound,
}

/// `PreparedArgentMultisigInvokeV3`, in the extension's key order. Public: `proofFacts` (a
/// STRK20 private multisig) is refused as an unknown field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreparedInvoke {
    pub account_deployment_data: Vec<String>,
    pub calldata: Vec<String>,
    pub calls: Vec<MultisigCall>,
    pub chain_id: String,
    pub fee_data_availability_mode: String,
    pub nonce: String,
    pub nonce_data_availability_mode: String,
    pub paymaster_data: Vec<String>,
    pub resource_bounds: PreparedBounds,
    pub sender_address: String,
    pub tip: String,
    pub version: String,
    pub transaction_hash: String,
}

/// `StarkSignature`: one signer's `(r, s)` over a hash, with its x-only public key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MultisigSignature {
    pub public_key: String,
    pub r: String,
    pub s: String,
}

impl From<crate::kms::StarkSignature> for MultisigSignature {
    fn from(signature: crate::kms::StarkSignature) -> Self {
        Self {
            public_key: signature.public_key,
            r: signature.r,
            s: signature.s,
        }
    }
}

/// `canonicalArgentMultisigFelt`: any spelling `BigInt` reads, if it is a field element.
pub(crate) fn input_felt(value: &str) -> Option<Felt> {
    felt::parse(&canonical_felt(value)?)
}

fn is_entrypoint(name: &str) -> bool {
    let bytes = name.as_bytes();
    (1..=64).contains(&bytes.len())
        && (bytes[0].is_ascii_lowercase() || bytes[0] == b'_')
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'_')
}

/// `normalizeArgentMultisigCalls`: 1..=16 calls, each with at most 256 calldata felts (512 in
/// all), a non-zero contract and an entrypoint name; every felt canonicalised.
pub fn normalize_calls(calls: &[MultisigCall]) -> Result<Vec<MultisigCall>, MultisigInvokeError> {
    if calls.is_empty() || calls.len() > MAX_CALLS {
        return Err(MultisigInvokeError::InvalidCalls);
    }
    let mut total = 0;
    calls
        .iter()
        .map(|call| {
            if call.calldata.len() > MAX_CALL_CALLDATA_FELTS || !is_entrypoint(&call.entrypoint) {
                return Err(MultisigInvokeError::InvalidCalls);
            }
            let calldata = call
                .calldata
                .iter()
                .map(|entry| input_felt(entry).map(|f| felt::canonical(&f)))
                .collect::<Option<Vec<_>>>()
                .ok_or(MultisigInvokeError::InvalidCalls)?;
            total += calldata.len();
            if total > MAX_TOTAL_CALLDATA_FELTS {
                return Err(MultisigInvokeError::CalldataLength);
            }
            let contract = input_felt(&call.contract_address)
                .filter(|address| *address != Felt::ZERO)
                .ok_or(MultisigInvokeError::InvalidCalls)?;
            Ok(MultisigCall {
                calldata,
                contract_address: felt::canonical(&contract),
                entrypoint: call.entrypoint.clone(),
            })
        })
        .collect()
}

/// `isArgentMultisigCalls`: already canonical and within the same budgets.
pub fn is_calls(calls: &[MultisigCall]) -> bool {
    (1..=MAX_CALLS).contains(&calls.len())
        && calls.iter().all(|call| {
            call.calldata.len() <= MAX_CALL_CALLDATA_FELTS
                && call
                    .calldata
                    .iter()
                    .all(|entry| felt::parse_canonical(entry).is_some())
                && felt::parse_canonical_non_zero(&call.contract_address).is_some()
                && is_entrypoint(&call.entrypoint)
        })
        && calls.iter().map(|call| call.calldata.len()).sum::<usize>() <= MAX_TOTAL_CALLDATA_FELTS
}

/// `transaction.getExecuteCalldata(calls, "1")`: the count, then each call's contract, selector,
/// calldata length and calldata. `None` for a felt that does not parse or a selector the
/// primitives cannot name.
pub fn execute_calldata(
    calls: &[MultisigCall],
    primitives: &dyn StarkPrimitives,
) -> Option<Vec<Felt>> {
    let mut out = vec![Felt::from(calls.len() as u64)];
    for call in calls {
        out.push(input_felt(&call.contract_address)?);
        out.push(primitives.selector(&call.entrypoint)?);
        out.push(Felt::from(call.calldata.len() as u64));
        for entry in &call.calldata {
            out.push(input_felt(entry)?);
        }
    }
    Some(out)
}

fn hex(value: u128) -> String {
    format!("{value:#x}")
}

fn prepared_bound(bound: &Bound) -> PreparedBound {
    PreparedBound {
        max_amount: hex(bound.max_amount),
        max_price_per_unit: hex(bound.max_price_per_unit),
    }
}

impl PreparedBounds {
    pub fn from_bounds(bounds: &Bounds) -> Self {
        Self {
            l1_data_gas: prepared_bound(&bounds.l1_data_gas),
            l1_gas: prepared_bound(&bounds.l1_gas),
            l2_gas: prepared_bound(&bounds.l2_gas),
        }
    }

    /// The bounds, if every value is canonical, every amount below 2^64 and every price below
    /// 2^128 (`isPreparedResourceBound`).
    pub fn to_bounds(&self) -> Option<Bounds> {
        let number = |value: &str, bits: u32| {
            felt::parse_canonical(value)?;
            let parsed = u128::from_str_radix(&value[2..], 16).ok()?;
            (bits == 128 || parsed >> bits == 0).then_some(parsed)
        };
        let bound = |b: &PreparedBound| {
            Some(Bound {
                max_amount: number(&b.max_amount, 64)?,
                max_price_per_unit: number(&b.max_price_per_unit, 128)?,
            })
        };
        Some(Bounds {
            l1_gas: bound(&self.l1_gas)?,
            l2_gas: bound(&self.l2_gas)?,
            l1_data_gas: bound(&self.l1_data_gas)?,
        })
    }
}

/// The network a chain id names; the prepared record must be for one the wallet knows.
pub(crate) fn network_of(chain_id: &Felt) -> Option<NetworkId> {
    [NetworkId::Mainnet, NetworkId::Sepolia]
        .into_iter()
        .find(|network| network.chain_id() == *chain_id)
}

impl PreparedInvoke {
    /// Freezes the fields the RPC sequence gathered and hashes them (`argentMultisigTransactionHash`).
    pub(crate) fn freeze(
        network: NetworkId,
        sender: &Felt,
        calls: Vec<MultisigCall>,
        calldata: &[Felt],
        nonce: &Felt,
        bounds: &Bounds,
    ) -> Option<Self> {
        let hash = invoke_hash(network, sender, calldata, nonce, bounds)
            .ok()
            .filter(|hash| *hash != Felt::ZERO)?;
        Some(Self {
            account_deployment_data: Vec::new(),
            calldata: calldata.iter().map(felt::canonical).collect(),
            calls,
            chain_id: felt::canonical(&network.chain_id()),
            fee_data_availability_mode: "L1".into(),
            nonce: felt::canonical(nonce),
            nonce_data_availability_mode: "L1".into(),
            paymaster_data: Vec::new(),
            resource_bounds: PreparedBounds::from_bounds(bounds),
            sender_address: felt::canonical(sender),
            tip: "0x0".into(),
            version: "0x3".into(),
            transaction_hash: felt::canonical(&hash),
        })
    }

    /// `isPreparedArgentMultisigInvokeV3`: every field canonical and in range, the calldata the
    /// calls compile to, and the hash those fields hash to. Never panics on hostile input.
    pub fn is_valid(&self, primitives: &dyn StarkPrimitives) -> bool {
        self.checked(primitives).is_some()
    }

    /// The network, sender, calldata, nonce and bounds of a valid record.
    pub(crate) fn checked(
        &self,
        primitives: &dyn StarkPrimitives,
    ) -> Option<(NetworkId, Felt, Vec<Felt>, Felt, Bounds)> {
        let fixed = self.version == "0x3"
            && self.account_deployment_data.is_empty()
            && self.paymaster_data.is_empty()
            && self.tip == "0x0"
            && self.nonce_data_availability_mode == "L1"
            && self.fee_data_availability_mode == "L1"
            && self.calldata.len() <= MAX_EXECUTE_CALLDATA_FELTS
            && is_calls(&self.calls);
        if !fixed {
            return None;
        }
        let calldata = self
            .calldata
            .iter()
            .map(|entry| felt::parse_canonical(entry))
            .collect::<Option<Vec<_>>>()?;
        let network = network_of(&felt::parse_canonical(&self.chain_id)?)?;
        let nonce = felt::parse_canonical(&self.nonce)?;
        let sender = felt::parse_canonical_non_zero(&self.sender_address)?;
        let hash = felt::parse_canonical_non_zero(&self.transaction_hash)?;
        let bounds = self.resource_bounds.to_bounds()?;
        let compiled = execute_calldata(&self.calls, primitives)?;
        let recomputed = invoke_hash(network, &sender, &calldata, &nonce, &bounds).ok()?;
        (compiled == calldata && recomputed == hash)
            .then_some((network, sender, calldata, nonce, bounds))
    }

    /// `preparedArgentMultisigFee`: the most FRI the signed bounds authorise, never an estimate.
    pub fn fee(&self, primitives: &dyn StarkPrimitives) -> Result<FeeQuote, MultisigInvokeError> {
        let (_, _, _, _, bounds) = self
            .checked(primitives)
            .ok_or(MultisigInvokeError::InvalidPrepared)?;
        let spend = max_spend(&bounds).ok_or(MultisigInvokeError::InvalidPrepared)?;
        Ok(FeeQuote {
            spend,
            unit: FeeUnit::Fri,
        })
    }
}

fn signer_felt(value: &str) -> Result<Felt, MultisigInvokeError> {
    input_felt(value)
        .filter(|parsed| *parsed != Felt::ZERO)
        .ok_or(MultisigInvokeError::InvalidSignature)
}

/// `checkedMemberSignatures`: exactly `expected` signatures by distinct members of `config`,
/// canonicalised and sorted by signer GUID, as `(public key, r, s)`.
fn member_signatures(
    signatures: &[MultisigSignature],
    config: &ArgentMultisigConfig,
    class: &ArgentMultisigClass,
    expected: usize,
) -> Result<Vec<[Felt; 3]>, MultisigInvokeError> {
    let (normalized, _) =
        multisig::normalize(config, class).map_err(|_| MultisigInvokeError::InvalidConfig)?;
    if signatures.len() != expected {
        return Err(MultisigInvokeError::SignatureCount);
    }
    let mut checked = signatures
        .iter()
        .map(|signature| {
            Ok([
                signer_felt(&signature.public_key)?,
                signer_felt(&signature.r)?,
                signer_felt(&signature.s)?,
            ])
        })
        .collect::<Result<Vec<_>, MultisigInvokeError>>()?;
    if checked.iter().any(|[key, _, _]| {
        !normalized
            .signer_public_keys
            .contains(&felt::canonical(key))
    }) {
        return Err(MultisigInvokeError::NotASigner);
    }
    let mut keyed = checked
        .drain(..)
        .map(|signature| {
            multisig::signer_guid(&signature[0])
                .map(|guid| (guid, signature))
                .map_err(|_| MultisigInvokeError::InvalidSignature)
        })
        .collect::<Result<Vec<_>, _>>()?;
    keyed.sort_by_key(|entry| entry.0);
    if keyed.windows(2).any(|pair| pair[0].1[0] == pair[1].1[0]) {
        return Err(MultisigInvokeError::DuplicateSigner);
    }
    Ok(keyed.into_iter().map(|(_, signature)| signature).collect())
}

/// `serializeArgentMultisigSignature` as felts: the count, then `Signer::Starknet` (variant 0),
/// public key, `r`, `s` per signer in GUID order. Exactly `config.threshold` signatures.
pub(crate) fn signature_felts(
    signatures: &[MultisigSignature],
    config: &ArgentMultisigConfig,
    class: &ArgentMultisigClass,
) -> Result<Vec<Felt>, MultisigInvokeError> {
    let checked = member_signatures(signatures, config, class, config.threshold as usize)?;
    let mut out = vec![Felt::from(checked.len() as u64)];
    for [key, r, s] in checked {
        out.extend([Felt::ZERO, key, r, s]);
    }
    Ok(out)
}

/// `serializeArgentMultisigSignature`, in the extension's spelling: the count and the signer
/// variant in decimal, keys and `(r, s)` as canonical hex.
pub fn serialize_signature(
    signatures: &[MultisigSignature],
    config: &ArgentMultisigConfig,
    class: &ArgentMultisigClass,
) -> Result<Vec<String>, MultisigInvokeError> {
    let felts = signature_felts(signatures, config, class)?;
    Ok(felts
        .iter()
        .enumerate()
        .map(|(index, value)| {
            if index == 0 || index % 4 == 1 {
                value.to_string()
            } else {
                felt::canonical(value)
            }
        })
        .collect())
}

/// What an estimate carries (`argentQueryAccount`): the first `threshold` signers in GUID order,
/// each with `(r, s) = (1, 1)`.
pub(crate) fn query_signature(
    config: &ArgentMultisigConfig,
    class: &ArgentMultisigClass,
) -> Result<Vec<Felt>, MultisigInvokeError> {
    let (normalized, _) =
        multisig::normalize(config, class).map_err(|_| MultisigInvokeError::InvalidConfig)?;
    let dummies: Vec<MultisigSignature> = normalized
        .signer_public_keys
        .iter()
        .take(normalized.threshold as usize)
        .map(|key| MultisigSignature {
            public_key: key.clone(),
            r: "0x1".into(),
            s: "0x1".into(),
        })
        .collect();
    signature_felts(&dummies, &normalized, class)
}

#[cfg(test)]
#[path = "multisig_invoke_tests.rs"]
mod tests;
