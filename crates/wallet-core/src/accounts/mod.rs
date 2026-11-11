//! Where an account lives: which key tree its index walks (`Ladder`), which contract family it
//! is (`AccountFamily`), and the counterfactual address a public key gets under one class and
//! salt (`family_address`), as the extension's `deriveAccountContractAddress`.
//!
//! Which class and salt to use is the wallet's choice and comes in as an argument: there is no
//! default class and no default salt here. Krusty builds every constructor calldata and address.
//! For Argent the caller also names the constructor layout its own reviewed table gives, and the
//! two must agree: a disagreement would be a plausible address that matches nothing, so it fails
//! closed. A class Krusty does not list is refused unless the caller vouches for it with
//! `ArgentShape::Unlisted`.

pub mod advisory;

use krusty_kms::{
    AccountClass, ArgentAccount, ArgentConstructorLayout, BraavosAccount, OpenZeppelinAccount,
    SaltPolicy,
};
use starknet_types_core::felt::Felt;

use crate::Error;

/// Which key tree an account's index walks (the extension's issue 505). Each wallet app steps a
/// different field, so one index names a different key on each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Ladder {
    /// `m/44'/9004'/index'/0/0`: `starknet-bip44` on the hardened account field.
    Account,
    /// Braavos's ladder, `m/44'/9004'/0'/0/index`: `starknet-bip44` on the address field.
    Address,
    /// Ready (Argent X), `argentx-bip44`: the Ethereum key at `m/44'/60'/0'/0/0` seeds
    /// `m/44'/9004'/0'/0/index`. It takes no BIP-39 passphrase.
    ArgentX,
}

impl Ladder {
    /// The extension's derivation kind, as its records and selection keys spell it.
    pub fn kind(self) -> &'static str {
        match self {
            Self::Account | Self::Address => "starknet-bip44",
            Self::ArgentX => "argentx-bip44",
        }
    }

    /// The ladder `family`'s own wallet app walks.
    pub fn vendor(family: AccountFamily) -> Self {
        match family {
            AccountFamily::OpenZeppelin => Self::Account,
            AccountFamily::Argent => Self::ArgentX,
            AccountFamily::Braavos => Self::Address,
        }
    }

    /// Whether an account of `family` can sit on this ladder: the account ladder takes every
    /// family, and a vendor ladder takes only its own vendor's.
    pub fn fits(self, family: AccountFamily) -> bool {
        self == Self::Account || self == Self::vendor(family)
    }
}

/// The account contract families, as the extension's `AccountContractType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AccountFamily {
    OpenZeppelin,
    Argent,
    Braavos,
}

impl AccountFamily {
    /// The extension's wire spelling, used in stored records and selection keys.
    pub fn wire(self) -> &'static str {
        match self {
            Self::OpenZeppelin => "openzeppelin",
            Self::Argent => "argent",
            Self::Braavos => "braavos",
        }
    }

    pub fn from_wire(value: &str) -> Option<Self> {
        [Self::OpenZeppelin, Self::Argent, Self::Braavos]
            .into_iter()
            .find(|family| family.wire() == value)
    }
}

/// An Argent constructor layout, as the extension's class table (`constructorShape`) names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArgentLayout {
    /// `owner-guardian`: `constructor(owner: felt252, guardian: felt252)` (0.3.x).
    OwnerGuardian,
    /// `signer-owner-guardian-option`: `constructor(owner: Signer, guardian: Option<Signer>)`
    /// (0.4.0 and later).
    SignerOwnerGuardianOption,
}

impl ArgentLayout {
    pub fn wire(self) -> &'static str {
        match self {
            Self::OwnerGuardian => "owner-guardian",
            Self::SignerOwnerGuardianOption => "signer-owner-guardian-option",
        }
    }

    pub fn from_wire(value: &str) -> Option<Self> {
        [Self::OwnerGuardian, Self::SignerOwnerGuardianOption]
            .into_iter()
            .find(|layout| layout.wire() == value)
    }

    fn krusty(self) -> ArgentConstructorLayout {
        match self {
            Self::OwnerGuardian => ArgentConstructorLayout::OwnerGuardianFelts,
            Self::SignerOwnerGuardianOption => ArgentConstructorLayout::SignerWithOptionalGuardian,
        }
    }
}

/// The layout the caller's table gives an Argent class, and whether Krusty is expected to list
/// it. `Listed` must match Krusty's own layout; `Unlisted` is the caller vouching for a class
/// Krusty does not know (such as Xverse's Argent 0.4 build), and is refused for one it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgentShape {
    Listed(ArgentLayout),
    Unlisted(ArgentLayout),
}

fn argent(class_hash: Felt, shape: ArgentShape) -> Result<ArgentAccount, Error> {
    let layout = match (ArgentAccount::layout_for_class_hash(&class_hash), shape) {
        (Some(listed), ArgentShape::Listed(named)) if listed == named.krusty() => listed,
        (None, ArgentShape::Unlisted(named)) => named.krusty(),
        _ => return Err(Error::kms("class hash")),
    };
    Ok(ArgentAccount::with_class_hash_and_layout(
        class_hash, layout,
    ))
}

/// Krusty's preset for `family` at `class_hash`. `argent_shape` is required for Argent and
/// refused for the others.
fn preset(
    family: AccountFamily,
    class_hash: &Felt,
    argent_shape: Option<ArgentShape>,
) -> Result<Box<dyn AccountClass>, Error> {
    if *class_hash == Felt::ZERO {
        return Err(Error::invalid("unknown account class"));
    }
    Ok(match (family, argent_shape) {
        (AccountFamily::Argent, Some(shape)) => Box::new(argent(*class_hash, shape)?),
        (AccountFamily::OpenZeppelin, None) => {
            Box::new(OpenZeppelinAccount::from_class_hash(*class_hash))
        }
        (AccountFamily::Braavos, None) => Box::new(BraavosAccount::with_class_hash(*class_hash)),
        _ => {
            return Err(Error::invalid(
                "only an Argent account names a layout, and it must",
            ))
        }
    })
}

