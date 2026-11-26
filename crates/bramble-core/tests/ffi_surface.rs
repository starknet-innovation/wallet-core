//! The FFI surface never hands secret-shaped bytes to the apps. Scans every exported function in
//! every file under `src/` and fails when one returns raw bytes (or, for a trait the apps
//! implement, takes them), so a future method cannot hand the DEK, a wrap secret or vault bytes to
//! Swift or Kotlin without this test changing.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

/// Every source file under `src/`, read at test time so a new exporting file is scanned without
/// this test changing.
fn sources() -> Vec<(String, String)> {
    fn walk(dir: &std::path::Path, root: &std::path::Path, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).expect("read src") {
            let path = entry.expect("read src entry").path();
            if path.is_dir() {
                walk(&path, root, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let name = path.strip_prefix(root).unwrap().display().to_string();
                out.push((name, std::fs::read_to_string(&path).expect("read source")));
            }
        }
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut out = Vec::new();
    walk(&root, &root, &mut out);
    out
}

/// `#[uniffi::export]`, written plainly or under `cfg_attr(feature = "mobile", ..)` (the catalog
/// exports shared with the extension's build); a mention in a comment or path is not one.
fn is_export_attribute(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with("#[uniffi::export")
        || (line.starts_with("#[cfg_attr(") && line.contains("uniffi::export)"))
}

/// One exported function, its signature up to the body (or the `;` of a trait method).
struct Exported {
    signature: String,
    /// A method of an exported `with_foreign` trait: the apps implement it and the core calls it,
    /// so what the core hands over is the arguments, not the return value.
    foreign: bool,
    /// Gated on `raw-keys` or `extension`, which mobile's build leaves out.
    extension_only: bool,
}

impl Exported {
    /// The part of the signature that carries values out to Swift or Kotlin.
    fn outgoing(&self) -> &str {
        let mut halves = self.signature.splitn(2, "->");
        let arguments = halves.next().unwrap_or("");
        if self.foreign {
            arguments
        } else {
            halves.next().unwrap_or("")
        }
    }

    fn name(&self) -> &str {
        let after = self.signature.split("fn ").nth(1).unwrap_or("");
        after.split('(').next().unwrap_or("")
    }
}

/// The declaration from `fn` on, when `line` declares a function: a visibility and `async`, `const`
/// or `unsafe` before it are skipped, since uniffi exports every fn of an exported impl.
fn declared_fn(line: &str) -> Option<&str> {
    let mut rest = line;
    loop {
        if rest.starts_with("fn ") {
            return Some(rest);
        }
        rest = [
            "pub(crate) ",
            "pub(super) ",
            "pub ",
            "async ",
            "const ",
            "unsafe ",
        ]
        .iter()
        .find_map(|qualifier| rest.strip_prefix(qualifier))?;
    }
}

/// The signature starting at `lines[0]` (from `first`), up to its body or the `;` that ends it.
fn signature(first: &str, lines: &[&str]) -> String {
    let mut text = String::new();
    for line in std::iter::once(first).chain(lines.iter().skip(1).copied()) {
        if let Some(body) = line.find('{') {
            text.push_str(&line[..body]);
            break;
        }
        text.push_str(line);
        text.push(' ');
        if line.trim_end().ends_with(';') {
            break;
        }
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Functions inside `#[uniffi::export]` items: a free fn, every fn of an impl block, and the
/// methods of a trait. Panics on an export it reads no function under, so a shape this parser
/// does not know is a failure, never a clean item.
fn exported_signatures(source: &str) -> Vec<Exported> {
    let lines: Vec<&str> = source.lines().collect();
    let mut signatures = Vec::new();
    for (at, line) in lines.iter().enumerate() {
        if !is_export_attribute(line) {
            continue;
        }
        // The item itself, past any further attributes and comments on it.
        let Some(head) = (at + 1..lines.len()).find(|&i| {
            let line = lines[i].trim_start();
            !line.starts_with("#[") && !line.starts_with("//")
        }) else {
            panic!("an export attribute ends the file");
        };
        // A builtin trait exported on a type (`#[uniffi::export(Display)]` on a refusal) hands
        // out only that impl's text, which the no-secret rule already bounds: no function here.
        if line.trim_start().starts_with("#[uniffi::export(Display)]")
            && (lines[head].starts_with("pub enum ") || lines[head].starts_with("pub struct "))
        {
            continue;
        }
        let before = signatures.len();
        let extension_only = lines[..head]
            .iter()
            .rev()
            .take_while(|line| {
                let line = line.trim_start();
                line.starts_with("#[") || line.starts_with("//")
            })
            .any(|line| {
                let line = line.trim_start();
                line.starts_with("#[cfg(feature = \"raw-keys\")]")
                    || line.starts_with("#[cfg(feature = \"extension\")]")
            });
        if let Some(declaration) = declared_fn(lines[head]) {
            signatures.push(Exported {
                signature: signature(declaration, &lines[head..]),
                foreign: false,
                extension_only,
            });
        } else if lines[head].starts_with("impl") || lines[head].starts_with("pub trait") {
            let foreign = lines[head].starts_with("pub trait");
            // Items run to the closing brace at column zero; methods sit at one indent.
            for i in head + 1..lines.len() {
                if lines[i] == "}" {
                    break;
                }
                let method = lines[i]
                    .strip_prefix("    ")
                    .filter(|rest| !rest.starts_with(' '))
                    .and_then(declared_fn);
                if let Some(declaration) = method {
                    signatures.push(Exported {
                        signature: signature(declaration, &lines[i..]),
                        foreign,
                        extension_only,
                    });
                }
            }
        }
        assert!(
            signatures.len() > before,
            "the scan read no function under `{}`",
            lines[head]
        );
    }
    signatures
}

#[test]
fn no_exported_function_returns_bytes() {
    let mut seen = 0;
    for (file, source) in sources() {
        for exported in exported_signatures(&source) {
            seen += 1;
            let outgoing = exported.outgoing();
            assert!(
                !outgoing.contains("u8>") && !outgoing.contains("[u8"),
                "{file}: `{}` hands bytes across the FFI",
                exported.signature
            );
        }
    }
    assert!(seen >= 200, "the scan found only {seen} exported functions");
}

#[test]
fn the_wrap_secret_only_flows_into_the_core() {
    let biometric = include_str!("../src/biometric.rs");
    for exported in exported_signatures(biometric) {
        if exported.signature.contains("wrap_secret") {
            assert!(
                !exported.foreign && exported.outgoing().contains("Result<(), CoreError>"),
                "{}",
                exported.signature
            );
        }
    }
}

/// Signing is by account record only (`docs/accounts.md` §2.7): no exported function signs, plans
/// a transfer or deployment, or reads activity by account index, so the retired `sign_hash(index)`
/// cannot come back as a path the apps could reach.
#[test]
fn nothing_exported_signs_or_plans_by_account_index() {
    let mut signers = 0;
    for (file, source) in sources() {
        // A foreign trait is the core calling the apps, not a path the apps could reach.
        for exported in exported_signatures(&source).iter().filter(|e| !e.foreign) {
            let (name, signature) = (exported.name(), &exported.signature);
            assert_ne!(name, "sign_hash", "{file}: the index shim is retired");
            let acts =
                name.contains("sign") || name.starts_with("prepare_") || name.ends_with("activity");
            if acts {
                signers += 1;
                assert!(
                    !signature.contains("account_index"),
                    "{file}: `{signature}` takes an account index"
                );
            }
        }
    }
    assert!(
        signers >= 5,
        "found only {signers} signing or planning functions"
    );
}

/// AGENTS.md: mobile's build takes a raw private key in exactly one export, `import_private_key`,
/// which seals it and returns only the public record, and returns one from none. The extension's
/// key exports (`raw-keys`, `extension`) are not compiled into it, and its key records live only
/// in `lib.rs` behind `raw-keys`. A tripwire on the names a key goes by, beside the
/// `compile_error!` that refuses `mobile` with `raw-keys`.
#[test]
fn a_private_key_enters_mobile_only_through_its_import_and_never_leaves() {
    let mut takers = Vec::new();
    for (file, source) in sources() {
        if file != "lib.rs" {
            assert!(
                !source.contains("pub private_key"),
                "{file}: a record outside the raw-keys exports carries a private key"
            );
        }
        for exported in exported_signatures(&source)
            .iter()
            .filter(|e| !e.extension_only)
        {
            let signature = &exported.signature;
            assert!(
                !exported.outgoing().contains("private_key")
                    && !exported.outgoing().contains("SecretFelt"),
                "{file}: `{signature}` hands a key out"
            );
            if signature.contains("private_key") || signature.contains("SecretFelt") {
                takers.push(format!("{file}: {}", exported.name()));
            }
        }
    }
    assert_eq!(takers, ["key_import.rs: import_private_key"]);
    let import = exported_signatures(include_str!("../src/key_import.rs"));
    assert_eq!(import.len(), 1);
    assert!(import[0]
        .outgoing()
        .contains("Result<AccountRecord, CoreError>"));
}

/// AGENTS.md: mobile returns nothing a key can be recovered from, and a raw signature over a hash
/// the host picks is such a thing: an account's STRK20 viewing key is one deterministic signature
/// over a public pool scope hash. So no mobile export hands out a `StarkSignature`; signatures
/// stay inside the plans that made them. Only the extension's `raw-keys` `sign_stark_hash` does.
#[test]
fn mobile_hands_out_no_raw_signature() {
    let mut extension = Vec::new();
    for (file, source) in sources() {
        for exported in exported_signatures(&source) {
            if !exported.outgoing().contains("StarkSignature") {
                continue;
            }
            assert!(
                exported.extension_only,
                "{file}: `{}` is a signing oracle on mobile's surface",
                exported.signature
            );
            extension.push(exported.name().to_string());
        }
    }
    assert_eq!(extension, ["sign_stark_hash"]);
}

/// P1's multisig transaction exports sign by vault record and judge reads the host drove: none
/// takes or returns a key, a phrase or a raw signature, so a co-signer's key never crosses the
/// FFI (AGENTS.md). Their files are named so a new export there is scanned here.
#[test]
fn multisig_transactions_take_and_return_no_key() {
    let mut seen = Vec::new();
    for (file, source) in sources() {
        if !file.starts_with("multisig_tx")
            && file != "multisig_rpc.rs"
            && file != "multisig_guard.rs"
        {
            continue;
        }
        for exported in exported_signatures(&source) {
            let signature = exported.signature.to_lowercase();
            for word in [
                "private",
                "secret",
                "mnemonic",
                "phrase",
                "passphrase",
                "signature:",
            ] {
                assert!(
                    !signature.contains(word),
                    "{file}: `{}` carries `{word}`",
                    exported.signature
                );
            }
            seen.push(exported.name().to_string());
        }
    }
    for name in [
        "prepare_multisig_deployment",
        "propose_multisig_transaction",
        "sign_multisig_intent",
        "sign_multisig_intent_on_device",
        "submit_multisig_intent",
        "import_multisig_intent",
        "multisig_intent_read",
        "multisig_intent_review",
    ] {
        assert!(
            seen.iter().any(|seen| seen == name),
            "{name} is not exported"
        );
    }
}

/// P14's gasless exports start from a reviewed `TransferPlan` and sign only the outside execution
/// the core checked: none takes a call, a hash, a signature or typed data from the host, and none
/// hands a key back. The one signature that leaves is the account's over the checked execution's
/// hash, inside the execute request's body the host posts to the relay as described.
#[test]
fn gasless_takes_no_calls_hash_or_signature_and_returns_none() {
    let source = [
        include_str!("../src/send/gasless.rs"),
        include_str!("../src/send/gasless_offers.rs"),
        include_str!("../src/send/gasless_sign.rs"),
    ];
    let exported: Vec<Exported> = source.iter().flat_map(|s| exported_signatures(s)).collect();
    for exported in &exported {
        let signature = exported.signature.to_lowercase();
        for word in [
            "private",
            "secret",
            "mnemonic",
            "phrase",
            "signature",
            "hash:",
            "typed",
            "felt",
            "call",
        ] {
            assert!(
                !signature.contains(word),
                "`{}` carries `{word}`",
                exported.signature
            );
        }
    }
    let names: Vec<&str> = exported.iter().map(Exported::name).collect();
    assert_eq!(
        names,
        [
            "accept_offer_reply",
            "choose",
            "resume",
            "accept_submit_reply",
            "gasless_offers",
            "advance_device"
        ]
    );
}

/// The records that carry a call's parts (a `calldata` or `entrypoint` field), by name, from every
/// source file: what an export would have to take to be handed calls to sign.
fn call_shaped_records(sources: &[(String, String)]) -> Vec<String> {
    let mut names = vec!["Call".to_string()];
    for (_, source) in sources {
        let lines: Vec<&str> = source.lines().collect();
        for (at, line) in lines.iter().enumerate() {
            let Some(rest) = line.trim_start().strip_prefix("pub struct ") else {
                continue;
            };
            let name = rest
                .split(|c: char| !c.is_alphanumeric() && c != '_')
                .next()
                .unwrap_or("");
            let body = lines[at..].iter().take_while(|line| **line != "}");
            if body
                .clone()
                .any(|line| line.contains("calldata") || line.contains("entrypoint"))
            {
                names.push(name.to_string());
            }
        }
    }
    names
}

/// P9: no export signs calls the host supplies. A batch is built in the core by its reviewed
/// consumer (the revoke takes an allowance id, never a call), so no exported function the apps
/// call takes calldata, an entrypoint, or a record carrying either.
#[test]
fn no_export_takes_calls_to_sign() {
    let sources = sources();
    let call_shaped = call_shaped_records(&sources);
    assert!(
        call_shaped.iter().any(|name| name == "ReviewedCall"),
        "the scan found no call-shaped record: {call_shaped:?}"
    );
    for (file, source) in &sources {
        for exported in exported_signatures(source).iter().filter(|e| !e.foreign) {
            let incoming = exported.signature.split("->").next().unwrap_or("");
            let arguments = incoming.split_once('(').map_or("", |(_, rest)| rest);
            for word in arguments.split(|c: char| !c.is_alphanumeric() && c != '_') {
                assert!(
                    word != "calldata"
                        && word != "entrypoint"
                        && !call_shaped.iter().any(|name| name == word),
                    "{file}: `{}` takes calls",
                    exported.signature
                );
            }
        }
    }
}

/// P16: a site's requests reach the wallet only through `DappSession`. The one export that signs,
/// `approve_sign_typed_data`, takes a queued card's id (and the account the person reviewed),
/// never a hash or a document: what it signs is only ever what the site queued and the person
/// approved. Each new signing export must keep that shape; widen this list deliberately.
#[test]
fn the_dapp_session_signs_only_an_approved_card() {
    let names: Vec<String> = [
        include_str!("../src/dapp_session.rs"),
        include_str!("../src/dapp_sign.rs"),
        include_str!("../src/dapp_sites.rs"),
    ]
    .into_iter()
    .flat_map(exported_signatures)
    .map(|exported| exported.name().to_owned())
    .collect();
    assert_eq!(
        names,
        [
            "new",
            "request",
            "pending_approvals",
            "approve_connect",
            "deny",
            "settle_expired",
            "approve_sign_typed_data",
            "refresh_phishing_feed",
            "answer_phishing_feed"
        ]
    );
    let signing = exported_signatures(include_str!("../src/dapp_sign.rs"));
    assert_eq!(
        signing
            .iter()
            .map(|exported| exported.signature.as_str())
            .collect::<Vec<_>>(),
        ["fn approve_sign_typed_data( &self, approval_id: String, \
             reviewed_account_id: String, now_ms: f64, ) -> Vec<DappSettled>"]
    );
}
