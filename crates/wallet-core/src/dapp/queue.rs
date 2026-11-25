//! The approval queue: requests from sites waiting for the person, with the extension's limits
//! (`apps/extension/src/background/approvals/{approvals,approval-limits,signature-waiters}.ts`).
//! What a card holds is the caller's (`T`); this owns who may queue, for how long, and how often.
//!
//! - A site holds at most `max_per_origin` approvals. At `max_pending` in all, a new site evicts
//!   the oldest approval of the site holding the most, and only from a site holding more than
//!   one, so no site can deny the queue to every other; otherwise the request is refused.
//! - An approval expires `ttl_ms` after it was queued. Expiry is swept on every call, so an
//!   expired approval is never seen or claimed.
//! - A request with a dedupe key (connect, switch-chain, watch-asset) joins the approval its site
//!   already has under that key, up to `max_waiters` requests on one approval.
//! - Reservations count requests still being prepared, so a site cannot outrun the caps with
//!   many slow requests at once.
//! - A fee estimate is written only while its revision is the newest, so a card is bound to the
//!   fee it shows.
//!
//! Sans-IO: the host passes its monotonic clock (`host::Clock::monotonic_ms`), holds the page's
//! replies, and after each call drains [`ApprovalQueue::take_dropped`] to answer every approval
//! that expired or was evicted as refused.

use crate::envelope::random;
use crate::Error;

/// The wallet's queue limits (`MAX_PENDING_APPROVALS`, `MAX_PENDING_APPROVALS_PER_ORIGIN`,
/// `APPROVAL_TTL_MS`, `MAX_WAITERS_PER_APPROVAL`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueLimits {
    pub max_pending: usize,
    pub max_per_origin: usize,
    pub ttl_ms: u64,
    pub max_waiters: usize,
}

/// An approval's id: 128 random bits in hex, never reused, so a decision made on a stale screen
/// can never land on a newer approval.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ApprovalId(String);

impl ApprovalId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn new() -> Result<Self, Error> {
        let bytes = random::<16>()?;
        Ok(Self(bytes.iter().map(|b| format!("{b:02x}")).collect()))
    }
}

/// Why a request was not queued. The page sees UNKNOWN_ERROR (163) either way, as in the
/// extension, which never tells a site why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// "Too many pending approvals for this site".
    SiteFull,
    /// "Too many pending approvals".
    QueueFull,
}

/// A queued approval, as the wallet's screens read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Approval<T> {
    pub id: ApprovalId,
    pub origin: String,
    pub queued_at: u64,
    pub payload: T,
}

/// A request's place in the queue: a new approval, or the one its site already had under the
/// same dedupe key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Queued {
    pub id: ApprovalId,
    pub joined: bool,
}

/// A request being prepared, counted against the caps until it is released.
#[derive(Debug)]
pub struct Reservation {
    origin: String,
}

/// A fee estimate's place in line: written only if no newer estimate began since.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeeRevision {
    id: ApprovalId,
    revision: u64,
}

#[derive(Debug)]
struct Entry<T> {
    approval: Approval<T>,
    dedupe: Option<String>,
    waiters: usize,
    fee_revision: u64,
}

#[derive(Debug)]
pub struct ApprovalQueue<T> {
    limits: QueueLimits,
    entries: Vec<Entry<T>>,
    reserved: Vec<String>,
    dropped: Vec<ApprovalId>,
}

impl<T> ApprovalQueue<T> {
    pub fn new(limits: QueueLimits) -> Self {
        Self {
            limits,
            entries: Vec::new(),
            reserved: Vec::new(),
            dropped: Vec::new(),
        }
    }

    /// The approvals that expired or were evicted since the last call, for the host to answer.
    pub fn take_dropped(&mut self) -> Vec<ApprovalId> {
        std::mem::take(&mut self.dropped)
    }

    fn sweep(&mut self, now: u64) {
        let ttl = self.limits.ttl_ms;
        let dropped = &mut self.dropped;
        self.entries.retain(|entry| {
            let live = now.saturating_sub(entry.approval.queued_at) < ttl;
            if !live {
                dropped.push(entry.approval.id.clone());
            }
            live
        });
    }

