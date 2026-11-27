//! The extension's endpoint grammar (`core/service-url.ts` `isCanonicalFor`): the secure scheme
//! anywhere, the cleartext one only on a loopback host, no credentials or fragment, spelled
//! exactly as the WHATWG URL parser serialises it. Hand-parsed (no URL crate): a spelling the
//! parser would rewrite is refused, and so are the exotic ones it repairs (IDN hosts, `@`, an
//! empty `#`). Two schemes use it: a Nostr relay (`isNostrRelayUrl`, which is
//! `isWebSocketServiceUrl`) and a custom RPC endpoint (`isCustomRpcUrl`, which is `isServiceUrl`).

use crate::js::num::is_js_whitespace;

/// `LOOPBACK_HOSTS`: the hosts the cleartext scheme is allowed on, as the parser prints them.
const LOOPBACK_HOSTS: [&str; 2] = ["127.0.0.1", "localhost"];
/// `MAX_SERVICE_URL_LENGTH`, measured on the canonical spelling (here the value itself).
const MAX_SERVICE_URL_LENGTH: usize = 2048;

fn is_host_label(label: &str) -> bool {
    !label.is_empty()
        && label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn is_dotted_quad(host: &str) -> bool {
    let parts: Vec<&str> = host.split('.').collect();
    parts.len() == 4
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.len() <= 3
                && part.bytes().all(|b| b.is_ascii_digit())
                && (part.len() == 1 || !part.starts_with('0'))
                && part.parse::<u16>().is_ok_and(|n| n <= 255)
        })
}

// The host as the WHATWG parser would print it back: lowercase names, an IPv4 address in plain
// dotted-quad form, an IPv6 literal in its compressed RFC 5952 form. Any other spelling the
// parser would rewrite is refused.
fn is_host(host: &str) -> bool {
    if let Some(inner) = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        // An embedded IPv4 tail prints differently in the two serializers; refuse it.
        return !inner.contains('.')
            && inner
                .parse::<std::net::Ipv6Addr>()
                .is_ok_and(|address| address.to_string() == inner);
    }
    if host.bytes().any(|b| b.is_ascii_uppercase()) {
        return false;
    }
    let name = host.strip_suffix('.').unwrap_or(host);
    let labels: Vec<&str> = name.split('.').collect();
    // WHATWG's "ends in a number": decimal, or `0x` hex (`0x7f000001` is 127.0.0.1 to the
    // parser and to `inet_aton`), so only a plain dotted quad may end that way.
    let numeric_tail = labels.last().is_some_and(|label| {
        label.bytes().all(|b| b.is_ascii_digit())
            || label
                .strip_prefix("0x")
                .is_some_and(|hex| hex.bytes().all(|b| b.is_ascii_hexdigit()))
    });
    if numeric_tail {
        return is_dotted_quad(name);
    }
    labels.iter().all(|label| is_host_label(label))
}

// Whether the path and query are already in the parser's canonical form: a query follows a path
// (it prints `host/?q`, never `host?q`), no `.` or `..` segment (it resolves them, `%2e` spellings
// included) and no `'` in the query (it percent-encodes it).
fn is_canonical_tail(tail: &str) -> bool {
    if tail.starts_with('?') {
        return false;
    }
    let (path, query) = tail.split_once('?').unwrap_or((tail, ""));
    let dot_segment = path.split('/').any(|segment| {
        let lowered = segment.to_ascii_lowercase().replace("%2e", ".");
        lowered == "." || lowered == ".."
    });
    !dot_segment && !query.contains('\'')
}

/// `isNostrRelayUrl`: `wss://host[:port][/path][?query]`, or `ws://` on `127.0.0.1` or
/// `localhost`, in plain ASCII with no credentials or fragment, spelled exactly as the WHATWG
/// parser prints it (lowercase host, no default port, no leading zeros, no dot segments).
pub fn is_nostr_relay_url(value: &str) -> bool {
    is_canonical_endpoint(value, "wss://", "ws://")
}

/// `isCustomRpcUrl` (`wallet-model/networks/custom-network.ts`, which is `isServiceUrl`): the
/// same grammar over `https://`, or `http://` on `127.0.0.1` or `localhost`. A wallet that may
/// not use cleartext at all narrows it to `https://` itself.
pub fn is_custom_rpc_url(value: &str) -> bool {
    is_canonical_endpoint(value, "https://", "http://")
}

