//! The ERC-20 transfer call and the rules the extension applies to it before anything is
//! estimated: recipient grammar (`normalizeFeltInput` + `isStarkFieldElement`), amount grammar
//! (`parseTokenAmount`), `buildErc20TransferCall`, the Cairo-1 `__execute__` calldata, and the
//! verifier's summary and flags (`verifyCalls`). Pinned by `extension-transactions-*.json`.

use starknet_types_core::felt::Felt;

use super::u256::U256;
use crate::felt;
use crate::js::felt::address_label;

/// A reviewable risk the verifier raised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewFlag {
    SelfTransfer,
    /// The recipient is a token contract the wallet knows. A warning: it does not block Confirm.
    TokenContractRecipient,
    AddressPoisoning,
}

/// Why an amount was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendAmountError {
    /// "Invalid transfer amount"
    Invalid,
    /// "Too many decimal places"
    TooManyDecimals,
    /// "Transfer amount must be greater than zero"
    NotPositive,
    /// At or above 2^256: no ERC-20 can carry it.
    TooLarge,
}

/// JavaScript `trim`, re-exported from `js` so existing callers keep their path.
pub use crate::js::num::js_trim;

/// `starknet_keccak("transfer")`, checked against `kms::selector` in the tests and
/// against starknet.js `getSelectorFromName` by the fixture.
pub const TRANSFER_SELECTOR: &str =
    "0x83afd3f4caedc6eebf44246fe54e38c95e3179a5ec9ea81740eca5b482d12e";
/// The extension's `isSafeDecimals` ceiling.
const MAX_DECIMALS: u8 = 36;

/// `normalizeFeltInput` then `isStarkFieldElement`: trims; lowercases and strips leading zeros
/// when the text starts with `0x`; then applies the lowercase-hex felt grammar. Returns the
/// canonical felt (including `0x0`, which the extension's field check also accepts; the call
/// builder refuses it) or `None`.
pub fn validate_recipient(input: &str) -> Option<Felt> {
    let trimmed = js_trim(input);
    let normalised = match trimmed.strip_prefix("0x") {
        Some(digits) if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_hexdigit()) => {
            let lower = digits.to_ascii_lowercase();
            let significant = lower.trim_start_matches('0');
            format!(
                "0x{}",
                if significant.is_empty() {
                    "0"
                } else {
                    significant
                }
            )
        }
        _ => trimmed.to_string(),
    };
    felt::parse(&normalised)
}

/// `parseTokenAmount`: base units from decimal text. The extension has no upper bound here and
/// fails later in `cairo.uint256`; the core refuses 2^256 and above at this point instead.
pub fn parse_token_amount(amount: &str, decimals: u8) -> Result<U256, SendAmountError> {
    let trimmed = js_trim(amount);
    let (whole, fraction) = match trimmed.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (trimmed, None),
    };
    let whole_ok = whole == "0"
        || (!whole.is_empty()
            && !whole.starts_with('0')
            && whole.bytes().all(|b| b.is_ascii_digit()));
    let fraction_ok =
        fraction.is_none_or(|f| !f.is_empty() && f.bytes().all(|b| b.is_ascii_digit()));
    if decimals > MAX_DECIMALS || !whole_ok || !fraction_ok {
        return Err(SendAmountError::Invalid);
    }
    let fraction = fraction.unwrap_or("");
    if fraction.len() > usize::from(decimals) {
        return Err(SendAmountError::TooManyDecimals);
    }
    let digits = format!("{whole}{fraction:0<width$}", width = usize::from(decimals));
    let significant = digits.trim_start_matches('0');
    if significant.is_empty() {
        return Err(SendAmountError::NotPositive);
    }
    U256::from_decimal(significant).ok_or(SendAmountError::TooLarge)
}

/// `formatTokenAmount`: full precision, trailing fractional zeros removed.
pub fn format_token_amount(raw: &U256, decimals: u8) -> String {
    crate::rpc::format_amount(&raw.to_decimal(), decimals, decimals)
        .unwrap_or_else(|_| raw.to_decimal())
}

fn u128_felt(value: u128) -> Felt {
    Felt::from(value)
}

/// One Cairo 1 call (starknet.js `Call`), its felts already parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    pub contract_address: Felt,
    pub entrypoint: String,
    pub calldata: Vec<Felt>,
}

