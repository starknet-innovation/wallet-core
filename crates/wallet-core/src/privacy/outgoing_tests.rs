//! Expected bodies and channels were observed from the pinned SDK's `IndexerDiscoveryProvider`
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

fn walk(recipients: &[&str]) -> OutgoingWalk {
    OutgoingWalk::new(
        felt("0x1"),
        felt("0x208"),
        recipients.iter().map(|r| felt(r)).collect(),
        BlockRef::Number(7000),
    )
}

fn text(body: &[u8]) -> &str {
    std::str::from_utf8(body).unwrap()
}

fn more(walk: OutgoingWalk, reply: &str) -> OutgoingWalk {
    match walk.on_reply(200, reply.as_bytes()).unwrap() {
        Walked::More(walk) => walk,
        Walked::Done(_) => panic!("walk ended early"),
    }
}

fn done(walk: OutgoingWalk, reply: &str) -> OutgoingChannels {
    match walk.on_reply(200, reply.as_bytes()).unwrap() {
        Walked::Done(found) => found,
        Walked::More(_) => panic!("walk did not end"),
    }
}

fn page_one() -> String {
    format!(
        r#"{{"block_ref":123456,"channels":[
        {{"recipient_addr":"0xabc","recipient_public_key":"0x111","channel_key":"0x222","precomputed":true}},
        {{"recipient_addr":"0x5","recipient_public_key":"0x55","channel_key":"0x9","precomputed":false}},
        {{"recipient_addr":"0x6","recipient_public_key":"0x66","channel_key":"0x10"}}],
        "subchannels":[{{"recipient_addr":"0x5","token":"{T1}","last_note_index":null}},
        {{"recipient_addr":"0x6","token":"{T2}","last_note_index":0}}],
        "cursor":{{"channel_discovery_complete":false,"last_channel_index":1,"channels":{{
        "0x5":{{"channel_key":"0x9","subchannel_discovery_complete":true,"last_subchannel_index":0,"subchannels":{{"{T1}":{{"note_discovery_complete":true,"last_note_index":3}}}}}},
        "0x6":{{"channel_key":"0x10","subchannel_discovery_complete":false,"subchannels":{{"{T2}":{{"note_discovery_complete":true}}}}}},
        "0x7":{{"channel_key":"0x11","subchannel_discovery_complete":true,"subchannels":{{"{T1}":{{"note_discovery_complete":true}},"{T2}":{{"note_discovery_complete":false,"last_note_index":2}}}}}},
        "0x8":{{"channel_key":"0x12","subchannel_discovery_complete":true,"subchannels":{{}}}},
        "0x9":{{"channel_key":"0x13","subchannel_discovery_complete":true}}}}}}}}"#
    )
}

fn page_two() -> String {
    format!(
        r#"{{"block_ref":123456,"channels":[{{"recipient_addr":"0xabc","recipient_public_key":"0x111","channel_key":"0x333","precomputed":false}}],
        "subchannels":[{{"recipient_addr":"0x5","token":"{T2}","last_note_index":null}},
        {{"recipient_addr":"0x5","token":"{T1}","last_note_index":4}},
        {{"recipient_addr":"0x5","token":"{T1}","last_note_index":null}},
        {{"recipient_addr":"0xabc","token":"{T1}","last_note_index":0}}],
        "cursor":{{"channel_discovery_complete":true,"total_n_channels":3,"channels":{{}}}}}}"#
    )
}

