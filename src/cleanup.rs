//! Removing clips (unshared, taken down, expired, or never finished
//! uploading), and the hourly sweep that finds the last two.

use std::sync::Arc;
use std::time::Duration;

use crate::App;
use crate::db::{self, Clip};

/// Delete a clip's files, then the clip. Files go first: if deleting one
/// fails, the clip stays so it can be tried again, and nothing lingers in
/// the bucket unnoticed.
pub async fn remove(app: &App, clip: &Clip) -> Result<(), String> {
    if let Some(upload_id) = &clip.upload_id {
        app.storage.abort_upload(&clip.video_key, upload_id).await?;
    }
    app.storage.delete(&clip.video_key).await?;
    if let Some(thumb) = &clip.thumb_key {
        app.storage.delete(thumb).await?;
    }
    db::delete_clip(&app.db, &clip.id).await.map_err(|e| e.to_string())
}

/// Write views every 30 seconds and sweep every hour, until the server stops.
pub fn spawn(app: Arc<App>) {
    let views = app.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(30));
        loop {
            tick.tick().await;
            views.views.write(&views.db).await;
        }
    });
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(60 * 60));
        loop {
            tick.tick().await;
            sweep(&app).await;
        }
    });
}

async fn sweep(app: &App) {
    let stale = db::stale_uploads(&app.db).await;
    let unwatched = match app.config.retention_days {
        Some(days) => db::unwatched(&app.db, days).await,
        None => Ok(Vec::new()),
    };
    let (stale, unwatched) = match (stale, unwatched) {
        (Ok(s), Ok(u)) => (s, u),
        (Err(e), _) | (_, Err(e)) => return tracing::warn!("sweep: {e}"),
    };
    let mut removed = 0;
    for clip in stale.iter().chain(&unwatched) {
        match remove(app, clip).await {
            Ok(()) => removed += 1,
            Err(e) => tracing::warn!(clip = clip.id, "sweep: couldn't remove: {e}"),
        }
    }
    if removed > 0 {
        tracing::info!(stale = stale.len(), unwatched = unwatched.len(), removed, "swept");
    }
}
