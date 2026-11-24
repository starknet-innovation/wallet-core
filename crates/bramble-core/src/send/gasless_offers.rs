//! Where the gasless path starts: the offer round for a reviewed `TransferPlan`
//! (`reviewGaslessOffers`), over Bramble's curated tokens and the wallet's own prices. The rest
//! of the path is `gasless.rs`.

use std::sync::{Arc, Mutex};

use wallet_core::gasless::{
    GaslessAccount, GaslessHeldToken, GaslessSession, OfferRound, RoundStep,
};
use wallet_core::send::u256::U256;

use super::gasless::{
    offer_step, reference, strk_address, GaslessFee, GaslessOffers, Stage, POLICY,
};
use super::plan::TransferPlan;
use super::wallet_ext::Use;
use crate::activity::with_log;
use crate::felt;
use crate::gasless::gasless_v2_classes;
use crate::portfolio::prices::PriceView;
use crate::portfolio::registry::known_tokens;
use crate::wallet_accounts::signing_record;

#[uniffi::export]
impl TransferPlan {
    /// The gasless choice for this reviewed transfer (`reviewGaslessOffers`): `None` unless the
    /// review found its STRK short of an FRI fee, the network's route is on, and the account's
    /// class signs SNIP-9 v2 and is deployed. `prices` is the wallet's own fresh USD answer for
    /// STRK and its curated tokens (`PriceBook::plan`); `balances` the host's last read of those
    /// tokens (base units; a token missing or `None` is unread). The offers are curated tokens
    /// other than STRK, with Bramble's labels and decimals, never the paymaster's. `now_ms` is
    /// the host's wall clock.
    pub fn gasless_offers(
        self: Arc<Self>,
        prices: Option<PriceView>,
        balances: Vec<GaslessBalance>,
        now_ms: u64,
    ) -> Option<GaslessOffers> {
        let core = self.core.upgrade()?;
        let basis = self.gasless_basis()?;
        // A signed execution still able to run is pending: a second one could pay twice.
        let pending = with_log(&core.data_dir, |log| {
            log.relay_pending(&self.account_id, now_ms / 1000)
        });
        if pending {
            return None;
        }
        let acting = core.plan_account_for(&self.account_id, Use::Sign).ok()?;
        let (family, class_hash) = core
            .with_session(false, |session| {
                let record = signing_record(&session.payload, &self.account_id)?;
                Ok((record.family(), record.class_felt()))
            })
            .ok()?;
        let account = GaslessAccount {
            network: self.network,
            address: acting.account.address,
            family,
            class_hash,
            deployed: basis.deployed,
            deployment: None,
        };
        let calls = [basis.call.clone()];
        let session =
            GaslessSession::open(&POLICY, &account, &calls, &gasless_v2_classes()).ok()?;
        let strk = strk_address(self.network)?;
        let held = known_tokens(self.network)
            .into_iter()
            .filter_map(|known| {
                let address = felt::parse(&known.address).filter(|address| *address != strk)?;
                let balance = balances
                    .iter()
                    .find(|b| felt::parse(&b.token_address) == Some(address))
                    .and_then(|b| b.balance.as_deref().and_then(U256::from_decimal));
                let reference = prices
                    .as_ref()
                    .and_then(|p| reference(&self, &basis, p, address, now_ms));
                let token = GaslessHeldToken {
                    address,
                    balance,
                    decimals: known.decimals,
                    label: known.symbol,
                };
                Some((token, reference))
            })
            .collect();
        let (round, step) = OfferRound::start(session.clone(), held, strk, now_ms / 1000);
        let stage = match &step {
            RoundStep::Done(offers) => Stage::Offered(offers.clone()),
            RoundStep::Post { .. } => Stage::Offering(Box::new(round)),
        };
        let fee = Arc::new(GaslessFee {
            plan: self.clone(),
            account: acting.account,
            session,
            basis,
            stage: Mutex::new(stage),
            journaled: Mutex::new(None),
        });
        Some(GaslessOffers {
            fee,
            step: offer_step(step),
        })
    }
}

/// The host's last read of one curated token's balance (`getTokenBalances`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct GaslessBalance {
    pub token_address: String,
    /// Base units as decimal digits; `None` when the read failed.
    pub balance: Option<String>,
}
