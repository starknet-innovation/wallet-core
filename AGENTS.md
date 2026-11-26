# Agent guide — wallet-core

Starknet wallet logic in Rust, shared by Bramble's browser extension (wasm32, npm
`@starknet-innovation/bramble-core`) and its iOS and Android apps (UniFFI). It holds key custody and
signs, so treat every change as security-sensitive. `docs/PLAN.md` owns what goes where and why;
this file is the operational guide.

## Start here

- `bash tools/check.sh quick` before editing, to see any existing drift.
- Read `docs/PLAN.md` and the affected crate's `Cargo.toml` and `src/lib.rs` before changing it.
- Enable the hooks once per clone: `git config core.hooksPath .githooks` (pre-commit: fmt +
  guardrails; pre-push: clippy + default-feature tests, skipped when a push touches no Rust; CI
  adds mobile's test targets). bramble-core's integration tests are one binary
  (`tests/integration.rs`): add a file there as `mod <file>;`, and run it with
  `cargo test -p bramble-core --test integration <file>::`.

## Commands

`tools/check.sh` is the only command surface; CI calls the same modes. Don't copy its cargo
sequences into workflows or docs.

```bash
bash tools/check.sh quick        # fmt + guardrails
bash tools/check.sh rust         # fmt + clippy (default, mobile, wasm32) + tests
bash tools/check.sh all          # everything CI runs; finish with this
cargo test -p bramble-core <name>   # narrow loop while iterating
```

- `wasm` mode needs `cargo install wasm-bindgen-cli --version 0.2.130 --locked`; the version must
  equal `wasm-bindgen` in `Cargo.lock` (a guardrail checks every place it is written).
- The toolchain is pinned in `rust-toolchain.toml` and repeated as `RUST_TOOLCHAIN` in each
  workflow; change them together (also checked).
- `deny` needs network for the advisory DB. Offline, run the rest and say so.
- A fresh worktree starts with an empty `target/`. To reuse the main checkout's build, set
  `CARGO_TARGET_DIR=<main checkout>/target` for checks and pushes. Cargo locks the shared
  directory, so two sessions building at once wait on each other.
- `secrets` needs `gitleaks` (CI pins the version and sha256 in `ci.yml`). `.gitleaks.toml`
  exempts only the broad `generic-api-key` rule, only under `tests/fixtures/`; never widen it
  to `src/`, and never add a key there that is not derived from the published parity phrase.

## Layout

- `crates/wallet-core` (package `starknet-wallet-core`) — generic custody (Krusty KMS), Nostr
  coordination, and as the plan lands, vault, transactions and portfolio. Plain Rust: no FFI, no
  UniFFI, no Bramble policy.
- `crates/bramble-core` — Bramble's choices on top, exported through UniFFI. The only FFI crate.
- `crates/bramble-core/tests/fixtures` — golden fixtures written by the extension (see its
  README).
- `js/` — the npm package: ubrn-generated wasm bindings plus a thin TypeScript entry.

## Hard rules

- **Krusty KMS owns wallet crypto.** Never add fallback signing, key derivation or mnemonic
  validation; if Krusty can't do it, fail closed. `krusty-kms` stays pinned with `=` and is bumped
  deliberately, never in passing.
- **`starknet-types-core` follows Krusty** (`=0.2.4` today). Bump it only together with
  `krusty-kms`; `cargo tree -i starknet-types-core` must show one version.
- **Every dependency is pinned exactly** (`=x.y.z`), justified in the PR, and used immediately.
  Prefer none. Don't bump one unless a feature needs it or an advisory forces it; Dependabot
  opens no version-update PRs, and advisories reach us through security updates and
  `cargo deny`. Hashes are pinned by `Cargo.lock` (built `--locked`), `js/package-lock.json`
  (`npm ci`, install scripts off in `js/.npmrc`) and action commit SHAs.
- **No secret leaves through a side door.** Private keys, mnemonics, passphrases and viewing keys
  never reach logs, error messages, `Debug`/`Display` output or UniFFI error strings. Hold them in
  `Zeroizing` where the API allows. Library code has no print macros (checked).
- **The mobile build returns no raw key and takes exactly one.** Nothing on its surface returns a
  private key, a phrase-derived key, or anything a key can be recovered from. The one inbound
  exception is `WalletCore::import_private_key` (P3), which seals the pasted key into the
  unlocked vault and returns only the account's public record. Anything else that takes or
  returns a raw private key goes behind the `raw-keys` feature; `--no-default-features --features
  mobile` must stay clean (clippy checks it, and `tests/ffi_surface.rs` fails if any other mobile
  export takes a key or any returns one).
- **Fixtures are evidence, not inputs.** Never edit a file under `tests/fixtures/` to make a test
  pass. Regenerate it in Bramble, copy it unchanged, and update its sha256 row (and the Bramble
  commit line) in the fixtures README.
- **`wallet-core` takes no product policy.** If a rule is Bramble's choice, it lives in
  `bramble-core`.
- **Imports never loop across `wallet-core`'s top-level modules** (`send`, `history`, ...), so
  each can move into a crate of its own; `crates/wallet-core/tests/module_layers.rs` checks it.
- `#![forbid(unsafe_code)]` stays on both crates, and so does the deny on `unwrap`/`expect`:
  library code returns errors, it does not panic. Fix a lint rather than allow it; the one
  accepted `allow` is `too_many_arguments` on flat UniFFI exports. A panic on input the crate
  itself controls (a literal, a bundled catalog, a record validated on open) may take
  `#[expect(clippy::expect_used, reason = "...")]` naming why it cannot fail; never on input.
- Source files stay under 500 lines (checked); split by behaviour as you approach it.

## Reuse before writing

Bramble's worst consistency bugs came from one agent re-deriving what another had already written.
Grep first; extend in place rather than adding a sibling.

- Felt parsing, canonical hex and the private-key range live in `crates/wallet-core/src/felt.rs`.
- Vault crypto (envelope v3, DEK container, AES-GCM and randomness helpers) lives in
  `crates/wallet-core/src/envelope.rs` and `container.rs`; host I/O goes through `host.rs`.
- The v13 vault's records as written are `bramble-core`'s `vault_wire.rs`, the extension's
  validator over them `vault_rules.rs` (`is_account_id`, `is_wallet_origin`, `chain_id`), its JSON
  budget `vault_budget.rs`; what mobile does not operate is `VaultPayload::kept`, never `accounts`.
  A v14 vault's `accountProvenance` (`Origin`, `Provenance`, `note_origin`) is
  `vault_provenance.rs`; every account mobile adds goes through `account_store::push_active`,
  which records its origin; the schema a new vault is written at is
  `vault_v13::NEW_VAULT_SCHEMA`.
- JSON-RPC bodies and reply judging live in `rpc.rs` and `send/{rpc_bodies,judge}.rs`; v3
  transaction hashes in `send/sign.rs`; fee bounds in `send/fees.rs`; u256 in `send/u256.rs`.
- Starknet ID names (name → address: grammar, `useEncoded`, the `domain_to_address` body and its
  judge) are `stark_name`; Bramble's naming contracts and exports are `bramble-core`'s
  `stark_name.rs`.
- JavaScript semantics live in `crates/wallet-core/src/js/`: `trim`, UTF-16 lengths and
  `Number` formatting in `js::num` (`js_trim`; `send::calls` re-exports it), `BigInt` in
  `js::big`, `JSON` in `js::json`, the budgeted regex matcher in `js::regex`, felt helpers over
  them in `js::felt`. Fixed-UTC dates are `history::time`.
- Every address or hash shown is spelled by `js::felt::address_label` (`0x0208…98d9`, cut from
  the 64-digit padded spelling) or `address_text` (in full, padded): Bramble's `addressLabel` and
  `addressText`. A non-felt passes through unchanged. Never cut an address anywhere else.
- Ladders, account families, family addresses and constructor calldata live in
  `crates/wallet-core/src/accounts/`; an account's key is `kms::account_key(.., ladder, index)`.
- The recovery scan is `accounts::scan::Scan`, fed a `ScanConfig` (Bramble's shapes, limits,
  tokens, pool and `Authorization`s); its signer and privacy-pool reads and verdicts are
  `accounts::advisory`, whose own viewing public key is `own_viewing_public_key`. A host's batch
  of bodies and replies is `rpc::{Request, Response}`; selectors sit in `send::plan_probe`.
- Argent multisig accounts are `accounts::multisig` (GUIDs, normalised config, calldata, address,
  over a caller's `ArgentMultisigClass`), `accounts::multisig_records` (the extension's
  `ArgentMultisigRecord` and its validators), `accounts::multisig_read` (the pinned-block
  authority read) and `accounts::multisig_declared` (the class-declared read). `lenient_hex_felt`
  (hex in any case with trailing JavaScript whitespace) is `js::felt`.
- Argent multisig transactions are `send::multisig_deploy` (deployment, through `Plan`),
  `send::multisig_invoke` (calls, execute calldata, the frozen `PreparedInvoke`, the threshold
  signature array), `send::multisig_invoke_plan` (`ProposePlan`, `SubmitPlan`) and
  `send::multisig_intent` (the signing intent, package, merge, validator). Selectors of
  arbitrary entrypoints (`kms::selector`) and Stark signature verification
  (`kms::stark_signature_is_valid`) come from Krusty through `kms.rs`, never a local copy.
  Importing a co-signer's package (chain authority and nonce, the wallet's own review, merge,
  store checks) is `send::multisig_import::import_intent`.
