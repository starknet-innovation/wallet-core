// The extension's `tests/starknet/typed-data-injectivity.test.ts`: two structurally different
// accepted type graphs never reach one message hash. The generator is the TS one step for step
// (same xorshift32 seed and draw order), so the corpus is the same: the gate's verdict on every
// offered graph is pinned against the extension's (`MASK_*`, from running its generator and
// `isTypedDataShape`), and the sum mod P of every hashed graph's message hash against
// starknet.js's (`SUM_*`, over the graphs Krusty hashes: it refuses exactly those declaring an
// empty struct), which pins Krusty's hash to starknet.js's across the corpus. The envelope's
// domain is the one Krusty hashes rather than the TS `{ name }`. The `encodeType` search is
// starknet.js's own function and is not ported; the message-hash search covers it.

use std::collections::{BTreeMap, HashMap};

use super::tests::{domain, felt, hash, raw_hash, BUDGET};
use super::CheckedTypedData;
use crate::js::json;
use crate::Felt;
use serde_json::{json, Map, Value};

const SEED: u32 = 2_463_534_242;
const ATTEMPTS: usize = 300;
const REV0_HASH_STRIDE: usize = 5;

const NAMES: [&str; 15] = [
    "Details",
    "Detail",
    "s",
    "Order",
    "Order Details",
    "a b",
    "a",
    "b",
    "x y",
    "A-B",
    "1",
    "n_1",
    "n 1",
    "n-1",
    "_",
];
const FIELD_NAMES: [&str; 21] = [
    "Details",
    "Detail",
    "s",
    "Order",
    "Order Details",
    "a b",
    "a",
    "b",
    "x y",
    "A-B",
    "1",
    "n_1",
    "n 1",
    "n-1",
    "_",
    "token",
    "amount",
    "felt",
    "details",
    "V1",
    "V2",
];
const LEAVES: [&str; 5] = ["felt", "shortstring", "u128", "selector", "ContractAddress"];
const PRESETS: [&str; 3] = ["u256", "TokenAmount", "NftId"];

const MASK_0: &str = "111111111111111111111111111111111111111111111111111111111111111111111111110111111111111111111111111111111111111110111111111111111111111111111111111111111111111111111111111111111111111110111111111111111111111011111111111111111111111111111111111111111111111111111111111101111111011110111111111111111111";
const MASK_1: &str = "11111110101101011111111101100111110111101011111111111011010000111110000011110011001011101100101111110010011111110011101011011110010111111011101000110011111111001100001001111101111101011111101111111110101110111100010100111000011111111111110001111001000011101101110011110011111011010101011111111010111001101101101011110111001111011111101000011011000100001111110111101000011011101111001110001";
const SUM_0: &str = "0x743e20ecfdb8d7ea88ae88ee5eb5b942073eb6be0313b8a63ffa5876bd67125";
const SUM_1: &str = "0x395d9b5e0f2293c89abf1ff6b068607a2d2ab7f3ce963298bf40a97599a1af0";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct GField {
    name: &'static str,
    ty: String,
    contains: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Graph {
    primary: String,
    types: BTreeMap<String, Vec<GField>>,
}

/// xorshift32, as the TS `random` (its `|0` and `>>>` are these u32 operations).
struct Rng(u32);

impl Rng {
    fn random(&mut self) -> f64 {
        let mut s = self.0;
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        self.0 = s;
        f64::from(s) / 4_294_967_296.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.random() * n as f64) as usize
    }

    fn pick<T: Copy>(&mut self, values: &[T]) -> T {
        values[self.below(values.len())]
    }
}

fn field(name: &'static str, ty: impl Into<String>, contains: Option<&str>) -> GField {
    GField {
        name,
        ty: ty.into(),
        contains: contains.map(str::to_owned),
    }
}

fn link_field(rng: &mut Rng, next: &str, rev1: bool) -> (GField, Option<Vec<GField>>) {
    let roll = rng.random();
    if rev1 && roll < 0.25 {
        let link = field(rng.pick(&FIELD_NAMES), "enum", Some(next));
        let name = rng.pick(&FIELD_NAMES);
        let variant = field(name, format!("({})", rng.pick(&LEAVES)), None);
        return (link, Some(vec![variant]));
    }
    if roll < 0.45 {
        return (
            field(rng.pick(&FIELD_NAMES), "merkletree", Some(next)),
            None,
        );
    }
    let name = rng.pick(&FIELD_NAMES);
    let ty = if rng.random() < 0.5 {
        format!("{next}*")
    } else {
        next.to_string()
    };
    (field(name, ty, None), None)
}

