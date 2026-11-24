//! Totals per hour, for the charts on status.hasteclip.com: clips shared,
//! pages opened, videos played, apps signed up, uploads refused and shared
//! clips removed. Counted in memory and added to the hour's row every half
//! minute (with views), along with how many clips are up and how much they
//! take. Only how many: nothing about who, or which clip.

use std::sync::atomic::{AtomicI32, Ordering};

use sqlx::PgPool;

use crate::db;

#[derive(Clone, Copy)]
pub enum Event {
    /// A clip finished uploading and its link works.
    Share,
    /// Someone opened a clip's page.
    Open,
    /// Someone (or a chat showing it) started a clip's video.
    Play,
    SignUp,
    /// An upload wasn't a video a browser plays.
    Refused,
    /// A shared clip went: unshared, taken down or expired.
    Removed,
}

#[derive(Default)]
pub struct Hourly {
    counts: [AtomicI32; 6],
}

impl Hourly {
    pub fn count(&self, event: Event) {
        self.counts[event as usize].fetch_add(1, Ordering::Relaxed);
    }

    /// Add what's been counted to this hour's row (even nothing, so how
    /// many clips are up is noted every hour). On failure the counts are put
    /// back, to try again next time.
    pub async fn write(&self, db: &PgPool) {
        let n: [i32; 6] = std::array::from_fn(|i| self.counts[i].swap(0, Ordering::Relaxed));
        if let Err(e) = db::add_hourly(db, n).await {
            tracing::warn!("couldn't write the hour's totals: {e}");
            for (count, n) in self.counts.iter().zip(n) {
                count.fetch_add(n, Ordering::Relaxed);
            }
        }
    }
}
