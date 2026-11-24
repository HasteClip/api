//! Every query, in one place (and tested against a real Postgres in
//! `tests`, since they're checked at run time, not compile time).

use sqlx::PgPool;
use time::OffsetDateTime;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Clip {
    pub id: String,
    pub uploader_id: i64,
    pub state: String,
    pub video_key: String,
    pub thumb_key: Option<String>,
    pub upload_id: Option<String>,
    pub size: i64,
    pub thumb_size: Option<i32>,
    pub duration_ms: i32,
    pub width: i32,
    pub height: i32,
    pub title: String,
    pub game: Option<String>,
    pub views: i64,
    pub created_at: OffsetDateTime,
    pub ready_at: Option<OffsetDateTime>,
    /// Goes up each time the clip is replaced by a new version.
    pub version: i32,
}

impl Clip {
    pub fn is_ready(&self) -> bool {
        self.state == "ready"
    }
}

/// A clip before it's in the database: what the app said about it, and
/// where its files go.
pub struct NewClip<'a> {
    pub id: &'a str,
    pub uploader_id: i64,
    pub video_key: &'a str,
    pub thumb_key: Option<&'a str>,
    pub upload_id: &'a str,
    pub size: i64,
    pub thumb_size: Option<i32>,
    pub duration_ms: i32,
    pub width: i32,
    pub height: i32,
    pub fps: Option<f32>,
    pub title: &'a str,
    pub game: Option<&'a str>,
}

/// A query for clips: `clips!("where id = $1")`. A macro so the query is
/// still one literal string, which sqlx wants (no SQL built at run time).
macro_rules! clips {
    ($rest:literal) => {
        concat!(
            "select id, uploader_id, state, video_key, thumb_key, upload_id, size, thumb_size, duration_ms, ",
            "width, height, title, game, views, created_at, ready_at, version from clips ",
            $rest
        )
    };
}

pub async fn add_uploader(db: &PgPool, token_hash: &[u8]) -> sqlx::Result<i64> {
    sqlx::query_scalar("insert into uploaders (token_hash) values ($1) returning id")
        .bind(token_hash)
        .fetch_one(db)
        .await
}

/// The uploader with this token: `(id, banned)`.
pub async fn uploader(db: &PgPool, token_hash: &[u8]) -> sqlx::Result<Option<(i64, bool)>> {
    sqlx::query_as("select id, banned_at is not null from uploaders where token_hash = $1")
        .bind(token_hash)
        .fetch_optional(db)
        .await
}

/// What an uploader has shared: bytes in all their clips (finished or
/// not), and how many clips they started in the last day.
pub async fn usage(db: &PgPool, uploader_id: i64) -> sqlx::Result<(i64, i64)> {
    sqlx::query_as(
        "select coalesce(sum(size), 0)::bigint, count(*) filter (where created_at > now() - interval '1 day') \
         from clips where uploader_id = $1",
    )
    .bind(uploader_id)
    .fetch_one(db)
    .await
}

pub async fn add_clip(db: &PgPool, c: &NewClip<'_>) -> sqlx::Result<()> {
    sqlx::query(
        "insert into clips (id, uploader_id, video_key, thumb_key, upload_id, size, thumb_size, duration_ms, width, \
         height, fps, title, game) values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
    )
    .bind(c.id)
    .bind(c.uploader_id)
    .bind(c.video_key)
    .bind(c.thumb_key)
    .bind(c.upload_id)
    .bind(c.size)
    .bind(c.thumb_size)
    .bind(c.duration_ms)
    .bind(c.width)
    .bind(c.height)
    .bind(c.fps)
    .bind(c.title)
    .bind(c.game)
    .execute(db)
    .await
    .map(drop)
}

pub async fn clip(db: &PgPool, id: &str) -> sqlx::Result<Option<Clip>> {
    sqlx::query_as(clips!("where id = $1")).bind(id).fetch_optional(db).await
}

