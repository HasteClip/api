//! What status.hasteclip.com reads: `/readyz`, whether the server can do its
//! job right now, and `/stats.json`, the numbers and charts it shows.
//!
//! `/healthz` (in `pages.rs`) is Coolify's, and only says the server is up:
//! a database that's away for a minute shouldn't make Coolify replace a
//! server that's fine. `/readyz` asks the database and the bucket too.
//!
//! The numbers are totals of what's kept anyway (see the README's Privacy)
//! and the totals per hour in `hourly.rs`: nothing about anyone in
//! particular. Both answers are reused for a while, so asking a thousand
//! times a second costs what asking once does.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::App;
use crate::db;

const READY_FOR: Duration = Duration::from_secs(10);
const STATS_FOR: Duration = Duration::from_secs(60);
const DOWNLOADS_FOR: Duration = Duration::from_secs(60 * 60);
/// How long the database or the bucket may take to answer `/readyz`.
const CHECK_TIMEOUT: Duration = Duration::from_secs(5);
/// How far back the charts go.
const CHART_DAYS: i32 = 30;
/// The app's releases, whose downloads the status page shows.
const RELEASES: &str = "https://api.github.com/repos/hasteclip/hasteclip/releases?per_page=100";
const STATUS_PAGE: &str = "https://status.hasteclip.com";

/// The answers being reused, and when each was worked out.
#[derive(Default)]
pub struct Status {
    ready: Mutex<Option<(Instant, Result<(), &'static str>)>>,
    stats: Mutex<Option<(Instant, Arc<Stats>)>>,
    downloads: Mutex<Option<(Instant, Option<Vec<ReleaseDownloads>>)>>,
}

/// `/readyz`: `ok`, or 503 and what isn't answering. What went wrong in
/// detail goes to the log, not to whoever asked.
pub async fn ready(State(app): State<Arc<App>>) -> Response {
    let last = *app.status.ready.lock().unwrap();
    let answer = match last {
        Some((at, answer)) if at.elapsed() < READY_FOR => answer,
        _ => {
            let answer = check(&app).await;
            *app.status.ready.lock().unwrap() = Some((Instant::now(), answer));
            answer
        }
    };
    let (status, text) = match answer {
        Ok(()) => (StatusCode::OK, "ok"),
        Err(what) => (StatusCode::SERVICE_UNAVAILABLE, what),
    };
    (status, [(header::CACHE_CONTROL, "no-store")], text).into_response()
}

async fn check(app: &App) -> Result<(), &'static str> {
    let (db, bucket) = tokio::join!(
        tokio::time::timeout(CHECK_TIMEOUT, db::ping(&app.db)),
        tokio::time::timeout(CHECK_TIMEOUT, app.storage.reachable()),
    );
    let db = match db {
        Ok(r) => r.map_err(|e| e.to_string()),
        Err(_) => Err("no answer in time".into()),
    };
    let bucket = bucket.unwrap_or_else(|_| Err("no answer in time".into()));
    if let Err(e) = &db {
        tracing::warn!("readyz: database: {e}");
    }
    if let Err(e) = &bucket {
        tracing::warn!("readyz: bucket: {e}");
    }
    match (db.is_ok(), bucket.is_ok()) {
        (true, true) => Ok(()),
        (false, true) => Err("the database isn't answering"),
        (true, false) => Err("the bucket isn't answering"),
        (false, false) => Err("the database and the bucket aren't answering"),
    }
}

/// `/stats`, the page these numbers were on: they're on the status page now.
pub async fn stats_page() -> Redirect {
    Redirect::permanent(STATUS_PAGE)
}

/// What `/stats.json` answers.
#[derive(Serialize)]
struct Stats {
    updated: String,
    totals: Totals,
    /// The most shared games.
    games: Vec<Game>,
    /// Every hour of the last 30 days that the server was up for, oldest
    /// first. An hour that isn't here had nothing counted.
    hours: Vec<Hour>,
    /// The app's releases, newest first, and how many times each was
    /// downloaded to install. `null` until GitHub has answered once.
    releases: Option<Vec<ReleaseDownloads>>,
}

#[derive(Serialize)]
struct Totals {
    clips: i64,
    clips_ever: i64,
    views: i64,
    bytes: i64,
    duration_ms: i64,
    this_week: i64,
    apps: i64,
    /// When `clips_ever` and the hours began counting.
    counted_since: Option<String>,
}

#[derive(Serialize)]
struct Game {
    name: String,
    clips: i64,
}

#[derive(Serialize)]
struct Hour {
    hour: String,
    shares: i32,
    opens: i32,
    plays: i32,
    sign_ups: i32,
    refused: i32,
    removed: i32,
    clips: i32,
    bytes: i64,
}

#[derive(Clone, Serialize)]
struct ReleaseDownloads {
    tag: String,
    published: Option<String>,
    downloads: i64,
}