- Signer changes are `accounts::multisig_change` (change, self-call, parser, next config,
  review); a multisig batch's safety rule and review are `send::multisig_review`; the
  extension's call summaries (`decodeCall`) and recipient positions are `send::call_decode`, over
  the pinned AVNU route layout in `send::avnu_route`.
- The send state machine is `send::plan::Plan`; a wallet's session, keys and activity log reach
  it only through `PlanHost`, and its numbers through `Policy` (Bramble's: `SEND_POLICY`). A
  single-signer call batch is `send::invoke::Invoke` (`Kind::Invoke`), its checks
  `invoke::check`; every `__execute__` calldata is `calls::execute_calldata`, and the
  ordinary-call review both batches share is `multisig_review::summarise_calls`. A batch's
  annotation flags (`verifyCalls`) are `send::review_flags` (`RiskFlag`, `ReviewContext`), over
  `call_decode::decode_call`'s `CallKind`; the full review is `Invoke::reviewed`, the only
  constructor, over `invoke::verify_calls` (the unrefused per-call review a stored multisig
  intent's card reads too, `WalletCore::multisig_intent_review`) and `invoke::batch_flags`; the plan's card reads its flags through `Plan::risk_flags()`, and a batch's
  activity text is `Invoke::with_activity_summary` or `with_dapp_origin`. A batch's allowance
  changes and its row's amount and token (`ActivityAsset`) reach the host in the plan's pending
  record (`PendingKind::Invoke`). The approve spellings are `call_decode::{APPROVE,
  APPROVE_SET, DECREASE_ALLOWANCE}` and the approve call is `send::calls::erc20_approve`.
- The Ledger device protocol is `crates/wallet-core/src/ledger/` (APDUs and their replies,
  `LedgerRejection`, `LedgerPath`); one signature, verified against the account's key before it
  can be used, is `ledger::DeviceSigning`. A plan signs a Ledger account through it
  (`Plan::with_device`, `send::plan_device`), never through `PlanHost::sign`. Bramble's minimums
  and path schemes are `bramble-core`'s `ledger.rs` (`LedgerScheme`); a vault's Ledger record is
  `KeySource::Ledger`, admitted when its stored key places it, and reaches a plan only through
  `ledger::record_device` (`signing_key` refuses it). Creating a Ledger-only wallet and adding a
  Ledger account are `bramble-core`'s `ledger_wallet.rs`; a vault's phrase is read only through
  `VaultPayload::phrase`, which refuses a wallet without one. A page of public keys for the
  account picker is `ledger::KeyEnumeration`, exported as `ledger_reader::LedgerKeyReader`
  beside `ledger_account_candidates`. Ledger's Bluetooth framing is `ledger::ble`, exported as
  `ledger_ble::LedgerBleExchange` beside `ledger_ble_link` (GATT specs, MTU request); it only carries bytes, and
  the apps' Bluetooth shims hold no protocol logic of their own.
- AVNU swaps are `crates/wallet-core/src/swap/`: the quote (`quote.rs`: `PendingQuote`,
  `SwapQuote`, the value check), the build and its reply judge (`build.rs`: `PendingBuild`), the
  build policy and floor (`policy.rs`: `build_policy`, `reviewed_min_buy_amount`) and the frozen
  preparation (`prepare.rs`: `SwapPreparation`); Bramble's deployments and
  numbers are `bramble-core`'s `swap::SWAP_POLICY`.
- Native staking lives in `crates/wallet-core/src/staking/`: the pinned chain read
  (`read::StakingRead`, decoders and `verify_staking_pool` in `decode`), Starkscan's directory
  and positions (`directory`, `positions`, the shared `coverage` rule), the rows (`view`), the
  calls and sentences (`calls`, `review`), and the prepared action (`prepare`, built through
  `send::invoke::Invoke::reviewed`). Bramble's numbers are `bramble-core`'s
  `staking::STAKING_POLICY`.
- Network and token records (`NetworkInfo`, `TokenInfo`, `TokenRegistry`, borrowed together as
  `Catalogs`) live in `crates/wallet-core/src/catalog.rs`; the core bundles none, the wallet
  passes its own in. A network's catalog spelling (`SN_MAIN`) is `NetworkId::catalog_id`. A
  network with the person's RPC override first is `bramble-core`'s
  `networks::with_rpc_override`; the stored override is `rpc_override.rs`.
- Bramble's catalogs live in `crates/bramble-core/src/catalogs/`, embedded from
  `crates/bramble-core/generated/`: `networks` (`all`, `info`, `registry`, `catalogs()`, and
  `LEGITIMATE_ALIASES`), `accounts` (the class table, family names, `send_authorization`,
  `RecoveryLimits`, `ARGENT_SIGNER_ARRAY`), `account_address` (`resolve_class`, `argent_shape`)
  and `multisig` (`argent_multisig_class`, the limits). Tests load them from there rather than
  rebuilding records from fixture JSON. Bramble's portfolio and history choices are
  `PORTFOLIO_POLICY` and `HISTORY_POLICY` in `bramble-core`'s `lib.rs` (the records are
  `portfolio::PortfolioPolicy` and `history::HistoryPolicy`).
- Portfolio lives in `crates/wallet-core/src/portfolio/`: the price book (`prices::PriceBook`,
  its cache in `price_cache.rs`), saved balances (`snapshots::BalanceSnapshots`), tracked tokens
  (`tokens::TokenPreferences`), token identity, registry and spam (`registry.rs`), on-chain token
  reads (`token_meta.rs`), fiat and amount display (`fiat.rs`), the HTTP and portfolio RPC wire
  contract (`wire.rs`). Account ids and explorer links are `history::explorer`
  (`activity_account_id`, `canonical_account_id`, `explorer_transaction_url`, ...).
- ERC-20 allowances live in `crates/wallet-core/src/allowances/`: the record, its id, scope and
  load rules and the unlimited cut-off (`record.rs`), eviction ranking (`retention.rs`), the log
  over `host::Storage` with its deltas and revoke settling (`store::AllowanceLog`), the
  `Approval` decoder, merge and listing rules (`discovery.rs`), the external scan
  (`scan::ExternalAllowanceScan`) and the tracked reads and their `allowance` call
  (`tracked.rs`). Allowance writes and the revoke are `allowances::calls` (`AllowanceOwner`,
  `record_submitted`, `record_changes`, `approved_asset_kind`, `standing_allowance_totals`) and
  `allowances::revoke` (`RevokeTarget`, `revoke_outcomes`); a submitted batch's approvals reach
  the log only through `record_changes`, from the plan's pending record, or `record_submitted`
  for calls no plan carried. Bramble's numbers are `ALLOWANCE_POLICY` in
  `bramble-core`'s `lib.rs`.
- Activity lives in `crates/wallet-core/src/history/` and `activity/`: the row type
  (`history::activity::ActivityEntry`, `ActivityKind`), the Starkscan lane (`starkscan.rs`), the
  RPC history scan (`events::HistoryScan`), the read cadence (`activity_book::ActivityBook`), row
  presentation (`appearance.rs`, `classify.rs`; a row's tracked token is
  `appearance::activity_tracked_token`), the feed's filter, amount sort and stale-axis reset
  (`history::filter`) and its per-counterparty summary (`history::party`); the one log is
  `activity::ActivityLog` over `host::Storage` (`monotonic` in `activity::store`, the kind and
  status text codecs in `history::activity_rules`, the observation windows in `activity::observation`, the receipt fee and
  value at submission an expanded row shows in `activity::detail`, the CSV export in
  `activity::csv`; a receipt's fee is `send::judge::receipt_fee`). The UTC date-time the export
  prints is `history::time::format_utc_date_time`; JavaScript string order is
  `js::num::utf16_cmp`, `bigint` addition `js::big::Big::plus`.
- Privacy reads live in `crates/wallet-core/src/privacy/`: the read plan
  (`plan::PrivacyReadPlan`, whose viewing key reaches it only through `PrivacyHost`, each reply in
  `plan_steps.rs`), the discovery and pool-fee bodies (`bodies.rs`, the only writer of the key),
  reply judging (`judge.rs`, `notes.rs`), the order-keeping JSON the cursor echo needs
  (`privacy::json`, stricter than `js::json`), routing and limits records (`config.rs`), and the
  gates (`gates.rs`, `home.rs`). `DeploymentState` is `send::types`. STRK20 proofs (P15) go
  through the wallet's engine (`engine::PrivacyEngine`, its records, `engine::prove`); the
  `CallSet` digest and its signature check are `call_set.rs`, the `compile_actions` decoder and
  its binding to the reviewed actions `pool_actions.rs`, and the engine's results are judged in
  `engine_judge.rs`.
- Nostr relay work lives beside the keys in `crates/wallet-core/src/nostr/`: the relay-URL
  grammar (`relay_url::is_nostr_relay_url`, which a custom RPC URL shares as
  `is_custom_rpc_url`; the endpoint redactor `redact_endpoints` sits beside it), relay selection, frame parsing, gift-wrap batches and
  publishing (`relay.rs`: `valid_relay_urls`, `parsed_gift_wraps`, `PublishJudge`,
  `publish_many_nostr_gift_wraps`), the paged sync (`sync::SyncJudge`) and the sync window
  (`sync_window.rs`). Bramble's numbers for them (`RELAY_LIMITS`, `SYNC_LOOKBACKS`) and its
  recipient and sender rules are `bramble-core`'s `coordination.rs`.
- A dapp's requests (P16) live in `crates/wallet-core/src/dapp/`:
  - the hostile-JSON budget over the text a bridge passes (`budget::parse_within`);
  - the wallet API's error codes (`WalletRpcError`);
  - the method table and each method's param reader (`request::{Method, read_request}`), an
    invoke's calls read into `send::calls::Call` by `invoke` under `InvokeLimits`, a chain by
    `add_chain` (URLs through `relay_url::is_custom_rpc_url`), an asset by `watch_asset`.
  - which accounts a site may see (`grants`: `OriginGrant`, the vault's record, and
    `granted_account`, `with_origin_grant`, `without_origin_access`, `connected_sites`).
  - the approval queue (`queue::ApprovalQueue`: caps, eviction, TTL over the host's monotonic
    clock, dedupe and waiters, reservations, fee revisions; dropped ids via `take_dropped`).
  - SNIP-12 typed data (`typed_data`): the gate (`CheckedTypedData::check`, the name grammar
    `is_safe_snip12_name`, revisions, merkle and enum rules, value conformance in
    `typed_data_values`, leaf readings in `typed_data_scalars`), the chain binding
    (`message_hash_for_chain`, the only hash a dapp request gets, through Krusty), the card
    line (`describe_typed_data`) and the card's decoded rows (`sign_rows`:
    `CheckedTypedData::sign_rows`, the extension's `signValueText`, over
    `typed_data_scalars::snip12_reading`).
  - a SNIP-9 outside execution in typed data (`outside_execution::review_outside_execution`,
    at `Stage::Queue` and again at `Stage::Sign`): its calls read with the hash's encoder,
    then the invoke route's review (`send::invoke::verify_calls`,
    `multisig_review::is_poisoned`), never a second one; a queued card's lookalike payee is
    `RiskFlag::AddressPoisoning`.
  - connecting a site and the passive reads around it (`connect`: `Site`, `answer`,
    `request_accounts`, `approve_connect`, the result shapes `addresses` and `accounts_view`),
    in the extension's gate order: blocklist, lock, grant.
  - whether a site is blocked and its warnings (`blocklist`: `origin_host`, `is_blocked_origin`,
    `origin_insights` over `SiteLists`), the live feed (`phishing_feed::PhishingFeed`: request,
    judge, cache, health) and when a site was first seen (`origin_history::OriginHistory`).
  Bramble's budgets, advertised versions, `mc_*` methods, site lists and feed are
  `bramble-core`'s `dapp.rs` (`DAPP_POLICY`, `DAPP_QUEUE_LIMITS`, `read_page_request`,
  `SITE_LISTS`, `PHISHING_FEED`).
  The apps reach all of it through `bramble-core`'s `dapp_session::DappSession` (the request,
  the cards (`DappCard`), their decisions and expiry; grants are written to the vault's
  `kept.origin_grants` through `WalletCore::commit`); its blocklist, feed refresh, card
  warnings and navigation check (`is_blocked_site`) are `dapp_sites.rs`; cards that end
  undecided (`settle_expired`, and `withdraw` for a page that has gone) are `dapp_expiry.rs`;
  `wallet_signTypedData`'s card and its signature at approval
  (`approve_sign_typed_data`, through `signing_record` and `sign_for` under one session hold)
  are `dapp_sign.rs`, which reviews an outside execution's calls with the invoke route's
  inputs (`trusted_recipients`, the tracked tokens from the session's `TokenPreferences`, the
  allowance log), read before the session hold.