/// The uploader's clips, newest first.
pub async fn clips_of(db: &PgPool, uploader_id: i64) -> sqlx::Result<Vec<Clip>> {
    sqlx::query_as(clips!("where uploader_id = $1 order by created_at desc")).bind(uploader_id).fetch_all(db).await
}

/// The clip is uploaded and checked: it can be watched. `has_thumb` false
/// forgets a thumbnail that never arrived.
pub async fn mark_ready(db: &PgPool, id: &str, has_thumb: bool) -> sqlx::Result<()> {
    sqlx::query(
        "update clips set state = 'ready', upload_id = null, ready_at = now(), \
         thumb_key = case when $2 then thumb_key end, thumb_size = case when $2 then thumb_size end \
         where id = $1",
    )
    .bind(id)
    .bind(has_thumb)
    .execute(db)
    .await
    .map(drop)
}

pub async fn delete_clip(db: &PgPool, id: &str) -> sqlx::Result<()> {
    sqlx::query("delete from clips where id = $1").bind(id).execute(db).await.map(drop)
}

/// The clip was renamed in the app, or moved to another game's folder.
pub async fn set_names(db: &PgPool, id: &str, title: &str, game: Option<&str>) -> sqlx::Result<()> {
    sqlx::query("update clips set title = $2, game = $3 where id = $1")
        .bind(id)
        .bind(title)
        .bind(game)
        .execute(db)
        .await
        .map(drop)
}

/// A new version of a clip being uploaded (see `replacements`).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Replacement {
    pub clip_id: String,
    pub video_key: String,
    pub thumb_key: Option<String>,
    pub upload_id: String,
    pub size: i64,
    pub thumb_size: Option<i32>,
}

/// Start replacing clip `r.id` with what `r` says (its `uploader_id` isn't
/// used: the clip has one).
pub async fn add_replacement(db: &PgPool, r: &NewClip<'_>) -> sqlx::Result<()> {
    sqlx::query(
        "insert into replacements (clip_id, video_key, thumb_key, upload_id, size, thumb_size, duration_ms, width, \
         height, fps, title, game) values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
    )
    .bind(r.id)
    .bind(r.video_key)
    .bind(r.thumb_key)
    .bind(r.upload_id)
    .bind(r.size)
    .bind(r.thumb_size)
    .bind(r.duration_ms)
    .bind(r.width)
    .bind(r.height)
    .bind(r.fps)
    .bind(r.title)
    .bind(r.game)
    .execute(db)
    .await
    .map(drop)
}

pub async fn replacement(db: &PgPool, clip_id: &str) -> sqlx::Result<Option<Replacement>> {
    sqlx::query_as(
        "select clip_id, video_key, thumb_key, upload_id, size, thumb_size from replacements where clip_id = $1",
    )
    .bind(clip_id)
    .fetch_optional(db)
    .await
}

pub async fn delete_replacement(db: &PgPool, clip_id: &str) -> sqlx::Result<()> {
    sqlx::query("delete from replacements where clip_id = $1").bind(clip_id).execute(db).await.map(drop)
}

/// Replacements started more than a day ago that never finished (their
/// upload links have expired).
pub async fn stale_replacements(db: &PgPool) -> sqlx::Result<Vec<Replacement>> {
    sqlx::query_as(
        "select clip_id, video_key, thumb_key, upload_id, size, thumb_size from replacements \
         where created_at < now() - interval '1 day' order by created_at limit 500",
    )
    .fetch_all(db)
    .await
}