fn extra_field(rng: &mut Rng, later: &[&str], rev1: bool) -> GField {
    let roll = rng.random();
    if rev1 && roll < 0.2 {
        let name = rng.pick(&FIELD_NAMES);
        return field(name, rng.pick(&PRESETS), None);
    }
    if roll < 0.35 && !later.is_empty() {
        let contains = rng.pick(later);
        return field(rng.pick(&FIELD_NAMES), "merkletree", Some(contains));
    }
    let base = if !later.is_empty() && rng.random() < 0.4 {
        rng.pick(later)
    } else {
        rng.pick(&LEAVES)
    };
    let name = rng.pick(&FIELD_NAMES);
    let ty = if rng.random() < 0.25 {
        format!("{base}*")
    } else {
        base.to_string()
    };
    field(name, ty, None)
}

fn graph(rng: &mut Rng, rev1: bool) -> Graph {
    let size = 1 + rng.below(4);
    let mut names: Vec<&str> = Vec::new();
    while names.len() < size {
        let candidate = rng.pick(&NAMES);
        if !names.contains(&candidate) {
            names.push(candidate);
        }
    }
    let mut types: BTreeMap<String, Vec<GField>> = BTreeMap::new();
    for (index, name) in names.iter().enumerate() {
        let mut fields: Vec<GField> = Vec::new();
        let mut add = |f: GField| {
            if !fields.iter().any(|taken| taken.name == f.name) {
                fields.push(f);
            }
        };
        if let Some(next) = names.get(index + 1) {
            let (link, variants) = link_field(rng, next, rev1);
            add(link);
            if let Some(variants) = variants {
                types.insert(next.to_string(), variants);
            }
        }
        for _ in 0..rng.below(3) {
            add(extra_field(rng, &names[index + 1..], rev1));
        }
        types.entry(name.to_string()).or_insert(fields);
    }
    Graph {
        primary: names[0].to_string(),
        types,
    }
}

fn preset_fields(ty: &str) -> Option<Vec<GField>> {
    let fields = match ty {
        "NftId" => vec![
            field("collection_address", "ContractAddress", None),
            field("token_id", "u256", None),
        ],
        "TokenAmount" => vec![
            field("token_address", "ContractAddress", None),
            field("amount", "u256", None),
        ],
        "u256" => vec![field("low", "u128", None), field("high", "u128", None)],
        _ => return None,
    };
    Some(fields)
}

/// Path-derived values: two graphs differing only in structure get the same values.
fn felt_of(seed: &str) -> Value {
    let acc = seed
        .bytes()
        .fold(7u64, |acc, b| (acc * 31 + u64::from(b)) % (1 << 32));
    json!(format!("0x{:x}", acc % 1_000_000 + 1))
}

fn sample(ty: &str, types: &BTreeMap<String, Vec<GField>>, seed: &str, depth: usize) -> Value {
    if depth > 6 {
        return felt_of(seed);
    }
    if let Some(element) = ty.strip_suffix('*') {
        return json!([0, 1].map(|i| sample(element, types, &format!("{seed}[{i}]"), depth + 1)));
    }
    let Some(fields) = types.get(ty).cloned().or_else(|| preset_fields(ty)) else {
        return felt_of(seed);
    };
    let mut out = Map::new();
    for f in &fields {
        out.insert(
            f.name.to_string(),
            field_value(f, types, &format!("{seed}.{}", f.name), depth + 1),
        );
    }
    Value::Object(out)
}

fn field_value(
    f: &GField,
    types: &BTreeMap<String, Vec<GField>>,
    seed: &str,
    depth: usize,
) -> Value {
    let Some(contains) = &f.contains else {
        return sample(&f.ty, types, seed, depth);
    };
    if f.ty == "merkletree" {
        return json!([0, 1].map(|i| sample(contains, types, &format!("{seed}[{i}]"), depth + 1)));
    }
    let Some(variant) = types.get(contains).and_then(|v| v.first()) else {
        return json!({});
    };
    let inner = variant
        .ty
        .get(1..variant.ty.len().saturating_sub(1))
        .unwrap_or("");
    let parts: Vec<Value> = if inner.is_empty() {
        Vec::new()
    } else {
        inner
            .split(',')
            .enumerate()
            .map(|(i, part)| sample(part, types, &format!("{seed}.{i}"), depth + 1))
            .collect()
    };
    let mut out = Map::new();
    out.insert(variant.name.to_string(), Value::Array(parts));
    Value::Object(out)
}