/// `/stats.json`.
pub async fn stats(State(app): State<Arc<App>>) -> Response {
    let last = app.status.stats.lock().unwrap().clone();
    let stats = match last {
        Some((at, stats)) if at.elapsed() < STATS_FOR => stats,
        _ => match count(&app).await {
            Ok(stats) => {
                let stats = Arc::new(stats);
                *app.status.stats.lock().unwrap() = Some((Instant::now(), stats.clone()));
                stats
            }
            Err(e) => {
                tracing::error!("stats: {e}");
                return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": "the database" })))
                    .into_response();
            }
        },
    };
    let max_age = format!("public, max-age={}", STATS_FOR.as_secs());
    ([(header::CACHE_CONTROL, max_age)], Json(&*stats)).into_response()
}

async fn count(app: &App) -> sqlx::Result<Stats> {
    let (totals, games, hours, releases) = tokio::join!(
        db::totals(&app.db),
        db::top_games(&app.db, 5, 3),
        db::hours(&app.db, CHART_DAYS),
        downloads(app),
    );
    let t = totals?;
    Ok(Stats {
        updated: rfc3339(OffsetDateTime::now_utc()),
        totals: Totals {
            clips: t.clips,
            clips_ever: t.clips_ever,
            views: t.views,
            bytes: t.bytes,
            duration_ms: t.duration_ms,
            this_week: t.this_week,
            apps: t.apps,
            counted_since: t.counted_since.map(rfc3339),
        },
        games: games?.into_iter().map(|(name, clips)| Game { name, clips }).collect(),
        hours: hours?
            .into_iter()
            .map(|h| Hour {
                hour: rfc3339(h.hour),
                shares: h.shares,
                opens: h.opens,
                plays: h.plays,
                sign_ups: h.sign_ups,
                refused: h.refused,
                removed: h.removed,
                clips: h.clips,
                bytes: h.bytes,
            })
            .collect(),
        releases,
    })
}

fn rfc3339(t: OffsetDateTime) -> String {
    t.format(&Rfc3339).unwrap_or_default()
}

/// The app's downloads per release, asked of GitHub once an hour. When
/// GitHub doesn't answer, the last answer is kept (none, if it never has).
async fn downloads(app: &App) -> Option<Vec<ReleaseDownloads>> {
    let last = app.status.downloads.lock().unwrap().clone();
    if let Some((at, n)) = &last
        && at.elapsed() < DOWNLOADS_FOR
    {
        return n.clone();
    }
    let n = match fetch_downloads(&app.http).await {
        Ok(n) => Some(n),
        Err(e) => {
            tracing::warn!("stats: downloads from GitHub: {e}");
            last.and_then(|(_, n)| n)
        }
    };
    *app.status.downloads.lock().unwrap() = Some((Instant::now(), n.clone()));
    n
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    published_at: Option<String>,
    #[serde(default)]
    draft: bool,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    download_count: i64,
}

async fn fetch_downloads(http: &reqwest::Client) -> Result<Vec<ReleaseDownloads>, String> {
    let mut releases = Vec::new();
    for n in 1..=10 {
        let res = http
            .get(format!("{RELEASES}&page={n}"))
            .header(header::USER_AGENT, "hasteclip-api")
            .header(header::ACCEPT, "application/vnd.github+json")
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|e| e.to_string())?;
        let page: Vec<Release> = res.json().await.map_err(|e| e.to_string())?;
        if page.is_empty() {
            break;
        }
        releases.extend(page.into_iter().filter(|r| !r.draft).map(|r| {
            let assets: Vec<(String, i64)> = r.assets.into_iter().map(|a| (a.name, a.download_count)).collect();
            ReleaseDownloads { tag: r.tag_name, published: r.published_at, downloads: installs(&assets) }
        }));
    }
    Ok(releases)
}

/// How many times a release was downloaded to install, from its files and
/// their download counts. Updates aren't installs: on Windows they're the
/// `.nupkg`s and on macOS the `.zip`, so neither is counted. On Linux an
/// update downloads the same tarball as an install does, and its `.sha256`
/// too, which a download by hand doesn't, so those come off.
fn installs(assets: &[(String, i64)]) -> i64 {
    let (mut n, mut tarballs, mut checked) = (0, 0, 0);
    for (name, count) in assets {
        match name.as_str() {
            "HasteClip-win-Setup.exe" | "HasteClip-win-Portable.zip" | "HasteClip-mac-arm64.dmg" => n += count,
            "HasteClip-linux-x64.tar.gz" => tarballs += count,
            "HasteClip-linux-x64.tar.gz.sha256" => checked += count,
            _ => {}
        }
    }
    n + (tarballs - checked).max(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn updates_dont_count_as_downloads() {
        let assets: Vec<(String, i64)> = [
            ("HasteClip-win-Setup.exe", 1430),
            ("HasteClip-win-Portable.zip", 40),
            ("HasteClip-0.9.0-delta.nupkg", 1431),
            ("HasteClip-0.9.0-full.nupkg", 53),
            ("releases.win.json", 806_274),
            ("HasteClip-mac-arm64.dmg", 166),
            ("HasteClip-mac-arm64.zip", 22),
            ("HasteClip-linux-x64.tar.gz", 192),
            ("HasteClip-linux-x64.tar.gz.sha256", 43),
        ]
        .into_iter()
        .map(|(name, n)| (name.to_owned(), n))
        .collect();
        assert_eq!(installs(&assets), 1430 + 40 + 166 + (192 - 43));
    }
}
