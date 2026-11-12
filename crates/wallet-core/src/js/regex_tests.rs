//! Unit tests for `regex.rs` (kept apart to hold it under 500 lines): JavaScript semantics and
//! the step and depth budgets on hostile input.
use super::*;

#[test]
fn javascript_semantics() {
    let swap = Regex::new(r"\bswap(ped|ping)?\b", false);
    assert!(swap.is_match("i swapped 1 eth"));
    assert!(!swap.is_match("swapp"));
    let group = Regex::new(r"\B(?=(\d{3})+(?!\d))", false);
    assert_eq!(group.replace("1234567", true, |_| ",".into()), "1,234,567");
    let figure = Regex::new(r"(?<![\w.])(\d[\d,]*\.\d+)(?![\w.])", false);
    assert_eq!(
        figure.replace("sent 1.23456 and v1.5", true, |_| "X".into()),
        "sent X and v1.5"
    );
    let verb = Regex::new(r"^(?:send|sent)\b\s*", true);
    assert_eq!(
        verb.replace("SENT 5 STRK", false, |_| String::new()),
        "5 STRK"
    );
    let other = Regex::new(r"\b(?:into|to)\s+(?!this account\b)\S", false);
    assert!(other.is_match("shield 5 strk to 0xabc"));
    assert!(!other.is_match("unshield 5 strk to this account"));
    let pair = Regex::new(
        r"\bswap(?:ped|ping)?\s+~?[\d.,]*\s*([a-z0-9.$-]+)\s+for\s+(?:(?:at\s+least|~)\s*)?[\d.,]*\s*([a-z0-9.$-]+)",
        true,
    );
    let caps = pair.captures("Swap 1 STRK for ~2.5 USDC via AVNU").unwrap();
    assert_eq!(caps[1].as_deref(), Some("STRK"));
    assert_eq!(caps[2].as_deref(), Some("USDC"));
    assert!(!Regex::new("a.b", false).is_match("a\nb"));
}

/// Catastrophic backtracking: `(a+)+b` over forty `a`s explores ~2^40 paths without a
/// budget. The search must stop at `STEP_BUDGET`, report `Exhausted`, and match nothing.
#[test]
fn a_pathological_input_exhausts_the_budget_and_fails_closed() {
    let evil = Regex::new(r"^(a+)+b", false);
    let input: Vec<char> = "a".repeat(40).chars().collect();
    let started = std::time::Instant::now();
    assert_eq!(
        evil.try_find_at(&input, 0).map(|m| m.is_some()),
        Err(Exhausted)
    );
    assert!(evil.find_at(&input, 0).is_none());
    assert!(!evil.is_match(&"a".repeat(40)));
    assert_eq!(
        evil.replace(&"a".repeat(40), true, |_| "X".into()),
        "a".repeat(40)
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "the budget bounds the time"
    );
    // The same pattern still decides ordinary input well inside the budget.
    assert_eq!(
        evil.try_find_at(&"aab".chars().collect::<Vec<_>>(), 0)
            .map(|m| m.is_some()),
        Ok(true)
    );
}

/// A long hostile figure is abandoned at the depth budget instead of overflowing the stack, on
/// a stack no larger than a wallet's threads have (the test runner's own is twice this), and
/// the longest summary a caller accepts (200 characters) is still decided.
#[test]
fn the_depth_budget_fits_a_small_stack_and_a_full_summary() {
    let search = |text: String| {
        std::thread::Builder::new()
            .stack_size(1024 * 1024)
            .spawn(move || {
                let figure = Regex::new(r"(?<![\w.])(\d[\d,]*\.\d+)(?![\w.])", false);
                let chars: Vec<char> = text.chars().collect();
                figure.try_find_at(&chars, 0).map(|m| m.is_some())
            })
            .unwrap()
            .join()
            .unwrap()
    };
    assert_eq!(search("1,".repeat(20_000)), Err(Exhausted));
    assert_eq!(search(format!("{}.5", "1".repeat(198))), Ok(true));
    assert_eq!(search("1".repeat(200)), Ok(false));
}
