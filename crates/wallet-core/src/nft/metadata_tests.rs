//! `tests/collectibles/nft-token-metadata.test.ts`, with its values; the fetch is the host's, so
//! its cases check the request built and the judge of the reply.

use super::*;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;

const LIMITS: NftMetadataLimits = NftMetadataLimits {
    uri_bytes: 32_768,
    data_uri_bytes: 32_768,
    document_bytes: 65_536,
    fetch_deadline_ms: 10_000,
    url_length: 2048,
    art_data_uri_length: 32_768,
    attributes: 32,
    text_length: 128,
    description_length: 1_024,
    attribute_value_length: 256,
    display_type_length: 64,
};

const METADATA: &str = r#"{"attributes":[{"trait_type":"Catalog","value":"CryptoPunks"},{"display_type":"number","trait_type":"Punk ID","value":0}],"description":"A test collectible","external_url":"https://example.com/punks/0","image":"ipfs://bafybeigdyrzt/image.png","name":"Starknet Test Punk #0"}"#;

fn btoa(text: &str) -> String {
    STANDARD.encode(text)
}

// `encodeURIComponent`.
fn encode_uri_component(text: &str) -> String {
    text.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

fn resolved(uri: &str) -> Option<NftTokenMetadata> {
    match resolve_metadata_uri(uri, &LIMITS) {
        MetadataSource::Resolved(metadata) => metadata,
        MetadataSource::Fetch(request) => panic!("data URIs must not fetch: {request:?}"),
    }
}

fn attribute(display_type: Option<&str>, trait_type: &str, value: &str) -> NftAttribute {
    NftAttribute {
        display_type: display_type.map(str::to_owned),
        trait_type: trait_type.into(),
        value: value.into(),
    }
}

#[test]
fn reads_the_already_minted_percent_encoded_format() {
    let uri = format!("data:application/json,{}", encode_uri_component(METADATA));
    assert_eq!(
        resolved(&uri),
        Some(NftTokenMetadata {
            name: "Starknet Test Punk #0".into(),
            collection: String::new(),
            description: "A test collectible".into(),
            image: "https://ipfs.io/ipfs/bafybeigdyrzt/image.png".into(),
            animation: None,
            external_url: "https://example.com/punks/0".into(),
            attributes: vec![
                attribute(None, "Catalog", "CryptoPunks"),
                attribute(Some("number"), "Punk ID", "0"),
            ],
        })
    );
}

#[test]
fn reads_base64_json_data_uris() {
    let document = format!(r#"{{"collection":{{"name":"MC Punks"}},{}"#, &METADATA[1..]);
    let metadata = resolved(&format!("data:application/json;base64,{}", btoa(&document))).unwrap();
    assert_eq!(metadata.collection, "MC Punks");
    assert_eq!(metadata.name, "Starknet Test Punk #0");
}

#[test]
fn preserves_a_fully_on_chain_svg_image() {
    let image = format!(
        "data:image/svg+xml;base64,{}",
        btoa("<svg xmlns='http://www.w3.org/2000/svg' width='150' height='150'/>")
    );
    let document = format!(
        r#"{{"attributes":[{{"trait_type":"Status","value":"Alive"}},{{"trait_type":"Kind","value":"Loop"}},{{"trait_type":"Sequence Length","value":3}},{{"trait_type":"Age","value":7}}],"description":"An autonomous Conway's Game of Life lifeform living forever on Starknet.","image":"{image}","name":"Lifeform #1"}}"#
    );
    let metadata = resolved(&format!("data:application/json;base64,{}", btoa(&document))).unwrap();
    assert_eq!(metadata.name, "Lifeform #1");
    assert_eq!(metadata.image, image);
    assert_eq!(
        metadata.attributes,
        vec![
            attribute(None, "Status", "Alive"),
            attribute(None, "Kind", "Loop"),
            attribute(None, "Sequence Length", "3"),
            attribute(None, "Age", "7"),
        ]
    );
}

#[test]
fn keeps_a_fully_on_chain_html_animation_marked_as_html() {
    let animation = format!(
        "data:text/html;base64,{}",
        btoa("<!doctype html><html><body><canvas id='c'></canvas><script>/* conway */</script></body></html>")
    );
    let document = format!(
        r#"{{"animation_url":"{animation}","attributes":[{{"trait_type":"Status","value":"Alive"}},{{"trait_type":"Kind","value":"Loop"}}],"description":"An autonomous Conway's Game of Life lifeform living forever on Starknet.","name":"Lifeform 2924"}}"#
    );
    let uri = format!("data:application/json,{}", encode_uri_component(&document));
    let metadata = resolved(&uri).unwrap();
    assert_eq!(metadata.name, "Lifeform 2924");
    assert_eq!(metadata.image, "");
    assert_eq!(metadata.animation, Some(NftAnimation::Html(animation)));
}

#[test]
fn fetches_ipfs_metadata_through_the_fixed_gateway() {
    let MetadataSource::Fetch(request) =
        resolve_metadata_uri("ipfs://bafybeigdyrzt/0.json", &LIMITS)
    else {
        panic!("expected a fetch")
    };
    assert_eq!(request.url, "https://ipfs.io/ipfs/bafybeigdyrzt/0.json");
    assert_eq!(request.method, HttpMethod::Get);
    assert_eq!(
        request.headers,
        vec![HttpHeader {
            name: "accept".into(),
            value: "application/json".into()
        }]
    );
    assert_eq!(
        (request.max_body_bytes, request.deadline_ms),
        (65_536, 10_000)
    );
    let reply = HttpOutcome {
        status: 200,
        retry_after: None,
        body: Some(METADATA.as_bytes().to_vec()),
        timed_out: false,
    };
    let metadata = metadata_fetch_judge(&reply, &LIMITS).unwrap();
    assert_eq!(
        metadata.image,
        "https://ipfs.io/ipfs/bafybeigdyrzt/image.png"
    );
}

#[test]
fn a_failed_or_over_large_fetch_is_no_metadata() {
    let reply = |status: u16, body: Option<Vec<u8>>| HttpOutcome {
        status,
        retry_after: None,
        body,
        timed_out: false,
    };
    let ok = Some(METADATA.as_bytes().to_vec());
    assert_eq!(metadata_fetch_judge(&reply(404, ok.clone()), &LIMITS), None);
    assert_eq!(metadata_fetch_judge(&reply(200, None), &LIMITS), None);
    let large = Some(vec![b' '; 65_537]);
    assert_eq!(metadata_fetch_judge(&reply(200, large), &LIMITS), None);
    assert_eq!(
        metadata_fetch_judge(&reply(200, Some(b"{}".to_vec())), &LIMITS),
        None
    );
    let bom = [&[0xEF, 0xBB, 0xBF][..], METADATA.as_bytes()].concat();
    assert!(metadata_fetch_judge(&reply(200, Some(bom)), &LIMITS).is_some());
}

#[test]
fn fails_closed_for_malformed_local_and_over_large_metadata() {
    assert_eq!(
        resolve_metadata_uri("data:application/json,%7Bbad", &LIMITS),
        MetadataSource::Resolved(None)
    );
    assert_eq!(
        resolve_metadata_uri("https://127.0.0.1/meta.json", &LIMITS),
        MetadataSource::Resolved(None)
    );
    let large = format!("data:application/json,{}", "x".repeat(33_000));
    assert_eq!(
        resolve_metadata_uri(&large, &LIMITS),
        MetadataSource::Resolved(None)
    );
    assert_eq!(
        resolve_metadata_uri("data:text/plain,{}", &LIMITS),
        MetadataSource::Resolved(None)
    );
    assert_eq!(
        resolve_metadata_uri("http://cdn.example/m.json", &LIMITS),
        MetadataSource::Resolved(None)
    );
}

#[test]
fn reads_attributes_within_their_bounds() {
    let items: Vec<String> = (0..40)
        .map(|i| format!(r#"{{"trait_type":"t{i}","value":true}}"#))
        .collect();
    let document = format!(
        r#"{{"attributes":[{{"trait_type":"","value":"x"}},{{"trait_type":"a","value":null}},{}]}}"#,
        items.join(",")
    );
    let metadata = metadata_from_json(&document, &LIMITS).unwrap();
    // The first 32 items are read, and the two unusable ones among them dropped.
    assert_eq!(metadata.attributes.len(), 30);
    assert_eq!(metadata.attributes[0], attribute(None, "t0", "true"));
    assert_eq!(metadata_from_json("[]", &LIMITS), None);
    assert_eq!(metadata_from_json(r#"{"name":"  "}"#, &LIMITS), None);
}

#[test]
fn a_padded_data_uri_is_never_a_fetch() {
    // Trimmed, it is an image `data:` URI the image rule keeps; a fetch is `https` only.
    let svg = format!(" data:image/svg+xml;base64,{}", btoa(r#"{"name":"x"}"#));
    assert_eq!(
        resolve_metadata_uri(&svg, &LIMITS),
        MetadataSource::Resolved(None)
    );
}
