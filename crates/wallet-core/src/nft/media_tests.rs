//! `tests/collectibles/nft-metadata.test.ts`'s `normalizeImageUrl` and `normalizeAnimationUri`
//! cases, with their values.

use super::*;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;

const URL: usize = 2048;
const DATA: usize = 32_768;

fn btoa(text: &str) -> String {
    STANDARD.encode(text)
}

fn image(value: &str) -> String {
    normalize_image_url(value, URL, DATA)
}

fn animation(value: &str) -> Option<String> {
    normalize_animation_uri(value, DATA).map(|NftAnimation::Html(uri)| uri)
}

#[test]
fn keeps_https_urls_and_rewrites_ipfs_through_the_gateway() {
    assert_eq!(
        image("https://cdn.example/a.png"),
        "https://cdn.example/a.png"
    );
    assert_eq!(
        image("ipfs://bafyCID/1.png"),
        "https://ipfs.io/ipfs/bafyCID/1.png"
    );
    assert_eq!(image("ipfs://ipfs/bafyCID"), "https://ipfs.io/ipfs/bafyCID");
}

#[test]
fn drops_non_https_and_ipfs_references() {
    assert_eq!(image("http://insecure.example/a.png"), "");
    assert_eq!(image("javascript:alert(1)"), "");
    assert_eq!(image("ipfs://"), "");
    assert_eq!(image(""), "");
}

#[test]
fn drops_literal_local_and_private_hosts() {
    for url in [
        "https://localhost/a.png",
        "https://localhost./a.png",
        "https://sub.localhost/a.png",
        "https://printer.local/a.png",
        "https://printer.local./a.png",
        "https://127.0.0.1/a.png",
        "https://127.0.0.1./a.png",
        "https://[::1]/a.png",
        "https://10.1.2.3/a.png",
        "https://172.16.0.1/a.png",
        "https://192.168.1.1/a.png",
        "https://169.254.169.254/a.png",
        "https://224.0.0.1/a.png",
        "https://0.0.0.0/a.png",
    ] {
        assert_eq!(image(url), "", "{url}");
    }
    assert_eq!(
        image("https://172.32.0.1/a.png"),
        "https://172.32.0.1/a.png"
    );
}

#[test]
fn spells_the_url_as_the_parser_prints_it() {
    assert_eq!(
        image("  HTTPS://CDN.Example/A.png "),
        "https://cdn.example/A.png"
    );
    assert_eq!(image("https://cdn.example"), "https://cdn.example/");
    assert_eq!(image("https://cdn.example?x=1"), "https://cdn.example/?x=1");
    assert_eq!(
        image("https://cdn.example:8443/a"),
        "https://cdn.example:8443/a"
    );
    // Credentials, and spellings the parser would rewrite, are dropped rather than repaired.
    assert_eq!(image("https://user:pw@cdn.example/a.png"), "");
    assert_eq!(image("https://cdn.example/a/../b.png"), "");
    assert_eq!(image("https://cdn.example:443/a.png"), "");
    assert_eq!(image("https://2130706433/a.png"), "");
    assert_eq!(image("https://0x7f000001/a.png"), "");
    let long = format!("https://cdn.example/{}", "a".repeat(URL));
    assert_eq!(image(&long), "");
}

#[test]
fn passes_safe_image_data_uris_through_verbatim() {
    let svg = format!(
        "data:image/svg+xml;base64,{}",
        btoa("<svg xmlns='http://www.w3.org/2000/svg'/>")
    );
    assert_eq!(image(&svg), svg);
    assert_eq!(image(&image(&svg)), svg);
    let png = format!("data:image/png;base64,{}", btoa("PNGDATA"));
    assert_eq!(image(&png), png);
    let raw_svg = "data:image/svg+xml,<svg viewBox='0,0,1,1'></svg>";
    assert_eq!(image(raw_svg), raw_svg);
}

#[test]
fn drops_unsafe_or_malformed_data_uris() {
    assert_eq!(image("data:text/html,<h1>hi</h1>"), "");
    assert_eq!(
        image(&format!("data:application/json;base64,{}", btoa("{}"))),
        ""
    );
    assert_eq!(image("data:image/png;base64,AB CD"), "");
    assert_eq!(image("data:image/png,rawbytes"), "");
    assert_eq!(image("data:image/svg+xml;base64,"), "");
    assert_eq!(image("data:image/svg+xml"), "");
    let large = format!("data:image/svg+xml;base64,{}", "A".repeat(33_000));
    assert_eq!(image(&large), "");
}

#[test]
fn passes_safe_html_animations_through_verbatim() {
    let html = format!(
        "data:text/html;base64,{}",
        btoa("<html><body><canvas></canvas></body></html>")
    );
    assert_eq!(animation(&html), Some(html.clone()));
    let raw = "data:text/html,<html><body>hi</body></html>";
    assert_eq!(animation(raw).as_deref(), Some(raw));
    let charset = format!(
        "data:text/html;charset=utf-8;base64,{}",
        btoa("<html></html>")
    );
    assert_eq!(animation(&charset), Some(charset.clone()));
}

#[test]
fn rejects_non_html_remote_malformed_and_oversized_animations() {
    let svg = format!("data:image/svg+xml;base64,{}", btoa("<svg/>"));
    assert_eq!(animation(&svg), None);
    assert_eq!(animation("https://example.com/a.html"), None);
    assert_eq!(animation("data:text/html;base64,"), None);
    assert_eq!(animation("data:text/html;base64,not valid!!"), None);
    assert_eq!(animation("data:text/html"), None);
    let large = format!("data:text/html;base64,{}", "A".repeat(33_000));
    assert_eq!(animation(&large), None);
    assert_eq!(animation(""), None);
}

#[test]
fn clamps_text_in_utf16_units() {
    assert_eq!(clamp_text("  Punk #5  ", 128), "Punk #5");
    assert_eq!(clamp_text(&"x".repeat(200), 128).len(), 128);
}
