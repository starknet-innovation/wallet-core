//! The sign card's decoded rows (the extension's side panel `sign-request-decode.ts`,
//! `sign-request-rows.ts`, `sign-request-text.ts`): each domain and message value of accepted
//! typed data, resolved against the document's own types and printed as the extension prints it
//! (`signValueText`), split into the rows the hash covers and the ones it does not.
//!
//! The extension decodes any payload and withholds Sign when it cannot place a value
//! (`classified: false`). Here only a document [`CheckedTypedData`] accepted is decoded, so every
//! declared field is present, every enum one declared variant of its tuple's arity, and every
//! leaf a value its type can mean; a value that could not be placed anyway, or a key longer than
//! the card prints, gives `None`, and the caller refuses the request. An undeclared key is a
//! certain answer (not signed): Krusty refuses one in the message, and the gate allows only a
//! revision 0 domain's `revision`, which Krusty drops before hashing.
//!
//! Two felts the extension reads from starknet.js are not printed: a `merkletree` field's root
//! and a revision 1 `string`'s ByteArray hash. Krusty owns the encoder and exposes neither, and a
//! felt derived a second way could differ from the one signed.
// ponytail: no root row and no ByteArray felt until Krusty exports its `encode_value`; a leaf
// carrying a revision 1 `string` may then also order differently inside its merkle group.

use super::typed_data::{is_safe_snip12_name, CheckedTypedData, Field, ENUM, MERKLETREE};
use super::typed_data_scalars::{snip12_reading, Encoded, Kind, Reading};
use crate::js::big::parse_bigint;
use crate::js::json::JsValue;
use crate::js::num::{number_to_string, utf16_cmp, utf16_len, utf16_slice};

/// One row: the dotted path a value sits at and the text the card prints for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignRow {
    pub key: String,
    pub value: String,
}

/// One object's rows: what the hash covers, and what the site sent that it does not.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SignBlock {
    pub signed: Vec<SignRow>,
    pub unsigned: Vec<SignRow>,
}

/// The card's domain and message blocks. The primary type is the message block's first signed
/// row, never a heading: it is site-chosen text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignRows {
    pub domain: SignBlock,
    pub message: SignBlock,
}

/// `SIGN_KEY_MAX_LENGTH`: four maximum-length names and their separators.
const SIGN_KEY_MAX_LENGTH: usize = 4 * super::typed_data::SNIP12_MAX_NAME_LENGTH + 3;
const MAX_VALUE_TEXT: usize = 256;
const MAX_NAME_SKETCH: usize = 24;

/// A row value before it is printed.
#[derive(Debug, Clone)]
enum Value {
    Scalar(Reading, String),
    /// Text the card prints as is: wallet-made, or a type name the gate accepted.
    Text(String),
    /// A value the hash does not cover, as the site sent it.
    Raw(JsValue),
}

type Row = (String, Value);

#[derive(Default)]
struct Split {
    signed: Vec<Row>,
    unsigned: Vec<Row>,
}

impl CheckedTypedData<'_> {
    /// `decodeSignRequest` for an accepted document, or `None` where the extension would withhold
    /// Sign.
    pub fn sign_rows(&self) -> Option<SignRows> {
        let value = self.value();
        let primary = value.get("primaryType")?.as_str()?;
        let walk = Walk { data: self };
        let mut domain = Split::default();
        let domain_type = self.revision().domain_type();
        let declared = self.graph().get(domain_type)?;
        walk.structure("", value.get("domain")?, declared, &mut domain)?;
        let mut message = Split::default();
        let fields = walk.fields_of(primary)?;
        message.signed.push((
            "⟨primary type⟩".to_string(),
            Value::Text(primary.to_string()),
        ));
        walk.structure("", value.get("message")?, fields, &mut message)?;
        Some(SignRows {
            domain: block(domain)?,
            message: block(message)?,
        })
    }
}

struct Walk<'c, 'a> {
    data: &'c CheckedTypedData<'a>,
}