/// Every `enum` field rewritten as a plain reference to its `contains`: one `encodeType`.
fn plain_reference_twin(g: &Graph) -> Graph {
    let mut twin = g.clone();
    for fields in twin.types.values_mut() {
        for f in fields.iter_mut() {
            if f.ty == "enum" {
                if let Some(contains) = f.contains.take() {
                    f.ty = contains;
                }
            }
        }
    }
    twin
}

fn envelope(g: &Graph, message: &Value, rev1: bool) -> String {
    let mut types: Map<String, Value> = g
        .types
        .iter()
        .map(|(name, fields)| {
            let fields = fields.iter().map(|f| match &f.contains {
                Some(c) => json!({ "contains": c, "name": f.name, "type": f.ty }),
                None => json!({ "name": f.name, "type": f.ty }),
            });
            (name.clone(), Value::Array(fields.collect()))
        })
        .collect();
    let (domain, declaration) = domain(rev1);
    let name = if rev1 {
        "StarknetDomain"
    } else {
        "StarkNetDomain"
    };
    types.insert(name.into(), declaration);
    json!({ "domain": domain, "message": message, "primaryType": g.primary, "types": types })
        .to_string()
}

struct Entry {
    graph: Graph,
    payload: String,
}

/// The accepted corpus for a revision, and the gate's verdict on every offered spelling.
fn corpus(rev1: bool) -> (Vec<Entry>, String) {
    let mut rng = Rng(SEED);
    let mut accepted = Vec::new();
    let mut mask = String::new();
    for _ in 0..ATTEMPTS {
        let candidate = graph(&mut rng, rev1);
        let twin = plain_reference_twin(&candidate);
        let spellings = if twin == candidate {
            vec![candidate]
        } else {
            vec![candidate, twin]
        };
        for spelling in spellings {
            let message = sample(&spelling.primary, &spelling.types, "m", 0);
            let payload = envelope(&spelling, &message, rev1);
            let ok = CheckedTypedData::check(&json::parse(&payload).unwrap(), &BUDGET).is_ok();
            mask.push(if ok { '1' } else { '0' });
            if ok {
                accepted.push(Entry {
                    graph: spelling,
                    payload,
                });
            }
        }
    }
    (accepted, mask)
}

fn holds(g: &Graph, matches: impl Fn(&GField) -> bool) -> usize {
    usize::from(g.types.values().flatten().any(matches))
}

/// Hashes every entry Krusty hashes, failing on any collision. Returns the sum of the hashes and
/// the positions Krusty refused.
fn hash_search<'a>(entries: impl Iterator<Item = &'a Entry>) -> (Felt, Vec<usize>) {
    let mut seen: HashMap<Felt, &Graph> = HashMap::new();
    let mut sum = Felt::ZERO;
    let mut refused = Vec::new();
    for (index, entry) in entries.enumerate() {
        let Ok(hashed) = hash(&entry.payload) else {
            refused.push(index);
            continue;
        };
        if let Some(prior) = seen.insert(hashed, &entry.graph) {
            assert_eq!(prior, &entry.graph, "two graphs share {hashed:#x}");
        }
        sum += hashed;
    }
    (sum, refused)
}

// "the corpus is big enough and holds every construct the rules are about", and the gate agrees
// with the extension on every offered graph.
#[test]
fn the_corpus_is_the_extensions_and_holds_every_construct() {
    for (rev1, mask, sizes) in [
        (false, MASK_0, [293, 283, 189, 0, 0]),
        (true, MASK_1, [257, 244, 83, 52, 85]),
    ] {
        let (accepted, verdicts) = corpus(rev1);
        assert_eq!(verdicts, mask);
        let distinct: std::collections::BTreeSet<String> =
            accepted.iter().map(|e| format!("{:?}", e.graph)).collect();
        let count = |f: &dyn Fn(&GField) -> bool| {
            accepted.iter().map(|e| holds(&e.graph, f)).sum::<usize>()
        };
        let merkle = count(&|f| f.ty == "merkletree");
        let enums = count(&|f| f.ty == "enum");
        let presets = count(&|f| PRESETS.contains(&f.ty.trim_end_matches('*')));
        assert_eq!(
            [accepted.len(), distinct.len(), merkle, enums, presets],
            sizes
        );
    }
}

