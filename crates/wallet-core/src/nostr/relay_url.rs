//! `isNostrRelayUrl`, which is the extension's `isWebSocketServiceUrl` (`core/service-url.ts`):
//! `wss:` anywhere, `ws:` only on a loopback host, no credentials or fragment, spelled exactly as
//! the WHATWG URL parser serialises it. Hand-parsed (no URL crate): a spelling the parser would
//! rewrite is refused, and so are the exotic ones it repairs (IDN hosts, `@`, an empty `#`).

/// `LOOPBACK_HOSTS`: the hosts `ws:` is allowed on, as the parser prints them.
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
    let numeric_tail = labels
        .last()
        .is_some_and(|label| label.bytes().all(|b| b.is_ascii_digit()));
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
    if value.is_empty() || value.len() > MAX_SERVICE_URL_LENGTH || !value.is_ascii() {
        return false;
    }
    let (rest, default_port, loopback_only) = if let Some(rest) = value.strip_prefix("wss://") {
        (rest, 443, false)
    } else if let Some(rest) = value.strip_prefix("ws://") {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
