//! The live smoke harness's environment (`strk20_live_smoke`): every variable, read and checked
//! before anything is sent. A refusal names a variable, never its value.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

use std::fmt;

use bramble_core::catalogs::account_address::{argent_shape, resolve_class};
use starknet_wallet_core::accounts::{family_address, AccountFamily, Ladder};
use starknet_wallet_core::nostr::relay_url::is_custom_rpc_url;
use starknet_wallet_core::privacy::config::{is_service_url, Route};
use starknet_wallet_core::privacy::proof::ReviewedAction;
use starknet_wallet_core::{account_keypair, felt, Felt, NetworkId, SecretFelt, Zeroizing};

pub const NETWORK: &str = "STRK20_SMOKE_NETWORK";
pub const MAINNET_ACK: &str = "STRK20_SMOKE_MAINNET_ACK";
pub const POOL: &str = "STRK20_SMOKE_POOL";
pub const DISCOVERY_URL: &str = "STRK20_SMOKE_DISCOVERY_URL";
pub const PROVER_URL: &str = "STRK20_SMOKE_PROVER_URL";
pub const RPC_URL: &str = "STRK20_SMOKE_RPC_URL";
pub const MNEMONIC: &str = "STRK20_SMOKE_MNEMONIC";
pub const ACCOUNT_INDEX: &str = "STRK20_SMOKE_ACCOUNT_INDEX";
pub const LADDER: &str = "STRK20_SMOKE_LADDER";
pub const FAMILY: &str = "STRK20_SMOKE_FAMILY";
pub const ADDRESS: &str = "STRK20_SMOKE_ADDRESS";
pub const JOB: &str = "STRK20_SMOKE_JOB";
pub const TOKEN: &str = "STRK20_SMOKE_TOKEN";
pub const AMOUNT: &str = "STRK20_SMOKE_AMOUNT";
pub const SEND_VIEWING_KEY: &str = "STRK20_SMOKE_SEND_VIEWING_KEY";

/// `STRK20_SMOKE_MAINNET_ACK`'s one accepted value.
pub const MAINNET_ACK_TEXT: &str = "I am using a named dev wallet on mainnet";
/// `STRK20_SMOKE_SEND_VIEWING_KEY`'s one accepted value.
pub const SEND_TEXT: &str = "yes, send";

/// The environment, as a lookup: the live test reads the process's, the offline tests a map.
pub type Env<'a> = &'a dyn Fn(&str) -> Option<String>;

/// Why the harness stopped before (or instead of) running the job. Names a variable, never its
/// value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    Missing(&'static str),
    Invalid(&'static str),
    MainnetNotAcknowledged,
    /// The address is not the one the phrase, index, ladder and family derive.
    AddressMismatch,
    /// `STRK20_SMOKE_SEND_VIEWING_KEY` is not `yes, send`.
    NotConfirmed,
    ProvingBlockUnavailable,
    /// `ProofJob::new` refused the route.
    RouteRefused,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(name) => write!(f, "{name} is not set"),
            Self::Invalid(name) => write!(f, "{name} is not valid"),
            Self::MainnetNotAcknowledged => {
                write!(f, "mainnet needs {MAINNET_ACK}=\"{MAINNET_ACK_TEXT}\"")
            }
            Self::AddressMismatch => write!(
                f,
                "{ADDRESS} is not the address {MNEMONIC}, {ACCOUNT_INDEX}, {LADDER} and {FAMILY} \
                 derive"
            ),
            Self::NotConfirmed => write!(
                f,
                "nothing sent: set {SEND_VIEWING_KEY}=\"{SEND_TEXT}\" to send the viewing key to \
                 the hosts above"
            ),
            Self::ProvingBlockUnavailable => write!(f, "the node did not return a block number"),
            Self::RouteRefused => write!(f, "the core refused the route"),
        }
    }
}

/// A validated configuration. Holds the account key; never printed.
pub struct Config {
    pub network: NetworkId,
    pub route: Route,
    pub prover_url: String,
    pub rpc_url: String,
    pub account: Felt,
    pub(crate) public_key: Felt,
    pub(crate) key: SecretFelt,
    pub actions: Vec<ReviewedAction>,
}

fn required(env: Env, name: &'static str) -> Result<String, Refusal> {
    env(name)
        .filter(|value| !value.trim().is_empty())
        .ok_or(Refusal::Missing(name))
}

