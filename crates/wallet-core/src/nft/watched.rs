//! Watched NFTs and the gallery's switches, in the host's preferences store (never the vault):
//! `collectibles/watched-nft.ts` (`isWatchedNft`, `withNftId`), the preferences store's
//! `addNft`, `removeNft`, `setNftSpam`, `setNftIndexerUrl` and `nftsShown`, `nft-candidates.ts`'s
//! `watchedCandidates`, `verificationContext`'s `nftContracts`, and the pure half of
//! `nft-ops.ts`'s `watchOwnedNft` and `applyWatchNftApproval` (the ownership read is the host's).
//!
//! The document under the host's key holds the extension's own fields of `bramble:preferences`:
//! `nfts` (each record exactly as the extension writes it), `nftIndexerUrl` and
//! `collectiblesPreview`. Any other key is ignored on read and not written back.

use serde_json::{json, Map, Value};
use starknet_types_core::felt::Felt;

use crate::felt;
use crate::host::Storage;
use crate::js::num::{js_trim, utf16_len};
use crate::nostr::relay_url::is_custom_rpc_url;
use crate::send::u256::U256;
use crate::{Error, NetworkId};

const KEYS: [&str; 7] = [
    "contract",
    "id",
    "label",
    "networkId",
    "owner",
    "spam",
    "tokenId",
];

/// A collectible the person watches (`WatchedNft`), spelled as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchedNft {
    /// `nftId`: `<networkId>:<contract>:<tokenId>`.
    pub id: String,
    pub contract: String,
    pub network: NetworkId,
    /// Base-10, no leading zeros.
    pub token_id: String,
    pub label: Option<String>,
    /// The account it was watched for; absent only on records written before account scoping.
    pub owner: Option<String>,
    pub spam: Option<bool>,
}

/// The NFT a watch names: the watch form's fields, or a site card's (no label).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NftTarget {
    pub contract: Felt,
    pub network: NetworkId,
    pub token_id: U256,
    pub label: Option<String>,
}

fn nft_id(network: NetworkId, contract: &str, token_id: &str) -> String {
    format!("{}:{contract}:{token_id}", network.catalog_id())
}

// `isBaseUnitsAmount` with at most 78 digits.
fn is_token_id(value: &str) -> bool {
    value == "0"
        || (!value.is_empty()
            && value.len() <= 78
            && !value.starts_with('0')
            && value.bytes().all(|b| b.is_ascii_digit()))
}

fn is_label(value: &str) -> bool {
    (1..=64).contains(&utf16_len(js_trim(value)))
}

// An optional key: absent, or present and accepted by `read` (an explicit `null` is not absent).
fn optional<T>(value: &Value, key: &str, read: impl Fn(&Value) -> Option<T>) -> Option<Option<T>> {
    match value.get(key) {
        None => Some(None),
        Some(v) => read(v).map(Some),
    }
}

impl WatchedNft {
    /// `isWatchedNft`.
    fn from_json(value: &Value) -> Option<Self> {
        let object = value.as_object()?;
        if !object.keys().all(|k| KEYS.contains(&k.as_str())) {
            return None;
        }
        let text = |key: &str| value.get(key)?.as_str().map(str::to_string);
        let felt_text = |v: &Value| {
            v.as_str()
                .filter(|s| felt::parse_non_zero(s).is_some())
                .map(str::to_string)
        };
        let nft = Self {
            id: text("id")?,
            contract: value.get("contract").and_then(felt_text)?,
            network: NetworkId::from_catalog_id(value.get("networkId")?.as_str()?)?,
            token_id: text("tokenId").filter(|t| is_token_id(t))?,
            label: optional(value, "label", |v| {
                v.as_str().filter(|s| is_label(s)).map(str::to_string)
            })?,
            owner: optional(value, "owner", felt_text)?,
            spam: optional(value, "spam", Value::as_bool)?,
        };
        (nft.id == nft_id(nft.network, &nft.contract, &nft.token_id)).then_some(nft)
    }

    fn to_json(&self) -> Value {
        let mut out = json!({ "contract": self.contract, "id": self.id,
                              "networkId": self.network.catalog_id(), "tokenId": self.token_id });
        if let Some(label) = &self.label {
            out["label"] = json!(label);
        }
        if let Some(owner) = &self.owner {
            out["owner"] = json!(owner);
        }
        if let Some(spam) = self.spam {
            out["spam"] = json!(spam);
        }
        out
    }

