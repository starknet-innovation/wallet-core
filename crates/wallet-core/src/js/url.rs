//! `new URL(text)` for an absolute `https` URL, as the WHATWG URL Standard parses and serialises
//! it (`url.href`, `url.hostname`), so a check on the parsed host is a check on the host every
//! client will connect to. Only what an `https` URL with no base needs is here: the scheme,
//! credentials (reported, never kept), the host (a domain or an IPv4 address in any spelling the
//! standard reads as one), the port, and the path, query and fragment with their percent-encode
//! sets and dot segments.
//!
//! Fails closed where the standard needs IDNA: a host that is not ASCII after percent-decoding,
//! or has an `xn--` label, is refused, as is an IPv6 literal (`[...]`).

/// A parsed `https` URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpsUrl {
    /// `url.href`.
    pub href: String,
    /// `url.hostname`: a lowercase domain or a dotted-quad IPv4 address.
    pub hostname: String,
    /// The URL carried a username or a password.
    pub credentials: bool,
}

// C0 controls and everything above `~` (the C0 control percent-encode set).
fn c0(byte: u8) -> bool {
    !(0x20..=0x7e).contains(&byte)
}

fn in_fragment_set(byte: u8) -> bool {
    c0(byte) || matches!(byte, b' ' | b'"' | b'<' | b'>' | b'`')
}

// The special-query percent-encode set.
fn in_query_set(byte: u8) -> bool {
    c0(byte) || matches!(byte, b' ' | b'"' | b'#' | b'<' | b'>' | b'\'')
}

fn in_path_set(byte: u8) -> bool {
    c0(byte)
        || matches!(
            byte,
            b' ' | b'"' | b'#' | b'<' | b'>' | b'?' | b'^' | b'`' | b'{' | b'}'
        )
}

fn encode(text: &str, set: fn(u8) -> bool) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if set(byte) {
            out.push_str(&format!("%{byte:02X}"));
        } else {
            out.push(char::from(byte));
        }
    }
    out
}

fn percent_decode(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        let escaped = bytes
            .get(index + 1..index + 3)
            .filter(|_| byte == b'%')
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .filter(|hex| hex.bytes().all(|b| b.is_ascii_hexdigit()))
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match escaped {
            Some(decoded) => {
                out.push(decoded);
                index += 3;
            }
            None => {
                out.push(byte);
                index += 1;
            }
        }
    }
    out
}

/// The IPv4 number parser: `None` is failure, `Some(None)` a valid number too large for any
/// address, `Some(Some(n))` the number.
fn ipv4_number(text: &str) -> Option<Option<u64>> {
    if text.is_empty() {
        return None;
    }
    let (digits, radix) = if text.len() >= 2 && (text.starts_with("0x") || text.starts_with("0X")) {
        (&text[2..], 16)
    } else if text.len() >= 2 && text.starts_with('0') {
        (&text[1..], 8)
    } else {
        (text, 10)
    };
    if digits.is_empty() {
        return Some(Some(0));
    }
    if !digits.chars().all(|c| c.is_digit(radix)) {
        return None;
    }
    Some(u64::from_str_radix(digits, radix).ok())
}

/// The "ends in a number" checker.
fn ends_in_a_number(host: &str) -> bool {
    let mut parts: Vec<&str> = host.split('.').collect();
    if parts.last() == Some(&"") && parts.len() > 1 {
        parts.pop();
    }
    let last = parts.last().copied().unwrap_or_default();
    (!last.is_empty() && last.bytes().all(|b| b.is_ascii_digit())) || ipv4_number(last).is_some()
}

/// The IPv4 parser, serialised; `None` is failure.
fn ipv4(host: &str) -> Option<String> {
    let mut parts: Vec<&str> = host.split('.').collect();
    if parts.last() == Some(&"") && parts.len() > 1 {
        parts.pop();
    }
    if parts.len() > 4 {
        return None;
    }
    let numbers: Vec<u64> = parts
        .iter()
        .map(|part| ipv4_number(part).flatten())
        .collect::<Option<_>>()?;
    let (last, rest) = numbers.split_last()?;
    if rest.iter().any(|n| *n > 255) || *last >= 256u64.pow(5 - numbers.len() as u32) {
        return None;
    }
    let mut address = *last;
    for (index, n) in rest.iter().enumerate() {
        address += n * 256u64.pow(3 - index as u32);
    }
    let octets = (address as u32).to_be_bytes();
    Some(format!(
        "{}.{}.{}.{}",
        octets[0], octets[1], octets[2], octets[3]
    ))
}

