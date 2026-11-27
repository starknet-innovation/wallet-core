//! A Ledger account's signatures inside a plan (P8): the extension's `accountHashSigner`, which
//! routes a keyless Ledger account to the device and never to the vault.
//!
//! Where a plan signs its invoke or its deployment, an account given a device
//! (`Plan::with_device`) hands the hash to a `ledger::HashSigning` instead of `PlanHost::sign`:
//! the plan emits each APDU as `SendStep::Device`, the host relays the device's answer, and only
//! a signature that verified against the account's key continues to the write. `PlanHost::sign`
//! is refused for such an account wherever it would be reached, so a Ledger account never signs
//! with a key. A Braavos deployment's extended authorization is two device signatures, its
//! parameters' hash then the transaction's (`SignFor::BraavosParams`), for its estimates as for
//! its write, as the extension's `braavosDeploySignature` asks the device twice. A lock
//! ends the exchange at the next device answer, before another APDU is sent, and a signature the
//! device returns after the session ended is dropped (`Locked`), as the extension's sign queue
//! rejects at lock and at settle time. A gasless transfer's outside execution is signed here
//! too (`SignFor::Relay`, driven by `Plan::advance_relay`).
//!
//! As the extension's side panel keeps a refused request parked (`sign-session.ts`), a device
//! refusal (locked, app not open, blind signing off, the wrong device, a lost link) parks the plan
//! at `SendStep::DeviceRetry`: the host fixes the device and replies `RetryDevice`, which signs the
//! same hash again from the start, or closes the plan. A signature that does not verify ends the
//! plan (`DeviceRefused`), as the extension's background gate throws.

use super::fees::Bounds;
use super::plan::{DeploymentFor, Expect, Plan, PlanHost, Stage};
use super::sign::Deployment;
use super::types::{SendFailureCode as Code, SendStep};
pub use crate::ledger::DeviceKey;
use crate::ledger::{HashFailure, HashSigning, HashStep, Reply};
use crate::Felt;

/// An invoke's write, all but its signature.
pub(crate) struct InvokeWrite {
    pub nonce: Felt,
    pub bounds: Bounds,
    /// The automatic deployment accepted before it, if any.
    pub deployment: Option<Felt>,
    pub calldata: Vec<Felt>,
}

/// A deployment's write, all but its signature.
pub(crate) struct DeployWrite {
    pub bounds: Bounds,
    pub purpose: DeploymentFor,
    pub deployment: Deployment,
}

/// What a signature is for: everything the write needs once it is signed.
pub(crate) enum SignFor {
    Invoke(InvokeWrite),
    Deploy(DeployWrite),
    /// A Braavos deployment estimate, sent with the signature (`estimate_deployment`).
    DeployEstimate(Expect),
    /// Braavos's deploy parameters, signed first; then `hash` is signed for `then`.
    BraavosParams {
        hash: Felt,
        params: Vec<Felt>,
        then: Box<SignFor>,
    },
    /// Braavos's transaction hash, after its parameters signed as `aux`: the two make the
    /// extended authorization `[r, s, ...params, aux_r, aux_s]` `then` carries.
    BraavosHash {
        params: Vec<Felt>,
        aux: Vec<Felt>,
        then: Box<SignFor>,
    },
    /// A gasless transfer's checked outside execution (`plan_gasless`): no write, the signature
    /// goes back to the gasless path.
    Relay,
}

/// A signature in progress on the device.
pub(crate) struct DeviceWait {
    signing: HashSigning,
    then: SignFor,
}

impl DeviceWait {
    pub(super) fn relays(&self) -> bool {
        matches!(self.then, SignFor::Relay)
    }
}

impl Plan {
    /// Signs this plan's account on `device` rather than through `PlanHost::sign`: a Ledger
    /// account. The account's `public_key` is the key the device must hold.
    pub fn with_device(mut self, device: DeviceKey) -> Self {
        self.device = Some(device);
        self
    }

