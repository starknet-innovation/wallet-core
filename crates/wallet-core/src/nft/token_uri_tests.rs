//! `tests/starknet/starknet-rpc-assets.test.ts`'s `getNftTokenUri` cases, with their values.

use super::*;
use serde_json::Value;

const CONTRACT: &str = "0x049d36570d4e46f48e99674bd3fcc84644ddd6b96f7c741b1562b82f9e004dc7";
const MAX: usize = 32_768;

fn read(token_id: u128) -> TokenUriRead {
    let contract = crate::felt::parse(CONTRACT).unwrap();
    TokenUriRead::new(contract, U256::from_u128(token_id), MAX).unwrap()
}

fn hex(bytes: &[u8]) -> String {
    format!(
        "0x{}",
        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
    )
}

// starknet.js `byteArrayFromString`, spelled as the test's `encodedByteArray` (counts in decimal).
fn encoded_byte_array(value: &str) -> Vec<String> {
    let chunks: Vec<&[u8]> = value.as_bytes().chunks(31).collect();
    let (data, pending, len) = match chunks.last() {
        Some(last) if last.len() < 31 => (&chunks[..chunks.len() - 1], hex(last), last.len()),
        _ => (&chunks[..], "0x00".to_string(), 0),
    };
    let mut out = vec![data.len().to_string()];
    out.extend(data.iter().map(|word| hex(word)));
    out.push(pending);
    out.push(len.to_string());
    out
}

fn result(id: u64, items: &[String]) -> RpcReply {
    RpcReply {
        id,
        status: 200,
        body: Some(format!(
            r#"{{"jsonrpc":"2.0","id":{id},"result":{}}}"#,
            serde_json::to_string(items).unwrap()
        )),
    }
}

fn not_found(id: u64) -> RpcReply {
    RpcReply {
        id,
        status: 200,
        body: Some(format!(
            r#"{{"jsonrpc":"2.0","id":{id},"error":{{"code":21,"message":"entrypoint not found"}}}}"#
        )),
    }
}

fn name(request: &RpcRequest) -> &'static str {
    let body: Value = serde_json::from_str(&request.body).unwrap();
    let selector = body["params"]["request"]["entry_point_selector"].clone();
    ["token_uri", "tokenURI", "uri"]
        .into_iter()
        .find(|name| selector == crate::felt::canonical(&crate::kms::selector(name)).as_str())
        .unwrap()
}