// The forbidden domain code points.
fn forbidden_in_domain(byte: u8) -> bool {
    byte < 0x20
        || byte == 0x7f
        || matches!(
            byte,
            b' ' | b'#'
                | b'%'
                | b'/'
                | b':'
                | b'<'
                | b'>'
                | b'?'
                | b'@'
                | b'['
                | b'\\'
                | b']'
                | b'^'
                | b'|'
        )
}

/// The host parser for a special URL, ASCII only (see the module note).
fn host(text: &str) -> Option<String> {
    if text.is_empty() || text.starts_with('[') {
        return None;
    }
    let decoded = String::from_utf8(percent_decode(text)).ok()?;
    if !decoded.is_ascii() {
        return None;
    }
    let domain = decoded.to_ascii_lowercase();
    if domain.is_empty()
        || domain.bytes().any(forbidden_in_domain)
        || domain.split('.').any(|label| label.starts_with("xn--"))
    {
        return None;
    }
    if ends_in_a_number(&domain) {
        return ipv4(&domain);
    }
    Some(domain)
}

fn is_single_dot(segment: &str) -> bool {
    segment == "." || segment.eq_ignore_ascii_case("%2e")
}

fn is_double_dot(segment: &str) -> bool {
    matches!(
        segment.to_ascii_lowercase().as_str(),
        ".." | ".%2e" | "%2e." | "%2e%2e"
    )
}

/// The path state over a special URL's path (`\` is `/`), serialised.
fn path(text: &str) -> String {
    let text = text.strip_prefix(['/', '\\']).unwrap_or(text);
    let raw: Vec<&str> = text.split(['/', '\\']).collect();
    let mut segments: Vec<String> = Vec::new();
    for (index, raw_segment) in raw.iter().enumerate() {
        let segment = encode(raw_segment, in_path_set);
        let last = index + 1 == raw.len();
        if is_double_dot(&segment) {
            segments.pop();
            if last {
                segments.push(String::new());
            }
        } else if is_single_dot(&segment) {
            if last {
                segments.push(String::new());
            }
        } else {
            segments.push(segment);
        }
    }
    format!("/{}", segments.join("/"))
}

/// `new URL(text)` when `text` is an absolute `https` URL; `None` where it throws, for any
/// other scheme, and where the module note refuses.
pub fn parse_https(text: &str) -> Option<HttpsUrl> {
    // Leading and trailing C0 controls and spaces are stripped, tabs and newlines removed.
    let text: String = text
        .trim_matches(|c: char| c <= ' ')
        .chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .collect();
    let rest = text
        .get(..6)
        .filter(|scheme| scheme.eq_ignore_ascii_case("https:"))
        .and_then(|_| text.get(6..))?;
    // The special authority states ignore any run of slashes and backslashes.
    let rest = rest.trim_start_matches(['/', '\\']);
    let end = rest.find(['/', '\\', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    let (userinfo, host_port) = match authority.rfind('@') {
        Some(at) => (&authority[..at], &authority[at + 1..]),
        None => ("", authority),
    };
    let (host_text, port_text) = match host_port.find(':') {
        Some(colon) => (&host_port[..colon], Some(&host_port[colon + 1..])),
        None => (host_port, None),
    };
    let hostname = host(host_text)?;
    let port = match port_text {
        None | Some("") => None,
        Some(digits) if digits.bytes().all(|b| b.is_ascii_digit()) => {
            let digits = digits.trim_start_matches('0');
            if digits.len() > 5 {
                return None;
            }
            let port: u32 = if digits.is_empty() {
                0
            } else {
                digits.parse().ok()?
            };
            if port > 65_535 {
                return None;
            }
            (port != 443).then_some(port)
        }
        Some(_) => return None,
    };
    let (before_fragment, fragment) = match tail.find('#') {
        Some(hash) => (&tail[..hash], Some(&tail[hash + 1..])),
        None => (tail, None),
    };
    let (path_text, query) = match before_fragment.find('?') {
        Some(mark) => (&before_fragment[..mark], Some(&before_fragment[mark + 1..])),
        None => (before_fragment, None),
    };
    let mut href = format!("https://{hostname}");
    if let Some(port) = port {
        href.push_str(&format!(":{port}"));
    }
    href.push_str(&path(path_text));
    if let Some(query) = query {
        href.push('?');
        href.push_str(&encode(query, in_query_set));
    }
    if let Some(fragment) = fragment {
        href.push('#');
        href.push_str(&encode(fragment, in_fragment_set));
    }
    Some(HttpsUrl {
        href,
        hostname,
        credentials: !userinfo.is_empty() && userinfo != ":",
    })
}

#[cfg(test)]
#[path = "url_tests.rs"]
mod tests;
