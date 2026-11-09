// Replays the extension parity fixture through the built wasm module, so the shipped artifact is
// what is tested, not only the native library. Run `npm run build` first.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";

const fixture = JSON.parse(
  await readFile(new URL("../../crates/bramble-core/tests/fixtures/extension-parity.json", import.meta.url), "utf8"),
);
const nostr = JSON.parse(
  await readFile(new URL("../../crates/bramble-core/tests/fixtures/nostr-parity.json", import.meta.url), "utf8"),
);
const core = await import("../dist/index.js");
await core.uniffiInitAsync(await readFile(new URL("../dist/generated/bramble_core.wasm", import.meta.url)));

test("core info names the pinned Krusty release", () => {
  assert.equal(core.coreInfo().krustyVersion, "2.0.0");
});

test("accounts and signatures match the extension", () => {
  for (const vector of fixture.accounts) {
    const network = vector.networkId === "SN_MAIN" ? core.NetworkId.Mainnet : core.NetworkId.Sepolia;
    const pair = core.deriveStarknetKeypair(fixture.mnemonic, 0, vector.accountIndex, undefined);
    assert.equal(pair.publicKeyX, vector.publicKeyX);
    const classHash = core.ozAccountClassHash(network);
    assert.equal(classHash, vector.classHash);
    assert.equal(core.deriveOzAccountAddress(pair.publicKeyX, classHash, "0x0"), vector.address);
    const signature = core.signStarkHash(pair.privateKey, fixture.messageHash);
    assert.equal(signature.publicKey, vector.publicKeyX);
    assert.equal(signature.r, vector.signatureR);
    assert.equal(signature.s, vector.signatureS);
  }
});

test("mnemonics generate, validate, and are refused when malformed", () => {
  const phrase = core.generateMnemonic(12);
  assert.equal(phrase.split(" ").length, 12);
  assert.equal(core.validateMnemonic(phrase), true);
  assert.equal(core.validateMnemonic(`${phrase} extra`), false);
  assert.throws(() => core.generateMnemonic(13), (e) => core.CoreError.instanceOf(e));
});

test("nostr identity and gift wraps match the extension", () => {
  for (const vector of nostr.nostrKeys) {
    assert.equal(core.deriveNostrPublicKey(vector.mnemonic, vector.index, 0, undefined), vector.publicKey);
  }
  const mnemonic = nostr.nostrKeys[0].mnemonic;
  const opened = core.openNostrApplicationData(
    mnemonic,
    nostr.giftWrap.recipientNostrIndex,
    0,
    nostr.giftWrap.eventJson,
    undefined,
  );
  assert.equal(opened.content, nostr.giftWrap.content);
  assert.equal(opened.senderPublicKey, nostr.nostrKeys[0].publicKey);
  const wrapped = core.wrapNostrApplicationData(
    mnemonic,
    0,
    0,
    nostr.nostrKeys[1].publicKey,
    "bramble.check",
    "round-trip",
    BigInt(Math.floor(Date.now() / 1000)),
    undefined,
  );
  assert.equal(core.openNostrApplicationData(mnemonic, 1, 0, wrapped, undefined).content, "round-trip");
});
