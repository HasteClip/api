//! What people open: a clip's page (`/<id>`), its video and thumbnail
//! (`/<id>.mp4`, `/<id>.jpg`, which send you on to the bucket), and the
//! report form. No scripts, no cookies, nothing tracked but a view count.

use std::sync::{Arc, LazyLock};

use askama::Template;
use axum::Form;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Redirect, Response};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use time::macros::format_description;

use crate::db::{self, Clip};
use crate::storage::WATCH_LINK_CACHE;
use crate::{App, ids};

const SITE: &str = "https://hasteclip.com";

pub async fn home() -> Redirect {
    Redirect::permanent(SITE)
}

pub async fn health() -> &'static str {
    "ok"
}

/// `/<id>`, `/<id>.mp4` or `/<id>.jpg`.
pub async fn clip(State(app): State<Arc<App>>, Path(name): Path<String>) -> Response {
    let (id, file) = match name.rsplit_once('.') {
        Some((id, "mp4")) => (id, Some(File::Video)),
        Some((id, "jpg")) => (id, Some(File::Thumb)),
        Some(_) => return gone(),
        None => (name.as_str(), None),
    };
    let clip = match ready_clip(&app, id).await {
        Ok(Some(clip)) => clip,
        Ok(None) => return gone(),
        Err(e) => return e,
    };
    match file {
        Some(file) => watch(&app, &clip, file),
        None => {
            app.views.page(&clip.id);
            page(&app, &ClipPage::new(&app, &clip))
        }
    }
}

enum File {
    Video,
    Thumb,
}

/// Send the browser (or Discord) on to the file in the bucket, with a link
/// that expires.
fn watch(app: &App, clip: &Clip, file: File) -> Response {
    let (key, content_type) = match file {
        File::Video => (Some(&clip.video_key), "video/mp4"),
        File::Thumb => (clip.thumb_key.as_ref(), "image/jpeg"),
    };
    let Some(key) = key else { return gone() };
    app.views.play(&clip.id);
    let link = app.storage.watch_link(key, content_type);
    (
        StatusCode::FOUND,
        [(header::LOCATION, link.to_string()), (header::CACHE_CONTROL, format!("public, max-age={WATCH_LINK_CACHE}"))],
    )
        .into_response()
}

#[derive(Template)]
#[template(path = "clip.html")]
struct ClipPage {
    v: &'static str,
    id: String,
    title: String,
    /// What's under the title: game, length, date.
    meta: String,
    /// For link previews: game, length, and where it's from.
    description: String,
    page_url: String,
    video_url: String,
    thumb_url: Option<String>,
    width: i32,
    height: i32,
}

impl ClipPage {
    fn new(app: &App, clip: &Clip) -> Self {
        let length = duration(clip.duration_ms);
        let date = clip
            .ready_at
            .unwrap_or(clip.created_at)
            .format(format_description!("[day padding:none] [month repr:short] [year]"))
            .unwrap_or_default();
        let game = clip.game.as_deref().map(|g| format!("{g} · ")).unwrap_or_default();
        Self {
            v: asset_version(),
            id: clip.id.clone(),
            title: clip.title.clone(),
            meta: format!("{game}{length} · {date}"),
            description: format!("{game}{length} · Shared with HasteClip"),
            page_url: app.link(&clip.id),
            video_url: app.link(&format!("{}.mp4", clip.id)),
            thumb_url: clip.thumb_key.as_ref().map(|_| app.link(&format!("{}.jpg", clip.id))),
            width: clip.width,
            height: clip.height,
        }
    }
}

#[derive(Template)]
#[template(path = "gone.html")]
struct Gone {
    v: &'static str,
}

fn gone() -> Response {
    let mut res = render(&Gone { v: asset_version() }, None);
    *res.status_mut() = StatusCode::NOT_FOUND;
    res
}

/// What can be reported, and how the form puts it.
const REASONS: [(&str, &str); 8] = [
    ("child-safety", "It sexualizes or harms a child"),
    ("sexual", "Sexual content or nudity"),
    ("violence", "Real violence or gore"),
    ("hate", "Hate or harassment"),
    ("private", "It shares someone's private information"),
    ("copyright", "It's my work, shared without my permission"),
    ("spam", "Spam or a scam"),
    ("other", "Something else"),
];

#[derive(Template)]
#[template(path = "report.html")]
struct ReportForm {
    v: &'static str,
    id: String,
    title: String,
    reasons: &'static [(&'static str, &'static str)],
}

#[derive(Template)]
#[template(path = "reported.html")]
struct Reported {
    v: &'static str,
    id: String,
}

pub async fn report_form(State(app): State<Arc<App>>, Path(id): Path<String>) -> Response {
    match ready_clip(&app, &id).await {
        Ok(Some(clip)) => {
            let form = ReportForm { v: asset_version(), id: clip.id, title: clip.title, reasons: &REASONS };
            page(&app, &form)
        }
        Ok(None) => gone(),
        Err(e) => e,
    }
}

