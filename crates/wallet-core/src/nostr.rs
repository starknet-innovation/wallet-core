//! Nostr coordination identity and NIP-59 application data. The event grammar and bounds are
//! Bramble's `wallet-model/nostr` ones, kept as generic resource bounds, and public so a wallet
//! checks relay traffic with the same grammar the core does. The private key is derived per call
//! into a `Zeroizing` buffer, only borrowed by Krusty, and never returned.

use serde::Serialize;
use serde_json::Value;
use zeroize::Zeroizing;

use crate::kms::{check_derivation_input, utf16_length};
use crate::Error;

pub mod relay_url;

/// NIP-59 gift-wrap kind.
pub const GIFT_WRAP_KIND: u64 = 1_059;
/// `MAX_NOSTR_EVENT_JSON_BYTES` in `wallet-model/nostr`.
pub const MAX_EVENT_JSON_BYTES: usize = 131_070;
/// `MAX_NOSTR_APPLICATION_DATA_BYTES` in `wallet-model/nostr`.
pub const MAX_APPLICATION_DATA_BYTES: usize = 32_768;
/// `MAX_NOSTR_BATCH_ITEMS` in `wallet-model/nostr`: the most wraps one publish or sync handles.
pub const MAX_BATCH_ITEMS: usize = 50;
/// JavaScript's `Number.MAX_SAFE_INTEGER`.
pub(crate) const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;

/// A bounded signed event that passed `isNostrEvent`; authenticity is still unknown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NostrEvent {
    pub content: String,
    pub created_at: u64,
    pub id: String,
    pub kind: u64,
    pub pubkey: String,
    pub sig: String,
    pub tags: Vec<Vec<String>>,
}

/// Authenticated application data opened from a gift wrap addressed to this wallet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NostrApplicationData {
    pub content: String,
    pub identifier: String,
    pub sender_public_key: String,
}

fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// `isNostrPublicKey`: 64 lowercase hex digits (also `isNostrEventId`).
pub fn is_public_key(value: &str) -> bool {
    is_lower_hex(value, 64)
}

/// The adapter's application identifier grammar: 1–128 printable ASCII characters.
pub fn is_identifier(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && value.bytes().all(|b| (b' '..=b'~').contains(&b))
}

/// `isNostrApplicationData`: 1..=32,768 UTF-8 bytes (UTF-8 is never shorter than UTF-16).
pub fn is_application_data(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_APPLICATION_DATA_BYTES
}

/// `Number.isSafeInteger(value) && value >= 0`, including JSON spellings such as `1.0` or `1e3`.
pub fn safe_non_negative_integer(value: &Value) -> Option<u64> {
    if let Some(n) = value.as_u64() {
        return (n <= MAX_SAFE_INTEGER).then_some(n);
    }
    if value.as_i64().is_some() {
        return None; // negative
    }
    let float = value.as_f64()?;
    let valid = float.is_finite()
        && float.fract() == 0.0
        && (0.0..=MAX_SAFE_INTEGER as f64).contains(&float);
    // The range check above makes the cast exact.
    valid.then_some(float as u64)
}

fn tag(value: &Value) -> Option<Vec<String>> {
    let parts = value.as_array()?;
    if parts.len() > 8 {
        return None;
    }
    parts
        .iter()
        .map(|part| {
            part.as_str()
                .filter(|text| utf16_length(text) <= 512)
                .map(str::to_string)
        })
        .collect()
}

/// `isNostrEvent`: exactly the seven NIP-01 keys, each bounded as the extension bounds it.
pub fn event_from_value(value: &Value) -> Option<NostrEvent> {
    let object = value.as_object()?;
    const KEYS: [&str; 7] = [
        "content",
        "created_at",
        "id",
        "kind",
        "pubkey",
        "sig",
        "tags",
    ];
    if !object.keys().all(|key| KEYS.contains(&key.as_str())) {
        return None;
    }
    let content = object.get("content")?.as_str()?;
    if content.is_empty() || content.len() > MAX_EVENT_JSON_BYTES {
        return None;
    }
    let created_at = safe_non_negative_integer(object.get("created_at")?)?;
    let id = object.get("id")?.as_str().filter(|id| is_public_key(id))?;
    let kind = safe_non_negative_integer(object.get("kind")?).filter(|kind| *kind <= 65_535)?;
    let pubkey = object
        .get("pubkey")?
        .as_str()
        .filter(|key| is_public_key(key))?;
    let sig = object
        .get("sig")?
        .as_str()
        .filter(|sig| is_lower_hex(sig, 128))?;
    let tags = object.get("tags")?.as_array()?;
    if tags.len() > 32 {
        return None;
    }
    Some(NostrEvent {
        content: content.to_string(),
        created_at,
        id: id.to_string(),
        kind,
        pubkey: pubkey.to_string(),
        sig: sig.to_string(),
        tags: tags.iter().map(tag).collect::<Option<_>>()?,
    })
}

