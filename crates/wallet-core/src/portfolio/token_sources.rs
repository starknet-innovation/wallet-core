//! Who added a tracked token, and so whether the review trusts it (audit 171): a token a site
//! added through `wallet_watchAsset` is recorded in `dappTokenIds` (`withUpsertedToken(..,
//! "dapp")`), and is not trusted unless it is exactly a curated token (`isTrustedToken` in
//! `transactions/src/routing/verification-context.ts`). The person adding it in the wallet is
//! what makes it trusted (`TokenPreferences::add`).

use super::model::TokenConfig;
use super::registry::{is_token_config, known_tokens, token_spam_reasons, with_token_id};
use super::tokens::{AddTokenOutcome, TokenPreferences};
use crate::catalog::network_info;
use crate::js::felt::same_felt;
use crate::Error;
use crate::NetworkId;

impl TokenPreferences<'_> {
    /// The tracked tokens on `network` the reviews trust (`trustedTokens`): every one the spam
    /// heuristic does not flag, except one a site added that is not exactly a curated token
    /// (`isCuratedToken`: address, symbol and decimals). Hidden tokens are trusted: hiding STRK
    /// changes nothing here.
    pub fn trusted(&self, network: NetworkId) -> Vec<TokenConfig> {
        let curated = known_tokens(self.catalogs.registry, network);
        let is_curated = |token: &TokenConfig| {
            curated.iter().any(|known| {
                same_felt(&known.address, &token.address)
                    && known.symbol == token.symbol
                    && known.decimals == token.decimals
            })
        };
        self.tracked(network)
            .into_iter()
            .filter(|tracked| !tracked.likely_spam)
            .map(|tracked| tracked.token)
            .filter(|token| !self.prefs.dapp.contains(&token.id) || is_curated(token))
            .collect()
    }

    /// A site's token, approved by the person (`applyWatchAssetApproval`): stored as the site's,
    /// and hidden when the spam heuristic flags it. An address already tracked on the network,
    /// under any id, is left as it is (`None`), so a site cannot replace the person's record or
    /// its trust.
    pub fn add_from_dapp(&mut self, token: TokenConfig) -> Result<Option<AddTokenOutcome>, Error> {
        let token = with_token_id(
            token.network,
            &token.address,
            &token.symbol,
            &token.name,
            token.decimals,
        );
        if !is_token_config(&token) {
            return Err(Error::invalid("Invalid token"));
        }
        if self.is_tracked(token.network, &token.address) {
            return Ok(None);
        }
        self.require_room(&token)?;
        let spam = !token_spam_reasons(self.catalogs.registry, token.clone()).is_empty();
        let prefs = &mut self.prefs;
        prefs.tokens.push(token.clone());
        prefs.dapp.push(token.id.clone());
        if spam && !prefs.hidden.contains(&token.id) {
            prefs.hidden.push(token.id.clone());
        }
        Ok(Some(AddTokenOutcome {
            token,
            hidden_as_spam: spam,
        }))
    }

    /// `requireRoomFor`: refuses one more added token on a network already holding the policy's
    /// `max_added_tokens`. Replacing a tracked address, or a token the network itself lists,
    /// never counts against it.
    pub(super) fn require_room(&self, token: &TokenConfig) -> Result<(), Error> {
        let network = token.network;
        let listed = network_info(self.catalogs.networks, network)
            .map(|info| info.tokens.as_slice())
            .unwrap_or_default();
        let is_default = |address: &str| listed.iter().any(|t| same_felt(&t.address, address));
        let added = self
            .prefs
            .tokens
            .iter()
            .filter(|t| t.network == network && !is_default(&t.address))
            .count();
        let max = self.policy.max_added_tokens;
        if !self.is_tracked(network, &token.address) && !is_default(&token.address) && added >= max
        {
            return Err(Error::invalid(&format!(
                "You can track up to {max} added tokens on a network. Remove one first."
            )));
        }
        Ok(())
    }

    /// Whether `address` is tracked on `network`, under any id.
    pub fn is_tracked(&self, network: NetworkId, address: &str) -> bool {
        self.prefs
            .tokens
            .iter()
            .any(|t| t.network == network && same_felt(&t.address, address))
    }
}

#[cfg(test)]
#[path = "token_sources_tests.rs"]
mod tests;
