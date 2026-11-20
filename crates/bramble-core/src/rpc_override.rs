//! Custom RPC endpoints, as the extension keeps them: one override per network, a preference
//! (`rpcOverrides`, in `public_store`'s preferences file), set and cleared by
//! `setRpcOverride` (`settings/src/preferences.ts`). It does not define a network: Mainnet and
//! Sepolia keep their chain ids, tokens and explorers, and the override is only tried before the
//! bundled endpoints (`networkRpcUrls`, `networks::with_rpc_override`).
//!
//! The extension does not probe `starknet_chainId` when an override is saved; its writes probe
//! every endpoint before signing (`withRpcWrite`). Mobile's send plan probes the chain at review
//! and again before submit, against Krusty's chain id rather than anything the catalog or an
//! endpoint says, and `NetworkInfo` asks the host to verify the chain before trusting any reply.
//!
//! An endpoint can carry an API key in its path or query, so no error here names it.

use std::sync::Arc;

use wallet_core::js::num::js_trim;
use wallet_core::nostr::relay_url::is_custom_rpc_url;

use crate::networks::{self, NetworkId, NetworkInfo};
use crate::portfolio::tokens::TokenPreferences;
use crate::wallet::WalletCore;
use crate::CoreError;

/// Whether `value` is an RPC endpoint mobile may store: the core's `is_custom_rpc_url` (the
/// extension's `isCustomRpcUrl`) over `https:` only. The extension also accepts `http:` on
/// `127.0.0.1` and `localhost`; mobile refuses it, as it does `ws:` relays, because App Transport
/// Security and `usesCleartextTraffic=false` block cleartext, loopback included.
pub(crate) fn is_rpc_url(value: &str) -> bool {
    value.starts_with("https://") && is_custom_rpc_url(value)
}

/// `setRpcOverride`'s input rule: trimmed as JavaScript trims; empty clears (`Ok(None)`); else a
/// URL `is_rpc_url` accepts in exactly the spelling given, or "Invalid RPC endpoint". The
/// extension's form first rewrites a typed URL to the parser's spelling (`canonicalServiceUrl`);
/// the core has no URL parser, so it refuses a spelling the parser would change instead.
fn rpc_override_input(raw: &str) -> Result<Option<String>, CoreError> {
    let trimmed = js_trim(raw);
    if trimmed.is_empty() {
        return Ok(None);
    }
    let refuse = |reason: &str| CoreError::InvalidInput {
        reason: reason.into(),
    };
    if trimmed
        .get(..7)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("http://"))
    {
        return Err(refuse("mobile RPC endpoints must use https"));
    }
    if !is_rpc_url(trimmed) {
        return Err(refuse("Invalid RPC endpoint"));
    }
    Ok(Some(trimmed.to_string()))
}

#[uniffi::export]
impl WalletCore {
    /// The bundled networks, each with its RPC override (if any) tried before the bundled
    /// endpoints. The host takes its endpoints from here rather than `supported_networks`.
    /// Passive: never renews the idle window.
    pub fn networks(&self) -> Result<Vec<NetworkInfo>, CoreError> {
        self.with_session(false, |session| {
            let overrides = &session.payload.rpc_overrides;
            Ok(networks::all()
                .iter()
                .map(|info| {
                    let custom = overrides.get(info.network.catalog_id());
                    networks::with_rpc_override(info.network, custom.map(String::as_str))
                })
                .collect())
        })
    }

    /// `explorerFallbackAllowed`: whether `network`'s history may fall back to the third-party
    /// explorer. The stored choice in `preferences`, else only while the wallet holds no RPC
    /// override for `network` (nor dropped a refused one, `public_store::load_preferences`): a
    /// person who set their own endpoint chose that no third party sees their addresses. `false`
    /// while locked or with no wallet, so a failed lookup never discloses. Passive.
    pub fn explorer_fallback_allowed(
        &self,
        network: NetworkId,
        preferences: Arc<TokenPreferences>,
    ) -> bool {
        let id = network.catalog_id();
        self.with_session(false, |session| {
            let payload = &session.payload;
            Ok(
                payload.rpc_overrides.contains_key(id)
                    || payload.explorer_fallback_off.contains(id),
            )
        })
        .is_ok_and(|custom| preferences.explorer_fallback_allowed(network, custom))
    }

    /// `network`'s RPC override, for the settings screen; `None` when it uses the defaults.
    pub fn rpc_override(&self, network: NetworkId) -> Result<Option<String>, CoreError> {
        self.with_session(false, |session| {
            Ok(session
                .payload
                .rpc_overrides
                .get(network.catalog_id())
                .cloned())
        })
    }

    /// `setRpcOverride`: saves `rpc_url` as `network`'s endpoint, or, when it trims to empty,
    /// clears it, restoring the defaults. Returns what is stored. A refusal changes nothing.
    pub fn set_rpc_override(
        &self,
        network: NetworkId,
        rpc_url: String,
    ) -> Result<Option<String>, CoreError> {
        let url = rpc_override_input(&rpc_url)?;
        self.with_session(false, |session| {
            self.commit(session, |payload| {
                let key = network.catalog_id().to_string();
                match &url {
                    Some(url) => payload.rpc_overrides.insert(key, url.clone()),
                    None => payload.rpc_overrides.remove(&key),
                };
                Ok(())
            })
        })?;
        Ok(url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_override_is_trimmed_https_and_canonical() {
        for (raw, stored) in [
            ("", None),
            (" \u{feff}\t", None),
            ("https://rpc.example", Some("https://rpc.example")),
            (
                " https://rpc.example/v0_10/KEY \n",
                Some("https://rpc.example/v0_10/KEY"),
            ),
            (
                "https://rpc.example:8443/?key=a%20b",
                Some("https://rpc.example:8443/?key=a%20b"),
            ),
        ] {
            assert_eq!(rpc_override_input(raw).unwrap().as_deref(), stored, "{raw}");
        }
        for (raw, reason) in [
            (
                "http://127.0.0.1:5050/rpc",
                "mobile RPC endpoints must use https",
            ),
            (
                "HTTP://localhost:5050",
                "mobile RPC endpoints must use https",
            ),
            ("http://rpc.example", "mobile RPC endpoints must use https"),
            ("https://user:secret@rpc.example", "Invalid RPC endpoint"),
            ("https://rpc.example/#key", "Invalid RPC endpoint"),
            ("https://RPC.example/key", "Invalid RPC endpoint"),
            ("https://rpc.example:443/key", "Invalid RPC endpoint"),
            ("https://rpc.example/a b", "Invalid RPC endpoint"),
            ("https://rpc.example/a\\b", "Invalid RPC endpoint"),
            ("wss://rpc.example", "Invalid RPC endpoint"),
            ("rpc.example", "Invalid RPC endpoint"),
            ("httpé://rpc.example", "Invalid RPC endpoint"),
        ] {
            match rpc_override_input(raw) {
                Err(CoreError::InvalidInput { reason: got }) => {
                    assert_eq!(got, reason, "{raw}");
                    assert!(!got.contains("rpc.example") && !got.contains("secret"));
                }
                other => panic!("{raw}: {other:?}"),
            }
        }
        let long = format!("https://rpc.example/{}", "k".repeat(2048));
        assert!(rpc_override_input(&long).is_err());
    }
}