/// `parseNostrEventJson`: a bounded JSON event, or `None`.
pub fn parse_event_json(text: &str) -> Option<NostrEvent> {
    if text.is_empty() || text.len() > MAX_EVENT_JSON_BYTES {
        return None;
    }
    event_from_value(&serde_json::from_str(text).ok()?)
}

/// `hasNostrRecipientTag`: some tag is `["p", recipient, …]`.
pub fn has_recipient_tag(event: &NostrEvent, recipient: &str) -> bool {
    event
        .tags
        .iter()
        .any(|tag| tag.len() >= 2 && tag[0] == "p" && tag[1] == recipient)
}

/// The coordination private key at `m/44'/1237'/account_index'/0/address_index`.
fn private_key(
    mnemonic: &str,
    address_index: u32,
    account_index: u32,
    passphrase: Option<&str>,
) -> Result<Zeroizing<[u8; 32]>, Error> {
    check_derivation_input(mnemonic, passphrase)?;
    krusty_kms::derive_nostr_private_key(mnemonic, address_index, account_index, passphrase)
        .map(Zeroizing::new)
        .map_err(|_| Error::kms("nostr key"))
}

/// The wallet's coordination public key: BIP-340 x-only, 64 lowercase hex digits.
pub fn derive_nostr_public_key(
    mnemonic: &str,
    address_index: u32,
    account_index: u32,
    passphrase: Option<&str>,
) -> Result<String, Error> {
    let key = private_key(mnemonic, address_index, account_index, passphrase)?;
    let public = krusty_kms::nostr_public_key(&key).map_err(|_| Error::kms("nostr public key"))?;
    let hex: String = public.iter().map(|b| format!("{b:02x}")).collect();
    if !is_public_key(&hex) {
        return Err(Error::kms("nostr public key"));
    }
    Ok(hex)
}

/// Wraps application data for one recipient at the host's clock (`created_at`, Unix seconds;
/// wasm32 has no clock of its own). The output is re-checked as the adapter does: a bounded
/// kind-1059 event carrying the recipient's `p` tag.
#[allow(clippy::too_many_arguments)]
pub fn wrap_nostr_application_data(
    mnemonic: &str,
    address_index: u32,
    account_index: u32,
    recipient_public_key: &str,
    identifier: &str,
    content: &str,
    created_at: u64,
    passphrase: Option<&str>,
) -> Result<String, Error> {
    if !is_public_key(recipient_public_key)
        || !is_identifier(identifier)
        || !is_application_data(content)
    {
        return Err(Error::invalid("invalid nostr application data"));
    }
    let key = private_key(mnemonic, address_index, account_index, passphrase)?;
    let wrapped = krusty_kms::wrap_nostr_application_data_at(
        &key,
        recipient_public_key,
        identifier,
        content,
        created_at,
    )
    .map_err(|_| Error::kms("nostr wrap"))?;
    match parse_event_json(&wrapped) {
        Some(event)
            if event.kind == GIFT_WRAP_KIND && has_recipient_tag(&event, recipient_public_key) =>
        {
            Ok(wrapped)
        }
        _ => Err(Error::kms("nostr wrap")),
    }
}

/// Opens one gift wrap addressed to this wallet, from event JSON a relay delivered. A wrapper over
/// [`open_nostr_event`].
pub fn open_nostr_application_data(
    mnemonic: &str,
    address_index: u32,
    account_index: u32,
    event_json: &str,
    passphrase: Option<&str>,
) -> Result<NostrApplicationData, Error> {
    let event =
        parse_event_json(event_json).ok_or_else(|| Error::invalid("invalid nostr gift wrap"))?;
    open_nostr_event(mnemonic, address_index, account_index, &event, passphrase)
}

