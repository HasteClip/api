//! The bucket (S3-compatible: Hetzner Object Storage) where clips' files are.
//!
//! The bucket is private: nothing in it can be read without a signature.
//! Uploads go straight from the app to the bucket with links this signs
//! (size and type are part of the signature, so they can't be changed), and
//! watching goes through `clips.hasteclip.com/<id>.mp4`, which answers with a
//! link to the file that expires. That's what makes Unshare final, lets a
//! clip become private later without moving it, and lets the files move to
//! other storage without breaking a link anyone has posted.

use std::time::Duration;

use base64::Engine;
use md5::{Digest, Md5};
use rusty_s3::actions::{
    AbortMultipartUpload, CompleteMultipartUpload, CreateBucket, CreateMultipartUpload, DeleteObject, GetObject,
    HeadBucket, HeadObject, PutObject, UploadPart,
};
use rusty_s3::{Bucket, Credentials, S3Action, UrlStyle};
use url::Url;

use crate::config;

/// How long the links the app uploads with stay good: long enough for a big
/// clip on a slow connection.
pub const UPLOAD_LINK_LIFE: Duration = Duration::from_secs(24 * 60 * 60);
/// Links to watch a file are signed for the hour they're asked in (so the
/// same link comes back all hour, and browsers and Discord can cache it) and
/// stay good for an hour after it.
const WATCH_WINDOW: i64 = 60 * 60;
/// How long the server's own requests are signed for.
const REQUEST_LIFE: Duration = Duration::from_secs(60);
/// How long a browser may keep a file once it has it (the link itself
/// expires, the file doesn't change).
const FILE_CACHE: &str = "public, max-age=86400";
/// Parts a video is uploaded in. S3 wants at least 5 MiB (except the last)
/// and at most 10 000 parts.
const PART_SIZE: u64 = 8 * 1024 * 1024;
const MAX_PARTS: u64 = 10_000;

pub struct Storage {
    bucket: Bucket,
    credentials: Credentials,
    http: reqwest::Client,
}

pub type Result<T> = std::result::Result<T, String>;

