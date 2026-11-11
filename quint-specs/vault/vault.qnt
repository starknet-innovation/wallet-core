module vault {
  // Vault storage: the container (container.rs), the password envelope (envelope.rs) and the
  // host Storage / Clock abstractions (host.rs).
  //
  // Abstraction:
  //  * AES-GCM is structural: a ciphertext IS {key, iv, aad, plaintext}; it opens only under the
  //    same key, iv and additional data. PBKDF2 is the record {pw, salt, iters}.
  //  * A base64 field that must decode to an exact length is `Dec(value)` or `Undec`.
  //  * The OS CSPRNG is a counter (`entropy`): every draw is fresh; `entropyOk` lets it fail.
  //  * Values the caller holds between calls (keys, DEKs, files, envelopes, slots, clock
  //    readings) are sets in the state: a returned value is available to later calls.
  //  * Host storage is a key -> stored-bytes map whose writes are atomic (host.rs:11-13).

  // ---------------------------------------------------------------- types

  /// A lineage or slot id: 32 lower-hex chars, or anything else.
  type Lin = LHex(int) | LBad
  /// A base64 field checked by decode::<N>: decodes to N bytes, or does not.
  type B64 = Dec(int) | Undec
  /// A biometric domain field: empty, a 64-hex SHA-256, or malformed.
  type Dom = DEmpty | DHash(int) | DBad

  /// PBKDF2-SHA-256(password, salt, iterations).
  type Kdf = { pw: str, salt: int, iters: int }
  type AeadKey = KPw(Kdf) | KDek(int) | KWrap(int)
  type EnvAad = { scope: str, lineage: Lin, commit: B64 }
  type BioAad = { lineage: Lin, id: Lin, domain: Dom }
  type Aad = AadEnv(EnvAad) | AadBio(BioAad)

  type PayloadDoc = { version: int, body: int }
  /// Plaintexts: a DekRecord JSON, something that is not one, a vault payload, raw DEK bytes.
  type Plain = PDekRecord(int) | PNotDekRecord | PPayload(PayloadDoc) | PDek(int)

  type Ct = { key: AeadKey, iv: int, aad: Aad, pt: Plain }
  /// The `data` field: a real ciphertext, bytes that decode but do not authenticate, or bad base64.
  /// DFlipped: an authentic ciphertext with bytes edited (same length, fails authentication).
  type Data = DCt(Ct) | DFlipped(Ct) | DForged(int) | DUndec
  type OptPlain = NoPlain | SomePlain(Plain)

  type Envelope = {
    cipher: str, commit: B64, data: Data, iterations: int, iv: B64,
    kdf: str, lineage: Lin, salt: B64, scope: str, version: int,
  }
  type SealedPayload = { scope: str, iv: B64, commit: B64, data: Data }
  type BioSlot = { id: Lin, iv: B64, data: Data, domain: Dom }
  type OptBio = NoBio | SomeBio(BioSlot)
  type VaultFile = {
    format: str, version: int, lineage: Lin, passwordSlot: Envelope,
    biometricSlot: OptBio, payload: SealedPayload,
  }
  type SealingKey = { key: Kdf, salt: int, lineage: Lin }
  type Randomness = { dekIv: int, dekCommit: int, payloadIv: int, payloadCommit: int }

  /// What vault.json holds once read as text.
  type Text = TNotUtf8 | TNotJson | TNoKnownKeys | TContainer(VaultFile) | TEnvelope(Envelope)
  type Stored = SAbsent | SText(Text)

  type Err = EInvalid | EWrongPassword | EBioUnavailable | EBioInvalidated
  type ParseRes = PInvalid | PContainer(VaultFile) | PEnvelope(Envelope)
  type ReadRes = RdNone | RdParsed(ParseRes) | RdHostErr
  type Opened = { pt: Plain, key: SealingKey }
  type OpenRes = OpenOk(Opened) | OpenErr(Err)
  type DekAndKey = { dek: int, key: SealingKey }
  type OpenDekRes = OdOk(DekAndKey) | OdErr(Err)
  type DekRes = DekOk(int) | DekErr(Err)
  type PlainRes = PlainOk(Plain) | PlainErr(Err)

  type Reading = { boot: int, ms: int }
  type Elapsed = ENone | ESome(int)

  type BioSealed = { file: VaultFile, slot: BioSlot }

  type St = {
    storage: str -> Stored,
    hostFailing: bool,
    entropyOk: bool,
    entropy: int,
    keyring: Set[SealingKey],
    deks: Set[int],
    randoms: Set[Randomness],
    files: Set[VaultFile],
    envelopes: Set[Envelope],
    bioSlots: Set[BioSealed],
    hostDomainState: int,
    clock: Reading,
    readings: Set[Reading],
  }

  type Ghost = {
    written: Set[Stored],
    tampered: Set[Stored],
    persisted: Set[VaultFile],
    internalCts: Set[Ct],
    foreignDek: Set[VaultFile],
    versionMismatch: Set[VaultFile],
    tamperedEnvs: Set[Envelope],
    produced: Set[VaultFile],
    ctLog: Set[Ct],
    sealedBy: Set[{ sk: SealingKey, env: Envelope }],
    createdKeys: Set[SealingKey],
    newLog: Set[{ sk: SealingKey, dek: int, version: int, payload: Plain, r: Randomness, file: VaultFile }],
    fromPartsKeys: Set[SealingKey],
    derivedLog: Set[{ base: VaultFile, dek: int, version: int, payload: Plain, out: VaultFile }],
    openLog: Set[{ env: Envelope, pw: str, scope: str, res: OpenRes }],
    openDekLog: Set[{ file: VaultFile, pw: str, res: OpenDekRes }],
    dekWithLog: Set[{ file: VaultFile, sk: SealingKey, res: DekRes }],
    payloadLog: Set[{ file: VaultFile, dek: int, res: PlainRes }],
    bioLog: Set[{ file: VaultFile, wrap: int, res: DekRes }],
    sealedLog: Set[{ sk: SealingKey, env: Envelope, scope: str, res: PlainRes }],
    readLog: Set[{ key: str, stored: Stored, cv: int, res: ReadRes }],
    elapsedLog: Set[{ now: Reading, earlier: Reading, res: Elapsed }],
    bootIdsSeen: Set[int],
    reboots: int,
    crashKeptOld: bool,
  }

  // ---------------------------------------------------------------- constants

  pure val FORMAT = "bramble.mobile.vault"
  pure val FIRST_CONTAINER_VERSION = 2
  pure val V1_SCOPE = "bramble.mobile.vault.v1"
  pure val DEK_SCOPE = "bramble.mobile.dek.v1"
  pure val PAYLOAD_SCOPE = "bramble.mobile.payload.v2"
  pure val KDF_ITERATIONS = 600000
  pure val LEGACY_KDF_ITERATIONS = 310000
  pure val CIPHER = "AES-256-GCM"
  pure val KDF_NAME = "PBKDF2-SHA-256"
  pure val ENVELOPE_VERSION = 3

  // Domains (test values plus values that break each check).
  pure val PASSWORDS = Set("fixture passphrase", "wrong passphrase", "")
  pure val SCOPES = Set(DEK_SCOPE, V1_SCOPE, "other")
  pure val VERSIONS = 1.to(6)
  pure val CURRENT_VERSIONS = Set(4, 5)
  pure val BODIES = 0.to(1)
  pure val WRAPS = Set(9, 8)
  pure val DOMAIN_STATES = 0.to(1)
  pure val STORAGE_KEYS = Set("vault.json", "legacy.json", "spare.json")
  pure val FIXED_NONCES = 0.to(3)
  pure val SALTS = 0.to(2)
  pure val LINEAGES = Set(LHex(1), LHex(2), LBad)
  /// Edits to a held envelope that keep its shape (shape-breaking edits go through storage).
  pure val ENVELOPE_TAMPERS = Set("slotCommit", "lineage", "slotData", "legacyIterations")
  pure val TAMPERS = Set("notUtf8", "notJson", "noKeys", "slotCommit", "lineage", "iterations", "legacyIterations",
    "slotData", "slotSalt", "slotDataB64", "payloadData", "payloadIv", "bioDomain", "bioId", "version", "bioData")

  // ---------------------------------------------------------------- pure helpers

  pure def isLowerHex(l: Lin): bool = match l { | LHex(_) => true | LBad => false }
  pure def isDec(b: B64): bool = match b { | Dec(_) => true | Undec => false }
  pure def decVal(b: B64): int = match b { | Dec(n) => n | Undec => -1 }
  pure def dataDecodes(d: Data): bool = match d { | DUndec => false | _ => true }
  pure def domainOk(d: Dom): bool = match d { | DBad => false | _ => true }

  pure def plainLen(p: Plain): int = match p {
    | PDek(_) => 32
    | PDekRecord(_) => 60
    | PNotDekRecord => 7
    | PPayload(_) => 40
  }
  /// Decoded length of a data field (ciphertext plus the 16-byte tag).
  pure def dataLen(d: Data): int = match d {
    | DCt(c) => plainLen(c.pt) + 16
    | DFlipped(c) => plainLen(c.pt) + 16
    | DForged(n) => n
    | DUndec => -1
  }

  /// aead_open (envelope.rs:98-109): None when authentication fails.
  pure def aeadOpen(k: AeadKey, iv: int, aad: Aad, d: Data): OptPlain = match d {
    | DCt(c) => if (c.key == k and c.iv == iv and c.aad == aad) SomePlain(c.pt) else NoPlain
    | DFlipped(_) => NoPlain
    | DForged(_) => NoPlain
    | DUndec => NoPlain
  }

  /// domain_hash (container.rs:141-146): SHA-256 hex of the host's domain state.
  pure def domain_hash(state: int): Dom = DHash(state)

  /// SealingKey::lineage (envelope.rs:160-162).
  pure def keyLineage(sk: SealingKey): Lin = sk.lineage

  /// SealingKey::from_parts (envelope.rs:121-127).
  pure def fromParts(password: str, salt: int, lineage: Lin): SealingKey =
    { key: { pw: password, salt: salt, iters: KDF_ITERATIONS }, salt: salt, lineage: lineage }

  /// SealingKey::create (envelope.rs:115-117): fresh salt and lineage from the CSPRNG.
  pure def createKey(password: str, entropy: int): SealingKey =
    fromParts(password, entropy, LHex(entropy + 1))

  /// SealingKey::seal_with (envelope.rs:135-157).
  pure def sealWith(sk: SealingKey, plaintext: Plain, scope: str, iv: int, commit: int): Envelope = {
    val aad = AadEnv({ scope: scope, lineage: sk.lineage, commit: Dec(commit) })
    {
      cipher: CIPHER,
      commit: Dec(commit),
      data: DCt({ key: KPw(sk.key), iv: iv, aad: aad, pt: plaintext }),
      iterations: KDF_ITERATIONS,
      iv: Dec(iv),
      kdf: KDF_NAME,
      lineage: sk.lineage,
      salt: Dec(sk.salt),
      scope: scope,
      version: ENVELOPE_VERSION,
    }
  }

  pure def envCt(e: Envelope): Set[Ct] = match e.data { | DCt(c) => Set(c) | _ => Set() }

  /// valid_shape (envelope.rs:184-193).
  pure def validShape(e: Envelope, scope: str): bool = and {
    e.version == ENVELOPE_VERSION,
    e.cipher == CIPHER,
    e.kdf == KDF_NAME,
    e.iterations == KDF_ITERATIONS or e.iterations == LEGACY_KDF_ITERATIONS,
    e.scope == scope,
    isLowerHex(e.lineage),
    isDec(e.commit),
  }

  /// open (envelope.rs:197-226): a failed AEAD is WrongPassword; shape failures are invalid.
  pure def openEnvelope(e: Envelope, password: str, scope: str): OpenRes =
    if (not(validShape(e, scope))) OpenErr(EInvalid)
    else if (not(isDec(e.salt) and isDec(e.iv))) OpenErr(EInvalid)
    else if (not(dataDecodes(e.data))) OpenErr(EInvalid)
    else {
      val salt = decVal(e.salt)
      val k = { pw: password, salt: salt, iters: e.iterations }
      val aad = AadEnv({ scope: scope, lineage: e.lineage, commit: e.commit })
      match aeadOpen(KPw(k), decVal(e.iv), aad, e.data) {
        | NoPlain => OpenErr(EWrongPassword)
        | SomePlain(pt) => OpenOk({
            pt: pt,
            key: { key: { pw: password, salt: salt, iters: KDF_ITERATIONS }, salt: salt, lineage: e.lineage },
          })
      }
    }

  /// SealingKey::open_sealed (envelope.rs:165-182): no KDF; any failure is invalid.
  pure def openSealed(sk: SealingKey, e: Envelope, scope: str): PlainRes =
    if (not(validShape(e, scope)) or e.iterations != KDF_ITERATIONS
        or e.lineage != sk.lineage or e.salt != Dec(sk.salt)) PlainErr(EInvalid)
    else if (not(isDec(e.iv)) or not(dataDecodes(e.data))) PlainErr(EInvalid)
    else {
      val aad = AadEnv({ scope: scope, lineage: e.lineage, commit: e.commit })
      match aeadOpen(KPw(sk.key), decVal(e.iv), aad, e.data) {
        | NoPlain => PlainErr(EInvalid)
        | SomePlain(pt) => PlainOk(pt)
      }
    }

  /// decode_dek (container.rs:165-174).
  pure def decode_dek(pt: Plain): DekRes = match pt {
    | PDekRecord(d) => DekOk(d)
    | _ => DekErr(EInvalid)
  }

  /// seal_payload (container.rs:148-163).
  pure def sealPayload(dek: int, lineage: Lin, pt: Plain, iv: int, commit: int): SealedPayload = {
    scope: PAYLOAD_SCOPE,
    iv: Dec(iv),
    commit: Dec(commit),
    data: DCt({ key: KDek(dek), iv: iv,
                aad: AadEnv({ scope: PAYLOAD_SCOPE, lineage: lineage, commit: Dec(commit) }), pt: pt }),
  }

  /// seal_new (container.rs:178-212).
  pure def sealNew(sk: SealingKey, dek: int, version: int, payload: Plain, r: Randomness): VaultFile = {
    format: FORMAT,
    version: version,
    lineage: keyLineage(sk),
    passwordSlot: sealWith(sk, PDekRecord(dek), DEK_SCOPE, r.dekIv, r.dekCommit),
    biometricSlot: NoBio,
    payload: sealPayload(dek, keyLineage(sk), payload, r.payloadIv, r.payloadCommit),
  }

  /// VaultFile::with_payload (container.rs:285-296): no check of the DEK nor of the version.
  pure def withPayload(f: VaultFile, dek: int, version: int, payload: Plain, iv: int, commit: int): VaultFile =
    f.with("version", version).with("payload", sealPayload(dek, f.lineage, payload, iv, commit))

  /// VaultFile::seal_biometric (container.rs:299-314): returns the slot, does not install it.
  pure def sealBiometric(f: VaultFile, dek: int, wrap: int, domain: Dom, id: int, iv: int): BioSlot = {
    id: LHex(id),
    iv: Dec(iv),
    data: DCt({ key: KWrap(wrap), iv: iv, aad: AadBio({ lineage: f.lineage, id: LHex(id), domain: domain }), pt: PDek(dek) }),
    domain: domain,
  }

  pure def bioCts(b: OptBio): Set[Ct] = match b {
    | NoBio => Set()
    | SomeBio(s) => match s.data { | DCt(c) => Set(c) | _ => Set() }
  }

  /// VaultFile::validate (container.rs:235-256).
  pure def bioSlotOk(b: OptBio): bool = match b {
    | NoBio => true
    | SomeBio(s) => isLowerHex(s.id) and isDec(s.iv) and dataLen(s.data) == 48 and domainOk(s.domain)
  }
  pure def validate(f: VaultFile, currentVersion: int): bool = and {
    f.format == FORMAT,
    FIRST_CONTAINER_VERSION <= f.version and f.version <= currentVersion,
    isLowerHex(f.lineage),
    f.passwordSlot.lineage == f.lineage,
    f.passwordSlot.scope == DEK_SCOPE,
    f.payload.scope == PAYLOAD_SCOPE,
    isDec(f.payload.iv),
    isDec(f.payload.commit),
    bioSlotOk(f.biometricSlot),
  }

  /// parse (container.rs:93-107): a format key is a container, cipher+kdf an envelope, else invalid.
  pure def parse(t: Text, currentVersion: int): ParseRes = match t {
    | TContainer(f) => if (validate(f, currentVersion)) PContainer(f) else PInvalid
    | TEnvelope(e) => PEnvelope(e)
    | _ => PInvalid
  }

  /// read (container.rs:215-225).
  pure def readStored(s: Stored, currentVersion: int, hostFails: bool): ReadRes =
    if (hostFails) RdHostErr
    else match s {
      | SAbsent => RdNone
      | SText(t) => RdParsed(parse(t, currentVersion))
    }

  /// VaultFile::open_dek (container.rs:259-262).
  pure def dekResToOd(r: DekRes, key: SealingKey): OpenDekRes = match r {
    | DekOk(d) => OdOk({ dek: d, key: key })
    | DekErr(e) => OdErr(e)
  }
  pure def openDek(f: VaultFile, password: str): OpenDekRes =
    match openEnvelope(f.passwordSlot, password, DEK_SCOPE) {
      | OpenErr(e) => OdErr(e)
      | OpenOk(o) => dekResToOd(decode_dek(o.pt), o.key)
    }

  /// VaultFile::open_dek_with (container.rs:265-267).
  pure def openDekWith(f: VaultFile, sk: SealingKey): DekRes =
    match openSealed(sk, f.passwordSlot, DEK_SCOPE) {
      | PlainErr(e) => DekErr(e)
      | PlainOk(pt) => decode_dek(pt)
    }

  /// VaultFile::open_payload (container.rs:271-281).
  pure def openPayload(f: VaultFile, dek: int): PlainRes =
    if (not(isDec(f.payload.iv)) or not(dataDecodes(f.payload.data))) PlainErr(EInvalid)
    else {
      val aad = AadEnv({ scope: PAYLOAD_SCOPE, lineage: f.lineage, commit: f.payload.commit })
      match aeadOpen(KDek(dek), decVal(f.payload.iv), aad, f.payload.data) {
        | NoPlain => PlainErr(EInvalid)
        | SomePlain(pt) => PlainOk(pt)
      }
    }

  /// VaultFile::open_biometric (container.rs:318-335).
  pure def plainAsDek(pt: Plain): DekRes = match pt {
    | PDek(d) => DekOk(d)
    | _ => DekErr(EBioInvalidated)
  }
  pure def openBioSlot(lineage: Lin, s: BioSlot, wrap: int): DekRes =
    if (not(isDec(s.iv)) or dataLen(s.data) != 48) DekErr(EBioInvalidated)
    else match aeadOpen(KWrap(wrap), decVal(s.iv), AadBio({ lineage: lineage, id: s.id, domain: s.domain }), s.data) {
      | NoPlain => DekErr(EBioInvalidated)
      | SomePlain(pt) => plainAsDek(pt)
    }
  pure def openBiometric(f: VaultFile, wrap: int): DekRes = match f.biometricSlot {
    | NoBio => DekErr(EBioUnavailable)
    | SomeBio(s) => openBioSlot(f.lineage, s, wrap)
  }

  /// ClockReading::elapsed_since (host.rs:60-65).
  pure def elapsedSince(now: Reading, earlierBoot: int, earlierMs: int): Elapsed =
    if (now.boot != earlierBoot) ENone
    else if (now.ms < earlierMs) ENone
    else ESome(now.ms - earlierMs)

  /// The wallet's passphrase rule, checked by the caller before SealingKey::create.
  pure def passwordMeetsRule(password: str): bool = password != ""

  /// The caller's version check on an opened payload (container.rs:270).
  pure def payloadVersion(pt: Plain): int = match pt { | PPayload(p) => p.version | _ => -1 }
  pure def callerAcceptsPayload(f: VaultFile, r: PlainRes): bool = match r {
    | PlainErr(_) => true
    | PlainOk(pt) => payloadVersion(pt) == f.version
  }

  // Environment: edits to the untrusted vault.json.
  pure def flip(d: Data): Data = match d { | DCt(c) => DFlipped(c) | _ => DForged(60) }
  pure def tamperEnv(e: Envelope, kind: str): Envelope =
    if (kind == "slotCommit") e.with("commit", Dec(99))
    else if (kind == "lineage") e.with("lineage", LHex(98))
    else if (kind == "iterations") e.with("iterations", 1)
    else if (kind == "legacyIterations") e.with("iterations", LEGACY_KDF_ITERATIONS)
    else if (kind == "slotData") e.with("data", flip(e.data))
    else if (kind == "slotSalt") e.with("salt", Undec)
    else if (kind == "slotDataB64") e.with("data", DUndec)
    else e
  pure def tamperBio(b: OptBio, kind: str): OptBio = match b {
    | NoBio => NoBio
    | SomeBio(s) =>
        if (kind == "bioDomain") SomeBio(s.with("domain", DEmpty))
        else if (kind == "bioId") SomeBio(s.with("id", LHex(97)))
        else if (kind == "bioData") SomeBio(s.with("data", DForged(10)))
        else b
  }
  pure def tamperFile(f: VaultFile, kind: str): VaultFile =
    if (kind == "lineage") f.with("lineage", LHex(98))
    else if (kind == "version") f.with("version", 9)
    else if (kind == "payloadData") f.with("payload", f.payload.with("data", flip(f.payload.data)))
    else if (kind == "payloadIv") f.with("payload", f.payload.with("iv", Undec))
    else if (Set("slotCommit", "iterations", "legacyIterations", "slotData", "slotSalt", "slotDataB64").contains(kind))
      f.with("passwordSlot", tamperEnv(f.passwordSlot, kind))
    else f.with("biometricSlot", tamperBio(f.biometricSlot, kind))
  pure def tamperText(t: Text, kind: str): Text =
    if (kind == "notUtf8") TNotUtf8
    else if (kind == "notJson") TNotJson
    else if (kind == "noKeys") TNoKnownKeys
    else match t {
      | TContainer(f) => TContainer(tamperFile(f, kind))
      | TEnvelope(e) => TEnvelope(tamperEnv(e, kind))
      | _ => t
    }
  pure def tamperStored(s: Stored, kind: str): Stored = match s {
    | SAbsent => SText(tamperText(TNoKnownKeys, kind))
    | SText(t) => SText(tamperText(t, kind))
  }

  // Readings of the model's own data, for properties.
  /// The ciphertext an envelope was sealed as, even after its bytes were edited.
  pure def origCt(e: Envelope): Set[Ct] = match e.data { | DCt(c) => Set(c) | DFlipped(c) => Set(c) | _ => Set() }
  pure def slotPassword(e: Envelope): Set[str] =
    origCt(e).fold(Set(), (acc, c) => match c.key { | KPw(k) => acc.union(Set(k.pw)) | _ => acc })
  pure def slotDek(f: VaultFile): Set[int] =
    envCt(f.passwordSlot).fold(Set(), (acc, c) => match c.pt { | PDekRecord(d) => acc.union(Set(d)) | _ => acc })
  pure def payloadCts(f: VaultFile): Set[Ct] = match f.payload.data { | DCt(c) => Set(c) | _ => Set() }
  pure def aadLineage(a: Aad): Lin = match a { | AadEnv(x) => x.lineage | AadBio(x) => x.lineage }
  pure def aadScope(a: Aad): str = match a { | AadEnv(x) => x.scope | AadBio(_) => "bramble.mobile.biometric.v1" }
  pure def readContainers(r: ReadRes): Set[VaultFile] =
    match r { | RdParsed(p) => match p { | PContainer(f) => Set(f) | _ => Set() } | _ => Set() }
  pure def storedFiles(v: Stored): Set[VaultFile] =
    match v { | SText(t) => match t { | TContainer(f) => Set(f) | _ => Set() } | SAbsent => Set() }
  pure def openedKeys(r: OpenRes): Set[SealingKey] = match r { | OpenOk(o) => Set(o.key) | OpenErr(_) => Set() }
  pure def odKeys(r: OpenDekRes): Set[SealingKey] = match r { | OdOk(x) => Set(x.key) | OdErr(_) => Set() }
  pure def isOk(r: PlainRes): bool = match r { | PlainOk(_) => true | _ => false }
  pure def dekOk(r: DekRes): bool = match r { | DekOk(_) => true | _ => false }

  // ---------------------------------------------------------------- fixtures

  pure val FIXTURE_PW = "fixture passphrase"
  pure val FIXTURE_KEY: SealingKey = fromParts(FIXTURE_PW, 1, LHex(1))
  pure val FIXTURE_DEK = 3
  pure val FIXTURE_RANDOMNESS: Randomness = { dekIv: 1, dekCommit: 1, payloadIv: 1, payloadCommit: 1 }
  pure val FIXTURE_FILE: VaultFile =
    sealNew(FIXTURE_KEY, FIXTURE_DEK, 2, PPayload({ version: 2, body: 0 }), FIXTURE_RANDOMNESS)
  /// A pre-container (vault v1) envelope at the legacy KDF strength.
  pure val LEGACY_ENV: Envelope = {
    val k = { pw: FIXTURE_PW, salt: 100, iters: LEGACY_KDF_ITERATIONS }
    val aad = AadEnv({ scope: V1_SCOPE, lineage: LHex(100), commit: Dec(100) })
    {
      cipher: CIPHER, commit: Dec(100),
      data: DCt({ key: KPw(k), iv: 100, aad: aad, pt: PPayload({ version: 1, body: 0 }) }),
      iterations: LEGACY_KDF_ITERATIONS, iv: Dec(100), kdf: KDF_NAME, lineage: LHex(100),
      salt: Dec(100), scope: V1_SCOPE, version: ENVELOPE_VERSION,
    }
  }

  // ---------------------------------------------------------------- state

  var st: St
  var gh: Ghost

  // ---------------------------------------------------------------- actions: container

  /// new_dek (container.rs:126-128).
  action new_dek = all {
    st.entropyOk,
    st' = { ...st, deks: st.deks.union(Set(st.entropy)), entropy: st.entropy + 1 },
    gh' = gh,
  }

  /// Randomness::draw (container.rs:116-124).
  action Randomness_draw = all {
    st.entropyOk,
    val r = { dekIv: st.entropy, dekCommit: st.entropy + 1, payloadIv: st.entropy + 2, payloadCommit: st.entropy + 3 }
    st' = { ...st, randoms: st.randoms.union(Set(r)), entropy: st.entropy + 4 },
    gh' = gh,
  }

  /// seal_new (container.rs:178-212): version is not range-checked.
  action seal_new(key: SealingKey, dek: int, version: int, payload: Plain, randomness: Randomness): bool = all {
    st.keyring.contains(key),
    st.deks.contains(dek),
    st.randoms.contains(randomness),
    val f = sealNew(key, dek, version, payload, randomness)
    all {
      st' = { ...st, files: st.files.union(Set(f)) },
      gh' = { ...gh,
        produced: gh.produced.union(Set(f)),
        ctLog: gh.ctLog.union(envCt(f.passwordSlot)).union(payloadCts(f)),
        newLog: gh.newLog.union(Set({ sk: key, dek: dek, version: version, payload: payload, r: randomness, file: f })),
        versionMismatch: if (payloadVersion(payload) != version) gh.versionMismatch.union(Set(f)) else gh.versionMismatch,
      },
    },
  }

  /// write (container.rs:229-232): no validate; host write is atomic.
  action write(key: str, file: VaultFile): bool = all {
    st.files.contains(file),
    not(st.hostFailing),
    val v = SText(TContainer(file))
    all {
      st' = { ...st, storage: st.storage.set(key, v) },
      gh' = { ...gh, persisted: gh.persisted.union(Set(file)) },
    },
  }

  /// read (container.rs:215-225), with parse (container.rs:93-107). The caller keeps the result.
  action read(key: str, current_version: int): bool = all {
    val s = st.storage.get(key)
    val r = readStored(s, current_version, st.hostFailing)
    val newFiles = match r { | RdParsed(p) => match p { | PContainer(f) => Set(f) | _ => Set() } | _ => Set() }
    val newEnvs = match r { | RdParsed(p) => match p { | PEnvelope(e) => Set(e) | _ => Set() } | _ => Set() }
    all {
      st' = { ...st, files: st.files.union(newFiles), envelopes: st.envelopes.union(newEnvs) },
      gh' = { ...gh, readLog: gh.readLog.union(Set({ key: key, stored: s, cv: current_version, res: r })) },
    },
  }

  /// VaultFile::with_payload (container.rs:285-296): fresh IV and commit from the CSPRNG.
  action VaultFile_with_payload(file: VaultFile, dek: int, version: int, payload: Plain): bool = all {
    st.files.contains(file),
    st.deks.contains(dek),
    st.entropyOk,
    val out = withPayload(file, dek, version, payload, st.entropy, st.entropy + 1)
    all {
      st' = { ...st, files: st.files.union(Set(out)), entropy: st.entropy + 2 },
      gh' = { ...gh,
        produced: gh.produced.union(Set(out)),
        ctLog: gh.ctLog.union(payloadCts(out)),
        internalCts: gh.internalCts.union(payloadCts(out)),
        foreignDek: if (not(slotDek(file).contains(dek))) gh.foreignDek.union(Set(out)) else gh.foreignDek,
        versionMismatch: if (payloadVersion(payload) != version) gh.versionMismatch.union(Set(out)) else gh.versionMismatch,
        derivedLog: gh.derivedLog.union(Set({ base: file, dek: dek, version: version, payload: payload, out: out })),
      },
    },
  }

  /// VaultFile::seal_biometric (container.rs:299-314): random id and iv; the slot is returned.
  action VaultFile_seal_biometric(file: VaultFile, dek: int, wrap_secret: int, domain: Dom): bool = all {
    st.files.contains(file),
    st.deks.contains(dek),
    st.entropyOk,
    val slot = sealBiometric(file, dek, wrap_secret, domain, st.entropy, st.entropy + 1)
    all {
      st' = { ...st, bioSlots: st.bioSlots.union(Set({ file: file, slot: slot })), entropy: st.entropy + 2 },
      gh' = { ...gh, ctLog: gh.ctLog.union(bioCts(SomeBio(slot))), internalCts: gh.internalCts.union(bioCts(SomeBio(slot))) },
    },
  }

  /// The caller assigns the returned slot to `file.biometric_slot` (a public field).
  action VaultFile_set_biometric_slot(sealed: BioSealed): bool = all {
    st.bioSlots.contains(sealed),
    val f = sealed.file.with("biometricSlot", SomeBio(sealed.slot))
    all {
      st' = { ...st, files: st.files.union(Set(f)) },
      gh' = { ...gh,
        produced: gh.produced.union(Set(f)),
        foreignDek: if (gh.foreignDek.contains(sealed.file)) gh.foreignDek.union(Set(f)) else gh.foreignDek,
        versionMismatch: if (gh.versionMismatch.contains(sealed.file)) gh.versionMismatch.union(Set(f)) else gh.versionMismatch,
      },
    },
  }

  /// VaultFile::open_dek (container.rs:259-262).
  action VaultFile_open_dek(file: VaultFile, password: str): bool = all {
    st.files.contains(file),
    val r = openDek(file, password)
    val got = match r { | OdOk(x) => Set(x) | OdErr(_) => Set() }
    all {
      st' = { ...st, deks: st.deks.union(got.map(x => x.dek)), keyring: st.keyring.union(got.map(x => x.key)) },
      gh' = { ...gh, openDekLog: gh.openDekLog.union(Set({ file: file, pw: password, res: r })) },
    },
  }

  /// VaultFile::open_dek_with (container.rs:265-267).
  action VaultFile_open_dek_with(file: VaultFile, key: SealingKey): bool = all {
    st.files.contains(file),
    st.keyring.contains(key),
    val r = openDekWith(file, key)
    all {
      st' = st,
      gh' = { ...gh, dekWithLog: gh.dekWithLog.union(Set({ file: file, sk: key, res: r })) },
    },
  }

  /// VaultFile::open_payload (container.rs:271-281); the caller then checks the payload version.
  action VaultFile_open_payload(file: VaultFile, dek: int): bool = all {
    st.files.contains(file),
    callerAcceptsPayload(file, openPayload(file, dek)),
    st' = st,
    gh' = { ...gh, payloadLog: gh.payloadLog.union(Set({ file: file, dek: dek, res: openPayload(file, dek) })) },
  }

  /// VaultFile::open_biometric (container.rs:318-335).
  action VaultFile_open_biometric(file: VaultFile, wrap_secret: int): bool = all {
    st.files.contains(file),
    st' = st,
    gh' = { ...gh, bioLog: gh.bioLog.union(Set({ file: file, wrap: wrap_secret, res: openBiometric(file, wrap_secret) })) },
  }

  // ---------------------------------------------------------------- actions: envelope

  /// SealingKey::create (envelope.rs:115-117). The caller applies the passphrase rule first.
  action SealingKey_create(password: str): bool = all {
    passwordMeetsRule(password),
    st.entropyOk,
    val k = createKey(password, st.entropy)
    all {
      st' = { ...st, keyring: st.keyring.union(Set(k)), entropy: st.entropy + 2 },
      gh' = { ...gh, createdKeys: gh.createdKeys.union(Set(k)) },
    },
  }

  /// SealingKey::from_parts (envelope.rs:121-127): no check on the lineage.
  action SealingKey_from_parts(password: str, salt: int, lineage: Lin): bool = all {
    val k = fromParts(password, salt, lineage)
    all {
      st' = { ...st, keyring: st.keyring.union(Set(k)) },
      gh' = { ...gh, fromPartsKeys: gh.fromPartsKeys.union(Set(k)) },
    },
  }

  /// SealingKey::seal_with (envelope.rs:135-157): caller-supplied IV and commit.
  action SealingKey_seal_with(key: SealingKey, plaintext: Plain, scope: str, iv: int, commit: int): bool = all {
    st.keyring.contains(key),
    val e = sealWith(key, plaintext, scope, iv, commit)
    all {
      st' = { ...st, envelopes: st.envelopes.union(Set(e)) },
      gh' = { ...gh, ctLog: gh.ctLog.union(envCt(e)), sealedBy: gh.sealedBy.union(Set({ sk: key, env: e })) },
    },
  }

  /// SealingKey::seal (envelope.rs:130-132): fresh IV and commit from the CSPRNG.
  action SealingKey_seal(key: SealingKey, plaintext: Plain, scope: str): bool = all {
    st.keyring.contains(key),
    st.entropyOk,
    val e = sealWith(key, plaintext, scope, st.entropy, st.entropy + 1)
    all {
      st' = { ...st, envelopes: st.envelopes.union(Set(e)), entropy: st.entropy + 2 },
      gh' = { ...gh, ctLog: gh.ctLog.union(envCt(e)), internalCts: gh.internalCts.union(envCt(e)),
              sealedBy: gh.sealedBy.union(Set({ sk: key, env: e })) },
    },
  }

  /// SealingKey::open_sealed (envelope.rs:165-182).
  action SealingKey_open_sealed(key: SealingKey, envelope: Envelope, scope: str): bool = all {
    st.keyring.contains(key),
    st.envelopes.contains(envelope),
    st' = st,
    gh' = { ...gh, sealedLog: gh.sealedLog.union(Set({ sk: key, env: envelope, scope: scope, res: openSealed(key, envelope, scope) })) },
  }

  /// open (envelope.rs:197-226). The caller keeps the returned key.
  action open(envelope: Envelope, password: str, scope: str): bool = all {
    st.envelopes.contains(envelope),
    val r = openEnvelope(envelope, password, scope)
    val got = match r { | OpenOk(o) => Set(o.key) | OpenErr(_) => Set() }
    all {
      st' = { ...st, keyring: st.keyring.union(got) },
      gh' = { ...gh, openLog: gh.openLog.union(Set({ env: envelope, pw: password, scope: scope, res: r })) },
    },
  }

  // ---------------------------------------------------------------- actions: clock (host.rs)

  /// ClockReading::read (host.rs:43-56); the caller stores the reading.
  action ClockReading_read = all {
    st' = { ...st, readings: st.readings.union(Set(st.clock)) },
    gh' = gh,
  }

  /// ClockReading::read(clock).elapsed_since(earlier_boot, earlier_ms) (host.rs:60-65).
  action ClockReading_elapsed_since(earlier: Reading): bool = all {
    st.readings.contains(earlier),
    st' = st,
    gh' = { ...gh, elapsedLog: gh.elapsedLog.union(Set({ now: st.clock, earlier: earlier,
                                                            res: elapsedSince(st.clock, earlier.boot, earlier.ms) })) },
  }

  // ---------------------------------------------------------------- environment

  action clockAdvance(d: int): bool = all {
    st' = { ...st, clock: { boot: st.clock.boot, ms: st.clock.ms + d } },
    gh' = gh,
  }

  /// A host clock that steps back (threat: non-monotonic host).
  action clockBackwards(d: int): bool = all {
    st.clock.ms >= d,
    st' = { ...st, clock: { boot: st.clock.boot, ms: st.clock.ms - d } },
    gh' = gh,
  }

  /// A reboot: the host reports a fresh boot id.
  action reboot = all {
    val id = gh.bootIdsSeen.fold(0, (m, x) => if (x > m) x else m) + 1
    all {
      st' = { ...st, clock: { boot: id, ms: 0 } },
      gh' = { ...gh, bootIdsSeen: gh.bootIdsSeen.union(Set(id)), reboots: gh.reboots + 1 },
    },
  }

  /// A misbehaving host that reboots but keeps reporting the old boot id.
  action rebootKeepingId = all {
    st' = { ...st, clock: { boot: st.clock.boot, ms: 0 } },
    gh' = { ...gh, reboots: gh.reboots + 1 },
  }

  /// Untrusted vault.json: anything can be written to it behind the core's back.
  action tamperStorage(key: str, kind: str): bool = all {
    val v = tamperStored(st.storage.get(key), kind)
    all {
      st' = { ...st, storage: st.storage.set(key, v) },
      gh' = { ...gh, tampered: gh.tampered.union(Set(v)) },
    },
  }

  /// An envelope handed to the core from outside (an extension export, a backup) was edited.
  action tamperHeldEnvelope(envelope: Envelope, kind: str): bool = all {
    st.envelopes.contains(envelope),
    val e = tamperEnv(envelope, kind)
    all {
      st' = { ...st, envelopes: st.envelopes.union(Set(e)) },
      gh' = { ...gh, tamperedEnvs: gh.tamperedEnvs.union(Set(e)) },
    },
  }

  /// A crash during Storage::write: the host's atomic write leaves the old value.
  action crashDuringWrite(key: str, file: VaultFile): bool = all {
    st.files.contains(file),
    st' = st,
    gh' = { ...gh, crashKeptOld: gh.crashKeptOld or st.storage.get(key) != SText(TContainer(file)) },
  }

  action setHostFailing(b: bool): bool = all { st' = { ...st, hostFailing: b }, gh' = gh }
  action setEntropy(b: bool): bool = all { st' = { ...st, entropyOk: b }, gh' = gh }
  action changeBiometricDomain(s: int): bool = all { st' = { ...st, hostDomainState: s }, gh' = gh }

  // ---------------------------------------------------------------- init / step

  action init = all {
    st' = {
      storage: Map("vault.json" -> SText(TContainer(FIXTURE_FILE)),
                   "legacy.json" -> SText(TEnvelope(LEGACY_ENV)),
                   "spare.json" -> SAbsent),
      hostFailing: false,
      entropyOk: true,
      entropy: 10,
      keyring: Set(FIXTURE_KEY),
      deks: Set(FIXTURE_DEK),
      randoms: Set(FIXTURE_RANDOMNESS),
      files: Set(FIXTURE_FILE),
      envelopes: Set(LEGACY_ENV),
      bioSlots: Set(),
      hostDomainState: 0,
      clock: { boot: 0, ms: 100 },
      readings: Set({ boot: 0, ms: 40 }),
    },
    gh' = {
      written: Set(SText(TContainer(FIXTURE_FILE)), SText(TEnvelope(LEGACY_ENV)), SAbsent),
      tampered: Set(),
      persisted: Set(),
      internalCts: Set(),
      foreignDek: Set(),
      versionMismatch: Set(),
      tamperedEnvs: Set(),
      produced: Set(FIXTURE_FILE),
      ctLog: envCt(FIXTURE_FILE.passwordSlot).union(payloadCts(FIXTURE_FILE)).union(envCt(LEGACY_ENV)),
      sealedBy: Set(),
      createdKeys: Set(),
      fromPartsKeys: Set(),
      newLog: Set(),
      derivedLog: Set(),
      openLog: Set(),
      openDekLog: Set(),
      dekWithLog: Set(),
      payloadLog: Set(),
      bioLog: Set(),
      sealedLog: Set(),
      readLog: Set(),
      elapsedLog: Set(),
      bootIdsSeen: Set(0),
      reboots: 0,
      crashKeptOld: false,
    },
  }

  action step = any {
    new_dek,
    Randomness_draw,
    nondet key = st.keyring.oneOf()
    nondet dek = st.deks.oneOf()
    nondet version = VERSIONS.oneOf()
    nondet pv = VERSIONS.oneOf()
    nondet body = BODIES.oneOf()
    nondet r = st.randoms.oneOf()
    seal_new(key, dek, version, PPayload({ version: pv, body: body }), r),
    nondet k = STORAGE_KEYS.oneOf()
    nondet file = st.files.oneOf()
    write(k, file),
    nondet k = STORAGE_KEYS.oneOf()
    nondet cv = CURRENT_VERSIONS.oneOf()
    read(k, cv),
    nondet file = st.files.oneOf()
    nondet dek = st.deks.oneOf()
    nondet version = VERSIONS.oneOf()
    nondet pv = VERSIONS.oneOf()
    nondet body = BODIES.oneOf()
    VaultFile_with_payload(file, dek, version, PPayload({ version: pv, body: body })),
    nondet file = st.files.oneOf()
    nondet dek = st.deks.oneOf()
    nondet wrap = WRAPS.oneOf()
    nondet domain = Set(DEmpty, domain_hash(st.hostDomainState), DBad).oneOf()
    VaultFile_seal_biometric(file, dek, wrap, domain),
    all {
      st.bioSlots.size() > 0,
      nondet sealed = st.bioSlots.oneOf()
      VaultFile_set_biometric_slot(sealed),
    },
    nondet file = st.files.oneOf()
    nondet pw = PASSWORDS.oneOf()
    VaultFile_open_dek(file, pw),
    nondet file = st.files.oneOf()
    nondet key = st.keyring.oneOf()
    VaultFile_open_dek_with(file, key),
    nondet file = st.files.oneOf()
    nondet dek = st.deks.oneOf()
    VaultFile_open_payload(file, dek),
    nondet file = st.files.oneOf()
    nondet wrap = WRAPS.oneOf()
    VaultFile_open_biometric(file, wrap),
    nondet pw = PASSWORDS.oneOf()
    SealingKey_create(pw),
    nondet pw = PASSWORDS.oneOf()
    nondet salt = SALTS.oneOf()
    nondet lineage = LINEAGES.oneOf()
    SealingKey_from_parts(pw, salt, lineage),
    nondet key = st.keyring.oneOf()
    nondet pt = Set(PNotDekRecord, PDekRecord(FIXTURE_DEK), PPayload({ version: 1, body: 0 })).oneOf()
    nondet scope = SCOPES.oneOf()
    nondet iv = FIXED_NONCES.oneOf()
    nondet commit = FIXED_NONCES.oneOf()
    SealingKey_seal_with(key, pt, scope, iv, commit),
    nondet key = st.keyring.oneOf()
    nondet pt = Set(PNotDekRecord, PDekRecord(FIXTURE_DEK), PPayload({ version: 1, body: 0 })).oneOf()
    nondet scope = SCOPES.oneOf()
    SealingKey_seal(key, pt, scope),
    all {
      st.envelopes.size() > 0,
      nondet key = st.keyring.oneOf()
      nondet env = st.envelopes.oneOf()
      nondet scope = SCOPES.oneOf()
      SealingKey_open_sealed(key, env, scope),
    },
    all {
      st.envelopes.size() > 0,
      nondet env = st.envelopes.oneOf()
      nondet pw = PASSWORDS.oneOf()
      nondet scope = SCOPES.oneOf()
      open(env, pw, scope),
    },
    ClockReading_read,
    nondet earlier = st.readings.oneOf()
    ClockReading_elapsed_since(earlier),
    // the host clock
    any {
      nondet d = 0.to(60).oneOf()
      clockAdvance(d),
      nondet d = 0.to(60).oneOf()
      clockBackwards(d),
      reboot,
      rebootKeepingId,
    },
    nondet k = STORAGE_KEYS.oneOf()
    nondet kind = TAMPERS.oneOf()
    tamperStorage(k, kind),
    all {
      st.envelopes.size() > 0,
      nondet env = st.envelopes.oneOf()
      nondet kind = ENVELOPE_TAMPERS.oneOf()
      tamperHeldEnvelope(env, kind),
    },
    // host faults
    any {
      nondet k = STORAGE_KEYS.oneOf()
      nondet file = st.files.oneOf()
      crashDuringWrite(k, file),
      nondet b = Set(true, false).oneOf()
      setHostFailing(b),
      nondet b = Set(true, false).oneOf()
      setEntropy(b),
      nondet s = DOMAIN_STATES.oneOf()
      changeBiometricDomain(s),
    },
  }

  // ---------------------------------------------------------------- invariants

  /// No ciphertext sealed under an IV the core draws itself (seal, with_payload, seal_biometric)
  /// shares its key and IV with any other ciphertext; otherwise AES-GCM leaks the XOR of the
  /// plaintexts and its authentication key. IVs the caller supplies (seal_with, seal_new's
  /// Randomness) are the caller's to keep fresh, so they count only as the other side.
  invariant no_nonce_reuse_under_one_key: bool =
    gh.internalCts.forall(a => gh.ctLog.forall(b => a == b or a.key != b.key or a.iv != b.iv))

  /// A file seal_new built holds the DEK under the caller's key and the payload under that DEK,
  /// built from key, DEK and randomness the caller held, and is returned to the caller; every
  /// biometric-free file the core produced came from seal_new or with_payload. Otherwise the
  /// user's new vault cannot be unlocked with their passphrase.
  invariant seal_new_builds_dek_container: bool = and {
    gh.produced.forall(f => f == FIXTURE_FILE or f.biometricSlot != NoBio
      or gh.newLog.exists(e => e.file == f) or gh.derivedLog.exists(e => e.out == f)),
    gh.newLog.forall(e => and {
      st.keyring.contains(e.sk),
      st.deks.contains(e.dek),
      st.randoms.contains(e.r),
      st.files.contains(e.file),
      envCt(e.file.passwordSlot).forall(c => c.iv == e.r.dekIv),
      payloadCts(e.file).forall(c => c.iv == e.r.payloadIv),
    }),
    gh.newLog.forall(e => and {
      e.file.version == e.version,
      e.file.biometricSlot == NoBio,
      envCt(e.file.passwordSlot).forall(c => c.key == KPw(e.sk.key) and c.pt == PDekRecord(e.dek)),
      envCt(e.file.passwordSlot).size() == 1,
      payloadCts(e.file).forall(c => c.key == KDek(e.dek) and c.pt == e.payload),
      payloadCts(e.file).size() == 1,
    }),
  }

  /// with_payload keeps format, lineage and both slots, sets the version, and re-seals under a
  /// fresh IV and commit; otherwise a payload update would lock the user out or reuse a nonce.
  invariant with_payload_keeps_slots_and_refreshes_nonce: bool =
    gh.derivedLog.forall(e => and {
      e.out.format == e.base.format,
      e.out.lineage == e.base.lineage,
      e.out.passwordSlot == e.base.passwordSlot,
      e.out.biometricSlot == e.base.biometricSlot,
      e.out.version == e.version,
      e.out.payload.iv != e.base.payload.iv,
      e.out.payload.commit != e.base.payload.commit,
    })

  /// with_payload keeps the DEK: the re-sealed payload is under the DEK the slots wrap;
  /// otherwise the user's passphrase still unlocks a vault whose payload can no longer be read.
  /// with_payload does not check the DEK (container.rs:285-296): given the DEK the slot wraps,
  /// it keeps that slot and seals the payload under that same DEK.
  /// Every held file the core produced, unless a caller re-sealed it (or the file it came from)
  /// under a DEK its slot does not wrap, has its payload under the DEK its passphrase slot wraps.
  invariant with_payload_keeps_dek: bool = and {
    gh.derivedLog.forall(e => slotDek(e.base).contains(e.dek) implies and {
      slotDek(e.out) == slotDek(e.base),
      payloadCts(e.out).forall(c => c.key == KDek(e.dek)),
      payloadCts(e.out).size() == 1,
    }),
    st.files.forall(f => (gh.produced.contains(f) and not(gh.foreignDek.contains(f))) implies
      payloadCts(f).forall(c => slotDek(f).forall(d => c.key == KDek(d)))),
  }

  /// A file this core produced always opens its payload with the DEK its passphrase slot
  /// proves; a failure then means tampering, else the user's data is silently lost.
  /// Files a caller re-sealed under a foreign DEK (with_payload does not check it) are excluded:
  /// there the failure is the caller's bug, not tampering.
  /// The exclusion is sound: every with_payload under a foreign DEK is in it, and only produced,
  /// held files are.
  invariant payload_failure_means_tampering: bool = and {
    gh.derivedLog.forall(e => not(slotDek(e.base).contains(e.dek)) implies gh.foreignDek.contains(e.out)),
    gh.foreignDek.forall(f => gh.produced.contains(f) and st.files.contains(f)),
    gh.payloadLog.forall(e =>
      isOk(e.res) or not(gh.produced.contains(e.file)) or gh.foreignDek.contains(e.file)
      or not(slotDek(e.file).contains(e.dek))),
  }

  /// A produced container's version equals the version of the payload it seals; otherwise
  /// migrations run against the wrong schema and corrupt the wallet.
  /// The match is the caller's to keep (container.rs:5-7): seal_new and with_payload take version
  /// and payload separately. When the caller passes a matching pair, the file keeps it.
  /// Every produced file the caller did not seal with a mismatched pair carries its payload's
  /// version.
  invariant container_version_matches_payload: bool = and {
    gh.produced.forall(f => not(gh.versionMismatch.contains(f)) implies
      payloadCts(f).forall(c => payloadVersion(c.pt) == f.version)),
    gh.newLog.forall(e => payloadVersion(e.payload) == e.version implies
      payloadCts(e.file).forall(c => payloadVersion(c.pt) == e.file.version)),
    gh.derivedLog.forall(e => payloadVersion(e.payload) == e.version implies
      payloadCts(e.out).forall(c => payloadVersion(c.pt) == e.out.version)),
  }

  /// Every part of a held vault file is bound to the file's lineage; otherwise a slot or payload
  /// from another vault could be spliced in.
  /// Every file the core returned (built, re-sealed, enrolled or read) is in the caller's hands.
  invariant vault_parts_share_lineage: bool = and {
    gh.produced.forall(f => st.files.contains(f)),
    gh.readLog.forall(e => readContainers(e.res).forall(f => st.files.contains(f))),
    st.files.forall(f => and {
      f.passwordSlot.lineage == f.lineage,
      envCt(f.passwordSlot).forall(c => aadLineage(c.aad) == f.lineage),
      payloadCts(f).forall(c => aadLineage(c.aad) == f.lineage),
      bioCts(f.biometricSlot).forall(c => aadLineage(c.aad) == f.lineage),
    }),
  }

  /// The biometric slot unwraps a DEK only when its lineage, id and domain are exactly those it
  /// was sealed with, under the host's wrap secret; otherwise an edited (e.g. blanked) domain
  /// would skip the enrolment-change check and let a new fingerprint unlock the wallet.
  /// Every biometric ciphertext a slot or held file carries was sealed by seal_biometric, bound
  /// to the file's lineage and the slot's own id and domain.
  invariant biometric_opens_only_bound_slot: bool = and {
    st.bioSlots.forall(s => bioCts(SomeBio(s.slot)).forall(c => and {
      gh.internalCts.contains(c),
      c.aad == AadBio({ lineage: s.file.lineage, id: s.slot.id, domain: s.slot.domain }),
    })),
    st.files.forall(f => bioCts(f.biometricSlot).forall(c => gh.ctLog.contains(c))),
    gh.bioLog.forall(e => dekOk(e.res) implies match e.file.biometricSlot {
      | NoBio => false
      | SomeBio(s) => bioCts(e.file.biometricSlot).exists(c => and {
          gh.ctLog.contains(c),
          c.key == KWrap(e.wrap),
          c.aad == AadBio({ lineage: e.file.lineage, id: s.id, domain: s.domain }),
        })
    }),
  }

  /// open returns exactly the plaintext sealed in the envelope; otherwise the user gets
  /// someone else's or garbage data.
  /// The re-sealing key open returns is handed to the caller.
  invariant open_returns_sealed_plaintext: bool =
    gh.openLog.forall(e => match e.res {
      | OpenOk(o) => envCt(e.env).exists(c => c.pt == o.pt) and st.keyring.contains(o.key)
      | OpenErr(_) => true
    })

  /// An envelope opens only under the scope it was sealed for; otherwise a DEK slot could be
  /// replayed as a payload (or vice versa).
  invariant opened_only_under_sealing_scope: bool = and {
    gh.openLog.forall(e => match e.res {
      | OpenOk(_) => gh.ctLog.exists(c => envCt(e.env).contains(c) and aadScope(c.aad) == e.scope)
      | OpenErr(_) => true
    }),
    gh.sealedLog.forall(e => isOk(e.res) implies
      gh.ctLog.exists(c => envCt(e.env).contains(c) and aadScope(c.aad) == e.scope)),
  }

  /// Passphrase opens have two outcomes on a well-formed envelope or slot: WrongPassword, or
  /// success that hands the caller its sealing key. So tampering and a wrong passphrase are
  /// indistinguishable, and a passphrase other than the sealing one is WrongPassword; otherwise
  /// an attacker learns which fields were checked, or the user sees a misleading corruption error.
  invariant tamper_indistinguishable_from_wrong_password: bool = and {
    gh.openLog.forall(e =>
      (validShape(e.env, e.scope) and isDec(e.env.salt) and isDec(e.env.iv) and dataDecodes(e.env.data))
      implies e.res != OpenErr(EInvalid)),
    gh.openLog.forall(e => openedKeys(e.res).forall(k => st.keyring.contains(k))),
    gh.openDekLog.forall(e =>
      (validShape(e.file.passwordSlot, DEK_SCOPE) and isDec(e.file.passwordSlot.salt)
       and isDec(e.file.passwordSlot.iv) and dataDecodes(e.file.passwordSlot.data)
       and slotPassword(e.file.passwordSlot).exists(p => p != e.pw))
      implies e.res == OdErr(EWrongPassword)),
    gh.openDekLog.forall(e => odKeys(e.res).forall(k => st.keyring.contains(k))),
  }

  /// KDF strength: envelope reads accept only the fields the extension accepts (open: 600k or
  /// legacy 310k; open_sealed: 600k only, so an iterations field edited to 310k fails closed).
  /// A legacy envelope opened by open yields a key re-derived at full strength, and every key
  /// the caller holds is at full strength and was returned by create, from_parts, open or
  /// open_dek. Otherwise a tampered-down count weakens the passphrase protection, or the vault
  /// is re-sealed at the weak legacy strength forever.
  invariant opened_keys_at_current_strength: bool = and {
    gh.openLog.forall(e => match e.res {
      | OpenOk(_) => and {
          e.env.iterations == KDF_ITERATIONS or e.env.iterations == LEGACY_KDF_ITERATIONS,
          e.env.version == ENVELOPE_VERSION, e.env.cipher == CIPHER, e.env.kdf == KDF_NAME,
        }
      | OpenErr(_) => true
    }),
    gh.sealedLog.forall(e => isOk(e.res) implies e.env.iterations == KDF_ITERATIONS),
    gh.openLog.forall(e => match e.res {
      | OpenOk(o) => o.key.key.iters == KDF_ITERATIONS and o.key.lineage == e.env.lineage
      | OpenErr(_) => true
    }),
    st.keyring.forall(k => k.key.iters == KDF_ITERATIONS and or {
      k == FIXTURE_KEY,
      gh.createdKeys.contains(k),
      gh.fromPartsKeys.contains(k),
      gh.openLog.exists(e => openedKeys(e.res).contains(k)),
      gh.openDekLog.exists(e => odKeys(e.res).contains(k)),
    }),
  }

  /// open_sealed opens every envelope the same key sealed under its own scope; otherwise a
  /// freshly sealed secret cannot be read back.
  /// Every envelope the caller holds was sealed by one of its keys, read from storage, or
  /// edited by the environment, and a sealing key is one the caller holds.
  invariant open_sealed_opens_own_envelopes: bool = and {
    st.envelopes.forall(env => or {
      env == LEGACY_ENV,
      gh.sealedBy.exists(x => x.env == env),
      gh.readLog.exists(e => e.res == RdParsed(PEnvelope(env))),
      gh.tamperedEnvs.contains(env),
    }),
    gh.sealedBy.forall(x => st.keyring.contains(x.sk) and st.envelopes.contains(x.env)),
    gh.sealedLog.forall(e =>
      (gh.sealedBy.contains({ sk: e.sk, env: e.env }) and e.scope == e.env.scope
       and isLowerHex(e.sk.lineage)) implies isOk(e.res)),
  }

  /// read yields a container only for stored bytes that are a well-formed container of an
  /// accepted version, and an envelope only for an envelope; otherwise a malformed file reaches
  /// key derivation.
  /// The caller only ever holds files the core built itself or validated on read.
  invariant read_fails_closed: bool = and {
    st.files.forall(f => gh.produced.contains(f) or gh.readLog.exists(e => readContainers(e.res).contains(f))),
    gh.readLog.forall(e => match e.res {
      | RdParsed(p) => match p {
          | PContainer(f) => and {
              e.stored == SText(TContainer(f)),
              f.format == FORMAT,
              FIRST_CONTAINER_VERSION <= f.version, f.version <= e.cv,
              isLowerHex(f.lineage), f.passwordSlot.lineage == f.lineage,
              f.passwordSlot.scope == DEK_SCOPE, f.payload.scope == PAYLOAD_SCOPE,
            }
          | PEnvelope(env) => e.stored == SText(TEnvelope(env))
          | PInvalid => true
        }
      | RdNone => e.stored == SAbsent
      | RdHostErr => true
    }),
  }

  /// Every stored value is the initial value, an edit by the environment, or one whole
  /// VaultFile the caller held when it called write. Otherwise a crash leaves the user with a
  /// torn, unreadable vault, or storage holds a file the core never wrote.
  invariant storage_never_torn: bool =
    STORAGE_KEYS.forall(k => {
      val v = st.storage.get(k)
      or {
        gh.written.contains(v),
        gh.tampered.contains(v),
        storedFiles(v).size() == 1 and storedFiles(v).forall(f => gh.persisted.contains(f) and st.files.contains(f)),
      }
    })

  /// SealingKey::create never repeats a salt or lineage; otherwise two vaults share a lineage
  /// and their envelopes can be swapped.
  invariant created_keys_fresh: bool =
    gh.createdKeys.forall(a => gh.createdKeys.forall(b =>
      a == b or (a.salt != b.salt and a.lineage != b.lineage)))

  /// The core trusts a clock reading only within one boot and forward in time. Readings carry
  /// only boot ids the host reported. elapsed_since reports a duration only for the same boot id
  /// and a clock that did not go backwards, and the duration is then the true difference.
  /// Otherwise a reboot or a lying clock shortens a passphrase backoff or extends an idle
  /// session. (That the host changes the id on every boot is the host's contract; the core
  /// cannot check it.)
  invariant clock_trusted_only_within_boot: bool = and {
    gh.bootIdsSeen.contains(st.clock.boot),
    st.readings.forall(r => gh.bootIdsSeen.contains(r.boot)),
    gh.elapsedLog.forall(e => match e.res {
      | ESome(d) => e.now.boot == e.earlier.boot and e.now.ms >= e.earlier.ms and d == e.now.ms - e.earlier.ms
      | ENone => true
    }),
  }

  // ---------------------------------------------------------------- observations

  /// A container whose payload is sealed under a random DEK was written to storage.
  observation dek_vault_persisted: bool =
    gh.persisted.exists(f => payloadCts(f).exists(c => match c.key { | KDek(_) => true | _ => false })
      and STORAGE_KEYS.exists(k => st.storage.get(k) == SText(TContainer(f))))

  /// open_dek with a passphrase other than the sealing one returned WrongPassword.
  observation wrong_password_rejected: bool =
    gh.openDekLog.exists(e => e.res == OdErr(EWrongPassword) and not(slotPassword(e.file.passwordSlot).contains(e.pw)))

  /// The right passphrase got WrongPassword: the envelope or slot was tampered with.
  observation tampered_envelope_looks_like_wrong_password: bool = or {
    gh.openDekLog.exists(e => e.res == OdErr(EWrongPassword) and slotPassword(e.file.passwordSlot).contains(e.pw)),
    gh.openLog.exists(e => e.res == OpenErr(EWrongPassword) and slotPassword(e.env).contains(e.pw)),
  }

  /// open_dek unwrapped the DEK and a sealing key.
  observation dek_unlocked: bool =
    gh.openDekLog.exists(e => match e.res { | OdOk(_) => true | OdErr(_) => false })

  /// A legacy 310k envelope opened and yielded a key re-derived at 600k.
  observation legacy_vault_upgraded: bool =
    gh.openLog.exists(e => e.env.iterations == LEGACY_KDF_ITERATIONS
      and match e.res { | OpenOk(_) => true | OpenErr(_) => false })

  /// open rejected an envelope as invalid (bad shape, scope or encoding) before the KDF.
  observation envelope_rejected_invalid: bool =
    gh.openLog.exists(e => e.res == OpenErr(EInvalid))

  /// read found a container but rejected it as invalid.
  observation invalid_container_rejected: bool =
    gh.readLog.exists(e => e.res == RdParsed(PInvalid)
      and match e.stored { | SText(t) => match t { | TContainer(_) => true | _ => false } | SAbsent => false })

  /// read rejected bytes that are not a vault at all.
  observation corrupt_bytes_rejected: bool =
    gh.readLog.exists(e => e.res == RdParsed(PInvalid)
      and (e.stored == SText(TNotUtf8) or e.stored == SText(TNotJson) or e.stored == SText(TNoKnownKeys)))

  /// read classified a bare pre-container envelope.
  observation legacy_envelope_read: bool =
    gh.readLog.exists(e => match e.res { | RdParsed(p) => match p { | PEnvelope(_) => true | _ => false } | _ => false })

  /// read accepted a container.
  observation container_read: bool =
    gh.readLog.exists(e => match e.res { | RdParsed(p) => match p { | PContainer(_) => true | _ => false } | _ => false })

  /// read found no vault under the key.
  observation vault_absent: bool = gh.readLog.exists(e => e.res == RdNone)

  /// The host's Storage::read failed.
  observation host_read_failed: bool = gh.readLog.exists(e => e.res == RdHostErr)

  /// The biometric slot unwrapped the DEK.
  observation biometric_unlocked: bool = gh.bioLog.exists(e => dekOk(e.res))

  /// The biometric slot exists but was invalidated (stale secret or tampered slot).
  observation biometric_invalidated: bool = gh.bioLog.exists(e => e.res == DekErr(EBioInvalidated))

  /// No biometric slot was enrolled.
  observation biometric_unavailable: bool = gh.bioLog.exists(e => e.res == DekErr(EBioUnavailable))

  /// The payload decrypted under the DEK.
  observation payload_opened: bool = gh.payloadLog.exists(e => isOk(e.res))

  /// The payload failed to decrypt.
  observation payload_rejected: bool = gh.payloadLog.exists(e => not(isOk(e.res)))

  /// with_payload re-sealed a container at a higher version (upgrade in place).
  observation vault_upgraded_in_place: bool = gh.derivedLog.exists(e => e.version > e.base.version)

  /// with_payload set a lower version than the base file (unchecked downgrade).
  observation vault_version_downgraded: bool = gh.derivedLog.exists(e => e.version < e.base.version)

  /// with_payload re-sealed the payload under a DEK the passphrase slot does not wrap.
  observation payload_resealed_under_foreign_dek: bool =
    gh.derivedLog.exists(e => not(slotDek(e.base).contains(e.dek)))

  /// open_sealed opened an envelope without the KDF.
  observation sealed_envelope_reopened: bool = gh.sealedLog.exists(e => isOk(e.res))

  /// open_sealed rejected an envelope.
  observation sealed_envelope_rejected: bool = gh.sealedLog.exists(e => not(isOk(e.res)))

  /// open_dek_with unwrapped the DEK with an already-derived key.
  observation dek_unlocked_without_kdf: bool = gh.dekWithLog.exists(e => dekOk(e.res))

  /// elapsed_since measured a duration.
  observation elapsed_measured: bool =
    gh.elapsedLog.exists(e => match e.res { | ESome(_) => true | ENone => false })

  /// elapsed_since refused to compare readings across a boot change.
  observation elapsed_refused_across_boot: bool =
    gh.elapsedLog.exists(e => e.res == ENone and e.now.boot != e.earlier.boot)

  /// elapsed_since refused because the clock went backwards.
  observation elapsed_refused_clock_backwards: bool =
    gh.elapsedLog.exists(e => e.res == ENone and e.now.boot == e.earlier.boot)

  /// A write was interrupted and the old value stayed in place.
  observation interrupted_write_kept_old_value: bool = gh.crashKeptOld
}
