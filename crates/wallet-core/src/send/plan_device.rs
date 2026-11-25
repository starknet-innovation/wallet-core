//! A Ledger account's signatures inside a plan (P8): the extension's `accountHashSigner`, which
//! routes a keyless Ledger account to the device and never to the vault.
//!
//! Where a plan signs its invoke or its deployment, an account given a device
//! (`Plan::with_device`) hands the hash to a `ledger::DeviceSigning` instead of `PlanHost::sign`:
//! the plan emits each APDU as `SendStep::Device`, the host relays the device's answer, and only
//! a signature that verified against the account's key continues to the write. `PlanHost::sign`
//! is refused for such an account wherever it would be reached (a Braavos deployment's review
//! signature, its two-signature deployment), so a Ledger account never signs with a key. As the
//! extension's sign queue does at settle time, a signature the device returns after the session
//! ended is dropped (`Locked`).

use super::fees::Bounds;
use super::plan::{DeploymentFor, Plan, PlanHost, Stage};
use super::sign::Deployment;
use super::types::{SendFailureCode as Code, SendStep};
use crate::ledger::{DeviceSigning, DeviceStep, LedgerPath, Reply, Version};
use crate::Felt;

/// The device a plan's account signs on: its derivation path, and the oldest Starknet app the
/// wallet accepts there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceKey {
    pub path: LedgerPath,
    pub app_minimum: Version,
}

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
}

/// A signature in progress on the device.
pub(crate) struct DeviceWait {
    signing: DeviceSigning,
    hash: Felt,
    then: SignFor,
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
        if !host.session_current() {
            return self.sign_failed(&then, Code::Locked, None);
        }
        let (signing, step) = DeviceSigning::start(
            device.path,
            hash,
            self.account.public_key,
            device.app_minimum,
        );
        self.on_device_step(
            host,
            DeviceWait {
                signing,
                hash,
                then,
            },
            step,
        )
    }

    /// Feeds the device's answer to the signature in progress.
    pub(super) fn on_device(
        &mut self,
        host: &dyn PlanHost,
        mut wait: DeviceWait,
        reply: Reply<'_>,
    ) -> SendStep {
        let step = wait.signing.on_reply(reply);
        self.on_device_step(host, wait, step)
    }

    fn on_device_step(
        &mut self,
        host: &dyn PlanHost,
        wait: DeviceWait,
        step: DeviceStep,
    ) -> SendStep {
        match step {
            DeviceStep::Send(apdu) => {
                self.stage = Stage::Device(Box::new(wait));
                SendStep::Device { apdu }
            }
            DeviceStep::Signed(_) if !host.session_current() => {
                self.sign_failed(&wait.then, Code::Locked, None)
            }
            DeviceStep::Signed(signature) => {
                self.signatures += 1;
                let signature = self.account.signature(signature.r(), signature.s());
                self.send_signed(host, wait.hash, wait.then, signature)
            }
            DeviceStep::Refused(rejection) => self.sign_failed(
                &wait.then,
                Code::DeviceRefused,
                Some(rejection.code.as_str()),
            ),
        }
    }

    /// Ends the plan with nothing signed: an invoke after its accepted deployment reports that
    /// deployment, as any failure before the invoke's write does.
    fn sign_failed(&mut self, then: &SignFor, code: Code, detail: Option<&str>) -> SendStep {
        let mut step = match then {
            SignFor::Invoke(write) => self.after_deployment(write.deployment, code, None),
            SignFor::Deploy(_) => self.fail(code, None),
        };
        if let SendStep::Failed { failure } = &mut step {
            if failure.code == code {
                failure.detail = detail.map(str::to_string);
            }
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
        }
    }
}