/// The clip becomes its replacement: its files, size and details, as the
/// next version. Its old files are retired, to be deleted `keep` seconds
/// from now. All at once, or not at all. `has_thumb` false forgets a
/// thumbnail that never arrived.
pub async fn apply_replacement(db: &PgPool, clip_id: &str, has_thumb: bool, keep: f64) -> sqlx::Result<()> {
    let mut tx = db.begin().await?;
    sqlx::query(
        "insert into retired_files (key, delete_after) \
         select k, now() + make_interval(secs => $2) from clips, unnest(array[video_key, thumb_key]) as k \
         where id = $1 and k is not null on conflict (key) do nothing",
    )
    .bind(clip_id)
    .bind(keep)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "update clips set video_key = r.video_key, thumb_key = case when $2 then r.thumb_key end, \
         thumb_size = case when $2 then r.thumb_size end, size = r.size, duration_ms = r.duration_ms, \
         width = r.width, height = r.height, fps = r.fps, title = r.title, game = r.game, version = clips.version + 1 \
         from replacements r where clips.id = $1 and r.clip_id = $1",
    )
    .bind(clip_id)
    .bind(has_thumb)
    .execute(&mut *tx)
    .await?;
    sqlx::query("delete from replacements where clip_id = $1").bind(clip_id).execute(&mut *tx).await?;
    tx.commit().await
}

/// Retired files whose time has come.
pub async fn due_retired(db: &PgPool) -> sqlx::Result<Vec<String>> {
    sqlx::query_scalar("select key from retired_files where delete_after < now() order by delete_after limit 500")
        .fetch_all(db)
        .await
}

pub async fn forget_retired(db: &PgPool, key: &str) -> sqlx::Result<()> {
    sqlx::query("delete from retired_files where key = $1").bind(key).execute(db).await.map(drop)
}

/// Count views (`views[i]` more for `ids[i]`) and note that each was seen
/// just now, all in one query.
pub async fn add_views(db: &PgPool, ids: &[String], views: &[i64]) -> sqlx::Result<()> {
    sqlx::query(
        "update clips set views = clips.views + v.n, last_viewed_at = now() \
         from unnest($1::text[], $2::bigint[]) as v(id, n) where clips.id = v.id",
    )
    .bind(ids)
    .bind(views)
    .execute(db)
    .await
    .map(drop)
}

/// Uploads started more than a day ago that never finished (their links
/// have expired, so they never will).
pub async fn stale_uploads(db: &PgPool) -> sqlx::Result<Vec<Clip>> {
    sqlx::query_as(clips!(
        "where state = 'uploading' and created_at < now() - interval '1 day' order by created_at limit 500"
    ))
    .fetch_all(db)
    .await
}

/// Clips nobody has opened or played for `days` days.
pub async fn unwatched(db: &PgPool, days: i32) -> sqlx::Result<Vec<Clip>> {
    sqlx::query_as(clips!(
        "where state = 'ready' and coalesce(last_viewed_at, ready_at) < now() - make_interval(days => $1) \
         order by created_at limit 500"
    ))
    .bind(days)
    .fetch_all(db)
    .await
}

pub async fn add_report(
    db: &PgPool,
    clip_id: &str,
    reason: &str,
    details: &str,
    email: Option<&str>,
) -> sqlx::Result<()> {
    sqlx::query("insert into reports (clip_id, reason, details, email) values ($1, $2, $3, $4)")
        .bind(clip_id)
        .bind(reason)
        .bind(details)
        .bind(email)
        .execute(db)
        .await
        .map(drop)
}

#[derive(Debug, sqlx::FromRow)]
pub struct Report {
    pub id: i64,
    pub clip_id: String,
    pub reason: String,
    pub details: String,
    pub email: Option<String>,
    pub created_at: OffsetDateTime,
}

/// Reports nobody has dealt with yet, oldest first.
pub async fn open_reports(db: &PgPool) -> sqlx::Result<Vec<Report>> {
    sqlx::query_as(
        "select id, clip_id, reason, details, email, created_at from reports where resolved_at is null order by created_at",
    )
    .fetch_all(db)
    .await
}

/// Mark the clip's reports dealt with.
pub async fn resolve_reports(db: &PgPool, clip_id: &str) -> sqlx::Result<u64> {
    sqlx::query("update reports set resolved_at = now() where clip_id = $1 and resolved_at is null")
        .bind(clip_id)
        .execute(db)
        .await
        .map(|r| r.rows_affected())
}