    /// `withNftId`, then `isWatchedNft` over the result (the label's bounds).
    fn new(target: &NftTarget, owner: &Felt) -> Option<Self> {
        let contract = felt::canonical(&target.contract);
        let token_id = target.token_id.to_decimal();
        let nft = Self {
            id: nft_id(target.network, &contract, &token_id),
            contract,
            network: target.network,
            token_id,
            label: target.label.as_deref().map(|l| js_trim(l).to_string()),
            owner: Some(felt::canonical(owner)),
            spam: None,
        };
        Self::from_json(&nft.to_json())
    }
}

#[derive(Debug, Default)]
struct Doc {
    nfts: Vec<WatchedNft>,
    indexer_url: Option<String>,
    shown: Option<bool>,
}

impl Doc {
    fn parse(bytes: &[u8]) -> Option<Self> {
        let value: Value = serde_json::from_slice(bytes).ok()?;
        value.as_object()?;
        let nfts = match value.get("nfts") {
            None => Vec::new(),
            Some(list) => list
                .as_array()?
                .iter()
                .map(WatchedNft::from_json)
                .collect::<Option<Vec<_>>>()?,
        };
        let unique = nfts
            .iter()
            .enumerate()
            .all(|(i, nft)| nfts[..i].iter().all(|other| other.id != nft.id));
        // An endpoint the grammar now refuses is dropped, as `normalizeStoredServiceUrl` drops
        // one it cannot respell; anything but a string fails the document.
        let indexer_url = optional(&value, "nftIndexerUrl", |v| v.as_str().map(str::to_string))?
            .filter(|url| is_custom_rpc_url(url));
        let shown = optional(&value, "collectiblesPreview", Value::as_bool)?;
        unique.then_some(Self {
            nfts,
            indexer_url,
            shown,
        })
    }

    fn to_bytes(&self) -> Vec<u8> {
        let mut out = Map::new();
        out.insert(
            "nfts".into(),
            Value::Array(self.nfts.iter().map(WatchedNft::to_json).collect()),
        );
        if let Some(url) = &self.indexer_url {
            out.insert("nftIndexerUrl".into(), json!(url));
        }
        if let Some(shown) = self.shown {
            out.insert("collectiblesPreview".into(), json!(shown));
        }
        Value::Object(out).to_string().into_bytes()
    }
}

// `readRemoveNftInput` / `readSetNftSpamInput`'s id.
fn require_id(id: &str) -> Result<(), Error> {
    if (1..=200).contains(&utf16_len(id)) {
        Ok(())
    } else {
        Err(Error::invalid("Invalid NFT"))
    }
}

/// The watched NFTs and the gallery's switches under one key in the host's storage. Every call
/// reads the stored document; a document that fails the extension's rules fails every call and
/// is never overwritten (the extension's "Invalid stored preferences"). A wallet that shares one
/// across threads wraps it in its own lock.
pub struct WatchedNfts<'a> {
    storage: &'a dyn Storage,
    key: &'a str,
}

impl<'a> WatchedNfts<'a> {
    /// The document under `key` in `storage`.
    pub fn new(storage: &'a dyn Storage, key: &'a str) -> Self {
        Self { storage, key }
    }

    fn load(&self) -> Result<Doc, Error> {
        match self.storage.read(self.key)? {
            None => Ok(Doc::default()),
            Some(bytes) => {
                Doc::parse(&bytes).ok_or_else(|| Error::storage("invalid stored NFT preferences"))
            }
        }
    }

    fn update(&self, change: impl FnOnce(&mut Doc)) -> Result<(), Error> {
        let mut doc = self.load()?;
        change(&mut doc);
        self.storage.write(self.key, &doc.to_bytes())
    }

    /// Every watched NFT, in stored order.
    pub fn list(&self) -> Result<Vec<WatchedNft>, Error> {
        Ok(self.load()?.nfts)
    }

