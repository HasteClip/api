//! What status.hasteclip.com checks and links to: `/readyz`, whether the
//! server can do its job right now, and `/stats`, a page of totals.
//!
//! `/healthz` (in `pages.rs`) is Coolify's, and only says the server is up:
//! a database that's away for a minute shouldn't make Coolify replace a
//! server that's fine. `/readyz` asks the database and the bucket too.
//!
//! The stats are totals of what's kept anyway (see the README's Privacy):
//! nothing about anyone in particular, and nothing new is counted for them.
//! Both answers are reused for a while, so asking a thousand times a second
//! costs what asking once does.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use askama::Template;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::App;
use crate::db::{self, Totals};
use crate::pages::{asset_version, page};

const READY_FOR: Duration = Duration::from_secs(10);
const STATS_FOR: Duration = Duration::from_secs(5 * 60);
const DOWNLOADS_FOR: Duration = Duration::from_secs(60 * 60);
/// How long the database or the bucket may take to answer `/readyz`.
const CHECK_TIMEOUT: Duration = Duration::from_secs(5);
/// The app's releases, whose downloads the stats page counts.
const RELEASES: &str = "https://api.github.com/repos/hasteclip/hasteclip/releases?per_page=100";
const STATUS_PAGE: &str = "https://status.hasteclip.com";

/// The answers being reused, and when each was worked out.
#[derive(Default)]
pub struct Status {
    ready: Mutex<Option<(Instant, Result<(), &'static str>)>>,
    stats: Mutex<Option<(Instant, Stats)>>,
    downloads: Mutex<Option<(Instant, Option<i64>)>>,
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

#[derive(Clone)]
struct Stats {
    totals: Totals,
    games: Vec<(String, i64)>,
}

#[derive(Template)]
#[template(path = "stats.html")]
struct StatsPage {
    v: &'static str,
    clips: String,
    this_week: String,
    views: String,
    video: String,
    stored: String,
    downloads: Option<String>,
    games: Vec<(String, String)>,
    retention_days: Option<i32>,
    status_page: &'static str,
}

/// `/stats`.
pub async fn stats(State(app): State<Arc<App>>) -> Response {
    let last = app.status.stats.lock().unwrap().clone();
    let stats = match last {
        Some((at, stats)) if at.elapsed() < STATS_FOR => stats,
        _ => match count(&app).await {
            Ok(stats) => {
                *app.status.stats.lock().unwrap() = Some((Instant::now(), stats.clone()));
                stats
            }
            Err(e) => {
                tracing::error!("stats: {e}");
                return (StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong on our side. Try again in a bit.")
                    .into_response();
            }
        },
    };
    let t = &stats.totals;
    let downloads = downloads(&app).await;
    page(
        &app,
        &StatsPage {
            v: asset_version(),
            clips: thousands(t.clips),
            this_week: thousands(t.this_week),
            views: thousands(t.views),
            video: hours(t.duration_ms),
            stored: bytes(t.bytes),
            downloads: downloads.map(thousands),
            games: stats.games.iter().map(|(g, n)| (g.clone(), thousands(*n))).collect(),
            retention_days: app.config.retention_days,
            status_page: STATUS_PAGE,
        },
    )
}

async fn count(app: &App) -> sqlx::Result<Stats> {
    let (totals, games) = tokio::join!(db::totals(&app.db), db::top_games(&app.db, 5, 3));
    Ok(Stats { totals: totals?, games: games? })
}

/// The app's downloads, asked of GitHub once an hour. When GitHub doesn't
/// answer, the last count is kept (none, if it never has).
async fn downloads(app: &App) -> Option<i64> {
    let last = *app.status.downloads.lock().unwrap();
    if let Some((at, n)) = last
        && at.elapsed() < DOWNLOADS_FOR
    {
        return n;
    }
    let n = match fetch_downloads(&app.http).await {
        Ok(n) => Some(n),
        Err(e) => {
            tracing::warn!("stats: downloads from GitHub: {e}");
            last.and_then(|(_, n)| n)
        }
    };
    *app.status.downloads.lock().unwrap() = Some((Instant::now(), n));
    n
}

#[derive(Deserialize)]
struct Release {
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    download_count: i64,
}

async fn fetch_downloads(http: &reqwest::Client) -> Result<i64, String> {
    let mut assets = Vec::new();
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
        assets.extend(page.into_iter().flat_map(|r| r.assets).map(|a| (a.name, a.download_count)));
    }
    Ok(installs(&assets))
}

/// How many times the app was downloaded to install, from its releases'
/// files and their download counts. Updates aren't installs: on Windows
/// they're the `.nupkg`s and on macOS the `.zip`, so neither is counted. On
/// Linux an update downloads the same tarball as an install does, and its
/// `.sha256` too, which a download by hand doesn't, so those come off.
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

/// `1,785`.
fn thousands(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
}

/// `12.3 GB`, `640 MB`: as a computer's own file browser counts them
/// (a GB is a billion bytes).
fn bytes(n: i64) -> String {
    let n = n as f64;
    match n {
        n if n >= 1e12 => format!("{:.1} TB", n / 1e12),
        n if n >= 1e9 => format!("{:.1} GB", n / 1e9),
        n if n >= 1e6 => format!("{:.0} MB", n / 1e6),
        n => format!("{:.0} KB", n / 1e3),
    }
}

/// `37 hours`, or `45 minutes` under an hour.
fn hours(ms: i64) -> String {
    let minutes = (ms + 30_000) / 60_000;
    match minutes {
        m if m < 60 => format!("{m} minute{}", if m == 1 { "" } else { "s" }),
        m => {
            let h = (m + 30) / 60;
            format!("{} hour{}", thousands(h), if h == 1 { "" } else { "s" })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_read_right() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1785), "1,785");
        assert_eq!(thousands(1_234_567), "1,234,567");
        assert_eq!(bytes(0), "0 KB");
        assert_eq!(bytes(640_000_000), "640 MB");
        assert_eq!(bytes(12_345_678_901), "12.3 GB");
        assert_eq!(bytes(2_500_000_000_000), "2.5 TB");
        assert_eq!(hours(0), "0 minutes");
        assert_eq!(hours(60_000), "1 minute");
        assert_eq!(hours(45 * 60_000), "45 minutes");
        assert_eq!(hours(60 * 60_000), "1 hour");
        assert_eq!(hours(37 * 3_600_000 + 10 * 60_000), "37 hours");
        assert_eq!(hours(1500 * 3_600_000), "1,500 hours");
    }

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
