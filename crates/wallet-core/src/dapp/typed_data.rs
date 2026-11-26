//! SNIP-12 typed data a dapp asks the wallet to sign (`wallet_signTypedData`): the extension's
//! gate (`packages/platform/starknet/src/transactions/{typed-data,snip12-vocabulary}.ts`), its
//! chain binding (`typedDataHashForChain`) and its card summary (`describeTypedData`).
//!
//! The SNIP-12 hash is built from `encodeType`, a flat string joining type and field names with
//! `(`, `)`, `,`, `:` (and `"` on revision 1), none escaped. A name carrying a separator lets two
//! type graphs spell one string and one hash, so the person is shown one structure and signs
//! another. Both revisions are closed by one deny-by-default name grammar
//! ([`is_safe_snip12_name`]) applied to every type name, field name, field type and tuple
//! element, and each revision is checked against the vocabulary its own encoder resolves:
//! revision 0 has no presets, enums or tuples and coerces anything unknown to a felt.
//!
//! Two constructs never reach `encodeType` and need shape rules too. A `merkletree` leaf must be
//! a declared struct or a preset, never a basic type, or a site could paste a struct's leaf
//! hashes in as felts and reach the same root. An `enum` field spells like a plain reference to
//! its `contains` type, so a type named by an enum is a variant list (non-empty, all tuples) and
//! is reachable only through that enum.
//!
//! The values must then hold the declared shape (`typed_data_values`). Hashing is Krusty's
//! (`kms::typed_data_message_hash`), and only of a document this gate accepted. Krusty hashes
//! less than starknet.js: only its own domain (`name`, `version`, `chainId`, and `revision` on
//! revision 1, in that order), no undeclared message key, no empty struct, no legacy domain
//! beside the revision 1 one. Those pass this gate and fail closed as `Unhashable`.

use std::collections::{BTreeMap, BTreeSet};

use super::budget::{is_within, JsonBudget};
use super::error::WalletRpcError;
use super::typed_data_scalars::{snip12_scalar, Encoded};
use super::typed_data_values::conforms;
use crate::js::json::JsValue;
use crate::{kms, Felt};

/// The longest type or field name the wallet hashes or prints.
pub const SNIP12_MAX_NAME_LENGTH: usize = 64;

/// The domain key that selects the hash method, the one key allowed outside the domain type.
const REVISION_KEY: &str = "revision";

/// Leaf types the encoder resolves without a declaration, on both revisions.
const BASIC_TYPES: [&str; 10] = [
    "ClassHash",
    "ContractAddress",
    "bool",
    "felt",
    "i128",
    "selector",
    "shortstring",
    "string",
    "timestamp",
    "u128",
];

pub(super) const ENUM: &str = "enum";
pub(super) const MERKLETREE: &str = "merkletree";

/// One entry of a struct declaration. `contains` belongs only on `enum` and `merkletree`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Field<'a> {
    pub name: &'a str,
    pub ty: &'a str,
    pub contains: Option<&'a str>,
}

/// A payload's declared types by name.
pub(super) type Graph<'a> = BTreeMap<&'a str, Vec<Field<'a>>>;

const fn leaf(name: &'static str, ty: &'static str) -> Field<'static> {
    Field {
        name,
        ty,
        contains: None,
    }
}

/// Revision 1's preset structs as starknet.js's `presetTypes` hashes them.
const PRESETS: [(&str, &[Field<'static>]); 3] = [
    (
        "NftId",
        &[
            leaf("collection_address", "ContractAddress"),
            leaf("token_id", "u256"),
        ],
    ),
    (
        "TokenAmount",
        &[
            leaf("token_address", "ContractAddress"),
            leaf("amount", "u256"),
        ],
    ),
    ("u256", &[leaf("low", "u128"), leaf("high", "u128")]),
];

/// A SNIP-12 revision. The two hash differently and resolve different vocabularies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Revision {
    Zero,
    One,
}

impl Revision {
    /// The struct the domain object is hashed against.
    pub fn domain_type(self) -> &'static str {
        match self {
            Self::Zero => "StarkNetDomain",
            Self::One => "StarknetDomain",
        }
    }

    /// Whether this revision's encoder resolves `enum` fields and `(a,b)` tuples.
    pub(super) fn has_enums(self) -> bool {
        self == Self::One
    }

    /// A preset struct's fields; revision 0 has none.
    pub(super) fn preset(self, name: &str) -> Option<&'static [Field<'static>]> {
        match self {
            Self::Zero => None,
            Self::One => PRESETS
                .iter()
                .find(|(preset, _)| *preset == name)
                .map(|(_, fields)| *fields),
        }
    }

    /// Names a payload may not declare: the encoder keeps its own meaning for them.
    ///
    /// Deviation: the extension lets revision 0 declare a preset name (`u256`) as an ordinary
    /// struct, as starknet.js hashes it. Krusty hashes such a declaration differently from
    /// starknet.js, so the signature would not verify where the dapp checks it; it is refused
    /// on both revisions here.
    fn is_reserved(self, name: &str) -> bool {
        BASIC_TYPES.contains(&name)
            || is_container(name)
            || PRESETS.iter().any(|(preset, _)| *preset == name)
    }

    fn has_container(self, ty: &str) -> bool {
        ty == MERKLETREE || (ty == ENUM && self.has_enums())
    }
}

