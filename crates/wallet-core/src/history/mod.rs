//! Activity history. For now only what portfolio snapshots share with it: the fixed-UTC
//! timestamp rendering (`time`) and account-id validation (`explorer`); the rest of mobile's
//! `history/*` follows in plan step 2.4c.

pub mod explorer;
pub mod time;
