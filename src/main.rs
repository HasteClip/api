//! HasteClip's API: share links for clips, at `clips.hasteclip.com` (the
//! pages people open) and `api.hasteclip.com` (what the app talks to). One
//! server for both; see `api.rs` and `pages.rs`.
//!
//! `hasteclip-api` serves; `hasteclip-api <command>` is for dealing with
//! reports (see `admin.rs`). Either way the database is brought up to date
//! first (`migrations/`).

mod admin;
mod api;
mod cleanup;
mod config;
mod db;
mod ids;
mod limits;
mod pages;
mod storage;
mod views;

use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
use sqlx::postgres::PgPoolOptions;

use crate::config::Config;
use crate::storage::Storage;
use crate::views::Views;

pub struct App {
    pub config: Config,
    pub db: sqlx::PgPool,
    pub storage: Storage,
    pub views: Views,
    /// For the report webhook.
    pub http: reqwest::Client,
}

impl App {
    /// A link on the share site: `link("aB3xY9k2Qp")` is the clip's page.
    pub fn link(&self, path: &str) -> String {
        format!("{}/{path}", self.config.share_url)
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,sqlx=warn".into()),
        )
        .init();
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("{e}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), String> {
    let config = Config::from_env()?;
    let db = PgPoolOptions::new()
        .max_connections(10)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&config.database_url)
        .await
        .map_err(|e| format!("database: {e}"))?;
    sqlx::migrate!().run(&db).await.map_err(|e| format!("migrations: {e}"))?;
    let storage = Storage::new(&config.s3)?;
    let http = reqwest::Client::builder().timeout(Duration::from_secs(10)).build().map_err(|e| e.to_string())?;
    let app = Arc::new(App { config, db, storage, views: Views::default(), http });

    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.is_empty() {
        return admin::run(&app, &args).await;
    }

    if let Err(e) = app.storage.prepare().await {
        tracing::warn!("bucket: {e}");
    }
    cleanup::spawn(app.clone());

    // Signing up is rare (once per app), so it's held tightest; reports
    // next; the rest is a few calls per shared clip.
    let sign_ups = limits::limit(Duration::from_secs(10 * 60), 5);
    let reports = limits::limit(Duration::from_secs(60), 5);
    let calls = limits::limit(Duration::from_millis(500), 30);
    limits::spawn_cleanup(vec![sign_ups.clone(), reports.clone(), calls.clone()]);

    let router = Router::new()
        .route("/v1/uploaders", post(api::sign_up).layer(limits::layer(&sign_ups)))
        .merge(
            Router::new()
                .route("/v1/clips", post(api::share).get(api::list))
                .route("/v1/clips/{id}/complete", post(api::complete))
                .route("/v1/clips/{id}", axum::routing::delete(api::unshare))
                .layer(limits::layer(&calls)),
        )
        .route("/{id}/report", get(pages::report_form).post(pages::report).layer(limits::layer(&reports)))
        .route("/", get(pages::home))
        .route("/healthz", get(pages::health))
        .route("/static/{name}", get(pages::asset))
        .route("/{name}", get(pages::clip))
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .with_state(app.clone());

    let addr = SocketAddr::from(([0, 0, 0, 0], app.config.port));
    let listener = tokio::net::TcpListener::bind(addr).await.map_err(|e| format!("listen on {addr}: {e}"))?;
    tracing::info!("listening on {addr}");
    axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(stopped())
        .await
        .map_err(|e| e.to_string())?;
    // Don't lose the last half minute of views.
    app.views.write(&app.db).await;
    Ok(())
}

/// Ctrl+C, or SIGTERM from Docker when Coolify stops or replaces us.
async fn stopped() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {},
        () = term => {},
    }
    tracing::info!("stopping");
}
