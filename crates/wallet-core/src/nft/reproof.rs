//! The on-chain re-proof (`rpc-assets.ts getNftOwnership`, `nft-ops.ts ownedNftStandard`): an
//! indexer's or a watch list's claim is never trusted until the chain confirms the account holds
//! the token. ERC-721 is tried first (`owner_of`, then `ownerOf`); a contract that answers
//! neither is read as ERC-1155 (`balance_of`, then `balanceOf`).

use starknet_types_core::felt::Felt;

use super::read::{Ladder, ReadStep, Rung};
use super::NftStandard;
use crate::js::felt::{is_stark_field_element, uint256_felts};
use crate::js::json::JsValue;
use crate::portfolio::wire::{RpcReply, RpcRequest};
use crate::send::u256::U256;
use crate::Error;

const OWNER_RUNGS: usize = 2;

/// `NftOwnership`: the standard the contract answered as, and the account's balance of the token
/// (zero or one for ERC-721, the full quantity for ERC-1155).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NftOwnership {
    pub standard: NftStandard,
    pub balance: U256,
}

impl NftOwnership {
    /// `ownedNftStandard`: the standard to transfer under when the account holds the token, `None`
    /// when it does not ("this account does not own that NFT").
    pub fn owned(&self) -> Option<NftStandard> {
        (self.balance > U256::ZERO).then_some(self.standard)
    }
}

/// `u256` calldata: low limb, then high.
pub(super) fn u256_calldata(value: U256) -> [Felt; 2] {
    [Felt::from(value.low), Felt::from(value.high)]
}

/// One account's re-proof of one token. `ReadStep::Failed` is the extension's "Could not read NFT
/// ownership": in discovery the candidate is left out and the list marked incomplete.
#[derive(Debug, Clone)]
pub struct OwnershipRead {
    account: Felt,
    ladder: Ladder,
}

impl OwnershipRead {
    /// Refuses a zero contract or account ("Invalid NFT ownership address").
    pub fn new(contract: Felt, account: Felt, token_id: U256) -> Result<Self, Error> {
        if contract == Felt::ZERO || account == Felt::ZERO {
            return Err(Error::invalid("Invalid NFT ownership address"));
        }
        let token = u256_calldata(token_id).to_vec();
        let balance = [&[account][..], &token].concat();
        Ok(Self {
            account,
            ladder: Ladder::new(
                contract,
                vec![
                    ("owner_of", token.clone()),
                    ("ownerOf", token),
                    ("balance_of", balance.clone()),
                    ("balanceOf", balance),
                ],
            ),
        })
    }

    /// The first call, under `id`; again on each new endpoint after `ReadStep::Transport`.
    pub fn start(&mut self, id: u64) -> RpcRequest {
        self.ladder.start(id)
    }

    /// Judges the reply to the last call; a next call, if any, goes under `next_id`.
    pub fn judge(&mut self, reply: &RpcReply, next_id: u64) -> ReadStep<NftOwnership> {
        let answer = match self.ladder.rung(reply) {
            Rung::Transport => return ReadStep::Transport,
            Rung::Skip => return self.ladder.next(next_id),
            Rung::Answer(answer) => answer,
        };
        let read = if self.ladder.at() < OWNER_RUNGS {
            self.owner(&answer)
        } else {
            balance(&answer)
        };
        match read {
            Some(ownership) => ReadStep::Done(ownership),
            None => self.ladder.next(next_id),
        }
    }

    // `tryOwnerEntrypoint`: the first item, if it is a field element, is the owner.
    fn owner(&self, answer: &[JsValue]) -> Option<NftOwnership> {
        let owner = answer
            .first()
            .and_then(JsValue::as_str)
            .filter(|text| is_stark_field_element(text))
            .and_then(crate::felt::parse)?;
        let held = if owner == self.account { 1 } else { 0 };
        Some(NftOwnership {
            standard: NftStandard::Erc721,
            balance: U256::from_u128(held),
        })
    }
}