fn is_container(ty: &str) -> bool {
    ty == ENUM || ty == MERKLETREE
}

/// `identifyRevision`: the revision starknet.js would hash under (revision 1 first, on `"1"` or
/// `1`; revision 0 on an absent or `"0"` revision), or `None` where it refuses to hash.
fn identify_revision(types: &JsValue, domain: &JsValue) -> Option<Revision> {
    let declared = domain.get(REVISION_KEY);
    let one = matches!(declared, Some(JsValue::String(s)) if s == "1")
        || matches!(declared, Some(JsValue::Number(n)) if *n == 1.0);
    let zero = declared.is_none() || matches!(declared, Some(JsValue::String(s)) if s == "0");
    if types.get(Revision::One.domain_type()).is_some() && one {
        Some(Revision::One)
    } else if types.get(Revision::Zero.domain_type()).is_some() && zero {
        Some(Revision::Zero)
    } else {
        None
    }
}

/// `isSafeSnip12Name`: 1–64 ASCII characters from `/^[A-Za-z0-9_][A-Za-z0-9 _-]*$/`, no trailing
/// space. No `encodeType` separator, no `.` (the card's path separator), nothing non-ASCII.
pub fn is_safe_snip12_name(value: &str) -> bool {
    let mut bytes = value.bytes();
    let first = bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_');
    first
        && bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b' ' | b'_' | b'-'))
        && value.len() <= SNIP12_MAX_NAME_LENGTH
        && !value.ends_with(' ')
}

/// Why typed data was refused. Every reason is the extension's "invalid typed data", which a
/// page sees as INVALID_REQUEST_PAYLOAD (114).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypedDataRefusal {
    /// Not a SNIP-12 envelope the wallet will hash.
    Invalid,
    /// Krusty would not hash it.
    Unhashable,
    /// The signed domain does not bind the active network's chain id.
    ChainMismatch,
}

impl From<TypedDataRefusal> for WalletRpcError {
    fn from(_: TypedDataRefusal) -> Self {
        WalletRpcError::invalid_request_payload()
    }
}

/// Typed data the gate accepted (`isTypedDataShape`).
#[derive(Debug, Clone)]
pub struct CheckedTypedData<'a> {
    value: &'a JsValue,
    revision: Revision,
    graph: Graph<'a>,
}

impl<'a> CheckedTypedData<'a> {
    /// Checks untrusted typed data: within `budget` (the typed-data one), only the four envelope
    /// keys, a revision starknet.js identifies, its domain struct declared with no undeclared
    /// domain key, a declared primary type, an unambiguous type graph, and values that hold it.
    pub fn check(value: &'a JsValue, budget: &JsonBudget) -> Result<Self, TypedDataRefusal> {
        Self::read(value, budget).ok_or(TypedDataRefusal::Invalid)
    }

    fn read(value: &'a JsValue, budget: &JsonBudget) -> Option<Self> {
        let entries = value.as_object()?;
        let envelope = ["domain", "message", "primaryType", "types"];
        if !is_within(value, budget) || !entries.iter().all(|(k, _)| envelope.contains(&&**k)) {
            return None;
        }
        let types = value.get("types").filter(|t| t.as_object().is_some())?;
        let primary = value
            .get("primaryType")?
            .as_str()
            .filter(|p| !p.is_empty())?;
        let domain = value.get("domain").filter(|d| d.as_object().is_some())?;
        let message = value.get("message").filter(|m| m.as_object().is_some())?;
        let revision = identify_revision(types, domain)?;
        let graph = read_graph(types, revision)?;
        let variants = variant_lists(&graph, revision);
        let accepted = graph.contains_key(primary)
            && !variants.contains(primary)
            && is_unambiguous(&graph, revision, &variants)
            && is_supported_domain(domain, &graph, revision, &variants)
            && conforms(
                domain,
                revision.domain_type(),
                &graph,
                revision,
                budget.max_depth,
            )
            && conforms(message, primary, &graph, revision, budget.max_depth);
        accepted.then_some(Self {
            value,
            revision,
            graph,
        })
    }

