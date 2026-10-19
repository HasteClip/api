//! Settings, all from environment variables (Coolify sets them in
//! production; a `.env` file is not read, export them yourself locally).

use std::env;

use url::Url;

pub struct Config {
    pub port: u16,
    pub database_url: String,
    /// Where share pages live, like `https://clips.hasteclip.com` (no
    /// trailing slash).
    pub share_url: String,
    pub s3: S3,
    pub limits: Limits,
    /// Delete a clip once nobody has opened or played it for this many days.
    /// `None` keeps clips until they're unshared.
    pub retention_days: Option<i32>,
    /// A Discord webhook that hears about new reports.
    pub report_webhook: Option<String>,
}

pub struct S3 {
    /// Like `https://fsn1.your-objectstorage.com`.
    pub endpoint: Url,
    pub region: String,
    pub bucket: String,
    pub access_key: String,
    pub secret_key: String,
}

pub struct Limits {
    pub clip_bytes: i64,
    pub uploader_bytes: i64,
    pub clips_per_day: i64,
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        let region = optional("S3_REGION").unwrap_or_else(|| "fsn1".into());
        let endpoint = optional("S3_ENDPOINT").unwrap_or_else(|| format!("https://{region}.your-objectstorage.com"));
        let retention_days = number("RETENTION_DAYS", 90)?;
        Ok(Self {
            port: number("PORT", 8080)?,
            database_url: required("DATABASE_URL")?,
            share_url: url("SHARE_URL", "https://clips.hasteclip.com")?,
            s3: S3 {
                endpoint: endpoint.parse().map_err(|e| format!("S3_ENDPOINT: {e}"))?,
                region,
                bucket: required("S3_BUCKET")?,
                access_key: required("S3_ACCESS_KEY")?,
                secret_key: required("S3_SECRET_KEY")?,
            },
            limits: Limits {
                clip_bytes: number::<i64>("MAX_CLIP_MB", 1024)? * 1024 * 1024,
                uploader_bytes: number::<i64>("MAX_UPLOADER_GB", 10)? * 1024 * 1024 * 1024,
                clips_per_day: number("MAX_CLIPS_PER_DAY", 100)?,
            },
            retention_days: (retention_days > 0).then_some(retention_days),
            report_webhook: optional("REPORT_WEBHOOK"),
        })
    }
}

fn optional(name: &str) -> Option<String> {
    env::var(name).ok().map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
}

fn required(name: &str) -> Result<String, String> {
    optional(name).ok_or_else(|| format!("{name} isn't set"))
}

fn number<T: std::str::FromStr>(name: &str, default: T) -> Result<T, String> {
    match optional(name) {
        Some(v) => v.parse().map_err(|_| format!("{name} isn't a number: {v}")),
        None => Ok(default),
    }
}

fn url(name: &str, default: &str) -> Result<String, String> {
    let v = optional(name).unwrap_or_else(|| default.into());
    Url::parse(&v).map_err(|e| format!("{name}: {e}"))?;
    Ok(v.trim_end_matches('/').to_owned())
}
