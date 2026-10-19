//! The JSON API the app talks to (`/v1/…`).
//!
//! Sharing a clip:
//!
//! 1. `POST /v1/uploaders` once per app, for a token (`Authorization: Bearer`).
//! 2. `POST /v1/clips` with the clip's size and details: the share link, and
//!    links to upload the video in parts (each `PUT` exactly `part_size`
//!    bytes, the last one the rest) and the thumbnail.
//! 3. Upload them straight to the bucket, keeping each part's `ETag`.
//! 4. `POST /v1/clips/{id}/complete` with the ETags in order. Now the link
//!    works.
//!
//! `DELETE /v1/clips/{id}` unshares; `GET /v1/clips` lists what's shared.

use std::sync::Arc;

use axum::Json;
use axum::extract::{FromRequestParts, Path, State};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;

use crate::db::{self, NewClip};
use crate::storage::{self, UPLOAD_LINK_LIFE};
use crate::{App, cleanup, ids};

/// An error the app can show as it is: `{"error": "…"}`.
pub struct ApiError(StatusCode, String);

impl ApiError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self(status, message.into())
    }

    /// Our fault, logged; the app only hears that something went wrong.
    fn internal(what: &str, e: impl std::fmt::Display) -> Self {
        tracing::error!("{what}: {e}");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong on our side. Try again in a bit.")
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        Self::internal("database", e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({ "error": self.1 }))).into_response()
    }
}

/// The app making the request, from its token.
pub struct Uploader(i64);

impl FromRequestParts<Arc<App>> for Uploader {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, app: &Arc<App>) -> Result<Self, ApiError> {
        let unknown = || ApiError::new(StatusCode::UNAUTHORIZED, "This app isn't signed up to share clips.");
        let token = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or_else(unknown)?;
        match db::uploader(&app.db, &ids::token_hash(token.trim())).await? {
            Some((id, false)) => Ok(Self(id)),
            Some((_, true)) => Err(ApiError::new(StatusCode::FORBIDDEN, "This app can't share clips anymore.")),
            None => Err(unknown()),
        }
    }
}

#[derive(Serialize)]
pub struct Token {
    token: String,
}

pub async fn sign_up(State(app): State<Arc<App>>) -> Result<(StatusCode, Json<Token>), ApiError> {
    let token = ids::token();
    db::add_uploader(&app.db, &ids::token_hash(&token)).await?;
    Ok((StatusCode::CREATED, Json(Token { token })))
}

#[derive(Deserialize)]
pub struct Share {
    size: i64,
    duration_ms: i32,
    width: i32,
    height: i32,
    fps: Option<f32>,
    title: String,
    game: Option<String>,
    /// The thumbnail's size, when there's one to upload (a JPEG).
    thumb_size: Option<i32>,
}

#[derive(Serialize)]
pub struct Upload {
    id: String,
    url: String,
    /// Every part is this big, except the last one (the rest).
    part_size: u64,
    parts: Vec<String>,
    thumb: Option<String>,
    /// Seconds the upload links stay good.
    expires_in: u64,
}

const MAX_THUMB_BYTES: i32 = 5 * 1024 * 1024;
const MAX_SIDE: i32 = 16_384;
const MAX_DURATION_MS: i32 = 6 * 60 * 60 * 1000;

pub async fn share(
    State(app): State<Arc<App>>,
    Uploader(uploader_id): Uploader,
    Json(s): Json<Share>,
) -> Result<(StatusCode, Json<Upload>), ApiError> {
    let limits = &app.config.limits;
    let bad = |m: &str| ApiError::new(StatusCode::BAD_REQUEST, m);
    if s.size < 1 {
        return Err(bad("The clip is empty."));
    }
    if s.size > limits.clip_bytes {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("Clips can be up to {} MB. Export a smaller copy and share that.", limits.clip_bytes >> 20),
        ));
    }
    if !(1..=MAX_SIDE).contains(&s.width) || !(1..=MAX_SIDE).contains(&s.height) {
        return Err(bad("The clip's size in pixels isn't right."));
    }
    if !(1..=MAX_DURATION_MS).contains(&s.duration_ms) {
        return Err(bad("The clip's length isn't right."));
    }
    if s.fps.is_some_and(|f| !(f > 0.0 && f <= 1000.0)) {
        return Err(bad("The clip's frame rate isn't right."));
    }
    if s.thumb_size.is_some_and(|t| !(1..=MAX_THUMB_BYTES).contains(&t)) {
        return Err(bad("The thumbnail is too big."));
    }

    let (used, today) = db::usage(&app.db, uploader_id).await?;
    if used + s.size > limits.uploader_bytes {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            format!(
                "You've shared {} GB of clips, as much as you can. Unshare some to share more.",
                limits.uploader_bytes >> 30
            ),
        ));
    }
    if today >= limits.clips_per_day {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "That's a lot of clips for one day. Try again tomorrow.",
        ));
    }

    let id = ids::clip_id();
    let video_key = format!("clips/{id}/video.mp4");
    let thumb_key = s.thumb_size.map(|_| format!("clips/{id}/thumb.jpg"));
    let upload_id = app.storage.start_upload(&video_key).await.map_err(|e| ApiError::internal("bucket", e))?;
    let title = clean(&s.title, 200).unwrap_or_else(|| "Clip".into());
    let game = s.game.as_deref().and_then(|g| clean(g, 100));
    let added = db::add_clip(
        &app.db,
        &NewClip {
            id: &id,
            uploader_id,
            video_key: &video_key,
            thumb_key: thumb_key.as_deref(),
            upload_id: &upload_id,
            size: s.size,
            thumb_size: s.thumb_size,
            duration_ms: s.duration_ms,
            width: s.width,
            height: s.height,
            fps: s.fps,
            title: &title,
            game: game.as_deref(),
        },
    )
    .await;
    if let Err(e) = added {
        let _ = app.storage.abort_upload(&video_key, &upload_id).await;
        return Err(e.into());
    }

    let size = s.size as u64;
    let parts = app.storage.part_links(&video_key, &upload_id, size);
    let thumb = thumb_key.as_deref().zip(s.thumb_size).map(|(k, n)| app.storage.jpeg_link(k, n as u64).into());
    Ok((
        StatusCode::CREATED,
        Json(Upload {
            url: app.link(&id),
            id,
            part_size: storage::part_sizes(size)[0],
            parts: parts.into_iter().map(String::from).collect(),
            thumb,
            expires_in: UPLOAD_LINK_LIFE.as_secs(),
        }),
    ))
}

