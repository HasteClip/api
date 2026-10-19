# HasteClip API

Share links for [HasteClip](https://hasteclip.com) clips. In the app, **Share → Copy link** uploads a clip and copies a link like `https://clips.hasteclip.com/aB3xY9k2Qp`. The link opens a page with the clip, and plays the clip right in Discord and other chats.

One Rust server ([axum](https://github.com/tokio-rs/axum)) does both jobs: it serves the pages at `clips.hasteclip.com` and the JSON API the app calls at `api.hasteclip.com`. Clips are stored in a private S3 bucket (Hetzner Object Storage) and everything else in Postgres.

## How sharing works

1. Once per app, `POST /v1/uploaders` returns a token (send it as `Authorization: Bearer …`). There's no account: the token *is* the app's identity. We store only its SHA-256.
2. `POST /v1/clips` with the clip's size and details (`size`, `duration_ms`, `width`, `height`, `fps`, `title`, `game`, `thumb_size`). It returns the clip's link, plus links to upload the video in parts and the thumbnail.
3. The app uploads straight to the bucket, so the bytes never pass through this server. Every part is `part_size` bytes except the last one, and each upload link is signed for its exact size and type. The app keeps each part's `ETag`.
4. `POST /v1/clips/{id}/complete` with `{"etags": [...]}` in part order. The server joins the parts and checks the file is the right size and is an MP4. From then on the link works.

`GET /v1/clips` lists the app's shared clips. `DELETE /v1/clips/{id}` unshares one and deletes its files.

The bucket stays private. `clips.hasteclip.com/<id>.mp4` (and `.jpg`) redirects to a bucket link that expires within two hours. That's why unsharing is final, and why storage can move without breaking links people have already posted.

## Privacy

Nothing is stored about who watches or shares beyond:

- a view count per clip
- when the clip was last watched
- the hash of each app's token

Rate limits key on IP addresses in memory only, and those addresses are never written down or logged. The pages have no scripts, cookies or analytics.

## Settings

Everything comes from environment variables:

| Variable | Default | |
|---|---|---|
| `DATABASE_URL` | (required) | Postgres |
| `S3_BUCKET`, `S3_ACCESS_KEY`, `S3_SECRET_KEY` | (required) | The bucket, which must already exist. On start, the server sets its rule that unfinished uploads are thrown away after a day |
| `S3_REGION` | `fsn1` | |
| `S3_ENDPOINT` | `https://<region>.your-objectstorage.com` | |
| `SHARE_URL` | `https://clips.hasteclip.com` | Where links point |
| `MAX_CLIP_MB` | `1024` | Largest clip |
| `MAX_UPLOADER_GB` | `10` | Most one app can have shared at once |
| `MAX_CLIPS_PER_DAY` | `100` | Per app |
| `RETENTION_DAYS` | `90` | Remove clips nobody has opened or played in this long (`0`: keep them until unshared) |
| `REPORT_WEBHOOK` | | A Discord webhook that's told about every report |
| `PORT` | `8080` | |

## Running it

```sh
export DATABASE_URL=postgres://… S3_BUCKET=hasteclip-clips-dev S3_ACCESS_KEY=… S3_SECRET_KEY=…
# Once: make the bucket (or make it in Hetzner's Console)
curl -X PUT --aws-sigv4 aws:amz:fsn1:s3 --user "$S3_ACCESS_KEY:$S3_SECRET_KEY" https://$S3_BUCKET.fsn1.your-objectstorage.com/
cargo run
```

Migrations (`migrations/`) are built into the binary and run when it starts.

Tests need a Postgres they can create databases on, because each test gets a fresh one: `DATABASE_URL=postgres://postgres:postgres@localhost/postgres cargo test`.

## Reports

Every share page has a **Report** link. Reports are saved in the database and, if `REPORT_WEBHOOK` is set, posted to Discord. To deal with them, open a terminal in the container (Coolify → the app → Terminal):

```sh
hasteclip-api reports          # what's open
hasteclip-api remove <clip>    # take a clip down and close its reports
hasteclip-api ban <clip>       # take down everything its uploader shared and stop them sharing more
hasteclip-api dismiss <clip>   # close a clip's reports and keep it up
```

## Deploying

Every push to `main` runs the tests. If they pass, it builds the image, publishes it as `ghcr.io/hasteclip/api`, and tells Coolify to deploy it (see `.github/workflows/ci.yml`).

## License

[AGPL-3.0-or-later](LICENSE). If you run a changed copy of this server for others, you have to share its source. The HasteClip name and icon aren't covered by the license; see the app's [TRADEMARKS.md](https://github.com/hasteclip/hasteclip/blob/main/TRADEMARKS.md). Hanken Grotesk (`static/`) is under the SIL Open Font License.
