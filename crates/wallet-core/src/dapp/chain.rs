//! The chain methods' gates and results: `wallet_addStarknetChain`
//! (`background/provider/add-chain-ops.ts`), `wallet_switchStarknetChain` (`transactions/
//! chain-ops.ts`'s `handleSwitchChain` and `executeSwitchChainApproval`) and
//! `wallet_deploymentData` (`portfolio/discovery-ops.ts`), with the page's results as
//! `wallet-protocol/src/provider/results.ts` shapes them.
//!
//! Every refusal string the extension returns here maps to UNKNOWN_ERROR (163) on the dapp wire
//! (`errorForMessage`), except an unsupported chain (117), an unsupported API version (162), an
//! already deployed account (115) and missing deployment data (116).

use super::connect::{DappAccount, Site, Versions};
use super::error::WalletRpcError;
use crate::felt;
use crate::js::felt::padded_felt;
use crate::js::json::JsValue;
use crate::kms::NetworkId;
use crate::send::sign::Deployment;
use crate::send::types::DeploymentState;

/// `networkIdForChainId`: the wallet's network whose chain id has this felt value, whatever its
/// spelling.
pub fn network_for_chain_id(chain_id: &str) -> Option<NetworkId> {
    crate::send::multisig_invoke::network_of(&felt::parse(&chain_id.to_ascii_lowercase())?)
}

/// `handleAddChainOps`: a chain the wallet already has is added (`true`); any other is refused as
/// unsupported, never stored (the wallet has no custom networks, so a later switch could not
/// honour it). Nothing is queued. A blocked site learns nothing.
pub fn add_chain(site: &Site<'_>, chain_id: &str) -> Result<JsValue, WalletRpcError> {
    if site.blocked {
        return Err(WalletRpcError::unknown_error());
    }
    match network_for_chain_id(chain_id) {
        Some(_) => Ok(JsValue::Bool(true)),
        None => Err(WalletRpcError::chain_id_not_supported()),
    }
}

/// `handleSwitchChain`'s gates: the blocklist, a chain the wallet has (117), then a site the
/// active account is granted to (locked counts as not connected). Passing, the host queues a
/// switch card under [`switch_dedupe`]; the switch never happens without the person.
pub fn switch_chain(site: &Site<'_>, chain_id: &str) -> Result<NetworkId, WalletRpcError> {
    if site.blocked {
        return Err(WalletRpcError::unknown_error());
    }
    let network =
        network_for_chain_id(chain_id).ok_or_else(WalletRpcError::chain_id_not_supported)?;
    site.granted()
        .map(|_| network)
        .ok_or_else(WalletRpcError::unknown_error)
}

/// The dedupe key a switch card is queued under: a site asking again for the same chain while its
/// card is open joins it (`requestSwitchChain`).
pub fn switch_dedupe(network: NetworkId) -> String {
    format!("switch-chain:{}", network.catalog_id())
}

/// `executeSwitchChainApproval`'s pick, before it switches: the account `origin`'s stored grant
/// holds on `network`, which the switch lands on (`heldAccountOn`). `None` lets the wallet pick
/// its own account there, and then the site is not connected to it until it asks again.
pub fn held_account_on<'a>(
    grants: &[super::grants::OriginGrant],
    accounts: impl IntoIterator<Item = (&'a str, NetworkId)>,
    origin: &str,
    network: NetworkId,
) -> Option<&'a str> {
    let held = &grants
        .iter()
        .find(|grant| grant.origin == origin)?
        .account_ids;
    accounts
        .into_iter()
        .find(|(id, on)| *on == network && held.iter().any(|granted| granted == id))
        .map(|(id, _)| id)
}

/// `handleDeploymentData`'s gates, in its order: the blocklist, the API version (162), the lock,
/// and the granted active account. Passing, the host reads whether that account is deployed and
/// answers with [`deployment_data`].
pub fn deployment_data_gate<'a>(
    site: &Site<'a>,
    api_version: Option<&str>,
    versions: &Versions,
) -> Result<&'a DappAccount, WalletRpcError> {
    if site.blocked {
        return Err(WalletRpcError::unknown_error());
    }
    if api_version.is_some_and(|asked| !versions.wallet_api.contains(&asked)) {
        return Err(WalletRpcError::api_version_not_supported());
    }
    site.granted().ok_or_else(WalletRpcError::unknown_error)
}

/// `wallet_deploymentData`'s result (`ACCOUNT_DEPLOYMENT_DATA`, every felt padded to 64 digits),
/// for an account the chain reports undeployed (deployed is 115; an unknown answer is 163, never
/// read as undeployed) whose deployment this wallet builds and that reproduces `address`, the
/// account's stored address (`accountDeployment`; otherwise 116).
pub fn deployment_data(
    address: &str,
    deployment: Option<&Deployment>,
    state: DeploymentState,
) -> Result<JsValue, WalletRpcError> {
    match state {
        DeploymentState::Deployed => return Err(WalletRpcError::account_already_deployed()),
        DeploymentState::Unknown => return Err(WalletRpcError::unknown_error()),
        DeploymentState::Undeployed => {}
    }
    let deployment = deployment.ok_or_else(WalletRpcError::deployment_data_not_available)?;
    let address = deployment
        .address()
        .filter(|computed| felt::parse(&address.to_ascii_lowercase()) == Some(*computed))
        .ok_or_else(WalletRpcError::deployment_data_not_available)?;
    let padded = |value: &starknet_types_core::felt::Felt| {
        padded_felt(&felt::canonical(value))
            .map(JsValue::String)
            .ok_or_else(WalletRpcError::unknown_error)
    };
    let calldata = deployment
        .constructor_calldata
        .iter()
        .map(padded)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(JsValue::Object(vec![
        ("address".into(), padded(&address)?),
        ("calldata".into(), JsValue::Array(calldata)),
        ("class_hash".into(), padded(&deployment.class_hash)?),
        ("salt".into(), padded(&deployment.salt)?),
        ("version".into(), JsValue::Number(1.0)),
    ]))
}

#[cfg(test)]
#[path = "chain_tests.rs"]
mod tests;