    pub fn revision(&self) -> Revision {
        self.revision
    }

    pub fn value(&self) -> &'a JsValue {
        self.value
    }

    pub(super) fn graph(&self) -> &Graph<'a> {
        &self.graph
    }

    /// `typedDataHashForChain`: the message hash `account` signs, only when the domain declares a
    /// `chainId` (no `contains`) whose value, encoded under its declared type, is `chain_id`. So
    /// `"SN_MAIN"` and its numeric felt both bind mainnet; a missing, malformed or foreign chain
    /// id is refused, and there is no chain-agnostic fallback.
    pub fn message_hash_for_chain(
        &self,
        account: &Felt,
        chain_id: &Felt,
    ) -> Result<Felt, TypedDataRefusal> {
        let field = self
            .graph
            .get(self.revision.domain_type())
            .and_then(|fields| {
                fields
                    .iter()
                    .find(|f| f.name == "chainId" && f.contains.is_none())
            });
        let encoded = field.and_then(|field| {
            let value = self.value.get("domain")?.get("chainId")?;
            snip12_scalar(value, field.ty, self.revision)
        });
        if encoded != Some(Encoded::Felt(*chain_id)) {
            return Err(TypedDataRefusal::ChainMismatch);
        }
        self.message_hash(account)
    }

    /// `typedDataHash`: Krusty's SNIP-12 hash of the accepted document. Private: a dapp request
    /// is always hashed through [`Self::message_hash_for_chain`].
    fn message_hash(&self, account: &Felt) -> Result<Felt, TypedDataRefusal> {
        kms::typed_data_message_hash(&self.value.to_json(), account)
            .map_err(|_| TypedDataRefusal::Unhashable)
    }
}

/// `readField`: only `contains`, `name`, `type`; a safe name, a string type, a string `contains`
/// when present.
fn read_field(entry: &JsValue) -> Option<Field<'_>> {
    let entries = entry.as_object()?;
    let known = ["contains", "name", "type"];
    if !entries.iter().all(|(k, _)| known.contains(&&**k)) {
        return None;
    }
    let name = entry
        .get("name")?
        .as_str()
        .filter(|n| is_safe_snip12_name(n))?;
    let ty = entry.get("type")?.as_str()?;
    let contains = match entry.get("contains") {
        None => None,
        Some(contains) => Some(contains.as_str()?),
    };
    Some(Field { name, ty, contains })
}

/// `readTypeGraph`: every type name safe and not reserved, every declaration a list of fields
/// with unique names.
fn read_graph(types: &JsValue, revision: Revision) -> Option<Graph<'_>> {
    let mut graph = Graph::new();
    for (name, declaration) in types.as_object()? {
        if !is_safe_snip12_name(name) || revision.is_reserved(name) {
            return None;
        }
        let fields = declaration
            .as_array()?
            .iter()
            .map(read_field)
            .collect::<Option<Vec<_>>>()?;
        let names: BTreeSet<&str> = fields.iter().map(|field| field.name).collect();
        if names.len() != fields.len() {
            return None;
        }
        graph.insert(name.as_str(), fields);
    }
    Some(graph)
}

fn is_tuple(ty: &str) -> bool {
    ty.starts_with('(') && ty.ends_with(')')
}

/// The types some `enum` field names: variant lists, never structs.
fn variant_lists<'a>(graph: &Graph<'a>, revision: Revision) -> BTreeSet<&'a str> {
    if !revision.has_enums() {
        return BTreeSet::new();
    }
    graph
        .values()
        .flatten()
        .filter(|field| field.ty == ENUM)
        .filter_map(|field| field.contains)
        .collect()
}

/// A basic type, preset or declared struct (not a variant list), optionally one array of it.
fn is_type_reference(ty: &str, graph: &Graph<'_>, revision: Revision, variants: &Set) -> bool {
    let base = ty.strip_suffix('*').unwrap_or(ty);
    is_safe_snip12_name(base)
        && !variants.contains(base)
        && (BASIC_TYPES.contains(&base)
            || revision.preset(base).is_some()
            || graph.contains_key(base))
}