fn non_zero(env: Env, name: &'static str) -> Result<Felt, Refusal> {
    felt::parse_non_zero(&required(env, name)?).ok_or(Refusal::Invalid(name))
}

fn url(env: Env, name: &'static str, valid: fn(&str) -> bool) -> Result<String, Refusal> {
    Some(required(env, name)?)
        .filter(|url| valid(url))
        .ok_or(Refusal::Invalid(name))
}

/// Reads and checks every variable but the send confirmation; sends nothing.
pub fn config(env: Env) -> Result<Config, Refusal> {
    let network =
        NetworkId::from_catalog_id(&required(env, NETWORK)?).ok_or(Refusal::Invalid(NETWORK))?;
    if network == NetworkId::Mainnet && env(MAINNET_ACK).as_deref() != Some(MAINNET_ACK_TEXT) {
        return Err(Refusal::MainnetNotAcknowledged);
    }
    let pool_address = required(env, POOL)?;
    let pool = felt::parse_non_zero(&pool_address).ok_or(Refusal::Invalid(POOL))?;
    let discovery_url = url(env, DISCOVERY_URL, is_service_url)?;
    let prover_url = url(env, PROVER_URL, is_service_url)?;
    // A custom RPC URL, narrowed to HTTPS (curl runs `--proto =https`).
    let rpc_url = url(env, RPC_URL, |url| {
        is_custom_rpc_url(url) && url.starts_with("https://")
    })?;

    let phrase = Zeroizing::new(required(env, MNEMONIC)?);
    let index = match env(ACCOUNT_INDEX) {
        None => 0,
        Some(index) => index.parse().map_err(|_| Refusal::Invalid(ACCOUNT_INDEX))?,
    };
    let ladder = match env(LADDER).as_deref() {
        None | Some("account") => Ladder::Account,
        Some("address") => Ladder::Address,
        Some("argentx") => Ladder::ArgentX,
        Some(_) => return Err(Refusal::Invalid(LADDER)),
    };
    let family = match env(FAMILY).as_deref() {
        None | Some("openzeppelin") => AccountFamily::OpenZeppelin,
        Some("argent") => AccountFamily::Argent,
        Some(_) => return Err(Refusal::Invalid(FAMILY)),
    };
    // The wallet's derivation: `VaultPayload::signing_key` for a phrase account.
    let (pair, _) =
        account_keypair(&phrase, None, ladder, index).map_err(|_| Refusal::Invalid(MNEMONIC))?;
    drop(phrase);
    let public_key = felt::parse(&pair.public_key_x).ok_or(Refusal::Invalid(MNEMONIC))?;
    // The wallet's placement: `account_address::family_address` (salt 0 for OpenZeppelin, the
    // public key otherwise) at the class the catalog resolves.
    let class = resolve_class(family, None, network).map_err(|_| Refusal::Invalid(FAMILY))?;
    let shape = argent_shape(family, &class).map_err(|_| Refusal::Invalid(FAMILY))?;
    let salt = match family {
        AccountFamily::OpenZeppelin => Felt::ZERO,
        _ => public_key,
    };
    let derived = family_address(family, &class, shape, &salt, &public_key)
        .map_err(|_| Refusal::Invalid(FAMILY))?;
    let account = non_zero(env, ADDRESS)?;
    if account != derived {
        return Err(Refusal::AddressMismatch);
    }

    let deposit = || -> Result<ReviewedAction, Refusal> {
        let amount = Felt::from_dec_str(&required(env, AMOUNT)?)
            .ok()
            .filter(|amount| *amount != Felt::ZERO)
            .ok_or(Refusal::Invalid(AMOUNT))?;
        Ok(ReviewedAction::Deposit {
            token: non_zero(env, TOKEN)?,
            amount,
            recipient: None,
        })
    };
    let actions = match required(env, JOB)?.as_str() {
        "register" => vec![ReviewedAction::Register],
        "register-deposit" => vec![ReviewedAction::Register, deposit()?],
        "deposit" => vec![deposit()?],
        _ => return Err(Refusal::Invalid(JOB)),
    };
    Ok(Config {
        network,
        route: Route {
            network,
            ohttp: false,
            discovery_url,
            pool_address,
            pool,
        },
        prover_url,
        rpc_url,
        account,
        public_key,
        key: pair.private_key,
        actions,
    })
}