/// Positions whose graph declares a struct with no fields: starknet.js hashes those, Krusty
/// refuses them (and so does the wallet, as 114).
fn empty_structs<'a>(entries: impl Iterator<Item = &'a Entry>) -> Vec<usize> {
    entries
        .enumerate()
        .filter(|(_, e)| e.graph.types.values().any(Vec::is_empty))
        .map(|(index, _)| index)
        .collect()
}

// "no two accepted revision 1 graphs share a message hash". `SUM_1` is starknet.js's sum over the
// graphs Krusty hashes, so Krusty agrees with it on every one.
#[test]
fn no_two_accepted_revision_1_graphs_share_a_message_hash() {
    let (accepted, _) = corpus(true);
    let (sum, refused) = hash_search(accepted.iter());
    assert_eq!(refused, empty_structs(accepted.iter()));
    assert_eq!(accepted.len() - refused.len(), 172);
    assert_eq!(sum, felt(SUM_1));
}

// "no two accepted revision 0 graphs share a message hash" (every fifth, as the TS strides).
#[test]
fn no_two_accepted_revision_0_graphs_share_a_message_hash() {
    let (accepted, _) = corpus(false);
    let sample = || accepted.iter().step_by(REV0_HASH_STRIDE);
    let (sum, refused) = hash_search(sample());
    assert_eq!(refused, empty_structs(sample()));
    assert_eq!(sample().count() - refused.len(), 34);
    assert_eq!(sum, felt(SUM_0));
}

const P_HONEST: &str = r#"{"domain":{"chainId":"SN_MAIN","name":"Bramble Test","version":"1"},"message":{"root":[{"contract_address":"0xb8b38"},{"contract_address":"0x457f9"}]},"primaryType":"Session","types":{"Policy":[{"name":"contract_address","type":"felt"}],"Session":[{"contains":"Policy","name":"root","type":"merkletree"}],"StarkNetDomain":[{"name":"name","type":"felt"},{"name":"version","type":"felt"},{"name":"chainId","type":"felt"}]}}"#;
const P_FORGED: &str = r#"{"domain":{"chainId":"SN_MAIN","name":"Bramble Test","version":"1"},"message":{"root":["0x309bbbda0380c6554f07adf5c74a88e4640d1ac94673738bc5543fa9f207ab0","0x1ffc4af909e678508136a42e33e9a27c0aaaf5aabc0948149044432f18c087b"]},"primaryType":"Session","types":{"Session":[{"contains":"felt","name":"root","type":"merkletree"}],"StarkNetDomain":[{"name":"name","type":"felt"},{"name":"version","type":"felt"},{"name":"chainId","type":"felt"}]}}"#;
const H_HONEST: &str = "0x2c1ec91ee3ed7b4c1391d09ed8af8a0434453b551fadf07d003785c5d505ed3";

// "both searches flag the known collision pairs, so a clean sweep means something" (its merkle
// half; the separator half is an `encodeType` collision, pinned in `typed_data_forgery_tests`).
#[test]
fn the_merkle_substitution_is_a_real_collision_the_gate_refuses() {
    let honest = Graph {
        primary: "Session".into(),
        types: BTreeMap::from([
            (
                "Policy".into(),
                vec![field("contract_address", "felt", None)],
            ),
            (
                "Session".into(),
                vec![field("root", "merkletree", Some("Policy"))],
            ),
        ]),
    };
    let generated = envelope(&honest, &sample("Session", &honest.types, "m", 0), false);
    assert_eq!(hash(&generated), Ok(felt(H_HONEST)));
    assert_eq!(hash(P_HONEST), Ok(felt(H_HONEST)));
    assert_eq!(raw_hash(P_FORGED), Some(felt(H_HONEST)));
    assert!(hash(P_FORGED).is_err());
}