    /// Signs `hash` for `then`, through the host's key custody or the account's device, and
    /// sends the write once signed.
    pub(super) fn sign_for(&mut self, host: &dyn PlanHost, hash: Felt, then: SignFor) -> SendStep {
        let Some(device) = self.device else {
            return match self.sign(host, &hash) {
                Ok(signature) => self.send_signed(host, hash, then, signature),
                Err(code) => self.sign_failed(&then, code, None),
            };
        };
        let (signing, step) = HashSigning::start(device, hash, self.account.public_key, &|| {
            host.session_current()
        });
        self.on_device_step(host, DeviceWait { signing, then }, step)
    }

    /// Feeds the device's answer to the signature in progress.
    pub(super) fn on_device(
        &mut self,
        host: &dyn PlanHost,
        mut wait: DeviceWait,
        reply: Reply<'_>,
    ) -> SendStep {
        let step = wait.signing.on_reply(reply, &|| host.session_current());
        self.on_device_step(host, wait, step)
    }

    /// Signs the parked hash again from the start, after the host fixed the device.
    pub(super) fn retry_device(&mut self, host: &dyn PlanHost, mut wait: DeviceWait) -> SendStep {
        let step = wait.signing.retry(&|| host.session_current());
        self.on_device_step(host, wait, step)
    }

    fn on_device_step(
        &mut self,
        host: &dyn PlanHost,
        wait: DeviceWait,
        step: HashStep,
    ) -> SendStep {
        match step {
            HashStep::Send(apdu) => {
                self.stage = Stage::Device(Box::new(wait));
                SendStep::Device { apdu }
            }
            HashStep::Retry(code) => {
                self.stage = Stage::DeviceRefused(Box::new(wait));
                SendStep::DeviceRetry {
                    reason: code.as_str().to_string(),
                }
            }
            HashStep::Signed(signature) => {
                self.signatures += 1;
                let hash = wait.signing.hash();
                let signature = self.account.signature(signature.r(), signature.s());
                self.send_signed(host, hash, wait.then, signature)
            }
            HashStep::Failed(HashFailure::Locked) => {
                self.sign_failed(&wait.then, Code::Locked, None)
            }
            HashStep::Failed(HashFailure::Refused(code)) => {
                self.sign_failed(&wait.then, Code::DeviceRefused, Some(code.as_str()))
            }
        }
    }

    /// Ends the plan with nothing signed: an invoke after its accepted deployment reports that
    /// deployment, as any failure before the invoke's write does, still naming the device's
    /// reason in `detail`.
    fn sign_failed(&mut self, then: &SignFor, code: Code, detail: Option<&str>) -> SendStep {
        let mut step = match then {
            SignFor::Invoke(write) => self.after_deployment(write.deployment, code, None),
            SignFor::BraavosParams { then, .. } | SignFor::BraavosHash { then, .. } => {
                return self.sign_failed(then, code, detail)
            }
            SignFor::Deploy(_) | SignFor::DeployEstimate(_) | SignFor::Relay => {
                self.fail(code, None)
            }
        };
        if let (SendStep::Failed { failure }, Some(detail)) = (&mut step, detail) {
            failure.detail = Some(detail.to_string());
        }
        step
    }

    fn send_signed(
        &mut self,
        host: &dyn PlanHost,
        hash: Felt,
        then: SignFor,
        signature: Vec<Felt>,
    ) -> SendStep {
        match then {
            SignFor::Invoke(write) => self.send_invoke(host, hash, write, signature),
            SignFor::Deploy(write) => self.send_deploy(host, hash, write, signature),
            SignFor::DeployEstimate(expect) => self.send_deploy_estimate(expect, signature, false),
            SignFor::BraavosParams { hash, params, then } => {
                let then = SignFor::BraavosHash {
                    params,
                    aux: signature,
                    then,
                };
                self.sign_for(host, hash, then)
            }
            SignFor::BraavosHash { params, aux, then } => {
                let mut signature = signature;
                signature.extend(params);
                signature.extend(aux);
                self.send_signed(host, hash, *then, signature)
            }
            SignFor::Relay => self.relayed(signature),
        }
    }
}