pub async fn ban(db: &PgPool, uploader_id: i64) -> sqlx::Result<()> {
    sqlx::query("update uploaders set banned_at = coalesce(banned_at, now()) where id = $1")
        .bind(uploader_id)
        .execute(db)
        .await
        .map(drop)
}

/// Whether the database answers (for `/readyz`).
pub async fn ping(db: &PgPool) -> sqlx::Result<()> {
    sqlx::query("select 1").execute(db).await.map(drop)
}

/// Totals over the clips that are shared, for the status page.
#[derive(Debug, Clone, Default, PartialEq, sqlx::FromRow)]
pub struct Totals {
    pub clips: i64,
    /// Clips ever shared: those up, and those removed since counting began
    /// (`hourly`'s first hour). Clips removed before that aren't known.
    pub clips_ever: i64,
    /// Times a clip's page was opened.
    pub views: i64,
    /// Videos and thumbnails.
    pub bytes: i64,
    pub duration_ms: i64,
    /// Shared (or, for an older clip, last replaced) in the last 7 days.
    pub this_week: i64,
    /// Apps that have signed up to share, ever.
    pub apps: i64,
    pub counted_since: Option<OffsetDateTime>,
}

pub async fn totals(db: &PgPool) -> sqlx::Result<Totals> {
    sqlx::query_as(
        "select count(*) as clips, \
         count(*) + (select coalesce(sum(removed), 0) from hourly)::bigint as clips_ever, \
         coalesce(sum(views), 0)::bigint as views, \
         coalesce(sum(size + coalesce(thumb_size, 0)), 0)::bigint as bytes, \
         coalesce(sum(duration_ms), 0)::bigint as duration_ms, \
         count(*) filter (where ready_at > now() - interval '7 days') as this_week, \
         (select count(*) from uploaders) as apps, \
         (select min(hour) from hourly) as counted_since \
         from clips where state = 'ready'",
    )
    .fetch_one(db)
    .await
}

/// An hour's totals (see `migrations/0003_hourly.sql`).
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct Hour {
    pub hour: OffsetDateTime,
    pub shares: i32,
    pub opens: i32,
    pub plays: i32,
    pub sign_ups: i32,
    pub refused: i32,
    pub removed: i32,
    pub clips: i32,
    pub bytes: i64,
}

/// Add `n` (shares, opens, plays, sign-ups, refused, removed) to this hour's
/// totals, and note how many clips are up now and how much they take.
pub async fn add_hourly(db: &PgPool, n: [i32; 6]) -> sqlx::Result<()> {
    sqlx::query(
        "insert into hourly (hour, shares, opens, plays, sign_ups, refused, removed, clips, bytes) \
         select date_trunc('hour', now(), 'UTC'), $1, $2, $3, $4, $5, $6, count(*), \
         coalesce(sum(size + coalesce(thumb_size, 0)), 0) from clips where state = 'ready' \
         on conflict (hour) do update set shares = hourly.shares + excluded.shares, \
         opens = hourly.opens + excluded.opens, plays = hourly.plays + excluded.plays, \
         sign_ups = hourly.sign_ups + excluded.sign_ups, refused = hourly.refused + excluded.refused, \
         removed = hourly.removed + excluded.removed, clips = excluded.clips, bytes = excluded.bytes",
    )
    .bind(n[0])
    .bind(n[1])
    .bind(n[2])
    .bind(n[3])
    .bind(n[4])
    .bind(n[5])
    .execute(db)
    .await
    .map(drop)
}

/// Every hour's totals in the last `days` days, oldest first.
pub async fn hours(db: &PgPool, days: i32) -> sqlx::Result<Vec<Hour>> {
    sqlx::query_as(
        "select hour, shares, opens, plays, sign_ups, refused, removed, clips, bytes from hourly \
         where hour > now() - make_interval(days => $1) order by hour",
    )
    .bind(days)
    .fetch_all(db)
    .await
}