/// `buildErc20ApproveCall(token, spender, amount)`: `approve(spender, amount)`, the uint256 as
/// its two limbs. A zero amount is the revoke.
pub fn erc20_approve(token: Felt, spender: Felt, amount: U256) -> Call {
    Call {
        contract_address: token,
        entrypoint: super::call_decode::APPROVE_SET.into(),
        calldata: vec![spender, Felt::from(amount.low), Felt::from(amount.high)],
    }
}

/// `transaction.getExecuteCalldata(calls, "1")`: the account `__execute__` calldata, the count
/// then each call's contract, selector, calldata length and calldata. Every family this wallet
/// signs for takes this one encoding (the extension builds every account with `cairoVersion:
/// "1"`, `krustyAccount` in `rpc/account-signer.ts`). Selectors are Krusty's (`kms::selector`);
/// the entrypoint grammar is the caller's check.
pub fn execute_calldata(calls: &[Call]) -> Vec<Felt> {
    let mut out = vec![Felt::from(calls.len() as u64)];
    for call in calls {
        out.extend(call_head(
            &call.contract_address,
            &call.entrypoint,
            call.calldata.len(),
        ));
        out.extend_from_slice(&call.calldata);
    }
    out
}

/// What [`execute_calldata`] writes ahead of one call's calldata: the contract, the selector and
/// the calldata's length. For a caller that lays the calldata into its own buffer (the
/// key-bearing `compile_actions` in `privacy::invocation`) rather than copying it into a `Call`.
pub(crate) fn call_head(contract: &Felt, entrypoint: &str, calldata_len: usize) -> [Felt; 3] {
    [
        *contract,
        crate::kms::selector(entrypoint),
        Felt::from(calldata_len as u64),
    ]
}

/// `transaction.getExecuteCalldata([transfer(recipient, amount)], "1")`: the Cairo-1 account
/// `__execute__` calldata for one ERC-20 transfer. `None` for a zero token or recipient, as
/// `buildErc20TransferCall` throws "Invalid transfer address" for either.
pub fn transfer_execute_calldata(
    token: &Felt,
    recipient: &Felt,
    amount: &U256,
) -> Option<Vec<Felt>> {
    if *token == Felt::ZERO || *recipient == Felt::ZERO {
        return None;
    }
    let call = Call {
        contract_address: *token,
        entrypoint: "transfer".into(),
        calldata: vec![*recipient, u128_felt(amount.low), u128_felt(amount.high)],
    };
    Some(execute_calldata(std::slice::from_ref(&call)))
}

/// The verifier's transfer summary: `Send <formatTokenAmount> <SYMBOL> to <addressLabel>`.
pub fn transfer_summary(amount: &U256, decimals: u8, symbol: &str, recipient: &Felt) -> String {
    format!(
        "Send {} {symbol} to {}",
        format_token_amount(amount, decimals),
        address_label(&felt::canonical(recipient))
    )
}

/// `displayPrefixes`: the four digits after `0x` that each zero-padded spelling of a canonical
/// felt shows, from the canonical form up to the 64-digit padded one (at most five windows).
fn display_prefixes(canonical: &str) -> Vec<String> {
    let digits = canonical.strip_prefix("0x").unwrap_or(canonical);
    let max_zeros = 64usize.saturating_sub(digits.len()).min(4);
    (0..=max_zeros)
        .map(|zeros| {
            format!("{}{digits}", "0".repeat(zeros))
                .chars()
                .take(4)
                .collect()
        })
        .collect()
}

/// `isLookalikeAddress`: a different account whose last four digits match `trusted` and whose
/// leading window, in any spelling the wallet can render, matches one of `trusted`'s. Comparing
/// every spelling keeps a padded contact from letting the attacker choose the compared window
/// (extension audit #242).
pub fn is_lookalike_address(candidate: &Felt, trusted: &Felt) -> bool {
    if candidate == trusted {
        return false;
    }
    let (left, right) = (felt::canonical(candidate), felt::canonical(trusted));
    let tail = |value: &str| value[value.len().saturating_sub(4)..].to_string();
    if tail(&left) != tail(&right) {
        return false;
    }
    let trusted_prefixes = display_prefixes(&right);
    display_prefixes(&left)
        .iter()
        .any(|prefix| trusted_prefixes.contains(prefix))
}