#[derive(Deserialize)]
pub struct Complete {
    etags: Vec<String>,
}

#[derive(Serialize)]
pub struct Shared {
    id: String,
    url: String,
}

pub async fn complete(
    State(app): State<Arc<App>>,
    Uploader(uploader_id): Uploader,
    Path(id): Path<String>,
    Json(c): Json<Complete>,
) -> Result<Json<Shared>, ApiError> {
    let clip = own_clip(&app, uploader_id, &id).await?;
    let url = app.link(&clip.id);
    if clip.is_ready() {
        return Ok(Json(Shared { id: clip.id, url }));
    }
    let Some(upload_id) = &clip.upload_id else {
        return Err(ApiError::internal("complete", format!("{id} is uploading with no upload id")));
    };
    if c.etags.len() != storage::part_sizes(clip.size as u64).len() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "Not every part of the clip was uploaded."));
    }
    // A part that didn't arrive can be uploaded again, then this tried again.
    app.storage
        .finish_upload(&clip.video_key, upload_id, &c.etags)
        .await
        .map_err(|e| ApiError::new(StatusCode::CONFLICT, format!("The upload isn't complete ({e}).")))?;

    let whole =
        app.storage.size(&clip.video_key).await.map_err(|e| ApiError::internal("bucket", e))? == Some(clip.size as u64);
    let mp4 = whole && is_mp4(&app.storage.head_bytes(&clip.video_key, 12).await.unwrap_or_default());
    if !mp4 {
        let _ = cleanup::remove(&app, &clip).await;
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "The clip didn't arrive as an MP4 video. Share it again.",
        ));
    }
    let has_thumb = match (&clip.thumb_key, clip.thumb_size) {
        (Some(key), Some(size)) => app.storage.size(key).await.ok().flatten() == Some(size as u64),
        _ => false,
    };
    db::mark_ready(&app.db, &clip.id, has_thumb).await?;
    tracing::info!(clip = clip.id, size = clip.size, "shared");
    Ok(Json(Shared { id: clip.id, url }))
}

pub async fn unshare(
    State(app): State<Arc<App>>,
    Uploader(uploader_id): Uploader,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let clip = own_clip(&app, uploader_id, &id).await?;
    cleanup::remove(&app, &clip).await.map_err(|e| ApiError::internal("unshare", e))?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
pub struct Listed {
    id: String,
    url: String,
    title: String,
    /// `uploading` or `ready`.
    state: String,
    size: i64,
    views: i64,
    created_at: String,
}

pub async fn list(State(app): State<Arc<App>>, Uploader(uploader_id): Uploader) -> Result<Json<Vec<Listed>>, ApiError> {
    let clips = db::clips_of(&app.db, uploader_id).await?;
    Ok(Json(
        clips
            .into_iter()
            .map(|c| Listed {
                url: app.link(&c.id),
                created_at: c.created_at.format(&Rfc3339).unwrap_or_default(),
                id: c.id,
                title: c.title,
                state: c.state,
                size: c.size,
                views: c.views,
            })
            .collect(),
    ))
}

/// The uploader's own clip `id`; someone else's is as good as not there.
async fn own_clip(app: &App, uploader_id: i64, id: &str) -> Result<db::Clip, ApiError> {
    let not_found = || ApiError::new(StatusCode::NOT_FOUND, "That clip isn't shared.");
    if !ids::is_clip_id(id) {
        return Err(not_found());
    }
    db::clip(&app.db, id).await?.filter(|c| c.uploader_id == uploader_id).ok_or_else(not_found)
}

/// Whether a file starts like an MP4 (or MOV): a box, then `ftyp`.
fn is_mp4(start: &[u8]) -> bool {
    start.len() >= 8 && &start[4..8] == b"ftyp"
}

/// Text from the app, made fit to show: control characters out, runs of
/// spaces as one, at most `max` characters. `None` when nothing's left.
fn clean(s: &str, max: usize) -> Option<String> {
    let words: Vec<&str> = s.split(|c: char| c.is_whitespace() || c.is_control()).filter(|w| !w.is_empty()).collect();
    let mut out = words.join(" ");
    if let Some((i, _)) = out.char_indices().nth(max) {
        out.truncate(i);
        out.truncate(out.trim_end().len());
    }
    (!out.is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_cleaned() {
        assert_eq!(clean("  Ace   on\tMirage\n", 200).as_deref(), Some("Ace on Mirage"));
        assert_eq!(clean("a\u{0}b", 200).as_deref(), Some("a b"));
        assert_eq!(clean(" \n ", 200), None);
        assert_eq!(clean("ééééé", 3).as_deref(), Some("ééé"));
        assert_eq!(clean("ab cd", 3).as_deref(), Some("ab"));
    }

    #[test]
    fn mp4_is_recognized() {
        assert!(is_mp4(b"\0\0\0\x20ftypisom"));
        assert!(!is_mp4(b"<html><body>"));
        assert!(!is_mp4(b"\0\0"));
    }
}