#[test]
fn a_two_page_walk_matches_the_sdk() {
    let walk = walk(&["0x208", "0xabc", "0x5"]);
    assert_eq!(
        text(&walk.body(&key())),
        r#"{"contract_address":"0x1","sender_address":"0x208","viewing_key":"0x3b","cursor":{"channel_discovery_complete":false,"channels":{}},"block_ref":7000,"recipients":["0x208","0xabc","0x5"]}"#
    );
    let walk = more(walk, &page_one());
    // Pruned: 0x5 (all complete), 0x8 (empty subchannels), 0x7's complete subchannel. Kept: 0x6
    // (subchannel discovery incomplete) and 0x9 (no subchannels object). The block is the reply's.
    assert_eq!(
        text(&walk.body(&key())),
        r#"{"contract_address":"0x1","sender_address":"0x208","viewing_key":"0x3b","cursor":{"channel_discovery_complete":false,"last_channel_index":1,"channels":{"0x6":{"channel_key":"0x10","subchannel_discovery_complete":false,"subchannels":{"0x4718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d":{"note_discovery_complete":true}}},"0x7":{"channel_key":"0x11","subchannel_discovery_complete":true,"subchannels":{"0x4718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d":{"note_discovery_complete":false,"last_note_index":2}}},"0x9":{"channel_key":"0x13","subchannel_discovery_complete":true}}},"block_ref":123456,"recipients":["0x208","0xabc","0x5"]}"#
    );
    let found = done(walk, &page_two());
    assert_eq!(found.total, Some(3));
    // SDK: [[5, 55, 9, [[T1,{0,5}],[T2,{1,0}]]], [abc, 111, 333, [[T1,{0,1}]]]]; 0x6 was not
    // requested, 0x208 has no channel.
    let channels: Vec<_> = found.channels.iter().collect();
    assert_eq!(channels.len(), 2);
    let (recipient, five) = channels[0];
    assert_eq!(*recipient, felt("0x5"));
    assert_eq!(five.public_key, felt("0x55"));
    assert_eq!(five.key.as_ref().unwrap(), &felt("0x9"));
    let tokens: Vec<_> = five.tokens.iter().map(|(t, c)| (*t, *c)).collect();
    assert_eq!(
        tokens,
        [
            (
                felt(T1),
                TokenChannel {
                    token_index: 0,
                    note_nonce: 5
                }
            ),
            (
                felt(T2),
                TokenChannel {
                    token_index: 1,
                    note_nonce: 0
                }
            ),
        ]
    );
    // Precomputed on page one, created on page two: the created entry wins.
    let (recipient, abc) = channels[1];
    assert_eq!(*recipient, felt("0xabc"));
    assert_eq!(abc.key.as_ref().unwrap(), &felt("0x333"));
    assert_eq!(
        abc.tokens.get(&felt(T1)),
        Some(&TokenChannel {
            token_index: 0,
            note_nonce: 1
        })
    );
}

#[test]
fn a_precomputed_channel_has_no_key_and_never_displaces_a_created_one() {
    let reply = r#"{"block_ref":"0x5a1f00d","channels":[
        {"recipient_addr":"0xabc","recipient_public_key":"0x111","channel_key":"0x222","precomputed":true},
        {"recipient_addr":"0x5","recipient_public_key":"0x55","channel_key":"0x9"},
        {"recipient_addr":"0x5","recipient_public_key":"0x56","channel_key":"0x8","precomputed":true}],
        "subchannels":[{"recipient_addr":"0xabc","token":"0x1","last_note_index":3}],
        "cursor":{"channel_discovery_complete":true}}"#;
    let found = done(walk(&["0x5", "0xabc"]), reply);
    assert_eq!(found.total, None);
    let keys: Vec<_> = found.channels.keys().copied().collect();
    assert_eq!(keys, [felt("0x5"), felt("0xabc")]);
    let five = found.channels.get(&felt("0x5")).unwrap();
    assert_eq!(five.public_key, felt("0x55"));
    let abc = found.channels.get(&felt("0xabc")).unwrap();
    assert!(abc.key.is_none() && abc.tokens.is_empty());
}