impl Storage {
    pub fn new(cfg: &config::S3) -> Result<Self> {
        let bucket = Bucket::new(cfg.endpoint.clone(), UrlStyle::VirtualHost, cfg.bucket.clone(), cfg.region.clone())
            .map_err(|e| format!("bucket: {e}"))?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| format!("http client: {e}"))?;
        Ok(Self { bucket, credentials: Credentials::new(cfg.access_key.clone(), cfg.secret_key.clone()), http })
    }

    /// Where the bucket's files are served from (`https://<bucket>.<endpoint>`),
    /// for the share page's Content-Security-Policy.
    pub fn origin(&self) -> String {
        self.bucket.base_url().origin().ascii_serialization()
    }

    /// Check the bucket is there, and set its one rule: uploads that never
    /// finished are thrown away after a day. The bucket isn't made here:
    /// Hetzner doesn't make buckets for a signed link, only for a signed
    /// request.
    pub async fn prepare(&self) -> Result<()> {
        let head = HeadBucket::new(&self.bucket, Some(&self.credentials)).sign(REQUEST_LIFE);
        let status = self.send(self.http.head(head)).await?.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(format!("there's no bucket called {}; make it first", self.bucket.name()));
        } else if !status.is_success() {
            return Err(format!("the bucket can't be reached: {status}"));
        }

        let rules = r#"<LifecycleConfiguration xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><Rule><ID>abort-unfinished-uploads</ID><Filter><Prefix></Prefix></Filter><Status>Enabled</Status><AbortIncompleteMultipartUpload><DaysAfterInitiation>1</DaysAfterInitiation></AbortIncompleteMultipartUpload></Rule></LifecycleConfiguration>"#;
        let md5 = base64::engine::general_purpose::STANDARD.encode(Md5::digest(rules.as_bytes()));
        // There's no lifecycle action in rusty-s3; CreateBucket is the same
        // request (PUT on the bucket) without the `?lifecycle`.
        let mut put = CreateBucket::new(&self.bucket, &self.credentials);
        put.query_mut().insert("lifecycle", "");
        put.headers_mut().insert("content-md5", md5.clone());
        let url = put.sign(REQUEST_LIFE);
        let req = self.http.put(url).header("content-md5", md5).body(rules);
        check(self.send(req).await?, "set the bucket's lifecycle rule").await.map(drop)
    }

    /// Start uploading a video in parts; returns the upload's id.
    pub async fn start_upload(&self, key: &str) -> Result<String> {
        let mut action = CreateMultipartUpload::new(&self.bucket, Some(&self.credentials), key);
        action.headers_mut().insert("content-type", "video/mp4");
        let url = action.sign(REQUEST_LIFE);
        let res = check(self.send(self.http.post(url).header("content-type", "video/mp4")).await?, "start the upload")
            .await?;
        let body = res.text().await.map_err(|e| format!("start the upload: {e}"))?;
        let parsed = CreateMultipartUpload::parse_response(&body).map_err(|e| format!("start the upload: {e}"))?;
        Ok(parsed.upload_id().to_owned())
    }

    /// Links to upload each part of a video, each signed for its exact size.
    pub fn part_links(&self, key: &str, upload_id: &str, size: u64) -> Vec<Url> {
        part_sizes(size)
            .iter()
            .enumerate()
            .map(|(i, len)| {
                let n = u16::try_from(i + 1).expect("at most 10 000 parts");
                let mut action = UploadPart::new(&self.bucket, Some(&self.credentials), key, n, upload_id);
                action.headers_mut().insert("content-length", len.to_string());
                action.sign(UPLOAD_LINK_LIFE)
            })
            .collect()
    }

    /// A link to upload a JPEG of exactly `size` bytes to `key`.
    pub fn jpeg_link(&self, key: &str, size: u64) -> Url {
        let mut action = PutObject::new(&self.bucket, Some(&self.credentials), key);
        action.headers_mut().insert("content-type", "image/jpeg");
        action.headers_mut().insert("content-length", size.to_string());
        action.sign(UPLOAD_LINK_LIFE)
    }

    /// Put the uploaded parts together into the video (`etags` in part order,
    /// as the bucket answered each part's upload).
    pub async fn finish_upload(&self, key: &str, upload_id: &str, etags: &[String]) -> Result<()> {
        let action = CompleteMultipartUpload::new(
            &self.bucket,
            Some(&self.credentials),
            key,
            upload_id,
            etags.iter().map(String::as_str),
        );
        let url = action.sign(REQUEST_LIFE);
        let res = check(self.send(self.http.post(url).body(action.body())).await?, "finish the upload").await?;
        // This one can fail with 200 OK and the error in the body.
        let body = res.text().await.map_err(|e| format!("finish the upload: {e}"))?;
        if body.contains("<Error>") {
            return Err(format!("finish the upload: {body}"));
        }
        Ok(())
    }

    /// Throw away an upload that didn't finish (and its parts).
    pub async fn abort_upload(&self, key: &str, upload_id: &str) -> Result<()> {
        let url = AbortMultipartUpload::new(&self.bucket, Some(&self.credentials), key, upload_id).sign(REQUEST_LIFE);
        let res = self.send(self.http.delete(url)).await?;
        if res.status().is_success() || res.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(());
        }
        check(res, "abort the upload").await.map(drop)
    }

    /// The size of the file at `key`, or `None` when there isn't one.
    pub async fn size(&self, key: &str) -> Result<Option<u64>> {
        let url = HeadObject::new(&self.bucket, Some(&self.credentials), key).sign(REQUEST_LIFE);
        let res = self.send(self.http.head(url)).await?;
        if res.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let res = check(res, "look at the file").await?;
        let len = res.headers().get(reqwest::header::CONTENT_LENGTH).and_then(|v| v.to_str().ok()?.parse().ok());
        len.map(Some).ok_or_else(|| "look at the file: no size".into())
    }

    /// The first `n` bytes of the file at `key`.
    pub async fn head_bytes(&self, key: &str, n: u64) -> Result<Vec<u8>> {
        let url = GetObject::new(&self.bucket, Some(&self.credentials), key).sign(REQUEST_LIFE);
        let req = self.http.get(url).header(reqwest::header::RANGE, format!("bytes=0-{}", n - 1));
        let res = check(self.send(req).await?, "read the file").await?;
        Ok(res.bytes().await.map_err(|e| format!("read the file: {e}"))?.to_vec())
    }

    /// Delete the file at `key` (nothing to do when it isn't there).
    pub async fn delete(&self, key: &str) -> Result<()> {
        let url = DeleteObject::new(&self.bucket, Some(&self.credentials), key).sign(REQUEST_LIFE);
        let res = self.send(self.http.delete(url)).await?;
        if res.status().is_success() || res.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(());
        }
        check(res, "delete the file").await.map(drop)
    }

    /// A link to watch or download the file at `key`, good for at least an
    /// hour, and the same link all hour.
    pub fn watch_link(&self, key: &str, content_type: &str) -> Url {
        let now = jiff::Timestamp::now().as_second();
        let window = jiff::Timestamp::from_second(now - now.rem_euclid(WATCH_WINDOW)).expect("a time in range");
        let mut action = GetObject::new(&self.bucket, Some(&self.credentials), key);
        action.query_mut().insert("response-content-type", content_type.to_owned());
        action.query_mut().insert("response-cache-control", FILE_CACHE);
        action.sign_with_time(Duration::from_secs(2 * WATCH_WINDOW as u64), &window)
    }

    async fn send(&self, req: reqwest::RequestBuilder) -> Result<reqwest::Response> {
        req.send().await.map_err(|e| format!("the bucket: {e}"))
    }
}

/// How long a cached answer to "where's this file?" can be reused: a little
/// less than the shortest time a watch link has left.
pub const WATCH_LINK_CACHE: u64 = WATCH_WINDOW as u64 - 10 * 60;

/// The sizes of the parts a video of `size` bytes is uploaded in.
pub fn part_sizes(size: u64) -> Vec<u64> {
    let part = PART_SIZE.max(size.div_ceil(MAX_PARTS));
    let mut parts = vec![part; (size / part) as usize];
    if !size.is_multiple_of(part) || size == 0 {
        parts.push(size % part);
    }
    parts
}

async fn check(res: reqwest::Response, what: &str) -> Result<reqwest::Response> {
    if res.status().is_success() {
        return Ok(res);
    }
    let status = res.status();
    let body = res.text().await.unwrap_or_default();
    Err(format!("{what}: {status} {}", body.chars().take(500).collect::<String>()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parts_add_up() {
        for size in [1, PART_SIZE - 1, PART_SIZE, PART_SIZE + 1, 30_000_000, 1 << 30, 200 << 30] {
            let parts = part_sizes(size);
            assert_eq!(parts.iter().sum::<u64>(), size, "{size}");
            assert!(parts.len() as u64 <= MAX_PARTS, "{size}");
            assert!(parts.iter().all(|&p| p > 0), "{size}");
            assert!(parts[..parts.len() - 1].iter().all(|&p| p >= 5 * 1024 * 1024), "{size}");
        }
        assert_eq!(part_sizes(PART_SIZE), vec![PART_SIZE]);
        assert_eq!(part_sizes(PART_SIZE + 1), vec![PART_SIZE, 1]);
    }
}
