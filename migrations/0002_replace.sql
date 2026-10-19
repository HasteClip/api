-- A shared clip can be replaced by a new version (the clip was trimmed or
-- edited in the app): its link stays, and plays the new one once it's
-- whole. `version` goes up with each, and pages link to their files by it,
-- so nothing that cached a link to the old files is sent to them.
alter table clips add column version integer not null default 1;

-- A new version being uploaded, at most one per clip.
create table replacements (
    clip_id      text primary key references clips (id) on delete cascade,
    video_key    text not null,
    thumb_key    text,
    upload_id    text not null,
    size         bigint not null,
    thumb_size   integer,
    duration_ms  integer not null,
    width        integer not null,
    height       integer not null,
    fps          real,
    title        text not null,
    game         text,
    created_at   timestamptz not null default now()
);

-- Files of a version that was replaced. They're deleted a while later, not
-- at once: links to them that were handed out are cached for up to an hour,
-- and someone may be watching.
create table retired_files (
    key           text primary key,
    delete_after  timestamptz not null
);

create index retired_files_due on retired_files (delete_after);
