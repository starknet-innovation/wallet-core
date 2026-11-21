//! Starknet ID name → address for mobile (the extension's `resolveStarkName`): the grammar, body
//! and judging are `wallet_core::stark_name`; this module holds Bramble's naming contracts and the
//! UniFFI exports. Like the extension, the host reads each endpoint's chain id
//! (`rpc_verify_chain_id`) before sending the call and never sends it to an endpoint on another
//! chain; `TryNextEndpoint` and transport failures move to the next endpoint, and with none left
//! the name is unreachable.

use wallet_core::{stark_name, Felt};

use crate::{CoreError, NetworkId};

pub use wallet_core::stark_name::StarkNameAnswer;

/// One endpoint's answer to the name lookup.
#[uniffi::remote(Enum)]
pub enum StarkNameAnswer {
    Resolved { address: String },
    NotFound,
    UnsupportedNetwork,
    TryNextEndpoint,
    Failed,
}

/// The Starknet ID naming contract per network. Not in the generated catalogs: the extension
/// takes it from starknet.js 10.8.0's `StarknetIdContract` (`starknetId.getStarknetIdContract`,
/// called in `packages/platform/starknet/src/rpc/stark-name.ts`).
fn naming_contract(network: NetworkId) -> Felt {
    match network {
        NetworkId::Mainnet => Felt::from_hex_unchecked(
            "0x6ac597f8116f886fa1c97a23fa4e08299975ecaf6b598873ca6792b9bbfb678",
        ),
        NetworkId::Sepolia => Felt::from_hex_unchecked(
            "0x154bc2e1af9260b9e66af0e9c46fc757ff893b3ff6a85718a810baf1474",
        ),
    }
}

/// The typed text as a canonical `name.stark`, or `None` when it is not a Starknet name (the
/// `.stark` suffix is required, never added). The extension's `normalizeStarkName`; its send
/// form's `isStarkName` is whether this is `Some`.
#[uniffi::export]
pub fn stark_name_normalize(name: String) -> Option<String> {
    stark_name::normalize(&name)
}

/// `starknet_call` body resolving a typed Starknet name on `network`. Refuses text the extension
/// would not look up.
#[uniffi::export]
pub fn stark_name_address_request(
    id: u64,
    network: NetworkId,
    name: String,
) -> Result<String, CoreError> {
    Ok(stark_name::address_request(
        id,
        &name,
        &naming_contract(network),
    )?)
}

/// Judges one endpoint's reply to that body.
#[uniffi::export]
pub fn stark_name_judge_address(body: String, id: u64) -> StarkNameAnswer {
    stark_name::judge_address(&body, id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contracts_are_the_extensions() {
        // stark-name.test.ts:86-92 pins the mainnet contract, and the body it is sent in.
        let body: serde_json::Value = serde_json::from_str(
            &stark_name_address_request(1, NetworkId::Mainnet, "fricoben.stark".into()).unwrap(),
        )
        .unwrap();
        assert_eq!(
            body["params"]["request"]["contract_address"],
            "0x6ac597f8116f886fa1c97a23fa4e08299975ecaf6b598873ca6792b9bbfb678"
        );
        assert_eq!(
            body["params"]["request"]["calldata"],
            serde_json::json!(["0x1", "0x15d246f6c1b", "0x0"])
        );
        // starknet.js 10.8.0's `StarknetIdContract.TESTNET_SEPOLIA`, in the spelling it has.
        let body: serde_json::Value = serde_json::from_str(
            &stark_name_address_request(1, NetworkId::Sepolia, "fricoben.stark".into()).unwrap(),
        )
        .unwrap();
        assert_eq!(
            body["params"]["request"]["contract_address"],
            "0x154bc2e1af9260b9e66af0e9c46fc757ff893b3ff6a85718a810baf1474"
        );
    }
}
