//! Expected bodies and notes were observed from the pinned SDK's `IndexerDiscoveryProvider`
//! (`mc-wallet/vendor/starknet-privacy/sdk/src`) run under bun with `fetch` replaced by a recorder
//! replaying the replies below; the bodies are pasted as it printed them.

use super::*;

const T1: &str = "0x49d36570d4e46f48e99674bd3fcc84644ddd6b96f7c741b1562b82f9e004dc7";
const T2: &str = "0x4718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d";

fn felt(hex: &str) -> Felt {
    crate::felt::parse(hex).unwrap()
}

fn key() -> SecretFelt {
    SecretFelt::new(felt("0x3b"))
}

fn walk_with(max_notes: usize) -> IncomingWalk {
    IncomingWalk::new(
        felt("0x1"),
        felt("0x208"),
        vec![felt(T1)],
        BlockRef::Number(7000),
        max_notes,
    )
}

fn walk() -> IncomingWalk {
    walk_with(64)
}

fn text(body: &[u8]) -> &str {
    std::str::from_utf8(body).unwrap()
}

fn more(walk: IncomingWalk, reply: &str) -> IncomingWalk {
    match walk.on_reply(200, reply.as_bytes()).unwrap() {
        Walked::More(walk) => walk,
        Walked::Done(_) => panic!("walk ended early"),
    }
}

fn done(walk: IncomingWalk, reply: &str) -> AddressMap<Vec<Note>> {
    spendable(walk, reply).notes
}

fn spendable(walk: IncomingWalk, reply: &str) -> SpendableNotes {
    match walk.on_reply(200, reply.as_bytes()).unwrap() {
        Walked::Done(notes) => notes,
        Walked::More(_) => panic!("walk did not end"),
    }
}

fn note(sender: &str, token: &str, index: u32, id: &str, amount: &str, salt: &str) -> String {
    format!(
        r#"{{"sender_addr":"{sender}","token":"{token}","index":{index},"note_id":"{id}","amount":"{amount}","salt":"{salt}","block_number":10}}"#
    )
}

fn page(channels: &str, notes: &[String], cursor: &str) -> String {
    format!(
        r#"{{"block_ref":7000,"channels":[{channels}],"subchannels":[],"notes":[{}],"cursor":{cursor}}}"#,
        notes.join(",")
    )
}

const DONE: &str = r#"{"channel_discovery_complete":true}"#;
const FIVE: &str = r#"{"channel_key":"0x9","sender_addr":"0x5"}"#;

#[test]
fn a_two_page_walk_matches_the_sdk() {
    let walk = walk();
    assert_eq!(
        text(&walk.body(&key())),
        r#"{"contract_address":"0x1","recipient_address":"0x208","viewing_key":"0x3b","cursor":{},"block_ref":7000}"#
    );
    let cursor = format!(
        r#"{{"channel_discovery_complete":false,"channels":{{"0x05":{{"channel_key":"0x9","subchannel_discovery_complete":false,"subchannels":{{"{T1}":{{"note_discovery_complete":false,"last_note_index":1}}}}}},"0x6":{{"channel_key":"0x44"}}}},"last_channel_index":1}}"#
    );
    let first = page(
        FIVE,
        &[
            note("0x5", T2, 0, "0xa1", "5", "77"),
            note("0x5", T1, 0, "0xb1", "100", "1"),
            note("0x5", T1, 1, "0xb2", "0x20", "12345"),
        ],
        &cursor,
    );
    let walk = more(walk, &first);
    // The cursor is echoed whole (no pruning on this endpoint), the block is the reply's.
    assert_eq!(
        text(&walk.body(&key())),
        r#"{"contract_address":"0x1","recipient_address":"0x208","viewing_key":"0x3b","cursor":{"channel_discovery_complete":false,"channels":{"0x05":{"channel_key":"0x9","subchannel_discovery_complete":false,"subchannels":{"0x49d36570d4e46f48e99674bd3fcc84644ddd6b96f7c741b1562b82f9e004dc7":{"note_discovery_complete":false,"last_note_index":1}}},"0x6":{"channel_key":"0x44"}},"last_channel_index":1},"block_ref":7000}"#
    );
    // Sender 0x6 has no channel in this page: its key comes from the first page's cursor.
    let notes = done(
        walk,
        &page("", &[note("0x6", T1, 0, "0xc1", "7", "9")], DONE),
    );
    // SDK: [[T1, [{0xb1, 100, open, ck 9, nonce 0, r 1}, {0xb2, 32, ck 9, nonce 1, r 12345},
    //            {0xc1, 7, ck 0x44, nonce 0, r 9}]]]; T2 was filtered out.
    assert_eq!(notes.len(), 1);
    let found = notes.get(&felt(T1)).unwrap();
    let summary: Vec<_> = found
        .iter()
        .map(|n| (n.id, n.amount, n.sender, n.open, n.witness.nonce))
        .collect();
    assert_eq!(
        summary,
        [
            (felt("0xb1"), 100, felt("0x5"), true, 0),
            (felt("0xb2"), 32, felt("0x5"), false, 1),
            (felt("0xc1"), 7, felt("0x6"), false, 0),
        ]
    );
    assert_eq!(found[0].witness.channel_key, felt("0x9"));
    assert_eq!(found[0].witness.r, Felt::ONE);
    assert_eq!(found[1].witness.r, Felt::from(12345u32));
    assert_eq!(found[2].witness.channel_key, felt("0x44"));
}