fn checked_public_key(public_key: &Felt) -> Result<&Felt, Error> {
    if *public_key == Felt::ZERO {
        return Err(Error::invalid("invalid public key"));
    }
    Ok(public_key)
}

/// The constructor calldata a `family` account at `class_hash` deploys with for `public_key`,
/// the same placement `family_address` derives.
pub fn constructor_calldata(
    family: AccountFamily,
    class_hash: &Felt,
    argent_shape: Option<ArgentShape>,
    public_key: &Felt,
) -> Result<Vec<Felt>, Error> {
    let public_key = checked_public_key(public_key)?;
    Ok(preset(family, class_hash, argent_shape)?.build_constructor_calldata(public_key))
}

/// The counterfactual address of `public_key` under `family` at `class_hash` with `salt`
/// (deployer zero). Class and salt are the wallet's choice: the extension uses `0x0` for
/// OpenZeppelin and the public key otherwise.
pub fn family_address(
    family: AccountFamily,
    class_hash: &Felt,
    argent_shape: Option<ArgentShape>,
    salt: &Felt,
    public_key: &Felt,
) -> Result<Felt, Error> {
    let public_key = checked_public_key(public_key)?;
    let address = preset(family, class_hash, argent_shape)?
        .calculate_address(public_key, SaltPolicy::Explicit(*salt))
        .map_err(|_| Error::kms("address"))?;
    if address == Felt::ZERO {
        return Err(Error::kms("address"));
    }
    Ok(address)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::send::sign::Deployment;

    const ARGENT_0_4: &str = "0x036078334509b514626504edc9fb252328d1a240e4e948bef8d0c08dff45927f";
    const XVERSE: &str = "0x0663fc01a0dbe1bacc4cd2a4c856eb9784b255a20988aa33d4d52b6fc20bd024";
    const SIGNER: ArgentLayout = ArgentLayout::SignerOwnerGuardianOption;

    fn felt(hex: &str) -> Felt {
        Felt::from_hex(hex).unwrap()
    }

    #[test]
    fn ladders_fit_their_vendors_and_the_account_ladder() {
        use AccountFamily::*;
        for family in [OpenZeppelin, Argent, Braavos] {
            assert!(Ladder::Account.fits(family));
            assert!(Ladder::vendor(family).fits(family));
            assert_eq!(AccountFamily::from_wire(family.wire()), Some(family));
        }
        assert!(!Ladder::Address.fits(Argent) && !Ladder::Address.fits(OpenZeppelin));
        assert!(!Ladder::ArgentX.fits(Braavos) && !Ladder::ArgentX.fits(OpenZeppelin));
        assert_eq!(Ladder::Address.kind(), Ladder::Account.kind());
        assert_eq!(AccountFamily::from_wire("Argent"), None);
        assert_eq!(ArgentLayout::from_wire(SIGNER.wire()), Some(SIGNER));
    }

    /// `deployment.ts`: Xverse's Argent 0.4 build deploys with Argent 0.4.0's calldata, at the
    /// address the scan derives for it, which is the check a deployment makes before it signs.
    #[test]
    fn xverse_deploys_with_argent_0_4_calldata_at_its_own_address() {
        let key = Felt::from(0x1234_5678_u64);
        let (xverse, default) = (felt(XVERSE), felt(ARGENT_0_4));
        let unlisted = Some(ArgentShape::Unlisted(SIGNER));
        let calldata =
            constructor_calldata(AccountFamily::Argent, &xverse, unlisted, &key).unwrap();
        let listed = Some(ArgentShape::Listed(SIGNER));
        assert_eq!(
            calldata,
            constructor_calldata(AccountFamily::Argent, &default, listed, &key).unwrap()
        );
        assert_eq!(calldata, vec![Felt::ZERO, key, Felt::ONE]);
        let address = family_address(AccountFamily::Argent, &xverse, unlisted, &key, &key).unwrap();
        let deployment = Deployment {
            class_hash: xverse,
            salt: key,
            constructor_calldata: calldata,
        };
        assert_eq!(deployment.address(), Some(address));
    }

    #[test]
    fn argent_layouts_fail_closed() {
        let key = Felt::from(7u64);
        let place = |class: &str, shape| {
            family_address(AccountFamily::Argent, &felt(class), shape, &key, &key)
        };
        let owner = ArgentLayout::OwnerGuardian;
        // Krusty lists 0.4.0 with the signer layout: naming another is a disagreement.
        assert!(place(ARGENT_0_4, Some(ArgentShape::Listed(SIGNER))).is_ok());
        assert!(place(ARGENT_0_4, Some(ArgentShape::Listed(owner))).is_err());
        // A listed class cannot be vouched for, and an unlisted one needs vouching.
        assert!(place(ARGENT_0_4, Some(ArgentShape::Unlisted(SIGNER))).is_err());
        assert!(place(XVERSE, Some(ArgentShape::Listed(SIGNER))).is_err());
        assert!(place(ARGENT_0_4, None).is_err());
        assert!(place("0x0", Some(ArgentShape::Listed(SIGNER))).is_err());
        let oz = |shape| family_address(AccountFamily::OpenZeppelin, &key, shape, &key, &key);
        assert!(oz(None).is_ok());
        assert!(oz(Some(ArgentShape::Listed(SIGNER))).is_err());
        assert!(family_address(AccountFamily::Braavos, &key, None, &key, &Felt::ZERO).is_err());
    }
}