    /// `watchedCandidates`: `owner`'s watched NFTs on `network` that are not hidden as spam, with
    /// records from before account scoping (no owner) shown for every account.
    pub fn gallery(&self, network: NetworkId, owner: &Felt) -> Result<Vec<WatchedNft>, Error> {
        let mut nfts = self.list()?;
        nfts.retain(|nft| {
            nft.network == network
                && nft.spam != Some(true)
                && nft
                    .owner
                    .as_deref()
                    .is_none_or(|stored| felt::parse(stored) == Some(*owner))
        });
        Ok(nfts)
    }

    /// `verificationContext`'s `nftContracts`: every watched contract on `network`, whichever
    /// account watched it and spam-hidden or not, as `call_decode::KnownContracts::nft_contracts`
    /// takes them.
    pub fn collections(&self, network: NetworkId) -> Result<Vec<String>, Error> {
        Ok(self
            .list()?
            .into_iter()
            .filter(|nft| nft.network == network)
            .map(|nft| nft.contract)
            .collect())
    }

    /// `watchOwnedNft` once the host has re-proved, on `target`'s network, that `account` owns it
    /// (`owned`): refused unless the target is on the active network and owned; then
    /// `addNft` for `account`, replacing any record of the same id (and so its owner, label and
    /// spam flag). The host clears its NFT snapshot cache for the account after.
    pub fn watch_owned(
        &self,
        target: &NftTarget,
        active_network: NetworkId,
        account: &Felt,
        owned: bool,
    ) -> Result<WatchedNft, Error> {
        if target.network != active_network {
            return Err(Error::invalid("switch to the NFT's network first"));
        }
        if !owned {
            return Err(Error::invalid("this account does not own that NFT"));
        }
        let nft = WatchedNft::new(target, account).ok_or_else(|| Error::invalid("Invalid NFT"))?;
        self.update(|doc| {
            doc.nfts.retain(|stored| stored.id != nft.id);
            doc.nfts.push(nft.clone());
        })?;
        Ok(nft)
    }

    /// `applyWatchNftApproval`: a site's approved card, refused while NFTs are turned off, then
    /// [`Self::watch_owned`]. Stored like the person's own watch, as the extension stores it.
    pub fn watch_from_site(
        &self,
        target: &NftTarget,
        active_network: NetworkId,
        account: &Felt,
        owned: bool,
    ) -> Result<WatchedNft, Error> {
        if !self.shown()? {
            return Err(Error::invalid("nfts are turned off"));
        }
        self.watch_owned(target, active_network, account, owned)
    }

    /// `removeNft`: an unknown id changes nothing.
    pub fn remove(&self, id: &str) -> Result<(), Error> {
        require_id(id)?;
        self.update(|doc| doc.nfts.retain(|stored| stored.id != id))
    }

    /// `setNftSpam`: hide as spam, or unhide; an unknown id changes nothing.
    pub fn set_spam(&self, id: &str, spam: bool) -> Result<(), Error> {
        require_id(id)?;
        self.update(|doc| {
            for stored in doc.nfts.iter_mut().filter(|stored| stored.id == id) {
                stored.spam = Some(spam);
            }
        })
    }

    /// The person's indexer endpoint (`nftIndexerUrl`), if set.
    pub fn indexer_url(&self) -> Result<Option<String>, Error> {
        Ok(self.load()?.indexer_url)
    }

    /// `setNftIndexerUrl`: trimmed; empty clears it, anything else must pass `isCustomRpcUrl`.
    pub fn set_indexer_url(&self, url: &str) -> Result<(), Error> {
        let trimmed = js_trim(url);
        if !trimmed.is_empty() && !is_custom_rpc_url(trimmed) {
            return Err(Error::invalid("Invalid indexer endpoint"));
        }
        self.update(|doc| doc.indexer_url = Some(trimmed.to_string()).filter(|u| !u.is_empty()))
    }

    /// `nftsShown`: on unless the person turned NFTs off (`collectiblesPreview: false`).
    pub fn shown(&self) -> Result<bool, Error> {
        Ok(self.load()?.shown != Some(false))
    }

    /// The display switch (`setDisplay({ collectiblesPreview })`).
    pub fn set_shown(&self, shown: bool) -> Result<(), Error> {
        self.update(|doc| doc.shown = Some(shown))
    }
}

#[cfg(test)]
#[path = "watched_tests.rs"]
mod tests;