// `isCanonicalFor(value, secure, loopback)`. Both scheme pairs are special in the WHATWG parser,
// so they share one serialisation; only the default ports differ (443 secure, 80 cleartext).
fn is_canonical_endpoint(value: &str, secure: &str, loopback: &str) -> bool {
    if value.is_empty() || value.len() > MAX_SERVICE_URL_LENGTH || !value.is_ascii() {
        return false;
    }
    let (rest, default_port, loopback_only) = if let Some(rest) = value.strip_prefix(secure) {
        (rest, 443, false)
    } else if let Some(rest) = value.strip_prefix(loopback) {
        (rest, 80, true)
    } else {
        return false;
    };
    let split = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(split);
    let (host, port) = match authority.rfind(':').filter(|_| !authority.ends_with(']')) {
        Some(index) => (&authority[..index], Some(&authority[index + 1..])),
        None => (authority, None),
    };
    // The parser drops the scheme's default port and leading zeros.
    let port_ok = port.is_none_or(|port| {
        !port.is_empty()
            && port.len() <= 5
            && port.bytes().all(|b| b.is_ascii_digit())
            && (port == "0" || !port.starts_with('0'))
            && port
                .parse::<u32>()
                .is_ok_and(|n| n <= 65_535 && n != default_port)
    });
    let tail_ok = tail
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@/?%".contains(&b));
    is_host(host)
        && (!loopback_only || LOOPBACK_HOSTS.contains(&host))
        && port_ok
        && tail_ok
        && is_canonical_tail(tail)
}

/// `REDACTED_ENDPOINT`: what an endpoint reads as once `redact_endpoints` has removed it.
pub const REDACTED_ENDPOINT: &str = "[endpoint]";

fn is_scheme_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-')
}

/// The end of an `ENDPOINT_PATTERN` match starting at `start`, an ASCII letter: the rest of a
/// scheme, `:`, two slashes each after any run of backslashes (a JSON-escaped `https:\/\/`), then
/// one or more characters up to whitespace, a quote, an angle bracket or a backtick.
fn endpoint_end(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut at = start + 1;
    while bytes
        .get(at)
        .is_some_and(|b| is_scheme_char(char::from(*b)))
    {
        at += 1;
    }
    for expected in *b"://" {
        while expected == b'/' && bytes.get(at) == Some(&b'\\') {
            at += 1;
        }
        if bytes.get(at) != Some(&expected) {
            return None;
        }
        at += 1;
    }
    let tail = text.get(at..)?;
    let len = tail
        .find(|c| is_js_whitespace(c) || matches!(c, '"' | '<' | '>' | '`'))
        .unwrap_or(tail.len());
    (len > 0).then_some(at + len)
}

