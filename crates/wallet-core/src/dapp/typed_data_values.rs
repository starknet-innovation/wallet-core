//! Value-side conformance for a SNIP-12 envelope (the extension's `typed-data-values.ts`): does
//! the object a dapp sent hold the shape its own declarations describe.
//!
//! The graph check names one structure; starknet.js then reads values by plain indexing and
//! would hash something else: an empty `merkletree` never terminates, an enum payload that is a
//! string is indexed by character, a payload shorter or longer than its tuple hashes `undefined`
//! or hides elements, and a leaf is read as a number or a short string whatever its type says.
//! So every declared field must be present, every array an array, every merkle tree non-empty,
//! every enum exactly one declared variant with exactly its tuple's arity, and every leaf a value
//! its type can mean (`typed_data_scalars`). Undeclared keys are allowed: they hash to nothing,
//! and the approval card shows them as not signed.

use super::typed_data::{Field, Graph, Revision, ENUM, MERKLETREE};
use super::typed_data_scalars::snip12_scalar;
use crate::js::json::JsValue;

/// `conformsToTypeGraph`: whether `value` holds the shape `ty` describes, nesting no deeper than
/// `max_depth` (the typed-data budget's).
pub(super) fn conforms(
    value: &JsValue,
    ty: &str,
    graph: &Graph<'_>,
    revision: Revision,
    max_depth: usize,
) -> bool {
    Conform {
        graph,
        revision,
        max_depth,
    }
    .ty(value, ty, 0)
}

struct Conform<'g, 'a> {
    graph: &'g Graph<'a>,
    revision: Revision,
    max_depth: usize,
}

impl Conform<'_, '_> {
    fn ty(&self, value: &JsValue, ty: &str, depth: usize) -> bool {
        if depth > self.max_depth {
            return false;
        }
        if let Some(element) = ty.strip_suffix('*') {
            return self.list(value, element, depth, true);
        }
        match self.fields_of(ty) {
            Some(fields) => self.structure(value, fields, depth),
            None => snip12_scalar(value, ty, self.revision).is_some(),
        }
    }

    /// A declaration first, then a preset: `encodeValue`'s order.
    fn fields_of(&self, ty: &str) -> Option<&[Field<'_>]> {
        match self.graph.get(ty) {
            Some(fields) => Some(fields),
            None => self.revision.preset(ty),
        }
    }

    fn list(&self, value: &JsValue, element: &str, depth: usize, allow_empty: bool) -> bool {
        value.as_array().is_some_and(|items| {
            (allow_empty || !items.is_empty())
                && items.iter().all(|item| self.ty(item, element, depth + 1))
        })
    }

    /// Exactly one declared variant, its payload an array of exactly the tuple's length.
    fn enumeration(&self, value: &JsValue, contains: &str, depth: usize) -> bool {
        let (Some(variants), Some(entries)) = (self.graph.get(contains), value.as_object()) else {
            return false;
        };
        let [(selected, payload)] = entries else {
            return false;
        };
        let Some(declared) = variants.iter().find(|variant| variant.name == selected) else {
            return false;
        };
        let Some(inner) = declared
            .ty
            .strip_prefix('(')
            .and_then(|rest| rest.strip_suffix(')'))
        else {
            return false;
        };
        let elements: Vec<&str> = if inner.is_empty() {
            Vec::new()
        } else {
            inner.split(',').collect()
        };
        payload.as_array().is_some_and(|tuple| {
            tuple.len() == elements.len()
                && tuple
                    .iter()
                    .zip(&elements)
                    .all(|(item, element)| self.ty(item, element, depth + 1))
        })
    }

    fn field(&self, value: &JsValue, field: &Field<'_>, depth: usize) -> bool {
        match field.contains {
            Some(contains) if field.ty == ENUM && self.revision.has_enums() => {
                self.enumeration(value, contains, depth)
            }
            // A root over no leaves is not a hash: the library loops forever building it.
            Some(contains) if field.ty == MERKLETREE => self.list(value, contains, depth, false),
            _ => self.ty(value, field.ty, depth),
        }
    }

    fn structure(&self, value: &JsValue, fields: &[Field<'_>], depth: usize) -> bool {
        value.as_object().is_some()
            && fields.iter().all(|field| {
                value
                    .get(field.name)
                    .is_some_and(|own| self.field(own, field, depth + 1))
            })
    }
}
