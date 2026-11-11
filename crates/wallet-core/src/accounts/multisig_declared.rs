//! Whether a multisig class is declared on a network, as a sans-IO read (the extension's
//! `isArgentMultisigClassDeclared` in `packages/features/multisig/src/deploy.ts`).

use serde_json::json;

use super::multisig::ArgentMultisigClass;
use crate::kms::NetworkId;
use crate::rpc::{envelope, Request};
use crate::send::judge::{self, Answer, ChainAnswer};
use crate::Error;

/// A class definition is large; this bound applies to the class-declared read only.
const MAX_CLASS_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const CLASS_HASH_NOT_FOUND: i64 = 28;

#[derive(Debug, Clone, Copy)]
enum ClassPhase {
    ChainId,
    GetClass,
    Done(Option<bool>),
}

/// Whether a class is declared on a network. Only a class-not-found answer (JSON-RPC 28) means
/// `false`. Unlike the extension this does not yet re-hash the CASM against the pinned compiled
/// class hash: deployment is deferred, and this only drives a warning.
#[derive(Debug, Clone)]
pub struct ClassDeclaredRead {
    network: NetworkId,
    class_hash: String,
    phase: ClassPhase,
    pending: Option<u64>,
    next_id: u64,
}

impl ClassDeclaredRead {
    /// Starts a read of whether `class` is declared on `network`. The request names the class
    /// hash exactly as the class spells it.
    pub fn new(network: NetworkId, class: &ArgentMultisigClass) -> Self {
        Self {
            network,
            class_hash: class.class_hash().to_string(),
            phase: ClassPhase::ChainId,
            pending: None,
            next_id: 1,
        }
    }

    /// The next request, or `None` once decided.
    pub fn next_request(&mut self) -> Option<Request> {
        let (method, params) = match self.phase {
            ClassPhase::ChainId => ("starknet_chainId", json!([])),
            ClassPhase::GetClass => (
                "starknet_getClass",
                json!({ "block_id": "latest", "class_hash": self.class_hash }),
            ),
            ClassPhase::Done(_) => return None,
        };
        let id = self.pending.unwrap_or(self.next_id);
        if self.pending.is_none() {
            self.pending = Some(id);
            self.next_id += 1;
        }
        Some(Request::bounded(
            id,
            envelope(id, method, params),
            MAX_CLASS_RESPONSE_BYTES,
        ))
    }

    /// Judges the pending response; any other id is refused.
    pub fn accept_response(&mut self, id: u64, body: &str) -> Result<(), Error> {
        if self.pending != Some(id) {
            return Err(Error::rpc("response does not answer this request"));
        }
        self.pending = None;
        let answer = judge::answer_within(body, id, MAX_CLASS_RESPONSE_BYTES);
        self.phase = match (self.phase, answer) {
            (ClassPhase::ChainId, Some(Answer::Result(chain))) => {
                match judge::chain(Some(Answer::Result(chain)), self.network) {
                    ChainAnswer::Match => ClassPhase::GetClass,
                    _ => ClassPhase::Done(None),
                }
            }
            (ClassPhase::GetClass, Some(Answer::Result(class))) if class.is_object() => {
                ClassPhase::Done(Some(true))
            }
            (
                ClassPhase::GetClass,
                Some(Answer::Error {
                    code: Some(CLASS_HASH_NOT_FOUND),
                    ..
                }),
            ) => ClassPhase::Done(Some(false)),
            _ => ClassPhase::Done(None),
        };
        Ok(())
    }

    /// The host could not complete the pending request.
    pub fn transport_failed(&mut self, id: u64) {
        if self.pending == Some(id) {
            self.pending = None;
            self.phase = ClassPhase::Done(None);
        }
    }

    /// Whether the read has settled (including as unavailable).
    pub fn finished(&self) -> bool {
        matches!(self.phase, ClassPhase::Done(_))
    }

    /// `Some(declared)` once known; `None` while pending or when the network was unavailable.
    pub fn declared(&self) -> Option<bool> {
        match self.phase {
            ClassPhase::Done(declared) => declared,
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::*;
    use crate::accounts::multisig::tests::class;

    fn reply(read: &mut ClassDeclaredRead, reply: Value) -> String {
        let request = read.next_request().unwrap();
        let mut body = json!({"jsonrpc": "2.0", "id": request.id});
        body.as_object_mut()
            .unwrap()
            .extend(reply.as_object().unwrap().clone());
        read.accept_response(request.id, &body.to_string()).unwrap();
        request.body
    }

    fn after_chain(chain: &str) -> ClassDeclaredRead {
        let mut read = ClassDeclaredRead::new(NetworkId::Sepolia, &class());
        reply(&mut read, json!({ "result": chain }));
        read
    }

    #[test]
    fn only_class_not_found_means_undeclared() {
        let mut declared = after_chain("0x534e5f5345504f4c4941");
        let body = reply(&mut declared, json!({ "result": { "abi": "[]" } }));
        assert!(body.contains(class().class_hash()), "the class as spelled");
        assert_eq!(declared.declared(), Some(true));
        assert!(declared.next_request().is_none());

        let mut missing = after_chain("0x534e5f5345504f4c4941");
        reply(
            &mut missing,
            json!({ "error": { "code": 28, "message": "x" } }),
        );
        assert_eq!(missing.declared(), Some(false));

        for other in [
            json!({ "error": { "code": 24, "message": "x" } }),
            json!({ "result": "0x1" }),
        ] {
            let mut read = after_chain("0x534e5f5345504f4c4941");
            reply(&mut read, other);
            assert!(read.finished());
            assert_eq!(read.declared(), None);
        }

        let wrong_chain = after_chain("0x534e5f4d41494e");
        assert!(wrong_chain.finished());
        assert_eq!(wrong_chain.declared(), None);
    }

    #[test]
    fn a_stray_reply_is_refused_and_a_transport_failure_settles_unknown() {
        let mut read = ClassDeclaredRead::new(NetworkId::Sepolia, &class());
        let request = read.next_request().unwrap();
        assert_eq!(
            read.next_request().unwrap().id,
            request.id,
            "same pending id"
        );
        assert!(read.accept_response(request.id + 1, "{}").is_err());
        assert!(!read.finished());
        read.transport_failed(request.id);
        assert!(read.finished());
        assert_eq!(read.declared(), None);
    }
}
