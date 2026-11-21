//! Deploying a new Argent multisig, as the extension's `wallet.deployArgentMultisig` does it:
//! `canonicalArgentMultisigDeployConfig` and `argentMultisigDeployment` (here
//! `accounts::multisig::predict`), the background's rule that the deploying account is a member
//! of the reviewed constructor (`deploymentContext`), then the platform's
//! `estimateAccountDeploymentFee` / `submitAccountDeployment`, which are the plan's standalone
//! deployment (`Kind::Deployment`) under `Authorization::ArgentMultisig`. The wire order is the
//! plan's: `extension-multisig-transactions.json`'s `deploy`.
//!
//! The pinned class and the fee policy are the wallet's (Bramble's are
//! `catalogs::multisig::argent_multisig_class` and `SEND_POLICY`). Checking that the class is
//! declared on the network (`accounts::multisig_declared`) is the host's read before it plans.

use super::plan::{Account, Authorization, Kind, Plan, Policy};
use super::sign::Deployment;
use crate::accounts::multisig::{
    calldata_felt, predict, ArgentMultisigClass, ArgentMultisigConfig, ArgentMultisigPrediction,
};
use crate::kms::NetworkId;
use crate::{felt, Felt};

/// Why a multisig deployment was refused before any plan exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MultisigDeployRefusal {
    /// The salt, signer set or threshold is not one the class deploys
    /// (`canonicalArgentMultisigDeployConfig`).
    InvalidConfig,
    /// The deploying account is not a member of the constructor ("selected signer is not a
    /// member of the reviewed constructor").
    DeployerNotMember,
}

/// `argentMultisigDeployment`: the DEPLOY_ACCOUNT a prediction commits to.
pub fn deployment(prediction: &ArgentMultisigPrediction) -> Option<Deployment> {
    Some(Deployment {
        class_hash: felt::parse(&prediction.class_hash)?,
        salt: felt::parse(&prediction.salt)?,
        constructor_calldata: prediction
            .constructor_calldata
            .iter()
            .map(|value| calldata_felt(value))
            .collect::<Option<_>>()?,
    })
}

/// The standalone deployment of the multisig `config` at `salt` on `class`, signed by the member
/// whose key is `deployer`. The plan's account is the predicted address, so the deployment it
/// signs always reproduces it.
pub fn deployment_plan(
    network: NetworkId,
    config: &ArgentMultisigConfig,
    salt: &str,
    class: &ArgentMultisigClass,
    deployer: &Felt,
    policy: Policy,
) -> Result<Plan, MultisigDeployRefusal> {
    use MultisigDeployRefusal::{DeployerNotMember, InvalidConfig};
    let prediction = predict(config, salt, class).map_err(|_| InvalidConfig)?;
    let address = felt::parse(&prediction.address).ok_or(InvalidConfig)?;
    let deployment = deployment(&prediction).ok_or(InvalidConfig)?;
    let members = prediction
        .config
        .signer_public_keys
        .iter()
        .map(|key| felt::parse(key))
        .collect::<Option<Vec<Felt>>>()
        .ok_or(InvalidConfig)?;
    if !members.contains(deployer) {
        return Err(DeployerNotMember);
    }
    let account = Account {
        network,
        address,
        public_key: *deployer,
        authorization: Authorization::ArgentMultisig {
            class_hash: class.hash(),
            query_signer: members[0],
        },
        deployment: Some(deployment),
    };
    Ok(Plan::new(account, Kind::Deployment, policy))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::multisig::tests::class;
    use crate::send::fees::Headroom;

    const POLICY: Policy = Policy {
        fee_margin_percent: 50,
        reviewed_fee_headroom: Headroom {
            numerator: 3,
            denominator: 2,
        },
        confirmation_delays_ms: &[0],
        max_invoke_calls: 16,
        max_call_calldata_felts: 4_096,
    };

    fn config(threshold: u32, keys: &[&str]) -> ArgentMultisigConfig {
        ArgentMultisigConfig {
            threshold,
            signer_public_keys: keys.iter().map(|k| k.to_string()).collect(),
        }
    }

    fn plan(
        config: &ArgentMultisigConfig,
        salt: &str,
        deployer: u8,
    ) -> Option<MultisigDeployRefusal> {
        deployment_plan(
            NetworkId::Sepolia,
            config,
            salt,
            &class(),
            &Felt::from(deployer),
            POLICY,
        )
        .err()
    }

    #[test]
    fn a_bad_config_or_salt_is_refused() {
        use MultisigDeployRefusal::InvalidConfig;
        assert_eq!(
            plan(&config(3, &["0x1", "0x2"]), "0x0", 1),
            Some(InvalidConfig)
        );
        assert_eq!(
            plan(&config(1, &["0x1", "0x1"]), "0x0", 1),
            Some(InvalidConfig)
        );
        assert_eq!(plan(&config(1, &["0x1"]), "0xZZ", 1), Some(InvalidConfig));
        assert_eq!(plan(&config(1, &["0x1", "0x2"]), "0x7", 2), None);
    }

    #[test]
    fn only_a_member_deploys() {
        assert_eq!(
            plan(&config(1, &["0x1", "0x2"]), "0x0", 3),
            Some(MultisigDeployRefusal::DeployerNotMember)
        );
    }

    #[test]
    fn the_deployment_reproduces_the_prediction() {
        let prediction = predict(&config(2, &["0x1", "0x2"]), "0x7", &class()).unwrap();
        let deployment = deployment(&prediction).unwrap();
        assert_eq!(
            deployment.address().map(|a| felt::canonical(&a)),
            Some(prediction.address)
        );
        assert_eq!(deployment.class_hash, class().hash());
    }
}