/// The `n` games with the most shared clips, and how many each has. Only
/// games that at least `apps` different apps shared clips of: a clip's game
/// is the app's to name, so one app can't put a name of its choosing on the
/// stats page by sharing a hundred clips. Desktop (a clip of no game) and
/// Other (no folder) aren't games, as the app's library says too.
pub async fn top_games(db: &PgPool, n: i64, apps: i64) -> sqlx::Result<Vec<(String, i64)>> {
    sqlx::query_as(
        "select game, count(*) from clips where state = 'ready' and game is not null \
         and game not in ('Desktop', 'Other') \
         group by game having count(distinct uploader_id) >= $2 order by count(*) desc, game limit $1",
    )
    .bind(n)
    .bind(apps)
    .fetch_all(db)
    .await
}

#[cfg(test)]
mod tests {
    //! Each test gets a fresh database with the migrations run (`sqlx::test`,
    //! which needs `DATABASE_URL` to point at a Postgres it can make
    //! databases on).

    use super::*;
    use crate::ids;

    async fn someone(db: &PgPool) -> i64 {
        add_uploader(db, &ids::token_hash(&ids::token())).await.unwrap()
    }

    fn new_clip(id: &str, uploader_id: i64, size: i64) -> NewClip<'_> {
        NewClip {
            id,
            uploader_id,
            video_key: "clips/x/video.mp4",
            thumb_key: Some("clips/x/thumb.jpg"),
            upload_id: "upload-1",
            size,
            thumb_size: Some(1000),
            duration_ms: 30_000,
            width: 1920,
            height: 1080,
            fps: Some(60.0),
            title: "Ace",
            game: Some("Counter-Strike 2"),
        }
    }

    async fn age(db: &PgPool, id: &str, column: &str, days: i32) {
        let sql = match column {
            "created_at" => "update clips set created_at = now() - make_interval(days => $2) where id = $1",
            "ready_at" => "update clips set ready_at = now() - make_interval(days => $2) where id = $1",
            _ => "update clips set last_viewed_at = now() - make_interval(days => $2) where id = $1",
        };
        sqlx::query(sql).bind(id).bind(days).execute(db).await.unwrap();
    }

    #[sqlx::test]
    async fn tokens_find_their_uploader_and_bans_stick(db: PgPool) {
        let token = ids::token();
        let id = add_uploader(&db, &ids::token_hash(&token)).await.unwrap();
        assert_eq!(uploader(&db, &ids::token_hash(&token)).await.unwrap(), Some((id, false)));
        assert_eq!(uploader(&db, &ids::token_hash("hct_nope")).await.unwrap(), None);
        ban(&db, id).await.unwrap();
        assert_eq!(uploader(&db, &ids::token_hash(&token)).await.unwrap(), Some((id, true)));
    }

    #[sqlx::test]
    async fn a_clip_goes_from_uploading_to_ready(db: PgPool) {
        let u = someone(&db).await;
        add_clip(&db, &new_clip("aaaaaaaaaa", u, 100)).await.unwrap();
        let c = clip(&db, "aaaaaaaaaa").await.unwrap().unwrap();
        assert!(!c.is_ready());
        assert_eq!(c.upload_id.as_deref(), Some("upload-1"));
        assert_eq!(c.game.as_deref(), Some("Counter-Strike 2"));

        mark_ready(&db, "aaaaaaaaaa", false).await.unwrap();
        let c = clip(&db, "aaaaaaaaaa").await.unwrap().unwrap();
        assert!(c.is_ready());
        assert!(c.upload_id.is_none() && c.ready_at.is_some());
        assert!(c.thumb_key.is_none() && c.thumb_size.is_none(), "a thumbnail that never came is forgotten");

        assert_eq!(clips_of(&db, u).await.unwrap().len(), 1);
        delete_clip(&db, "aaaaaaaaaa").await.unwrap();
        assert!(clip(&db, "aaaaaaaaaa").await.unwrap().is_none());
    }

    #[sqlx::test]
    async fn usage_counts_bytes_and_todays_clips(db: PgPool) {
        let u = someone(&db).await;
        assert_eq!(usage(&db, u).await.unwrap(), (0, 0));
        add_clip(&db, &new_clip("aaaaaaaaaa", u, 100)).await.unwrap();
        add_clip(&db, &new_clip("bbbbbbbbbb", u, 50)).await.unwrap();
        age(&db, "bbbbbbbbbb", "created_at", 3).await;
        add_clip(&db, &new_clip("cccccccccc", someone(&db).await, 999)).await.unwrap();
        assert_eq!(usage(&db, u).await.unwrap(), (150, 1));
    }

    #[sqlx::test]
    async fn views_add_up_and_plays_count_as_seen(db: PgPool) {
        let u = someone(&db).await;
        add_clip(&db, &new_clip("aaaaaaaaaa", u, 100)).await.unwrap();
        add_views(&db, &["aaaaaaaaaa".into(), "zzzzzzzzzz".into()], &[3, 1]).await.unwrap();
        add_views(&db, &["aaaaaaaaaa".into()], &[0]).await.unwrap();
        let (views, seen): (i64, bool) =
            sqlx::query_as("select views, last_viewed_at is not null from clips where id = 'aaaaaaaaaa'")
                .fetch_one(&db)
                .await
                .unwrap();
        assert_eq!((views, seen), (3, true));
    }

    #[sqlx::test]
    async fn sweeps_find_dead_uploads_and_unwatched_clips(db: PgPool) {
        let u = someone(&db).await;
        for id in ["newupload0", "oldupload0", "watched000", "unwatched0", "neverseen0", "freshclip0"] {
            add_clip(&db, &new_clip(id, u, 1)).await.unwrap();
        }
        age(&db, "oldupload0", "created_at", 2).await;
        for id in ["watched000", "unwatched0", "neverseen0", "freshclip0"] {
            mark_ready(&db, id, true).await.unwrap();
            age(&db, id, "created_at", 200).await;
        }
        for id in ["watched000", "unwatched0", "neverseen0"] {
            age(&db, id, "ready_at", 200).await;
        }
        age(&db, "watched000", "last_viewed_at", 10).await;
        age(&db, "unwatched0", "last_viewed_at", 100).await;

        let stale: Vec<String> = stale_uploads(&db).await.unwrap().into_iter().map(|c| c.id).collect();
        assert_eq!(stale, ["oldupload0"]);
        let mut old: Vec<String> = unwatched(&db, 90).await.unwrap().into_iter().map(|c| c.id).collect();
        old.sort();
        assert_eq!(old, ["neverseen0", "unwatched0"]);
    }

    #[sqlx::test]
    async fn a_clip_is_renamed_and_replaced(db: PgPool) {
        let u = someone(&db).await;
        add_clip(&db, &new_clip("aaaaaaaaaa", u, 100)).await.unwrap();
        mark_ready(&db, "aaaaaaaaaa", true).await.unwrap();
        set_names(&db, "aaaaaaaaaa", "Ace clutch", None).await.unwrap();
        let c = clip(&db, "aaaaaaaaaa").await.unwrap().unwrap();
        assert_eq!((c.title.as_str(), c.game), ("Ace clutch", None));

        let next = NewClip {
            video_key: "clips/aaaaaaaaaa/v2.mp4",
            thumb_key: Some("clips/aaaaaaaaaa/v2.jpg"),
            upload_id: "upload-2",
            size: 60,
            duration_ms: 12_000,
            title: "Ace clutch (trimmed)",
            ..new_clip("aaaaaaaaaa", u, 60)
        };
        add_replacement(&db, &next).await.unwrap();
        let r = replacement(&db, "aaaaaaaaaa").await.unwrap().unwrap();
        assert_eq!((r.upload_id.as_str(), r.size), ("upload-2", 60));
        // Until it's applied, the clip is as it was.
        let c = clip(&db, "aaaaaaaaaa").await.unwrap().unwrap();
        assert_eq!((c.video_key.as_str(), c.version), ("clips/x/video.mp4", 1));

        apply_replacement(&db, "aaaaaaaaaa", true, 3600.0).await.unwrap();
        let c = clip(&db, "aaaaaaaaaa").await.unwrap().unwrap();
        assert_eq!(
            (c.video_key.as_str(), c.thumb_key.as_deref(), c.size, c.duration_ms),
            ("clips/aaaaaaaaaa/v2.mp4", Some("clips/aaaaaaaaaa/v2.jpg"), 60, 12_000)
        );
        assert_eq!((c.title.as_str(), c.version), ("Ace clutch (trimmed)", 2));
        assert!(replacement(&db, "aaaaaaaaaa").await.unwrap().is_none());
        // The old files wait an hour before they go.
        assert!(due_retired(&db).await.unwrap().is_empty());
        sqlx::query("update retired_files set delete_after = now() - interval '1 second'").execute(&db).await.unwrap();
        let mut due = due_retired(&db).await.unwrap();
        due.sort();
        assert_eq!(due, ["clips/x/thumb.jpg", "clips/x/video.mp4"]);
        forget_retired(&db, "clips/x/video.mp4").await.unwrap();
        assert_eq!(due_retired(&db).await.unwrap(), ["clips/x/thumb.jpg"]);
    }

    #[sqlx::test]
    async fn replacements_that_never_finish_are_found(db: PgPool) {
        let u = someone(&db).await;
        for id in ["aaaaaaaaaa", "bbbbbbbbbb"] {
            add_clip(&db, &new_clip(id, u, 1)).await.unwrap();
            add_replacement(&db, &new_clip(id, u, 1)).await.unwrap();
        }
        sqlx::query("update replacements set created_at = now() - interval '2 days' where clip_id = 'aaaaaaaaaa'")
            .execute(&db)
            .await
            .unwrap();
        let stale: Vec<String> = stale_replacements(&db).await.unwrap().into_iter().map(|r| r.clip_id).collect();
        assert_eq!(stale, ["aaaaaaaaaa"]);
        // A clip that goes takes its replacement with it.
        delete_clip(&db, "bbbbbbbbbb").await.unwrap();
        assert!(replacement(&db, "bbbbbbbbbb").await.unwrap().is_none());
        delete_replacement(&db, "aaaaaaaaaa").await.unwrap();
        assert!(replacement(&db, "aaaaaaaaaa").await.unwrap().is_none());
    }

    #[sqlx::test]
    async fn reports_open_and_close(db: PgPool) {
        add_report(&db, "aaaaaaaaaa", "spam", "", None).await.unwrap();
        add_report(&db, "aaaaaaaaaa", "copyright", "mine", Some("me@example.com")).await.unwrap();
        add_report(&db, "bbbbbbbbbb", "other", "", None).await.unwrap();
        assert_eq!(open_reports(&db).await.unwrap().len(), 3);
        assert_eq!(resolve_reports(&db, "aaaaaaaaaa").await.unwrap(), 2);
        let open = open_reports(&db).await.unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].clip_id, "bbbbbbbbbb");
    }

    #[sqlx::test]
    async fn the_database_answers(db: PgPool) {
        ping(&db).await.unwrap();
    }

    #[sqlx::test]
    async fn totals_count_only_shared_clips(db: PgPool) {
        assert_eq!(totals(&db).await.unwrap(), Totals::default());
        let u = someone(&db).await;
        someone(&db).await;
        for (id, size) in [("aaaaaaaaaa", 100), ("bbbbbbbbbb", 200), ("cccccccccc", 400)] {
            add_clip(&db, &new_clip(id, u, size)).await.unwrap();
        }
        mark_ready(&db, "aaaaaaaaaa", true).await.unwrap();
        mark_ready(&db, "bbbbbbbbbb", false).await.unwrap();
        age(&db, "bbbbbbbbbb", "ready_at", 8).await;
        add_views(&db, &["aaaaaaaaaa".into(), "bbbbbbbbbb".into(), "cccccccccc".into()], &[3, 4, 5]).await.unwrap();
        // cccccccccc is still uploading; bbbbbbbbbb has no thumbnail.
        let t = totals(&db).await.unwrap();
        let expected = Totals {
            clips: 2,
            clips_ever: 2,
            views: 7,
            bytes: 100 + 1000 + 200,
            duration_ms: 60_000,
            this_week: 1,
            apps: 2,
            counted_since: None,
        };
        assert_eq!(t, expected);
    }

    #[sqlx::test]
    async fn hours_add_up_and_count_what_was_ever_shared(db: PgPool) {
        let u = someone(&db).await;
        add_clip(&db, &new_clip("aaaaaaaaaa", u, 100)).await.unwrap();
        mark_ready(&db, "aaaaaaaaaa", true).await.unwrap();
        add_hourly(&db, [1, 5, 3, 1, 0, 0]).await.unwrap();
        add_hourly(&db, [1, 2, 0, 0, 1, 2]).await.unwrap();
        let rows = hours(&db, 1).await.unwrap();
        assert_eq!(rows.len(), 1);
        let h = &rows[0];
        assert_eq!((h.shares, h.opens, h.plays, h.sign_ups, h.refused, h.removed), (2, 7, 3, 1, 1, 2));
        // How many are up, and how much they take, as of the last write.
        assert_eq!((h.clips, h.bytes), (1, 1100));
        assert_eq!(h.hour.minute(), 0);

        // An hour long gone isn't in the last day's, but its removed clips
        // still count as shared once.
        sqlx::query("insert into hourly (hour, removed) values (now() - interval '3 days', 4)")
            .execute(&db)
            .await
            .unwrap();
        assert_eq!(hours(&db, 1).await.unwrap().len(), 1);
        assert_eq!(hours(&db, 7).await.unwrap().len(), 2);
        let t = totals(&db).await.unwrap();
        assert_eq!((t.clips, t.clips_ever), (1, 1 + 2 + 4));
        assert!(t.counted_since.is_some());
    }

    #[sqlx::test]
    async fn top_games_need_several_apps(db: PgPool) {
        let (a, b, c) = (someone(&db).await, someone(&db).await, someone(&db).await);
        // Two apps share Deadlock; one app shares many clips of a game it named.
        let mut shares = vec![(a, Some("Deadlock")), (b, Some("Deadlock"))];
        shares.extend([(c, Some("Mine")); 5]);
        shares.extend([(a, Some("osu!")), (b, Some("osu!")), (c, Some("osu!")), (c, None)]);
        // Desktop and Other aren't games, however many share them.
        shares.extend([(a, Some("Desktop")), (b, Some("Desktop")), (c, Some("Desktop")), (a, Some("Other"))]);
        shares.extend([(b, Some("Other")), (c, Some("Other")), (a, Some("Desktop")), (b, Some("Desktop"))]);
        for (i, (uploader, game)) in shares.into_iter().enumerate() {
            let id = format!("{i:a>10}");
            add_clip(&db, &NewClip { game, ..new_clip(&id, uploader, 1) }).await.unwrap();
            mark_ready(&db, &id, false).await.unwrap();
        }
        assert_eq!(top_games(&db, 5, 2).await.unwrap(), [("osu!".into(), 3), ("Deadlock".into(), 2)]);
        assert_eq!(top_games(&db, 1, 2).await.unwrap(), [("osu!".into(), 3)]);
        assert_eq!(top_games(&db, 5, 1).await.unwrap()[0], ("Mine".into(), 5));
    }
}
