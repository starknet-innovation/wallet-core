//! Request bodies, byte for byte what the vendored SDK's `IndexerDiscoveryProvider` sends through
//! `JSON.stringify` (pinned by `tests/fixtures/extension-privacy-wire.json`), plus the pool-fee
//! `starknet_call` the extension's `readStrk20PoolFee` makes through starknet.js.
//!
//! The discovery bodies carry the viewing key, and the walks' bodies the service's cursor, which
//! holds channel keys in clear. Only the public parts are built as plain strings; the key's hex
//! (`toHex(vk)`) lives in one zeroizing string, the cursor's echo in one exactly sized zeroizing
//! string (`json::sized`), and both in one exactly sized zeroizing body that never reallocates.
//! Callers move the finished buffer straight into a `Post`; a wallet that crosses FFI makes its
//! one copy there (Bramble's `mobile/docs/privacy.md` §1.1, §5).

use serde_json::json;
use zeroize::Zeroizing;

use super::json::{sized, write_str, Ordered, Sink};
use crate::{felt, Felt, SecretFelt};

/// `sn_keccak("get_fee_amount")`, checked against `kms::selector` in the tests.
const GET_FEE_AMOUNT_SELECTOR: &str =
    "0x3d323cd692ad43935b81ce230c47bfc57f69656249c5a33fe5223c17dd32ed2";

/// The discovery paths, appended to the configured URL as the SDK does.
pub const PREFLIGHT_PATH: &str = "/v1/sync/preflight_check";
pub const INCOMING_PATH: &str = "/v1/sync/incoming_state";
pub const OUTGOING_PATH: &str = "/v1/sync/outgoing_state";

/// A pinned block, echoed exactly as the service first sent it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockRef {
    Text(String),
    Number(u64),
}

impl BlockRef {
    fn write(&self, out: &mut dyn Sink) {
        match self {
            Self::Text(text) => write_str(out, text),
            Self::Number(number) => out.put(&number.to_string()),
        }
    }
}

fn member(out: &mut dyn Sink, key: &str, value: &str) {
    write_str(out, key);
    out.put(":");
    write_str(out, value);
}

/// Joins the public `head` and `tail` around `"viewing_key":"0x…"` in one buffer sized exactly
/// up front, so it never reallocates (a reallocation would free a copy of the key unwiped). The key
/// is canonical lowercase hex, which JSON needs no escaping for, so it is pushed without the
/// serde temporary `write_str` would allocate.
fn with_key(head: &str, viewing_key: &SecretFelt, tail: &str) -> Zeroizing<Vec<u8>> {
    const KEY_MEMBER: &str = "\"viewing_key\":\"";
    // `{:#x}`, the same spelling as `felt::canonical`, straight into a zeroizing string.
    let key = viewing_key.expose_secret_hex();
    debug_assert!(key.bytes().all(|b| b.is_ascii_hexdigit() || b == b'x'));
    let len = head.len() + KEY_MEMBER.len() + key.len() + 1 + tail.len();
    let mut out = Zeroizing::new(Vec::with_capacity(len));
    out.extend_from_slice(head.as_bytes());
    out.extend_from_slice(KEY_MEMBER.as_bytes());
    out.extend_from_slice(key.as_bytes());
    out.push(b'"');
    out.extend_from_slice(tail.as_bytes());
    debug_assert_eq!(out.len(), len);
    debug_assert_eq!(out.capacity(), len);
    out
}

/// `POST /v1/sync/preflight_check`, registration read for the account itself.
pub fn preflight(
    pool: &Felt,
    account: &Felt,
    viewing_key: &SecretFelt,
    token: &Felt,
) -> Zeroizing<Vec<u8>> {
    let mut head = String::from("{");
    member(&mut head, "contract_address", &felt::canonical(pool));
    head.push(',');
    member(&mut head, "sender_address", &felt::canonical(account));
    head.push(',');
    let mut tail = String::from(",");
    member(&mut tail, "recipient", &felt::canonical(account));
    tail.push(',');
    member(&mut tail, "token", &felt::canonical(token));
    tail.push('}');
    with_key(&head, viewing_key, &tail)
}