    fn position(&self, id: &ApprovalId) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| entry.approval.id == *id)
    }

    /// Every queued approval's origin, then `extra`'s (reservations), as `admissionView` lists them.
    fn origins<'a>(&'a self, extra: &'a [String]) -> impl Iterator<Item = &'a str> {
        self.entries
            .iter()
            .map(|entry| entry.approval.origin.as_str())
            .chain(extra.iter().map(String::as_str))
    }

    /// `approvalAdmissionRefusal` over the queued approvals plus `extra`.
    fn refusal(&self, origin: &str, extra: &[String]) -> Option<Refusal> {
        let total = self.origins(extra).count();
        let for_origin = self.origins(extra).filter(|held| *held == origin).count();
        if for_origin >= self.limits.max_per_origin {
            return Some(Refusal::SiteFull);
        }
        // At the cap a newcomer gets in only if some site holds more than one, whose oldest
        // approval it will evict (`approvalCapacityVictim(queued) === null` otherwise).
        if total >= self.limits.max_pending && (for_origin > 0 || self.largest_share(extra) < 2) {
            return Some(Refusal::QueueFull);
        }
        None
    }

    fn share(&self, origin: &str, extra: &[String]) -> usize {
        self.origins(extra).filter(|held| *held == origin).count()
    }

    fn largest_share(&self, extra: &[String]) -> usize {
        self.origins(extra)
            .map(|origin| self.share(origin, extra))
            .max()
            .unwrap_or(0)
    }

    /// `approvalCapacityVictim` over the queued approvals: at the cap, the oldest approval of the
    /// site holding the most, if that site holds more than one.
    fn victim(&self) -> Option<usize> {
        if self.entries.len() < self.limits.max_pending {
            return None;
        }
        let largest = self.largest_share(&[]);
        if largest < 2 {
            return None;
        }
        self.entries
            .iter()
            .position(|entry| self.share(&entry.approval.origin, &[]) == largest)
    }

    /// `canAdmit`: whether `origin` could queue now, reservations included.
    pub fn can_admit(&mut self, origin: &str, now: u64) -> Result<(), Refusal> {
        self.sweep(now);
        self.refusal(origin, &self.reserved).map_or(Ok(()), Err)
    }

    /// `reserve`: hold a place for a request still being prepared. Release it with
    /// [`Self::release`] once the request is queued or abandoned.
    pub fn reserve(&mut self, origin: &str, now: u64) -> Result<Reservation, Refusal> {
        self.can_admit(origin, now)?;
        self.reserved.push(origin.to_owned());
        Ok(Reservation {
            origin: origin.to_owned(),
        })
    }

    pub fn release(&mut self, reservation: Reservation) {
        if let Some(at) = self
            .reserved
            .iter()
            .position(|held| *held == reservation.origin)
        {
            self.reserved.remove(at);
        }
    }

    /// Queues a request from `origin`, or joins the approval it already has under `dedupe`.
    /// `arrived_at`, when the request arrived before it was prepared, dates the approval no later
    /// than `now`. Reservations are not counted here: the request being queued holds one.
    pub fn enqueue(
        &mut self,
        origin: &str,
        dedupe: Option<String>,
        payload: T,
        now: u64,
        arrived_at: Option<u64>,
    ) -> Result<Result<Queued, Refusal>, Error> {
        self.sweep(now);
        if let Some(key) = &dedupe {
            let max_waiters = self.limits.max_waiters;
            if let Some(entry) = self
                .entries
                .iter_mut()
                .find(|entry| entry.approval.origin == origin && entry.dedupe.as_ref() == Some(key))
            {
                if entry.waiters >= max_waiters {
                    return Ok(Err(Refusal::QueueFull));
                }
                entry.waiters += 1;
                return Ok(Ok(Queued {
                    id: entry.approval.id.clone(),
                    joined: true,
                }));
            }
        }
        if let Some(refusal) = self.refusal(origin, &[]) {
            return Ok(Err(refusal));
        }
        let id = ApprovalId::new()?;
        if let Some(at) = self.victim() {
            let evicted = self.entries.remove(at);
            self.dropped.push(evicted.approval.id);
        }
        self.entries.push(Entry {
            approval: Approval {
                id: id.clone(),
                origin: origin.to_owned(),
                queued_at: arrived_at.map_or(now, |arrived| arrived.min(now)),
                payload,
            },
            dedupe,
            waiters: 1,
            fee_revision: 0,
        });
        Ok(Ok(Queued { id, joined: false }))
    }

    /// The approval, if it is still queued.
    pub fn get(&mut self, id: &ApprovalId, now: u64) -> Option<&Approval<T>> {
        self.sweep(now);
        let at = self.position(id)?;
        Some(&self.entries[at].approval)
    }

    /// Every queued approval, oldest first.
    pub fn list(&mut self, now: u64) -> Vec<&Approval<T>> {
        self.sweep(now);
        self.entries.iter().map(|entry| &entry.approval).collect()
    }

    /// `claim`: takes the approval out for its decision, once. The host then answers every
    /// request waiting on it.
    pub fn claim(&mut self, id: &ApprovalId, now: u64) -> Option<Approval<T>> {
        self.sweep(now);
        let at = self.position(id)?;
        Some(self.entries.remove(at).approval)
    }

    /// `beginFeeEstimate`: a newer estimate begins; any older one in flight is now stale.
    pub fn begin_fee_estimate(&mut self, id: &ApprovalId, now: u64) -> Option<FeeRevision> {
        self.sweep(now);
        let at = self.position(id)?;
        let entry = &mut self.entries[at];
        entry.fee_revision += 1;
        Some(FeeRevision {
            id: id.clone(),
            revision: entry.fee_revision,
        })
    }

    /// `updateFee`: applies `update` only while `revision` is the newest estimate.
    pub fn update_fee(
        &mut self,
        revision: &FeeRevision,
        now: u64,
        update: impl FnOnce(&mut T),
    ) -> bool {
        self.sweep(now);
        let Some(at) = self.position(&revision.id) else {
            return false;
        };
        let entry = &mut self.entries[at];
        if entry.fee_revision != revision.revision {
            return false;
        }
        update(&mut entry.approval.payload);
        true
    }
}

#[cfg(test)]
#[path = "queue_tests.rs"]
mod tests;
