//! The sans-IO discovery scan. The core derives each batch's candidate addresses and hands the
//! host JSON-RPC bodies; the host POSTs them through its chain-verified client and passes the
//! bodies back; the core judges them. One `Scan` serves one (fingerprint, network), as the
//! extension's `recovery-cache.ts` is keyed; the host drops it on lock.
//!
//! Probe judging follows `isAccountDeployed` / `isContractNotFoundError`: a felt result is
//! deployed; error code 20 or a "contract not found" message is not deployed; anything else, or a
//! missing, duplicate or foreign id, pauses the scan with the batch left to be retried.
//!
//! The second round reads balances for undeployed candidates and, for deployed ones, the
//! advisory signer and privacy-pool reads (`advisory`), whose failures never pause.
//!
//! What to scan is the wallet's choice and comes in as `ScanConfig`: the shapes in order (with
//! class, layout, salt rule and whether an unused one is listed), the limits, the unusable
//! indexes, the tokens, the pool and the classes it signs for. The phrase is borrowed per call
//! and never stored.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};
use starknet_types_core::felt::Felt;

use super::advisory::{self, Gathered, Read};
use super::{family_address, Ladder};
use crate::kms::{account_public_key, NetworkId};
use crate::rpc::{self, Request, Response};
use crate::send::judge::{self, Answer};
use crate::{felt, Error};

pub use super::scan_rules::{
    keep, scan_target, Found, SaltRule, ScanConfig, ScanLimits, ScanShape, ScanStep,
};

/// Candidates one scan may accumulate, and probes one batch may issue (an input bound).
pub const MAX_CANDIDATES: usize = 512;

fn paused() -> Error {
    Error::rpc("account search paused")
}

fn no_scan() -> Error {
    Error::invalid("no account search in progress")
}

fn class_hash_request(id: u64, address: &Felt) -> String {
    rpc::envelope(
        id,
        "starknet_getClassHashAt",
        json!({ "block_id": "latest", "contract_address": felt::canonical(address) }),
    )
}

