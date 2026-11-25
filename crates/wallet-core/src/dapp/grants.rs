//! Which accounts a site may see: the vault's origin grants and the extension's rules over them
//! (`packages/platform/vault/src/service/origin-grants.ts`, and `grantOrigin` in its
//! `service.ts`). Pure: the caller reads the grants from its vault, and writes back what these
//! return.

use serde::{Deserialize, Serialize};

use crate::js::num::utf16_cmp;

/// `OriginGrant`, as the vault stores it: a site and every account it has been approved for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OriginGrant {
    pub account_ids: Vec<String>,
    pub origin: String,
}

/// `grantedAccountIdsFor`: the account `origin` may see now, which is the active account if its
/// grant covers it, and nothing otherwise. A grant accumulates every account a site was ever
/// approved for, but the page only ever sees the one the person has selected, so a site can
/// never correlate two of their accounts.
pub fn granted_account<'a>(
    grants: &[OriginGrant],
    active_account_id: Option<&'a str>,
    origin: &str,
) -> Option<&'a str> {
    let active = active_account_id?;
    grants
        .iter()
        .find(|grant| grant.origin == origin)
        .filter(|grant| grant.account_ids.iter().any(|id| id == active))
        .map(|_| active)
}

/// `grantOrigin`'s write: `origin`'s grant, with `account_id` added once, moved to the end of the
/// list, as the extension rewrites it.
pub fn with_origin_grant(
    grants: &[OriginGrant],
    origin: &str,
    account_id: &str,
) -> Vec<OriginGrant> {
    let mut account_ids = grants
        .iter()
        .find(|grant| grant.origin == origin)
        .map(|grant| grant.account_ids.clone())
        .unwrap_or_default();
    if !account_ids.iter().any(|id| id == account_id) {
        account_ids.push(account_id.to_owned());
    }
    let mut next: Vec<OriginGrant> = grants
        .iter()
        .filter(|grant| grant.origin != origin)
        .cloned()
        .collect();
    next.push(OriginGrant {
        account_ids,
        origin: origin.to_owned(),
    });
    next
}

/// `withoutOriginAccess`: `origin`'s grant without `account_id`, or without the whole site when
/// none is named; a grant left with no account is dropped. `None` when nothing was granted, so
/// the caller can skip the write.
pub fn without_origin_access(
    grants: &[OriginGrant],
    origin: &str,
    account_id: Option<&str>,
) -> Option<Vec<OriginGrant>> {
    // Only the first grant for `origin` changes, as the extension's identity check has it.
    let at = grants.iter().position(|grant| grant.origin == origin)?;
    let grant = &grants[at];
    if let Some(account_id) = account_id {
        if !grant.account_ids.iter().any(|id| id == account_id) {
            return None;
        }
    }
    let remaining: Vec<String> = match account_id {
        None => Vec::new(),
        Some(account_id) => grant
            .account_ids
            .iter()
            .filter(|id| *id != account_id)
            .cloned()
            .collect(),
    };
    let mut next = grants.to_vec();
    if remaining.is_empty() {
        next.remove(at);
    } else {
        next[at].account_ids = remaining;
    }
    Some(next)
}

/// `connectedSitesFromVault`: every grant, whichever account is active, ordered by origin in
/// JavaScript string order (UTF-16 code units, never the locale), equal origins kept in place.
/// For the wallet's own permissions screen only; a site never sees this list.
pub fn connected_sites(grants: &[OriginGrant]) -> Vec<OriginGrant> {
    let mut sites = grants.to_vec();
    sites.sort_by(|left, right| utf16_cmp(&left.origin, &right.origin));
    sites
}

#[cfg(test)]
#[path = "grants_tests.rs"]
mod tests;