#[test]
fn notes_keep_token_and_arrival_order_and_a_page_key_beats_the_cursor() {
    let walk = IncomingWalk::new(
        felt("0x1"),
        felt("0x208"),
        vec![felt(T1), felt(T2)],
        BlockRef::Number(7000),
        64,
    );
    let cursor = r#"{"channel_discovery_complete":false,"channels":{"0x5":{"channel_key":"0x8"}}}"#;
    let walk = more(
        walk,
        &page(FIVE, &[note("0x5", T2, 0, "0x1", "1", "3")], cursor),
    );
    let second = page(
        r#"{"channel_key":"0x7","sender_addr":"0x5"}"#,
        &[
            note("0x5", T1, 0, "0x2", "2", "3"),
            note("0x5", T2, 1, "0x3", "3", "3"),
        ],
        DONE,
    );
    let notes = done(walk, &second);
    assert_eq!(
        notes.keys().copied().collect::<Vec<_>>(),
        [felt(T2), felt(T1)]
    );
    let t2 = notes.get(&felt(T2)).unwrap();
    assert_eq!(
        t2.iter().map(|n| n.id).collect::<Vec<_>>(),
        [felt("0x1"), felt("0x3")]
    );
    assert_eq!(t2[0].witness.channel_key, felt("0x9"));
    assert_eq!(t2[1].witness.channel_key, felt("0x7"));
}

#[test]
fn a_note_without_a_channel_key_is_refused() {
    let invalid = |reply: String| walk().on_reply(200, reply.as_bytes()).unwrap_err();
    // No channel anywhere.
    let orphan = page("", &[note("0x5", T1, 0, "0x1", "1", "3")], DONE);
    assert_eq!(invalid(orphan), PrivacyFailure::InvalidResponse);
    // The page's channel spelled differently from the note's sender (the SDK would fall back).
    let respelled = page(FIVE, &[note("0x05", T1, 0, "0x1", "1", "3")], DONE);
    assert_eq!(invalid(respelled), PrivacyFailure::InvalidResponse);
    // An earlier cursor named the sender without a key (the SDK would use key 0).
    let keyless = r#"{"channel_discovery_complete":false,"channels":{"0x5":{}}}"#;
    let walk = more(walk(), &page("", &[], keyless));
    let later = page("", &[note("0x5", T1, 0, "0x1", "1", "3")], DONE);
    assert_eq!(
        walk.on_reply(200, later.as_bytes()).unwrap_err(),
        PrivacyFailure::InvalidResponse
    );
    // The cursor of the same page does not count: the SDK merges it after the notes.
    let same_page = page("", &[note("0x5", T1, 0, "0x1", "1", "3")], keyless);
    assert_eq!(invalid(same_page), PrivacyFailure::InvalidResponse);
}

#[test]
fn filtered_and_zero_notes_are_not_kept_and_the_cull_counts_nonzero_notes() {
    let notes = vec![
        note("0x6", T2, 0, "0x1", "1", "3"),
        note("0x5", T1, 0, "0x2", "0", "3"),
        note("0x5", T1, 1, "0x3", "0", "3"),
        note("0x5", T1, 2, "0x4", "4", "3"),
    ];
    // Sender 0x6 has no key, but its token is filtered out before the key is looked for.
    let found = done(walk_with(1), &page(FIVE, &notes, DONE));
    let t1 = found.get(&felt(T1)).unwrap();
    assert_eq!(t1.iter().map(|n| n.amount).collect::<Vec<_>>(), [4]);
    let over = page(
        FIVE,
        &[
            note("0x5", T1, 0, "0x2", "1", "3"),
            note("0x5", T1, 1, "0x3", "1", "3"),
        ],
        DONE,
    );
    // Past `max_notes` a note is culled, not refused: the earlier of two equal ones stays.
    let found = spendable(walk_with(1), &over);
    let t1 = found.notes.get(&felt(T1)).unwrap();
    assert_eq!(t1.iter().map(|n| n.id).collect::<Vec<_>>(), [felt("0x2")]);
    assert_eq!(found.dropped.get(&felt(T1)), Some(&U256::from_u128(1)));
}