// `decodeUint256Response`: exactly two limbs, each below 2^128.
fn balance(answer: &[JsValue]) -> Option<NftOwnership> {
    if answer.len() != 2 {
        return None;
    }
    let (low, high) = uint256_felts(answer.first(), answer.get(1))?;
    Some(NftOwnership {
        standard: NftStandard::Erc1155,
        balance: U256 { high, low },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    const CONTRACT: &str = "0x049d36570d4e46f48e99674bd3fcc84644ddd6b96f7c741b1562b82f9e004dc7";
    const OWNER: &str = "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcde";
    const NOT_FOUND: &str = r#"{"code":21,"message":"entrypoint not found"}"#;

    fn felt(text: &str) -> Felt {
        crate::felt::parse(text).unwrap()
    }

    fn read(token_id: u128) -> OwnershipRead {
        OwnershipRead::new(felt(CONTRACT), felt(OWNER), U256::from_u128(token_id)).unwrap()
    }

    fn result(id: u64, result: &str) -> RpcReply {
        RpcReply {
            id,
            status: 200,
            body: Some(format!(
                r#"{{"jsonrpc":"2.0","id":{id},"result":{result}}}"#
            )),
        }
    }

    fn error(id: u64) -> RpcReply {
        RpcReply {
            id,
            status: 200,
            body: Some(format!(
                r#"{{"jsonrpc":"2.0","id":{id},"error":{NOT_FOUND}}}"#
            )),
        }
    }

    fn entrypoint(request: &RpcRequest) -> (String, Vec<Value>) {
        let body: Value = serde_json::from_str(&request.body).unwrap();
        let call = &body["params"]["request"];
        let selector = call["entry_point_selector"].as_str().unwrap().to_string();
        let calldata = call["calldata"].as_array().unwrap().clone();
        let name = ["owner_of", "ownerOf", "balance_of", "balanceOf"]
            .into_iter()
            .find(|name| crate::felt::canonical(&crate::kms::selector(name)) == selector)
            .unwrap();
        (name.to_string(), calldata)
    }

    // Drives one read through `replies` (one per call), recording the entrypoints called.
    fn drive(
        mut read: OwnershipRead,
        mut reply: impl FnMut(u64, &str) -> RpcReply,
    ) -> (ReadStep<NftOwnership>, Vec<String>) {
        let mut seen = Vec::new();
        let mut request = read.start(1);
        for id in 2.. {
            let (name, _) = entrypoint(&request);
            seen.push(name.clone());
            match read.judge(&reply(id - 1, &name), id) {
                ReadStep::Request(next) => request = next,
                done => return (done, seen),
            }
        }
        unreachable!()
    }

    #[test]
    fn the_calls_carry_the_token_and_account() {
        let mut read = read(342);
        let (name, calldata) = entrypoint(&read.start(1));
        assert_eq!(name, "owner_of");
        assert_eq!(calldata, vec![Value::from("0x156"), Value::from("0x0")]);
        let (name, calldata) = match read.judge(&error(1), 2) {
            ReadStep::Request(next) => entrypoint(&next),
            other => panic!("{other:?}"),
        };
        assert_eq!(name, "ownerOf");
        assert_eq!(calldata.len(), 2);
        let ReadStep::Request(next) = read.judge(&error(2), 3) else {
            panic!()
        };
        let (name, calldata) = entrypoint(&next);
        assert_eq!(name, "balance_of");
        assert_eq!(
            calldata,
            vec![Value::from(OWNER), "0x156".into(), "0x0".into()]
        );
    }

    #[test]
    fn falls_back_from_erc721_owner_reads_to_an_erc1155_balance() {
        let (step, seen) = drive(read(342), |id, name| {
            if name == "balance_of" {
                result(id, r#"["0x2","0x0"]"#)
            } else {
                error(id)
            }
        });
        let ownership = NftOwnership {
            standard: NftStandard::Erc1155,
            balance: U256::from_u128(2),
        };
        assert_eq!(step, ReadStep::Done(ownership));
        assert_eq!(seen, ["owner_of", "ownerOf", "balance_of"]);
        assert_eq!(ownership.owned(), Some(NftStandard::Erc1155));
    }

    #[test]
    fn a_zero_erc721_balance_is_not_misclassified() {
        let (step, seen) = drive(read(5), |id, _| result(id, r#"["0x999"]"#));
        let ReadStep::Done(ownership) = step else {
            panic!("{step:?}")
        };
        assert_eq!(ownership.standard, NftStandard::Erc721);
        assert_eq!(ownership.balance, U256::ZERO);
        assert_eq!(ownership.owned(), None);
        assert_eq!(seen, ["owner_of"]);
    }

    #[test]
    fn the_owner_is_compared_as_a_felt() {
        let padded = format!(r#"["{}"]"#, crate::js::felt::address_text(OWNER));
        let (step, _) = drive(read(5), |id, _| result(id, &padded));
        let ReadStep::Done(ownership) = step else {
            panic!()
        };
        assert_eq!(ownership.owned(), Some(NftStandard::Erc721));
    }

    #[test]
    fn a_malformed_owner_falls_through_to_the_next_entrypoint() {
        // `getNftOwner`: `[]` then a non-felt; both fall through, and so do the balances.
        let (step, seen) = drive(read(9), |id, name| match name {
            "owner_of" => result(id, "[]"),
            "ownerOf" => result(id, r#"["not-a-felt"]"#),
            "balance_of" => result(id, r#"["0x1"]"#),
            _ => result(id, r#"["0x1","0x100000000000000000000000000000000"]"#),
        });
        assert_eq!(step, ReadStep::Failed);
        assert_eq!(seen, ["owner_of", "ownerOf", "balance_of", "balanceOf"]);
    }

    #[test]
    fn a_transport_failure_abandons_the_endpoint_and_restarts() {
        let mut read = read(1);
        read.start(1);
        let rate_limited = RpcReply {
            id: 1,
            status: 429,
            body: Some("{}".into()),
        };
        assert_eq!(read.judge(&rate_limited, 2), ReadStep::Transport);
        assert_eq!(read.judge(&result(1, "null"), 2), ReadStep::Transport);
        assert_eq!(entrypoint(&read.start(7)).0, "owner_of");
        assert_eq!(read.judge(&result(1, r#"["0x1"]"#), 8), ReadStep::Transport);
    }

    #[test]
    fn refuses_a_zero_contract_or_account() {
        assert!(OwnershipRead::new(Felt::ZERO, felt(OWNER), U256::ZERO).is_err());
        assert!(OwnershipRead::new(felt(CONTRACT), Felt::ZERO, U256::ZERO).is_err());
    }
}
