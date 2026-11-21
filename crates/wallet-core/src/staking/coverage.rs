//! `staking-coverage.ts`: whether a Starkscan staking snapshot is offered as whole, the one rule
//! the validator directory and "Your stakes" share. The block is untrusted: every field the rule
//! reads is checked, a malformed block is `None` (which refuses the whole answer), and fields it
//! does not read are ignored. Nothing here makes a validator or a position trusted.

use crate::js::json::JsValue;
use crate::js::num::is_safe_integer;

/// Starkscan's interval lists are only read for emptiness, but still bounded so an oversized
/// answer is refused rather than walked (`MAX_COVERAGE_INTERVALS`).
const MAX_COVERAGE_INTERVALS: usize = 1024;

const STATUSES: [&str; 3] = ["catching_up", "prepared", "unavailable"];

/// `StakingCoverage`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coverage {
    /// `prepared`, or `catching_up` finalized-only with no gap, nothing outstanding and at most
    /// the policy's lag behind.
    Usable,
    Incomplete,
}

fn interval_list(value: Option<&JsValue>) -> Option<usize> {
    value?
        .as_array()
        .map(<[JsValue]>::len)
        .filter(|len| *len <= MAX_COVERAGE_INTERVALS)
}

/// `stakingCoverage`, with the policy's `max_lag_blocks` (`MAX_CATCHING_UP_LAG_BLOCKS`).
pub fn staking_coverage(value: Option<&JsValue>, max_lag_blocks: u64) -> Option<Coverage> {
    // `get` on anything but an object is `None`, as `isRecord` refuses it.
    let value = value?;
    let status = value
        .get("status")?
        .as_str()
        .filter(|s| STATUSES.contains(s))?;
    let finalized_only = match value.get("finalizedOnly")? {
        JsValue::Bool(flag) => *flag,
        _ => return None,
    };
    let lag = value
        .get("lagBlocks")?
        .as_f64()
        .filter(|lag| is_safe_integer(*lag) && *lag >= 0.0)?;
    let gaps = interval_list(value.get("gapIntervals"))?;
    let outstanding = interval_list(value.get("outstandingMaterializationIntervals"))?;
    let just_behind = status == "catching_up"
        && finalized_only
        && gaps == 0
        && outstanding == 0
        && lag <= max_lag_blocks as f64;
    Some(if status == "prepared" || just_behind {
        Coverage::Usable
    } else {
        Coverage::Incomplete
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::js::json::parse;

    const LAG: u64 = 100;

    // `CATCHING_UP_COVERAGE` (`staking-directory-fixture.ts`), with `overrides` spliced in as
    // JSON members after it (a later key wins, as `{ ...CATCHING_UP_COVERAGE, ...overrides }`).
    fn catching_up(overrides: &str) -> Option<Coverage> {
        let text = format!(
            r#"{{"activityHistoryTruncated":true,"finalizedOnly":true,"gapIntervals":[],"lagBlocks":28,"outstandingMaterializationIntervals":[],"status":"catching_up"{}{overrides}}}"#,
            if overrides.is_empty() { "" } else { "," }
        );
        staking_coverage(Some(&parse(&text).unwrap()), LAG)
    }

    #[test]
    fn decides_whether_a_list_is_offered_as_whole() {
        let table = [
            (r#""status":"prepared","lagBlocks":0"#, Coverage::Usable),
            (r#""status":"prepared","lagBlocks":10000"#, Coverage::Usable),
            ("", Coverage::Usable),
            (r#""lagBlocks":0"#, Coverage::Usable),
            (r#""lagBlocks":100"#, Coverage::Usable),
            (r#""lagBlocks":101"#, Coverage::Incomplete),
            (r#""gapIntervals":[{"fromBlock":1}]"#, Coverage::Incomplete),
            (
                r#""outstandingMaterializationIntervals":[{}]"#,
                Coverage::Incomplete,
            ),
            (r#""finalizedOnly":false"#, Coverage::Incomplete),
            (r#""status":"unavailable""#, Coverage::Incomplete),
            (
                r#""status":"prepared","finalizedOnly":false,"gapIntervals":[{}]"#,
                Coverage::Usable,
            ),
        ];
        for (overrides, expected) in table {
            assert_eq!(catching_up(overrides), Some(expected), "{overrides}");
        }
    }

    #[test]
    fn ignores_fields_the_rule_does_not_read() {
        assert_eq!(
            catching_up(r#""newField":{"nested":true},"gapIntervals":[]"#),
            Some(Coverage::Usable)
        );
    }

    #[test]
    fn refuses_a_block_it_cannot_read_whatever_the_status() {
        for value in ["null", "[]", r#""prepared""#] {
            assert_eq!(staking_coverage(parse(value).as_ref(), LAG), None);
        }
        assert_eq!(staking_coverage(None, LAG), None);
        for status in [r#""done""#, r#""PREPARED""#, "null"] {
            assert_eq!(catching_up(&format!(r#""status":{status}"#)), None);
        }
        let over = format!("[{}]", vec!["{}"; 1025].join(","));
        for status in ["prepared", "catching_up", "unavailable"] {
            for bad in [
                r#""finalizedOnly":"true""#.to_string(),
                r#""finalizedOnly":1"#.into(),
                r#""finalizedOnly":null"#.into(),
                r#""lagBlocks":null"#.into(),
                r#""lagBlocks":"0""#.into(),
                r#""lagBlocks":-1"#.into(),
                r#""lagBlocks":1.5"#.into(),
                r#""lagBlocks":1e400"#.into(),
                r#""lagBlocks":9007199254740992"#.into(),
                r#""gapIntervals":null"#.into(),
                r#""gapIntervals":{}"#.into(),
                r#""gapIntervals":"""#.into(),
                format!(r#""gapIntervals":{over}"#),
                r#""outstandingMaterializationIntervals":null"#.into(),
                r#""outstandingMaterializationIntervals":{"length":0}"#.into(),
                format!(r#""outstandingMaterializationIntervals":{over}"#),
            ] {
                let overrides = format!(r#""status":"{status}",{bad}"#);
                assert_eq!(catching_up(&overrides), None, "{overrides}");
            }
        }
        // A missing field is as malformed as a wrong one.
        for key in [
            "finalizedOnly",
            "lagBlocks",
            "gapIntervals",
            "outstandingMaterializationIntervals",
        ] {
            let text = r#"{"finalizedOnly":true,"gapIntervals":[],"lagBlocks":0,"outstandingMaterializationIntervals":[],"status":"prepared"}"#;
            let mut value = parse(text).unwrap();
            if let JsValue::Object(entries) = &mut value {
                entries.retain(|(k, _)| k != key);
            }
            assert_eq!(staking_coverage(Some(&value), LAG), None, "{key}");
        }
        // The bound itself is accepted: an answer at the limit is not oversized.
        let at = format!("[{}]", vec!["{}"; 1024].join(","));
        assert_eq!(
            catching_up(&format!(r#""gapIntervals":{at}"#)),
            Some(Coverage::Incomplete)
        );
    }
}
