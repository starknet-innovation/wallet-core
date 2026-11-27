//! Whether an address is in use on chain: the extension's `readLedgerAccountState` (the
//! deployment status and the known tokens' balances) and the verdict `ledgerStoreRefusal` draws
//! from it (`background/ledger/ledger-activity.ts`). Deployed, or any balance above zero, is
//! `Active`; known undeployed with every balance read as zero (or no tokens to read) is
//! `Unused`; anything else is `Unknown`, which a caller must never take as unused.
//!
//! Sans-IO: `requests` gives the host one `starknet_getClassHashAt` (id 0) and one `balanceOf`
//! per token (ids 1..), and `judge` reads the replies. Which tokens to read is the wallet's.

use crate::rpc::{self, Request, Response};
use crate::send::judge::{self, ClassHashAnswer};
use crate::send::rpc_bodies;
use crate::{felt, Error, Felt};

/// What the reads say about an address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    /// Deployed, or holding a known token.
    Active,
    /// Not deployed and holding none of the known tokens.
    Unused,
    /// A read failed without proving activity.
    Unknown,
}

/// The reads for `address`: its class hash (id 0), then each token's balance (id `1 + i`).
pub fn requests(address: &Felt, tokens: &[Felt]) -> Result<Vec<Request>, Error> {
    let account = felt::canonical(address);
    let mut out = vec![Request::new(0, rpc_bodies::class_hash_at(0, address))];
    for (id, token) in (1u64..).zip(tokens) {
        let body = rpc::balance_of_request(id, &felt::canonical(token), &account)?;
        out.push(Request::new(id, body));
    }
    Ok(out)
}

/// Judges the replies to `requests` for `token_count` tokens. A missing reply is a failed read.
pub fn judge(token_count: usize, responses: &[Response]) -> Activity {
    let body = |id: u64| {
        responses
            .iter()
            .find(|response| response.id == id)
            .map_or("", |response| response.body.as_str())
    };
    let deployed = judge::class_hash(judge::answer(body(0), 0));
    let balances: Vec<Option<bool>> = (1..=token_count as u64)
        .map(|id| rpc::parse_balance_of(body(id), id).ok().map(|b| b != "0"))
        .collect();
    if deployed == ClassHashAnswer::Deployed || balances.contains(&Some(true)) {
        return Activity::Active;
    }
    if deployed == ClassHashAnswer::Undeployed && !balances.contains(&None) {
        return Activity::Unused;
    }
    Activity::Unknown
}

#[cfg(test)]
#[path = "activity_tests.rs"]
mod tests;
