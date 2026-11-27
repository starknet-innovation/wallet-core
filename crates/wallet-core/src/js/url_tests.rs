use super::*;

fn href(text: &str) -> Option<String> {
    parse_https(text).map(|url| url.href)
}

fn hostname(text: &str) -> Option<String> {
    parse_https(text).map(|url| url.hostname)
}

#[test]
fn serialises_as_new_url_href_does() {
    for (input, expected) in [
        ("https://cdn.example", "https://cdn.example/"),
        ("HTTPS://CDN.Example?x=1", "https://cdn.example/?x=1"),
        (
            "https://cdn.example/My Image.png",
            "https://cdn.example/My%20Image.png",
        ),
        (
            "https://cdn.example/{id}.png",
            "https://cdn.example/%7Bid%7D.png",
        ),
        ("https://cdn.example/é", "https://cdn.example/%C3%A9"),
        ("https://cdn.example/a%20b", "https://cdn.example/a%20b"),
        (
            "https://cdn.example/?q='x' y",
            "https://cdn.example/?q=%27x%27%20y",
        ),
        (
            "https://cdn.example/a#frag `x`",
            "https://cdn.example/a#frag%20%60x%60",
        ),
        ("https://cdn.example/a?#", "https://cdn.example/a?#"),
        ("https://cdn.example:443/a", "https://cdn.example/a"),
        ("https://cdn.example:0443/a", "https://cdn.example/a"),
        ("https://cdn.example:/a", "https://cdn.example/a"),
        ("https://cdn.example:8443/a", "https://cdn.example:8443/a"),
        (
            "https://cdn.example/a/../b.png",
            "https://cdn.example/b.png",
        ),
        ("https://cdn.example/a/./b", "https://cdn.example/a/b"),
        ("https://cdn.example/a/%2E%2e/b", "https://cdn.example/b"),
        ("https://cdn.example/a/..", "https://cdn.example/"),
        ("https://cdn.example/a/.", "https://cdn.example/a/"),
        ("https://cdn.example/../../a", "https://cdn.example/a"),
        ("https://cdn.example\\a\\b", "https://cdn.example/a/b"),
        ("https:cdn.example/a", "https://cdn.example/a"),
        ("https:///cdn.example/a", "https://cdn.example/a"),
        (
            " \u{1} https://cdn.ex\nample/a\t \u{1f}",
            "https://cdn.example/a",
        ),
        ("https://@cdn.example/", "https://cdn.example/"),
        ("https://%41.example/", "https://a.example/"),
        ("https://cdn.example./a", "https://cdn.example./a"),
    ] {
        assert_eq!(href(input).as_deref(), Some(expected), "{input}");
    }
}

#[test]
fn reads_every_ipv4_spelling_the_standard_reads() {
    for (input, expected) in [
        ("https://0x7f000001/", "127.0.0.1"),
        ("https://127.1/", "127.0.0.1"),
        ("https://2130706433/", "127.0.0.1"),
        ("https://0177.0.0.1/", "127.0.0.1"),
        ("https://0x7F.0.0.1/", "127.0.0.1"),
        ("https://10.0.0.1./", "10.0.0.1"),
        ("https://192.168.0x1.1/", "192.168.1.1"),
        ("https://0x/", "0.0.0.0"),
        ("https://8.8.8.8/", "8.8.8.8"),
    ] {
        assert_eq!(hostname(input).as_deref(), Some(expected), "{input}");
    }
    // Ends in a number but is no address: the URL does not parse.
    for input in [
        "https://1.2.3.4.5/",
        "https://256.0.0.1/",
        "https://1.2.3.256/x",
        "https://a.0x/",
        "https://a.08/",
        "https://4294967296/",
        "https://example.0xffffffffffffffffffffffff/",
    ] {
        assert_eq!(href(input), None, "{input}");
    }
}

#[test]
fn reports_credentials() {
    assert!(
        parse_https("https://user:pw@cdn.example/")
            .unwrap()
            .credentials
    );
    assert!(
        parse_https("https://user@cdn.example/")
            .unwrap()
            .credentials
    );
    assert!(parse_https("https://:pw@cdn.example/").unwrap().credentials);
    assert!(!parse_https("https://:@cdn.example/").unwrap().credentials);
    assert_eq!(
        hostname("https://a@b@cdn.example/").as_deref(),
        Some("cdn.example")
    );
}

#[test]
fn refuses_what_throws_and_what_needs_idna() {
    for input in [
        "",
        "https://",
        "https:///",
        "https://?q",
        "https://:443/",
        "https://user@/",
        "http://cdn.example/",
        "ipfs://cid",
        "https://cdn.example:65536/",
        "https://cdn.example:8a/",
        "https://a b.example/",
        "https://a%25b.example/",
        "https://a^b.example/",
        "https://[::1]/",
        "https://ä.example/",
        "https://xn--nxasmq6b.example/",
    ] {
        assert_eq!(href(input), None, "{input:?}");
    }
}

#[test]
fn an_href_parses_to_itself() {
    for input in [
        "https://cdn.example/My Image.png?q='x'#f g",
        "https://0x7f000001:0443/a/../{b}",
        "https:\\\\CDN.example\\é",
    ] {
        let once = href(input).unwrap();
        assert_eq!(href(&once).as_deref(), Some(once.as_str()), "{input}");
    }
}
