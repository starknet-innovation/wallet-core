//! Bramble's multisig coordination choices over `wallet-core`'s Nostr relay mechanism
//! (`starknet_wallet_core::nostr::{relay, sync, sync_window}`): the application identifier, the
//! reviewed relays and "one custom relay replaces them", the page cap, the sync window, and who a
//! coordination message goes to and is trusted from. Names follow the extension's
//! `multisig-coordination-ops.ts`, `nostr-relay.ts`, `multisig-sync-window.ts`,
//! `multisig-nostr-publish.ts` and `preferences-nostr.ts`. Plain Rust for now; the UniFFI surface
//! arrives with the swap (docs/PLAN.md, 2.7).

use starknet_wallet_core::accounts::multisig::signer_guid_hex;
use starknet_wallet_core::js::num::js_trim;
use starknet_wallet_core::nostr::relay::RelayLimits;
use starknet_wallet_core::nostr::relay_url::is_nostr_relay_url;
use starknet_wallet_core::nostr::sync_window::SyncLookbacks;

use crate::CoreError;

/// `MULTISIG_NOSTR_IDENTIFIER`: the application kind Krusty authenticates on coordination wraps.
pub const MULTISIG_NOSTR_IDENTIFIER: &str = "bramble.multisig";

/// `MULTISIG_NOSTR_RELAYS`: the reviewed defaults, used unless the person replaces them.
pub const MULTISIG_NOSTR_RELAYS: [&str; 2] = ["wss://relay.primal.net", "wss://nos.lol"];

/// `MAX_SYNC_PAGES`: pages read from one relay per sync while its answers keep coming back full.
pub const MAX_SYNC_PAGES: usize = 4;

/// `NOSTR_SYNC_LOOKBACK_SECONDS`: the first-sync lookback (30 days), also the retry cut-off.
pub const NOSTR_SYNC_LOOKBACK_SECONDS: i64 = 30 * 24 * 60 * 60;

/// `NOSTR_INCREMENTAL_LOOKBACK_SECONDS`: the overlap kept behind the last checkpoint (3 days;
/// NIP-59 backdates gift wraps by up to two).
pub const NOSTR_INCREMENTAL_LOOKBACK_SECONDS: i64 = 3 * 24 * 60 * 60;

/// Bramble's relay numbers: at most as many relays as the defaults (`validRelayUrls`), and
/// `MAX_SYNC_PAGES`.
pub const RELAY_LIMITS: RelayLimits = RelayLimits {
    max_relays: MULTISIG_NOSTR_RELAYS.len(),
    max_sync_pages: MAX_SYNC_PAGES,
};

/// Bramble's sync window.
pub const SYNC_LOOKBACKS: SyncLookbacks = SyncLookbacks {
    lookback_seconds: NOSTR_SYNC_LOOKBACK_SECONDS,
    incremental_lookback_seconds: NOSTR_INCREMENTAL_LOOKBACK_SECONDS,
};

/// `coordinationRelayUrls`, over the stored preference `nostrRelayUrl`: one custom relay replaces
/// the reviewed defaults; it never expands the broadcast set.
pub fn coordination_relay_urls(nostr_relay_url: Option<&str>) -> Vec<String> {
    match nostr_relay_url {
        Some(url) if !url.is_empty() => vec![url.to_string()],
        _ => MULTISIG_NOSTR_RELAYS.map(str::to_string).to_vec(),
    }
}

/// `withNostrRelayUrl`, returning the preference it would store: the input trimmed as JavaScript
/// trims, `None` (the defaults restored) when that is empty, else a relay `isNostrRelayUrl`
/// accepts, or "Invalid Nostr relay endpoint".
pub fn with_nostr_relay_url(relay_url: &str) -> Result<Option<String>, CoreError> {
    let trimmed = js_trim(relay_url);
    if trimmed.is_empty() {
        return Ok(None);
    }
    if !is_nostr_relay_url(trimmed) {
        return Err(CoreError::InvalidInput {
            reason: "Invalid Nostr relay endpoint".into(),
        });
    }
    Ok(Some(trimmed.to_string()))
}

/// The parts of an address-book entry (`AddressBookEntry`) the coordination rules read; `id` is
/// how a caller finds its own record again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddressBookEntry {
    pub id: String,
    pub network_id: String,
    pub nostr_public_key: Option<String>,
    pub public_key_x: Option<String>,
}

// Whether the entry's Stark key (`!!contact.publicKeyX`) has an Argent signer GUID in the set. A
// key the GUID cannot be computed for is not a signer.
fn is_signer(contact: &AddressBookEntry, signer_guids: &[String]) -> bool {
    contact
        .public_key_x
        .as_deref()
        .filter(|key| !key.is_empty())
        .and_then(|key| signer_guid_hex(key).ok())
        .is_some_and(|guid| signer_guids.contains(&guid))
}

/// `isCoordinationSigner`: the Nostr sender is a saved contact on this network whose Stark key's
/// Argent signer GUID is in the signer set. The wallet's own key is not excluded.
pub fn is_coordination_signer(
    contacts: &[AddressBookEntry],
    sender_public_key: &str,
    network_id: &str,
    signer_guids: &[String],
) -> bool {
    contacts.iter().any(|contact| {
        contact.network_id == network_id
            && contact.nostr_public_key.as_deref() == Some(sender_public_key)
            && is_signer(contact, signer_guids)
    })
}

/// `coordinationRecipients`: saved co-signer cards on this network, excluding this wallet's own
/// Nostr key, one per Nostr key. As the extension's `Map`, a repeated key keeps the position of
/// its first card and the value of its last.
pub fn coordination_recipients<'a>(
    contacts: &'a [AddressBookEntry],
    network_id: &str,
    own_public_key: &str,
    signer_guids: &[String],
) -> Vec<&'a AddressBookEntry> {
    let mut recipients: Vec<&AddressBookEntry> = Vec::new();
    for contact in contacts {
        let Some(key) = contact.nostr_public_key.as_deref() else {
            continue;
        };
        if contact.network_id != network_id
            || key == own_public_key
            || key.is_empty()
            || !is_signer(contact, signer_guids)
        {
            continue;
        }
        match recipients
            .iter_mut()
            .find(|kept| kept.nostr_public_key.as_deref() == Some(key))
        {
            Some(kept) => *kept = contact,
            None => recipients.push(contact),
        }
    }
    recipients
}