#[test]
fn an_empty_recipient_list_starts_resolved_and_a_zero_block_falls_back() {
    let walk = walk(&[]);
    assert!(text(&walk.body(&key())).contains(r#""cursor":{"channel_discovery_complete":true,"channels":{}},"block_ref":7000,"recipients":[]}"#));
    // SDK: a reply at block 0 is falsy, so the next page sends the proving block again.
    let zero = r#"{"block_ref":0,"channels":[],"subchannels":[],"cursor":{"channel_discovery_complete":false,"channels":{}}}"#;
    let walk = more(
        OutgoingWalk::new(
            felt("0x1"),
            felt("0x208"),
            vec![felt("0x208")],
            BlockRef::Number(7000),
        ),
        zero,
    );
    assert_eq!(
        text(&walk.body(&key())),
        r#"{"contract_address":"0x1","sender_address":"0x208","viewing_key":"0x3b","cursor":{"channel_discovery_complete":false,"channels":{}},"block_ref":7000,"recipients":["0x208"]}"#
    );
}

#[test]
fn reorgs_and_service_errors_are_distinct() {
    let refuse = |status: u16, body: &str| {
        walk(&["0x5"])
            .on_reply(status, body.as_bytes())
            .unwrap_err()
    };
    assert_eq!(refuse(409, "rate limit"), PrivacyFailure::Reorg);
    assert_eq!(
        refuse(503, "too many requests"),
        PrivacyFailure::RateLimited
    );
    assert_eq!(refuse(500, "boom"), PrivacyFailure::ServiceUnavailable);
}

#[test]
fn malformed_replies_are_refused() {
    let ok_channel =
        r#"{"recipient_addr":"0x5","recipient_public_key":"0x55","channel_key":"0x9"}"#;
    let cursor = r#"{"channel_discovery_complete":true}"#;
    let reply = |channels: &str, subs: &str, cursor: &str| {
        format!(
            r#"{{"block_ref":1,"channels":[{channels}],"subchannels":[{subs}],"cursor":{cursor}}}"#
        )
    };
    assert!(walk(&["0x5"])
        .on_reply(200, reply(ok_channel, "", cursor).as_bytes())
        .is_ok());
    for bad in [
        reply(
            r#"{"recipient_addr":"0x5","recipient_public_key":"0x55","channel_key":"0x0"}"#,
            "",
            cursor,
        ),
        reply(
            r#"{"recipient_addr":5,"recipient_public_key":"0x55","channel_key":"0x9"}"#,
            "",
            cursor,
        ),
        reply(
            r#"{"recipient_addr":"0x5","recipient_public_key":"zz","channel_key":"0x9"}"#,
            "",
            cursor,
        ),
        reply(
            r#"{"recipient_addr":"0x5","recipient_public_key":"0x55","channel_key":"0x9","precomputed":1}"#,
            "",
            cursor,
        ),
        reply(
            r#"{"recipient_addr":"0x800000000000011000000000000000000000000000000000000000000000001","recipient_public_key":"0x55","channel_key":"0x9"}"#,
            "",
            cursor,
        ),
        // Two spellings of one recipient.
        reply(
            &format!(
                r#"{ok_channel},{{"recipient_addr":"0x05","recipient_public_key":"0x55","channel_key":"0x9"}}"#
            ),
            "",
            cursor,
        ),
        reply(
            ok_channel,
            r#"{"recipient_addr":"0x05","token":"0x1","last_note_index":0}"#,
            cursor,
        ),
        reply(
            ok_channel,
            r#"{"recipient_addr":"0x5","token":"0x1","last_note_index":0},{"recipient_addr":"0x5","token":"0x01","last_note_index":1}"#,
            cursor,
        ),
        // `last_note_index`: missing, negative, over u32, or one whose next nonce overflows.
        reply(
            ok_channel,
            r#"{"recipient_addr":"0x5","token":"0x1"}"#,
            cursor,
        ),
        reply(
            ok_channel,
            r#"{"recipient_addr":"0x5","token":"0x1","last_note_index":-1}"#,
            cursor,
        ),
        reply(
            ok_channel,
            r#"{"recipient_addr":"0x5","token":"0x1","last_note_index":4294967296}"#,
            cursor,
        ),
        reply(
            ok_channel,
            r#"{"recipient_addr":"0x5","token":"0x1","last_note_index":4294967295}"#,
            cursor,
        ),
        reply(
            ok_channel,
            "",
            r#"{"channel_discovery_complete":true,"total_n_channels":"3"}"#,
        ),
        reply(ok_channel, "", r#"{"channel_discovery_complete":"yes"}"#),
        reply(
            ok_channel,
            "",
            r#"{"channel_discovery_complete":false,"channels":{"0x5":{"subchannels":[]}}}"#,
        ),
        reply(
            ok_channel,
            "",
            r#"{"channel_discovery_complete":false,"channels":{"0x5":{"subchannel_discovery_complete":1}}}"#,
        ),
        // A key `JSON.stringify` would move to the front.
        reply(
            ok_channel,
            "",
            r#"{"channel_discovery_complete":false,"channels":{"0x5":{"7":true}}}"#,
        ),
        format!(r#"{{"channels":[],"subchannels":[],"cursor":{cursor}}}"#),
        format!(r#"{{"block_ref":1,"subchannels":[],"cursor":{cursor}}}"#),
        r#"{"block_ref":1,"channels":[],"subchannels":[]}"#.to_owned(),
        "not json".to_owned(),
    ] {
        assert_eq!(
            walk(&["0x5"]).on_reply(200, bad.as_bytes()).unwrap_err(),
            PrivacyFailure::InvalidResponse,
            "{bad}"
        );
    }
}

#[test]
fn a_later_page_must_answer_from_the_first_pages_block_and_pages_are_bounded() {
    let open = |block: &str| {
        format!(
            r#"{{"block_ref":{block},"channels":[],"subchannels":[],"cursor":{{"channel_discovery_complete":false}}}}"#
        )
    };
    let walk = more(walk(&["0x5"]), &open("1"));
    assert_eq!(
        walk.on_reply(200, open("2").as_bytes()).unwrap_err(),
        PrivacyFailure::InvalidResponse
    );
    let mut walk = super::OutgoingWalk::new(
        felt("0x1"),
        felt("0x2"),
        vec![felt("0x5")],
        BlockRef::Number(1),
    );
    for _ in 1..MAX_DISCOVERY_PAGES {
        walk = more(walk, &open("1"));
    }
    assert_eq!(
        walk.on_reply(200, open("1").as_bytes()).unwrap_err(),
        PrivacyFailure::InvalidResponse
    );
}

#[test]
fn the_total_is_read_only_from_the_final_cursor() {
    let found = done(
        walk(&["0x5"]),
        r#"{"block_ref":1,"channels":[],"subchannels":[],"cursor":{"channel_discovery_complete":true,"total_n_channels":4294967295}}"#,
    );
    assert_eq!(found.total, Some(u32::MAX));
    assert!(found.channels.is_empty());
}

/// One page naming `n` recipients nobody asked about, each with a channel and a subchannel, and
/// one of them twice in two spellings, around the requested 0x5.
fn crowded(n: u64) -> String {
    let mut channels = vec![
        r#"{"recipient_addr":"0x5","recipient_public_key":"0x55","channel_key":"0x9"}"#.to_owned(),
    ];
    let mut subs = vec![format!(
        r#"{{"recipient_addr":"0x5","token":"{T1}","last_note_index":2}}"#
    )];
    for at in 0..n {
        let addr = format!("{:#x}", 0x1000 + at);
        channels.push(format!(
            r#"{{"recipient_addr":"{addr}","recipient_public_key":"0x1","channel_key":"0x2"}}"#
        ));
        subs.push(format!(
            r#"{{"recipient_addr":"{addr}","token":"{T2}","last_note_index":null}}"#
        ));
    }
    channels.push(
        r#"{"recipient_addr":"0x01000","recipient_public_key":"0x1","channel_key":"0x2"}"#.into(),
    );
    format!(
        r#"{{"block_ref":7,"channels":[{}],"subchannels":[{}],"cursor":{{"channel_discovery_complete":false}}}}"#,
        channels.join(","),
        subs.join(",")
    )
}

#[test]
fn recipients_not_requested_are_read_then_ignored() {
    let walk = more(walk(&["0x5"]), &crowded(5_000));
    // Only what the job asked about is held.
    assert_eq!(
        (
            walk.created.len(),
            walk.spellings.len(),
            walk.subchannels.len()
        ),
        (1, 1, 1)
    );
    assert_eq!(walk.subchannel_count, 1);
    let found = done(
        walk,
        r#"{"block_ref":7,"channels":[],"subchannels":[],"cursor":{"channel_discovery_complete":true,"total_n_channels":9}}"#,
    );
    let channels: Vec<_> = found.channels.iter().collect();
    assert_eq!(channels.len(), 1);
    assert_eq!(*channels[0].0, felt("0x5"));
    assert_eq!(
        channels[0].1.tokens.get(&felt(T1)),
        Some(&TokenChannel {
            token_index: 0,
            note_nonce: 3
        })
    );
    // An unrequested entry is still judged.
    let bad = r#"{"block_ref":7,"channels":[{"recipient_addr":"0x6","recipient_public_key":"0x1","channel_key":"0x0"}],"subchannels":[],"cursor":{"channel_discovery_complete":false}}"#;
    assert!(self::walk(&["0x5"]).on_reply(200, bad.as_bytes()).is_err());
}

#[test]
fn a_walk_keeps_at_most_max_subchannels() {
    let subs = |n: usize| {
        let subs: Vec<String> = (0..n)
            .map(|at| {
                format!(
                    r#"{{"recipient_addr":"0x5","token":"{:#x}","last_note_index":null}}"#,
                    0x1000 + at
                )
            })
            .collect();
        format!(
            r#"{{"block_ref":7,"channels":[],"subchannels":[{}],"cursor":{{"channel_discovery_complete":false}}}}"#,
            subs.join(",")
        )
    };
    let walk = more(walk(&["0x5"]), &subs(MAX_SUBCHANNELS));
    assert_eq!(walk.subchannel_count, MAX_SUBCHANNELS);
    assert_eq!(
        self::walk(&["0x5"])
            .on_reply(200, subs(MAX_SUBCHANNELS + 1).as_bytes())
            .err(),
        Some(PrivacyFailure::InvalidResponse)
    );
}
