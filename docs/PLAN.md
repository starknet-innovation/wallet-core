# wallet-core plan

One Rust core for Bramble, consumed by the browser extension as a wasm32 module and by the iOS and
Android apps through UniFFI. It replaces two implementations of the same wallet logic: the
extension's TypeScript (`packages/*` in `starknet-innovation/mc-wallet`) and the mobile Rust crate
(`mobile/core`, `bramble-core`, on `feature/mobile-wallet`). The wallet logic itself is kept
generic, so another Starknet wallet can build on it without inheriting Bramble's choices.

Decisions, 2026-11-09:

- Mobile migrates first, because it is still being built: every module it moves here stops being
  written twice. The extension follows once the core covers what it needs (revised 2026-11-09;
  the first draft had the extension first).
- The extension is the oracle. Its real code paths generate golden fixtures, and every module
  ported here lands with them; nothing is ported on hand-written expectations (see Golden tests).
- Fresh crates here; modules are copied from `mobile/core` as each phase ports them. No history move.
- Two crates, one repo: `wallet-core` (generic) and `bramble-core` (Bramble). See
  [Generic and Bramble](#generic-and-bramble).
- UniFFI for every binding, including JavaScript through `uniffi-bindgen-react-native`'s `wasm2`
  flavour. That tool is pre-production; the Rust surface does not depend on it, so the JS layer can
  move to wasm-bindgen directly (or whatever UniFFI's own JS backend becomes) without touching Rust.
- Krusty KMS is compiled in (`krusty-kms =3.1.0`). The extension drops
  `@starknetfoundation/krusty-kms-wasm`; one Krusty, one pin, checked by `core_info()`.
- `starknet-types-core` stays at `=0.2.4`, the version Krusty 3.1.0 builds on. `Felt` crosses the
  Krusty boundary, so a second copy in the lockfile would be a second, incompatible `Felt`. It moves
  to 1.x only in the same change as a deliberate Krusty bump, never on its own.
- The extension's vault schema (v12) is the target. Mobile's own vault payload migrates into it.
  (Revised 2026-11-13: the target is the extension's current schema, v13, and mobile switches to
  it rather than migrating, since no mobile vault holds funds yet. See [Vault switch](#vault-switch).)
- The extension consumes a published npm package at an exact pin with a digest check, the way it
  pins Krusty today. Extension PRs go to `main`; mobile adopts the crate on `feature/mobile-wallet`.
- Host I/O is one key/value storage trait plus a clock, both host-provided (files on mobile,
  `chrome.storage` in the extension). Sans-IO RPC stays as the mobile core has it.
- The core becomes the source of truth for the network, account-class, token-registry and privacy
  catalogs. Design tokens stay in the extension's `ui-kit`.
- Everything is in scope long term (Ledger, dapp provider and permissions, STRK20 submission, swap,
  gasless, NFTs, allowances) but not in v1.
- Parity before the extension (2026-11-20). Next, mobile and the core reach feature parity with
  the extension wallet; Phase 3, the extension adopting the core, waits for it. What Phase 4
  listed that the extension already ships moves into [Parity](#parity), so the extension later
  swaps onto a core that already does everything it does.

## Status

Every Phase 2 step has two halves: the mechanism lands here, then mobile deletes its copy and
depends on this one. A step is done when both halves are.
Phase 2 is done: as of 2026-11-20 both halves of every step are merged. Every port here is merged
(#39–#50, with #51 and #52) and `0.3.0` carries them. In Bramble, G1–G3 (#918–#920) and
M5–M13 (#921–#929) are merged into `feature/mobile-wallet`, whose `mobile/core` is now a shell
that builds this crate's `bramble-core` at `tag = "v0.3.0"` with `features = ["mobile"]`. M7–M13
merged into each other's stacked branches and reached `feature/mobile-wallet` through M6's merge.
`0.4.0` adds #54 (stack scrub after Krusty and Nostr secret calls), #55 (mobile tests without
default features), V1 (#58, the core half of the [vault switch](#vault-switch)) and K1 (#59,
Krusty 3.1.0). M13 merged before the vault step that 2.7 placed ahead of it, so mobile at
`v0.3.0` still wrote its own vault version 5; M14 (Bramble #980) moved it to `v0.4.0` and the
v13 payload.

| Step | Here | Mobile (`mc-wallet`, `feature/mobile-wallet`) |
| --- | --- | --- |
| 0 Scaffold, 1 Golden suite | done | n/a |
| 2.1 KMS, Nostr, felt | done (#13, #20) | done (Bramble #907, #909) |
| 2.2 Vault | done (#19) | done (Bramble #911); the payload switch to v13 is the [vault switch](#vault-switch) |
| 2.2 Accounts | done (A1 #39, F3 #40, A2 #41) | done (G3 Bramble #918, M5 #921) |
| 2.3 Transactions | done (#22, #25, #31) | done (Bramble #916) |
| 2.4a Shims | done (#43) | done (M6, Bramble #922) |
| 2.4b Portfolio | done (#44) | done (M7, Bramble #923) |
| 2.4c History and activity | done (#45) | done (M8, Bramble #924) |
| 2.4d Privacy | done (#46) | done (G1 Bramble #919, M9 #925) |
| 2.5a Multisig account | done (#47) | done (M10, Bramble #926) |
| 2.5b Multisig coordination | done (#48) | done (G2 Bramble #920, M11 #927) |
| 2.6 Catalogs | done (#49) | done (M12, Bramble #928) |
| 2.7 Swap | done (#50), behind the `mobile` feature | done (M13, Bramble #929), at `v0.3.0`: `mobile/core` is a shell over this crate |
| Vault switch | done (V1 #58) | G4 done (Bramble #947); M14 done (Bramble #980): mobile pins `v0.4.0` |
| [Mobile parity](#mobile-parity) | P6 done (#65), P2 done (#68); P1's mechanism done (P1a #70, P1b #71, P1c #72), not exported until Krusty exports a selector and signature verification; P3 done (#80), not shippable until Bramble's imported-key v13 fixture lands; P4 done (#78), no fixture yet (needs a Bramble generator); P5 done (#83), no fixture yet | M15 (Bramble #1007–#1010) takes `v0.6.0`: P2 and P6 merged (#1008, #1009), P4 and P5 in #1010 (open); P1 waits on its export, P3's screens were left out |
| [Parity](#parity) P7–P16 | P7 done for reading (#87, #92): every v13 and v14 collection is read, every vault the extension opens opens here, and what mobile does not operate is kept unused and listed; the 13→14 migration waits on v0.10.11 reaching the extension's `main`. P9's mechanism done (#84) with its review flags, per-call summaries and caller's activity text (#91), not exported: it waits on Krusty's selector (krusty-kms#257). P10 done (#86). P11's core half done (#90), P12's done (#85, #89) and P13's done (#93), none exported: each waits on P9's export; the four are joined (#96). None has a fixture yet (G7). P8 and P14–P16 not started. See [Road to parity](#road-to-parity) | P7's kept accounts (#1009) and P10 (#1008) merged; the rest wait on their exports |
| 3 Extension | not started, and waits for parity; `js/package.json` is private | n/a |

`v0.1.0` is tagged at `35abc8b` (GitHub release only) and was what M2 and M3 pinned. `v0.2.0`
renamed the package to `starknet-wallet-core` and carries #25, which removed
`PlanHost::discard`. `v0.2.1` was what mobile pinned through M4: it makes `send::plan_probe`'s
selectors, `selector` and `braavos_signers` public, which mobile's `recovery_advisory.rs` used.
`v0.2.2` is the first crates.io publish (#38, #27) and adds, since `v0.2.1`, the vault's refusal
of a DEK the file's slots do not wrap (#36) and vault transition tests from the Quint specs (#34,
#35). Mobile went from `v0.2.1` to `v0.3.0` at M5; M5–M13 each merged pinned to that tag.
`v0.3.0` is a GitHub release only: its crates.io run was cancelled before approval, so crates.io
goes from `0.2.2` to `0.4.0`. `v0.4.0` breaks three things, none migrated because mobile has no
users: the vault is the extension's v13 payload and older vaults are refused (#58),
`importWallet` takes no BIP-39 passphrase (#58), and Ready (Argent X) legacy keys change for about
0.4% of phrases (#59; 3,949 of 1,000,000 random phrases measured).
`v0.5.0` carries #61–#96 (#74 was closed unmerged): Mobile parity P2–P6, and the core halves
of P1, P7 and P9–P13, which are not exported; new vaults are still schema 13. Mobile adopts
three changes with the bump: `RelayStep` gains `NextPage` (#77), which each app's relay loop
must handle; `explorerFallbackAllowed` moves from `TokenPreferences` to the wallet and takes the
preferences (#78), so an RPC override turns the fallback off; and `AccountRecord` gains
`hidden` and `imported` (#65, #80) with no defaults. `ActivityEntry`'s new fields default to
nil.
`v0.6.0` carries #98: mobile opens every vault the extension opens (P7). It only adds to the
bindings (`unusableAccounts`, `UnusableAccount`, `UnusableReason`, and
`AccountRecord.preFixReadyKey`, false by default), but the new public field breaks a Rust struct
literal, hence the minor bump. `starknet-wallet-core` is unchanged since `0.5.0`.

### Next PRs

Generators (G*) and adoption work (M*) are PRs in Bramble (`starknet-innovation/mc-wallet`)
against `feature/mobile-wallet`, where `mobile/core` and the generators live (`main` has neither).
The rest are PRs here. Phase 2's PRs (M1–M14, G1–G4, A1, A2, F1–F3, V1, K1 and the ports) are
all merged; each is described under its step in [Phases](#phases).

#83–#87 and #89–#93 (2026-11-21) landed the core halves of P5, P7 and P9–P13 at the product
owner's request: several steps ahead of mobile, where [Process](#process) says one, each on unit
tests ported from the extension's own, with its generator owed. M15 (Bramble #1007–#1010,
2026-11-22) takes `v0.6.0` and ships P2, P4–P6 and P10 on both apps; P3's screens were left out.

#### Road to parity

Written 2026-11-23, against #990's table. With M15 merged, mobile ships 17 of the extension's 29
user-facing rows; 12 remain. What decides the pace is not the porting, which is mostly done, but
one upstream export and three decisions (D1 and D3 settled 2026-11-23):

- **krusty-kms#257** (a selector from an entrypoint name and Stark signature verification). Open
  since 2026-11-20 with no movement. It blocks P1's export and P9's, and through P9 every
  feature that submits calls: P11, P12's revoke, P13, P14 and P16, and P8, whose check before
  broadcast verifies the device's signature. Chasing it is the highest-value thing anyone can do
  for parity.
- **D1 Ledger transport** (P8). *Decided* 2026-11-23 (product owner): Bluetooth first on both
  apps (Nano X, Stax, Flex), Android USB (Nano S Plus) as a follow-up, since iOS cannot reach a
  Ledger over USB at all; Ledger's own mobile SDK for the link to start with, ported into the core
  later; Ledger-only wallets (no phrase) at full parity.
- **D2 Dapp transport** (P16): an in-app browser that injects the extension's provider, or
  WalletConnect, or both. *Proposed*: the browser first, since every Starknet dapp already speaks
  to an injected wallet through `get-starknet` and the extension's provider and approval rules
  carry over unchanged; WalletConnect later, for a desktop dapp signing with a phone, once it is
  known which Starknet dapps would find Bramble through it. The core half is the same for both.
- **D3 STRK20 client** (P15). No phone makes a proof: the SDK the extension vendors
  (`starkware-libs/starknet-privacy`, TypeScript over starknet.js, `ohttp-ts` and zod, no
  WebAssembly, about 340 KB of source, still at `PRIVACY-0.14.3` release candidates) sends the
  actions to StarkWare's proving service (`starknet_proveTransaction`, optionally through an
  OHTTP relay), and the extension proves remotely too. The question was how mobile runs the
  SDK's client side, which holds keys. *Decided* 2026-11-23 (product owner): embed the pinned SDK
  for now, port it to Rust later.

Generators (G*) and adoption (M*) are Bramble PRs against `feature/mobile-wallet`; X* are export
PRs here, each tagged before its M*. Each feature follows the usual order: generator, mechanism,
export and tag, then the mobile PR. Numbers after G7 and M16 are placeholders, assigned when a
step starts.

**Wave 0: unblocked now.**

| PR | Repo | Needs | What it is |
| --- | --- | --- | --- |
| M15d | Bramble | — | #1010 (custom RPC screens, Starknet names), the last of M15 |
| G7a | Bramble | — | fixtures for what mobile already ships on unit tests alone: an imported-key v13 vault (P3), the RPC override rules (P4), Stark-name normalisation, calldata and judging (P5), and the CSV, filter and counterparty summary over one feed (P10). Also regenerates G6's `extension-activity-detail.jsonl` now that Bramble #1002 is closed (the extension stores a receipt fee; `throughProvider` should stop showing the gap), and adds G5's import over a partial intent now that #1003 is closed |
| G7b | Bramble | — | G4 at v13 with every collection filled, and at v14 with an `accountProvenance`, sealed by the extension's own store (P7) |
| M16 | Bramble | G7a | P3's screens (private-key import), if the product owner wants them back in; the export is in `v0.6.0` |
| M17 | Bramble | — | analytics consent and diagnostics screens; nothing here |
| Extension Krusty 3 | Bramble, `main` | — | the extension still pins `@starknetfoundation/krusty-kms-wasm` `0.11.0`, which predates the fix (Krusty 3.0 changed the derivation; 2.0.0 and earlier kept the leading zero bytes), so its keys and mobile's part for affected phrases. Its users have funds, so it follows Krusty's [migration guide](https://github.com/starknet-innovation/krusty-kms/blob/v3.0.0/docs/migration-3.0.md): keep the old key for any existing account whose public key differs. Needed before Phase 3, where the extension takes this crate's Krusty |
| P7 migrate | here | v0.10.11 on the extension's `main`, G7b | `NEW_VAULT_SCHEMA` to 14 and raising a v13 vault on unlock, as the extension's migration does. Until v0.10.11 ships, a backup mobile raised to 14 would not open in the released extension |
| D2 | — | — | the dapp transport above; D1 and D3 are settled |
| P15 spike | here | — | done 2026-11-23: see "P15 spike findings" under P15 in [Parity](#parity). The account key stays in Rust (the SDK only asks for a `CallSet` signature the core can compute and sign); the viewing key crosses into the runtime and on to StarkWare's prover and discovery service, as in the extension; a hidden WebView on both apps runs the pinned bundle unchanged behind a native fetch bridge; no StarkWare Rust client exists or is announced; the core's first P15 PR (the `PrivacyEngine` trait, `CallSet` signing, result judging) can start now |

**Wave 1: when krusty-kms#257 ships.** In order; each export is one PR here and one tag.

| PR | Repo | Needs | What it is |
| --- | --- | --- | --- |
| K2 | here | krusty-kms#257 | the Krusty bump (and `starknet-types-core` with it, if Krusty moves it), and the one implementation of `multisig_invoke::StarkPrimitives` over Krusty, in `wallet-core`. The test implementation stays for the tests |
| G7c | Bramble | — (can start now) | P1's missing fixtures: the multisig import cases and the call summaries beyond an ERC-20 transfer |
| X1 P1 | here | K2, G7c | P1's FFI surface: deploy, propose, co-sign, submit, change signers, and the import path with the checks the extension's `importIntent` makes and the plan says the export must enforce (the wallet's own call review replaces the remote summary; the chain's authority and nonce must equal the intent's snapshot) |
| M18 | Bramble | X1 | multisig transaction screens on both apps: propose, sign, submit, change signers |
| P9 rest | here | — | fee affordability for a batch: done (#104, fee-only as the extension's dapp card, over the host's `Invoke::with_fee_balance`). The pending record already carries the batch's allowance changes (#96); Bramble's host filing them is X2's |
| G7d | Bramble | — | a multicall and a refused batch through `submitInvokeTransaction`, and `verifyCalls` over batches that raise each flag (P9) |
| X2 P9 | here | K2, P9 rest, G7d | the batch through `Plan`, exported only with its first consumer: no UniFFI call signs arbitrary calls on its own |
| G7e, X3, M19 | both | X2 | P12 allowances first, the smallest consumer: `Approval` decoding, a scan with legacy windows, the merge, eviction, the revoke and its settling (G7e); the two step machines, the log and the merge exported (X3); the allowance manager and revoke screens, with Bramble's host filing every pending record's allowance changes (`allowances::calls::record_changes`) before its row (M19) |
| G7f, X4, M20 | both | X2 | P11 swap: a quote, a build and the refused builds through the real handler; the export; the swap screens |
| G7g, X5, M21 | both | X2 | P13 staking: a live directory page, a positions answer, a pinned read and each action's calls; the export (Mainnet only, `STAKING_POLICY`); the staking screens |
| P14 | here, then both | X2 | gasless and sponsored: a SNIP-9 v2 outside execution the gateway builds and the wallet checks (calls, fee transfer, ceiling, window), and the paymaster's bodies and replies (`transactions/src/gasless/`, 1,818 lines). Its generator first shows whether Krusty's SNIP-12 hash agrees with starknet.js on this message; if not, it waits on Krusty rather than adding hashing here. Adds the sponsored and gasless rows P2 left out |

**Wave 2: when its decision is made.** Each can run beside Wave 1.

| PR | Repo | Needs | What it is |
| --- | --- | --- | --- |
| P8 Ledger | here, then both | D1 (settled); krusty-kms#257 for signature verification | as the extension's v1: blind signing only. The Starknet app's version, public-key and `signHash` commands with the extension's minimum app version and its status-word catalogue (`ledger-protocol/`, 1,096; `background/ledger/`, 847), as sans-IO APDU bytes here; a signing step in `Plan` that hands the hash to the host's device and takes a signature back, verified against the record's stored `publicKeyX` before anything is broadcast, and never falls back to a key (`account-hash-signer.ts`); Ledger records made operable; and a vault with no phrase opened and created, for Ledger-only onboarding. The apps link to the device through Ledger's Device Management Kit (`LedgerHQ/device-sdk-mobile`, Kotlin Multiplatform; Android native, iOS through Objective-C interop; 0.0.x as of 2026-11, so pinned exactly and checked for maintenance first). It has no Starknet signer module, so it only scans, pairs and moves the core's APDUs (its raw APDU call). Bluetooth first; Android USB a follow-up. Later: Ledger's Bluetooth framing moves into the core and the kit goes. The generator records APDU transcripts against the Starknet Ledger app; *proposed*: under Ledger's Speculos emulator, so CI needs no device |
| P16 Dapp connect | here, then both | D2, X2 | request validation, grants (the vault's `originGrants`, and the per-account revocation P7 left, `withoutOriginAccess`), the approval queue and its review budget, typed-data review and signing (SNIP-12), add-chain and watch-asset, the blocklist and phishing feed as a sans-IO body and judge (`background/provider/`, 2,453; `background/approvals/`, 4,169; `permissions/`, 905). The core takes a request (origin, method, params) and returns a review, a grant or a refusal, whatever carried it. Mobile builds the transport: for a browser, a WebView whose bridge binds every request to the top frame's origin as the platform reports it and refuses frames it cannot attribute. P9's poisoned-payee refusal must not reach the site before the person acts |
| P15 STRK20 | here, then both | D3 (settled), the P15 spike | activate, shield, unshield and private send; then shadow accounts and multisig STRK20 (`privacy/`, 9,279; `background/privacy/`, 5,872), over 2.4d's read plan. The core owns the policy, the review and the extension's own rules around the SDK; the SDK is an engine the core drives through a trait, as `StarkPrimitives` brings in what Krusty lacks, and each app runs the same pinned bundle the extension vendors (its pin, its digests) in a sandboxed runtime the spike picks. The largest step: six PRs in that order. Later: the SDK's client ported to Rust, once upstream leaves release candidates or StarkWare ships one, with its crypto from Krusty (the core adds none of its own), and the embedded runtime goes |

Done means #990's every shipped row on both apps, each proved against the extension by its
generator. Phase 3 (the extension takes this crate) starts after that, and needs Extension Krusty
3 first.

### Mobile parity

From Bramble #990 (2026-11-20), a read-through comparison of the extension (`main` at
`a01b240c`) and the mobile apps (`feature/mobile-wallet` at `008f2968`). Most of its gaps are
mobile screens and stay in Bramble. These are the ones that need this repo. Each lands like a
Phase 2 step: golden fixtures from an extension generator first, the mechanism in `wallet-core`,
Bramble's choices in `bramble-core`, then a mobile PR that takes the tag.

Needed now:

- **P1 Multisig transactions.** Deploy, propose (calls, nonce, transaction hash), collect
  co-signer signatures, check the threshold, build the execute transaction, and change signers.
  It builds on `accounts::multisig*`, 2.5b's coordination and `send/sign.rs`. Its fixtures are
  G5's (Bramble #998, `extension-multisig-transactions*.json`).
  P1a (deploy, propose, co-sign, submit) is in `send::multisig_deploy` (the plan's standalone
  deployment under `Authorization::ArgentMultisig`), `send::multisig_invoke` and
  `multisig_invoke_plan` (`ProposePlan`, `SubmitPlan`: neither signs nor holds a key) and
  `send::multisig_intent` (the signing intent, its package, merge and validator); Bramble's fee
  overhead for a transaction that waits on co-signers is `MULTISIG_FEE_OVERHEAD`. Plain Rust,
  not yet exported. Krusty 3.1.0 exports neither a selector (`starknet_keccak` of an entrypoint
  name) nor Stark signature verification, and the core adds no crypto of its own, so both come
  in through `multisig_invoke::StarkPrimitives`; nothing implements it outside the tests until
  Krusty exports them, which the mobile export waits on. Left to the wallet: the class-declared
  read before a deployment, endpoint failover, and the extension's operation bookkeeping.
  P1b (change signers, and the review of any call batch) is in `accounts::multisig_change`
  (the change, its self-call, the strict parser and the next configuration) and
  `send::multisig_review` (`requireSafeArgentMultisigCalls`, `reviewArgentMultisigCalls`), whose
  ordinary-call line is the extension's `decodeCall` in `send::call_decode` and
  `send::avnu_route`; the wallet's tokens, watched collections, trusted addresses and summary
  length are arguments, and refusals are typed with the extension's sentences. Only the ERC-20
  transfer line and the signer-change paths have golden fixtures: the other call-summary
  decoders and the poisoned-recipient refusal have unit tests only, and need a Bramble
  generator.
  P1c replays G5's refusals (`-refusals.json`, all but call review) against those modules: every
  input the extension refuses is refused, each with the typed variant that carries its sentence.
  The replays found no wrong accept; the gap was `canReplaceArgentMultisigSigningIntent`, never
  ported, now `SigningIntent::can_replace`. A review of the import path then found one: an
  explicit `null` in an intent's or record's optional key read as absent and passed the validator,
  and now fails to parse; the package envelope is checked (`IntentPackage::parse`) and a remote's
  lifecycle claims are dropped before the merge (`SigningIntent::locally_derived_imported`).
  The rest of the extension's `importIntent` is `send::multisig_import::import_intent`: given
  the authority and nonce the host read, it refuses unless they equal the intent's snapshot and
  prepared nonce, replaces the remote summary with the wallet's own call review (refusing when
  the review refuses), refuses a stored intent under the id that froze another transaction,
  merges, and returns only what the validator and replacement rule accept. These rules have
  mechanism tests only (G5 recorded no import case) and need a Bramble generator. Still the
  wallet's: the session check, the active network and the store's capacity; reading the nonce
  of the record's own address (`import_intent` takes a bare felt, as the extension's unpinned
  read does, so nothing in the core binds it to an address); and showing the fee before
  co-signing (the extension's import also returns `feeReview`; the core returns only the
  intent, whose `PreparedInvoke::fee()` the wallet shows). The mobile export must route every
  import through `import_intent`. The import departs from the extension in four ways: the
  record must be the frozen transaction's account by address and network as well as by id
  (`UnknownAccount`; the extension's lookup cannot miss); an authority read of another address,
  network or class is `ReadForAnotherAccount` (no counterpart: the extension builds its read
  from the record); a read that ends other than `Available` is `AuthorityUnavailable`, the
  extension's fixed fallback sentence, where the extension shows whatever its read threw; and a
  stored intent with another snapshot is `Conflict` straight from the stricter merge, the
  answer the extension's store gives after its looser one. Two refusals deliberately differ: a
  non-member deployer is
  refused before any read, and a write answered with another hash is `HashMismatch`. The merge
  and submit deliberately differ too, closing gaps the extension has: `merge_imported` refuses
  another intent and stops at the threshold (the extension merged past it into an invalid
  intent; Bramble #1003, closed 2026-11-22, so G7a checks whether the two now agree), and `SubmitPlan` takes the `SigningIntent` and verifies it rather than trusting loose
  signatures. So does the deployment review, which refuses an address running another class
  (`AccountClassUnrecognised`) where the extension reports it as already deployed; and
  `ProposePlan` applies `require_safe_calls` itself rather than relying on the host's review.
  Neither app can deploy, propose, sign, submit or change signers today; Android's missing
  create stepper, participant picker and shared-account home are screens, Bramble's.
- **P2 Activity fee and value when sent.** `ActivityEntry` gains the fee (from the receipt) and
  the value when sent. It already has `network` and `token_address`, which mobile can show
  without a release. The oracle is the extension's `activity-detail.ts`. `PriceBook` has no
  historical price, so the value is either a historical read or recorded at submit time for this
  wallet's own sends; follow whichever the extension does.
  Done: G6 (Bramble #999: `extension-activity-detail.jsonl`, `scripts/mobile-parity-activity/`)
  and the core half (#68); mobile still calls `price_with` before Confirm and renders the texts. The extension records the price at submit time (`activity-valuation.ts`: a fresh
  cached price, only for a public send this wallet made) and reads the fee from the receipt
  during reconciliation, only for its own rows (`origin: "wallet"`). So `ActivityEntry` gains
  `fee_paid`, `submitted_price` and `origin`; the plan reads a settled record's receipt in the
  same pass as its status (`PlanHost::wants_fee`, `record_fee`); `TransferPlan::price_with`
  prices the record from a `PriceBook`; `activity::detail` holds the rules and the row text.
  Found on the way: through starknet.js 10 the extension's `readReceiptFee` refuses every
  receipt (a `ReceiptTx` is not a plain object), so its rows never store a fee; the core follows
  `readReceiptFee` over the node's JSON, and the fixture's `throughProvider` shows the gap until
  Bramble fixes it (#1002, closed 2026-11-22; G7a regenerates the fixture). Sponsored (`feePayer`) and gasless (`userFee`) rows and the slow-pass fee
  backfill are left out: mobile has none of them.

Wanted (2026-11-20: P3 and P4 by name, P5 with the parity decision):

- **P3 Private-key import.** Wanted: the product decision behind #80. An inbound-only export that seals the key into the vault and never
  returns it. That amends the hard rule that the mobile build exports no raw keys, so the rule
  in `AGENTS.md` changes in the same PR. V1's reader requires the collection the extension seals
  imported keys into to be empty, so it lifts that for this one collection.
  As built: `WalletCore::import_private_key(network, private_key)` (`key_import.rs`) is the
  extension's `wallet.importAccount` for an OpenZeppelin account: the text is read as
  `normalizeFeltInput` then `isStarkPrivateKey` read it (`felt::parse_private_key_input`), Krusty
  gives the public key, the account sits at Bramble's OZ class with no salt, an id the wallet
  holds is refused, and the account becomes active with its key sealed. The extension does not
  keep the key in one of the six unused collections, as assumed above: it seals it in
  `signingKeys`, under an account whose derivation is `imported-raw-key` (no `publicKeyY`), which
  V1 refused by its kind. So the reader accepts that kind, every unused collection stays
  required empty, and mobile's stronger rule for such a record is that its public key and address
  are what its sealed key derives. The session holds imported keys
  (`VaultPayload::imported_keys`); sends, signatures, the privacy viewing key and the network
  projection all take an account's key from `VaultPayload::signing_key`, and adding an account
  skips an index already held as an imported key (the extension's Sherlock 226). Deliberately
  not done: Argent import (releases picked after discovery, or a pasted address re-proved on
  chain), a deployment salt, a wallet made from a key alone (`createKeyVault`), revealing the key
  (the extension's reveal shows it; mobile returns no key), and removal, which the extension has
  for no single account either (hiding works). A vault holding an address-picked import, whose
  address is not what its key derives, is refused like any record that does not re-derive. The
  record's parity has unit tests only: G4 holds no imported key, and a fixture is never written
  by hand. Before a mobile release ships the import, Bramble needs a generator that seals, with
  the extension's own store, a v13 vault holding one imported OpenZeppelin key (an imported Argent
  one too, ideally) beside G4's accounts, for a test that opens it and writes it back unchanged. It must run at a v13 Bramble commit:
  some Bramble branches move the vault to schema 14 (`accountProvenance`), which mobile's reader
  (v13 only, unknown fields refused) does not open.
- **P5 Stark names.** A sans-IO body and reply judge for name → address and address → name,
  beside `rpc.rs`; the naming contracts are Bramble's catalog data.
  As built: name → address only, the extension's `resolveStarkName`
  (`packages/platform/starknet/src/rpc/stark-name.ts`); address → name has no oracle and is not
  built. `wallet_core::stark_name` holds `normalize` (`normalizeStarkName`: JavaScript `trim`
  and `toLowerCase`, then starknet.js 10.8.0's `isStarkDomain`), `address_request` (the
  `domain_to_address` `starknet_call` at `latest`, calldata the label count, each label's
  `useEncoded` felt and an empty hint, against a contract the caller passes) and `judge_address`
  (`StarkNameAnswer`: a nonzero felt resolves; zero, negative or past the prime is `NotFound`, as
  `canonicalFelt` then `isNonZeroStarkFieldElement` read it; JSON-RPC 20 or "contract not found"
  is `UnsupportedNetwork`; an unreadable or non-felt answer, or an error with no numeric code,
  is `TryNextEndpoint`, the extension's failover; any other node error is `Failed`, where the
  extension throws). `address_request` also refuses what the extension refuses before the call:
  typed text past 128 UTF-16 units (`readStarkNameInput`) and a 48-character label whose
  encoding passes the prime (`CallData.compile` throws). The encoder uses `js::big`; no new
  crypto. `bramble-core` (`stark_name.rs`) holds the mainnet and Sepolia naming contracts as
  constants (starknet.js's `StarknetIdContract`; the generated catalogs do not carry them) and
  exports `stark_name_normalize`, `stark_name_address_request` and `stark_name_judge_address`.
  Departures: the chain-id check and failover stay with the host, as for every sans-IO read
  (mobile reads each endpoint's chain id with `rpc_verify_chain_id` first and never sends the
  call to another chain's endpoint; the extension fails closed when none matches, rather than
  answering unreachable, and mobile should too); the network picks the contract, where the
  extension picks it from the chain id the endpoint reports, which is the same once that check
  passes, and both of Bramble's networks have one, so `UnsupportedNetwork` comes only from the
  node; and the judge is stricter than starknet.js, failing over on a reply to another id, a
  result that is not an array, or an entry that is not a string, which starknet.js would read.
  The extension's own test cases are ported as unit tests (`stark_name_tests.rs`, each citing
  `stark-name.test.ts` by line), with one per refusal branch; no fixture yet, and a Bramble
  generator should write the normalisation, calldata and judging cases from the extension's code.
  Mobile has none yet: its send form asks `stark_name_normalize` whether the recipient is a name,
  and a resolved address must be dropped if the network changed while the read was in flight,
  as the extension's panel does.

Checked:

- **P4 Custom RPC.** Wanted (product owner, 2026-11-20). The extension (`main` at `a01b240c`) keeps
  one override per network in its preferences, not the vault: `rpcOverrides`, network id to URL
  (`isRpcOverrides`, `wallet-model/preferences/preferences.ts`), set and cleared by `setRpcOverride`
  (`settings/src/preferences.ts`: trimmed, empty clears, else `isCustomRpcUrl`, which is
  `isServiceUrl` in `core/service-url.ts`). It defines no network: `networkRpcUrls`
  (`runtime/runtime-state.ts`) tries the override before the bundled endpoints, which stay as
  transport failover. It probes no chain id on save; writes probe every endpoint before signing
  (`withRpcWrite`). With an override and no stored choice, `explorerFallbackAllowed` is off; a
  preferences restore never applies `rpcOverrides`. So the URL grammar is the relay one over
  `https`/`http` (`nostr::relay_url::is_custom_rpc_url`), and `bramble-core` stores `rpcOverrides`
  in `public_store`'s preferences (`rpc_override.rs`), narrowed to `https` as relays are to `wss`.
  A stored override the rule refuses (after a tightened grammar) is dropped on load, as
  `withCanonicalEndpoints` drops one, with that network's explorer fallback kept off
  (`explorerFallbackOff`, the extension's pin of `explorerFallback`); one on an unknown network
  fails closed. `WalletCore::networks` hands the host the bundled records with the override first
  (`networks::with_rpc_override`); chain ids, tokens and explorers never change, and the send
  plan's chain probes still check Krusty's chain id. As in the extension, saving probes no chain:
  a Sepolia URL saved for Mainnet is caught by the host's per-endpoint chain check or, for a
  send, by the plan's probe. `WalletCore::explorer_fallback_allowed` reads the override itself
  (false while locked), so the host cannot pass the wrong answer. The extension's `redactEndpoints`
  is `relay_url::redact_endpoints`, exported to mobile; `NetworkInfo`'s `Debug` leaves out
  `rpc_urls`. Deliberately different: no `http` on loopback (cleartext is blocked on both
  platforms), and a typed URL the parser would respell (case, a default port, dot segments) is
  refused rather than rewritten, since the core has no URL parser. The rules have unit tests only
  and need a Bramble generator. Mobile has none yet. Mobile adoption: endpoints from
  `wallet.networks()`, not `supported_networks()`; keep the per-endpoint chain check; the explorer
  lane asks `wallet.explorer_fallback_allowed(network, preferences)`; pass every transport and RPC
  failure text through `redact_endpoints` before it is shown, logged or reported, and never log a
  `NetworkInfo`; add the settings field. Mobile has no explorer-fallback toggle, so a network
  pinned off by a dropped override stays off.
- **P6 Hidden accounts.** The extension (`main` at `a01b240c`) keeps them in its preferences,
  not the vault: `hiddenAccountIds`, unique canonical account ids, default `[]`
  (`packages/features/settings/src/preferences-accounts.ts`, `isHiddenAccountIds` in
  `packages/foundation/wallet-model/src/preferences/preferences.ts`). Its background
  (`apps/extension/src/background/accounts/account-visibility-ops.ts`) refuses hiding an unknown
  account or the last one a network still shows, and moves off the active account before
  recording the hide; reveal takes any id. Selection prefers a visible account
  (`accountForNetwork`, `runtime/runtime-state.ts`); a hidden one can still be selected. A
  preferences restore leaves the list alone, which mobile, with no preferences restore, has no
  counterpart for. So it lands in `public_store.rs`, and V1's reader is unchanged (G4's
  `vault-v13.json` carries no flag). Mobile has none yet.

Covered already: the vault format (V1, in `v0.4.0`; mobile took it at M14, so a
backup moves between mobile and the extension, unless it holds something V1 refuses: see P7).
Not covered, though this paragraph once said so: token swap. Step 2.7 "Swap" swapped mobile's
dependency; the core has no swap beyond the AVNU route layout `send::call_decode` reads. It is
P11. #990's other gaps are [Parity](#parity), below.

### Parity

Decided 2026-11-20: this comes next, before Phase 3. Parity means every row of #990's table the
extension ships is shipped by both apps, with its logic here and proved against the extension.
The oracle is the extension's `main` at `a01b240c` (v0.10.10, still its head on 2026-11-20).
v0.10.11 is on its release branch and will move the oracle when it merges.

Each step lands as P1–P6 did, in this order: a generator in Bramble writes golden fixtures from
the extension's own code; the mechanism lands in `wallet-core` and Bramble's choices and the
export in `bramble-core`; a tag; then one Bramble PR takes the tag and adds the screens to both
apps. A step is done when that last PR merges. The core stays at most one step ahead of mobile
(Process), so two steps are in flight at most. Sizes are the extension's TypeScript without
tests, as a guide to the oracle, not to the port. Placements are proposals, confirmed when a
step starts.

Foundations, which the features need, and Ledger, which needs only the first:

- **P7 Vault: all of v13, then v14.** V1 reads only what mobile writes and refuses a vault
  holding anything else, so a backup from an extension with a dapp grant, a Ledger account, a
  shadow account or a multisig viewing key does not open on mobile, and P8, P15 and P16 have
  nowhere to store. Port the rest of the extension's validators
  (`packages/platform/vault/src/schema/`, 1,106 lines), so the reader accepts every v13 vault
  the extension accepts and writes back unchanged what it does not use. Then follow v0.10.11's
  schema 14. #990 says that step changes only the number; it does not: on `release/v0.10.11`
  it adds account provenance (`schema-provenance.ts`, with a migration). Oracle: G4 extended to
  a vault with every collection filled, then the same at 14.
  As built, first half (v13; its stronger rules no longer refuse a vault but keep a record, see
  "Matched to the extension" below): the reader accepts the vaults the extension's v13 validator
  accepts, except those it refuses whole for one of mobile's stronger rules: a vault with no
  phrase (`mnemonic: null`: a Ledger-only or key-only wallet; mobile has no phrase-less wallet);
  a phrase record off mobile's three ladders (both index fields stepped), past a `u32` index or at
  the reserved agent index; an OpenZeppelin record at a class other than Bramble's; any record of
  a kind mobile operates that does not re-derive from its key, including an `imported-raw-key`
  record whose address is not what its key derives (the extension's Argent import at a picked
  address, see P3); more than 128 records; and the grammar cases below (origins, number literals,
  repeated keys, lone surrogates). `vault_wire.rs` is the schema's records as serde types
  (unknown keys refused; an optional key absent or set, never `null`, which V1 let through for
  `deploymentSalt` and `publicKeyY`; `activeAccountId` and `mnemonic` present), `vault_rules.rs`
  is `isVaultV13` rule for rule (every account kind, Ledger paths, the signing-key bindings with
  keyless Ledger records, origin grants, imported phrases and their bindings, Ledger viewing keys,
  shadow-account scopes, multisig viewing keys and conflict marks, the extension's devnet), and
  `vault_budget.rs` is `isJson`'s provider budget as `isVaultV11` applies it. Both run on every
  open and every write, so mobile neither opens nor writes a vault the extension refuses; a write
  past the budget fails with the extension's `vault storage limit reached`. Mobile operates only
  the records it holds keys for, its phrase's `starknet-bip44` and `argentx-bip44` records and
  `imported-raw-key` ones on Mainnet and Sepolia, under the stronger rules above (every record,
  y-coordinate and sealed key re-derived). Every other record (a Ledger account, one derived from
  an imported phrase, one on `SN_DEVNET`), its sealed key and the six collections are
  `VaultPayload::kept`, outside `accounts`, so no list, selection, signature, send or privacy read
  reaches them, and every write puts them back where they were (records at their positions,
  signing keys in the vault's order); a new record never takes a kept record's id, and the
  address-poisoning check trusts a kept record's public address on its network as the person's
  own, as the extension's `ownAccounts` does (`VaultPayload::own_addresses`). "Unchanged" means
  equal as JSON, as G4's test already compared V1's records: object key order (alphabetical here,
  insertion order in the extension) and string escapes may differ, nothing else. Where the port's
  grammar is stricter than the extension's: an origin is held to the shared endpoint grammar (no
  IDN or other host the WHATWG parser accepts beyond letters, digits, `-` and `_`; at most 2,048
  characters), a number to a plain integer literal (`1.0` and `1e0` are refused), and serde
  refuses repeated keys and lone surrogates that `JSON.parse` takes. One gap the other way: an
  origin with an `xn--` label that is not valid punycode passes the hand grammar where the
  extension's URL parser refuses it. A vault whose active account is a kept one opens with no
  active account until one is picked or the network switch picks one, and a vault holding a
  phrase and only kept records opens as waiting for discovery ("Find your accounts"); the product
  owner kept both (2026-11-22). Unit-tested only
  (`tests/vault_v13_kept.rs`, `vault_v13_rules.rs`, `vault_v13_accounts.rs`,
  `vault_v13_poisoning.rs`: the extension's own validator cases, moved onto G4's payload in test
  code): G4 holds none of these, so before a release relies on it Bramble needs G4 extended with
  every collection filled, sealed by the extension's own store at a v13 commit, for a test that
  opens it and writes it back unchanged.
  As built, second half (v14, #92): the reader accepts the extension's schema 14
  (`release/v0.10.11` at `62db6fc10`) beside 13. `isVaultV14` is v13's rules over the rest and
  `validAccountProvenance` over the new optional `accountProvenance` (`vault_provenance.rs`):
  absent or an object, keyed only by ids of accounts the vault holds, operated or kept, each
  `created` or `imported`; `null`, an array, another origin, and the key in a v13 vault are
  refused. As in the extension the map is outside the JSON budget; its parse stops past mobile's
  128-record cap, and a repeated key is refused where `JSON.parse` keeps the last. There is no
  migration while the released extension (v0.10.10) reads only 13: a vault is written back at
  the schema it was read at, in a container of that version (the payload's schema must equal
  the container's), and a new one at `NEW_VAULT_SCHEMA`, 13. That was the coordinator's call,
  not yet the product owner's; once v0.10.11 is on `main` the constant becomes 14 and a v13
  vault opened is raised to 14 with no provenance and written back on unlock, as the
  extension's migration does. At 14, mobile records origins as the extension's service does:
  `created` for `add_regular_account` (and a new wallet's first account), `imported` for
  `import_private_key`, the source's origin (or none) for the network switch's projection, none
  for a restore from the phrase; every other entry, a kept record's included, is written back
  unchanged, and a failed write rolls the map back. Unit-tested only (`tests/vault_v14.rs`,
  `vault_v14_writes.rs`), over G4's payload in test code; a v14 G4 sealed by the extension's
  own store is owed by a Bramble generator. Left for P16: per-account origin revocation
  (`withoutOriginAccess`).
  Matched to the extension (product owner, 2026-11-22: mobile opens every vault the extension
  opens): mobile's stronger rules decide which records mobile operates, never whether the vault
  opens. `vault_admit` re-derives each record mobile could operate; one that does not re-derive
  (a picked address, a key or y-coordinate that does not match, a coordinate, class or index
  mobile does not derive) or that comes past `MAX_ACCOUNT_RECORDS` (128, the bound on unlock
  work; the extension's only bound is its budget) is kept, as a Ledger record is, and
  `unusable_accounts` lists every kept record with its reason (`UnusableReason`: Ledger, an
  imported phrase, another network, not derived here), so the host shows it as not usable on
  this device rather than missing. The provenance map is bounded by the budget's array length
  (4,096) instead of 128. A Ready record whose sealed key an earlier Krusty derived
  (`ready-key-drift.ts`: Krusty 2.0 and earlier kept the ETH key's leading zero bytes, about 1
  phrase in 256) is operated with that sealed key, as v0.10.11 keeps signing with it, when the
  record is what the key places; the key is held in the session beside imported keys
  (`VaultPayload::held_keys`), the network switch seals a copy under the projection as
  `deriveNetworkAccount` does, and `AccountRecord::pre_fix_ready_key` tells the host to warn as
  the extension does. Only Ready's records are held this way; any other record whose sealed key
  is not the phrase's is kept. Still refused: a vault with no phrase (until P8), and the
  stricter grammar above. Unit-tested (`tests/vault_match.rs`), over vaults built in test code.
- **P8 Ledger.** Straight after the vault step: the extension ships it to every user (#990's
  "trusted testers" is wrong), so a Ledger user cannot use mobile at all until this lands. The
  device protocol and session as sans-IO APDU exchanges: `packages/platform/ledger-protocol/`
  (1,096) and `background/ledger/` (847). The transport is the app's (the extension's is
  `ledger-browser`: Web Bluetooth, WebUSB, WebHID), and how a phone reaches the device is an
  Open question to answer while P7 is in flight. Needs P7, for the vault's Ledger accounts; its
  tests need a device.
- **P9 Call batches.** `send::Kind` is a transfer, a deployment or a refresh; a single-signer
  account cannot submit arbitrary calls. Swap, staking, a revoke and a dapp's invoke all do. Add
  an invoke kind to `send::plan::Plan` whose review is the one multisig already has
  (`send::multisig_review`, `send::call_decode`). The oracle is the extension's submission
  (`packages/platform/starknet/src/rpc/rpc-submit.ts`) and its call checks
  (`packages/features/transactions/src/routing/verify*.ts`); neither has been read for this
  plan, so the step starts by reading them.
  As built (mechanism only, not exported): the extension has one submission path. Its ERC-20
  send is `submitInvokeTransaction` with one call (`submitErc20Transfer`, `rpc/rpc.ts`), and
  every family is a starknet.js `Account` with `cairoVersion: "1"` (`krustyAccount`), so a batch
  is the transfer path with N calls in the same `__execute__` encoding. `Kind::Invoke` therefore
  rides the transfer's steps unchanged (probe, review estimate, submit re-estimate under the
  reviewed ceiling, automatic deployment, sign, write, hash check, record), reading its calldata
  from `Plan::invoke_calldata`; `calls::execute_calldata` is the one encoder for the transfer,
  the batch and a multisig proposal. Before it signs a batch the extension checks, besides what
  the transfer path already does: a non-empty batch ("No calls to submit", `rpc-submit.ts`);
  `refuseOnPoisonedRecipient` (`verify.ts`), which every caller applies (swap, staking, NFT,
  transfer, STRK20, dapp); and, for a dapp's invoke only, `refuseOnAccountSelfCall` ("Dapps
  cannot call the wallet account contract itself") and `readProviderInvokeCall`'s bounds and
  grammar (`INVOKE_LIMITS`: 16 calls, 4,096 felts a call, entrypoints
  `^[A-Za-z_][A-Za-z0-9_]*$` of at most 64). `send::invoke::Invoke::reviewed` is the only way to
  build a batch: it applies all of them to every batch (the core cannot tell who built one),
  reviews it with `multisig_review::summarise_calls` (the ordinary-call half of the multisig
  review, now shared), and compiles the calldata; refusals are `InvokeRefusal`, with the
  extension's sentences where it has them. The plan re-applies the context-free rules at start
  (`invoke::check` against its own account and `Policy`, and the batch's sender and network), so
  a batch reviewed for another account or network, or a looser policy, fails
  `PreparationUnavailable` before any request. The two counts are Bramble's (`MAX_INVOKE_CALLS`,
  `MAX_CALL_CALLDATA_FELTS` in `SEND_POLICY`); the fee margin and headroom are the transfer's.
  A pending record is
  `PendingKind::Invoke`, mobile's row `ActivityKind::Invoke`, summarised as the review is.
  Departures: the self-call refusal, the bounds and the grammar apply to every batch, not only a
  dapp's; a poisoned payee refuses at `Invoke::reviewed`, where the dapp path lets the flag ride the
  approval card and refuses at execution (Sherlock 140: an instant refusal tells the site whom
  the person trusts), so P16 must not relay that refusal to the site before the person acts; the
  trusted set is fixed when the batch is built, where the extension re-reads it at execution
  (as for the transfer kind).
  The review keeps every call (#91): `Invoke::calls` is each call's `decodeCall` kind and
  summary (`call_decode::decode_call`, `CallKind`) and the annotation flags `verifyCalls` raises
  on it (`send::review_flags`: `unknown-contract`, `unlimited-approval`, `operator-approval`,
  `self-transfer`, `token-contract-recipient`, `foreign-beneficiary`, `undecoded-swap`, as
  `RiskFlag` with the extension's sentences); `Invoke::flags` is the card's deduplicated list,
  and `Invoke::summary` stays the multisig review's line. The approval rules run over the whole
  batch, `approve` replacing a pair's running total, an increase adding, a decrease subtracting
  (clamped at zero, never a grant), seeded from the wallet's own allowance log for the acting
  account on its network and a trusted token only. `Invoke::reviewed` takes the whole
  `ReviewContext` (trusted tokens and collections, the trusted set, pinned contracts such as a
  router or pool, the token contracts, Bramble's `known_token_addresses`, and the allowance
  log). It is the only constructor (#96 removed `Invoke::new`, which under-warned), and the
  review card reads the signed batch's flags from `Plan::risk_flags()`. `reviewed` also reads
  the batch's allowance changes (`allowanceChangesFromCalls`, with the lists the review read),
  and the plan's pending record carries them (`PendingKind::Invoke`) with the amount and token
  a builder named for the row (`ActivityAsset`): the extension files a batch's approvals and
  then its row for any hash that may be on chain, and the pending record is where the core
  hands a host both, before the write leaves. The host files them with
  `allowances::calls::record_changes`; only a transfer's row has a direction. The builder names the activity row (`with_activity_summary`, clamped
  as `clampActivitySummary`; `with_dapp_origin`, "<origin> submitted N call(s)" from the
  batch's own count and `activityOrigin`), falling back to the review line; it rides on the
  `Invoke` the plan already binds to its account and network. Departures: each flag is attached
  to its call; `address-poisoning` refuses instead of flagging; the standing totals are filtered
  in the core rather than by the caller; an empty activity text keeps the review line. A
  batch's fee affordability is the dapp card's (`estimateInvokeApprovalFee`): fee-only, the
  deployment added when the account needs one, FRI only, over the STRK balance the host last
  read (`Invoke::with_fee_balance`; none is `Unknown`); the STRK its calls spend counts only
  toward the gasless offer, not ported. Not ported: the approval queue's summary bound (P16). `RiskFlag` is not the transfer's `ReviewFlag`; the two
  meet at export. Not ported, as
  for the transfer: the `fast` speed, a SNIP-36 proof (P15) and the approval queue's review
  budget (P16). As for a multisig, Krusty 3.1.0 has no selector, so `Invoke::reviewed` takes
  `multisig_invoke::StarkPrimitives`, and nothing
  outside the tests can build a batch until krusty-kms#257. Evidence: every transfer capture
  (OpenZeppelin, Ready, Braavos) driven as a one-call batch puts the same requests on the wire
  and signs the same hashes (`transactions_invoke.rs`); the multicall, its refusals and failure
  paths, and the extension's `verify*` and submit cases, have unit tests only and need a Bramble
  generator (a multicall and a refused batch through `submitInvokeTransaction`). No UniFFI
  export signs arbitrary calls: it arrives with its first reviewed consumer (P11–P13, P16).

Features, smallest and least dependent first:

- **P10 Activity filter and CSV export.** `transactions/src/activity/activity-csv.ts`, and the
  filter in `wallet-ui`'s `portfolio/activity-filter.ts`. Small, and no longer left to the apps:
  two apps would write it twice.
  As built (core half, against Bramble `a01b240c`): `activity::csv::activity_to_csv` is
  `activityToCsv` cell for cell (the header, every cell quoted, the formula guard of
  MCW-V09-123 on every cell, the rebuilt transfer summary, `\n` between rows and none after);
  the value column shares `activity::detail::submitted_value` with the expanded row, so the two
  price the same rows. `history::filter` is the feed's filter, amount sort and stale-axis
  reset (`ActivityFilter`, `activityFilterOptions`, `activityTokenKey`, `activitySearchIndex`,
  `activityRowMatches`, `activityAmountComparator`, `reconcileActivityFilter`), and
  `history::party` the summary above a search that names one counterparty
  (`activityPartySummary`, `activityPartySummaryParts`, with unbounded `bigint` totals): the
  extension writes each row's facts onto its DOM node and filters and summarises the nodes,
  the core reads the same facts off the `ActivityEntry` through `activity_row_model`.
  `bramble-core` exports `activity_csv` (the text; the host writes and shares the file),
  `activity_csv_file_name` (`bramble-activity.csv`, Bramble's), `activity_filter_options`,
  `reconcile_activity_filter`, `filter_activity`, `activity_party_summary` (none while the
  search is blank, as in `activity-ui.ts`) and `activity_party_summary_parts`.
  Found on the way: `activity_row_model` resolved a row's tracked token by symbol alone, where
  `activityTrackedToken` also requires the recorded token address to be that token's
  (Sherlock 249); `activity_tracked_token` now follows it, so a spam contract recorded under a
  tracked symbol no longer takes the tracked token's logo or decimals. The golden row fixture
  passes unchanged. Departures: an axis's `all` is `None`; the explorer link comes from the
  caller's network catalog. Left out: `activityContacts` (it reads preferences; the host
  passes the contacts a search resolves) and `activityExportToast` (screen copy). The
  extension's export is one account's whole feed, unfiltered. Unit-tested only: the
  extension's own cases for these files are ported with their expected strings, plus the
  branches they leave out; a Bramble generator writing the CSV, the filtered rows and the
  summary over one feed is still owed.
- **P11 Token swap.** The AVNU quote and build as sans-IO bodies and reply judges, like the price
  book's: `transactions/src/swap/` (420), `wallet-model/src/swap/` (319) and the background's
  `transactions/swap-{inputs,preparations,build-policy,value-check,ops}.ts`. Needs P9.
  As built (#90; mechanism only, not exported): `crates/wallet-core/src/swap/`.
  `PendingQuote::new` applies `readSwapQuoteInput` and `swapPair`'s rules and builds
  `getQuote`'s GET (the query in the extension's order, the `swap` body cap and the 15 s
  deadline); `judge` is `readQuote`: the first quote, every field in its grammar, pinned to the
  requested pair and amount. `SwapQuote::prepare` applies `readSwapPrepareInput`'s quote id and
  slippage rules and `reviewedMinBuyAmount` (exact over u256), and builds `buildCalls`' POST;
  `PendingBuild::judge` checks the envelope (this network's chain id, 1–4 calls, counted before
  any is read) and `readCall`, then `buildPolicy` (one exact `approve` of the sell token to the
  router the wallet pinned, no direct transfer, one `multi_route_swap` on that router whose
  route, decoded by `send::avnu_route`, sells, buys, pays and floors exactly as reviewed, with
  an integrator fee of at most 100 bps), then `Invoke::reviewed` with that router the one
  pinned contract (#96). `SwapPreparation` carries the per-call review lines (the route's
  is wallet-authored: "Swap … for at least …, paid to …"), the review's flags and the route's
  minimum, and `into_invoke` releases the batch only to the account and network it was
  prepared for, consuming it (the store's one-time claim). The batch names its own activity
  row ("Swap … for ~… via avnu", the amount sold and the sold token) and carries the router
  approval for the allowance log.
  `SwapQuote::value_delta_percent` is `walletSwapValueDelta` over the price book's view. Every
  number and pin is `SwapPolicy`, Bramble's in `bramble-core`'s `swap::SWAP_POLICY`.
  Departures: a quote is a bound value, so preparation takes no quote id or buy amount from the
  screen; malformed felts in a build refuse as "invalid swap call" where the extension's
  harness reached the policy; amounts of 2^256 and above refuse at the input; no fee is
  estimated at preparation (the plan's review does it); no preparation store or digest (the
  host holds the value), and no expiry, as in the extension; the poisoned-payee refusal runs
  when the build is judged, not again at execute (the policy already refuses every transfer
  and any payee but the account). Not ported: `listTokens`. Unit-tested only, with the
  extension's cases and sentences, and not checked against live AVNU replies; a Bramble
  generator (a quote, a build and the refused builds through the real handler) is still owed.
- **P12 Allowances.** Discovery, scan, records, retention and the revoke call:
  `transactions/src/allowances/` (1,916). The revoke needs P9.
  As built, the read side (no generator yet, so unit tests only): `crates/wallet-core/src/
  allowances/`. `record` is the record, its id and scope, `isAllowanceRecord`'s rules on load
  and the unlimited cut-off (2^128 - 1); `retention` is `keepLargest` and `trimToTotal`, ranked
  across tokens by the decimals of a caller's trusted `RankToken`s; `store::AllowanceLog` is the
  log over `host::Storage` (`record`, `apply_delta`, `remove`, `settle_revocations` with revoke
  outcomes, `list` newest first), the extension's array of records under the policy's key. The
  log is plain local storage in the extension (`bramble:allowances`, `docs/WALLET_STORE.md`),
  neither vault nor backup, so no record crosses between mobile and the extension; the shape
  matches so the extension's own value reads unchanged in Phase 3. `discovery` is
  `decodeApprovalEvent`, `parseAllowanceAmount`, `mergeAllowanceViews` and the background's
  `outstandingAllowances` and `listingNotices`; `scan::ExternalAllowanceScan` is
  `collectExternalAllowances` and `tracked::TrackedAllowanceRead` is `collectTrackedAllowances`
  (with `tracked_pairs`, `readTrackedAmounts`' lower-bounds-first order), both one call at a
  time as the extension awaits them, under `HistoryScan`'s `next_calls`/`accept`/`restart`/
  `finish` contract and `ScanStep`, judged by `portfolio::wire::classify`. Every bound, cap and
  the storage key are `AllowancePolicy`, set in `bramble-core`'s `ALLOWANCE_POLICY`; the
  extension has no known-spender labels. Departures: amounts are `U256`, so a stored amount
  above 2^256 - 1 is dropped on load (the extension's 78-digit rule keeps it; no extension
  writer produces one); a record on devnet is dropped (`NetworkId` has none); an event without
  a `keys` or `data` array, or a continuation token over `HistoryScan`'s 4 KiB, fails over
  where the extension's handling varies by shape; a tracked pair whose token or spender is not
  a non-zero felt is skipped rather than sent; a stored value that is not JSON reads as empty,
  as a non-array does; the mutation queue is `&mut self`. The listing's orchestration stays the
  wallet's, as in `allowance-ops.ts`: which scopes (the account and the multisigs it signs
  for), dropping watched NFT contracts, reading revoke outcomes before readings, settling, then
  merging.
  The revoke and the write side followed (#89; mechanism only, not exported):
  `allowances::calls` is `allowances.ts` and the background's `allowance-recording.ts`
  (`decode_allowance_call`, `approved_asset_kind`, `allowance_changes`, `record_submitted`
  and `record_changes` under an `AllowanceOwner` that names the network, best effort per
  change, and `standing_allowance_totals`, the one copy the reviewer's unlimited-approval
  rule reads, #96: fungible contracts only, the account id compared canonically); `allowances::revoke`
  is `allowanceTarget` and `handleRevokeAllowance`'s rules: `RevokeTarget::resolve` (the id
  grammar and 240-character cap, a record this account owns on this network or an id naming
  its own pair, the watched-collection refusal), `confirmation_pair` (`isRevokableErc20`'s
  `allowance(owner, spender)` read, handed to the host as one `TrackedAllowancePair`), and
  `invoke`, which holds the account to the binding it was resolved for, checks collection and
  fungibility again, and hands `approve(spender, 0)` to `send::invoke::Invoke::reviewed` with
  no pinned contract, as `verifyCalls([call], trusted)` (so a token known only from the read
  still warns as an unknown contract); the activity line is the review's summary, and the
  zero approve rides the pending record to the log, which marks the grant `revoking`. `revoke_outcomes` is the background's `revokeOutcomes`
  over `ActivityEntry` rows (this wallet's own rows in the revoking scope and network,
  canonical hash). The extension revokes ERC-20 allowances only; operator and NFT approvals are
  refused, not revoked. `settle_revocations` now indexes readings and outcomes once. Departures:
  the refusals are checked again at build time; a stored record whose network disagrees with
  its account is refused; message parsing, the fee round trip and the lost-reply rows stay the
  wallet's and the plan's. Remains: the UniFFI export (two step machines, the log and the merge
  would add several hundred lines, so it comes with mobile's screens) and a Bramble generator:
  `Approval` decoding in both layouts, a scan over a scripted node with legacy windows, the
  merge, eviction across tokens and scopes, the revoke call and settling.
- **P13 Staking.** Positions, the validator directory, calls and their review:
  `transactions/src/staking/` (1,062) and `background/staking/` (1,150). Needs P9.
  As built (#93; mechanism only, not exported; no generator yet, so unit tests only):
  `crates/wallet-core/src/staking/`. `decode` is `staking-decode.ts`: the staking contract's
  and a pool's replies, `None` for any shape off the layout, and `verify_staking_pool` (the
  staking contract lists the pool for the staker and token, and the pool's own parameters name
  the same staker, contract and token). `read::StakingRead` is `staking-reads.ts` over
  `readAtPinnedBlock`: the facts (active tokens, exit window, rewards in STRK), then each
  pair's staker and pool reads, all pinned to one accepted block, re-pinned on "Block not
  found" at most three times and replayed per endpoint, under `HistoryScan`'s contract. The
  `StakingReading` it yields is bound to its account and network, and every consumer refuses
  another's. `coverage`, `directory` and `positions` are Starkscan's coverage rule, validator
  directory and positions as requests and strict judges: body cap before parse, every array
  bounded before it is walked, malformed answers refused whole, the directory's own order
  kept. `view` is the overview and "Your stakes" rows (`staking-row-view.ts`), incomplete
  whenever a candidate is untracked, inactive, unverifiable or over the read cap. `calls` and
  `review` are `staking-calls.ts` and `staking-review.ts`. `prepare` is the rule half of
  `staking-action.ts`, `-inputs.ts` and `-preparations.ts`: input rules, then calls built from
  a verified reading and reviewed by `send::invoke::Invoke::reviewed` with the verified pool
  the one pinned contract (#96), frozen in
  `StakingPreparations` for one claim bound to the account and network and refused once
  staking is off. Bramble's choices are `bramble-core`'s `staking::STAKING_POLICY` (Mainnet
  only). The batch names its own activity row (`stakingActivitySummary`, the amount entered,
  and the rewards token for a claim or the staked one otherwise) and a stake carries its
  exact-amount approval of the pool for the allowance log. Departures: fee freezing
  and the execute-time trusted-set re-check are the plan's (as P9); a claim's row names the
  reading's rewards token where the extension hard-codes STRK; preparation ids are a counter and there is no time expiry
  (the extension has none); the website hostname is hand-parsed and stricter than WHATWG
  (display only); amounts are exact integers; a staker is read once per pinned read. Not
  ported: message envelopes, the shared Starkscan cooldown (`history::starkscan::cooldown_ms`
  serves), screens. Owed: a Bramble generator capturing a live directory page, a positions
  answer, a pinned read and each action's calls, and the UniFFI export with mobile's screens.
- **P14 Gasless and sponsored transactions.** `transactions/src/gasless/` (1,818) and
  `routing/{outside-execution-*,paymaster-http}.ts`: a SNIP-9 v2 outside execution the gateway
  builds and the wallet checks before signing (its calls, the fee transfer, the ceiling, the
  window), and the paymaster's bodies and replies. The extension checks the hash with
  starknet.js; Krusty 3.1.0 hashes SNIP-12 messages (`compute_typed_data_message_hash`), and
  whether the two agree on this message is for the fixture to show. The core adds no hashing of
  its own. Adds the sponsored and gasless activity rows P2 left out.
- **P15 STRK20 submission.** Activate, shield, unshield and private send; then shadow accounts
  and multisig STRK20. The largest step: `packages/features/privacy/` (9,279) and
  `background/privacy/` (5,872), over the read plan 2.4d ported. The extension proves through a
  vendored JavaScript SDK (`scripts/vendor-privacy-sdk.ts`); D3 settled that mobile embeds the
  same pinned SDK (Road to parity), and the spike below says how.

  **P15 spike findings (2026-11-23).** Read from Bramble `main` (`a01b240c`) and
  `starkware-libs/starknet-privacy` at the extension's pin (`b40bf109`); nothing was built or run.
  Paths are Bramble's (`privacy/src/` is `packages/features/privacy/src/`, `background/` is
  `apps/extension/src/background/`) unless they start `sdk/` (upstream) or `crates/` (here).
  - *Secrets.* The account private key never reaches the SDK. The SDK's one signing call is
    `signer.signTransaction([{pool, "compile_actions", calldata}], ..)` while it builds the proof
    invocation (`sdk/src/internal/proof-invocation-factory.ts:148-161`), and the extension's signer
    ignores the transaction details and signs the SNIP-12 `CallSet` digest it recomputes itself from
    the calls, chain and account (Poseidon over fixed type hashes;
    `privacy/src/policy/call-set-signer.ts:41-91`; every other signer method throws, :93-107)
    through the vault's Krusty boundary or, for a multisig, the threshold aggregate `signCallSet`
    (`privacy/src/sdk/official-sdk-transfers.ts:17-49`). That is the "protected signing": the key
    stays behind `signHash`. The STRK20 viewing key does reach the SDK, as hex, through
    `viewingKeyProvider.getViewingKey` (Krusty's `deriveScopedStrk20ViewingKey`, derived lazily with
    the unlock checked before and after, `background/privacy/strk20-account-context.ts:94-124`); the
    SDK holds it as a `bigint` it cannot zero and sends it in plaintext to the discovery service
    (`viewing_key` in `/v1/sync/*` bodies, `sdk/src/internal/indexer-discovery.ts:162-378`) and to
    the prover inside `compile_actions` calldata `[user, viewing_key, actions]`
    (`sdk/src/internal/proof-invocation-factory.ts:132-136`), which the extension accepts
    (`privacy/src/sdk/official-sdk-preparation.ts:128-134`). OHTTP hides it from the relay, not from
    the gateway. Note randomness and salts are made inside the SDK (`sdk/src/utils/crypto.ts:80-94`,
    `getRandomValues`); channel keys and nullifiers are Poseidon of the viewing key
    (`sdk/src/utils/hashes.ts`). The extension's boundary
    (`background/privacy/strk20-approval-execution.ts:5-8`: "No SDK private-state operation,
    protected signing, or proving happens before this boundary") re-resolves the account, grant and
    network after consent, refuses Ledger accounts
    (`background/privacy/strk20-op-context.ts:247-256`) and hands the side panel neither key nor
    proof. On a phone, then: the account key and the signature stay in Rust; the viewing key, the
    actions and the SDK's note set cross into the runtime, and the viewing key leaves the device in
    any design.
  - *SDK surface.* The extension calls `createPrivateTransfers({account: {address, signer},
    viewingKeyProvider, discoveryProvider, provingProvider: {url, chainId, nodeUrl,
    requestTimeoutMs, retry, ohttp?}, poolContractAddress, shadowAccountAnonymizerAddress?})`
    (`privacy/src/sdk/official-sdk-transfers.ts:63-85`), and on its result `discoverNotes`,
    `discoverRequirement` (`SetupRequirement`: Register, SetupChannel, SetupToken, Ready, of which
    it reads only Register), `build({autoDiscover: refresh, autoSetup: true, autoSelectNotes:
    "naive", ..})` with `register`, `with(token).deposit|transfer|withdraw`, `surplusTo`, `invoke`,
    `shadowAccounts(scope).{partialCommitment, commitment, invoke}`, then
    `createProofInvocation({provingBlockId})` and `executeWithInvocation`, which returns
    `{callAndProof: {call: apply_actions, proof: {data, output, proofFacts}}, warnings}`
    (`privacy/src/sdk/official-sdk-builder.ts`, `official-sdk-preparation.ts:101-164`). `Open` is an
    amount symbol. `simulate` is never called (it would post the key-bearing `compile_actions` to
    the user's node). The SDK submits nothing: the wallet sends `[approve(pool fee), apply_actions]`
    from the account, or relays it through the paymaster. Two callbacks only: the signer and the
    viewing key; `fetch`, the provider and storage are not injectable.
  - *Network.* The SDK fetches by itself: the discovery service (also during `build`, for note
    selection), the prover (`starknet_proveTransaction`,
    `sdk/src/internal/proving-service.ts:210-290`), the RPC at `nodeUrl` for the pool's nonce
    (`sdk/src/internal/proving-service-provider.ts:89-114`), and with OHTTP on, `GET
    {gateway}/ohttp-keys` (trust on first use unless pinned) and the relay
    (`sdk/src/internal/ohttp-client.ts:89-149`). OHTTP is off in production
    (`privacy/src/config.ts:109-114`), and discovery, prover and paymaster all sit behind the
    wallet's gateway `https://wallet.nodes.starknet.org/api/strk20/*`
    (`privacy/src/config.ts:111-123`). The extension contacts itself: maturity and readiness reads,
    the pool fee, registration and public key, `get_shadow_accounts`, the submission, and the
    paymaster (quote, relay, sponsored shield and deploy, its SNIP-9 message signed by the wallet,
    never the SDK signer). It cannot fence the SDK's own fetches beyond a global wrapper and its CSP
    (`background/index.ts:230-234`). On mobile the wallet's half becomes sans-IO requests in the
    core, as `Plan` already is, and the SDK's half stays in the runtime but goes out through a
    native fetch bridge that admits only the route's discovery and prover origins and the network's
    RPC: a fence the extension lacks.
  - *Runtime.* The bundle is one ESM file (esbuild `es2020`, browser, starknet.js inside,
    `sdk/scripts/build-browser.ts:25-36`), checked by sha256 `b3d4c41a…c9c5` and its source map by
    `d89b6eeb…5e57` (`scripts/check-privacy-sdk.ts:34-42`), loaded statically by the MV3 worker
    (`background/index.ts:30`). It needs `fetch`, `AbortSignal.timeout`, `setTimeout`,
    `crypto.getRandomValues` and `BigInt`; no WebAssembly, workers or storage. With OHTTP on it also
    needs `DecompressionStream` and WebCrypto X25519, HKDF and HMAC-SHA256 and AES-128-GCM
    (`hpke@1.0.6` and `ohttp-ts@0.3.0` are pure WebCrypto; upstream's non-WebCrypto PRs #759 and
    #767 closed unmerged). Bundle size is not recorded (source 344 KB before starknet.js).
    *Recommended*: a hidden WebView on both apps (`WKWebView` with a non-persistent data store;
    Android `WebView` through `WebViewAssetLoader`), loading the same digest-checked file from the
    app's resources, never the network; a page whose CSP is `default-src 'none'` with only that
    script; navigation refused; `fetch` replaced by the bridge above before the module loads;
    requests and the viewing key passed over `WKScriptMessageHandlerWithReply` and `WebMessagePort`.
    Reasons: the bundle runs as the extension runs it, OHTTP included, with no polyfill; iOS runs it
    out of process (Android's renderer is sandboxed too), so the SDK's heap is never the app's;
    nothing ships beyond the system engine. JavaScriptCore or a bundled Hermes or QuickJS would need
    `fetch`, `AbortSignal` and `DecompressionStream` shims, and for OHTTP an X25519 HPKE in
    JavaScript: wallet crypto outside Krusty, which this repo refuses. Costs: Android's WebView
    updates apart from the app, so the app re-runs the extension's export-shape check
    (`check-privacy-sdk.ts:156-213`) at start and fails closed; which OS versions give WebCrypto
    X25519 is unconfirmed (it matters only once OHTTP is on). Bundled, never downloaded, code is
    what App Store guideline 2.5.2 and Play's device-abuse policy ask (to confirm at review).
  - *Overlap.* The core's read plan (`crates/wallet-core/src/privacy/`) already sends the SDK's
    preflight and incoming discovery bodies byte for byte (`bodies.rs`) and folds its note
    accumulation into totals (`notes.rs`), with the pool fee; it refuses OHTTP routes (`config.rs`).
    Mobile keeps the core's read for balances and registration: it is ported, proved and bounded,
    and holds no note. The SDK's own discovery stays inside `build`, which selects notes from it.
    When OHTTP turns on, the core's read cannot follow without an OHTTP client of its own, so
    balances would move into the engine; not needed while it is off.
  - *Rust path.* No StarkWare Rust client: `crates/discovery-core` (hashes, note decryption, storage
    slots) and `crates/discovery-service` (the axum indexer) are the server's; no roadmap, changelog
    or issue mentions one (#956, a third party's Dart client asking for prover access, is
    unanswered). A port needs Poseidon (here already, through `starknet-types-core`), the selector
    (krusty-kms#257), Pedersen contract addresses for shadow accounts (Krusty's
    `calculate_contract_address`), and for discovery decryption and the simulator Stark-curve ECDH
    with point-from-x, which Krusty does not export; whether the client path needs ECDH at all is
    unconfirmed (note encryption appears to run in Cairo, in `compile_actions`). OHTTP would need
    HPKE X25519, also not in Krusty. Plus the action compiler and invocation builder, the bulk of
    the port.
  - *Licence and pin.* The repository is Apache-2.0 with no NOTICE; `sdk/package.json` says ISC (a
    PR to align it, #933, closed unmerged). The bundle also carries MIT code (starknet.js, noble,
    scure, hpke, ohttp-ts, zod), whose notices the apps must ship. To be confirmed by legal before
    release. The pin is the version-bump commit after tag `PRIVACY-0.14.3-RC.8` (`fcc7c90`);
    upstream is at `PRIVACY-0.14.4-RC.2` (2026-11-17, `f7fb72a`, 7 files in `sdk/src`), which moves
    zod to 4 for an advisory 3.x has no fix for (unverified here). Mobile takes the extension's pin
    and digests unchanged and moves only when the extension does.
  - *The trait.* `wallet-core`'s `privacy::engine`, implemented by each app over its WebView; plain
    records only, one job per proof: `fn invocation(&self, job: EngineJob) -> Result<SignRequest,
    EngineError>` and `fn prove(&self, job: JobId, signature: Vec<Felt>) -> Result<EngineProof,
    EngineError>`, plus `fn shadow_commitment(&self, scope, nonce: Option<Felt>) -> Result<Felt,
    EngineError>` and `fn cancel(&self, job: JobId)`. `EngineJob` is the chain, pool, account, the
    reviewed actions (`Register`, `Deposit`, `Transfer` with an amount or `Open`, `Withdraw`,
    `SurplusTo`, `Invoke`, shadow `Invoke`), the proving block, the route's URLs, and the viewing
    key in a zeroizing buffer, its one crossing. `SignRequest` is the calls the SDK asked to sign;
    the core checks they are the pool's one `compile_actions`, recomputes the `CallSet` digest
    (Poseidon here; the selector through `StarkPrimitives`) and signs with Krusty or gathers the
    multisig aggregate, so no signing callback reaches the runtime (the app holds the SDK's signer
    promise open between the two calls). `EngineProof` is the `apply_actions` call, `{data, output,
    proof_facts}` and the warnings, which the core judges as `official-sdk-results.ts` does
    (`USER_LINKAGE` a privacy leak, any other warning refused, bounds from `STRK20_LIMITS`). The
    engine holds the viewing key and notes; the core holds the account key, the policy, the review,
    the batch and its submission.
  - *PRs.* Six, after a generator for each (CallSet digests, result judging, each action's batch):
    (1) core: the trait, records and a test engine, the digest and signing, result judging, the
    extension's rules around the SDK (re-resolve after consent, Ledger refused, foreign
    registration, recipient policy, maturity, wait budgets), with Bramble's runtime host (WebView,
    bridge, vendored bundle and its digests) beside it; (2) activate and (3) shield, whose
    `[approve, apply_actions]` batch goes through `Plan` and so waits on X2; (4) private send and
    unshield, relayed through the paymaster and its SNIP-9 message, so after P14; (5) shadow
    accounts, whose scope is a dapp's origin, after P16; (6) multisig STRK20, after X1. Unknown
    until built: the bundle's size, its cold start in a WebView, and whether proving's 180 s budget
    survives an app in the background.
- **P16 Dapp connect, approvals and anti-phishing.** The core takes request validation, grants
  (the vault's `originGrants`, P7), typed-data and invoke review (P9), add-chain and watch-asset,
  and the blocklist and phishing feed: `background/provider/` (2,453), `background/approvals/`
  (4,169), `packages/features/permissions/` (905). The transport is Bramble's and differs by
  platform: mobile has no injected provider, and #990 leaves WalletConnect open.

Bramble's alone, with nothing needed here: Android's token management, its multisig stepper,
participant picker and shared-account home, the analytics consent and diagnostics screens, and
the screens for what the core already has (hidden accounts, the activity facts).

Not parity:

- NFTs are parked in the extension, and no client scans a QR code.
- Devnet (`SN_DEVNET`, a developer's local node), and the custom chains a dapp once added
  (`wallet-model/src/networks/custom-network.ts`; nothing adds one now). P4's RPC override is the
  extension's whole custom-RPC feature.
- Mobile's stricter relay rule (no `ws:` on loopback) is deliberate and stays. Its backup order
  (the wallet is saved before the backup check, where the extension checks first) is a
  difference #990 lists and nobody has decided; it is Bramble's to settle, and needs nothing
  here.
- Two of #990's smaller differences are decided with P1's export: whether a Braavos key can be a
  multisig co-signer on mobile, and the class check before a multisig is created
  (`accounts::multisig_declared` already reads it).

## Generic and Bramble

`wallet-core` holds mechanism; `bramble-core` holds Bramble's choices and the FFI. The rule: a
product decision is an **argument** in `wallet-core` and a **constant** in `bramble-core`. No
traits or config objects for wallets that do not exist yet.

| `wallet-core` (plain Rust, no FFI) | `bramble-core` (UniFFI, ships to npm, Swift, Kotlin) |
| --- | --- |
| felt grammar; mnemonic generation and validation | `OZ_ACCOUNT_VERSION = "3.0.0"`; `oz_account_class_hash(network)` passes it in |
| Stark and Ready (Argent legacy) derivation, signing, STRK20 viewing key | which of those cross the FFI (`raw-keys`) |
| OZ address derivation with a **required** salt | `KmsPort`'s optional salt (omitted = public key, Krusty's default); OZ accounts pass `0x0` from `account-contract.ts` |
| OZ class hash at a caller-given manifest version | `CoreError`, `core_info()`, remote exports of `wallet-core`'s records |
| Nostr key derivation, NIP-59 wrap/open, event grammar and resource bounds | parity fixtures and tests against the extension |
| Later phases: vault container (envelope v3), storage and clock traits, sans-IO RPC, transaction hashing, transfer planning, portfolio/history/privacy logic, Nostr relay paging and gift-wrap batch checks | Later phases: the extension's vault schema (v13), UniFFI foreign traits adapting to `wallet-core`'s, catalogs, multisig coordination policy (identifier, relays, sync window, who counts as a co-signer), iOS/extension-only assets |

The resource bounds (mnemonic, passphrase, Nostr event and data sizes) are Bramble's numbers, kept in
`wallet-core` because they protect against oversized input rather than express product taste.
So are `container.rs`'s `bramble.mobile.*` format and scope strings: they are bytes on disk, bound
into every sealed slot, and changing them orphans every mobile vault. A second wallet wanting its
own would make them arguments then.

`bramble-core` keeps the crate, lib and UniFFI module names mobile already uses (`bramble_core`,
`BrambleCore`, `org.bramble.core`), so the last mobile step (2.7) swaps the dependency without
touching Swift or Kotlin imports. A second wallet would add its own `<wallet>-core` crate the same
way, or use `wallet-core` from Rust directly.

## Layout

```
Cargo.toml                 workspace; release profile (lto, panic=abort, strip)
rust-toolchain.toml        pinned channel, wasm32-unknown-unknown target
.cargo/config.toml         getrandom backend cfg for wasm32
deny.toml                  licences, advisories, sources
.gitattributes             golden fixtures checked out byte-for-byte
AGENTS.md                  contributor and agent rules (CLAUDE.md points to it)
tools/check.sh             the one check entrypoint: CI, hooks and agents run its modes
.githooks/                 pre-commit (quick), pre-push (lint + test)
crates/wallet-core/        generic: package starknet-wallet-core, lib starknet_wallet_core
  src/lib.rs               Error, KRUSTY_VERSION, re-exports
  src/felt.rs              strict lowercase-hex felt grammar
  src/kms.rs               key custody over Krusty, account keys per ladder
  src/accounts/            ladders, account families, family addresses and constructor calldata
  src/nostr.rs             coordination identity, NIP-59 wrap/open
  src/envelope.rs          the extension's EncryptedEnvelope v3 (PBKDF2-SHA-256, AES-256-GCM)
  src/container.rs         vault container: DEK, passphrase and biometric slots, sealed payload
  src/host.rs              host-provided Storage and Clock traits, ClockReading
  src/rpc.rs               sans-IO JSON-RPC: envelope, chain id, balanceOf, amounts, u256 text
  src/send/                transfers: calls, fees, judge, rpc_bodies, sign (v3 hashes), u256, and
                           the sans-IO Plan with its PlanHost
  tests/vault_*transitions.rs  vault transitions from the Quint vault spec
crates/bramble-core/       Bramble: lib bramble_core, the cdylib/staticlib/wasm32 module
  src/lib.rs               UniFFI scaffolding, CoreError, core_info(), Bramble constants, exports
  src/bin/uniffi-bindgen.rs  Swift/Kotlin generator (`bindgen` feature)
  uniffi.toml              Swift/Kotlin module names, as mobile's
  tests/parity.rs          extension fixtures and known-answer vectors
  tests/vault_parity.rs    envelope and DEK slot the extension sealed
  tests/accounts_parity.rs  every family, ladder and class the extension derives
  tests/transactions_*_parity.rs  send rules; every body, hash and signing; the plan over every capture
  tests/fixtures.rs        holds tests/fixtures/ to its README manifest (sha256 per file)
  tests/fixtures/          copied from Bramble; README is the manifest
js/                        npm package @starknet-innovation/bramble-core: ubrn config,
                           entrypoint, smoke test over the built wasm
.github/workflows/         ci.yml (rust + wasm jobs), release.yml (tag → npm, with provenance),
                           publish-crates.yml (tag → crates.io, starknet-wallet-core then
                           bramble-core),
                           advisories.yml (weekly `cargo deny check advisories` on main)
quint-specs/               Quint models: vault, send, the bramble-core FFI
```

Features (on `bramble-core`): `raw-keys` (default) exports the functions that take or return a raw
Stark private key, which the extension's vault needs until Phase 3.2; `extension` (default,
implied by `raw-keys`) exports the extension's other KMS and Nostr functions, several of which take
or return a recovery phrase; mobile builds `--no-default-features --features mobile`, so neither
reaches its bindings. `mobile` adds the mobile wallet's surface (2.7), and
`testing` (implies `mobile`) its integration-test hooks. `bindgen` adds the Swift/Kotlin generator
binary. `wallet-core` has no features.

## Phases

Each phase names the crate a module lands in; the table above decides anything not named.

0. **Scaffold** (done): the layout above, KMS and Nostr ported, native and wasm32 builds, parity
   tests in Rust and through the built wasm under Node, CI and release workflows. Split into
   `wallet-core` and `bramble-core` the same day; the exported API did not change.
1. **Golden suite** (done): every fixture the extension generates for mobile, 22 files from
   Bramble `0a4bd84c`, copied into `crates/bramble-core/tests/fixtures/` under the manifest
   described in Golden tests and hash-checked by `crates/bramble-core/tests/fixtures.rs`. The
   hand-cut `nostr-parity.json` subset gave way to the whole `nostr-multisig-parity.json`.
   Bramble writes them into `mobile/core/tests/fixtures/` with `bun run mobile:parity`, which also
   runs the transaction and privacy generators:
   - KMS, accounts and a sealed envelope: `extension-parity.json`, `accounts-parity-*.json`.
   - Nostr multisig: `nostr-multisig-parity.json`.
   - Portfolio, tokens, history rows: `portfolio-parity.jsonl`.
   - Privacy: `extension-privacy-*.json`.
   - Transactions: `extension-transactions-*.json`.

   Bramble's `mobile/core/tests/fixtures/` also holds fixtures that are *not* extension output;
   they are not golden and were not copied: `vault-v1.json`, `vault-v2-*.json`,
   `vault-v3-container.json` and `migration-v1-to-v5.golden.json` were written by earlier mobile
   cores and pin mobile's own migration chain; `ui-coordination.json` drives the iOS UI suite;
   `rust-gift-wrap.json` runs the other way (Rust writes it, the extension opens it). Those move
   with their modules as ordinary regression fixtures. The golden fixtures for modules not yet
   ported sit in this repo unused until their phase: `portfolio-parity.jsonl`,
   `extension-privacy-*.json`, `extension-transactions-observation.json` (2.4), and
   `accounts-parity-recovery.json` and `accounts-parity-rules.json` (Bramble's wording and rules,
   2.7). Where the
   extension has behaviour mobile ports but no generator covers it, the generator is added in
   Bramble first.
2. **Mobile adoption** (`feature/mobile-wallet`), one module group at a time. Mobile's crate is
   also named `bramble-core`, so until the last step it cannot depend on this repo's
   `bramble-core`; it depends on `wallet-core` (plain Rust, no UniFFI, so no second scaffolding
   in the binary) at a tag. Each step moves the module's mechanism from `mobile/core` into
   `wallet-core` per the Generic and Bramble table, and its Rust parity test
   (`tests/*_parity.rs` in `mobile/core`) into `crates/bramble-core/tests/`, reading the golden
   copies from Phase 1; `mobile/core`
   takes the new tag, deletes its copy and keeps Bramble's constants and exports. From then on,
   mobile work on that mechanism is a PR here.
   1. **KMS, Nostr, felt** (already in `wallet-core`). Mobile bumps Krusty 0.11 → 2.0.0 across
      the whole crate and then takes `kms.rs`, `nostr.rs` and `felt.rs` from `wallet-core`.
      Mobile has no users yet, so Krusty 2.0.0 is taken without a separate diff review: the
      golden fixtures are the check that mobile derives and signs as the extension does.
      Steps, in order:
      1. **Krusty bump in mobile** (one PR on `feature/mobile-wallet`; needs nothing from this
         repo). Tried on a copy of `0a4bd84c`: with `krusty-kms` and `krusty-kms-common` at
         `=2.0.0` and `bip39` at `=3.0.0` the crate builds with no source change, clippy is
         clean, and 369 tests pass (3 network tests ignored) with every fixture byte-identical.
         The PR:
         - `Cargo.toml`: `krusty-kms` and `krusty-kms-common` `=0.11.0` → `=2.0.0`; `bip39`
           `=2.2.2` → `=3.0.0`, the version Krusty 2.0.0 uses, so the lockfile holds one copy
           (`ui.rs`'s word assist needs no change). `starknet-types-core` stays `=0.2.4`.
           `cargo tree -d` lists no second `krusty-kms`, `bip39` or `starknet-types-core`. The
           comment above the pins names `mobile-core-pins.test.ts`, which does not exist; it
           should name `mobile-generated.test.ts`.
         - `src/lib.rs`: `krusty_version: "2.0.0"`.
         - Bramble's `tests/conformance/mobile-generated.test.ts` ("the Rust core pins the same
           Krusty release as the extension"): drop the npm-pin equality, which the extension
           keeps at 0.11 until 3.1, and check instead that `Cargo.toml`'s pin equals
           `lib.rs`'s `krusty_version`. Step 3 points it at `wallet_core::KRUSTY_VERSION`.
         - Comments and docs naming 0.11.0 as mobile's Krusty: `send/mod.rs`, `send/sign.rs`,
           `privacy/mod.rs`, `argent_multisig.rs`, `kms.rs`, `mobile/SCOPE.md`; and
           `scripts/generate-mobile-parity-vectors.ts`, whose header says to bump Krusty in
           `package.json` and `Cargo.toml` together (now: re-run after bumping either).
         - `rust-gift-wrap.json` regenerated (`BRAMBLE_WRITE_FIXTURES=1 cargo test --test
           nostr_multisig_parity`), so `mobile-nostr-parity.test.ts` opens a wrap Krusty 2.0
           made with the extension's 0.11. Without it the test opens the old 0.11 wrap and
           proves nothing about 2.0. It is a regression fixture, so regenerating it is allowed.
           Tried: the regenerated wrap opens in the extension's test, and a tampered one fails.
         - Checks: `cargo test`, clippy with `-D warnings`, `bun run check` in Bramble, and an
           iOS and Android build. Swift and Kotlin bindings regenerate with no diff.
         - If a golden fixture ever fails here, Krusty changed behaviour: stop and report it
           upstream; never edit the fixture.
      2. **Nostr parity gaps in `wallet-core`** (done), found by comparing mobile's `nostr.rs`
         and `kms.rs` with the core:
         - `open_nostr_event` opens a gift wrap from a parsed `NostrEvent`, re-serialised as the
           extension's `JSON.stringify(event)` and re-checked against the event grammar.
           `open_nostr_application_data` (event JSON) is now a wrapper over it.
         - `stark_public_key` takes a `SecretFelt`, for mobile's `scoped_viewing_public_key`
           (recovery compares it with the key a privacy pool holds).
         - Parity tests: the parsed open against `nostr-multisig-parity.json`'s gift wrap and
           its refused variants; the public key against `extension-parity.json`'s `publicKeyX`.
           Still open, in Bramble: the privacy fixtures hold no viewing *public* keys, so the
           composed `previewStrk20ViewingPublicKey` has no golden vector. Add one to the privacy
           generator, and test against it once it is copied here.
      3. **Mobile takes the core** (M2) at `v0.1.0`, tagged at `35abc8b`. That tag makes a
         GitHub release only: `release.yml` skips npm while `js/package.json` is private, which
         it stays until Phase 3.1. Mobile pins `wallet-core` by `git` and `tag`
         (`wallet-core = { git = "https://github.com/starknet-innovation/wallet-core", tag =
         "v0.1.0" }`); `Cargo.lock` records the commit. In `Cargo.toml`, `sha2` moves `=0.11.0`
         → `=0.11.1`: the core pins `=0.11.1`, and two exact pins in one semver range do not
         resolve. Every other shared pin already agrees. `krusty-kms` and `krusty-kms-common`
         stay direct dependencies at the core's pin, because `kms_accounts.rs`,
         `argent_multisig.rs`, `account_catalog.rs`, `networks.rs` and `send/` still name
         Krusty; `cargo tree -d` must list neither twice. Mobile's `CoreError` gains
         `From<wallet_core::Error>`, variant for variant as `bramble-core`'s here. From the tag
         after `v0.1.0` the package is `starknet-wallet-core`; mobile keeps every `wallet_core::`
         path with `wallet-core = { package = "starknet-wallet-core", git = …, tag = … }`.
         `felt.rs` becomes `pub(crate) use wallet_core::felt;`, `nostr.rs` maps onto the core
         (`ApplicationData` → `NostrApplicationData`), and `kms.rs` keeps only mobile's glue
         (`Ladder` and its serde names, `normalise_phrase`, error mapping, `OZ_ACCOUNT_VERSION`)
         until 2.7. `krusty_version` reads `wallet_core::KRUSTY_VERSION`. The KMS half of
         mobile's `parity.rs` and the key and gift-wrap tests in `nostr_multisig_parity.rs`
         are deleted, since `crates/bramble-core/tests/parity.rs` covers them;
         `privacy_key_boundary.rs` is pointed at where derivation now lives. By name: from
         `parity.rs`, `accounts_and_signatures_match_the_extension`; from
         `nostr_multisig_parity.rs`, `coordination_keys_match_the_extension` and
         `opens_the_extension_gift_wrap_and_refuses_its_variants`. Its
         `rust_gift_wraps_round_trip_and_the_committed_one_still_opens` stays: it writes
         `rust-gift-wrap.json`, which the extension's `mobile-nostr-parity.test.ts` opens.
         `felt.rs` here and in mobile export the same functions, so its 24 importers do not
         change. The conformance test from step 1 now checks that `Cargo.lock` resolves one
         `krusty-kms` equal to the `Cargo.toml` pin and that `lib.rs` reads
         `wallet_core::KRUSTY_VERSION`. Checks as step 1; the Swift and Kotlin bindings
         regenerate with no diff, because nothing exported changes.

      What step 3 needs from this repo:
      - (done) `wallet-core`'s API was String in, String out, and `StarkKeypair.private_key` a
        plain `String`. Mobile works in `Felt` and holds keys in `Zeroizing`
        (`kms_accounts.rs` calls `kms::private_key`). `wallet-core` now borrows secrets (`&str`)
        and returns them zeroizing: phrases as `Zeroizing<String>`, private and viewing keys as
        Krusty's `SecretFelt` (re-exported, redacted `Debug`). `bramble-core` converts to `String`
        only at the FFI, behind `raw-keys`. The Nostr event grammar mobile's relay code uses
        (`NostrEvent`, `parse_event_json`, `event_from_value`, `has_recipient_tag`,
        `is_public_key`, `safe_non_negative_integer`, `kms::utf16_length`) is public.
        Mobile's implementation is still in flux, so where it differs from the extension it takes
        the core's behaviour rather than the core bending to it: its 256-byte passphrase bound
        becomes the extension's 1,024 UTF-16 units (`wallet-model/preferences/passphrase.ts`), and
        its `oz_class_hash` stops reading Krusty's `latest` and uses `OZ_ACCOUNT_VERSION` (3.0.0,
        `account-contract.ts`), so a Krusty bump cannot move its addresses.
      - (done) The Nostr parity gaps in step 2.
   2. **Vault and accounts**: `container.rs` (envelope v3, byte-compatible) and the vault
      mechanics into `wallet-core`, with plain Rust `Storage` and `Clock` traits; mobile's
      `files.rs` implements `Storage`. The v12 schema and migrations are Bramble's and stay in
      `mobile/core` until 2.7; mobile's vault moves to the extension's schema after it (Vault switch).
      In `wallet-core`: `envelope.rs` (mobile's `vault.rs`), `container.rs`, and `host.rs`
      (`Storage`, `Clock`, mobile's `ClockReading`). The vault version is an argument (mobile's
      `VAULT_VERSION` stays Bramble's), and so is the passphrase rule: `SealingKey::create` no
      longer calls `check_password`, so mobile checks first. Mobile's `HostClock` adapts to
      `Clock`, and `migrations_tests.rs` uses `SealingKey::from_parts` where it used `fixed`.

      Mobile's side (M3):
      - `vault.rs` keeps only Bramble's passphrase rule (`check_password`,
        `MIN_PASSWORD_LENGTH`) and re-exports `wallet_core::envelope`. `container.rs` becomes a
        re-export of `wallet_core::container`, with `parse(text)` → `parse(text, VAULT_VERSION)`.
      - `clock.rs` keeps the UniFFI `HostClock` foreign trait and implements
        `wallet_core::host::Clock` for it; `ClockReading` comes from the core. `files.rs` gains
        a `Storage` over `write_atomic` for the container's `read` and `write`.
      - `SealingKey::create` no longer checks the passphrase. Its one production caller,
        `wallet.rs`'s `create_vault`, is reached only from `create_wallet_on_network` and
        `import_wallet`, which both call `check_password` first. The check moves into
        `create_vault`, so a third caller cannot skip it; `testing.rs`'s two hooks call it
        too. A test proves a passphrase under the minimum is still refused on create and on
        import.
      - `SealingKey::fixed` → `from_parts` in `container.rs`'s tests and `migrations_tests.rs`.
      - `parity.rs` loses `opens_an_envelope_the_extension_sealed` and
        `opens_a_dek_slot_the_extension_sealed` (`vault_parity.rs` here covers them) and, after
        M2, is empty and deleted. `vault-v1.json`, `vault-v2-*.json`, `vault-v3-container.json`
        and `migration-v1-to-v5.golden.json` stay and must pass untouched: they are the proof
        the swap changed no byte on disk.
      - Not in M3: the mobile vault → v12 migration.

      **Accounts** (A1, A2 here; M5 in mobile). Mobile's account mechanism is `kms_accounts.rs`
      (198 lines: family addresses through Krusty), `recovery_scan.rs` (499: the sans-IO
      discovery scan), `recovery_advisory.rs` (402: the scan's signer and privacy-pool reads)
      and the rule half of `recovery.rs` (499, of which scan shapes and the keep rule are
      mechanism and the rest is row wording and restore). It must move before 2.7 because
      `kms_accounts.rs` names Krusty and `bramble-core` may not. The split, by the
      argument-or-constant rule:

      | Into `wallet-core` (`src/accounts/`) | Stays Bramble's (mobile until 2.7, then `bramble-core`) |
      | --- | --- |
      | `Ladder` (`Account`, `Address`, `ArgentX`) with `kind()`, `vendor(family)` and `fits(family)`; `kms::account_key(phrase, passphrase, ladder, index)` (mobile's `kms::private_key`) and its public key | `Ladder`'s serde names (the vault spells them) and UniFFI derive, through `#[uniffi::remote]` |
      | `AccountFamily` with the extension's wire spelling | `accountFamilyName` ("Bramble", "Ready", "Braavos") |
      | `family_address(family, class_hash, argent_shape, salt, public_key)` and `constructor_calldata(..)` (the same placement, for a deployment): class and salt **required**; Krusty builds the calldata and address; an Argent class Krusty lists must have the layout the caller names, and one it does not list is refused unless the caller passes `Unlisted(shape)` | `resolve_class` (OZ at `OZ_ACCOUNT_VERSION`, `argent_default`, Braavos has none), salt `0x0` for OZ and the public key otherwise; Xverse as the one `Unlisted` class |
      | `accounts::scan::Scan`, one per (fingerprint, network), `next(phrase, passphrase, &ScanConfig, manual)` / `submit(responses)`; judging reuses `send::judge::answer` rather than a third reply parser; `MAX_CANDIDATES` (an input bound, like the resource bounds) | `ScanConfig`'s contents: shapes in order, each with its `creatable` flag (`isCreatableCandidate`), `RECOVERY_SCAN_LIMITS`, the tokens whose balance counts as funded, the privacy pool per network |
      | `accounts::advisory`: which reads a live class needs, `Restriction` (Guardian, OwnerChanged, OwnerType, Release, StrongSigner) from the reads and the block time, `registered_elsewhere` (the own viewing key derived in the core); "signs for" reads the same `send::Authorization` the plan takes, so the class lists are not passed twice; the pool's `get_public_key` selector beside `plan_probe`'s | `RESTRICTION_COPY`, the privacy-pool note, scan status lines |
      | `rpc::Request { id, body, max_response_bytes, optional }` and `rpc::Response` (mobile's `rpc_request.rs` without UniFFI) | their UniFFI records, through `#[uniffi::remote]` |
      | | `RecoveryCandidate` and its wording (`candidateName`, `candidateTitle`, hints, selection keys), `derive_selections` (restore checks over vault records), `account_store.rs` |

      Each `wallet-core` file stays under 500 lines: `accounts/mod.rs` (ladders, families,
      addresses), `accounts/scan.rs`, `accounts/advisory.rs`, with tests beside them.

      - **A1, account addresses** (needs nothing). `Ladder`, `AccountFamily`, `account_key` and
        `family_address`. Test `accounts_parity.rs` here: every vector in both
        `accounts-parity-families-*.json` derives the extension's public key and address on its
        ladder, class and salt (mobile's `every_family_and_class_derives_the_extensions_address`
        and `split_family_fixtures_agree_on_the_phrase`), and `kms_accounts.rs`'s Xverse test
        (it deploys with Argent 0.4 calldata at its own address); its class-rules test stays in
        mobile with `resolve_class`. The test
        reads the class table from `generated/accounts.json` once F3 copies it; until then it
        holds the extension's layouts for the four Argent classes the families fixtures name.
        `parity.rs`'s `every_family_ladder_matches_the_extension` stays as it is: it checks the
        FFI's raw-key exports, which `accounts_parity.rs` does not reach. Done: `Ladder`,
        `AccountFamily`, `ArgentLayout` and `ArgentShape`, `family_address` and
        `constructor_calldata` in `accounts/mod.rs`; `kms::{account_key, account_public_key}`.
      - **A2, recovery scan** (needs A1, F3). `accounts::{scan, advisory}` and `rpc::Request`.
        Tests: `accounts_scan_parity.rs` over `accounts-parity-scan.json` (G3: shapes, creatable
        rule, scan target, keep rule, signing verdicts, registered elsewhere), and
        `accounts_scan_mechanism.rs`, mobile's `tests/recovery.rs` scan cases against a canned
        node (a quiet wallet, activity across families, a pause and retry, exact ids, a manual
        index, a network reset) plus `recovery_scan.rs`'s and `recovery_advisory.rs`'s unit tests.
        Tag after A2. Done, with these differences from the text above: `submit` takes the
        phrase, passphrase and config again (mobile read its own session; the scan stores no
        secret) and refuses another phrase or network; the config, candidates, keep rule and
        gap limit are in a third file, `accounts/scan_rules.rs`; the scan keeps mobile's
        class-hash judging as `scan::judge_class_hash`, built on `send::judge::answer`, because
        `send::judge::class_hash` reads any result as deployed and does not return the class;
        `ScanConfig` also carries the network, the reserved indexes and a `SaltRule` per shape;
        limits that would underflow or pass `MAX_CANDIDATES` in one batch are refused; a Ready
        shape under a BIP-39 passphrase fails the batch closed. The two Krusty-against-catalog
        tests are in `accounts_scan_parity.rs`. One fixture case is not reproduced exactly: a
        pool key spelled with more than 64 hex digits is read by the extension (`BigInt`) and
        counts as no answer here (the felt grammar); the verdicts in the fixture agree.
      - **M5, mobile takes it** (Bramble, needs A2's tag). `kms_accounts.rs` and
        `recovery_advisory.rs` are deleted; `send/wallet_ext.rs`'s `deployment` takes its Argent
        calldata from `constructor_calldata`; `recovery_scan.rs` keeps the UniFFI step record and
        status lines and builds `ScanConfig` from `account_catalog`, `networks` and the privacy
        route; `recovery.rs` keeps wording and `derive_selections`, whose `record_at` calls the
        core's `family_address`; `argent_preset` and `account_catalog`'s Krusty import go. The core
        places a Braavos class only if Krusty lists it as a deployment class
        (`BraavosAccount::try_with_class_hash`), and an Argent class only on a layout that agrees
        with Krusty's; `resolve_class` still decides which of those Bramble offers, a check
        mobile's `family_address` made itself. Its two
        Krusty tests (`krusty_constructors_agree_with_the_extension_table`,
        `open_zeppelin_class_is_krustys_on_every_network`) check Krusty against the catalog, so
        they move here into A2's tests, over F3's `accounts.json`.
        `accounts_parity.rs` loses the two family tests (here now) and keeps
        `discovery_rows_are_worded_like_the_extension`, `advisory_notes_are_worded_like_the_extension`
        and the names, labels and groups tests, which are Bramble's copy and rules and move in
        2.7. So `accounts-parity-recovery.json` and `accounts-parity-rules.json` stay unread
        here until 2.7. `tests/recovery.rs`'s restore cases stay: they exercise the vault write.
        Checks as M4; the Swift and Kotlin bindings regenerate with no diff.

      After M5, mobile names Krusty (`krusty_kms_common`) only for `ChainId` and `KmsError` in
      `kms.rs` and `networks.rs`, which 2.6 replaces with `wallet_core::NetworkId` and
      `wallet_core::Error`, and in `argent_multisig.rs` (2.5). Since M7 (2.4b) `networks.rs` uses the
      core's `NetworkId` and no longer names Krusty; `kms.rs` and `argent_multisig.rs` remain.
   3. **Sans-IO transactions**: `rpc.rs`, `send/*` and transaction hashing, Argent and Braavos
      paths, into `wallet-core`; Bramble's defaults (fee margins, which flows it offers) stay
      with the Bramble crate. Done:
      - `rpc.rs` whole, and from `send/` the mechanism modules `u256`, `fees`, `judge`,
        `rpc_bodies`, `sign` and `calls`, with the types they use folded into them. The chain id
        comes from `NetworkId::chain_id()` (Krusty) rather than mobile's network catalog;
        `js_trim` and `short_label` live in `send::calls` until 2.4 brings the `js/*` shims;
        `sign::Deployment::address()` is the counterfactual-address check mobile made with
        Krusty directly. `CoreError` gains `Rpc`.
      - The state machine (`plan*.rs`) as `send::plan::Plan`, step for step. What tied it to
        mobile's `WalletCore` comes in from outside instead: `PlanHost` (lent to each
        `advance`) owns the session check, signing (`sign(hash, renew)`), the activity log
        (`record_pending`, `observe`; a signed write's record is never dropped, whatever the
        node answers) and explorer URLs; Bramble's account catalog rides in `Authorization`
        (Argent classes and their shapes, Braavos signing classes and implementation), as the
        Phase 2.6 rule asks; and its numbers are a `Policy`.
        `bramble-core` holds them: `FEE_MARGIN_PERCENT` (50), `REVIEWED_FEE_HEADROOM` (3/2),
        `CONFIRMATION_DELAYS_MS`, together as `SEND_POLICY`, each checked against the rules
        fixture. `Plan` is `&mut self`; the FFI object's mutex and its poison-closes rule stay
        with the wrapper. What the plan signs it derives itself: `Transfer::new` validates the
        request and builds the calldata, summary and verifier flags (mobile's
        `prepare_transfer` mechanism), and `Plan::new` drops a deployment that does not
        reproduce the account's address. A zero `Headroom` denominator fails closed.
      - Tests, all over fixtures already here: `transactions_rules_parity.rs` (mobile's
        `send/fixture_tests.rs`, less explorer URLs and the activity store, which move with
        2.4); `transactions_wire_parity.rs` (every request body in the OpenZeppelin, Ready and
        Braavos captures rebuilt, every signed hash recomputed from its own fields, its bounds
        re-derived from a quoted estimate, every hash the extension signed accounted for);
        `transactions_plan_parity.rs` and `transactions_refusal_parity.rs` (mobile's three
        `transactions_*_parity.rs`: the plan drives all 20 captures and 18 refusals with a test
        `PlanHost` on the published phrase, in `tests/plan_support`, checking every hash is signed
        in the extension's order, plus checkpoints, a lock after review and a fee past the
        headroom); `transactions_plan_mechanism.rs` (mobile's `transactions_plan.rs` against its
        canned node: wrong chain, out-of-order replies, node errors, write transport failure,
        hash mismatch, poisoned recipient, the 200-poll deployment wait, unknown deployment
        state, a missing bulk-estimate entry).

      Mobile's side (M4): take `v0.2.1` (`wallet-core = { package = "starknet-wallet-core", git =
      …, tag = "v0.2.1" }`, with `zeroize` `=1.9.0` → `=1.9.1` and `serde_json` `=1.0.151` →
      `=1.0.154`, the core's pins), delete `rpc.rs` and those `send/` files and `use
      wallet_core::{rpc, send::*}`. Mobile's plan still drops the pending record of a write the
      node errored on (`store::discard` in `plan_deploy.rs` and `plan_submit.rs`); the core's
      does not (#25), so those calls go, the record stays until `observe` settles it, and the
      failure carries the hash. Mobile's tests that expect the record gone change with it.
      `recovery_advisory.rs` reads the same account state outside a plan; its imports from
      mobile's `send::plan_probe` become `wallet_core::send::plan_probe`, except
      `ARGENT_SIGNER_ARRAY`: it is a catalog string the core does not hold (`ArgentClass` carries
      `signer_array` as a flag), so it moves from mobile's `plan_probe.rs` into `account_catalog`.
      `TransferPlan` becomes a UniFFI object around `Mutex<Plan>` holding `Weak<WalletCore>` and
      the session generation; its `PlanHost` maps onto `plan_sign`,
      `session_generation`, `activity::store` (`PendingRecord` → `Entry`, direction "out",
      status "pending") and `history::explorer`; `sign` must refuse once the generation moved,
      as `plan_sign` does. `wallet_ext.rs` keeps preparing plans (vault records, contacts,
      catalogs), building `Authorization` from `account_catalog` and calling `Transfer::new`.
      Records keep their UniFFI
      derives through `#[uniffi::remote]` until 2.7. Mobile's wallet-level tests
      (`transactions_lifecycle.rs`, `transactions_block_pin.rs`, and the contact, tracked-token
      and locked-prepare cases of `transactions_plan.rs`) stay there: they exercise its locks,
      store and address book through that wrapper.
   4. **Portfolio, history, privacy reads**: `portfolio/*`, `history/*`, `activity/*`,
      `privacy/*` into `wallet-core`, with the `js/*` JavaScript-semantics shims beside whichever
      code calls them; the iOS `include_str!` in `history/appearance.rs` moves to the Bramble
      crate's `generated/`. At about 11,200 lines this is the largest step, larger than 2.1–2.3
      together, so it goes in four parts, each a port here and an M PR in mobile, in the order
      the imports allow:
      1. **2.4a Shims** (1,638, plus `history/time.rs`, 196): `js/*` (Big, Number, JSON, regex)
         into `wallet-core::js`, with `send::calls::{js_trim, short_label}` moving beside them,
         and `history/time.rs` (no crate imports), whose `format_saved_data_label`
         `portfolio/snapshots.rs` uses. Unit tests only; their golden coverage comes through the
         later parts. Done (#43): the eight `js` files and `time.rs` are byte copies of
         mobile's; `short_label` joined `js::num`, and `send::calls` re-exports both names. M6
         (Bramble #922) makes mobile's `js` and `history::time` re-exports and points its
         private `js_trim` and `short_label` copies at the core.
      2. **2.4b Portfolio** (2,958): `portfolio/*`, plus `history/explorer.rs`'s
         `is_account_id_for_network`, which `snapshots.rs` uses and which itself imports
         `portfolio::model`, so it cannot go earlier. Oracle `portfolio-parity.jsonl`; tests
         `portfolio_parity.rs`, `token_parity.rs`, `portfolio_state.rs`. It reads `networks` six
         times: this is where the 2.6 rule first applies, so define the network and token
         records in `wallet-core` here, once, as arguments, and every later part reuses them.
         Done (#44): `catalog.rs` holds `NetworkInfo`, `TokenInfo`, `TokenRegistry` and the
         borrowed `Catalogs`, plain records with no serde (Bramble parses its own JSON), and
         `NetworkId` gains the extension's spelling (`catalog_id`). `PriceBook`,
         `BalanceSnapshots` and `TokenPreferences` are plain values with `&mut self`; mobile
         keeps the FFI objects as wrappers around a lock. `strk_address` answers `None` where
         mobile panicked on a catalog without STRK. Two functions that only return a sentence
         (`token_network_error`, `balance_failure_copy`) stay Bramble's. `canonical_account_id`
         came with `is_account_id_for_network`. `networks.json` and `token-registry.json` are
         copied as fixtures for the tests. `portfolio_state.rs` moved its portfolio cases; its
         history and activity cases follow in 2.4c. Still in `wallet-core` and candidates for
         2.6: the `bramble.mobile.portfolio.v1` schema string, the curated USDC.e alias, the
         CoinGecko base URL and the rule that only mainnet has fiat prices.
      3. **2.4c History and activity** (3,565): `history/*` and `activity/*`, which import each
         other. Oracle `portfolio-parity.jsonl` rows and `extension-transactions-observation.json`;
         tests `history_parity.rs`, `row_parity.rs`, and the explorer-URL and activity-store
         cases 2.3 left out of `transactions_rules_parity.rs`. Done (#45): `history/*` and
         `activity/*` as plain Rust. The activity log is `ActivityLog` over `host::Storage`,
         which gains one method, `rename`, for setting an unreadable file aside (the vault never
         calls it; it is a breaking change for anyone implementing `Storage`). The process-wide
         file lock becomes `&mut self`: the wallet keeps one log per file and serialises calls.
         The 4 MiB bound is checked on the bytes read, since `Storage` reports no size, and a
         read that errors is set aside like a corrupt file. `ActivityBook` and `HistoryScan`
         are plain values. `ActivityKind` is defined in `history::activity` (it was only in
         mobile's send wrapper). Explorer links take the network catalog and answer `None` for
         a network it lacks. Staying Bramble's: the empty-state and unavailable notices, the
         failure-notice sentence (with its fixture loops, in mobile's tests), deleting the log
         with the wallet, and the iOS icon-name test. Still in `wallet-core` and candidates for
         2.6: the Starkscan API base URL, "Untracked token", and row hints that name Bramble.
      4. **2.4d Privacy** (3,007): `privacy/*`. `plan.rs` and `plan_steps.rs` hold `WalletCore`
         as `send/plan*.rs` did, so they take a host trait the way `Plan` takes `PlanHost`;
         `wallet_ext.rs` stays in mobile. Oracle `extension-privacy-*.json` and G1's vector;
         tests `privacy_parity.rs`, `privacy_plan.rs`. Done (#46): `privacy/*` as plain
         Rust. `PrivacyReadPlan` is a `&mut self` state machine lent a `PrivacyHost` on every
         `advance`, whose one method returns the scoped viewing key as a `SecretFelt` or `None`
         (locked, or the account gone); the wallet keeps the session, the generation check and
         the record lookup. The key is held in `SecretFelt`, written into one zeroizing body,
         and the body stays zeroizing until the host takes it (mobile copied it out at the
         step). Routing and limits are plain records (`privacy::config::{Route, Limits}`); the
         JSON loader and the test override stay Bramble's. Two rules are enforced in the core
         as well as in the wallet's loader, because the route is now an argument: an OHTTP
         route is not configured, and the discovery URL must be an `https` service URL
         (`isServiceUrl`). `DeploymentState` is defined in `send::types` (it was only in
         mobile's send wrapper). `privacy/json.rs` stays beside `js::json`: it refuses
         duplicate keys and non-safe integers where `js::json` is last-wins. The parity suite
         is three files (`privacy_parity.rs`, `privacy_gates_parity.rs`, `privacy_plan.rs`)
         over `privacy.json` copied as a fixture.
   5. **Multisig coordination**: what sits around the Nostr wrap/open the core already has,
      and is today written twice (the extension's `background/multisig/multisig-coordination-ops.ts`,
      `multisig-nostr-publish.ts`, `multisig-sync-window.ts` and the paging half of
      `nostr-relay.ts`; mobile's `coordination.rs`, `nostr_relay.rs`, `ffi_nostr_multisig.rs`).
      Sans-IO: the host keeps the WebSockets and the address book and passes in what it read.
      The extension's TypeScript is the oracle *and* the shape: functions keep its names and
      arguments, so its later swap (3.4) replaces bodies, not call sites, and mobile adapts to
      them. A Bramble generator for these rules comes first (see Golden tests).
      - `wallet-core`: relay-URL grammar (`isNostrRelayUrl`), gift-wrap batch validation
        (`parsedGiftWraps`: kind 1059, `p` tag, no duplicate ids, batch bound), sync paging
        (unseen-first bounding, the `until` cursor, interleaving candidates across relays) and
        acknowledgement counting.
      - `bramble-core`: the `bramble.multisig` identifier, default relays and "one custom relay
        replaces the defaults", `MAX_SYNC_PAGES`, the sync window (30-day first lookback, 3-day
        incremental, retry cut-off), recipient selection and the sender rule (a saved contact on
        the network whose Stark key's Argent signer GUID is in the signer set).

      It goes in two parts. **2.5a, the multisig account** (done, #47): mobile's
      `argent_multisig.rs`, `multisig_rpc.rs`, `class_declared.rs` and the validators of
      `multisig_store.rs`, into `wallet-core`'s `accounts::{multisig, multisig_read,
      multisig_declared, multisig_records}`, so mobile names Krusty only in `kms.rs`. How it differs
      from the table above: the pinned class and its signer limit travel together as an argument,
      `ArgentMultisigClass` (no `bramble-core` constant yet: Bramble's value is
      `generated/multisig.json`, which stays mobile's until 2.6, and the tests read a fixture copy
      of it); the name-length limit is an argument to `is_record`; the reads are plain `&mut self`
      values; replies go through `send::judge::answer` and `judge::chain`, while the block pin,
      the class check (a zero class is unsupported, not undeployed) and code-only not-found
      matching keep mobile's judging, since `send::judge`'s would change those verdicts; the
      records keep serde (they are the extension's stored JSON shape); the observed-authority
      fallback and the registry file stay in mobile; `lenient_hex_felt` moved from mobile's
      `contact_card.rs` into `js::felt`. **2.5b, the coordination** (done, #48), the rest of this
      section, with the extension's names and arguments (snake-cased): `nostr::relay_url`
      (`is_nostr_relay_url`, which now accepts `ws:` on `127.0.0.1` and `localhost` as the
      extension does), `nostr::relay` (`valid_relay_urls`, `parsed_gift_wraps` in the extension's
      order of checks, `publish_many_nostr_gift_wraps` and mobile's acknowledgement counting),
      `nostr::sync` (`sync_nostr_gift_wraps`, mobile's `SyncJudge` extended to pages:
      unseen-first bounding, the `until` cursor, the page cap, per-page subscription ids) and
      `nostr::sync_window` (`nostr_sync_since`, `retryable_synced_at`, `is_wrap_past_retry`), with
      the page cap and relay count as `RelayLimits` and the lookbacks as `SyncLookbacks`; the
      retry cut-off is the first lookback, as in the TypeScript, so it has no constant of its own.
      `bramble-core`'s `coordination` holds the identifier, default relays, `MAX_SYNC_PAGES`, the
      lookbacks, `coordination_relay_urls`, `with_nostr_relay_url`, `is_coordination_signer` and
      `coordination_recipients` (over plain address-book records, with the extension's `Map`
      dedupe), as plain Rust until 2.7 exports them. Every case of the three G2 fixtures passes;
      none is skipped. Left for 2.7 with the coordination file: the stored-relay normaliser,
      `is_trusted`'s package parsing and `is_checkpoint` (no fixture cases).
   6. **Catalogs**: `generated/*.json` are Bramble's; `wallet-core` takes catalogs as arguments.
      Done (#49): `bramble-core` gains `generated/{networks,token-registry,accounts,multisig}.json`
      (Bramble's assets, byte-identical to their fixture copies, which a test checks) and
      `catalogs::{networks, accounts, account_address, multisig}`, mobile's loaders as plain Rust
      (no UniFFI until 2.7), including `multisig_constants.rs` and the class rules of
      `account_address.rs` (`resolve_class`, `argent_shape`), which the table below did not list.
      The tests that hand-built catalog records now read these. `wallet-core`'s remaining
      Bramble strings became arguments: `portfolio::PortfolioPolicy` (the
      `bramble.mobile.portfolio.v1` schema, the price API base, the one priced network) for
      `PriceBook`, `BalanceSnapshots` and `TokenPreferences`; `history::HistoryPolicy` (the
      Starkscan API base, the untracked-token name, the pre-confirmed hint that names Bramble) for
      `ActivityBook`, the Starkscan functions and `activity_groups`; and the USDC.e alias joined
      the registry as `TokenRegistry::aliases`. Bramble's values are `PORTFOLIO_POLICY`,
      `HISTORY_POLICY` and `LEGITIMATE_ALIASES` in `bramble-core`. All breaking. Left in
      `wallet-core`: `container.rs`'s format strings (Open questions) and the extension's own copy
      the fixtures check (row copy, activity labels, notices). Mobile keeps its own loaders and
      copies of the policies until 2.7 swaps them for these; M12 follows the new signatures and
      drops mobile's direct Krusty dependency (only `kms.rs` named it, for `ChainId` and
      `SecretFelt`; `networks.rs` no longer did).
   7. **Swap**: what remains in `mobile/core` that is Bramble's but not mobile-only (constants,
      v12 schema and migrations, catalogs, the UniFFI surface, `HostStorage` and `HostClock` as
      `#[uniffi::export(with_foreign)]` traits adapting to `wallet-core`'s) moves into this
      repo's `bramble-core`. `mobile/core` is then replaced by a git dependency on it at a tag
      (same crate, lib and module names), Swift and Kotlin regenerate on UniFFI 0.29.5 → 0.31.2,
      and builds use `--no-default-features --features mobile`.

      **The port** (open, #50): every file of `mobile/core/src` at Bramble
      `8d1f4b8c` except `bin/uniffi-bindgen.rs` moved, copied unchanged but for paths, behind
      `bramble-core`'s `mobile` feature; its modules keep their crate-root names (`src/mobile.rs`,
      included by `lib.rs`). Merged rather than copied: `CoreError` (the union, the extension's
      variants first), `CoreInfo` and `core_info`, `StarkSignature` (wallet-core's), the catalogs
      (2.6's `catalogs::*` replace `networks.rs`, `account_catalog.rs`, `multisig_constants.rs` and
      the class rules of `account_address.rs`, regaining mobile's UniFFI attributes under
      `mobile`), `OZ_ACCOUNT_VERSION`, and `coordination` (mobile's file becomes
      `coordination::file`, whose relay set and sync window are 2.5b's
      `coordination_relay_urls` and `SYNC_LOOKBACKS`). `security.rs`, `biometric.rs`, `backup.rs`
      and `public_store.rs` moved too, under `mobile`: `wallet.rs` uses them and two export through
      the same UniFFI module. Mobile's tests run with `--features mobile,testing`; its regression
      fixtures are in `crates/bramble-core/tests/regression/`. The vault chain still ends at v5;
      the plan put the v5 → v12 step before M13, but M13 merged first (see Vault switch). Found
      in review: one crate exporting both surfaces put the extension's phrase-handling exports
      (`generate_mnemonic` returns a phrase; five more take one) into mobile's bindings, so they
      moved behind a default `extension` feature mobile leaves off; and the FFI-surface test's scan
      missed exports written under `cfg_attr`, now fixed. `catalogs::GENERATED` lists every
      generated file with the path Bramble writes it to, for M13's drift test.
      **M13** (Bramble): `mobile/core` keeps its manifest, lockfile, `bin/uniffi-bindgen.rs`,
      `generated/` (the generators' output) and `tests/fixtures/` (the generators' output and the
      conformance tests' input), and depends on this crate by `rev` with
      `default-features = false, features = ["mobile"]`; the build scripts build the dependency
      (`cargo build -p bramble-core --lib`), which yields the same library names, and its own
      bindgen reads this crate's `uniffi.toml`, so Swift and Kotlin keep their module names. A test
      there holds `generated/` to `catalogs::GENERATED`. As built (Bramble #929): as above, with
      `uniffi.toml` gone (the bindgen reads this crate's) and cargo-ndk given `--manifest-path`;
      no Swift or Kotlin app code changed; both apps build and pass their unit tests on the
      regenerated bindings, which equal this crate's.

      <a id="vault-switch"></a>**Vault switch** (decided 2026-11-13; replaces the v5 → v12
      migration first placed here). M13 merged before it, so mobile still writes its own vault
      version 5 (`migrations.rs`: a chain v1 → v5 inside the DEK container) while the extension
      writes schema v13 (#826 added the multisig STRK20 viewing keys) inside a bare v3 envelope.
      No mobile vault holds funds yet, so mobile switches format rather than migrating:
      - `bramble-core` writes the extension's v13 payload, unchanged in shape and validated by a
        port of its rules (`packages/platform/vault/src/schema/`), inside mobile's existing DEK
        container. The container stays (its biometric slot is mobile's); which container the
        extension uses is the separate Open question, since this changes only the payload.
      - The v1 → v5 chain goes: most of `migrations.rs`, `migrations_tests.rs`,
        `tests/migration.rs`, and the regression vaults `vault-*.json` and
        `migration-v1-to-v5.golden.json`. A container whose payload is not v13 fails closed with
        an error telling the user to reset the wallet; there is no converter.
      - Mobile's records map onto the extension's. `id` (`<network>:<address>`), `address` and
        `class_hash` keep their values; `network` → `networkId`; `public_key` → `publicKeyX`, plus
        `publicKeyY` for `starknet-bip44` (`kms::StarkKeypair::public_key_y`); `family` →
        `accountContractType`; `deployment_salt` → `deploymentSalt`, which v13 requires for
        Argent and Braavos and forbids for OpenZeppelin. `ladder` and `account_index` →
        `derivation`: `Account` is `starknet-bip44` at `{accountIndex: i, addressIndex: 0}`,
        `Address` (Braavos's) at `{accountIndex: 0, addressIndex: i}`, `ArgentX` is
        `argentx-bip44` at `{addressIndex: i}` (the extension's `recoveryLadderIndex` reads each
        back as `i`). `originGrants`, `importedSeeds`, `accountSecrets`, `shadowAccountScopes` and
        the multisig key lists are written empty.
      - Every account's derived private key is sealed in `signingKeys`, as the extension does and
        its validator requires. The keys stay inside Rust and never cross the FFI outward, so the
        mobile build still returns no raw key; it is still a custody change and the PR says so.
        Since P3 one key crosses inward: `import_private_key` takes a pasted key and seals it
        (see AGENTS.md).
      - Mobile's BIP39 passphrase (`mnemonicPassphrase`) is dropped: v13 has no field for it, and
        the extension's recovery refuses one. Both apps pass `nil` to `import_wallet`'s
        `passphrase` today, so the argument goes, which changes the bindings (M14 updates the call
        sites).
      - Labels and groups leave the vault for `public_store.rs`, as the extension keeps them in
        preferences. `backupPending` and `recoveryPending` are mobile's own (the extension has
        neither) and move there too. None is secret.
      - The oracle is G4: the extension's own store and schema code seal a v13 vault for the
        published phrase and a fixed account set (OZ, Argent, Braavos, a Ready `argentx-bip44`
        account). The test opens it, checks its payload passes the ported validators and
        re-serialises to equal JSON, and checks that a vault mobile builds for the same phrase
        and accounts holds an equal payload.
      - When the extension's schema moves past v13, mobile follows in a release of this crate,
        until Phase 3.2 makes the core the one writer.

      As built (V1): `vault_v13.rs` replaces `migrations.rs`. The container's `version` is 13, the
      payload's `schemaVersion`. Reading accepts exactly the subset of v13 mobile writes: the six
      collections it has no use for (`originGrants`, `importedSeeds`, `accountSecrets`,
      `shadowAccountScopes` and both multisig key lists) must be empty, `mnemonic` must not be
      null, and every derivation must be one of mobile's three ladders or, since P3,
      `imported-raw-key`, whose sealed key is checked against the record's public key and
      address and is the one that signs (the rest of this paragraph is about phrase accounts). A
      vault holding anything else is refused, not half-read, so their validators are not ported
      until mobile holds them. On what it accepts, the extension's rules (field grammar, canonical
      felts, the account ↔ signing-key bijection, the active account) and mobile's stronger ones
      apply: every record, `publicKeyY` and sealed `privateKey` must equal what the phrase
      derives. Signing
      still derives each phrase account's key per call, so its sealed key is checked, never used.
      An imported key (P3) has no phrase to derive from, so the session holds it in memory
      (`VaultPayload::imported_keys`, zeroized on lock) for the whole unlock, where before P3 the
      session held only the phrase. An earlier
      build's vault (bare envelope or container 2 to 5) is refused only after its password is
      proven, which resets the backoff; `delete_wallet` proves the password without reading the
      payload, so such a vault can still be deleted in the app. Labels, groups and
      `backupPending` are `public_store`'s `preferences.json`. Each mutation moves either the
      accounts or the preferences, and writes only that file, so every write stays one atomic
      file; a new wallet's preferences are written just before its vault, and a label or group
      member naming an account the vault lacks is dropped on load. `recoveryPending` is no longer stored: it is `accounts.is_empty()`, which the old
      validator already required it to equal. Internal `passphrase` parameters (`kms`, the scan)
      stay and are passed `None`.

   Mobile-only platform glue with no extension counterpart (biometric slots, file storage, app
   UI state) stays in the app, or in `bramble-core` behind mobile's build, unless the extension
   gains it.

   <a id="unassigned-modules"></a>**Where every mobile file goes.** All of `mobile/core/src` at
   `01bd6517`, 23,114 lines including inline tests, so no file falls into 2.7's "what remains"
   by omission. Placements not yet ported are proposals, confirmed when their step starts.

   | Step | Mobile files | Lines | Lands in |
   | --- | --- | --- | --- |
   | 2.1–2.3, done | `kms.rs`, `nostr.rs`, `vault.rs`, `container.rs`, `clock.rs`, `files.rs` (glue left after M2–M4) | 448 | glue; 2.7 moves what is Bramble's |
   | 2.3, done | `send/{mod, plan, types, wallet_ext, fixture_tests}.rs` (`TransferPlan` and its tests) | 1,122 | `bramble-core` at 2.7 |
   | 2.2 Accounts | `kms_accounts.rs`, `recovery_scan.rs`, `recovery_advisory.rs`, `recovery.rs` | 1,598 | split as 2.2 says |
   | 2.4a–d | `js/*`, `portfolio/*`, `portfolio_testing.rs`, `history/*`, `activity/*`, `privacy/*` | 11,168 | `wallet-core`; copy and assets in `bramble-core` |
   | 2.5 | `coordination.rs`, `nostr_relay.rs`, `ffi_coordination.rs`, `ffi_nostr_multisig.rs` (each with `_tests`), `multisig_constants.rs`, `argent_multisig.rs`, `multisig_rpc.rs`, `multisig_store.rs`, `class_declared.rs` | 3,417 | constructor layout, authority read, relay paging in `wallet-core`; registry, pinned class, relays, sync window in `bramble-core` |
   | 2.6 | `account_catalog.rs`, `networks.rs` (and `generated/*.json`) | 638 | `bramble-core`; `wallet-core` takes them as arguments |
   | 2.7 | `wallet.rs`, `wallet_accounts.rs`, `records.rs`, `testing.rs`, `lib.rs`, `rpc_request.rs` (the UniFFI object and records); `account_store.rs`, `migrations.rs`, `migrations_tests.rs` (vault schema); `contacts.rs`, `contact_card.rs`, `groups.rs`, `account_names.rs`, `avatar.rs`, `ui.rs` (extension product rules) | 4,151 | `bramble-core` |
   | 2.7, provisional | `security.rs` (wrong-passphrase backoff) | 269 | `bramble-core` under `mobile`; see Open questions |
   | 2.7 | `biometric.rs`, `backup.rs`, `public_store.rs` | 299 | `bramble-core` under `mobile` (mobile-only glue that exports through the same module) |
   | stays | `bin/uniffi-bindgen.rs` | 4 | mobile's generator binary |

   2.7 is still about 5,700 lines (its own files plus the send wrapper and the glue). Its
   account rules and wording have golden fixtures already (`accounts-parity-rules.json`,
   `accounts-parity-recovery.json`); its UniFFI object has none, and needs none, since mobile's
   own tests come with it.

   One constraint orders this. `bramble-core` never names `krusty_kms`, and after M4 mobile
   still does in `kms_accounts.rs`, `account_catalog.rs` and `send/wallet_ext.rs` (2.2 Accounts
   removes all three), `kms.rs` and `networks.rs` (`ChainId`, `KmsError`; 2.6) and
   `argent_multisig.rs` (2.5). Each needs a home in `wallet-core` before 2.7.
3. **Extension adoption** (Bramble PRs to `main`), after [Parity](#parity), against a
   `bramble-core` that by then covers everything the extension does:
   1. **KMS swap**: pin the npm package; a thin `packages/platform/kms-bramble-core/` adapter maps
      the generated API onto `KmsPort` (snake to camel case is already done by the bindings;
      `CoreError` becomes `WalletError`); delete `packages/platform/kms-krusty` and the Krusty npm
      dependency; `scripts/check-bramble-core.ts` replaces `check-krusty-adapter.ts` (package
      fields, sha256 of `bramble_core.wasm` and `bramble_core_bg.js`, the same known-answer
      vectors); `uniffiInitAsync` runs in `apps/extension/src/background/index.ts` before
      `createWalletVaultService`; `build.ts` copies the wasm and glue; `PINNED_RUNTIME_FILES` and
      `EXPECTED_DEPENDENCIES` in `scripts/config-standards-policy.ts`, `ALLOWED_EDGES` in
      `check-workspace-architecture.ts`, and `packages/testing/testkit/src/fake-kms.ts` follow.
      `getOzAccountClassHash` in `vault/src/accounts/account-contract.ts` calls
      `ozAccountClassHash(network)` and keeps its devnet constant. `wrapNostrApplicationData`
      gains a `createdAt` argument supplied from `Date.now()`. Three behaviours the adapter must
      keep in mind:
      - `validateMnemonic` returns `false` for any text; the old adapter threw on blank or
        oversized text. Keep that `isMnemonicText` throw in the TypeScript adapter.
      - `ozAccountClassHash` resolves Krusty's manifest at the pinned `OZ_ACCOUNT_VERSION`
        (3.0.0), as `account-contract.ts` does, never `latest`, so a Krusty bump cannot move
        addresses. OpenZeppelin addresses still use salt `0x0`, which `account-contract.ts`
        passes explicitly; the adapter forwards an omitted salt as omitted (`bramble-core` then
        uses the public key, as Krusty does today).
      - `deriveStrk20ViewingKey` refuses non-canonical scopes (`0x0123`), as Krusty 0.11 does
        today.
   2. **Vault**: `encrypted-store.ts` becomes a shim over the core, with a write-through mirror
      over `chrome.storage`. Keys stop leaving Rust and `raw-keys` leaves the extension build.
      Migrations from schemas before v13 stay in TypeScript. See [Key exposure](#key-exposure).
   3. **Transactions, then reads**: `@bramble/transactions` drives the `TransferPlan` step loop;
      portfolio, history and privacy follow. Bramble's `scripts/mobile-assets.ts` renderers
      invert so TypeScript reads catalogs from the npm package.
   4. **Multisig coordination**: `multisig-coordination-ops.ts`, `multisig-nostr-publish.ts` and
      `multisig-sync-window.ts` call the core's functions from 2.5 under the same names;
      `nostr-relay.ts` keeps its sockets and loses its paging. Nostr keys still come from the
      vault (index `0, 0`, recovery-phrase wallets only), as today.

   The extension's TypeScript is the oracle until it is replaced, so each swap is proved by the
   golden suite passing through the extension's own conformance tests before the TypeScript goes.
4. **Later**: NFTs, which the extension has parked. The rest of this phase (Ledger, dapp
   provider and permissions, STRK20 submission, swap, gasless, allowances) is the extension's
   today and moved into [Parity](#parity), ahead of Phase 3.

## Key exposure

How far raw private keys, the mnemonic and the vault passphrase travel outside Rust, today and
after Phase 3.2.

**Today (Bramble `main`, `a01b240c`, v0.10.10).** Krusty's wasm is stateless: keys cross it as
hex strings in both directions. The v13 vault holds the mnemonic (recovery-phrase wallets), each
account's derived key (`signingKeys[].privateKey`), optional account secrets and multisig STRK20
viewing keys under AES-256-GCM.
- **Every signature decrypts the whole vault.** `signHash` (`vault/src/service/service.ts`) reads
  and decrypts the full vault into JavaScript objects, then passes one key string to
  `kms.signStarkHash`. Each signature puts the mnemonic, every account's key and the other vault
  secrets into the JS heap as immutable strings that cannot be zeroed; GC frees them later but never erases them.
- **The passphrase is held for the whole unlock.** The background worker's unlock session
  (`apps/extension/src/background/runtime/session.ts`) keeps the plaintext passphrase in memory on
  a rolling inactivity window (30 minutes by default, 8 hours at most) and mirrors it to
  `chrome.storage.session` under `bramble:session` (`runtime/unlock-session-boot.ts`), so an MV3
  worker restart does not lock the wallet. Lock or expiry erases the mirror or overwrites it with
  a revocation marker that a restarted worker refuses. The grant cache
  (`vault/src/service/grant-cache.ts`) keeps the passphrase as its entry key beside public account
  records only.
- **PBKDF2 runs once per unlock.** `encrypted-store.ts` memoises the derived AES key (at most 8,
  keyed by a passphrase fingerprint, salt and iterations) as a non-extractable WebCrypto key and
  drops it on lock. Non-extractable stops export, not use, and the plaintext passphrase sits beside
  it, so it protects little against code running in the worker.
- What limits it: everything runs in the background service worker; `storage.session` is
  in-memory, cleared when the browser exits, set to trusted contexts on Chromium
  (`apps/extension/src/browser-storage.ts`) and unreachable from content scripts by default
  elsewhere. Other extensions cannot reach the worker's memory or storage.

**`raw-keys` reproduces this model**, no worse and no better. Adopting the core with it on keeps
today's exposure; it is a bridge, not the target.

**Target (3.2, a precondition for dropping `raw-keys`, not a follow-up).**
- The core unlocks the vault and holds it: `unlock(vault_bytes, passphrase) → VaultSession`,
  with signing and derivation as session methods. Keys live in wasm linear memory as `Zeroizing`
  values; only UniFFI object handles reach JavaScript.
- Unavoidable JS exposure is limited to what the UI must touch: the passphrase on unlock, and a
  mnemonic typed on import or shown on backup. Each goes straight into the core and is dropped.
  That holds only if the extension stops holding and mirroring the passphrase (see the restart
  question below); keeping today's unlock session keeps the passphrase in JavaScript.
- Then the extension builds `--no-default-features` like mobile, and `raw-keys` is deleted.
- Not a hard boundary: wasm memory is readable by any script in the extension's context, so code
  execution there can still sign or read it. What this removes is accidental exposure (logs,
  error strings, serialisation, GC copies) and unreliable erasure.
- Locking wipes what the core holds. Neither RustCrypto nor Krusty wipes stack temporaries
  (`RustCrypto/meta#38`), so every function that takes or makes a secret runs its body through
  `scrub::scrubbed`, which overwrites 128 KiB of stack afterwards: the vault's KDF and AES-GCM
  calls, phrase generation and checks, every key derivation, Stark signing and public keys, the
  STRK20 viewing key, and Nostr's key, wrap and open. A phrase derivation, the deepest, uses about
  67 KiB; a test keeps every such call within the scrubbed size. Not covered: a secret a function
  returns by value (moving it out copies it into the caller's frames, so the extension's raw-key
  exports leave one copy); registers; on wasm32, values the engine keeps in wasm locals, on its
  native stack outside linear memory; and a memory read while unlocked or during an unlock.

Two things the target does not yet answer (both in Open questions, both to settle before 3.2 is
designed; only the first is measured):
- **The passphrase key loses non-extractability.** Today PBKDF2 runs in WebCrypto and yields a
  non-extractable key. In the core it runs as Rust compiled to wasm, and the derived key is bytes
  in linear memory: zeroized on lock, but readable while unlocked. This costs little while the
  plaintext passphrase is held beside it, as it is today. PBKDF2 runs once per unlock (and once
  per restored session), so its speed at 600,000 iterations in wasm matters there: about 155 ms,
  3.6 times WebCrypto's (Open questions).
- **A session in wasm memory ends when the service worker does.** An MV3 worker is stopped when
  idle. Today a restart restores the unlock from the plaintext passphrase in `storage.session`.
  3.2 picks one: keep that mirror (same as today, and the passphrase stays in JavaScript), or
  require unlocking again after a restart (the passphrase crosses once, at the cost of more
  unlocks).

**Interim, extension only, independent of this repo**: decrypt only the signing account's key
rather than the whole vault, narrowing each signature's exposure from the mnemonic and every
account's key to one key.

## Golden tests

The extension's TypeScript is the reference implementation. Its generators in Bramble
(`scripts/generate-mobile-parity-vectors.ts`, which also imports
`generate-mobile-transaction-vectors.ts` and `generate-mobile-privacy-vectors.ts`, all behind
`bun run mobile:parity`) run the real code paths (Krusty, `encryptJson`, account discovery,
transfer planning, portfolio rendering) and write JSON fixtures. Re-running them leaves no diff;
seals with fresh randomness are kept while the extension still opens them.

- **Here**: the fixtures live in `crates/bramble-core/tests/fixtures/`, with `README.md` as the
  manifest: the Bramble commit (one for all files), and per file the generator and the sha256. A
  test checks the hashes, so a hand-edited fixture fails. Bramble stays the source: syncing is
  re-running the generator there and copying the files, never editing them here.
- **Three harnesses, one set of vectors**: the Rust tests (`crates/bramble-core/tests/*.rs`,
  which exercise `wallet-core` through Bramble's constants), the built wasm under Node
  (`js/test/smoke.test.mjs`, so the shipped artifact is what is tested), and mobile, whose
  existing parity tests move here with their modules. `wallet-core`'s own unit tests cover its
  grammar and bounds without Bramble's fixtures.
- **Drift**: `bun run check` in Bramble (the `tests/conformance/mobile-*.test.ts` suites) fails
  when a committed fixture no longer matches what the extension produces, so an extension change
  that alters behaviour forces a re-generate, and the re-copied fixture then fails here until the
  core matches. After Phase 3 those same conformance tests run the extension through the core,
  proving each swap changed nothing.
- **Gaps**: mobile-only behaviour (biometric, file storage) has ordinary Rust tests; it has no
  oracle. Extension behaviour without a generator gets one in Bramble before its module is ported.
  Multisig coordination (2.5) is the known case: `nostr-multisig-parity.json` covers keys and one
  gift wrap, not recipient selection, the sender rule, sync paging or the sync window.
- **G1, the viewing public key vector** (Bramble, `feature/mobile-wallet`).
  `computeViewingKeys()` in `scripts/mobile-privacy-fixture.ts` adds, per case, the public key
  the extension's `previewStrk20ViewingPublicKey`
  (`packages/platform/vault/src/accounts/strk20-viewing-key.ts`) returns. It lands in
  `extension-privacy-wire.json` under `viewingKeys`; mobile's `privacy_parity.rs` asserts it.
  F1 here: copy the fixtures at the new Bramble commit (only that file's hash changes), update
  its row and the commit line, and add a test in `parity.rs` that
  `stark_public_key(derive_strk20_viewing_key(..))` equals it. As built (Bramble #919):
  `computeViewingKeys()` is in `scripts/mobile-privacy-wire.ts`, and each phrase account gains
  `viewingPublicKey`; mobile matched it with no change. F1 (#42) copied it; `parity.rs` checks both that
  composition and `accounts::advisory::own_viewing_public_key` against it.
- **G2, the multisig coordination generator** (Bramble, `feature/mobile-wallet`). It needs
  `main` merged into the branch first: `multisig-nostr-publish.ts` and
  `multisig-sync-window.ts` exist only on `main`, and the generators only on the branch. A new
  fixture, `nostr-coordination-parity.json`, written by a new file beside
  `scripts/mobile-assets/write-nostr-multisig-parity.ts` and reached from `mobile:parity`,
  calling the extension's own functions over fixed inputs. One section per rule 2.5 ports:
  - relay URLs `isNostrRelayUrl` accepts and refuses;
  - gift-wrap batches through `parsedGiftWraps`: wrong kind, missing `p` tag, duplicate ids,
    a batch over the bound;
  - sync paging: unseen-first bounding, the `until` cursor, candidates interleaved across
    relays, `MAX_SYNC_PAGES`;
  - acknowledgement counts per relay;
  - the sync window: first lookback, incremental lookback, the retry cut-off, at fixed clocks;
  - relay policy: the defaults, and one custom relay replacing them;
  - recipient selection and the sender rule, over a fixed address book and signer set.

  `tests/conformance/mobile-nostr-parity.test.ts` gains the drift check. Mobile's
  `nostr_relay.rs` and `coordination.rs` are tested against it where they already implement a
  rule, which shows where mobile and the extension differ before anything is ported. F2 here
  copies it; it sits unused until 2.5. As built (Bramble #920): three files,
  `nostr-coordination-parity-{rules,relays,paging}.json` (Bramble's 500-line limit covers JSON),
  with no extension code extracted. Mobile differs in six cases, which its test lists and does
  not assert: it refuses `ws:` on loopback where the extension accepts it, and it reads one page
  per relay with no `until` cursor and filters seen wraps after the batch bound. It has no
  gift-wrap batch validation, retry cut-off, recipient selection or `MAX_SYNC_PAGES` at all.
  2.5 takes the extension's behaviour for each.
- **G3, the recovery scan generator** (Bramble, `feature/mobile-wallet`). The families fixtures
  prove addresses and `accounts-parity-recovery.json` proves row wording, but nothing proves the
  scan itself: which shapes it walks, which coordinates it lists with no history, how far it
  goes, or the signing verdicts. Mobile's scan tests run against a canned node it wrote. G3
  exports `recoveryScanShapes` and `isCreatableCandidate` from
  `apps/extension/src/background/onboarding/onboarding-ops.ts` (module-private today; move them
  beside `recoveryScanTarget` in `wallet-model/src/accounts/recovery.ts`) and writes
  `accounts-parity-scan.json` from `scripts/mobile-parity-accounts.ts`, with sections (as built:
  a sibling `scripts/mobile-parity-scan.ts` writes two files, the signing verdicts in
  `accounts-parity-scan-signing.json`, because Bramble's 500-line limit covers JSON; the keep rule
  is exported too, as `keepsRecoveryCandidate`):
  - shapes in order, with and without a BIP-39 passphrase, and each shape's creatable flag;
  - `recoveryScanTarget` and the keep rule over fixed candidate sets (a quiet wallet, a ladder of
    real accounts, the `maxScannedIndexes` ceiling, a manual index);
  - `argentSigningRestriction` and `braavosSigningRestriction` over a fake provider answering
    fixed reads: each restriction, no restriction, a read that fails, a Braavos 2FA removal
    before and after `expire_at`;
  - `strk20RegisteredElsewhere`: no key, this wallet's key, another key.
- **F3** copies it and `mobile/core/generated/accounts.json` (written by `bun run mobile:assets`,
  `scripts/generate-mobile-assets.ts` through `mobile-assets/accounts.ts`, from the extension's
  account tables, so it is extension output; the only fixture not written by `mobile:parity`,
  which the README's intro then says) as
  a fixture, so tests here read the class table, Xverse's class and the scan limits rather than
  hand-copying hashes as `tests/plan_support` does today. One Bramble commit covers every file,
  so F3 re-copies all of them; a re-copied fixture that fails here means the core is behind the
  extension, and the core is fixed, never the fixture.
- **G4, a v13 vault** (Bramble #947, merged), for the [vault switch](#vault-switch): the
  extension's own store and schema code seal a v13 vault for the published phrase and a fixed
  account set (OZ, Argent, Braavos, Ready), written as the payload and the sealed file
  (`vault-v13.json`, read by `tests/vault_v13.rs`).
- **The wasm harness covers only what is exported.** `smoke.test.mjs` replays the KMS and Nostr
  vectors. The vault and send modules are not exported through UniFFI yet, so on wasm32 they are
  linted and never run. That is acceptable until 3.2 and 3.3 export them, and those steps extend
  the smoke test with the vault and transaction fixtures.
- **Where fixtures are written after 2.7.** The generators write into
  `mobile/core/tests/fixtures/`, which 2.7 deletes, and `rust-gift-wrap.json` is written by a
  mobile test. Both need a home by then (see Open questions).
- `krusty_version_matches_the_lockfile` (in `wallet-core`) keeps the Krusty version `core_info()`
  reports honest.

## Build notes

- wasm32 needs `wasm-bindgen-cli` at the exact version in `Cargo.lock` (getrandom's wasm backend
  reaches wasm-bindgen); `ubrn` runs it and writes `bramble_core_bg.js`, which ships beside the wasm.
- `wallet-core`'s records cross the FFI as `#[uniffi::remote(Record)]` / `remote(Enum)` declarations
  in `bramble-core`; their fields are repeated there and must match (the compiler checks). Under
  wasm2 they generate the same TypeScript as local records.
- Generated TypeScript uses extensionless relative imports; `js/scripts/node-esm-imports.mjs` adds
  `.js` so the package resolves under Node ESM as well as bundlers.
- Under a CSP without `unsafe-eval` (the extension's) the wasm2 player falls back to an interpreted
  dispatcher. `'wasm-unsafe-eval'` is all the manifest needs, and it already has it.
- `strip = "symbols"` drops the `target_features` section; wasm-bindgen warns but the module is
  correct. Switch to `strip = "debuginfo"` plus `wasm-opt` if the warning ever matters.

## Process

Added 2026-11-09 after reviewing the first day's work. Items marked *proposed* are not agreed.

- **Adoption keeps pace with porting.** The core stays at most one step ahead of mobile. 2.4 is
  not ported until M2, M3 and M4 have merged. A module in both repos is written twice, which is
  what this repo exists to stop.
- **Key-custody code gets a recorded second review.** #13, #19, #20 and #22 (about 7,100 added
  lines) merged with no review on GitHub. Before M2 merges, someone other than the author reviews
  `envelope.rs`, `container.rs`, `send/sign.rs` and `send/plan*.rs` and records it in an issue
  here. *Proposed*: the ruleset requires one approval on `main` from then on.
  **Not done as of 2026-11-10**: M2, M3 and M4 have merged, no issue records the review, and the
  `main` ruleset requires no approval (it blocks deletion and force-push only). The review now
  covers #25 and #31 as well, and should be recorded before A1 merges, since A1 and A2 add
  key-derivation paths.
- **The Status table is updated in the PR that changes it**, in either repo's half.
- **Tags are signed.** `v0.1.0` is annotated but unsigned, and a tag is never moved, so it stays
  that way; later tags use `git tag -s`.
- **Provisional, 2026-11-11**: the stacked port PRs (#39 onwards) are unmerged and untagged, so
  each mobile adoption PR pins `wallet-core` by `rev` at its port branch's tip and says it moves
  to a tag before merge. A port branch a mobile branch pins only gains commits; it is not
  rebased or amended. This is the proposed rule below, used because nothing can be tagged yet.
- **The package check and a stale local Cargo cache (2026-11-12).** On the machine that ran
  these ports, `check.sh`'s `package` step failed from 2.5b on, which was first put down to the
  published 0.2.2 lacking the APIs `bramble-core` now uses. Wrong: `cargo package --workspace`
  verifies `bramble-core` against the workspace's own `wallet-core`, and CI's package step passed
  on #48 and #49; the local failure came from a stale extracted copy of the published 0.2.2 in
  `~/.cargo/registry/src`. With a clean `CARGO_HOME`, `check.sh all` passes on #48, #49 and #50
  at 0.2.2. The breaking changes since 0.2.2 (`Storage::rename`, 2.6's constructors) still call
  for 0.3.0 at the next release.
- *Proposed*: **mobile may pin a `rev` between tags** during Phase 2. Every tag needs a version
  bump in three files, and `Cargo.lock` records the commit either way. Tags would mark 2.7 and
  each npm publish. `AGENTS.md` says mobile pins a tag; change both together if agreed.
- **Dependabot does not propose what the hard rules forbid.** Add ignores for
  `starknet-types-core` (follows Krusty), `uniffi*` (follows ubrn and `uniffi-runtime-wasm`) and
  `wasm-bindgen` (must equal the CLI), and close #10, #11 and #12. Done (#24; `wasm-bindgen`
  added later).
- **#16 (crate READMEs, crates.io) is split.** The READMEs and a LICENSE file (both crates
  declare MIT and the repo has none) go in now. Publishing waits on the crates.io question
  below. Its rename to `starknet-wallet-core` is the part with a deadline: M2 writes the package
  name into mobile, so the rename lands and is tagged before M2 merges, or is dropped.
- **Housekeeping** (done on this branch): `tests/fixtures/README.md` said only KMS, accounts and
  Nostr had ported modules; it now names the tests that read each group.

## Open questions

- Which vault container the extension uses after 3.2. `container.rs` is mobile's (a DEK, a
  passphrase slot, a biometric slot). The extension stores a bare v3 envelope under
  `bramble:encrypted-store:<key>`, with a freshness check on its lineage in `chrome.storage` that
  the core has no notion of. Either the extension migrates to the container or the core opens
  bare envelopes, and one of them owns the freshness check.
- What a `VaultSession` does when the MV3 service worker restarts: keep the extension's
  plaintext passphrase mirror in `storage.session`, or unlock again (see Key exposure).
- PBKDF2 in wasm: whether its unlock cost and losing WebCrypto's non-extractable key are
  accepted (see Key exposure). Measured 2026-11-20 (Apple M5, macOS 27.2, Node 24.18, rustc
  1.97.1): `envelope::open` at `KDF_ITERATIONS` (600,000) takes 155 ms median in the release
  wasm (n = 30, 152–171; a fresh process's first call 155–160), against 43 ms (42–48) for
  WebCrypto `deriveKey` and 28 ms for native release Rust: 3.6 times WebCrypto, per round.
  Method: a throwaway export (not kept) ran `envelope::open` on an envelope WebCrypto sealed, in
  the `npm run build` wasm, interleaved with `deriveKey` on the same password and 16-byte salt,
  first round discarded; native Rust derived the same key bytes. Under load the medians swung to
  250–1,000 ms. Not measured: Chrome's service worker, slower laptops and phones (assumed to
  keep the ratio), wasm `simd128` (off; `sha2` has a backend for it). The wasm call also blocks
  the worker for its length, where WebCrypto runs off-thread.
- Where the generators write after 2.7. *Provisional*: they keep writing Bramble's
  `mobile/core/tests/fixtures/`, which M13 keeps (the conformance tests read it there), and this
  repo copies from it as before.
  `rust-gift-wrap.json` and `ui-coordination.json` are settled by the 2.7 port: Rust writes them
  into `crates/bramble-core/tests/regression/` (`BRAMBLE_WRITE_FIXTURES=1`), and Bramble copies
  them to where the extension and the iOS UI suite read them.
- Whether `security.rs` (wrong-passphrase backoff) is mechanism for `wallet-core` or mobile-only
  glue; it depends on whether the extension has, or wants, the same rule. Provisionally in
  `bramble-core` under `mobile` (2.7).
- npm scope: `@starknet-innovation` (matches the GitHub org) or `@starknetfoundation` (matches Krusty).
  Deferred; settle before the first npm publish (Phase 3.1), since the extension's pin fixes the
  name. Tags before then don't publish to npm.
- Which `starknet-types-core` the sequencer runs (0.2.4 is reported but unconfirmed). It matters only
  if this repo starts sharing types with sequencer or node crates; until then Krusty sets the pin.
- UniFFI 0.29.5 → 0.31.2: what changes in the generated Swift and Kotlin (Phase 2.7). From the
  2.7 port: no exported name, argument, field or case of mobile's changed and nothing was added
  (the extension's exports are behind `extension`); `CoreError`'s cases are reordered (the
  extension's first), which changes their wire numbers; the rest is generator code (handles for
  pointers, synthesised `Equatable`/`Hashable`, qualified Kotlin type names). The apps' build
  against it is M13's.
- `HostStorage` is synchronous and `chrome.storage` is not; the extension side needs a write-through
  mirror (the pattern `encrypted-store.ts` already uses).
- The JS binding generator for the extension (ubrn `wasm2`, pre-production, or wasm-bindgen
  directly) is decided at Phase 3.1, not now; the Rust surface does not depend on it.
- Whether the devnet OZ class hash moves into the core or stays a TypeScript constant.
- When a second wallet wants `wallet-core`: publish it to crates.io, and whether it ships its own
  UniFFI feature or each wallet keeps exporting from its own crate (today's answer).
- *Provisional, 2.5b*: the core accepts `ws:` relays on loopback, as the extension does (its
  fixture says so); mobile keeps refusing every `ws:` relay as a platform rule (App Transport
  Security and `usesCleartextTraffic=false` block cleartext), on top of the core's grammar
  (M11, Bramble #927). Revisit if a mobile build ever needs a local relay.
- Mobile's coordination sync pages as the extension's does: `bramble-core` exports
  `RelayStep::NextPage { close_frame, request }` and runs the judge at `RELAY_LIMITS`
  (`MAX_SYNC_PAGES`, 4) instead of a page cap of 1, and the three paging cases mobile skipped are
  asserted through mobile's judge. Bramble owes the adoption, in the same PR as the bump, since
  the new case breaks no build: `RelayTransport.swift` and `CoordinationTransport.kt` check only
  for `Finish`, so they would ignore a `NextPage`, time out, and lose the relay's first page. On
  `NextPage` each sends `close_frame` then `request` on the same socket, restarts the request
  timeout (the extension's is per page), and keeps reading.
- Which modules count as mobile-only glue, and whether they live in the app or in `bramble-core`
  behind mobile's build (Phase 2).
- What makes a STRK20 proof on a phone (P15). Settled 2026-11-23: nothing on the phone; the
  SDK sends actions to StarkWare's proving service, as the extension does. Mobile embeds the
  pinned SDK for its client side for now and ports it to Rust later (see
  [Road to parity](#road-to-parity), D3).
- What carries a dapp's requests on mobile (P16): WalletConnect, as #990 assumes, or something
  else. It decides which of the extension's provider rules the core can share unchanged.
  *Proposed* 2026-11-23: an in-app browser first, WalletConnect later (see
  [Road to parity](#road-to-parity), D2).
- Whether Ledger on mobile is Bluetooth only, and whether iOS can reach the device at all the way
  the app needs (P8). Settled 2026-11-23: Bluetooth first on both apps, Android USB later (iOS
  has no USB route to a Ledger), through Ledger's Device Management Kit until the framing moves
  into the core; Ledger-only wallets supported (see [Road to parity](#road-to-parity), D1).
