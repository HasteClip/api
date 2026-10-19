-- Who shares clips: a HasteClip app on someone's computer, known only by a
-- random token it keeps. We store the token's SHA-256, never the token.
create table uploaders (
    id          bigint generated always as identity primary key,
    token_hash  bytea not null unique,
    created_at  timestamptz not null default now(),
    banned_at   timestamptz
);

-- A shared clip. Its files are in the bucket at video_key and thumb_key;
-- the keys are stored (not worked out from the id) so files can move.
create table clips (
    id              text primary key,
    uploader_id     bigint not null references uploaders (id),
    state           text not null default 'uploading' check (state in ('uploading', 'ready')),
    video_key       text not null,
    thumb_key       text,
    -- The bucket's multipart upload of the video, while it's uploading.
    upload_id       text,
    size            bigint not null,
    thumb_size      integer,
    duration_ms     integer not null,
    width           integer not null,
    height          integer not null,
    fps             real,
    title           text not null,
    game            text,
    views           bigint not null default 0,
    created_at      timestamptz not null default now(),
    ready_at        timestamptz,
    -- When someone last opened its page or played it.
    last_viewed_at  timestamptz
);

create index clips_by_uploader on clips (uploader_id);
create index clips_by_state on clips (state, created_at);

-- Someone reporting a clip from its page. No foreign key: a report stays
-- after the clip is removed.
create table reports (
    id           bigint generated always as identity primary key,
    clip_id      text not null,
    reason       text not null,
    details      text not null default '',
    email        text,
    created_at   timestamptz not null default now(),
    resolved_at  timestamptz
);

create index reports_open on reports (created_at) where resolved_at is null;
