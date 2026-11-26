//! Bramble's STRK20 invoke-phase protocol over `wallet_core::privacy` (#182, S6): the grammar of
//! an invoke action's calldata placeholders, the origin-bound shadow-account scope a site's label
//! becomes, and the numbers its shadow-account scan runs under. The core takes all three as plain
//! values (`InvokeArg`, a dapp-name felt, `ShadowScanLimits`). Not exported yet: the P15 flows
//! and P16's dapp methods will.

use starknet_wallet_core::container::domain_hash;
use starknet_wallet_core::privacy::proof::InvokeArg;
use starknet_wallet_core::privacy::shadow_reads::ShadowScanLimits;
use starknet_wallet_core::{encode_short_string, felt, Felt};

/// The domain the scope digest is taken under (`SCOPE_DOMAIN` in the extension's
/// `strk20-shadow-account.ts`).
const SCOPE_DOMAIN: &str = "bramble/strk20-shadow-account/v1";

/// The scope's length in hex characters: the digest's first 15 bytes, a Cairo short string.
const SCOPE_HEX_CHARS: usize = 30;

/// `STRK20_SHADOW_VIEW_NONCE_LIMIT` (`2^64 - 1`, the view's `end_nonce` is an exclusive `u64`)
/// and the anonymizer's `MAX_SCAN_RANGE` (1024 nonces per view call).
pub const SHADOW_SCAN_LIMITS: ShadowScanLimits = ShadowScanLimits {
    nonce_limit: u64::MAX,
    max_scan_range: 1024,
};

/// `strk20ShadowAccountScope(origin, label)`: the first 30 hex characters of
/// `SHA-256("bramble/strk20-shadow-account/v1|{origin}|{label}")`. `origin` must be the verified
/// request origin, never one read from the request: it is what keeps one site's shadow accounts
/// unreachable from another's.
pub fn shadow_account_scope(origin: &str, label: &str) -> String {
    let mut scope = domain_hash(format!("{SCOPE_DOMAIN}|{origin}|{label}").as_bytes());
    scope.truncate(SCOPE_HEX_CHARS);
    scope
}

/// The dapp name the SDK derives the shadow account from for a site's `label`
/// (`bindStrk20ActionsToOrigin`, then `encodeDappName`): the scope, as a Cairo short string.
pub fn shadow_dapp_name(origin: &str, label: &str) -> Option<Felt> {
    encode_short_string(&shadow_account_scope(origin, label))
}

/// One calldata item of an `invoke` action (`expandInvokeCalldata`): `${poolAddress}`,
/// `${openNoteIds[N]}` (decimal `N`, from 0), or a hex felt. `None` for anything else. An index
/// past `usize` names no note the batch can create, so it is kept as one the compiler refuses.
pub fn invoke_calldata_item(item: &str) -> Option<InvokeArg> {
    if item == "${poolAddress}" {
        return Some(InvokeArg::PoolAddress);
    }
    if let Some(digits) = item
        .strip_prefix("${openNoteIds[")
        .and_then(|rest| rest.strip_suffix("]}"))
    {
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        return Some(InvokeArg::OpenNoteId(digits.parse().unwrap_or(usize::MAX)));
    }
    felt::parse(item).map(InvokeArg::Felt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_follow_the_extensions_pattern() {
        assert_eq!(
            invoke_calldata_item("${poolAddress}"),
            Some(InvokeArg::PoolAddress)
        );
        assert_eq!(
            invoke_calldata_item("${openNoteIds[0]}"),
            Some(InvokeArg::OpenNoteId(0))
        );
        assert_eq!(
            invoke_calldata_item("${openNoteIds[012]}"),
            Some(InvokeArg::OpenNoteId(12))
        );
        assert_eq!(
            invoke_calldata_item("${openNoteIds[99999999999999999999999]}"),
            Some(InvokeArg::OpenNoteId(usize::MAX))
        );
        assert_eq!(
            invoke_calldata_item("0x5"),
            Some(InvokeArg::Felt(Felt::from(5u8)))
        );
        for refused in [
            "${openNoteIds[]}",
            "${openNoteIds[-1]}",
            "${openNoteIds[1] }",
            "${openNoteIds[0x1]}",
            "${PoolAddress}",
            "$poolAddress",
            "5",
            "",
        ] {
            assert_eq!(invoke_calldata_item(refused), None, "{refused}");
        }
    }
}