#[derive(Deserialize)]
pub struct Report {
    reason: String,
    #[serde(default)]
    details: String,
    #[serde(default)]
    email: String,
}

pub async fn report(State(app): State<Arc<App>>, Path(id): Path<String>, Form(r): Form<Report>) -> Response {
    let clip = match ready_clip(&app, &id).await {
        Ok(Some(clip)) => clip,
        Ok(None) => return gone(),
        Err(e) => return e,
    };
    let Some(&(reason, label)) = REASONS.iter().find(|(r_, _)| *r_ == r.reason) else {
        return (StatusCode::BAD_REQUEST, "Pick what's wrong with the clip.").into_response();
    };
    let details: String = r.details.trim().chars().take(2000).collect();
    let email: String = r.email.trim().chars().take(200).collect();
    let email = (!email.is_empty()).then_some(email);
    if let Err(e) = db::add_report(&app.db, &clip.id, reason, &details, email.as_deref()).await {
        tracing::error!("report: {e}");
        return (StatusCode::INTERNAL_SERVER_ERROR, "Couldn't send the report. Try again in a bit.").into_response();
    }
    tracing::warn!(clip = clip.id, reason, "reported");
    if let Some(hook) = app.config.report_webhook.clone() {
        let urgent = if reason == "child-safety" { "🚨 **Urgent** · " } else { "" };
        let mut text = format!("{urgent}🚩 **{label}**: <{}>", app.link(&clip.id));
        if !details.is_empty() {
            text += &format!("\n> {}", details.replace('\n', "\n> "));
        }
        if let Some(email) = &email {
            text += &format!("\nReply to: {email}");
        }
        text = text.chars().take(1900).collect();
        let http = app.http.clone();
        tokio::spawn(async move {
            // No pings: the text is partly someone else's.
            let body = serde_json::json!({ "content": text, "allowed_mentions": { "parse": [] } });
            if let Err(e) = http.post(hook).json(&body).send().await.and_then(|r| r.error_for_status()) {
                tracing::warn!("report webhook: {e}");
            }
        });
    }
    page(&app, &Reported { v: asset_version(), id: clip.id })
}

/// A clip that's shared and finished uploading; `Ok(None)` for anything
/// else (a wrong id, unshared, still uploading).
async fn ready_clip(app: &App, id: &str) -> Result<Option<Clip>, Response> {
    if !ids::is_clip_id(id) {
        return Ok(None);
    }
    match db::clip(&app.db, id).await {
        Ok(clip) => Ok(clip.filter(Clip::is_ready)),
        Err(e) => {
            tracing::error!("page: {e}");
            Err((StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong on our side. Try again in a bit.")
                .into_response())
        }
    }
}

/// A page as HTML with the headers every page gets: scripts and plugins
/// off, media only from here and the bucket, not indexed by search engines.
fn page(app: &App, t: &impl Template) -> Response {
    render(t, Some(&app.storage.origin()))
}

fn render(t: &impl Template, bucket: Option<&str>) -> Response {
    let html = match t.render() {
        Ok(html) => html,
        Err(e) => {
            tracing::error!("template: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong on our side.").into_response();
        }
    };
    let media = bucket.unwrap_or("");
    let csp = format!(
        "default-src 'none'; style-src 'self'; font-src 'self'; img-src 'self' {media}; media-src 'self' {media}; \
         form-action 'self'; base-uri 'none'; frame-ancestors 'none'"
    );
    let mut res = Html(html).into_response();
    let h = res.headers_mut();
    h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_str(&csp).expect("a valid header"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=60"));
    h.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert("x-robots-tag", HeaderValue::from_static("noindex"));
    res
}

/// `1:05`, or `1:02:05` past an hour.
fn duration(ms: i32) -> String {
    let s = (ms.max(0) + 500) / 1000;
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

const STYLE: &[u8] = include_bytes!("../static/style.css");
const FONT: &[u8] = include_bytes!("../static/HankenGrotesk.ttf");
const GLYPH: &[u8] = include_bytes!("../static/glyph.svg");

/// Changes whenever the stylesheet does, so pages can ask for
/// `style.css?v=…` and browsers keep it for a week.
fn asset_version() -> &'static str {
    static V: LazyLock<String> = LazyLock::new(|| {
        let hash = Sha256::digest(STYLE);
        hash[..4].iter().map(|b| format!("{b:02x}")).collect()
    });
    &V
}

pub async fn asset(Path(name): Path<String>) -> Response {
    let (body, content_type) = match name.as_str() {
        "style.css" => (STYLE, "text/css; charset=utf-8"),
        "HankenGrotesk.ttf" => (FONT, "font/ttf"),
        "glyph.svg" => (GLYPH, "image/svg+xml"),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    ([(header::CONTENT_TYPE, content_type), (header::CACHE_CONTROL, "public, max-age=604800")], body).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_right() {
        assert_eq!(duration(0), "0:00");
        assert_eq!(duration(32_400), "0:32");
        assert_eq!(duration(65_000), "1:05");
        assert_eq!(duration(3_725_000), "1:02:05");
    }
}
