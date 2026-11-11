//! Bramble's class rules for placing a key under an account family, over
//! `starknet_wallet_core::accounts::family_address` (the extension's
//! `deriveAccountContractAddress`): which class an account of each family has (`resolve_class`)
//! and the Argent constructor layout the catalog gives that class (`argent_shape`).
//!
//! The core places any non-zero class it is given, so every class reaches it through
//! `resolve_class`: that is what refuses an Argent or Braavos class the catalog does not list.

use starknet_wallet_core::accounts::{ArgentLayout, ArgentShape};
use starknet_wallet_core::{felt, Felt, NetworkId};

use super::accounts::{catalog, class_entry, deployment_classes, AccountFamily};
use crate::{CoreError, OZ_ACCOUNT_VERSION};

fn invalid(reason: &str) -> CoreError {
    CoreError::InvalidInput {
        reason: reason.into(),
    }
}

fn unknown_class() -> CoreError {
    CoreError::Kms {
        operation: "class hash".into(),
    }
}

/// The OpenZeppelin account class Bramble deploys on `network`: Krusty's manifest at
/// `OZ_ACCOUNT_VERSION`.
fn oz_class_hash(network: NetworkId) -> Result<Felt, CoreError> {
    let class_hash = starknet_wallet_core::oz_account_class_hash(network, OZ_ACCOUNT_VERSION)?;
    felt::parse(&class_hash).ok_or_else(unknown_class)
}

/// Resolves the class an account of `family` derives from: Krusty's per-network OZ class; the
/// class this wallet creates for Argent when none is named; a named deployment class otherwise.
/// Braavos has no default, since this wallet never creates one.
pub fn resolve_class(
    family: AccountFamily,
    class_hash: Option<&Felt>,
    network: NetworkId,
) -> Result<Felt, CoreError> {
    match (family, class_hash) {
        (AccountFamily::OpenZeppelin, None) => oz_class_hash(network),
        (AccountFamily::OpenZeppelin, Some(_)) => Err(invalid("unknown account class")),
        (AccountFamily::Argent, None) => Ok(catalog().argent_default),
        (AccountFamily::Braavos, None) => Err(invalid("a Braavos account must name its class")),
        (_, Some(class_hash)) => deployment_classes(family)
            .find(|entry| entry.class_hash == *class_hash)
            .map(|entry| entry.class_hash)
            .ok_or_else(|| invalid("unknown account class")),
    }
}

/// The constructor layout the core places a `family` account at a resolved `class_hash` with:
/// for Argent, the layout the extension's table (`constructorShape`) gives the class, vouched
/// for as `Unlisted` only for Xverse's build, which Krusty does not list; none otherwise. A
/// Braavos class must be the table's `public-key` constructor.
pub fn argent_shape(
    family: AccountFamily,
    class_hash: &Felt,
) -> Result<Option<ArgentShape>, CoreError> {
    if family == AccountFamily::OpenZeppelin {
        return Ok(None);
    }
    let shape = class_entry(class_hash)
        .filter(|entry| entry.family == family)
        .map(|entry| entry.constructor_shape.as_str())
        .ok_or_else(unknown_class)?;
    if family == AccountFamily::Braavos {
        return if shape == "public-key" {
            Ok(None)
        } else {
            Err(unknown_class())
        };
    }
    let layout = ArgentLayout::from_wire(shape).ok_or_else(unknown_class)?;
    Ok(Some(if *class_hash == catalog().argent_xverse {
        ArgentShape::Unlisted(layout)
    } else {
        ArgentShape::Listed(layout)
    }))
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
    }

    /// Mobile's `the_oz_class_is_the_pinned_release_on_each_network` (`kms.rs`).
    #[test]
    fn the_oz_class_is_the_pinned_release_on_each_network() {
        for network in [NetworkId::Mainnet, NetworkId::Sepolia] {
            let pinned =
                starknet_wallet_core::oz_account_class_hash(network, OZ_ACCOUNT_VERSION).unwrap();
            assert_eq!(
                felt::canonical(
                    &resolve_class(AccountFamily::OpenZeppelin, None, network).unwrap()
                ),
                pinned
            );
        }
    }
}
