use super::*;
use crate::privacy::pool_actions::{decode_compile_actions, PoolAction};

const ACCOUNT: Felt = Felt::from_hex_unchecked("0x123");
const BOB: Felt = Felt::from_hex_unchecked("0xb0b");
const STRK: Felt =
    Felt::from_hex_unchecked("0x04718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d");
const KEY: Felt = Felt::from_hex_unchecked("0x654321");

fn f(value: u64) -> Felt {
    Felt::from(value)
}

fn every_variant() -> Vec<ClientAction> {
    vec![
        ClientAction::SetViewingKey { random: f(5) },
        ClientAction::OpenChannel {
            recipient_addr: BOB,
            index: 1,
            random: f(2),
            salt: f(3),
        },
        ClientAction::OpenSubchannel {
            recipient_addr: BOB,
            recipient_public_key: f(9),
            channel_key: SecretFelt::new(f(8)),
            index: 4,
            token: STRK,
            salt: f(6),
        },
        ClientAction::CreateEncNote {
            recipient_addr: BOB,
            recipient_public_key: f(9),
            token: STRK,
            amount: 70,
            index: 2,
            salt: u128::MAX,
        },
        ClientAction::CreateOpenNote {
            recipient_addr: BOB,
            recipient_public_key: f(9),
            token: STRK,
            index: 0,
            random: f(1),
        },
        ClientAction::Deposit {
            token: STRK,
            amount: 100,
        },
        ClientAction::UseNote {
            channel_key: SecretFelt::new(f(8)),
            token: STRK,
            index: u32::MAX,
        },
        ClientAction::Withdraw {
            to_addr: ACCOUNT,
            token: STRK,
            amount: 30,
            random: f(7),
        },
        ClientAction::InvokeExternal {
            contract_address: BOB,
            calldata: vec![f(11), f(12)],
        },
        ClientAction::ComputeAndInvoke {
            contract_address: BOB,
            compute_additional_data: vec![],
            invoke_additional_data: vec![f(13)],
        },
    ]
}

fn plain(calldata: &[SecretFelt]) -> Vec<Felt> {
    calldata.iter().map(|felt| *felt.expose_secret()).collect()
}

#[test]
fn encodes_index_then_members_in_abi_order() {
    let calldata =
        compile_actions_calldata(&ACCOUNT, &SecretFelt::new(KEY), &every_variant()).unwrap();
    let u128_max = Felt::from(u128::MAX);
    #[rustfmt::skip]
    let expected = vec![
        ACCOUNT, KEY, f(10),
        f(0), f(5),
        f(1), BOB, f(1), f(2), f(3),
        f(2), BOB, f(9), f(8), f(4), STRK, f(6),
        f(3), BOB, f(9), STRK, f(70), f(2), u128_max,
        f(4), BOB, f(9), STRK, f(0), f(1),
        f(5), STRK, f(100),
        f(6), f(8), STRK, f(u64::from(u32::MAX)),
        f(7), ACCOUNT, STRK, f(30), f(7),
        f(8), BOB, f(2), f(11), f(12),
        f(9), BOB, f(0), f(1), f(13),
    ];
    assert_eq!(plain(&calldata), expected);
}

#[test]
fn every_variant_matches_the_layout() {
    for (at, action) in every_variant().iter().enumerate() {
        let (variant, members) = action.members();
        let names: Vec<&str> = members.iter().map(|(name, _)| *name).collect();
        assert_eq!((variant, names.as_slice()), LAYOUT[at]);
    }
}

#[test]
fn round_trips_through_the_pool_decoder() {
    let key = SecretFelt::new(KEY);
    // Every action decodes, in order, the open note (an `Open` transfer's) included.
    let calldata = compile_actions_calldata(&ACCOUNT, &key, &every_variant()).unwrap();
    assert_eq!(
        decode_compile_actions(&calldata, &ACCOUNT, &key),
        Some(vec![
            PoolAction::Register,
            PoolAction::Channel { recipient: BOB },
            PoolAction::Channel { recipient: BOB },
            PoolAction::Note {
                recipient: BOB,
                token: STRK,
                amount: f(70),
            },
            PoolAction::OpenNote {
                recipient: BOB,
                token: STRK,
                note_id: crate::privacy::hashes::compute_note_id(
                    &crate::compute_channel_key(&ACCOUNT, &key, &BOB, &f(9)),
                    &STRK,
                    0
                ),
            },
            PoolAction::Deposit {
                token: STRK,
                amount: f(100),
            },
            PoolAction::UseNote,
            PoolAction::Withdraw {
                recipient: ACCOUNT,
                token: STRK,
                amount: f(30),
            },
            PoolAction::Invoke {
                contract: BOB,
                calldata: vec![f(11), f(12)],
            },
            PoolAction::ComputeAndInvoke {
                contract: BOB,
                compute: vec![],
                invoke: vec![f(13)],
            },
        ])
    );
}

#[test]
fn debug_hides_channel_keys() {
    let action = ClientAction::UseNote {
        channel_key: SecretFelt::new(Felt::from_hex_unchecked("0xdeadbeef")),
        token: STRK,
        index: 0,
    };
    assert!(!format!("{action:?}").to_lowercase().contains("deadbeef"));
}
