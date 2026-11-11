//! Bramble's choices for placing a key under an account family, over
//! `wallet_core::accounts::family_address` (the extension's `deriveAccountContractAddress`):
//! which class an account of each family has and the Argent constructor layout the catalog gives
//! that class (`catalogs::account_address`'s `resolve_class` and `argent_shape`), and the salt
//! (`0x0` for OpenZeppelin, the public key otherwise).
//!
//! The core places any non-zero class it is given, so every class reaches it through
//! `resolve_class`: that is what refuses an Argent or Braavos class the catalog does not list.

use starknet_types_core::felt::Felt;
use wallet_core::accounts;

#[cfg(test)]
use crate::account_catalog::catalog;
use crate::account_catalog::AccountFamily;
pub(crate) use crate::catalogs::account_address::{argent_shape, resolve_class};
use crate::networks::NetworkId;
use crate::CoreError;

/// Public outputs of placing one signer key under one account family and class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FamilyAddress {
    pub address: Felt,
    pub class_hash: Felt,
    pub deployment_salt: Option<Felt>,
}

fn invalid(reason: &str) -> CoreError {
    CoreError::InvalidInput {
        reason: reason.into(),
    }
}

/// Places `public_key` under `family` at `class_hash` (see `resolve_class`) with `salt` (Argent
/// and Braavos only; defaults to the public key), exactly as the extension derives the address.
pub(crate) fn family_address(
    family: AccountFamily,
    class_hash: Option<&Felt>,
    salt: Option<&Felt>,
    public_key: &Felt,
    network: NetworkId,
) -> Result<FamilyAddress, CoreError> {
    let class_hash = resolve_class(family, class_hash, network)?;
    let deployment_salt = match (family, salt) {
        (AccountFamily::OpenZeppelin, Some(_)) => {
            return Err(invalid("OpenZeppelin accounts do not take a salt"))
        }
        (AccountFamily::OpenZeppelin, None) => None,
        (_, salt) => Some(*salt.unwrap_or(public_key)),
    };
    let address = accounts::family_address(
        family,
        &class_hash,
        argent_shape(family, &class_hash)?,
        &deployment_salt.unwrap_or(Felt::ZERO),
        public_key,
    )?;
    Ok(FamilyAddress {
        address,
        class_hash,
        deployment_salt,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_rules_follow_the_extension() {
        let argent = catalog().argent_default;
        assert!(resolve_class(AccountFamily::Braavos, None, NetworkId::Mainnet).is_err());
        assert!(resolve_class(
            AccountFamily::OpenZeppelin,
            Some(&argent),
            NetworkId::Mainnet
        )
        .is_err());
        assert!(resolve_class(AccountFamily::Braavos, Some(&argent), NetworkId::Mainnet).is_err());
        assert_eq!(
            resolve_class(AccountFamily::Argent, None, NetworkId::Sepolia).unwrap(),
            argent
        );
        let key = Felt::from(7u64);
        assert!(family_address(
            AccountFamily::OpenZeppelin,
            None,
            Some(&key),
            &key,
            NetworkId::Sepolia
        )
        .is_err());
        let placed =
            family_address(AccountFamily::Argent, None, None, &key, NetworkId::Sepolia).unwrap();
        assert_eq!(placed.deployment_salt, Some(key));
    }
}
