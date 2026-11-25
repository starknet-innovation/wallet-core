//! When the wallet first saw each site: the extension's
//! `packages/features/permissions/src/origin-history.ts`. Plain data outside the vault, one JSON
//! object of origin to first-seen milliseconds, kept to the newest `max_origins`. Times are the
//! host's wall clock, as `PriceBook`'s are: a wrong clock only mislabels a site's age.

use serde_json::{Map, Value};

use super::blocklist::origin_host;
use crate::host::Storage;
use crate::Error;

/// `createOriginHistoryStore` over the wallet's file and cap.
pub struct OriginHistory<'a> {
    pub storage: &'a dyn Storage,
    pub file: &'static str,
    pub max_origins: usize,
}

impl OriginHistory<'_> {
    // `readMap`: a missing or malformed file reads as empty; an entry that is not a wallet origin
    // with a whole, non-negative, safe time is dropped.
    fn read(&self) -> Result<Vec<(String, f64)>, Error> {
        let Some(bytes) = self.storage.read(self.file)? else {
            return Ok(Vec::new());
        };
        let Ok(Value::Object(map)) = serde_json::from_slice::<Value>(&bytes) else {
            return Ok(Vec::new());
        };
        Ok(map
            .into_iter()
            .filter_map(|(origin, at)| {
                let at = at.as_f64().filter(|at| is_timestamp(*at))?;
                origin_host(&origin).is_some().then_some((origin, at))
            })
            .collect())
    }

    /// When `origin` was first seen, or `None` for never.
    pub fn first_seen(&self, origin: &str) -> Result<Option<f64>, Error> {
        Ok(self
            .read()?
            .into_iter()
            .find(|(seen, _)| seen == origin)
            .map(|(_, at)| at))
    }

    /// `record`: the first-seen time, written now if there was none. An origin that is not a
    /// wallet origin is not kept, and reads as seen now.
    pub fn record(&self, origin: &str, now_ms: f64) -> Result<f64, Error> {
        if origin_host(origin).is_none() || !is_timestamp(now_ms) {
            return Ok(now_ms);
        }
        let mut seen = self.read()?;
        if let Some((_, at)) = seen.iter().find(|(known, _)| known == origin) {
            return Ok(*at);
        }
        seen.push((origin.to_owned(), now_ms));
        // `pruneOldest`: the newest `max_origins` stay.
        if seen.len() > self.max_origins {
            seen.sort_by(|a, b| b.1.total_cmp(&a.1));
            seen.truncate(self.max_origins);
        }
        let map: Map<String, Value> = seen
            .into_iter()
            // A safe integer, so whole in `u64` and written without a `.0`.
            .map(|(origin, at)| (origin, Value::from(at as u64)))
            .collect();
        let body = serde_json::to_vec(&map).map_err(|_| Error::storage("origin history"))?;
        self.storage.write(self.file, &body)?;
        Ok(now_ms)
    }
}

// `isTimestamp`: `Number.isSafeInteger(value) && value >= 0`.
fn is_timestamp(value: f64) -> bool {
    value.fract() == 0.0 && (0.0..=9_007_199_254_740_991.0).contains(&value)
}

#[cfg(test)]
#[path = "origin_history_tests.rs"]
mod tests;