type Set<'a> = BTreeSet<&'a str>;

/// An enum variant's payload, `(felt,u128)`; `()` is the variant with none.
fn is_tuple_reference(ty: &str, graph: &Graph<'_>, revision: Revision, variants: &Set) -> bool {
    let inner = ty.strip_prefix('(').and_then(|rest| rest.strip_suffix(')'));
    inner.is_some_and(|inner| {
        inner.is_empty()
            || inner
                .split(',')
                .all(|element| is_type_reference(element, graph, revision, variants))
    })
}

/// A merkle leaf hashes as a struct, so only its values can bind the declared shape.
fn is_merkle_leaf(contains: &str, graph: &Graph<'_>, revision: Revision, variants: &Set) -> bool {
    !contains.ends_with('*')
        && !variants.contains(contains)
        && (graph.contains_key(contains) || revision.preset(contains).is_some())
}

fn is_valid_field(
    field: &Field<'_>,
    graph: &Graph<'_>,
    revision: Revision,
    variants: &Set,
    in_variant_list: bool,
) -> bool {
    if in_variant_list {
        return field.contains.is_none()
            && revision.has_enums()
            && is_tuple(field.ty)
            && is_tuple_reference(field.ty, graph, revision, variants);
    }
    if is_container(field.ty) {
        let Some(contains) = field.contains.filter(|_| revision.has_container(field.ty)) else {
            return false;
        };
        return if field.ty == ENUM {
            graph.contains_key(contains)
        } else {
            is_merkle_leaf(contains, graph, revision, variants)
        };
    }
    field.contains.is_none()
        && !is_tuple(field.ty)
        && is_type_reference(field.ty, graph, revision, variants)
}

fn is_unambiguous(graph: &Graph<'_>, revision: Revision, variants: &Set) -> bool {
    variants
        .iter()
        .all(|name| graph.get(name).is_some_and(|fields| !fields.is_empty()))
        && graph.iter().all(|(name, fields)| {
            fields.iter().all(|field| {
                is_valid_field(field, graph, revision, variants, variants.contains(name))
            })
        })
}

/// The revision's domain struct is declared (and no variant list), and every domain key is a
/// declared field or `revision`: an undeclared key reaches no hash.
fn is_supported_domain(
    domain: &JsValue,
    graph: &Graph<'_>,
    revision: Revision,
    variants: &Set,
) -> bool {
    let domain_type = revision.domain_type();
    let (Some(declared), Some(entries)) = (graph.get(domain_type), domain.as_object()) else {
        return false;
    };
    !variants.contains(domain_type)
        && entries
            .iter()
            .all(|(key, _)| key == REVISION_KEY || declared.iter().any(|field| field.name == key))
}

/// `describeTypedData`: `"<primaryType> · <domain.name>"` for the approval card, each cut to
/// [`SNIP12_MAX_NAME_LENGTH`] code points and `…`; `"message"` without a primary type, the type
/// alone without a domain name, and `"typed data"` for anything that is not an object.
pub fn describe_typed_data(value: &JsValue) -> String {
    if !matches!(value, JsValue::Object(_) | JsValue::Array(_)) {
        return "typed data".into();
    }
    let label = summary_name(
        value
            .get("primaryType")
            .and_then(JsValue::as_str)
            .unwrap_or("message"),
    );
    match value
        .get("domain")
        .and_then(|domain| domain.get("name"))
        .and_then(JsValue::as_str)
    {
        Some(name) if !name.is_empty() => format!("{label} · {}", summary_name(name)),
        _ => label,
    }
}

fn summary_name(name: &str) -> String {
    if name.chars().count() > SNIP12_MAX_NAME_LENGTH {
        let cut: String = name.chars().take(SNIP12_MAX_NAME_LENGTH).collect();
        format!("{cut}…")
    } else {
        name.to_string()
    }
}

#[cfg(test)]
#[path = "typed_data_tests.rs"]
pub(super) mod tests;

#[cfg(test)]
#[path = "typed_data_forgery_tests.rs"]
mod forgery_tests;

#[cfg(test)]
#[path = "typed_data_container_tests.rs"]
mod container_tests;

#[cfg(test)]
#[path = "typed_data_scalar_tests.rs"]
mod scalar_tests;

#[cfg(test)]
#[path = "typed_data_injectivity_tests.rs"]
mod injectivity_tests;
