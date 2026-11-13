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
        let before = signatures.len();
        if let Some(declaration) = declared_fn(lines[head]) {
            signatures.push(Exported {
                signature: signature(declaration, &lines[head..]),
                foreign: false,
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
