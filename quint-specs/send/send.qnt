module send {
  // Sans-IO send plan (crates/wallet-core/src/send/plan*.rs). One plan at a time: `new` builds it,
  // `advance(host, reply)` feeds it the host's reply and `close` stops it. The host's pending
  // records (record_pending) are kept beside the plan, since the plan writes them.
  //
  // Abstractions: an RPC body is modeled at the level `judge.rs` classifies it (an `Answer`);
  // felts are small ints; a transaction hash and a contract address are structural records
  // (the hash IS what was signed), never arithmetic mixers; one resource bound stands for the
  // three (l1_gas, l2_gas, l1_data_gas) a fee estimate carries.

  // ---------------------------------------------------------------- types

  type Family = OZ | Argent | Braavos
  /// calculate_contract_address with deployer 0, as a structural record (injective).
  type Address = { pk: int, salt: int }
  type DeploymentOpt = NoDeployment | HasDeployment(int) // the deployment's salt
  type Account = {
    family: Family,
    network: int,
    publicKey: int,
    address: Address,
    deployment: DeploymentOpt,
  }
  type TransferT = { poisoned: bool, amount: int }
  type TxKind = TxInvoke | TxDeploy(int) // deploy carries the deployment salt it signs over
  /// A signed transaction hash: what was signed, structurally.
  type Hash = { tx: TxKind, net: int, addr: Address, nonce: int, amt: int, price: int }
  type HashOpt = NoHash | SomeHash(Hash)
  type Kind = KTransfer(TransferT) | KDeployment | KRefresh(Hash)
  type Policy = { margin: int, num: int, den: int, delays: int }

  type Unit = Fri | Wei
  type FeeQuote = { spend: int, unit: Unit }
  type FeeOpt = NoFee | SomeFee(FeeQuote)
  type Bounds = { amt: int, price: int }
  type BoundsOpt = NoBounds | SomeBounds(Bounds)
  type EntryUnit = EFri | EWei | EAbsent | EOther
  /// One fee-estimate entry: unit, whether every quantity is present and an integer, one bound.
  type Entry = { unit: EntryUnit, wellFormed: bool, amount: int, price: int }
  type QuoteRes = QOk({ bounds: Bounds, quote: FeeQuote }) | QMalformed | QOutOfRange | QUnit
  type Settled = SetBounds(Bounds) | SetRefused(Code)

  type Probe = PrReview | PrReviewDeploy | PrSubmit | PrDeploy
  type Expect =
    | ReviewChain
    | EBlock(Probe)
    | EClass(Probe)
    | EOwner(Probe)
    | EOwnerType(Probe)
    | EGuardian(Probe)
    | ESigners(Probe)
    | EDeferred(Probe)
    | EBlockTime(Probe)
    | ReviewNonce
    | ReviewEstimate
    | ReviewDeployNonce
    | ReviewDeployEstimate
    | SubmitChain
    | SubmitNonce
    | SubmitEstimate
    | DeployNonce
    | DeployEstimate
    | DeployWrite
    | DeployStatus
    | DeployReceipt
    | InvokeNonce
    | InvokeWrite
    | ConfirmStatus
    | RefreshStatus
  type AfterWait = AWDeployStatus | AWDeployReceipt | AWCheckpoint
  type TxStatus = Pending | Accepted | Rejected | Reverted
  type Stage =
    | Fresh
    | Awaiting({ id: int, expect: Expect })
    | Waiting(AfterWait)
    | Reviewed
    | Broadcasted
    | Finished(TxStatus)
    | Closed

  type CallAns = CVal(int) | CValNone | CNodeErr | CUnread
  type OwnerOpt = NoOwner | HeldOwner(CallAns)
  type DeployedOpt = DUnknown | DKnown(bool)

  type Work = {
    nextId: int,
    deployed: DeployedOpt,
    nonce: int,
    block: int,
    owner: OwnerOpt,
    expireAt: int,
    fee: FeeOpt,
    deployFee: FeeOpt,
    requiresDeployment: bool,
    alreadyDeployed: bool,
    reviewedFee: FeeOpt,
    reviewedDeployFee: FeeOpt,
    submitDeployed: bool,
    invokeBounds: BoundsOpt,
    deployHash: HashOpt,
    deploymentAccepted: bool,
    attempts: int,
    pinAttempts: int,
    invokeHash: HashOpt,
    checkpoint: int,
  }

  type Review = {
    feeFailed: bool,
    deployFeeFailed: bool,
    requiresDeployment: bool,
    alreadyDeployed: bool,
    canConfirm: bool,
  }
  type ReviewOpt = NoReview | SomeReview(Review)

  type Plan = {
    account: Account,
    kind: Kind,
    policy: Policy,
    stage: Stage,
    work: Work,
    signatures: int,
    review: ReviewOpt,
  }

  type Code =
    | WrongChain
    | PoisonedRecipient
    | Locked
    | SigningFailed
    | NodeError
    | PreparationUnavailable
    | ReviewedFeeRequired
    | ReviewedFeeExceeded
    | ReviewedDeploymentFeeRequired
    | ReviewedDeploymentFeeExceeded
    | BoundsOutOfRange
    | UnrecognisedFeeUnit
    | SubmissionIndeterminate
    | HashMismatch
    | DeploymentUnconfirmed
    | DeploymentStatusUnknown
    | AutoDeploymentFailed
    | DeploymentThenInvokeUnconfirmed
    | AlreadyDeployed
    | AccountNotDeployable
    | BraavosNotDeployable
    | BraavosDeployFirst
    | AccountOwnerMismatch
    | AccountOwnerTypeUnsupported
    | AccountGuardianUnsupported
    | AccountClassUnrecognised
    | AccountClassUnsupported
    | BraavosClassUnrecognised
    | BraavosOwnerMismatch
    | BraavosSignerListMalformed
    | BraavosStrongSigner
  type Failure = { code: Code, hash: HashOpt }
  type FailureOpt = NoFailure | SomeFailure(Failure)
  type Outcome = ODeployed | OUndeployed | ORefused(Code) | ONodeError | OUnreadable
  type StepT = SRpc | SWait | SReview | SBroadcast | SFinished(TxStatus) | SFailed(Failure)
  type Res = ROk(StepT) | ROutOfOrder | RPlanClosed
  type Call = CNew | CAdvance(Res) | CClose

  type StatusAns = { status: TxStatus, accepted: bool }
  type HashAns = HZero | HIs(Hash)
  /// A reply body as judge.rs classifies it. ANone: oversized, malformed, wrong jsonrpc/id.
  type Answer =
    | ANone
    | AError(int)
    | AFelt(int)
    | AFelts(List[int])
    | ABlock(int)
    | ATime(int)
    | AEstimate(List[Entry])
    | AWrite(HashAns)
    | AStatus(StatusAns)
    | AReceipt
  type Reply =
    | Start
    | Confirm
    | Proceed
    | WaitElapsed
    | CheckAgain
    | RetryEstimate
    | Rpc({ id: int, answer: Answer })
    | TransportFailure(int)
  type SignAns = SignOk | SignOtherKey | SignLocked | SignFailed
  type Host = { session: bool, sign: SignAns, honoursRenew: bool }
  /// The chain the plan's node reports from: its chain id and whether the account is deployed.
  type Node = { chain: int, deployed: bool, fee: Answer }

  type ClassVal = CVClass(int) | CVUndeployed | CVNodeError | CVUnreadable
  type FeltsAns = FOk(List[int]) | FErr(bool)
  type FirstSigner = NoSigner | Signer(int)
  type Signers = SMalformed | SParsed({ first: FirstSigner, strong: bool })
  type NonceAns = NSome(int) | NNone
  type WriteAns = WAccepted | WMismatch | WIndeterminate | WNodeError
  type Obs = ObsFound(StatusAns) | ObsNotFound | ObsUnknown
  type SignRes = { ok: bool, code: Code, plan: Plan }

  /// What one advance produced: the plan, its result, the pending records it persisted and the
  /// writes it handed out, whether a probe refused the account, whether the work was restarted.
  type Out = {
    plan: Plan,
    result: Res,
    persisted: Set[Hash],
    handed: Set[Hash],
    refused: bool,
    restarted: bool,
  }

  // ---------------------------------------------------------------- constants

  pure val SEPOLIA: int = 1
  pure val MAINNET: int = 2
  pure val PK: int = 7
  pure val OTHER_PK: int = 9
  pure val OZ_CLASS: int = 60
  pure val ARGENT_OK: int = 40     // signer_array = true ('argent 0.4.0')
  pure val ARGENT_OLD: int = 31    // signer_array = false ('argent 0.3.1')
  pure val BRAAVOS_CLASS: int = 50
  pure val UNKNOWN_CLASS: int = 1234
  pure val BLOCK_HASH: int = 77
  pure val U64_MAX: int = 2^8 - 1      // u64::MAX, scaled down: the simulator has bounded ints
  pure val U128_MAX: int = 2^16 - 1    // u128::MAX, scaled the same way
  pure val U256_LIMIT: int = 2^32     // 2^256, scaled the same way
  pure val PIN_ATTEMPTS: int = 3          // plan_probe.rs:39
  pure val DEPLOY_POLL_ATTEMPTS: int = 200 // plan.rs:24
  pure val NO_HASHES: Set[Hash] = Set()
  pure val ACCOUNT_ADDRESS: Address = { pk: PK, salt: 0 }
  pure val REFRESH_HASH: Hash =
    { tx: TxInvoke, net: SEPOLIA, addr: ACCOUNT_ADDRESS, nonce: 5, amt: 1, price: 1 }
  pure val BOGUS_HASH: Hash =
    { tx: TxInvoke, net: SEPOLIA, addr: ACCOUNT_ADDRESS, nonce: 999, amt: 0, price: 0 }

  pure val WORK0: Work = {
    nextId: 0,
    deployed: DUnknown,
    nonce: 0,
    block: 0,
    owner: NoOwner,
    expireAt: 0,
    fee: NoFee,
    deployFee: NoFee,
    requiresDeployment: false,
    alreadyDeployed: false,
    reviewedFee: NoFee,
    reviewedDeployFee: NoFee,
    submitDeployed: false,
    invokeBounds: NoBounds,
    deployHash: NoHash,
    deploymentAccepted: false,
    attempts: 0,
    pinAttempts: 0,
    invokeHash: NoHash,
    checkpoint: 0,
  }

  // ---------------------------------------------------------------- account / kind helpers

  pure def deriveAddress(pk: int, salt: int): Address = { pk: pk, salt: salt }
  pure def isOZ(p: Plan): bool = p.account.family == OZ
  pure def isArgent(p: Plan): bool = p.account.family == Argent
  pure def isBraavos(p: Plan): bool = p.account.family == Braavos
  pure def hasDeployment(p: Plan): bool = p.account.deployment != NoDeployment
  pure def isTransfer(p: Plan): bool = match p.kind { | KTransfer(_) => true | _ => false }
  pure def isPoisoned(k: Kind): bool = match k { | KTransfer(t) => t.poisoned | _ => false }
  /// Transfer::new / transfer_execute_calldata: an amount at or above 2^256 has no calldata.
  pure def transferAmountFits(k: Kind): bool =
    match k { | KTransfer(t) => t.amount >= 0 and t.amount < U256_LIMIT | _ => true }
  /// calls::parse_token_amount (calls.rs:90-115): an amount whose digits are all zero is
  /// NotPositive, so no Transfer carries it.
  pure def transferAmountPositive(k: Kind): bool =
    match k { | KTransfer(t) => t.amount > 0 | _ => true }
  pure def notDeployable(p: Plan): Code =
    if (isBraavos(p)) BraavosNotDeployable else AccountNotDeployable

  /// Plan::new (plan.rs:174): a deployment that does not reproduce the account's address is dropped.
  pure def filterDeployment(a: Account): DeploymentOpt =
    match a.deployment {
      | HasDeployment(salt) =>
        if (deriveAddress(a.publicKey, salt) == a.address) a.deployment else NoDeployment
      | NoDeployment => NoDeployment
    }

  pure def newPlan(account: Account, kind: Kind, policy: Policy): Plan = {
    account: { ...account, deployment: filterDeployment(account) },
    kind: kind,
    policy: policy,
    stage: Fresh,
    work: WORK0,
    signatures: 0,
    review: NoReview,
  }

  // ---------------------------------------------------------------- fees (fees.rs)

  /// x + floor(x*p/100), fees.rs:109-116.
  pure def withMargin(x: int, p: int): int = x + (x * p) / 100

  /// fees::quote: unit first, then quantities, then margin overflow at u128.
  pure def quote(e: Entry, margin: int): QuoteRes =
    if (e.unit == EOther) QUnit
    else if (not(e.wellFormed)) QMalformed
    else if (e.amount > U128_MAX or e.price > U128_MAX) QOutOfRange
    else {
      val a = withMargin(e.amount, margin)
      val pr = withMargin(e.price, margin)
      if (a > U128_MAX or pr > U128_MAX) QOutOfRange
      else QOk({
        bounds: { amt: a, price: pr },
        quote: { spend: a * pr, unit: if (e.unit == EWei) Wei else Fri },
      })
    }

  /// fees::krusty_bounds: max_amount must fit u64.
  pure def krustyFits(b: Bounds): bool = b.amt <= U64_MAX

  /// fees::exceeds_reviewed (fees.rs:173-185): unit change, zero denominator, or overflow fail closed.
  pure def exceedsReviewed(r: FeeQuote, q: FeeQuote, pol: Policy): bool =
    if (r.unit != q.unit or pol.den == 0) true
    else {
      val lhs = q.spend * pol.den
      val rhs = r.spend * pol.num
      lhs >= U256_LIMIT or rhs >= U256_LIMIT or lhs > rhs
    }

  pure def entryQuote(es: List[Entry], idx: int, margin: int): FeeOpt =
    if (idx >= es.length()) NoFee
    else match quote(es.nth(idx), margin) {
      | QOk(q) => SomeFee(q.quote)
      | _ => NoFee
    }

  pure def settleQuoted(q: { bounds: Bounds, quote: FeeQuote }, r: FeeQuote, pol: Policy, exceeded: Code): Settled =
    if (exceedsReviewed(r, q.quote, pol)) SetRefused(exceeded)
    else if (krustyFits(q.bounds)) SetBounds(q.bounds)
    else SetRefused(BoundsOutOfRange)

  /// plan_submit::settle.
  pure def settle(p: Plan, es: List[Entry], idx: int, r: FeeQuote, exceeded: Code, missing: Code): Settled =
    if (idx >= es.length()) SetRefused(missing)
    else match quote(es.nth(idx), p.policy.margin) {
      | QOk(q) => settleQuoted(q, r, p.policy, exceeded)
      | QOutOfRange => SetRefused(exceeded)
      | QUnit => SetRefused(UnrecognisedFeeUnit)
      | QMalformed => SetRefused(PreparationUnavailable)
    }

  // ---------------------------------------------------------------- judge.rs classifiers

  pure def chainMatches(a: Answer, net: int): int =
    // 1 match, 0 mismatch, -1 unreadable
    match a { | AFelt(c) => if (c == net) 1 else 0 | _ => -1 }
  pure def blockNotFound(a: Answer): bool = match a { | AError(c) => c == 24 | _ => false }
  pure def classHashOutcome(a: Answer): Outcome =
    match a {
      | ANone => OUnreadable
      | AError(c) => if (c == 20) OUndeployed else ONodeError
      | _ => ODeployed
    }
  pure def classValue(a: Answer): ClassVal =
    match a {
      | AFelt(f) => if (f == 0) CVUndeployed else CVClass(f)
      | AError(c) => if (c == 20) CVUndeployed else CVNodeError
      | _ => CVUnreadable
    }
  pure def singleFelt(a: Answer): CallAns =
    match a {
      | AFelts(l) => if (l.length() == 1) CVal(l.nth(0)) else CValNone
      | AError(_) => CNodeErr
      | ANone => CUnread
      | _ => CValNone
    }
  pure def callFelts(a: Answer): FeltsAns =
    match a { | AFelts(l) => FOk(l) | AError(_) => FErr(true) | _ => FErr(false) }
  pure def nonceOf(a: Answer): NonceAns =
    match a { | AFelt(n) => NSome(n) | AError(_) => NSome(0) | _ => NNone }
  pure def judgeHash(x: HashAns, h: Hash): WriteAns =
    match x { | HZero => WIndeterminate | HIs(g) => if (g == h) WAccepted else WMismatch }
  pure def judgeWrite(a: Answer, h: Hash): WriteAns =
    match a { | AWrite(x) => judgeHash(x, h) | AError(_) => WNodeError | _ => WIndeterminate }
  pure def judgeStatus(a: Answer): Obs =
    match a {
      | AStatus(s) => ObsFound(s)
      | AError(c) => if (c == 29) ObsNotFound else ObsFound({ status: Pending, accepted: false })
      | ANone => ObsUnknown
      | _ => ObsFound({ status: Pending, accepted: false })
    }
  pure def obsStatus(o: Obs): TxStatus = match o { | ObsFound(s) => s.status | _ => Pending }
  pure def obsAccepted(o: Obs): bool = match o { | ObsFound(s) => s.accepted | _ => false }
  pure def readFailed(node: bool): Outcome = if (node) ONodeError else OUnreadable

  /// plan_probe::braavos_signers: [stark_len, ...stark, secp_len, ...secp, webauthn_len, ...webauthn].
  pure def lenAt(l: List[int], off: int): int = if (off < l.length()) l.nth(off) else 0
  pure def braavosSigners(l: List[int]): Signers = {
    val len0 = lenAt(l, 0)
    val end0 = 1 + len0
    val len1 = lenAt(l, end0)
    val end1 = end0 + 1 + len1
    val len2 = lenAt(l, end1)
    val end2 = end1 + 1 + len2
    if (end0 > l.length() or end1 > l.length() or end2 > l.length()) SMalformed
    else SParsed({ first: if (len0 > 0) Signer(l.nth(1)) else NoSigner, strong: len1 + len2 > 0 })
  }

  // ---------------------------------------------------------------- step helpers (plan.rs)

  pure def mk(p: Plan, r: Res): Out =
    { plan: p, result: r, persisted: NO_HASHES, handed: NO_HASHES, refused: false, restarted: false }
  pure def rpc(p: Plan, e: Expect): Out =
    mk({ ...p, work: { ...p.work, nextId: p.work.nextId + 1 },
         stage: Awaiting({ id: p.work.nextId + 1, expect: e }) }, ROk(SRpc))
  /// persist_pending, then the write RPC.
  pure def rpcWrite(p: Plan, e: Expect, h: Hash): Out =
    { ...rpc(p, e), persisted: Set(h), handed: Set(h) }
  pure def failWith(p: Plan, code: Code, h: HashOpt): Out =
    mk({ ...p, stage: Closed }, ROk(SFailed({ code: code, hash: h })))
  pure def waitFor(p: Plan, a: AfterWait): Out = mk({ ...p, stage: Waiting(a) }, ROk(SWait))
  pure def broadcast(p: Plan): Out = mk({ ...p, stage: Broadcasted }, ROk(SBroadcast))
  pure def finish(p: Plan, s: TxStatus): Out = mk({ ...p, stage: Finished(s) }, ROk(SFinished(s)))

  pure def targetHash(p: Plan): HashOpt =
    match p.kind {
      | KTransfer(_) => p.work.invokeHash
      | KDeployment => p.work.deployHash
      | KRefresh(h) => SomeHash(h)
    }

  /// plan.rs start: resets the work except next_id.
  pure def start(p: Plan): Out = {
    val p1 = { ...p, work: { ...WORK0, nextId: p.work.nextId } }
    val o = match p.kind { | KRefresh(_) => rpc(p1, RefreshStatus) | _ => rpc(p1, ReviewChain) }
    { ...o, restarted: true }
  }

  pure def checkpoint(p: Plan, idx: int): Out =
    if (targetHash(p) == NoHash) failWith(p, SubmissionIndeterminate, NoHash)
    else if (idx < p.policy.delays)
      waitFor({ ...p, work: { ...p.work, checkpoint: idx } }, AWCheckpoint)
    else finish(p, Pending)

  /// Host signing (sign_renewing): `times` signatures (Braavos signs twice).
  pure def signWith(p: Plan, host: Host, times: int): SignRes =
    if (host.sign == SignLocked) { ok: false, code: Locked, plan: p }
    else if (host.sign == SignOk) { ok: true, code: SigningFailed, plan: { ...p, signatures: p.signatures + times } }
    else { ok: false, code: SigningFailed, plan: p }

  // ---------------------------------------------------------------- review (plan_review.rs)

  pure def canConfirm(p: Plan): bool =
    match p.kind {
      | KTransfer(t) =>
        not(t.poisoned) and p.work.fee != NoFee
          and (not(p.work.requiresDeployment) or p.work.deployFee != NoFee)
      | KDeployment => not(p.work.alreadyDeployed) and p.work.deployFee != NoFee
      | KRefresh(_) => false
    }

  pure def review(p: Plan): Out = {
    val req = p.work.requiresDeployment and not(p.work.alreadyDeployed)
    val r: Review =
      if (isTransfer(p)) {
        feeFailed: p.work.fee == NoFee,
        deployFeeFailed: req and p.work.deployFee == NoFee,
        requiresDeployment: req,
        alreadyDeployed: false,
        canConfirm: canConfirm(p),
      } else {
        feeFailed: false,
        deployFeeFailed: not(p.work.alreadyDeployed) and p.work.deployFee == NoFee,
        requiresDeployment: req,
        alreadyDeployed: p.work.alreadyDeployed,
        canConfirm: canConfirm(p),
      }
    mk({ ...p, stage: Reviewed, review: SomeReview(r) }, ROk(SReview))
  }

  pure def reviewUnknown(p: Plan): Out =
    review({ ...p, work: { ...p.work, requiresDeployment: true, fee: NoFee, deployFee: NoFee } })

  pure def probe(p: Plan, pr: Probe): Out = {
    val p1 = { ...p, work: { ...p.work, pinAttempts: 0 } }
    if (isOZ(p1)) rpc(p1, EClass(pr)) else rpc(p1, EBlock(pr))
  }

  pure def reviewDeployment(p: Plan): Out =
    if (isArgent(p)) probe(p, PrReviewDeploy) else rpc(p, ReviewDeployNonce)

  pure def readDeployNonceIfDeployable(p: Plan): Out =
    if (not(hasDeployment(p))) { if (isBraavos(p)) failWith(p, BraavosNotDeployable, NoHash) else review(p) }
    else rpc(p, ReviewDeployNonce)

  pure def reviewProbed(p: Plan, o: Outcome): Out =
    match o {
      | ODeployed => {
        val p1 = { ...p, work: { ...p.work, deployed: DKnown(true) } }
        if (p.kind == KDeployment) review({ ...p1, work: { ...p1.work, alreadyDeployed: true } })
        else rpc(p1, ReviewNonce)
      }
      | OUndeployed => {
        val p1 = { ...p, work: { ...p.work, deployed: DKnown(false), requiresDeployment: true } }
        if (p.kind == KDeployment) readDeployNonceIfDeployable(p1)
        else if (isBraavos(p1)) {
          if (hasDeployment(p1)) failWith(p1, BraavosDeployFirst, NoHash)
          else failWith(p1, BraavosNotDeployable, NoHash)
        }
        else if (not(hasDeployment(p1))) reviewDeployment(p1)
        else rpc(p1, ReviewNonce)
      }
      | ORefused(c) =>
        if (isBraavos(p)) failWith(p, c, NoHash)
        else review({ ...p, work: { ...p.work, deployed: DKnown(true) } })
      | ONodeError => reviewUnknown(p)
      | OUnreadable => reviewUnknown(p)
    }

  pure def reviewDeployProbed(p: Plan, o: Outcome): Out =
    match o {
      | ODeployed => readDeployNonceIfDeployable(p)
      | OUndeployed => readDeployNonceIfDeployable(p)
      | ORefused(c) => if (isBraavos(p)) failWith(p, c, NoHash) else review(p)
      | _ => review(p)
    }

  // ---------------------------------------------------------------- deployment (plan_deploy.rs)

  /// estimate_deployment: Braavos signs its query hash (twice: aux params, then the hash).
  pure def estimateDeployment(p: Plan, host: Host, e: Expect): Out =
    if (hasDeployment(p) and isBraavos(p)) {
      val s = signWith(p, host, 2)
      if (s.ok) rpc(s.plan, e) else failWith(s.plan, s.code, NoHash)
    } else rpc(p, e)

  pure def deployTransport(p: Plan): Code =
    if (isTransfer(p)) DeploymentStatusUnknown else PreparationUnavailable
  pure def deployNode(p: Plan): Code = if (isTransfer(p)) AutoDeploymentFailed else NodeError

  pure def signDeploy(p: Plan, host: Host, b: Bounds): Out =
    match p.account.deployment {
      | NoDeployment => failWith(p, AccountNotDeployable, NoHash)
      | HasDeployment(salt) => {
        // sign::deploy_hash: chain id from the plan's network, nonce 0.
        val h: Hash = { tx: TxDeploy(salt), net: p.account.network, addr: p.account.address,
                        nonce: 0, amt: b.amt, price: b.price }
        val s = signWith(p, host, if (isBraavos(p)) 2 else 1)
        if (not(s.ok)) failWith(s.plan, s.code, NoHash)
        else rpcWrite({ ...s.plan, work: { ...s.plan.work, deployHash: SomeHash(h) } }, DeployWrite, h)
      }
    }

  pure def onDeploySettled(p: Plan, host: Host, st: Settled): Out =
    match st {
      | SetBounds(b) => signDeploy(p, host, b)
      | SetRefused(c) => if (c == PreparationUnavailable) failWith(p, deployTransport(p), NoHash) else failWith(p, c, NoHash)
    }

  pure def onDeployEstimateReviewed(p: Plan, host: Host, r: FeeQuote, a: Answer): Out =
    match a {
      | AEstimate(es) =>
        onDeploySettled(p, host, settle(p, es, 0, r, ReviewedDeploymentFeeExceeded, deployNode(p)))
      | AError(_) => failWith(p, deployNode(p), NoHash)
      | _ => failWith(p, deployTransport(p), NoHash)
    }

  pure def onDeployEstimate(p: Plan, host: Host, a: Answer): Out =
    match p.work.reviewedDeployFee {
      | NoFee => failWith(p, ReviewedDeploymentFeeRequired, NoHash)
      | SomeFee(r) => onDeployEstimateReviewed(p, host, r, a)
    }

  pure def onDeployWriteHash(p: Plan, h: Hash, a: Answer): Out = {
    val w = judgeWrite(a, h)
    if (isTransfer(p)) {
      if (w == WAccepted) waitFor(p, AWDeployStatus)
      else if (w == WNodeError) failWith(p, AutoDeploymentFailed, SomeHash(h))
      else failWith(p, DeploymentStatusUnknown, SomeHash(h))
    } else {
      if (w == WAccepted) broadcast(p)
      else if (w == WMismatch) failWith(p, HashMismatch, SomeHash(h))
      else if (w == WIndeterminate) failWith(p, SubmissionIndeterminate, SomeHash(h))
      else failWith(p, NodeError, SomeHash(h))
    }
  }

  pure def onDeployWrite(p: Plan, a: Answer): Out =
    match p.work.deployHash {
      | NoHash => failWith(p, SubmissionIndeterminate, NoHash)
      | SomeHash(h) => onDeployWriteHash(p, h, a)
    }

  pure def retryDeployWait(p: Plan, after: AfterWait): Out = {
    val p1 = { ...p, work: { ...p.work, attempts: p.work.attempts + 1 } }
    if (p1.work.attempts >= DEPLOY_POLL_ATTEMPTS) failWith(p1, DeploymentUnconfirmed, p1.work.deployHash)
    else waitFor(p1, after)
  }

  pure def onDeployStatus(p: Plan, a: Answer): Out =
    if (p.work.deployHash == NoHash) failWith(p, DeploymentStatusUnknown, NoHash)
    else {
      val o = judgeStatus(a)
      if (obsAccepted(o)) rpc(p, DeployReceipt)
      else if (o == ObsFound({ status: Rejected, accepted: false }) or o == ObsFound({ status: Rejected, accepted: true }))
        failWith(p, DeploymentUnconfirmed, p.work.deployHash)
      else retryDeployWait(p, AWDeployStatus)
    }

  pure def afterWait(p: Plan, w: AfterWait): Out =
    if (w == AWCheckpoint) {
      if (targetHash(p) == NoHash) failWith(p, SubmissionIndeterminate, NoHash) else rpc(p, ConfirmStatus)
    } else if (p.work.deployHash == NoHash) failWith(p, DeploymentStatusUnknown, NoHash)
    else if (w == AWDeployStatus) rpc(p, DeployStatus)
    else rpc(p, DeployReceipt)

  // ---------------------------------------------------------------- submit (plan_submit.rs)

  pure def afterDeployment(p: Plan, code: Code, h: HashOpt): Out =
    if (p.work.deploymentAccepted) failWith(p, DeploymentThenInvokeUnconfirmed, p.work.deployHash)
    else failWith(p, code, h)

  /// confirm: poisoned recipient, then session, then can_confirm (OutOfOrder).
  pure def confirm(p: Plan, host: Host): Out =
    if (isPoisoned(p.kind)) failWith(p, PoisonedRecipient, NoHash)
    else if (not(host.session)) failWith(p, Locked, NoHash)
    else if (not(canConfirm(p))) mk(p, ROutOfOrder)
    else rpc({ ...p, work: { ...p.work, reviewedFee: p.work.fee,
                             reviewedDeployFee: p.work.deployFee, attempts: 0 } }, SubmitChain)

  pure def submitProbed(p: Plan, o: Outcome): Out =
    match o {
      | ODeployed =>
        if (not(isTransfer(p))) failWith(p, AlreadyDeployed, NoHash)
        else rpc({ ...p, work: { ...p.work, submitDeployed: true } }, SubmitNonce)
      | OUndeployed =>
        if (isTransfer(p) and isBraavos(p) and hasDeployment(p)) failWith(p, BraavosDeployFirst, NoHash)
        else if (not(hasDeployment(p))) failWith(p, notDeployable(p), NoHash)
        else rpc({ ...p, work: { ...p.work, submitDeployed: false } }, SubmitNonce)
      | ORefused(c) => failWith(p, c, NoHash)
      | ONodeError => failWith(p, NodeError, NoHash)
      | OUnreadable => failWith(p, PreparationUnavailable, NoHash)
    }

  pure def deployProbed(p: Plan, o: Outcome): Out =
    match o {
      | ODeployed => rpc(p, InvokeNonce)
      | OUndeployed =>
        if (p.work.reviewedDeployFee == NoFee) failWith(p, ReviewedDeploymentFeeRequired, NoHash)
        else if (not(hasDeployment(p))) failWith(p, notDeployable(p), NoHash)
        else rpc(p, DeployNonce)
      | ORefused(c) => failWith(p, c, NoHash)
      | ONodeError => failWith(p, NodeError, NoHash)
      | OUnreadable => failWith(p, PreparationUnavailable, NoHash)
    }

  pure def onInvokeSettled(p: Plan, st: Settled): Out =
    match st {
      | SetRefused(c) => failWith(p, c, NoHash)
      | SetBounds(b) => {
        val p1 = { ...p, work: { ...p.work, invokeBounds: SomeBounds(b) } }
        if (p.work.submitDeployed) rpc(p1, InvokeNonce) else probe(p1, PrDeploy)
      }
    }

  pure def onInvokeEstimateReviewed(p: Plan, r: FeeQuote, a: Answer): Out =
    match a {
      | AEstimate(es) =>
        onInvokeSettled(p, settle(p, es, if (p.work.submitDeployed) 0 else 1, r, ReviewedFeeExceeded, NodeError))
      | AError(_) => failWith(p, NodeError, NoHash)
      | _ => failWith(p, PreparationUnavailable, NoHash)
    }

  pure def onInvokeEstimate(p: Plan, a: Answer): Out =
    match p.work.reviewedFee {
      | NoFee => failWith(p, ReviewedFeeRequired, NoHash)
      | SomeFee(r) => onInvokeEstimateReviewed(p, r, a)
    }

  pure def signInvoke(p: Plan, host: Host, n: int, b: Bounds): Out =
    if (not(isTransfer(p))) failWith(p, ReviewedFeeRequired, NoHash)
    else if (not(krustyFits(b))) afterDeployment(p, BoundsOutOfRange, NoHash)
    else {
      // sign::invoke_hash: chain id from the plan's network.
      val h: Hash = { tx: TxInvoke, net: p.account.network, addr: p.account.address,
                      nonce: n, amt: b.amt, price: b.price }
      val s = signWith(p, host, 1)
      if (not(s.ok)) afterDeployment(s.plan, s.code, NoHash)
      else rpcWrite({ ...s.plan, work: { ...s.plan.work, invokeHash: SomeHash(h) } }, InvokeWrite, h)
    }

  pure def signAndSendInvoke(p: Plan, host: Host, n: int): Out =
    match p.work.invokeBounds {
      | NoBounds => failWith(p, ReviewedFeeRequired, NoHash)
      | SomeBounds(b) => signInvoke(p, host, n, b)
    }

  pure def onInvokeWriteHash(p: Plan, h: Hash, a: Answer): Out = {
    val w = judgeWrite(a, h)
    if (w == WAccepted) broadcast(p)
    else if (w == WMismatch) afterDeployment(p, HashMismatch, SomeHash(h))
    else if (w == WIndeterminate) afterDeployment(p, SubmissionIndeterminate, SomeHash(h))
    else afterDeployment(p, NodeError, SomeHash(h))
  }

  pure def onInvokeWrite(p: Plan, a: Answer): Out =
    match p.work.invokeHash {
      | NoHash => failWith(p, SubmissionIndeterminate, NoHash)
      | SomeHash(h) => onInvokeWriteHash(p, h, a)
    }

  pure def onConfirmStatus(p: Plan, e: Expect, a: Answer): Out =
    if (targetHash(p) == NoHash) failWith(p, SubmissionIndeterminate, NoHash)
    else {
      val s = obsStatus(judgeStatus(a))
      if (e == RefreshStatus or s != Pending) finish(p, s) else checkpoint(p, p.work.checkpoint + 1)
    }

  pure def withNonce(p: Plan, n: int): Plan = { ...p, work: { ...p.work, nonce: n } }

  pure def onSubmitReply(p: Plan, host: Host, e: Expect, a: Answer): Out =
    if (e == SubmitChain) {
      val c = chainMatches(a, p.account.network)
      if (c == 1) probe(p, PrSubmit)
      else if (c == 0) failWith(p, WrongChain, NoHash)
      else failWith(p, PreparationUnavailable, NoHash)
    } else if (e == SubmitNonce) {
      match nonceOf(a) {
        | NSome(n) =>
          if (isTransfer(p)) rpc(withNonce(p, n), SubmitEstimate)
          else estimateDeployment(withNonce(p, n), host, DeployEstimate)
        | NNone => failWith(p, PreparationUnavailable, NoHash)
      }
    } else if (e == SubmitEstimate) onInvokeEstimate(p, a)
    else if (e == DeployNonce) {
      match nonceOf(a) {
        | NSome(n) => estimateDeployment(withNonce(p, n), host, DeployEstimate)
        | NNone => failWith(p, DeploymentStatusUnknown, NoHash)
      }
    } else if (e == DeployEstimate) onDeployEstimate(p, host, a)
    else if (e == DeployWrite) onDeployWrite(p, a)
    else if (e == DeployStatus) onDeployStatus(p, a)
    else if (e == DeployReceipt) {
      if (a == AReceipt)
        rpc({ ...p, work: { ...p.work, deploymentAccepted: true, attempts: 0 } }, InvokeNonce)
      else retryDeployWait(p, AWDeployReceipt)
    } else if (e == InvokeNonce) {
      match nonceOf(a) {
        | NSome(n) => signAndSendInvoke(p, host, n)
        | NNone => afterDeployment(p, PreparationUnavailable, NoHash)
      }
    } else if (e == InvokeWrite) onInvokeWrite(p, a)
    else if (e == ConfirmStatus or e == RefreshStatus) onConfirmStatus(p, e, a)
    else failWith(p, NodeError, NoHash)

  pure def onReviewEstimate(p: Plan, a: Answer): Out = {
    val deployed = p.work.deployed == DKnown(true)
    val p1 = match a {
      | AEstimate(es) =>
        { ...p, work: { ...p.work, fee: entryQuote(es, if (deployed) 0 else 1, p.policy.margin) } }
      | _ => p
    }
    if (deployed) review(p1) else reviewDeployment(p1)
  }

  pure def onReviewReply(p: Plan, host: Host, e: Expect, a: Answer): Out =
    if (e == ReviewChain) {
      val c = chainMatches(a, p.account.network)
      if (c == 1) probe(p, PrReview)
      else if (c == 0) failWith(p, WrongChain, NoHash)
      else reviewUnknown(p)
    } else if (e == ReviewNonce) {
      match nonceOf(a) {
        | NSome(n) => rpc(withNonce(p, n), ReviewEstimate)
        | NNone => if (p.work.deployed == DKnown(true)) review(p) else reviewDeployment(p)
      }
    } else if (e == ReviewEstimate) onReviewEstimate(p, a)
    else if (e == ReviewDeployNonce) {
      match nonceOf(a) {
        | NSome(n) => estimateDeployment(withNonce(p, n), host, ReviewDeployEstimate)
        | NNone => review(p)
      }
    } else if (e == ReviewDeployEstimate) {
      match a {
        | AEstimate(es) =>
          review({ ...p, work: { ...p.work, deployFee: entryQuote(es, 0, p.policy.margin) } })
        | _ => review(p)
      }
    } else failWith(p, NodeError, NoHash)

  // ---------------------------------------------------------------- probe (plan_probe.rs)

  pure def probeOf(e: Expect): Probe =
    match e {
      | EBlock(x) => x
      | EClass(x) => x
      | EOwner(x) => x
      | EOwnerType(x) => x
      | EGuardian(x) => x
      | ESigners(x) => x
      | EDeferred(x) => x
      | EBlockTime(x) => x
      | _ => PrReview
    }
  pure def isProbeExpect(e: Expect): bool =
    match e {
      | EBlock(_) => true
      | EClass(_) => true
      | EOwner(_) => true
      | EOwnerType(_) => true
      | EGuardian(_) => true
      | ESigners(_) => true
      | EDeferred(_) => true
      | EBlockTime(_) => true
      | _ => false
    }
  pure def isReviewExpect(e: Expect): bool =
    Set(ReviewChain, ReviewNonce, ReviewEstimate, ReviewDeployNonce, ReviewDeployEstimate).contains(e)
  /// repin_on_block_miss applies to pinned reads only (not the Block read, not OZ's `latest`).
  pure def repinApplies(p: Plan, e: Expect): bool =
    match e {
      | EClass(_) => not(isOZ(p))
      | EBlock(_) => false
      | _ => isProbeExpect(e)
    }

  pure def probedOut(p: Plan, pr: Probe, o: Outcome): Out = {
    val r = match pr {
      | PrReview => reviewProbed(p, o)
      | PrReviewDeploy => reviewDeployProbed(p, o)
      | PrSubmit => submitProbed(p, o)
      | PrDeploy => deployProbed(p, o)
    }
    val isRefusal = match o { | ORefused(_) => true | _ => false }
    { ...r, refused: isRefusal }
  }

  pure def onClass(p: Plan, pr: Probe, c: int): Out =
    if (pr == PrReview and p.kind == KDeployment) probedOut(p, pr, ODeployed)
    else if (isBraavos(p)) {
      if (c == BRAAVOS_CLASS) rpc(p, ESigners(pr)) else probedOut(p, pr, ORefused(BraavosClassUnrecognised))
    } else if (isArgent(p)) {
      if (c == ARGENT_OK) rpc(p, EOwner(pr))
      else if (c == ARGENT_OLD) probedOut(p, pr, ORefused(AccountClassUnsupported))
      else probedOut(p, pr, ORefused(AccountClassUnrecognised))
    } else failWith(p, NodeError, NoHash)

  pure def onPinnedClass(p: Plan, pr: Probe, a: Answer): Out =
    match classValue(a) {
      | CVClass(c) => onClass(p, pr, c)
      | CVUndeployed => probedOut(p, pr, OUndeployed)
      | CVNodeError => probedOut(p, pr, ONodeError)
      | CVUnreadable => probedOut(p, pr, OUnreadable)
    }

  pure def heldOwner(p: Plan): CallAns = match p.work.owner { | HeldOwner(o) => o | NoOwner => CUnread }

  pure def onOwnerType(p: Plan, pr: Probe, a: Answer): Out = {
    val owner = heldOwner(p)
    val t = singleFelt(a)
    val p1 = { ...p, work: { ...p.work, owner: NoOwner } }
    if (owner == CNodeErr) probedOut(p1, pr, ONodeError)
    else if (owner == CUnread) probedOut(p1, pr, OUnreadable)
    else if (t == CNodeErr) probedOut(p1, pr, ONodeError)
    else if (t == CUnread) probedOut(p1, pr, OUnreadable)
    else if (owner != CVal(p.account.publicKey)) probedOut(p1, pr, ORefused(AccountOwnerMismatch))
    else if (t != CVal(0)) probedOut(p1, pr, ORefused(AccountOwnerTypeUnsupported))
    else rpc(p1, EGuardian(pr))
  }

  pure def guardianOutcome(c: CallAns): Outcome =
    match c {
      | CVal(g) => if (g == 0) ODeployed else ORefused(AccountGuardianUnsupported)
      | CValNone => ORefused(AccountGuardianUnsupported)
      | CNodeErr => ONodeError
      | CUnread => OUnreadable
    }

  pure def onSignerList(p: Plan, pr: Probe, l: List[int]): Out =
    match braavosSigners(l) {
      | SMalformed => probedOut(p, pr, ORefused(BraavosSignerListMalformed))
      | SParsed(s) =>
        if (s.first != Signer(p.account.publicKey)) probedOut(p, pr, ORefused(BraavosOwnerMismatch))
        else if (not(s.strong)) probedOut(p, pr, ODeployed)
        else rpc(p, EDeferred(pr))
    }

  pure def onSigners(p: Plan, pr: Probe, a: Answer): Out =
    match callFelts(a) {
      | FOk(l) => onSignerList(p, pr, l)
      | FErr(node) => probedOut(p, pr, readFailed(node))
    }

  pure def onDeferred(p: Plan, pr: Probe, a: Answer): Out =
    match callFelts(a) {
      | FOk(l) => {
        val ex = if (l.length() > 0) l.nth(0) else 0
        if (ex == 0) probedOut(p, pr, ORefused(BraavosStrongSigner))
        else rpc({ ...p, work: { ...p.work, expireAt: ex } }, EBlockTime(pr))
      }
      | FErr(node) => probedOut(p, pr, readFailed(node))
    }

  pure def onBlockTime(p: Plan, pr: Probe, a: Answer): Out =
    match a {
      | ATime(t) =>
        if (t > p.work.expireAt) probedOut(p, pr, ODeployed)
        else probedOut(p, pr, ORefused(BraavosStrongSigner))
      | AError(_) => probedOut(p, pr, ONodeError)
      | _ => probedOut(p, pr, OUnreadable)
    }

  pure def onBlock(p: Plan, pr: Probe, a: Answer): Out =
    match a {
      | ABlock(h) =>
        if (h != 0) rpc({ ...p, work: { ...p.work, block: h } }, EClass(pr))
        else probedOut(p, pr, OUnreadable)
      | AError(_) => probedOut(p, pr, ONodeError)
      | _ => probedOut(p, pr, OUnreadable)
    }

  pure def probeArm(p: Plan, e: Expect, a: Answer): Out =
    match e {
      | EBlock(pr) => onBlock(p, pr, a)
      | EClass(pr) => if (isOZ(p)) probedOut(p, pr, classHashOutcome(a)) else onPinnedClass(p, pr, a)
      | EOwner(pr) => rpc({ ...p, work: { ...p.work, owner: HeldOwner(singleFelt(a)) } }, EOwnerType(pr))
      | EOwnerType(pr) => onOwnerType(p, pr, a)
      | EGuardian(pr) => probedOut(p, pr, guardianOutcome(singleFelt(a)))
      | ESigners(pr) => onSigners(p, pr, a)
      | EDeferred(pr) => onDeferred(p, pr, a)
      | EBlockTime(pr) => onBlockTime(p, pr, a)
      | _ => failWith(p, NodeError, NoHash)
    }

  /// on_probe_reply: a block miss on a pinned read re-pins (up to PIN_ATTEMPTS), else the arm.
  pure def onProbeReply(p: Plan, e: Expect, a: Answer): Out = {
    val miss = repinApplies(p, e) and blockNotFound(a)
    val pins = p.work.pinAttempts + 1
    if (miss and pins < PIN_ATTEMPTS)
      rpc({ ...p, work: { ...p.work, pinAttempts: pins, owner: NoOwner, expireAt: 0 } }, EBlock(probeOf(e)))
    else if (miss) probeArm({ ...p, work: { ...p.work, pinAttempts: pins } }, e, a)
    else probeArm(p, e, a)
  }

  // ---------------------------------------------------------------- advance (plan.rs)

  pure def onReply(p: Plan, host: Host, e: Expect, a: Answer): Out =
    if (isProbeExpect(e)) onProbeReply(p, e, a)
    else if (isReviewExpect(e)) onReviewReply(p, host, e, a)
    else onSubmitReply(p, host, e, a)

  pure def onAwaiting(p: Plan, host: Host, aw: { id: int, expect: Expect }, reply: Reply): Out =
    match reply {
      | Rpc(r) => if (r.id == aw.id) onReply(p, host, aw.expect, r.answer) else mk(p, ROutOfOrder)
      | TransportFailure(i) => if (i == aw.id) onReply(p, host, aw.expect, ANone) else mk(p, ROutOfOrder)
      | _ => mk(p, ROutOfOrder)
    }

  pure def onFinished(p: Plan, s: TxStatus, reply: Reply): Out =
    if (s == Pending and reply == CheckAgain) checkpoint(p, 0) else mk(p, ROutOfOrder)

  /// Plan::step: the stage is replaced by Closed before the handler runs (plan.rs:241).
  pure def stepPlan(p: Plan, host: Host, reply: Reply): Out = {
    val p0 = { ...p, stage: Closed }
    match p.stage {
      | Fresh => if (reply == Start) start(p0) else mk(p0, ROutOfOrder)
      | Reviewed =>
        if (reply == RetryEstimate) start(p0)
        else if (reply == Confirm) confirm(p0, host)
        else mk(p0, ROutOfOrder)
      | Awaiting(aw) => onAwaiting(p0, host, aw, reply)
      | Waiting(w) => if (reply == WaitElapsed) afterWait(p0, w) else mk(p0, ROutOfOrder)
      | Broadcasted => if (reply == Proceed) checkpoint(p0, 0) else mk(p0, ROutOfOrder)
      | Finished(s) => onFinished(p0, s, reply)
      | Closed => mk(p0, RPlanClosed)
    }
  }

  pure def stepFailure(s: StepT): FailureOpt = match s { | SFailed(f) => SomeFailure(f) | _ => NoFailure }
  pure def resFailure(r: Res): FailureOpt = match r { | ROk(s) => stepFailure(s) | _ => NoFailure }
  pure def closesPlan(r: Res): bool =
    match r { | ROk(s) => stepFailure(s) != NoFailure | _ => true }

  /// Plan::advance: a closed plan answers PlanClosed; a Failed step or an Err closes the plan.
  pure def advanceOut(p: Plan, host: Host, reply: Reply): Out =
    if (p.stage == Closed) mk(p, RPlanClosed)
    else {
      val o = stepPlan(p, host, reply)
      if (closesPlan(o.result)) { ...o, plan: { ...o.plan, stage: Closed } } else o
    }

  // ---------------------------------------------------------------- queries

  /// current_review()
  pure def currentReview(p: Plan): ReviewOpt = p.review
  /// signatures_made()
  pure def signaturesMade(p: Plan): int = p.signatures
  /// interrupted_failure() (plan.rs:217-232)
  pure def interruptedFailure(p: Plan): FailureOpt =
    match p.work.invokeHash {
      | SomeHash(h) => SomeFailure({ code: SubmissionIndeterminate, hash: SomeHash(h) })
      | NoHash =>
        match p.work.deployHash {
          | SomeHash(d) =>
            if (isTransfer(p)) SomeFailure({ code: DeploymentUnconfirmed, hash: SomeHash(d) })
            else SomeFailure({ code: SubmissionIndeterminate, hash: SomeHash(d) })
          | NoHash => NoFailure
        }
    }

  // ---------------------------------------------------------------- host contract

  /// The host refuses to sign once the plan's session has ended (SignRefusal::Locked).
  pure def hostLocksEndedSession(host: Host): bool = host.session or host.sign == SignLocked

  // ---------------------------------------------------------------- state

  var plan: Plan
  /// Environment: the chain the current plan's node reports from.
  var node: Node
  /// Host pending records (record_pending), keyed by signed hash; never removed by the plan.
  var pending: Set[Hash]
  /// Ghost: every signed write the core has handed out, across plans.
  var handedOut: Set[Hash]
  /// Ghost: the signed writes the current plan handed out.
  var planWrites: Set[Hash]
  /// Ghost: a probe of the current attempt (since the last start) refused the account.
  var refusedAttempt: bool
  /// Ghost: whether the host reported the plan's session current at this plan's last Confirm.
  var sessionAtConfirm: bool
  /// Ghost latch: `new` dropped a deployment that does not reproduce the address.
  var droppedDeployment: bool
  /// Ghost latch: the caller closed the current plan.
  var closedByCall: bool
  /// Ghost: the last call made and what advance returned.
  var lastCall: Call

  // ---------------------------------------------------------------- properties

  pure def callFailure(c: Call): FailureOpt = match c { | CAdvance(r) => resFailure(r) | _ => NoFailure }
  def lastFailedWith(code: Code): bool =
    match callFailure(lastCall) { | SomeFailure(f) => f.code == code | NoFailure => false }
  pure def reportsWrite(f: FailureOpt, ws: Set[Hash]): bool =
    match f {
      | SomeFailure(x) => match x.hash { | SomeHash(h) => ws.contains(h) | NoHash => false }
      | NoFailure => false
    }

  pure def reviewedFor(p: Plan, h: Hash): FeeOpt =
    match h.tx { | TxInvoke => p.work.reviewedFee | TxDeploy(_) => p.work.reviewedDeployFee }
  pure def spendWithin(spend: int, r: FeeOpt, pol: Policy): bool =
    match r {
      | SomeFee(q) => pol.den > 0 and spend * pol.den <= q.spend * pol.num
      | NoFee => false
    }
  /// confirm copied the fees into the reviewed ones (plan_submit.rs:95-96); start clears them.
  pure def confirmProceeded(p: Plan): bool =
    p.work.reviewedFee != NoFee or p.work.reviewedDeployFee != NoFee
  pure def reproduces(p: Plan, h: Hash): bool =
    match h.tx { | TxDeploy(salt) => deriveAddress(p.account.publicKey, salt) == h.addr | TxInvoke => true }

  /// A signed write left the core without its pending record: a crash or lost reply leaves a
  /// possibly-landed transaction the wallet never tracks.
  invariant handed_writes_are_pending: bool = handedOut.subseteq(pending)

  /// A Failed step or an Err left the plan open: a stray or replayed reply could drive it on to sign.
  invariant failure_closes_plan: bool =
    match lastCall { | CAdvance(r) => closesPlan(r) implies plan.stage == Closed | _ => true }

  /// A write was signed at a spend above the policy's headroom over what the user reviewed
  /// (or with a zero denominator): the user pays more than they accepted.
  invariant signed_spend_within_reviewed_headroom: bool =
    planWrites.forall(h => spendWithin(h.amt * h.price, reviewedFor(plan, h), plan.policy))

  /// A write was hashed with a chain id other than the plan's network: a replayable or wrong-chain signature.
  invariant signed_writes_use_plan_network: bool =
    planWrites.forall(h => h.net == plan.account.network)

  /// A deployment was signed whose parameters do not reproduce the account's address: funds sent
  /// to the account address are not controlled by what was deployed.
  invariant deployment_signed_only_for_own_address: bool =
    planWrites.forall(h => reproduces(plan, h))

  /// A transfer to a poisoned look-alike recipient was signed: the user's funds go to an attacker.
  /// Confirm for a poisoned recipient took the fees as reviewed and went on towards signing.
  invariant poisoned_recipient_never_signed: bool =
    isPoisoned(plan.kind) implies (planWrites == NO_HASHES and not(confirmProceeded(plan)))

  /// An attempt whose account-state read refused the account went on: it left the plan open
  /// for anything but a review that cannot be confirmed, or it signed a write.
  invariant incompatible_account_never_signed: bool =
    refusedAttempt implies and {
      planWrites == NO_HASHES,
      plan.stage == Closed or (plan.stage == Reviewed and not(canConfirm(plan))),
    }

  /// Confirm took the reviewed fees and went on towards signing although the host reported
  /// the plan's session ended at that Confirm.
  invariant confirm_requires_current_session: bool =
    confirmProceeded(plan) implies sessionAtConfirm

  /// The plan handed out a write but interrupted_failure would report "not sent": the user
  /// retries and pays twice.
  invariant handed_write_is_reported: bool =
    planWrites == NO_HASHES or reportsWrite(interruptedFailure(plan), planWrites)

  /// A write was handed out that the host never signed: the node receives an unauthorised or
  /// unsigned transaction and the signature count under-reports what reached the signer.
  invariant handed_writes_were_signed: bool = signaturesMade(plan) >= planWrites.size()

  /// A plan carries a transfer of a non-positive amount: Transfer validation let a zero or
  /// negative amount through to review and signing.
  invariant transfer_amount_positive: bool =
    match plan.kind { | KTransfer(t) => t.amount > 0 | _ => true }

  /// The review offered Confirm.
  observation review_confirmable: bool =
    plan.stage == Reviewed and currentReview(plan) != NoReview and canConfirm(plan)
  /// The review was shown with Confirm blocked (a fee failed, poisoning, or nothing to deploy).
  observation review_blocked: bool =
    plan.stage == Reviewed and currentReview(plan) != NoReview and not(canConfirm(plan))
  /// The node named another network.
  observation wrong_chain_refused: bool = lastFailedWith(WrongChain)
  /// A reply out of order closed the plan.
  observation out_of_order_closed: bool = lastCall == CAdvance(ROutOfOrder)
  /// After the caller closed it, the plan refuses to advance.
  observation closed_plan_stays_closed: bool = closedByCall and lastCall == CAdvance(RPlanClosed)
  /// An account-state read refused the account (class, owner, guardian or signer checks).
  observation account_refused_at_probe: bool = refusedAttempt
  /// A pinned read missed its block and the plan re-pinned.
  observation repinned_after_block_miss: bool =
    plan.work.pinAttempts > 0 and
      match plan.stage { | Awaiting(a) => match a.expect { | EBlock(_) => true | _ => false } | _ => false }
  /// `new` dropped a deployment that does not reproduce the address.
  observation deployment_dropped: bool = droppedDeployment
  /// Confirm found the session ended.
  observation locked_at_confirm: bool = lastFailedWith(Locked)
  /// Confirm refused a poisoned recipient.
  observation poisoned_recipient_refused: bool = lastFailedWith(PoisonedRecipient)
  /// The plan signed a write and handed it out.
  observation write_handed_out: bool = planWrites != NO_HASHES
  /// A write was handed out and the plan stopped: interrupted_failure reports it.
  observation interrupted_write_reported: bool =
    plan.stage == Closed and reportsWrite(interruptedFailure(plan), planWrites)

  // ---------------------------------------------------------------- actions

  /// Plan::new keeps a deployment that reproduces the account's address (plan.rs:174-176).
  pure def deploymentReproduces(a: Account): bool =
    match a.deployment {
      | HasDeployment(salt) => deriveAddress(a.publicKey, salt) == a.address
      | NoDeployment => true
    }

  /// Plan::new's assignments; `dropped` says whether the filter at plan.rs:174 dropped the deployment.
  action applyNew(account: Account, kind: Kind, policy: Policy, chainState: Node, dropped: bool): bool = all {
    plan' = newPlan(account, kind, policy),
    node' = chainState,
    pending' = pending,
    handedOut' = handedOut,
    planWrites' = NO_HASHES,
    refusedAttempt' = false,
    sessionAtConfirm' = false,
    droppedDeployment' = dropped,
    closedByCall' = false,
    lastCall' = CNew,
  }

  /// Plan::new, the deployment reproduces the account's address and is kept.
  action newKeepingDeployment(account: Account, kind: Kind, policy: Policy, chainState: Node): bool = all {
    deploymentReproduces(account),
    applyNew(account, kind, policy, chainState, false),
  }

  /// Plan::new, the deployment does not reproduce the address and is dropped (plan.rs:174).
  action newDroppingDeployment(account: Account, kind: Kind, policy: Policy, chainState: Node): bool = all {
    not(deploymentReproduces(account)),
    applyNew(account, kind, policy, chainState, true),
  }

  /// Plan::new(account, kind, policy). `chainState` is not an argument of the code's `new`: it is
  /// the chain the plan's node reports from, fixed with the plan so a well-behaved node answers
  /// consistently (one chain id, one deployment state) across the review and the submit.
  action new(account: Account, kind: Kind, policy: Policy, chainState: Node): bool = all {
    transferAmountFits(kind),
    transferAmountPositive(kind),
    any {
      newKeepingDeployment(account, kind, policy, chainState),
      newDroppingDeployment(account, kind, policy, chainState),
    },
  }

  /// Plan::advance's assignments, whatever branch the code took.
  action applyAdvance(host: Host, reply: Reply): bool = {
    val o = advanceOut(plan, host, reply)
    all {
      plan' = o.plan,
      node' = node,
      pending' = pending.union(o.persisted),
      handedOut' = handedOut.union(o.handed),
      planWrites' = planWrites.union(o.handed),
      refusedAttempt' = ((refusedAttempt and not(o.restarted)) or o.refused),
      sessionAtConfirm' = if (isConfirmCall(plan, reply)) host.session else sessionAtConfirm,
      droppedDeployment' = droppedDeployment,
      closedByCall' = closedByCall,
      lastCall' = CAdvance(o.result),
    }
  }

  // The branches of `advance` whose conditions the code checks and the intents name.

  pure def replyAnswer(r: Reply): Answer = match r { | Rpc(x) => x.answer | _ => ANone }
  pure def answersOutstanding(p: Plan, r: Reply): bool =
    isAwaiting(p.stage) and
      match r {
        | Rpc(x) => x.id == outstandingId(p)
        | TransportFailure(i) => i == outstandingId(p)
        | _ => false
      }
  /// judge::answer_within: an Rpc body on the outstanding id that is well-formed JSON-RPC 2.0
  /// within the size bound (anything else is ANone, as a transport failure is).
  pure def answerUsable(p: Plan, r: Reply): bool =
    answersOutstanding(p, r) and match r { | Rpc(x) => x.answer != ANone | _ => false }
  pure def isConfirmCall(p: Plan, r: Reply): bool = p.stage == Reviewed and r == Confirm
  pure def isChainReply(p: Plan, r: Reply): bool =
    answersOutstanding(p, r) and Set(ReviewChain, SubmitChain).contains(awaitedExpect(p))
  pure def isSubmitEstimateReply(p: Plan, r: Reply): bool =
    answersOutstanding(p, r) and Set(SubmitEstimate, DeployEstimate).contains(awaitedExpect(p))
  pure def isPinnedReply(p: Plan, r: Reply): bool =
    answerUsable(p, r) and repinApplies(p, awaitedExpect(p))
  pure def isRoutineReply(p: Plan, r: Reply): bool =
    not(isConfirmCall(p, r) or isChainReply(p, r) or isSubmitEstimateReply(p, r) or isPinnedReply(p, r))

  /// settle (plan_submit.rs:42-66) on the outstanding submit-time estimate: a reviewed fee exists,
  /// every field is present and an integer, the unit is known and unchanged, the spend is within
  /// the headroom (a zero denominator never is) and the amounts fit Krusty's bounds.
  pure def estimateSettles(p: Plan, a: Answer): bool =
    estimateWithinHeadroom(p, a) and estimateBoundsFit(p, a)

  /// The quote of the entry settle reads (a missing entry or a non-estimate never quotes).
  pure def outstandingQuote(p: Plan, a: Answer): QuoteRes = {
    val isInvoke = awaitedExpect(p) == SubmitEstimate
    val idx = if (isInvoke and not(p.work.submitDeployed)) 1 else 0
    match a {
      | AEstimate(es) => if (idx >= es.length()) QMalformed else quote(es.nth(idx), p.policy.margin)
      | _ => QMalformed
    }
  }
  /// settle's fee part: a reviewed fee exists, every field is present and an integer, the unit
  /// is known and unchanged, and the spend is within the headroom (a zero denominator never is).
  pure def estimateWithinHeadroom(p: Plan, a: Answer): bool = {
    val reviewed =
      if (awaitedExpect(p) == SubmitEstimate) p.work.reviewedFee else p.work.reviewedDeployFee
    match reviewed {
      | NoFee => false
      | SomeFee(r) =>
        match outstandingQuote(p, a) { | QOk(q) => not(exceedsReviewed(r, q.quote, p.policy)) | _ => false }
    }
  }
  /// fees::quote's field check: the entry settle reads exists and every quantity in it is
  /// present and an integer (QMalformed otherwise).
  pure def estimateFieldsValid(p: Plan, a: Answer): bool = outstandingQuote(p, a) != QMalformed
  /// fees::krusty_bounds inside settle: a quoted amount above u64 is refused, never truncated.
  pure def estimateBoundsFit(p: Plan, a: Answer): bool =
    match outstandingQuote(p, a) { | QOk(q) => krustyFits(q.bounds) | _ => true }

  // Plan::advance, one action per branch the code takes; `advance` is their union.
  /// Any other reply whose body is well-formed and answers the outstanding id: its handler reads it.
  action advanceRoutineAnswered(host: Host, reply: Reply): bool = all {
    isRoutineReply(plan, reply),
    answerUsable(plan, reply),
    applyAdvance(host, reply),
  }
  /// A caller reply, a stale id (OutOfOrder), a transport failure or a malformed/oversized body
  /// (read as unreadable): no answer content is used.
  action advanceRoutineUnanswered(host: Host, reply: Reply): bool = all {
    isRoutineReply(plan, reply),
    not(answerUsable(plan, reply)),
    applyAdvance(host, reply),
  }
  /// confirm (plan_submit.rs:82-102): a clean recipient and a current session go on to submit.
  action advanceConfirmProceeds(host: Host, reply: Reply): bool = all {
    isConfirmCall(plan, reply),
    not(isPoisoned(plan.kind)),
    host.session,
    applyAdvance(host, reply),
  }
  /// confirm refuses a poisoned recipient, then a moved session, before anything else.
  action advanceConfirmRefused(host: Host, reply: Reply): bool = all {
    isConfirmCall(plan, reply),
    isPoisoned(plan.kind) or not(host.session),
    applyAdvance(host, reply),
  }
  /// judge::chain: the node names the plan's network.
  action advanceChainMatches(host: Host, reply: Reply): bool = all {
    isChainReply(plan, reply),
    answerUsable(plan, reply),
    chainMatches(replyAnswer(reply), plan.account.network) == 1,
    applyAdvance(host, reply),
  }
  /// judge::chain: another network (WrongChain) or an unreadable answer.
  action advanceChainRefused(host: Host, reply: Reply): bool = all {
    isChainReply(plan, reply),
    chainMatches(replyAnswer(reply), plan.account.network) != 1,
    applyAdvance(host, reply),
  }
  /// The submit-time re-estimate settles bounds only within the reviewed headroom.
  action advanceEstimateSettles(host: Host, reply: Reply): bool = all {
    isSubmitEstimateReply(plan, reply),
    answerUsable(plan, reply),
    estimateFieldsValid(plan, replyAnswer(reply)),
    estimateWithinHeadroom(plan, replyAnswer(reply)),
    estimateBoundsFit(plan, replyAnswer(reply)),
    applyAdvance(host, reply),
  }
  /// The re-estimate is refused (exceeded, unit change, malformed, missing, out of range).
  action advanceEstimateRefused(host: Host, reply: Reply): bool = all {
    isSubmitEstimateReply(plan, reply),
    not(estimateSettles(plan, replyAnswer(reply))),
    applyAdvance(host, reply),
  }
  /// A pinned account-state read whose block the node has.
  action advancePinnedBlockAvailable(host: Host, reply: Reply): bool = all {
    isPinnedReply(plan, reply),
    not(blockNotFound(replyAnswer(reply))),
    applyAdvance(host, reply),
  }
  /// A pinned read that missed its block (a sibling a block behind): re-pin, up to PIN_ATTEMPTS.
  action advancePinnedBlockMissed(host: Host, reply: Reply): bool = all {
    isPinnedReply(plan, reply),
    blockNotFound(replyAnswer(reply)),
    applyAdvance(host, reply),
  }

  /// Plan::advance(host, reply)
  action advance(host: Host, reply: Reply): bool = all {
    hostLocksEndedSession(host),
    host.honoursRenew,
    transferAmountFits(plan.kind),
    any {
      advanceRoutineAnswered(host, reply),
      advanceRoutineUnanswered(host, reply),
      advanceConfirmProceeds(host, reply),
      advanceConfirmRefused(host, reply),
      advanceChainMatches(host, reply),
      advanceChainRefused(host, reply),
      advanceEstimateSettles(host, reply),
      advanceEstimateRefused(host, reply),
      advancePinnedBlockAvailable(host, reply),
      advancePinnedBlockMissed(host, reply),
    },
  }

  /// Plan::close()
  action close: bool = all {
    plan' = { ...plan, stage: Closed },
    node' = node,
    pending' = pending,
    handedOut' = handedOut,
    planWrites' = planWrites,
    refusedAttempt' = refusedAttempt,
    sessionAtConfirm' = sessionAtConfirm,
    droppedDeployment' = droppedDeployment,
    closedByCall' = true,
    lastCall' = CClose,
  }

  // ---------------------------------------------------------------- domains

  pure val ACCOUNTS: Set[Account] =
    Set(OZ, Argent, Braavos).map(f =>
      Set(SEPOLIA, MAINNET).map(n =>
        Set(NoDeployment, HasDeployment(0), HasDeployment(1)).map(d =>
          { family: f, network: n, publicKey: PK, address: ACCOUNT_ADDRESS, deployment: d }))).flatten().flatten()
  pure val KINDS: Set[Kind] = Set(
    KTransfer({ poisoned: false, amount: 1500 }),
    KTransfer({ poisoned: true, amount: 1500 }),
    KTransfer({ poisoned: false, amount: U256_LIMIT }),
    KTransfer({ poisoned: false, amount: 0 }),
    KDeployment,
    KRefresh(REFRESH_HASH),
  )
  pure val POLICIES: Set[Policy] =
    Set(2, 0).map(d => Set(0, 2, 12).map(n => { margin: 50, num: 3, den: d, delays: n })).flatten()
  pure val HOSTS: Set[Host] =
    Set(true, false).map(s =>
      Set(SignOk, SignOtherKey, SignLocked, SignFailed).map(g =>
        Set(true, false).map(r => { session: s, sign: g, honoursRenew: r }))).flatten().flatten()

  pure val E_BASE: Entry = { unit: EFri, wellFormed: true, amount: 10, price: 2 }
  pure val ENTRIES: Set[Entry] = Set(
    E_BASE,
    { unit: EFri, wellFormed: true, amount: 12, price: 2 },        // within 3/2 headroom
    { unit: EFri, wellFormed: true, amount: 10, price: 4 },        // doubled price: exceeds
    { unit: EWei, wellFormed: true, amount: 10, price: 2 },        // unit change
    { unit: EOther, wellFormed: true, amount: 10, price: 2 },      // unrecognised unit
    { unit: EFri, wellFormed: false, amount: 10, price: 2 },       // missing / non-integer field
    { unit: EAbsent, wellFormed: true, amount: 200, price: 2 },      // margins above u64
    { unit: EFri, wellFormed: true, amount: 10, price: U128_MAX }, // margin overflows u128
  )
  pure val ESTIMATES: Set[Answer] =
    ENTRIES.map(e => AEstimate([e]))
      .union(ENTRIES.map(e => AEstimate([E_BASE, e])))
      .union(Set(AEstimate([])))
  pure val GENERIC: Set[Answer] = Set(ANone, AError(1))
  pure val NONCES: Set[Answer] = Set(AFelt(0), AFelt(3))
  pure val STATUSES: Set[Answer] = Set(
    AStatus({ status: Accepted, accepted: true }),
    AStatus({ status: Pending, accepted: false }),
    AStatus({ status: Rejected, accepted: false }),
    AStatus({ status: Reverted, accepted: true }),
    AError(29),
  )
  pure val CALLS: Set[Answer] =
    Set(AFelts([PK]), AFelts([0]), AFelts([OTHER_PK]), AFelts([]), AError(24))
  pure val SIGNER_LISTS: Set[Answer] = Set(
    AFelts([1, PK, 0, 0]), AFelts([1, PK, 0, 1, 5]), AFelts([1, OTHER_PK, 0, 0]),
    AFelts([2, PK, 0, 0]), AFelts([1, PK]), AError(24),
  )

  pure def writesFor(h: HashOpt): Set[Answer] =
    match h {
      | SomeHash(x) => Set(AWrite(HIs(x)), AWrite(HIs(BOGUS_HASH)), AWrite(HZero))
      | NoHash => Set(AWrite(HIs(BOGUS_HASH)), AWrite(HZero))
    }

  /// The reply bodies a node may send for what the plan awaits, valid and invalid.
  pure def answersFor(p: Plan, e: Expect): Set[Answer] = specificAnswers(p, e).union(GENERIC)

  /// What a well-behaved node on chain `n` answers: one chain id, one deployment state, a
  /// result of the expected shape (whose content may still be refused) for every other read.
  pure def nodeAnswers(p: Plan, n: Node, e: Expect): Set[Answer] =
    match e {
      | ReviewChain => Set(AFelt(n.chain))
      | SubmitChain => Set(AFelt(n.chain))
      | EBlock(_) => Set(ABlock(BLOCK_HASH))
      | ReviewEstimate => Set(n.fee)
      | ReviewDeployEstimate => Set(n.fee)
      | SubmitEstimate => Set(n.fee)
      | DeployEstimate => Set(n.fee)
      | EClass(_) =>
        if (not(n.deployed)) { if (isOZ(p)) Set(AError(20)) else Set(AFelt(0)) }
        else if (isOZ(p)) Set(AFelt(OZ_CLASS))
        else if (isArgent(p)) Set(AFelt(ARGENT_OK), AFelt(ARGENT_OLD), AFelt(UNKNOWN_CLASS))
        else Set(AFelt(BRAAVOS_CLASS), AFelt(UNKNOWN_CLASS))
      | _ => specificAnswers(p, e)
    }

  /// The well-formed reply bodies for what the plan awaits.
  pure def specificAnswers(p: Plan, e: Expect): Set[Answer] = {
    val specific: Set[Answer] = match e {
      | EBlock(_) => Set(ABlock(BLOCK_HASH), ABlock(0))
      | EClass(_) =>
        if (isOZ(p)) Set(AFelt(OZ_CLASS), AError(20))
        else Set(AFelt(0), AFelt(ARGENT_OK), AFelt(ARGENT_OLD), AFelt(BRAAVOS_CLASS), AFelt(UNKNOWN_CLASS), AError(20), AError(24))
      | EOwner(_) => CALLS
      | EOwnerType(_) => CALLS
      | EGuardian(_) => CALLS
      | ESigners(_) => SIGNER_LISTS
      | EDeferred(_) => Set(AFelts([100]), AFelts([]), AFelts([0]), AError(24))
      | EBlockTime(_) => Set(ATime(99), ATime(100), ATime(101), AError(24))
      | ReviewChain => Set(AFelt(SEPOLIA), AFelt(MAINNET))
      | SubmitChain => Set(AFelt(SEPOLIA), AFelt(MAINNET))
      | ReviewNonce => NONCES
      | ReviewDeployNonce => NONCES
      | SubmitNonce => NONCES
      | DeployNonce => NONCES
      | InvokeNonce => NONCES
      | ReviewEstimate => ESTIMATES
      | ReviewDeployEstimate => ESTIMATES
      | SubmitEstimate => ESTIMATES
      | DeployEstimate => ESTIMATES
      | DeployWrite => writesFor(p.work.deployHash)
      | InvokeWrite => writesFor(p.work.invokeHash)
      | DeployStatus => STATUSES
      | ConfirmStatus => STATUSES
      | RefreshStatus => STATUSES
      | DeployReceipt => Set(AReceipt)
    }
    specific
  }

  pure val NODES: Set[Node] =
    Set(SEPOLIA, MAINNET).map(c => Set(true, false).map(d => ESTIMATES.map(f =>
      { chain: c, deployed: d, fee: f }))).flatten().flatten()

  pure def outstandingId(p: Plan): int = match p.stage { | Awaiting(a) => a.id | _ => p.work.nextId }
  pure def awaitedExpect(p: Plan): Expect =
    match p.stage { | Awaiting(a) => a.expect | _ => ReviewChain }
  pure def isAwaiting(s: Stage): bool = match s { | Awaiting(_) => true | _ => false }

  pure def controlReplies(id: int): Set[Reply] = Set(
    Start, Confirm, Proceed, WaitElapsed, CheckAgain, RetryEstimate,
    TransportFailure(id), TransportFailure(id + 1),
  )

  // ---------------------------------------------------------------- init / step

  action initPlan(account: Account, kind: Kind, policy: Policy, chainState: Node): bool = all {
    transferAmountFits(kind),
    transferAmountPositive(kind),
    plan' = newPlan(account, kind, policy),
    node' = chainState,
    pending' = NO_HASHES,
    handedOut' = NO_HASHES,
    planWrites' = NO_HASHES,
    refusedAttempt' = false,
    sessionAtConfirm' = false,
    droppedDeployment' = false,
    closedByCall' = false,
    lastCall' = CNew,
  }

  action init: bool = {
    nondet account = ACCOUNTS.oneOf()
    nondet kind = KINDS.oneOf()
    nondet policy = POLICIES.oneOf()
    nondet chainState = NODES.oneOf()
    initPlan(account, kind, policy, chainState)
  }

  /// The replies a stage is waiting for from the caller (StepReply other than Rpc).
  pure def expectedReplies(s: Stage): Set[Reply] =
    match s {
      | Fresh => Set(Start)
      | Reviewed => Set(Confirm) // RetryEstimate is offered by the out-of-step arm
      | Waiting(_) => Set(WaitElapsed)
      | Broadcasted => Set(Proceed)
      | Finished(_) => Set(CheckAgain)
      | Awaiting(a) => Set(TransportFailure(a.id))
      | Closed => Set(Start)
    }

  pure def hostsSigning(g: SignAns): Set[Host] = HOSTS.filter(h => h.sign == g)
  /// The reply a caller and a well-behaved node give the plan in its current stage.
  pure def followReplies(p: Plan, n: Node): Set[Reply] =
    if (isAwaiting(p.stage))
      nodeAnswers(p, n, awaitedExpect(p)).map(a => Rpc({ id: outstandingId(p), answer: a }))
    else expectedReplies(p.stage)

  action step: bool = any {
    // The caller builds a new plan once the last one closed.
    all {
      plan.stage == Closed,
      nondet account = ACCOUNTS.oneOf()
      nondet kind = KINDS.oneOf()
      nondet policy = POLICIES.oneOf()
      nondet chainState = NODES.oneOf()
      new(account, kind, policy, chainState),
    },
    // The caller and a well-behaved node in step with the plan: the reply the stage waits for,
    // answered from the node's chain. One arm per way the host answers a signing request;
    // together they cover HOSTS.
    all {
      plan.stage != Closed,
      nondet host = hostsSigning(SignOk).oneOf()
      all {
        hostLocksEndedSession(host),
        host.honoursRenew,
        nondet reply = followReplies(plan, node).oneOf()
        advance(host, reply),
      },
    },
    all {
      plan.stage != Closed,
      nondet host = hostsSigning(SignOtherKey).oneOf()
      all {
        hostLocksEndedSession(host),
        host.honoursRenew,
        nondet reply = followReplies(plan, node).oneOf()
        advance(host, reply),
      },
    },
    all {
      plan.stage != Closed,
      nondet host = hostsSigning(SignLocked).filter(h => h.session).oneOf()
      all {
        hostLocksEndedSession(host),
        host.honoursRenew,
        nondet reply = followReplies(plan, node).oneOf()
        advance(host, reply),
      },
    },
    all {
      plan.stage != Closed,
      nondet host = hostsSigning(SignLocked).filter(h => not(h.session)).oneOf()
      all {
        hostLocksEndedSession(host),
        host.honoursRenew,
        nondet reply = followReplies(plan, node).oneOf()
        advance(host, reply),
      },
    },
    all {
      plan.stage != Closed,
      nondet host = hostsSigning(SignFailed).oneOf()
      all {
        hostLocksEndedSession(host),
        host.honoursRenew,
        nondet reply = followReplies(plan, node).oneOf()
        advance(host, reply),
      },
    },
    // The caller or the node out of step: a new plan, a close, any reply kind, a stale id, a
    // transport failure, any body (malformed, an error, another chain) on the outstanding id.
    any {
      {
        nondet account = ACCOUNTS.oneOf()
        nondet kind = KINDS.oneOf()
        nondet policy = POLICIES.oneOf()
        nondet chainState = NODES.oneOf()
        new(account, kind, policy, chainState)
      },
      close,
      {
        nondet host = HOSTS.oneOf()
        all {
          hostLocksEndedSession(host),
          host.honoursRenew,
          nondet reply = controlReplies(outstandingId(plan)).oneOf()
          advance(host, reply),
        }
      },
      {
        nondet host = HOSTS.oneOf()
        all {
          hostLocksEndedSession(host),
          host.honoursRenew,
          nondet id = Set(outstandingId(plan), outstandingId(plan) + 1).oneOf()
          nondet answer = answersFor(plan, awaitedExpect(plan)).oneOf()
          advance(host, Rpc({ id: id, answer: answer })),
        }
      },
    },
  }
}