impl Walk<'_, '_> {
    /// A declaration first, then the revision's preset: the encoder's order.
    fn fields_of(&self, ty: &str) -> Option<&[Field<'_>]> {
        match self.data.graph().get(ty) {
            Some(fields) => Some(fields),
            None => self.data.revision().preset(ty),
        }
    }

    fn structure(
        &self,
        path: &str,
        value: &JsValue,
        fields: &[Field<'_>],
        out: &mut Split,
    ) -> Option<()> {
        for field in fields {
            self.field(&join(path, field.name), value.get(field.name)?, field, out)?;
        }
        for (key, extra) in value.as_object()? {
            if !fields.iter().any(|field| field.name == key) {
                out.unsigned
                    .push((join(path, key), Value::Raw(extra.clone())));
            }
        }
        Some(())
    }

    fn field(&self, path: &str, value: &JsValue, field: &Field<'_>, out: &mut Split) -> Option<()> {
        match (field.ty, field.contains) {
            (ENUM, Some(contains)) => self.enumeration(path, value, contains, out),
            (MERKLETREE, Some(contains)) => self.merkle(path, value, contains, out),
            _ => self.ty(path, value, field.ty, out),
        }
    }

    fn ty(&self, path: &str, value: &JsValue, ty: &str, out: &mut Split) -> Option<()> {
        if let Some(element) = ty.strip_suffix('*') {
            let items = value.as_array()?;
            if items.is_empty() {
                out.signed
                    .push((path.to_string(), Value::Raw(JsValue::Array(Vec::new()))));
            }
            for (index, item) in items.iter().enumerate() {
                self.ty(&format!("{path}[{index}]"), item, element, out)?;
            }
            return Some(());
        }
        if let Some(fields) = self.fields_of(ty) {
            // Only the preset `u256` is two limbs the card also states as one amount.
            if self.data.graph().get(ty).is_none() && ty == "u256" {
                if let Some(total) = self.u256_total(value) {
                    out.signed.push((path.to_string(), Value::Text(total)));
                }
            }
            return self.structure(path, value, fields, out);
        }
        let reading = snip12_reading(value, ty, self.data.revision())?;
        out.signed
            .push((path.to_string(), Value::Scalar(reading, ty.to_string())));
        Some(())
    }

    /// `pushU256Total`: `(high << 128) + low` in decimal, when both limbs are `u128`.
    fn u256_total(&self, value: &JsValue) -> Option<String> {
        let limb = |name| {
            snip12_reading(value.get(name)?, "u128", self.data.revision())?
                .text
                .parse::<u128>()
                .ok()
        };
        let (low, high) = (limb("low")?, limb("high")?);
        let total = parse_bigint(&format!("0x{high:x}{low:032x}"))?;
        Some(format!(
            "{} (u256 total of high and low)",
            total.magnitude.to_decimal()
        ))
    }

    /// `walkEnum` and `walkVariant`: the selected variant's tuple type, then its elements.
    fn enumeration(
        &self,
        path: &str,
        value: &JsValue,
        contains: &str,
        out: &mut Split,
    ) -> Option<()> {
        let variants = self.fields_of(contains)?;
        let [(variant, payload)] = value.as_object()? else {
            return None;
        };
        let ty = variants.iter().find(|field| field.name == variant)?.ty;
        let inner = ty.strip_prefix('(')?.strip_suffix(')')?;
        let elements: Vec<&str> = if inner.is_empty() {
            Vec::new()
        } else {
            inner.split(',').collect()
        };
        let path = join(path, variant);
        out.signed.push((path.clone(), Value::Text(ty.to_string())));
        let items = payload
            .as_array()
            .filter(|items| items.len() == elements.len())?;
        for (index, (item, element)) in items.iter().zip(&elements).enumerate() {
            self.ty(&format!("{path}[{index}]"), item, element, out)?;
        }
        Some(())
    }

    /// `walkMerkle`: each leaf's rows, addressed by the group the library hashes it in.
    fn merkle(&self, path: &str, value: &JsValue, contains: &str, out: &mut Split) -> Option<()> {
        let leaves = value.as_array().filter(|leaves| !leaves.is_empty())?;
        let placeholder = format!("{path}{{}}");
        let decoded = leaves
            .iter()
            .map(|leaf| {
                let mut split = Split::default();
                self.ty(&placeholder, leaf, contains, &mut split)
                    .map(|()| split)
            })
            .collect::<Option<Vec<_>>>()?;
        let keys: Vec<String> = decoded.iter().map(leaf_order).collect();
        for (group, index) in layout(&keys) {
            let split = decoded.get(index)?;
            let to = format!("{path}{{{group}}}");
            let regroup = |rows: &[Row]| -> Vec<Row> {
                rows.iter()
                    .map(|(key, value)| {
                        let key = key
                            .strip_prefix(&placeholder)
                            .map_or_else(|| key.clone(), |rest| format!("{to}{rest}"));
                        (key, value.clone())
                    })
                    .collect()
            };
            out.signed.extend(regroup(&split.signed));
            out.unsigned.extend(regroup(&split.unsigned));
        }
        Some(())
    }
}

/// `sealed`: refuses a key longer than the card prints.
fn block(split: Split) -> Option<SignBlock> {
    let rows = |rows: Vec<Row>| {
        rows.into_iter()
            .map(|(key, value)| {
                (utf16_len(&key) <= SIGN_KEY_MAX_LENGTH).then(|| SignRow {
                    key,
                    value: value_text(&value),
                })
            })
            .collect::<Option<Vec<_>>>()
    };
    Some(SignBlock {
        signed: rows(split.signed)?,
        unsigned: rows(split.unsigned)?,
    })
}

/// `join`: one path segment, through the name grammar.
fn join(path: &str, part: &str) -> String {
    let segment = sign_name_text(part);
    if path.is_empty() {
        segment
    } else {
        format!("{path}.{segment}")
    }
}

/// `signNameText`: a safe name unchanged, anything else a bracketed sketch of its safe letters.
fn sign_name_text(name: &str) -> String {
    if is_safe_snip12_name(name) {
        return name.to_string();
    }
    let sketch: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '_' | '-'))
        .take(MAX_NAME_SKETCH)
        .collect();
    format!(
        "«{}»",
        if sketch.is_empty() {
            "unnamed"
        } else {
            &sketch
        }
    )
}

fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Boolean => "boolean",
        Kind::ByteString => "bytestring",
        Kind::Felt => "felt",
        Kind::Integer => "integer",
        Kind::Selector => "selector",
        Kind::Text => "text",
    }
}

fn encoded_text(encoded: &Encoded) -> String {
    match encoded {
        Encoded::Felt(felt) => crate::felt::canonical(felt),
        Encoded::ByteArray => String::new(),
    }
}

/// `leafOrder`: `JSON.stringify([signed, unsigned])`, a scalar as its `{encoded, kind, text, type}`.
fn leaf_order(split: &Split) -> String {
    let rows = |rows: &[Row]| {
        JsValue::Array(
            rows.iter()
                .map(|(key, value)| {
                    let value = match value {
                        Value::Scalar(reading, ty) => JsValue::Object(vec![
                            (
                                "encoded".into(),
                                JsValue::String(encoded_text(&reading.encoded)),
                            ),
                            (
                                "kind".into(),
                                JsValue::String(kind_name(reading.kind).into()),
                            ),
                            ("text".into(), JsValue::String(reading.text.clone())),
                            ("type".into(), JsValue::String(ty.clone())),
                        ]),
                        Value::Text(text) => JsValue::String(text.clone()),
                        Value::Raw(raw) => raw.clone(),
                    };
                    JsValue::Array(vec![JsValue::String(key.clone()), value])
                })
                .collect(),
        )
    };
    JsValue::Array(vec![rows(&split.signed), rows(&split.unsigned)]).to_json()
}

struct Node {
    key: String,
    placed: Vec<(String, usize)>,
}

