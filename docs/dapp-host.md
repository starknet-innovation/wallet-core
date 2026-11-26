# DappSession: the host contract

What the Bramble apps' in-app browser does around `bramble-core`'s `DappSession` (P16). The core
owns every rule: gates, the queue, grants, reviews, hashing and signing. The host establishes the
asking origin, moves bytes (JSON-RPC, HTTP, APDUs), shows cards, and relays each reply to the page
that asked. The method-level rules live in each `dapp_*.rs` module's docs; this page is the loop
around them.

## Setup

- One `DappSession::new(wallet, tokens)` per `WalletCore`, sharing the app's `TokenPreferences`
  (the one the portfolio screens use), so a site's tracked token and the person's own edits
  share one store.
- Approvals live in memory and die with the process, as the extension's die with its service
  worker. A card survives a lock; approving one needs the wallet unlocked.
- Before loading a page, `is_blocked_site(origin)`: a listed origin gets an interstitial, not the
  page. On launch and every six hours, `refresh_phishing_feed(now_ms)`: perform the returned
  `HttpRequest` exactly as described and pass the outcome to `answer_phishing_feed`. `None`
  means the cached list is fresh.

## Origins and ids

- `origin` is the main frame's committed `https` origin, as the platform reports it for the
  message (see "P16 spike findings" in `docs/PLAN.md`). A message the bridge cannot attribute
  to the current main frame and epoch is dropped, never passed in.
- `request_id` is the host's own, unique across every page. Each settled reply comes back as a
  `DappSettled { request_id, reply }`; deliver it to the page that sent that id.
- `shown` is the network the browser shows. It answers `requestChainId` only while locked.
- `now_ms` is the host's wall clock: site first-seen records and card warnings. Expiry and the
  idle lock run on the core's own monotonic clock.

## The request loop

`request(origin, method, params, shown, request_id)` returns a `DappReply`:

| Reply | Host does |
| --- | --- |
| `Result { json }` / `Error { code, message, data }` | answer the page now |
| `Read { network, requests }` | POST the batch to an endpoint for `network` (the person's RPC override first), then `answer_read(request_id, responses)`. A failed POST is passed as an empty body. Its reply is another `DappReply`, handled the same way. |
| `Pending { approval_id }` | hold the page's reply; refresh the card list |

`Read` is used by `wallet_deploymentData` (then answers) and `wallet_watchAsset` (then a card).
`request` itself fails (`CoreError`) only when the wallet's state cannot be read (not when it is
locked). Answer the page UNKNOWN_ERROR (163) and nothing is held.

## Cards

`pending_approvals(now_ms)` lists the cards, oldest first, each with its site's warnings. A
blocked site's card can only be denied. Re-read the list after every call that returns
`Pending` or settles anything. `deny(approval_id, now_ms)` declines any card (USER_REFUSED_OP,
except that a watch-asset notice answers `true`).

Approving, by card kind. Each returns the settled replies, or a step loop:

- **Connect**: `approve_connect(approval_id, reviewed_account_id, now_ms)`, echoing the active
  account the person saw beside the card. The grant is written to the vault.
- **SwitchChain**: `approve_switch_chain(approval_id, now_ms)`.
- **WatchAsset**: `approve_watch_asset(approval_id, now_ms)`. Afterwards, write
  `tokens.persisted()` to `preferences.json`, as after any other token edit. The core does not
  write it.
- **Invoke**: `approve_invoke(...)` returns `DappInvokeStart`. `Settled` answers the pages now.
  `Plan { plan }` is a `TransferPlan`, run like any other: `advance` from `Start`, show its
  review (the fee is estimated here), the person's `Confirm`, and a Ledger account's device
  steps. After each step the host handles, call `settle_invoke(approval_id)`. It returns
  nothing while the plan runs, then the page's answer: `{ transaction_hash }` once the node
  accepted it, USER_REFUSED_OP if the plan closed with nothing signed, UNKNOWN_ERROR if it
  closed after.
- **SignTypedData**: `approve_sign_typed_data(approval_id, reviewed_account_id, now_ms)`
  returns a `DappSignStep`; loop until `Settled`:

  | Step | Host does |
  | --- | --- |
  | `Read { network, id, body }` | POST `body`, then `answer_sign_read(approval_id, id, reply)` (an empty reply when the POST failed) |
  | `Device { apdu }` | send to the Ledger, then `answer_sign_device(approval_id, reply)`, or `sign_device_transport_failure(approval_id, failure)` |
  | `DeviceRetry { reason }` | show `reason` (the extension's rejection code); on the person's retry `retry_sign_device(approval_id)`, on their cancel (or the host's sign timeout) `cancel_sign_device(approval_id)` |
  | `Settled { settled }` | deliver each reply |

  The host never passes a hash or a document: the core signs only the card's hash, computed
  after every check.

A decision on a card that has already settled, expired or been withdrawn, or one of another
kind, answers nothing (an empty list or `Settled { settled: [] }`).

## Timers

- **Expiry.** While any request is held, call `settle_expired()` on a timer (once a second is
  plenty) and after every other call. It answers expired and evicted cards as a deny is
  answered.
- **Idle lock.** Only the person's decisions that act through the vault renew it: `approve_connect`,
  `approve_switch_chain`, `approve_invoke` and `approve_sign_typed_data`. A page's requests,
  reads, device steps, `deny` and `approve_watch_asset` never do, so a site cannot keep the
  wallet unlocked. A lock ends a typed-data signing (live read or device) and voids a plan
  that has not signed.

## Navigation

When a page navigates away, reloads or closes, call `withdraw(request_ids)` with every request
id that page still holds. Its requests are dropped unanswered. A card no other page waits on is
withdrawn, a typed-data signing in progress is dropped (the device is sent nothing more), and an
invoke plan no page waits on is closed. A `Pending` reply that arrives after its page has gone is
withdrawn too, not ignored, or its card stays approvable until it expires.

## What is not here

- `wallet_addDeclareTransaction` is refused (163) on mobile, by decision (`docs/PLAN.md`, P16).
- The STRK20 methods wait on P15.
- The inpage provider script, the WebView bridge and the approval screens are Bramble's.