/// `verifyCalls` flags for a transfer from `account`, in the extension's order. `trusted` is
/// every other address the person can be expected to recognise on this network (their other
/// accounts and saved contacts); a lookalike of any of them, or of `account`, is refused.
/// `tokens` are the token contracts the wallet knows here: paying one is warned, not refused.
pub fn transfer_flags(
    recipient: &Felt,
    account: &Felt,
    trusted: &[Felt],
    tokens: &[Felt],
) -> Vec<ReviewFlag> {
    let mut flags = Vec::new();
    if recipient == account {
        flags.push(ReviewFlag::SelfTransfer);
    }
    if tokens.contains(recipient) {
        flags.push(ReviewFlag::TokenContractRecipient);
    }
    if std::iter::once(account)
        .chain(trusted)
        .any(|known| is_lookalike_address(recipient, known))
    {
        flags.push(ReviewFlag::AddressPoisoning);
    }
    flags
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selector_is_starknet_keccak() {
        assert_eq!(
            crate::felt::canonical(&crate::kms::selector("transfer")),
            TRANSFER_SELECTOR
        );
    }

    #[test]
    fn zero_addresses_build_no_call() {
        let token = Felt::from(7u8);
        let amount = U256::from_u128(1);
        assert!(transfer_execute_calldata(&token, &Felt::ZERO, &amount).is_none());
        assert!(transfer_execute_calldata(&Felt::ZERO, &token, &amount).is_none());
        assert_eq!(validate_recipient("0x000"), Some(Felt::ZERO));
    }

    #[test]
    fn amounts_follow_the_grammar() {
        assert_eq!(
            parse_token_amount("0001.10", 6),
            Err(SendAmountError::Invalid)
        );
        assert_eq!(parse_token_amount("1.", 6), Err(SendAmountError::Invalid));
        assert_eq!(
            parse_token_amount("0.0", 6),
            Err(SendAmountError::NotPositive)
        );
        assert_eq!(parse_token_amount("1", 37), Err(SendAmountError::Invalid));
        assert_eq!(
            parse_token_amount("\u{feff} 2.5\u{a0}", 1).unwrap(),
            U256::from_u128(25)
        );
    }
}
#[cfg(test)]
mod poisoning_trusted_set {
    use super::*;

    /// A saved contact, stored padded as the address book stores it.
    const CONTACT: &str = "0x0555aa7890abcdef1234567890abcdef1234567890abcdef1234567890aabbcc";

    fn felt(value: &str) -> Felt {
        validate_recipient(value).unwrap()
    }

    #[test]
    fn a_twin_of_any_trusted_address_is_refused() {
        let sender = Felt::from(0x1111u32);
        let contact = felt(CONTACT);
        // Same canonical window (`0x555a…bbcc`) as the contact, different account.
        let twin = felt("0x555af000000000000000000000000000000000000000000000000000000bbcc");
        assert!(transfer_flags(&twin, &sender, &[], &[]).is_empty());
        assert_eq!(
            transfer_flags(&twin, &sender, &[contact], &[]),
            vec![ReviewFlag::AddressPoisoning]
        );
        // The contact itself is not a lookalike of itself.
        assert!(transfer_flags(&contact, &sender, &[contact], &[]).is_empty());
    }

    #[test]
    fn a_padded_spelling_cannot_choose_the_compared_window() {
        // 63 digits: canonically `0x555b…`, but one leading zero renders `0x0555…bbcc`, exactly
        // the padded contact's row. Comparing only canonical windows let this through (#242).
        let twin = felt("0x555b0000000000000000000000000000000000000000000000000000000bbcc");
        assert!(is_lookalike_address(&twin, &felt(CONTACT)));
        assert!(!is_lookalike_address(
            &felt("0x555b0000000000000000000000000000000000000000000000000000000bbcd"),
            &felt(CONTACT)
        ));
    }

    #[test]
    fn a_token_contract_recipient_is_warned_in_the_extension_order() {
        let account = Felt::from(0x1111u32);
        let token = Felt::from(0x2222u32);
        assert_eq!(
            transfer_flags(&token, &account, &[], &[token]),
            vec![ReviewFlag::TokenContractRecipient]
        );
    }
}