/// `POST /v1/sync/incoming_state`: `cursor` is `{}` on the first page and the reply's cursor
/// after; `block_ref` follows it when the caller has one (`discoverNotes` sends the pinned block
/// from the second page on, and on the first page only when given a `blockIdentifier`).
pub fn incoming(
    pool: &Felt,
    account: &Felt,
    viewing_key: &SecretFelt,
    cursor: Option<&Ordered>,
    block: Option<&BlockRef>,
) -> Zeroizing<Vec<u8>> {
    let mut head = String::from("{");
    member(&mut head, "contract_address", &felt::canonical(pool));
    head.push(',');
    member(&mut head, "recipient_address", &felt::canonical(account));
    head.push(',');
    // The cursor carries channel keys in clear: written into one exact zeroizing buffer.
    let tail = sized(|out| {
        out.put(",\"cursor\":");
        match cursor {
            None => out.put("{}"),
            Some(cursor) => cursor.write(out),
        }
        if let Some(block) = block {
            out.put(",\"block_ref\":");
            block.write(out);
        }
        out.put("}");
    });
    with_key(&head, viewing_key, &tail)
}

/// `POST /v1/sync/outgoing_state`, as `discoverChannels` sends it for a recipient list: the
/// cursor, the block, then every recipient as `toHex` writes it, in the caller's order.
pub fn outgoing(
    pool: &Felt,
    account: &Felt,
    viewing_key: &SecretFelt,
    cursor: &Ordered,
    block: &BlockRef,
    recipients: &[Felt],
) -> Zeroizing<Vec<u8>> {
    let mut head = String::from("{");
    member(&mut head, "contract_address", &felt::canonical(pool));
    head.push(',');
    member(&mut head, "sender_address", &felt::canonical(account));
    head.push(',');
    let tail = sized(|out| {
        out.put(",\"cursor\":");
        cursor.write(out);
        out.put(",\"block_ref\":");
        block.write(out);
        out.put(",\"recipients\":[");
        for (index, recipient) in recipients.iter().enumerate() {
            if index > 0 {
                out.put(",");
            }
            write_str(out, &felt::canonical(recipient));
        }
        out.put("]}");
    });
    with_key(&head, viewing_key, &tail)
}

/// `starknet_call` of the pool's `get_fee_amount()` at `latest`, with the pool spelled as
/// configured (starknet.js passes the address through unchanged).
pub fn pool_fee(id: u64, pool_address: &str) -> String {
    json!({
        "id": id,
        "jsonrpc": "2.0",
        "method": "starknet_call",
        "params": {
            "block_id": "latest",
            "request": {
                "calldata": [],
                "contract_address": pool_address,
                "entry_point_selector": GET_FEE_AMOUNT_SELECTOR,
            },
        },
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selector_is_starknet_keccak() {
        assert_eq!(
            crate::felt::canonical(&crate::selector("get_fee_amount")),
            GET_FEE_AMOUNT_SELECTOR
        );
    }

    #[test]
    fn first_page_has_an_empty_cursor_and_no_block() {
        let one = Felt::ONE;
        let key = SecretFelt::new(Felt::THREE);
        let body = incoming(&one, &Felt::TWO, &key, None, None);
        assert_eq!(
            std::str::from_utf8(&body).unwrap(),
            r#"{"contract_address":"0x1","recipient_address":"0x2","viewing_key":"0x3","cursor":{}}"#
        );
        let cursor = Ordered::Object(vec![("z".into(), Ordered::Bool(false))]);
        let later = incoming(
            &one,
            &Felt::TWO,
            &key,
            Some(&cursor),
            Some(&BlockRef::Number(7)),
        );
        assert!(std::str::from_utf8(&later)
            .unwrap()
            .ends_with(r#""cursor":{"z":false},"block_ref":7}"#));
    }

    #[test]
    fn bodies_are_sized_exactly_so_the_key_is_never_reallocated() {
        let max = Felt::MAX;
        let key = SecretFelt::new(max);
        let body = preflight(&max, &max, &key, &max);
        assert_eq!(body.len(), body.capacity());
        let text = std::str::from_utf8(&body).unwrap();
        assert!(text.starts_with(r#"{"contract_address":"0x8"#));
        assert!(text.contains(r#","viewing_key":"0x8"#));
        assert!(text.ends_with(r#"0000"}"#));
        let cursor = Ordered::Str("c".repeat(100_000));
        let page = incoming(
            &max,
            &max,
            &key,
            Some(&cursor),
            Some(&BlockRef::Text("b".into())),
        );
        assert_eq!(page.len(), page.capacity());
        let walk = outgoing(&max, &max, &key, &cursor, &BlockRef::Number(1), &[max, max]);
        assert_eq!(walk.len(), walk.capacity());
    }
}