/// The extension's `redactEndpoints` (`core/service-url.ts`): replaces every scheme-qualified URL
/// in failure text with [`REDACTED_ENDPOINT`], keeping everything else (the method, an RPC code, a
/// revert reason, felts). A configured endpoint may carry an API key, and failure text can echo
/// it. A match starts only at the front of a word (the pattern's lookbehind), so a failed start
/// rescans nothing and the scan stays linear.
pub fn redact_endpoints(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let (mut copied, mut prev) = (0, None);
    for (at, c) in text.char_indices() {
        if at >= copied && c.is_ascii_alphabetic() && !prev.is_some_and(is_scheme_char) {
            if let Some(end) = endpoint_end(text, at) {
                out.push_str(text.get(copied..at).unwrap_or_default());
                out.push_str(REDACTED_ENDPOINT);
                copied = end;
            }
        }
        prev = Some(c);
    }
    out.push_str(text.get(copied..).unwrap_or_default());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // The extension's `redactEndpoints` cases (`core/tests/service-url.test.ts`).
    #[test]
    fn redacts_endpoints_as_the_extension_does() {
        let e = REDACTED_ENDPOINT;
        for (text, redacted) in [
            (
                "failed at https://rpc.example.test/v0_8/node-key-123?apikey=abc then wss://ws.example.test/k",
                format!("failed at {e} then {e}"),
            ),
            ("Internal error:\nhttps://rpc.example/node-key-123", format!("Internal error:\n{e}")),
            ("at https://rpc.example/key\nnext line", format!("at {e}\nnext line")),
            (
                r#"{"url":"https://rpc.example.test/key","code":-32603}"#,
                r#"{"url":"[endpoint]","code":-32603}"#.into(),
            ),
            ("<a href=https://x.example/key>gateway</a>", "<a href=[endpoint]>gateway</a>".into()),
            (
                r#"upstream error: {"url":"https:\/\/rpc.example\/v1\/node-key-123"}"#,
                format!(r#"upstream error: {{"url":"{e}"}}"#),
            ),
            (r"no endpoint configured: rpc:\/", r"no endpoint configured: rpc:\/".into()),
            ("no endpoint configured: rpc://", "no endpoint configured: rpc://".into()),
            (
                "RPC: starknet_addInvokeTransaction with params {} 55: Account validation failed: 0x0123456789abcdef",
                "RPC: starknet_addInvokeTransaction with params {} 55: Account validation failed: 0x0123456789abcdef".into(),
            ),
            // Mobile's own: the lookbehind, case, a non-ASCII break, and a multi-byte tail.
            ("1https://a.example/k and HTTPS://B.example/k", format!("1https://a.example/k and {e}")),
            ("at https://a.example/k\u{a0}after", format!("at {e}\u{a0}after")),
            ("é https://a.example/ключ`", format!("é {e}`")),
        ] {
            assert_eq!(redact_endpoints(text), redacted, "{text}");
        }
        let nested = r#"{"error":"{\"url\":\"wss:\\/\\/relay.example\\/node-key-123\"}"}"#;
        assert!(!redact_endpoints(nested).contains("node-key-123"));
        let once = redact_endpoints("at https://rpc.example.test/key");
        assert_eq!(redact_endpoints(&once), once);
        for long in [
            "a".repeat(200_000),
            "a\n".repeat(100_000),
            "https:".repeat(50_000),
            format!("https:{}", "\\".repeat(200_000)),
            "https:/\\".repeat(50_000),
        ] {
            assert_eq!(redact_endpoints(&long), long);
        }
    }

    #[test]
    fn relay_urls_are_canonical_secure_or_loopback() {
        for good in [
            "wss://relay.example",
            "wss://relay.example/",
            "wss://relay.example:0",
            "wss://127.0.0.1:7000",
            "wss://[::1]:7000/nostr",
            "wss://[2001:db8::1]",
            "wss://relay.example/a/b?q=Upper%2F",
            "ws://localhost:7000",
            "ws://127.0.0.1",
            "ws://localhost:443",
        ] {
            assert!(is_nostr_relay_url(good), "{good}");
        }
        for bad in [
            "https://relay.example",
            "wss://user:pw@relay.example",
            "wss://relay.example/#x",
            "wss://",
            "wss://relay example",
            "wss://999.1.1.1",
            "wss://relay.example:70000",
            "wss://réseau.example",
            // Spellings the parser would rewrite, so they are not canonical.
            "WSS://relay.example",
            "wss://Relay.Example",
            "wss://relay.example:443",
            "wss://relay.example:0443",
            "wss://relay.example/a/../b",
            "wss://relay.example/./b",
            "wss://relay.example/%2E%2e/b",
            "wss://relay.example/?q='x'",
            "wss://[0:0::1]",
            "wss://[::FFFF]",
            "wss://[::ffff:127.0.0.1]",
            "wss://relay.example?q=1",
            // `ws:` only on the two loopback names, and without its default port.
            "ws://relay.example",
            "ws://[::1]:7000",
            "ws://localhost.",
            "ws://LOCALHOST",
            "ws://localhost:80",
        ] {
            assert!(!is_nostr_relay_url(bad), "{bad}");
        }
    }

    #[test]
    fn rpc_urls_are_canonical_https_or_loopback_http() {
        for good in [
            "https://rpc.example",
            "https://rpc.example/",
            "https://rpc.example/v0_10/abcDEF123",
            "https://rpc.example/rpc?apikey=Secret%2F",
            "https://rpc.example:8443",
            "https://[::1]:5050/rpc",
            "http://127.0.0.1:5050/rpc",
            "http://localhost:5050",
            "http://localhost:443",
        ] {
            assert!(is_custom_rpc_url(good), "{good}");
        }
        let long = format!("https://rpc.example/{}", "a".repeat(2048));
        for bad in [
            "",
            " https://rpc.example",
            "https://rpc.example ",
            "https://rpc.example\n",
            "https://rpc.example/a b",
            "https://rpc.example/\"key\"",
            "https://rpc.example/<k>",
            "https://rpc.example/`k`",
            "https://rpc.example\\rpc",
            "https://user:pw@rpc.example",
            "https://key@rpc.example",
            "https://rpc.example/#frag",
            "https://rpc.example:443",
            "https://RPC.example",
            "HTTPS://rpc.example",
            "https://rpc.example/a/../b",
            "https://rpc.example?q=1",
            "https://rpc.example/?q='x'",
            "https://réseau.example",
            "https://0x7f000001/rpc",
            "https://a.0x7f/rpc",
            "https://rpc.0x",
            "https://",
            "http://rpc.example",
            "http://10.0.0.1:5050",
            "http://[::1]:5050",
            "http://localhost:80",
            "wss://rpc.example",
            "ftp://rpc.example",
            "rpc.example",
            long.as_str(),
        ] {
            assert!(!is_custom_rpc_url(bad), "{bad}");
        }
    }
}