/// `snip12MerkleLayout`: each leaf's index in the payload and its group, in the order shown. The
/// library sorts the two children of a node and pads an odd one against `0x0`, so siblings are
/// ordered by content and a pair is one group.
fn layout(keys: &[String]) -> Vec<(String, usize)> {
    let mut distinct: Vec<&String> = keys.iter().collect();
    distinct.sort_by(|a, b| utf16_cmp(a, b));
    distinct.dedup();
    let mut level: Vec<Node> = keys
        .iter()
        .enumerate()
        .map(|(index, key)| Node {
            key: distinct
                .iter()
                .position(|d| *d == key)
                .unwrap_or(0)
                .to_string(),
            placed: vec![(String::new(), index)],
        })
        .collect();
    let mut labelled = false;
    while level.len() > 1 {
        let mut next = Vec::with_capacity(level.len().div_ceil(2));
        let mut nodes = level.into_iter();
        while let Some(left) = nodes.next() {
            next.push(match nodes.next() {
                Some(right) => paired(left, right, labelled),
                None => Node {
                    key: format!("[{}]", left.key),
                    placed: left.placed,
                },
            });
        }
        level = next;
        labelled = true;
    }
    level.pop().map(|root| root.placed).unwrap_or_default()
}

fn paired(left: Node, right: Node, labelled: bool) -> Node {
    let (mut low, mut high) = if right.key < left.key {
        (right, left)
    } else {
        (left, right)
    };
    if labelled {
        low.placed
            .iter_mut()
            .for_each(|(group, _)| group.insert(0, 'a'));
        high.placed
            .iter_mut()
            .for_each(|(group, _)| group.insert(0, 'b'));
    }
    low.placed.extend(high.placed);
    Node {
        key: format!("({},{})", low.key, high.key),
        placed: low.placed,
    }
}

/// `signValueText`.
fn value_text(value: &Value) -> String {
    match value {
        Value::Scalar(reading, ty) => scalar_text(reading, ty),
        Value::Text(text) => display(text),
        Value::Raw(JsValue::Null) => "—".into(),
        Value::Raw(JsValue::String(text)) => display(text),
        Value::Raw(JsValue::Number(n)) => display(&number_to_string(*n)),
        Value::Raw(raw) => display(&raw.to_json()),
    }
}

/// `scalarText`: the declared type always named, the encoded felt always shown.
fn scalar_text(reading: &Reading, ty: &str) -> String {
    let quoted = format!("\"{}\"", display(&reading.text));
    let Encoded::Felt(felt) = reading.encoded else {
        return format!("{quoted} ({ty})");
    };
    let encoded = crate::felt::canonical(&felt);
    match reading.kind {
        Kind::Felt => format!("{encoded} ({ty})"),
        Kind::Boolean | Kind::Integer => format!("{} ({ty} = {encoded})", reading.text),
        Kind::Text if !ty.ends_with("string") => {
            format!("{quoted} ({ty} short string = {encoded})")
        }
        Kind::Selector => format!("{quoted} ({ty} entry point = {encoded})"),
        Kind::Text | Kind::ByteString => format!("{quoted} ({ty} = {encoded})"),
    }
}

/// `truncateDisplay(escapeDisplay(text))`.
fn display(text: &str) -> String {
    let escaped = escape_display(text);
    let length = utf16_len(&escaped);
    if length <= MAX_VALUE_TEXT {
        return escaped;
    }
    format!(
        "{}… (+{} characters, see Raw typed data)",
        utf16_slice(&escaped, 0, MAX_VALUE_TEXT),
        length - MAX_VALUE_TEXT
    )
}

/// `escapeDisplay`: backslashes doubled, code points that move or hide text spelled `\uXXXX`.
fn escape_display(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        let code = u32::from(c);
        let hostile = code < 0x20
            || (0x7f..=0x9f).contains(&code)
            || matches!(code, 0xad | 0x61c | 0x2028 | 0x2029 | 0xfeff)
            || (0x200b..=0x200f).contains(&code)
            || (0x202a..=0x202e).contains(&code)
            || (0x2066..=0x2069).contains(&code);
        match c {
            '\\' => out.push_str("\\\\"),
            _ if hostile => out.push_str(&format!("\\u{code:04x}")),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
#[path = "sign_rows_tests.rs"]
mod tests;