#[path = "incoming_dust_tests.rs"]
mod dust;

#[test]
fn malformed_notes_are_refused() {
    let u128_max = u128::MAX.to_string();
    let over_u128 = format!("0x1{}", "0".repeat(32));
    let p = "0x800000000000011000000000000000000000000000000000000000000000001";
    for bad in [
        note("0x5", T1, 0, "0x1", &over_u128, "3"),
        note("0x5", T1, 0, "0x1", "-1", "3"),
        note("0x5", T1, 0, p, "1", "3"),
        note("0x5", T1, 0, "0x1", "1", p),
        note("0x5", T1, 0, "0x1", "1", "0x1"),
        note("0x5", T1, 0, "0x1", "1", "01"),
        note(p, T1, 0, "0x1", "1", "3"),
        note("0x5", p, 0, "0x1", "1", "3"),
        format!(
            r#"{{"sender_addr":"0x5","token":"{T1}","index":4294967296,"note_id":"0x1","amount":"1","salt":"3","block_number":1}}"#
        ),
        format!(
            r#"{{"sender_addr":"0x5","token":"{T1}","index":0,"note_id":1,"amount":"1","salt":"3","block_number":1}}"#
        ),
        format!(
            r#"{{"sender_addr":"0x5","token":"{T1}","index":0,"note_id":"0x1","amount":"1","salt":"3"}}"#
        ),
        format!(
            r#"{{"sender_addr":"0x5","token":"{T1}","index":0,"note_id":"0x1","amount":"1","salt":"3","block_number":"1"}}"#
        ),
        format!(
            "{},{}",
            note("0x5", T1, 0, "0x1", "1", "3"),
            note("0x5", T1, 1, "0x01", "2", "3")
        ),
    ] {
        let reply = page(FIVE, std::slice::from_ref(&bad), DONE);
        assert_eq!(
            walk().on_reply(200, reply.as_bytes()).unwrap_err(),
            PrivacyFailure::InvalidResponse,
            "{bad}"
        );
    }
    let max = done(
        walk(),
        &page(FIVE, &[note("0x5", T1, 0, "0x1", &u128_max, "3")], DONE),
    );
    assert_eq!(max.get(&felt(T1)).unwrap()[0].amount, u128::MAX);
    for channels in [
        r#"{"channel_key":"0x0","sender_addr":"0x5"}"#.to_owned(),
        format!("{FIVE},{FIVE}"),
        format!(r#"{FIVE},{{"channel_key":"0x9","sender_addr":"0x05"}}"#),
    ] {
        let reply = page(&channels, &[], DONE);
        assert_eq!(
            walk().on_reply(200, reply.as_bytes()).unwrap_err(),
            PrivacyFailure::InvalidResponse
        );
    }
    let index_key = r#"{"channel_discovery_complete":false,"channels":{"0x5":{"channel_key":"0x9","subchannels":{"12":{}}}}}"#;
    assert!(walk()
        .on_reply(200, page("", &[], index_key).as_bytes())
        .is_err());
}

#[test]
fn reorgs_blocks_and_the_page_bound() {
    assert_eq!(
        walk().on_reply(409, b"BLOCK_REORGED").unwrap_err(),
        PrivacyFailure::Reorg
    );
    assert_eq!(
        walk().on_reply(502, b"").unwrap_err(),
        PrivacyFailure::ServiceUnavailable
    );
    let open = |block: &str| {
        format!(
            r#"{{"block_ref":{block},"channels":[],"subchannels":[],"notes":[],"cursor":{{}}}}"#
        )
    };
    let walk_two = more(walk(), &open(r#""0x5a1f00d""#));
    assert!(text(&walk_two.body(&key())).ends_with(r#""cursor":{},"block_ref":"0x5a1f00d"}"#));
    assert_eq!(
        walk_two.on_reply(200, open("7000").as_bytes()).unwrap_err(),
        PrivacyFailure::InvalidResponse
    );
    let mut long = walk();
    for _ in 1..MAX_DISCOVERY_PAGES {
        long = more(long, &open("7000"));
    }
    assert_eq!(
        long.on_reply(200, open("7000").as_bytes()).unwrap_err(),
        PrivacyFailure::InvalidResponse
    );
}