fn drive(
    mut read: TokenUriRead,
    reply: impl Fn(u64, &str) -> RpcReply,
) -> (ReadStep<String>, Vec<&'static str>) {
    let mut seen = Vec::new();
    let mut request = read.start(1);
    for id in 2.. {
        seen.push(name(&request));
        match read.judge(&reply(id - 1, name(&request)), id) {
            ReadStep::Request(next) => request = next,
            done => return (done, seen),
        }
    }
    unreachable!()
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn decodes_a_cairo_byte_array_from_token_uri() {
    let uri = r#"data:application/json,{"name":"Punk"}"#;
    let (step, seen) = drive(read(0), |id, _| result(id, &encoded_byte_array(uri)));
    assert_eq!(step, ReadStep::Done(uri.into()));
    assert_eq!(seen, ["token_uri"]);
}

#[test]
fn falls_back_to_token_uri_camel_case() {
    let uri = "ipfs://bafybeigdyrzt/0.json";
    let (step, seen) = drive(read(5), |id, name| match name {
        "token_uri" => not_found(id),
        _ => result(id, &encoded_byte_array(uri)),
    });
    assert_eq!(step, ReadStep::Done(uri.into()));
    assert_eq!(seen, ["token_uri", "tokenURI"]);
}

#[test]
fn rejects_malformed_byte_array_responses() {
    let (step, _) = drive(read(5), |id, _| {
        result(id, &strings(&["1000000", "0x1", "0x0", "0"]))
    });
    assert_eq!(step, ReadStep::Failed);
}

#[test]
fn decodes_a_legacy_felt_array_token_uri_a_spok_on_mainnet() {
    let legacy = strings(&[
        "0x9",
        "0x68747470733a2f2f636c6f75642e617267656e742d6170692e636f6d2f7631",
        "0x2f6d6f6d656e74732f6d657461646174612f332f",
        "0x33",
        "0x31",
        "0x31",
        "0x35",
        "0x36",
        "0x32",
        "0x31",
    ]);
    let (step, seen) = drive(read(3_115_621), |id, _| result(id, &legacy));
    assert_eq!(
        step,
        ReadStep::Done("https://cloud.argent-api.com/v1/moments/metadata/3/3115621".into())
    );
    assert_eq!(seen, ["token_uri"]);
}

#[test]
fn decodes_a_legacy_uri_split_into_one_byte_pieces() {
    let uri = format!("https://metadata.example/{}", "a".repeat(1200));
    let mut reply = vec![format!("0x{:x}", uri.len())];
    reply.extend(uri.bytes().map(|b| format!("0x{b:x}")));
    let (step, _) = drive(read(5), |id, _| result(id, &reply));
    assert_eq!(step, ReadStep::Done(uri));
}

#[test]
fn rejects_malformed_legacy_felt_arrays() {
    let https = hex(b"https://");
    let long_piece = format!("0x{}", "61".repeat(32));
    let padded = format!("0x{}61", "0".repeat(64));
    let mut too_many = vec!["0x8001".to_string()];
    too_many.extend(std::iter::repeat_n("0x61".to_string(), 0x8001));
    let cases: Vec<(&str, Vec<String>)> = vec![
        (
            "a count that doesn't match",
            vec!["0x2".into(), https.clone()],
        ),
        ("an empty piece", vec!["0x2".into(), https, "0x0".into()]),
        ("a control byte", strings(&["0x1", "0x68740a"])),
        ("a non-ASCII byte", strings(&["0x1", "0x68ff"])),
        (
            "a piece longer than 31 bytes",
            vec!["0x1".into(), long_piece],
        ),
        ("a zero count", strings(&["0x0"])),
        ("a non-numeric count", strings(&["nope", "0x61"])),
        ("a non-hex piece", strings(&["0x1", "97"])),
        (
            "a zero-padded piece past felt length",
            vec!["0x1".into(), padded],
        ),
        ("more pieces than the URI byte bound", too_many),
    ];
    for (label, reply) in cases {
        let (step, _) = drive(read(5), |id, _| result(id, &reply));
        assert_eq!(step, ReadStep::Failed, "{label}");
    }
}

#[test]
fn reads_an_erc1155_uri_and_substitutes_the_id_placeholder() {
    let template = "https://metadata.example/{id}.json";
    let (step, seen) = drive(read(342), |id, name| match name {
        "uri" => result(id, &encoded_byte_array(template)),
        _ => not_found(id),
    });
    assert_eq!(
        step,
        ReadStep::Done(
            "https://metadata.example/0000000000000000000000000000000000000000000000000000000000000156.json"
                .into()
        )
    );
    assert_eq!(seen, ["token_uri", "tokenURI", "uri"]);
}

#[test]
fn an_empty_uri_is_no_answer_and_a_null_result_is_transport() {
    let (step, seen) = drive(read(1), |id, _| result(id, &encoded_byte_array("")));
    assert_eq!(step, ReadStep::Failed);
    assert_eq!(seen, ["token_uri", "tokenURI", "uri"]);
    let mut read = read(1);
    read.start(1);
    let null = RpcReply {
        id: 1,
        status: 200,
        body: Some(r#"{"jsonrpc":"2.0","id":1,"result":null}"#.into()),
    };
    assert_eq!(read.judge(&null, 2), ReadStep::Transport);
    // `response.every` throws on an object, so the extension fails over to the next endpoint.
    let mut again = super::tests::read(1);
    again.start(1);
    let object = RpcReply {
        id: 1,
        status: 200,
        body: Some(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#.into()),
    };
    assert_eq!(again.judge(&object, 2), ReadStep::Transport);
}

#[test]
fn a_byte_array_is_bounded_and_decodes_bytes_as_latin1() {
    // Exactly at the bound decodes; one byte past it does not.
    let at = "a".repeat(MAX);
    let past = "a".repeat(MAX + 1);
    let answer = |s: &str| -> Vec<JsValue> {
        encoded_byte_array(s)
            .into_iter()
            .map(JsValue::String)
            .collect()
    };
    assert_eq!(decode_byte_array(&answer(&at), MAX), Some(at));
    assert_eq!(decode_byte_array(&answer(&past), MAX), None);
    // starknet.js reads each byte as one character: 0xe9 is `é`, two UTF-8 bytes long.
    let latin1 = answer("x");
    let latin1 = [
        latin1[0].clone(),
        JsValue::String("0xe9".into()),
        latin1[2].clone(),
    ];
    assert_eq!(decode_byte_array(&latin1, MAX), Some("é".into()));
    assert_eq!(decode_byte_array(&latin1, 1), None);
}

#[test]
fn refuses_a_zero_contract() {
    assert!(TokenUriRead::new(Felt::ZERO, U256::ZERO, MAX).is_err());
}
