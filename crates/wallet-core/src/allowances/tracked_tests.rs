// `collectTrackedAllowances` (`allowance-scan.ts`) and the pair order of `readTrackedAmounts`
// (`allowance-ops.ts`), against `scan_node::Node`. The extension's tests drive these only
// through the background; these cases are the mechanism's own.

use serde_json::{json, Value};

use super::discovery::AllowanceReading;
use super::scan::ScanStep;
use super::scan_node::*;
use super::tracked::{tracked_pairs, TrackedAllowancePair, TrackedAllowanceRead};
use super::TEST_POLICY;
use crate::portfolio::wire::RpcReply;
use crate::send::u256::U256;

fn pair(id: &str, owner: &str, spender: &str) -> TrackedAllowancePair {
    TrackedAllowancePair {
        id: id.into(),
        owner: owner.into(),
        token: TOKEN.into(),
        spender: spender.into(),
    }
}

fn read_all(read: &mut TrackedAllowanceRead, node: &Node) -> Option<Vec<AllowanceReading>> {
    loop {
        let calls = read.next_calls();
        if calls.is_empty() {
            return read.finish().ok();
        }
        for call in calls {
            if read.accept(node.answer(&call)) == ScanStep::Transport {
                return None;
            }
        }
    }
}

#[test]
fn tracked_reads_read_each_pair_and_skip_what_the_node_refuses() {
    let mut chain = node(100, vec![]);
    chain
        .amounts
        .push((canonical("0x5"), ["0x0".into(), "0x0".into()]));
    chain.fault =
        Some(|b| (b["params"]["request"]["calldata"][1] == json!("0x6")).then(|| semantic(0)));
    let pairs = [
        pair("a", OWNER, &drainer()),
        pair("b", OWNER, "0x05"),
        pair("c", OWNER, "0x6"),
        pair("d", "0x0", "0x7"),
    ];
    let mut read = TrackedAllowanceRead::new(&pairs, TEST_POLICY);
    let readings = read_all(&mut read, &chain).unwrap();
    // A zero reading is kept (it settles a revoke); a refused pair and a zero owner give none.
    let expected = vec![
        AllowanceReading {
            id: "a".into(),
            amount: unlimited_low(),
        },
        AllowanceReading {
            id: "b".into(),
            amount: U256::ZERO,
        },
    ];
    assert_eq!(readings, expected);
    // The calldata is canonical: owner then spender.
    let mut read = TrackedAllowanceRead::new(&[pair("b", "0x01", "0x05")], TEST_POLICY);
    let call: Value = serde_json::from_str(&read.next_calls()[0].body).unwrap();
    assert_eq!(call["params"]["request"]["calldata"], json!(["0x1", "0x5"]));
    assert_eq!(call["params"]["block_id"], json!("latest"));
}

#[test]
fn tracked_reads_stop_at_the_cap_and_restart_on_transport() {
    // MAX_TRACKED_PAIRS: invalid pairs count against it, as `pairs.slice(0, 60)` does first.
    let mut pairs: Vec<TrackedAllowancePair> = vec![pair("bad", OWNER, "nope")];
    pairs.extend((0..70).map(|i| pair(&format!("p{i}"), OWNER, &format!("0x{:x}", 0x100 + i))));
    let chain = node(100, vec![]);
    let mut read = TrackedAllowanceRead::new(&pairs, TEST_POLICY);
    assert_eq!(read_all(&mut read, &chain).unwrap().len(), 59);
    let mut failing = node(100, vec![]);
    failing.fault = Some(|_| {
        Some(RpcReply {
            id: 0,
            status: 429,
            body: Some("{}".into()),
        })
    });
    let mut read = TrackedAllowanceRead::new(&pairs[1..3], TEST_POLICY);
    assert!(read_all(&mut read, &failing).is_none());
    assert!(read.finish().is_err());
    read.restart();
    assert_eq!(read_all(&mut read, &chain).unwrap().len(), 2);
    // Nothing to read finishes at once.
    assert_eq!(
        TrackedAllowanceRead::new(&[], TEST_POLICY)
            .finish()
            .unwrap(),
        []
    );
}

#[test]
fn tracked_pairs_put_lower_bounds_first() {
    use super::record::AllowanceRecord;
    use crate::NetworkId;
    let record = |spender: &str, account: &str, lower_bound: bool| AllowanceRecord {
        id: format!("{account}:{TOKEN}:{spender}"),
        account_id: account.into(),
        network: NetworkId::Mainnet,
        token: canonical(TOKEN),
        spender: spender.into(),
        amount: U256::from_u128(1),
        lower_bound,
        revoking: false,
        revoke_hash: None,
    };
    let records = [
        record("0x1", "SN_MAIN:0xa", false),
        record("0x2", "SN_MAIN:0xa", true),
        record("0x3", "SN_MAIN:0xb", true),
        record("0x4", "SN_MAIN:0xa", false),
    ];
    let owner_of = |account: &str| (account == "SN_MAIN:0xa").then(|| "0xa".to_string());
    let spenders: Vec<String> = tracked_pairs(&records, owner_of)
        .into_iter()
        .map(|p| p.spender)
        .collect();
    assert_eq!(spenders, ["0x2", "0x1", "0x4"]);
}