/// Opens one parsed gift wrap addressed to this wallet. The event is re-serialised from its fields,
/// as the extension hands Krusty `JSON.stringify(event)`, and that text must pass the extension's
/// grammar and be kind 1059 before Krusty sees it (the fields are public, so a caller can build an
/// event that never passed it). Krusty's signature and rumor checks and the output bounds all fail
/// closed, since a relay event is untrusted.
pub fn open_nostr_event(
    mnemonic: &str,
    address_index: u32,
    account_index: u32,
    event: &NostrEvent,
    passphrase: Option<&str>,
) -> Result<NostrApplicationData, Error> {
    let json = serde_json::to_string(event)
        .ok()
        .filter(|json| parse_event_json(json).is_some_and(|e| e.kind == GIFT_WRAP_KIND))
        .ok_or_else(|| Error::invalid("invalid nostr gift wrap"))?;
    let key = private_key(mnemonic, address_index, account_index, passphrase)?;
    let opened = krusty_kms::open_nostr_application_data(&key, &json)
        .map_err(|_| Error::kms("nostr open"))?;
    let valid = is_application_data(&opened.content)
        && is_identifier(&opened.identifier)
        && is_public_key(&opened.sender_public_key);
    if !valid {
        return Err(Error::kms("nostr open"));
    }
    Ok(NostrApplicationData {
        content: opened.content,
        identifier: opened.identifier,
        sender_public_key: opened.sender_public_key,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event() -> Value {
        json!({"content":"x","created_at":1,"id":"a".repeat(64),"kind":1059,
               "pubkey":"b".repeat(64),"sig":"c".repeat(128),"tags":[["p","d".repeat(64)]]})
    }

    #[test]
    fn event_grammar_matches_is_nostr_event() {
        assert!(event_from_value(&event()).is_some());
        let mut float = event();
        float["created_at"] = json!(1.0);
        assert_eq!(event_from_value(&float).unwrap().created_at, 1);
        for (key, bad) in [
            ("created_at", json!(-1)),
            ("created_at", json!(1.5)),
            ("created_at", json!(9_007_199_254_740_992_u64)),
            ("kind", json!(65_536)),
            ("id", json!("A".repeat(64))),
            ("sig", json!("c".repeat(127))),
            ("content", json!("")),
            ("tags", json!([vec!["p"; 9]])),
            ("tags", json!(vec![vec!["p"]; 33])),
            ("tags", json!([["x".repeat(513)]])),
        ] {
            let mut value = event();
            value[key] = bad;
            assert!(event_from_value(&value).is_none(), "{key}");
        }
        let mut extra = event();
        extra["extra"] = json!(1);
        assert!(event_from_value(&extra).is_none());
        let mut missing = event();
        missing.as_object_mut().unwrap().remove("tags");
        assert!(event_from_value(&missing).is_none());
        // UTF-16 length, not bytes: 512 two-byte characters is still within a tag part's bound.
        let mut wide = event();
        wide["tags"] = json!([["é".repeat(512)]]);
        assert!(event_from_value(&wide).is_some());
        assert!(parse_event_json(&"x".repeat(MAX_EVENT_JSON_BYTES + 1)).is_none());
    }

    #[test]
    fn identifiers_are_printable_ascii() {
        assert!(is_identifier("bramble.multisig"));
        assert!(!is_identifier(""));
        assert!(!is_identifier(&"a".repeat(129)));
        assert!(!is_identifier("tab\there"));
        assert!(!is_identifier("é"));
    }

    #[test]
    fn open_refuses_events_before_krusty_sees_them() {
        let phrase = "habit hope tip crystal because grunt nation idea electric witness alert like";
        let mut kind1 = event();
        kind1["kind"] = json!(1);
        let err = open_nostr_application_data(phrase, 0, 0, &kind1.to_string(), None);
        assert!(matches!(err, Err(Error::InvalidInput { .. })));
        let err = open_nostr_application_data(phrase, 0, 0, "{", None);
        assert!(matches!(err, Err(Error::InvalidInput { .. })));
        // Well-formed but unauthenticated: Krusty refuses it.
        let err = open_nostr_application_data(phrase, 0, 0, &event().to_string(), None);
        assert!(matches!(err, Err(Error::Kms { .. })));
        // A hand-built event is held to the same grammar as a parsed one.
        let mut built = event_from_value(&event()).unwrap();
        built.sig = "C".repeat(128);
        let err = open_nostr_event(phrase, 0, 0, &built, None);
        assert!(matches!(err, Err(Error::InvalidInput { .. })));
    }
}