- Only `wallet-core` depends on Krusty (`kms.rs`, `nostr.rs`, `accounts/`, `send/`);
  `bramble-core` goes through `wallet-core` and never names `krusty_kms`.

## Commits and PRs

- Imperative summary, optional scope (`ci:`, `kms:`), body says why. One concept per commit.
- Fill in the PR template. A behaviour change comes with a test; a parity change with fixtures.

## Releases

- Both crates share the workspace version. The tag `v<x.y.z>` must equal it and
  `js/package.json`; `release.yml` (npm) and `publish-crates.yml` (crates.io) refuse otherwise.
- While `js/package.json` is `private` (until Phase 3.1), `release.yml` makes a GitHub release
  only; the crates still go to crates.io from the same tag, and mobile pins `wallet-core` by that
  tag.
- npm and crates.io publish through OIDC trusted publishing only. Never add a registry token or
  publish locally. crates.io publishing also waits on a reviewer in the `crates-io` environment,
  which admins cannot bypass.
- The one exception: crates.io attaches trusted publishing only to a crate that exists, so a new
  crate name's first version is published once by a maintainer, with a token scoped to that
  crate and revoked straight after, from a clean checkout of the commit that will be tagged
  (`publish-crate.sh` fails the release unless the version on crates.io is that commit's
  package). Then add the trusted publisher (this repo, `publish-crates.yml`, environment
  `crates-io`) before the next tag.
- A version on crates.io can be yanked but never replaced or deleted: bump, don't retag.
- Only a maintainer tags, and only a merged `main` commit. Never move or recreate a tag.