/// Judges a `starknet_getClassHashAt` reply: `Ok(Some(class))` deployed, `Ok(None)` not, and
/// a pause for anything else. Built on `send::judge::answer`; not `send::judge::class_hash`,
/// which reads any result as deployed and does not return the class.
pub fn judge_class_hash(body: &str, id: u64) -> Result<Option<Felt>, Error> {
    match judge::answer(body, id) {
        Some(Answer::Result(Value::String(text))) => {
            felt::parse(&text).map(Some).ok_or_else(paused)
        }
        Some(Answer::Error { code, text })
            if code == Some(20) || text.contains("contract not found") =>
        {
            Ok(None)
        }
        _ => Err(paused()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    ClassHash(usize),
    Balance(usize),
    Advisory(usize, Read),
    Block,
}

struct Probe {
    found: Found,
    creatable: bool,
    /// The key the account was derived from, which a signable account names as its owner.
    public_key: Felt,
    gathered: Gathered,
}

struct Batch {
    end: u32,
    manual: bool,
    probes: Vec<Probe>,
    outstanding: HashMap<u64, Pending>,
    /// The second round (balances and advisory reads) has been issued.
    detailed: bool,
    /// The third round (the latest block, only for a Braavos 2FA removal) has been considered.
    timed: bool,
    block_time: Option<u64>,
}

/// One scan's in-memory state, for one (fingerprint, network).
pub struct Scan {
    /// The phrase's account-ladder key at index 0, and the network.
    key: Option<(Felt, NetworkId)>,
    candidates: Vec<Found>,
    next_index: u32,
    batch: Option<Batch>,
    next_id: u64,
}

impl Default for Scan {
    fn default() -> Self {
        Self::new()
    }
}

impl Scan {
    pub fn new() -> Self {
        Self {
            key: None,
            candidates: Vec::new(),
            next_index: 0,
            batch: None,
            next_id: 1,
        }
    }

    fn allocate_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn step(&self, requests: Vec<Request>, limits: &ScanLimits) -> ScanStep {
        let target = scan_target(&self.candidates, limits);
        ScanStep {
            requests,
            candidates: self.candidates.clone(),
            checked: self.next_index,
            complete: self.next_index >= target,
            target,
        }
    }

    /// Starts (or restarts) the next batch: the next `batch_size` indices, or one manual index.
    /// A different phrase or network starts a new scan. A finished scan returns its last step.
    pub fn next(
        &mut self,
        phrase: &str,
        passphrase: Option<&str>,
        config: &ScanConfig,
        manual_index: Option<u32>,
    ) -> Result<ScanStep, Error> {
        let limits = config.limits;
        let per_batch = config
            .shapes
            .len()
            .saturating_mul(usize::try_from(limits.batch_size).unwrap_or(usize::MAX));
        // No shapes would be a batch with nothing to probe, which no reply could ever finish.
        if limits.batch_size == 0 || per_batch == 0 || per_batch > MAX_CANDIDATES {
            return Err(Error::invalid("invalid account search limits"));
        }
        let fingerprint = account_public_key(phrase, passphrase, Ladder::Account, 0)?;
        let key = (fingerprint, config.network);
        if self.key != Some(key) {
            // Ids are never reused, so a late reply to the scan this replaces answers nothing.
            *self = Self {
                key: Some(key),
                next_id: self.next_id,
                ..Self::new()
            };
        }
        // An abandoned batch (a failed POST, a retry) is re-issued from the same index.
        self.batch = None;
        let (start, end) = match manual_index {
            Some(index) if !config.usable(index) => {
                return Err(Error::invalid("enter a valid non-negative account index"))
            }
            Some(index) => (index, index),
            None => {
                let target = scan_target(&self.candidates, &limits);
                // An index the wallet never derives is passed over here, so a batch always has
                // something to probe; nothing above `max_index` is usable, so the walk ends there.
                while self.next_index < target && !config.usable(self.next_index) {
                    self.next_index = if self.next_index > limits.max_index {
                        target
                    } else {
                        self.next_index + 1
                    };
                }
                if self.next_index >= target {
                    return Ok(self.step(Vec::new(), &limits));
                }
                let start = self.next_index;
                let last = start.saturating_add(limits.batch_size - 1);
                (start, last.min(target - 1))
            }
        };
        if self.candidates.len() >= MAX_CANDIDATES {
            return Err(Error::invalid("too many accounts checked"));
        }
        let mut probes = Vec::new();
        // The batch starts on a usable index; one further in that is not is left out.
        for account_index in (start..=end).filter(|index| config.usable(*index)) {
            let mut keys: HashMap<Ladder, Felt> = HashMap::new();
            for shape in &config.shapes {
                let public_key = match keys.get(&shape.ladder) {
                    Some(key) => *key,
                    None => {
                        let key =
                            account_public_key(phrase, passphrase, shape.ladder, account_index)?;
                        keys.insert(shape.ladder, key);
                        key
                    }
                };
                let salt = match shape.salt {
                    SaltRule::Zero => Felt::ZERO,
                    SaltRule::PublicKey => public_key,
                };
                let address = family_address(
                    shape.family,
                    &shape.class_hash,
                    shape.argent_shape,
                    &salt,
                    &public_key,
                )?;
                probes.push(Probe {
                    found: Found {
                        family: shape.family,
                        ladder: shape.ladder,
                        class_hash: shape.class_hash,
                        account_index,
                        address,
                        deployed: false,
                        funded: false,
                        restriction: None,
                        privacy_elsewhere: false,
                    },
                    creatable: shape.creatable,
                    public_key,
                    gathered: Gathered::default(),
                });
            }
        }
        let mut outstanding = HashMap::new();
        let mut requests = Vec::new();
        for (position, probe) in probes.iter().enumerate() {
            let id = self.allocate_id();
            outstanding.insert(id, Pending::ClassHash(position));
            requests.push(Request::new(
                id,
                class_hash_request(id, &probe.found.address),
            ));
        }
        self.batch = Some(Batch {
            end,
            manual: manual_index.is_some(),
            probes,
            outstanding,
            detailed: false,
            timed: false,
            block_time: None,
        });
        Ok(self.step(requests, &limits))
    }

    /// The second round: balances for each undeployed candidate, and each deployed one's
    /// advisory reads.
    fn detail_requests(
        &mut self,
        batch: &mut Batch,
        config: &ScanConfig,
    ) -> Result<Vec<Request>, Error> {
        let pool_address = config.privacy_pool.unwrap_or(Felt::ZERO);
        let mut requests = Vec::new();
        for position in 0..batch.probes.len() {
            let probe = &batch.probes[position];
            let address = probe.found.address;
            if let Some(live_class) = probe.gathered.live_class {
                let family = probe.found.family;
                let pool = config.privacy_pool.is_some();
                for read in advisory::reads(family, &live_class, &config.authorizations, pool) {
                    let id = self.allocate_id();
                    batch
                        .outstanding
                        .insert(id, Pending::Advisory(position, read));
                    requests.push(advisory::request(id, read, &address, &pool_address));
                }
                continue;
            }
            for token in &config.tokens {
                let id = self.allocate_id();
                let account = felt::canonical(&address);
                let body = rpc::balance_of_request(id, &felt::canonical(token), &account)?;
                batch.outstanding.insert(id, Pending::Balance(position));
                requests.push(Request::new(id, body));
            }
        }
        Ok(requests)
    }

    /// Judges the host's responses. Returns the second round's requests after the class-hash
    /// round, then the latest block when a Braavos verdict needs it, or the finished batch. The
    /// phrase and config must be the ones the batch was started with.
    pub fn submit(
        &mut self,
        phrase: &str,
        passphrase: Option<&str>,
        config: &ScanConfig,
        responses: &[Response],
    ) -> Result<ScanStep, Error> {
        let fingerprint = account_public_key(phrase, passphrase, Ladder::Account, 0)?;
        if self.key != Some((fingerprint, config.network)) {
            return Err(no_scan());
        }
        let mut batch = self.batch.take().ok_or_else(no_scan)?;
        // On failure the batch is dropped, so `next` re-issues it from the same index.
        judge(&mut batch, responses)?;
        batch.outstanding.clear();
        if !batch.detailed {
            batch.detailed = true;
            let requests = self.detail_requests(&mut batch, config)?;
            if !requests.is_empty() {
                self.batch = Some(batch);
                return Ok(self.step(requests, &config.limits));
            }
        }
        // A block lists every transaction hash, so it is read only when a verdict needs its time
        // (`braavosSignerVerdict` reads it only for a requested 2FA removal), as rarely as there.
        if !batch.timed {
            batch.timed = true;
            let needed = batch.probes.iter().any(|probe| {
                advisory::needs_block(
                    probe.found.family,
                    &probe.gathered,
                    &probe.public_key,
                    &config.authorizations,
                )
            });
            if needed {
                let id = self.allocate_id();
                batch.outstanding.insert(id, Pending::Block);
                let requests = vec![advisory::block_request(id)];
                self.batch = Some(batch);
                return Ok(self.step(requests, &config.limits));
            }
        }
        for probe in batch.probes.iter_mut().filter(|probe| probe.found.deployed) {
            let (found, gathered) = (&mut probe.found, &probe.gathered);
            advisory::settle(
                found,
                gathered,
                &probe.public_key,
                batch.block_time,
                phrase,
                passphrase,
                config,
            );
        }
        for probe in batch
            .probes
            .into_iter()
            .filter(|probe| keep(&probe.found, probe.creatable, batch.manual))
        {
            match self
                .candidates
                .iter_mut()
                .find(|known| known.address == probe.found.address)
            {
                Some(known) => *known = probe.found,
                None => self.candidates.push(probe.found),
            }
        }
        if !batch.manual {
            self.next_index = batch.end + 1;
        }
        Ok(self.step(Vec::new(), &config.limits))
    }
}

// Checks that `responses` answer exactly the outstanding ids, each once.
fn matched(batch: &Batch, responses: &[Response]) -> Result<Vec<(Pending, String, u64)>, Error> {
    if responses.len() != batch.outstanding.len() {
        return Err(paused());
    }
    let mut seen = HashSet::new();
    responses
        .iter()
        .map(|response| {
            let pending = batch.outstanding.get(&response.id).ok_or_else(paused)?;
            if !seen.insert(response.id) {
                return Err(paused());
            }
            Ok((*pending, response.body.clone(), response.id))
        })
        .collect()
}

fn judge(batch: &mut Batch, responses: &[Response]) -> Result<(), Error> {
    for (pending, body, id) in matched(batch, responses)? {
        match pending {
            Pending::ClassHash(position) => {
                let probe = &mut batch.probes[position];
                probe.gathered.live_class = judge_class_hash(&body, id)?;
                probe.found.deployed = probe.gathered.live_class.is_some();
            }
            Pending::Balance(position) => {
                let balance = rpc::parse_balance_of(&body, id).map_err(|_| paused())?;
                if balance != "0" {
                    batch.probes[position].found.funded = true;
                }
            }
            Pending::Advisory(position, read) => {
                let felts = advisory::judge_call(&body, id);
                batch.probes[position].gathered.reads.insert(read, felts);
            }
            Pending::Block => batch.block_time = advisory::judge_block(&body, id),
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "scan_tests.rs"]
mod tests;
