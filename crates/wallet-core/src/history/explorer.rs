//! Account ids, as the extension builds them: `canonicalAccountId` / `isAccountIdForNetwork`
//! (`wallet-model/accounts`). Explorer links follow with the rest of history (plan step 2.4c).

use crate::js::felt::{canonical_felt, is_non_zero_felt};
use crate::portfolio::model::{network_code, network_from_code};
use crate::NetworkId;

/// `canonicalAccountId`: `<networkId>:<canonicalFelt(address)>` for a strictly spelled non-zero
/// felt address, else `None`.
pub fn canonical_account_id(value: &str) -> Option<String> {
    let mut parts = value.split(':');
    let (network, address) = (parts.next()?, parts.next()?);
    if parts.next().is_some() || network_from_code(network).is_none() || !is_non_zero_felt(address)
    {
        return None;
    }
    Some(format!("{network}:{}", canonical_felt(address)?))
}

/// `isAccountIdForNetwork`.
pub fn is_account_id_for_network(value: &str, network: NetworkId) -> bool {
    canonical_account_id(value).as_deref() == Some(value)
        && value.starts_with(&format!("{}:", network_code(network)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_ids_are_unpadded() {
        assert_eq!(
            canonical_account_id("SN_MAIN:0x00ab").as_deref(),
            Some("SN_MAIN:0xab")
        );
        assert_eq!(canonical_account_id("SN_SEPOLIA:0xAB"), None);
        assert!(is_account_id_for_network(
            "SN_SEPOLIA:0xab",
            NetworkId::Sepolia
        ));
        assert!(!is_account_id_for_network(
            "SN_SEPOLIA:0x0ab",
            NetworkId::Sepolia
        ));
        assert!(!is_account_id_for_network(
            "SN_MAIN:0xab",
            NetworkId::Sepolia
        ));
    }
}
