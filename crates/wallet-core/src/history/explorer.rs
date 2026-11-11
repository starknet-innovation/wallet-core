//! Account ids and explorer links, as the extension builds them: `canonicalAccountId` /
//! `isAccountIdForNetwork` (`wallet-model/accounts`) and `explorerTransactionUrl` /
//! `explorerAccountUrl` (`wallet-model/networks`). The explorer base comes from the caller's
//! network catalog; a link is built only from a validated felt, never from a response body.

use crate::catalog::{network_info, NetworkInfo};
use crate::js::felt::{canonical_felt, is_non_zero_felt};
use crate::portfolio::model::{network_code, network_from_code};
use crate::{Error, NetworkId};

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

/// The address half of an account id.
pub fn account_id_address(value: &str) -> &str {
    value.split_once(':').map_or(value, |(_, address)| address)
}

/// The only builder of an activity, snapshot or history-scan account id:
/// `SN_MAIN`/`SN_SEPOLIA` + `:` + the **unpadded** canonical address (token ids, by contrast,
/// use the padded address). Swift and Kotlin never assemble this string.
pub fn activity_account_id(network: NetworkId, address: String) -> Result<String, Error> {
    canonical_account_id(&format!("{}:{address}", network_code(network)))
        .ok_or_else(|| Error::invalid("account address is not a field element"))
}

/// One gate for every explorer link: a well-formed non-zero felt, kept in the given spelling, on
/// a network the catalog holds.
pub fn explorer_url(
    networks: &[NetworkInfo],
    network: NetworkId,
    path: &str,
    felt: &str,
) -> Option<String> {
    let info = network_info(networks, network)?;
    is_non_zero_felt(felt).then(|| format!("{}/{path}/{felt}", info.explorer_base_url))
}

/// `explorerTransactionUrl`: `None` for anything that is not a non-zero lowercase felt.
pub fn explorer_transaction_url(
    networks: &[NetworkInfo],
    network: NetworkId,
    hash: String,
) -> Option<String> {
    explorer_url(networks, network, "tx", &hash)
}

/// `explorerAccountUrl`: the address in the spelling given; `None` when it is not a well-formed
/// non-zero felt, so no caller can build a link that cannot resolve.
pub fn explorer_account_url(
    networks: &[NetworkInfo],
    network: NetworkId,
    address: String,
) -> Option<String> {
    explorer_url(networks, network, "contract", &address)
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
        assert_eq!(
            activity_account_id(NetworkId::Mainnet, "0x00ab".into()).unwrap(),
            "SN_MAIN:0xab"
        );
        assert!(activity_account_id(NetworkId::Sepolia, "0xAB".into()).is_err());
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

    #[test]
    fn transaction_links_keep_the_spelling() {
        let networks = crate::catalog::testing::networks();
        assert_eq!(
            explorer_transaction_url(&networks, NetworkId::Mainnet, "0x00ab".into()).as_deref(),
            Some("https://starkscan.co/tx/0x00ab")
        );
        assert_eq!(
            explorer_transaction_url(&networks, NetworkId::Mainnet, "0x0".into()),
            None
        );
        assert_eq!(
            explorer_transaction_url(&networks, NetworkId::Sepolia, "0xAB".into()),
            None
        );
    }
}
