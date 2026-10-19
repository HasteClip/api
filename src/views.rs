//! Views and plays, counted in memory and written to the database every half
//! minute: one query for all of them, not one per view.

use std::collections::HashMap;
use std::sync::Mutex;

use sqlx::PgPool;

use crate::db;

#[derive(Default)]
pub struct Views {
    /// Views per clip since the last write. A clip that was only played
    /// (its video fetched) is here with 0: seen, but not a view.
    counts: Mutex<HashMap<String, i64>>,
}

impl Views {
    /// Someone opened the clip's page.
    pub fn page(&self, id: &str) {
        *self.counts.lock().unwrap().entry(id.to_owned()).or_default() += 1;
    }

    /// Someone (a browser, or Discord showing it in a chat) fetched the
    /// clip's video or thumbnail: it's still being watched.
    pub fn play(&self, id: &str) {
        self.counts.lock().unwrap().entry(id.to_owned()).or_default();
    }

    /// Write what's been counted. On failure the counts are put back, to try
    /// again next time.
    pub async fn write(&self, db: &PgPool) {
        let counts = std::mem::take(&mut *self.counts.lock().unwrap());
        if counts.is_empty() {
            return;
        }
        let (ids, views): (Vec<String>, Vec<i64>) = counts.iter().map(|(k, v)| (k.clone(), *v)).unzip();
        if let Err(e) = db::add_views(db, &ids, &views).await {
            tracing::warn!("couldn't write views: {e}");
            let mut now = self.counts.lock().unwrap();
            for (id, n) in counts {
                *now.entry(id).or_default() += n;
            }
        }
    }
}
