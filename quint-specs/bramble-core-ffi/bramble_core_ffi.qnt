module bramble_core_ffi {
  // Bramble Core FFI exports (crates/bramble-core/src/lib.rs).
  //
  // The component is a stateless facade: every export is a function of its arguments that
  // forwards to starknet_wallet_core with `?`. The model therefore keeps no component state of
  // its own. It keeps the host's side of the FFI boundary instead: the values each export has
  // handed back to the extension / mobile host (ghost logs), the relay the host publishes gift
  // wraps to, the build's `raw-keys` feature and the latest OZ version in Krusty's manifest.
  // Wallet-core's cryptography is abstracted by injective constructors: a key IS the record of
  // what it was derived from, an address IS its (public key, class hash, salt) triple, and a
  // signature IS its (signer, message) pair.

  // ---------------------------------------------------------------------------------------
  // Bramble's choices (lib.rs:30-54)
  // ---------------------------------------------------------------------------------------

  /// lib.rs:30, pinned; never Krusty's `latest`.
  pure val OZ_ACCOUNT_VERSION: str = "3.0.0"
  // FEE_MARGIN_PERCENT = 50, REVIEWED_FEE_HEADROOM = 3/2 and CONFIRMATION_DELAYS_MS =
  // [0, 250, 500, 900, 1500, 2500, 4000, 5000, 7500, 10000, 10000, 10000] (lib.rs:34-54) are
  // only read by wallet-core's send::plan through SEND_POLICY; the `send` component models them.
  // core_info (lib.rs:159-164) returns the compile-time versions "0.2.1" and KRUSTY_VERSION
  // "2.0.0" and touches nothing else.

  // ---------------------------------------------------------------------------------------
  // wallet-core's bounds (kms.rs:19-23, nostr.rs:16-20)
  // ---------------------------------------------------------------------------------------

  pure val MNEMONIC_WORD_COUNTS: Set[int] = Set(12, 15, 18, 21, 24)
  pure val MAX_MNEMONIC_TEXT_LENGTH: int = 512
  pure val MAX_BIP39_PASSPHRASE_LENGTH: int = 1024
  pure val MAX_SAFE_INTEGER: int = 9007199254740991
  /// BIP-32 hardened offset; Krusty rejects an index at or above it (kms.rs:331).
  pure val HARDENED: int = 2147483648
  /// Stand-in for the Stark curve order: a scalar is usable when 0 < n < CURVE_ORDER.
  pure val CURVE_ORDER: int = 1000

  // ---------------------------------------------------------------------------------------
  // Types
  // ---------------------------------------------------------------------------------------

  type Build = RawKeys | NoRawKeys
  type Network = Mainnet | Sepolia
  type Scheme = StarknetBip44 | ArgentLegacy | NostrCoordination

  type Passphrase = NoPassphrase | WithPassphrase({ secret: int, len: int })
  /// A phrase as text: which entropy it encodes, how many words, whether its checksum holds, and
  /// its length in UTF-16 units (0 = blank).
  type Mnemonic = { seed: int, words: int, checksum: bool, len: int }
  type DerivationInput = {
    mnemonic: Mnemonic, addressIndex: int, accountIndex: int, passphrase: Passphrase
  }

  /// A derived secret, as the record of what derived it.
  type Key = { scheme: Scheme, seed: int, passphrase: Passphrase, path: List[int] }
  type Scalar = Derived(Key) | Literal(int)
  type PublicKey = { of: Scalar }
  /// A raw key as the host passes it: felt text, or text that is not a felt.
  type KeyText = Text(Scalar) | Garbage
  /// Felt text: canonical, with leading zeros ("0x01"), or outside the grammar ("0xFF").
  type FeltText = Canonical(int) | Padded(int) | Malformed
  type SaltArg = OmittedSalt | GivenSalt(FeltText)

  type OzAddress = { publicKey: int, classHash: int, salt: int }
  type Signature = { publicKey: PublicKey, signer: Scalar, message: int }
  type ViewingKey = { owner: Scalar, chain: int, pool: int }

  type RecipientText = Hex(PublicKey) | BadHex
  type GiftWrap = {
    sender: Key, recipient: PublicKey, identifier: str, content: str, createdAt: int
  }
  /// A relay event: as wrapped, altered in transit, or not the event grammar at all.
  type Event = Wrapped(GiftWrap) | Tampered(GiftWrap) | Garbled
  type NostrApplicationData = { content: str, identifier: str, senderPublicKey: PublicKey }

  /// What the host holds from the calls it made across the FFI.
  type HostView = {
    generated: Set[Mnemonic],
    keypairs: Set[{ scheme: Scheme, input: DerivationInput, privateKey: Key, publicKeyX: PublicKey }],
    addresses: Set[{ publicKeyX: FeltText, classHash: FeltText, salt: SaltArg, address: OzAddress }],
    classHashes: Set[{ network: Network, manifestLatest: str, hash: int }],
    signatures: Set[{ key: Scalar, hash: FeltText, signature: Signature }],
    viewingKeys: Set[{ key: Scalar, chainId: FeltText, poolAddress: FeltText, viewingKey: ViewingKey }],
    nostrKeys: Set[{ input: DerivationInput, publicKey: PublicKey }],
    relay: Set[Event],
    opened: Set[{ opener: Key, event: Event, data: NostrApplicationData }],
  }

  // ---------------------------------------------------------------------------------------
  // Krusty's manifest of OZ account classes (stand-in values; one per network and version)
  // ---------------------------------------------------------------------------------------

  pure val MANIFEST_VERSIONS: Set[str] = Set("3.0.0", "3.1.0")
  pure val CLASS_MANIFEST: (Network, str) -> int = Map(
    (Mainnet, "3.0.0") -> 300,
    (Sepolia, "3.0.0") -> 300,
    (Mainnet, "3.1.0") -> 310,
    (Sepolia, "3.1.0") -> 310,
  )

  // ---------------------------------------------------------------------------------------
  // State
  // ---------------------------------------------------------------------------------------

  /// The `raw-keys` feature of this build: default for the extension, off for mobile.
  var build: Build
  /// The latest OZ version in Krusty's manifest; moves with Krusty releases.
  var krustyLatest: str
  var host: HostView

  // ---------------------------------------------------------------------------------------
  // Pure helpers: wallet-core's checks and results
  // ---------------------------------------------------------------------------------------

  /// is_mnemonic_text (kms.rs:76-78): non-blank and within the resource bound.
  pure def isMnemonicText(m: Mnemonic): bool =
    m.len > 0 and m.len <= MAX_MNEMONIC_TEXT_LENGTH

  pure def passphraseInBound(p: Passphrase): bool = match p {
    | NoPassphrase => true
    | WithPassphrase(x) => x.len <= MAX_BIP39_PASSPHRASE_LENGTH
  }

  /// check_derivation_input (kms.rs:80-91).
  pure def checkDerivationInput(m: Mnemonic, p: Passphrase): bool =
    isMnemonicText(m) and passphraseInBound(p)

  /// validate_mnemonic (kms.rs:148-157): text bound, supported word count, BIP-39 checksum.
  pure def validateMnemonic(m: Mnemonic): bool =
    isMnemonicText(m) and MNEMONIC_WORD_COUNTS.contains(m.words) and m.checksum

  /// Krusty's derivation: a parseable BIP-39 phrase and non-hardened indices, else Kms error.
  pure def krustyDerives(m: Mnemonic, addressIndex: int, accountIndex: int): bool =
    m.checksum and MNEMONIC_WORD_COUNTS.contains(m.words)
      and addressIndex >= 0 and addressIndex < HARDENED
      and accountIndex >= 0 and accountIndex < HARDENED

  /// Krusty's phrase for `wordCount` words; `krustyOk` false is a Krusty regression that emits a
  /// phrase failing its own checksum, which wallet-core's re-validation (kms.rs:141) catches.
  pure def applyGenerate(wordCount: int, entropy: int, krustyOk: bool): Mnemonic =
    { seed: entropy, words: wordCount, checksum: krustyOk, len: wordCount * 8 }

  pure def publicOf(s: Scalar): PublicKey = { of: s }

  /// m/44'/9004'/account_index'/0/address_index (kms.rs:160).
  pure def starknetKey(m: Mnemonic, addressIndex: int, accountIndex: int, p: Passphrase): Key =
    { scheme: StarknetBip44, seed: m.seed, passphrase: p, path: [44, 9004, accountIndex, 0, addressIndex] }

  /// Ready legacy: hardened account fixed at zero, no passphrase (kms.rs:186-207).
  pure def argentLegacyKey(m: Mnemonic, addressIndex: int): Key =
    { scheme: ArgentLegacy, seed: m.seed, passphrase: NoPassphrase, path: [44, 9004, 0, 0, addressIndex] }

  pure def nostrKey(m: Mnemonic, addressIndex: int, accountIndex: int, p: Passphrase): Key =
    { scheme: NostrCoordination, seed: m.seed, passphrase: p, path: [accountIndex, addressIndex] }

  pure def isFelt(t: FeltText): bool = match t {
    | Canonical(n) => true
    | Padded(n) => true
    | Malformed => false
  }

  /// The felt a parseable text denotes; read only where isFelt holds.
  pure def feltValue(t: FeltText): int = match t {
    | Canonical(n) => n
    | Padded(n) => n
    | Malformed => 0
  }

  pure def isCanonicalNonZero(t: FeltText): bool = match t {
    | Canonical(n) => n != 0
    | Padded(n) => false
    | Malformed => false
  }

  /// lib.rs:228: an omitted salt is the public key text.
  pure def effectiveSalt(publicKeyX: FeltText, salt: SaltArg): FeltText = match salt {
    | OmittedSalt => publicKeyX
    | GivenSalt(s) => s
  }

  /// key_from_text (lib.rs:141-148): the felt grammar only.
  pure def isKeyText(k: KeyText): bool = match k {
    | Text(s) => true
    | Garbage => false
  }

  pure def scalarInRange(s: Scalar): bool = match s {
    | Derived(k) => true
    | Literal(n) => n > 0 and n < CURVE_ORDER
  }

  /// usable_key (kms.rs:113-118): the range is wallet-core's to check.
  pure def isUsableKey(k: KeyText): bool = match k {
    | Text(s) => scalarInRange(s)
    | Garbage => false
  }

  pure def signEntries(k: KeyText, hash: FeltText): Set[{ key: Scalar, hash: FeltText, signature: Signature }] =
    match k {
      | Text(s) => Set({ key: s, hash: hash, signature: { publicKey: publicOf(s), signer: s, message: feltValue(hash) } })
      | Garbage => Set()
    }

  pure def viewEntries(k: KeyText, chainId: FeltText, poolAddress: FeltText): Set[{ key: Scalar, chainId: FeltText, poolAddress: FeltText, viewingKey: ViewingKey }] =
    match k {
      | Text(s) => Set({ key: s, chainId: chainId, poolAddress: poolAddress,
                         viewingKey: { owner: s, chain: feltValue(chainId), pool: feltValue(poolAddress) } })
      | Garbage => Set()
    }

  /// is_public_key (nostr.rs:50).
  pure def isRecipientKey(r: RecipientText): bool = match r {
    | Hex(pk) => true
    | BadHex => false
  }

  pure def recipientKeys(r: RecipientText): Set[PublicKey] = match r {
    | Hex(pk) => Set(pk)
    | BadHex => Set()
  }

  /// is_identifier / is_application_data (nostr.rs:55-62): non-empty (byte bounds abstracted).
  pure def isIdentifier(identifier: str): bool = identifier != ""
  pure def isApplicationData(content: str): bool = content != ""

  /// parse_event_json + kind check (nostr.rs:233-240).
  pure def isEventGrammar(e: Event): bool = match e {
    | Wrapped(w) => true
    | Tampered(w) => true
    | Garbled => false
  }

  /// Krusty's open: signature and seal checks fail closed on a tampered event, decryption fails
  /// for anyone but the recipient.
  pure def krustyOpens(k: Key, e: Event): bool = match e {
    | Wrapped(w) => w.recipient == publicOf(Derived(k))
    | Tampered(w) => false
    | Garbled => false
  }

  pure def openedEntries(k: Key, e: Event): Set[{ opener: Key, event: Event, data: NostrApplicationData }] =
    match e {
      | Wrapped(w) => Set({ opener: k, event: e,
                            data: { content: w.content, identifier: w.identifier,
                                    senderPublicKey: publicOf(Derived(w.sender)) } })
      | Tampered(w) => Set()
      | Garbled => Set()
    }

  pure def wrappedOf(e: Event): Set[GiftWrap] = match e {
    | Wrapped(w) => Set(w)
    | Tampered(w) => Set()
    | Garbled => Set()
  }

  // ---------------------------------------------------------------------------------------
  // Domains
  // ---------------------------------------------------------------------------------------

  pure val MNEMONICS: Set[Mnemonic] = Set(
    { seed: 1, words: 12, checksum: true, len: 70 },     // fixture phrase
    { seed: 2, words: 24, checksum: true, len: 150 },
    { seed: 1, words: 13, checksum: false, len: 76 },    // phrase + " extra"
    { seed: 3, words: 12, checksum: false, len: 70 },    // bad checksum
    { seed: 0, words: 0, checksum: false, len: 0 },      // blank
    { seed: 4, words: 300, checksum: false, len: 600 },  // over the text bound
  )
  pure val INDICES: Set[int] = Set(0, 1, HARDENED)
  pure val PASSPHRASES: Set[Passphrase] = Set(
    NoPassphrase,
    WithPassphrase({ secret: 1, len: 8 }),
    WithPassphrase({ secret: 2, len: 1024 }),
    WithPassphrase({ secret: 3, len: 1025 }),
  )
  pure val DERIVATION_INPUTS: Set[DerivationInput] =
    tuples(MNEMONICS, INDICES, INDICES, PASSPHRASES).map(t =>
      { mnemonic: t._1, addressIndex: t._2, accountIndex: t._3, passphrase: t._4 })
  /// Openers that fail check_derivation_input, beside the host's own identities.
  pure val BAD_OPENERS: Set[DerivationInput] = Set(
    { mnemonic: { seed: 0, words: 0, checksum: false, len: 0 }, addressIndex: 0, accountIndex: 0, passphrase: NoPassphrase },
    { mnemonic: { seed: 1, words: 12, checksum: true, len: 70 }, addressIndex: 0, accountIndex: 0,
      passphrase: WithPassphrase({ secret: 3, len: 1025 }) },
    { mnemonic: { seed: 1, words: 12, checksum: true, len: 70 }, addressIndex: HARDENED, accountIndex: 0, passphrase: NoPassphrase },
  )
  pure val WORD_COUNTS: Set[int] = Set(0, 12, 13, 24, 255)
  pure val ENTROPY: Set[int] = Set(100, 101)
  pure val PUBLIC_KEY_TEXTS: Set[FeltText] = Set(Canonical(11), Padded(11), Malformed)
  pure val CLASS_HASH_TEXTS: Set[FeltText] = Set(Canonical(300), Malformed)
  pure val SALTS: Set[SaltArg] = Set(OmittedSalt, GivenSalt(Canonical(0)), GivenSalt(Canonical(11)), GivenSalt(Malformed))
  pure val LITERAL_KEYS: Set[KeyText] = Set(Text(Literal(0)), Text(Literal(1)), Text(Literal(CURVE_ORDER)), Garbage)
  pure val MESSAGE_HASHES: Set[FeltText] = Set(Canonical(1), Padded(1), Canonical(2), Malformed)
  /// SN_SEPOLIA stand-in 2, pool 0x123 = 291.
  pure val CHAIN_IDS: Set[FeltText] = Set(Canonical(2), Padded(2), Canonical(0))
  pure val POOLS: Set[FeltText] = Set(Canonical(291), Padded(291), Canonical(0))
  pure val IDENTIFIERS: Set[str] = Set("bramble.multisig", "")
  pure val CONTENTS: Set[str] = Set("round-trip", "")
  pure val CREATED_AT: Set[int] = Set(1, 1750000000, MAX_SAFE_INTEGER + 1)

  // ---------------------------------------------------------------------------------------
  // Actions
  // ---------------------------------------------------------------------------------------

  pure val EMPTY_HOST: HostView = {
    generated: Set(), keypairs: Set(), addresses: Set(), classHashes: Set(), signatures: Set(),
    viewingKeys: Set(), nostrKeys: Set(), relay: Set(), opened: Set(),
  }

  action initialize(b: Build): bool = all {
    build' = b,
    krustyLatest' = OZ_ACCOUNT_VERSION,
    host' = EMPTY_HOST,
  }

  action init = {
    nondet b = Set(RawKeys, NoRawKeys).oneOf()
    initialize(b)
  }

  /// generate_mnemonic (lib.rs:168-172 -> kms.rs:132-145).
  action generate_mnemonic(word_count: int, entropy: int, krusty_ok: bool): bool = all {
    MNEMONIC_WORD_COUNTS.contains(word_count),
    validateMnemonic(applyGenerate(word_count, entropy, krusty_ok)),
    host' = { ...host, generated: host.generated.union(Set(applyGenerate(word_count, entropy, krusty_ok))) },
    build' = build,
    krustyLatest' = krustyLatest,
  }

  /// derive_starknet_keypair [raw-keys] (lib.rs:181-200).
  action derive_starknet_keypair(mnemonic: Mnemonic, address_index: int, account_index: int, passphrase: Passphrase): bool = all {
    build == RawKeys,
    checkDerivationInput(mnemonic, passphrase),
    krustyDerives(mnemonic, address_index, account_index),
    host' = { ...host, keypairs: host.keypairs.union(Set({
      scheme: StarknetBip44,
      input: { mnemonic: mnemonic, addressIndex: address_index, accountIndex: account_index, passphrase: passphrase },
      privateKey: starknetKey(mnemonic, address_index, account_index, passphrase),
      publicKeyX: publicOf(Derived(starknetKey(mnemonic, address_index, account_index, passphrase))),
    })) },
    build' = build,
    krustyLatest' = krustyLatest,
  }

  /// derive_argent_legacy_keypair [raw-keys] (lib.rs:203-216).
  action derive_argent_legacy_keypair(mnemonic: Mnemonic, address_index: int): bool = all {
    build == RawKeys,
    checkDerivationInput(mnemonic, NoPassphrase),
    krustyDerives(mnemonic, address_index, 0),
    host' = { ...host, keypairs: host.keypairs.union(Set({
      scheme: ArgentLegacy,
      input: { mnemonic: mnemonic, addressIndex: address_index, accountIndex: 0, passphrase: NoPassphrase },
      privateKey: argentLegacyKey(mnemonic, address_index),
      publicKeyX: publicOf(Derived(argentLegacyKey(mnemonic, address_index))),
    })) },
    build' = build,
    krustyLatest' = krustyLatest,
  }

  /// derive_oz_account_address (lib.rs:222-233 -> kms.rs:212-224).
  action derive_oz_account_address(public_key_x: FeltText, class_hash: FeltText, salt: SaltArg): bool = all {
    isFelt(public_key_x),
    isFelt(class_hash),
    isFelt(effectiveSalt(public_key_x, salt)),
    host' = { ...host, addresses: host.addresses.union(Set({
      publicKeyX: public_key_x, classHash: class_hash, salt: salt,
      address: {
        publicKey: feltValue(public_key_x),
        classHash: feltValue(class_hash),
        salt: feltValue(effectiveSalt(public_key_x, salt)),
      },
    })) },
    build' = build,
    krustyLatest' = krustyLatest,
  }

  /// oz_account_class_hash (lib.rs:237-243 -> kms.rs:229-234), always at OZ_ACCOUNT_VERSION.
  action oz_account_class_hash(network: Network): bool = all {
    CLASS_MANIFEST.keys().contains((network, OZ_ACCOUNT_VERSION)),
    host' = { ...host, classHashes: host.classHashes.union(Set({
      network: network,
      manifestLatest: krustyLatest,
      hash: CLASS_MANIFEST.get((network, OZ_ACCOUNT_VERSION)),
    })) },
    build' = build,
    krustyLatest' = krustyLatest,
  }

  /// Environment: a Krusty release moves its manifest's `latest` OZ version.
  action krusty_manifest_bump(version: str): bool = all {
    krustyLatest' = version,
    host' = host,
    build' = build,
  }

  /// sign_stark_hash [raw-keys] (lib.rs:246-254 -> kms.rs:238-251).
  action sign_stark_hash(private_key: KeyText, message_hash: FeltText): bool = all {
    build == RawKeys,
    isKeyText(private_key),
    isUsableKey(private_key),
    isFelt(message_hash),
    host' = { ...host, signatures: host.signatures.union(signEntries(private_key, message_hash)) },
    build' = build,
    krustyLatest' = krustyLatest,
  }

  /// derive_strk20_viewing_key [raw-keys] (lib.rs:257-268 -> kms.rs:266-285).
  action derive_strk20_viewing_key(private_key: KeyText, chain_id: FeltText, pool_address: FeltText): bool = all {
    build == RawKeys,
    isKeyText(private_key),
    isUsableKey(private_key),
    isCanonicalNonZero(chain_id),
    isCanonicalNonZero(pool_address),
    host' = { ...host, viewingKeys: host.viewingKeys.union(viewEntries(private_key, chain_id, pool_address)) },
    build' = build,
    krustyLatest' = krustyLatest,
  }

  /// derive_nostr_public_key (lib.rs:271-285 -> nostr.rs:170-183).
  action derive_nostr_public_key(mnemonic: Mnemonic, address_index: int, account_index: int, passphrase: Passphrase): bool = all {
    checkDerivationInput(mnemonic, passphrase),
    krustyDerives(mnemonic, address_index, account_index),
    host' = { ...host, nostrKeys: host.nostrKeys.union(Set({
      input: { mnemonic: mnemonic, addressIndex: address_index, accountIndex: account_index, passphrase: passphrase },
      publicKey: publicOf(Derived(nostrKey(mnemonic, address_index, account_index, passphrase))),
    })) },
    build' = build,
    krustyLatest' = krustyLatest,
  }

  /// wrap_nostr_application_data (lib.rs:288-312 -> nostr.rs:189-222); the host publishes it.
  action wrap_nostr_application_data(
    mnemonic: Mnemonic, address_index: int, account_index: int, recipient_public_key: RecipientText,
    identifier: str, content: str, created_at: int, passphrase: Passphrase
  ): bool = all {
    isRecipientKey(recipient_public_key),
    isIdentifier(identifier),
    isApplicationData(content),
    checkDerivationInput(mnemonic, passphrase),
    krustyDerives(mnemonic, address_index, account_index),
    created_at >= 0 and created_at <= MAX_SAFE_INTEGER,
    host' = { ...host, relay: host.relay.union(recipientKeys(recipient_public_key).map(pk => Wrapped({
      sender: nostrKey(mnemonic, address_index, account_index, passphrase),
      recipient: pk,
      identifier: identifier,
      content: content,
      createdAt: created_at,
    }))) },
    build' = build,
    krustyLatest' = krustyLatest,
  }

  /// open_nostr_application_data (lib.rs:315-331 -> nostr.rs:226-268).
  action open_nostr_application_data(
    mnemonic: Mnemonic, address_index: int, account_index: int, event_json: Event, passphrase: Passphrase
  ): bool = all {
    isEventGrammar(event_json),
    checkDerivationInput(mnemonic, passphrase),
    krustyDerives(mnemonic, address_index, account_index),
    krustyOpens(nostrKey(mnemonic, address_index, account_index, passphrase), event_json),
    host' = { ...host, opened: host.opened.union(
      openedEntries(nostrKey(mnemonic, address_index, account_index, passphrase), event_json)) },
    build' = build,
    krustyLatest' = krustyLatest,
  }

  /// Environment: a relay or peer alters a published gift wrap's payload.
  action relay_tamper(wrap: GiftWrap, content: str): bool = all {
    host.relay.contains(Wrapped(wrap)),
    host' = { ...host, relay: host.relay.union(Set(Tampered({ ...wrap, content: content }))) },
    build' = build,
    krustyLatest' = krustyLatest,
  }

  action step = any {
    nondet word_count = WORD_COUNTS.oneOf()
    nondet entropy = ENTROPY.oneOf()
    nondet krusty_ok = Set(true, false).oneOf()
    generate_mnemonic(word_count, entropy, krusty_ok),

    // The raw-keys arms are offered in every build: the action's own `build == RawKeys`
    // conjunct is the only thing that keeps a mobile build from reaching them.
    nondet input = DERIVATION_INPUTS.oneOf()
    derive_starknet_keypair(input.mnemonic, input.addressIndex, input.accountIndex, input.passphrase),

    nondet mnemonic = MNEMONICS.oneOf()
    nondet address_index = INDICES.oneOf()
    derive_argent_legacy_keypair(mnemonic, address_index),

    nondet public_key_x = PUBLIC_KEY_TEXTS.oneOf()
    nondet class_hash = CLASS_HASH_TEXTS.oneOf()
    nondet salt = SALTS.oneOf()
    derive_oz_account_address(public_key_x, class_hash, salt),

    nondet network = Set(Mainnet, Sepolia).oneOf()
    oz_account_class_hash(network),

    nondet version = MANIFEST_VERSIONS.oneOf()
    krusty_manifest_bump(version),

    nondet private_key = host.keypairs.map(e => Text(Derived(e.privateKey))).union(LITERAL_KEYS).oneOf()
    nondet message_hash = MESSAGE_HASHES.oneOf()
    sign_stark_hash(private_key, message_hash),

    nondet view_key = host.keypairs.map(e => Text(Derived(e.privateKey))).union(LITERAL_KEYS).oneOf()
    nondet chain_id = CHAIN_IDS.oneOf()
    nondet pool_address = POOLS.oneOf()
    derive_strk20_viewing_key(view_key, chain_id, pool_address),

    nondet input = DERIVATION_INPUTS.oneOf()
    derive_nostr_public_key(input.mnemonic, input.addressIndex, input.accountIndex, input.passphrase),

    nondet recipient = host.nostrKeys.map(e => Hex(e.publicKey)).union(Set(BadHex)).oneOf()
    nondet identifier = IDENTIFIERS.oneOf()
    nondet content = CONTENTS.oneOf()
    all {
      isRecipientKey(recipient),
      isIdentifier(identifier),
      isApplicationData(content),
      nondet created_at = CREATED_AT.oneOf()
      nondet sender = DERIVATION_INPUTS.oneOf()
      wrap_nostr_application_data(sender.mnemonic, sender.addressIndex, sender.accountIndex, recipient,
                                  identifier, content, created_at, sender.passphrase),
    },

    nondet event_json = host.relay.union(Set(Garbled)).oneOf()
    nondet opener = host.nostrKeys.map(e => e.input).union(BAD_OPENERS).oneOf()
    open_nostr_application_data(opener.mnemonic, opener.addressIndex, opener.accountIndex, event_json, opener.passphrase),

    all {
      host.relay.exists(e => isEventGrammar(e)),
      nondet wrap = host.relay.fold(Set(), (acc, e) => acc.union(wrappedOf(e))).union(Set({
        sender: nostrKey({ seed: 1, words: 12, checksum: true, len: 70 }, 0, 0, NoPassphrase),
        recipient: { of: Literal(1) }, identifier: "x", content: "y", createdAt: 1,
      })).oneOf()
      nondet content = CONTENTS.union(Set("forged")).oneOf()
      relay_tamper(wrap, content),
    },
  }

  // ---------------------------------------------------------------------------------------
  // Invariants
  // ---------------------------------------------------------------------------------------

  /// A mobile build (no `raw-keys`) must never hand a raw Stark private key, signature-by-text
  /// or viewing key to the host; otherwise Swift/Kotlin memory holds unwiped raw keys.
  invariant mobile_builds_hold_no_raw_keys: bool =
    build == NoRawKeys implies (
      host.keypairs.size() == 0 and host.signatures.size() == 0 and host.viewingKeys.size() == 0
    )

  /// Each derived Stark key sits at the requested BIP-44 path from the requested phrase and
  /// passphrase; a swapped or dropped index gives the user a key for a different account.
  invariant keypair_path_matches_request: bool =
    host.keypairs.forall(e =>
      e.privateKey.seed == e.input.mnemonic.seed
        and e.privateKey.scheme == e.scheme
        and e.publicKeyX == { of: Derived(e.privateKey) }
        and (if (e.scheme == ArgentLegacy)
               e.privateKey.path == [44, 9004, 0, 0, e.input.addressIndex]
                 and e.privateKey.passphrase == NoPassphrase
             else
               e.privateKey.path == [44, 9004, e.input.accountIndex, 0, e.input.addressIndex]
                 and e.privateKey.passphrase == e.input.passphrase)
        // and the request was a valid one: a non-blank, bounded, checksummed phrase of a
        // supported length, a bounded passphrase and non-hardened indices; otherwise the host
        // holds a key no restore of the backup reproduces.
        and e.input.mnemonic.len > 0 and e.input.mnemonic.len <= MAX_MNEMONIC_TEXT_LENGTH
        and e.input.mnemonic.checksum
        and MNEMONIC_WORD_COUNTS.contains(e.input.mnemonic.words)
        and (match e.input.passphrase {
          | NoPassphrase => true
          | WithPassphrase(p) => p.len <= MAX_BIP39_PASSPHRASE_LENGTH
        })
        and e.input.addressIndex >= 0 and e.input.addressIndex < HARDENED
        and e.input.accountIndex >= 0 and e.input.accountIndex < HARDENED)

  /// Every OZ address commits to the public key, the class hash and the salt the caller gave,
  /// with an omitted salt meaning the public key; a wrong default moves every account address.
  invariant oz_address_uses_effective_salt: bool =
    host.addresses.forall(e =>
      e.publicKeyX != Malformed and e.classHash != Malformed and e.salt != GivenSalt(Malformed)
        and e.address.publicKey == feltValue(e.publicKeyX)
        and e.address.classHash == feltValue(e.classHash)
        and e.address.salt == (match e.salt {
          | OmittedSalt => feltValue(e.publicKeyX)
          | GivenSalt(s) => feltValue(s)
        }))

  /// The class hash Bramble deploys on a network never changes across Krusty manifest bumps;
  /// otherwise every counterfactual address the user already funded moves.
  invariant class_hash_stable_across_manifest_bumps: bool =
    host.classHashes.forall(a => host.classHashes.forall(b =>
      a.network == b.network implies a.hash == b.hash))

  /// A signature names the signer's public key and the parsed message hash; otherwise the
  /// transaction it authorises fails verification or authorises a different hash.
  invariant signature_binds_signer_and_message: bool =
    host.signatures.forall(e =>
      e.hash != Malformed
        and e.signature.publicKey == { of: e.key }
        and e.signature.signer == e.key
        and e.signature.message == feltValue(e.hash))

  /// RFC-6979: the same key over the same hash value always yields the same signature, so a
  /// re-sign never leaks the key through nonce reuse with a different message.
  /// RFC-6979 nonces are defined only for a scalar in (0, n): a zero or out-of-range key must
  /// never have signed.
  invariant signing_is_deterministic: bool =
    host.signatures.forall(a =>
      (match a.key {
        | Derived(k) => true
        | Literal(n) => n > 0 and n < CURVE_ORDER
      })
      and host.signatures.forall(b =>
        (a.key == b.key and feltValue(a.hash) == feltValue(b.hash)) implies a.signature == b.signature))

  /// A STRK20 viewing key is scoped to exactly the requested chain and pool; otherwise the user
  /// reveals notes of a pool they never meant to share.
  invariant viewing_key_scoped_to_request: bool =
    host.viewingKeys.forall(e =>
      e.viewingKey.owner == e.key
        and Canonical(e.viewingKey.chain) == e.chainId
        and Canonical(e.viewingKey.pool) == e.poolAddress)

  /// Opened gift-wrap data is exactly what the sender wrapped, and only the addressed recipient
  /// opened it; otherwise coordination data leaks to, or is forged for, another wallet.
  invariant opened_data_matches_wrap: bool =
    host.opened.forall(o => match o.event {
      | Wrapped(w) =>
          o.data == { content: w.content, identifier: w.identifier, senderPublicKey: { of: Derived(w.sender) } }
            and w.recipient == { of: Derived(o.opener) }
      | Tampered(w) => false
      | Garbled => false
    })

  /// Every phrase generate_mnemonic returned passes validate_mnemonic; otherwise a user writes
  /// down a backup the wallet will refuse to restore.
  invariant generated_phrases_validate: bool =
    host.generated.forall(m =>
      m.checksum and MNEMONIC_WORD_COUNTS.contains(m.words)
        and m.len > 0 and m.len <= MAX_MNEMONIC_TEXT_LENGTH)

  // ---------------------------------------------------------------------------------------
  // Observations
  // ---------------------------------------------------------------------------------------

  /// A fresh phrase reached the host.
  observation phrase_generated: bool =
    host.generated.size() > 0

  /// A Ready (Argent X) legacy key was derived.
  observation argent_legacy_key_derived: bool =
    host.keypairs.exists(e => e.scheme == ArgentLegacy)

  /// The same phrase and indices under two passphrases gave two different Starknet keys.
  observation passphrase_moves_starknet_key: bool =
    host.keypairs.exists(a => host.keypairs.exists(b =>
      a.scheme == StarknetBip44 and b.scheme == StarknetBip44
        and a.input.mnemonic == b.input.mnemonic
        and a.input.addressIndex == b.input.addressIndex
        and a.input.accountIndex == b.input.accountIndex
        and a.input.passphrase != b.input.passphrase
        and a.privateKey != b.privateKey))

  /// An omitted salt gave the same address as passing the public key explicitly.
  observation omitted_salt_matches_explicit_public_key: bool =
    host.addresses.exists(a => host.addresses.exists(b =>
      a.salt == OmittedSalt and b.salt == GivenSalt(Canonical(feltValue(a.publicKeyX)))
        and feltValue(a.publicKeyX) == feltValue(b.publicKeyX)
        and feltValue(a.classHash) == feltValue(b.classHash)
        and a.address == b.address))

  /// An omitted salt gave a different address from Bramble's own OZ account (salt 0x0).
  observation omitted_salt_differs_from_bramble_zero_salt: bool =
    host.addresses.exists(a => host.addresses.exists(b =>
      a.salt == OmittedSalt and b.salt == GivenSalt(Canonical(0))
        and feltValue(a.publicKeyX) == feltValue(b.publicKeyX)
        and feltValue(a.classHash) == feltValue(b.classHash)
        and a.address != b.address))

  /// A class hash was read while Krusty's manifest `latest` was not Bramble's pinned version.
  observation class_hash_read_after_manifest_bump: bool =
    host.classHashes.exists(e => e.manifestLatest != OZ_ACCOUNT_VERSION)

  /// A message hash was signed with a key derived from a phrase.
  observation stark_hash_signed_with_derived_key: bool =
    host.signatures.exists(e => match e.key {
      | Derived(k) => true
      | Literal(n) => false
    })

  /// A STRK20 viewing key reached the host.
  observation viewing_key_derived: bool =
    host.viewingKeys.size() > 0

  /// A mobile build (no raw-keys) derived its Nostr coordination key.
  observation mobile_build_derived_nostr_key: bool =
    build == NoRawKeys and host.nostrKeys.size() > 0

  /// A gift wrap from one wallet to a different wallet was published.
  observation wrap_addressed_to_another_wallet: bool =
    host.relay.exists(e => match e {
      | Wrapped(w) => w.recipient != { of: Derived(w.sender) }
      | Tampered(w) => false
      | Garbled => false
    })

  /// A gift wrap was opened by the wallet it was addressed to.
  observation gift_wrap_opened_by_recipient: bool =
    host.opened.size() > 0
}
