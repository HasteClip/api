-- Each part's checks, per hour: how many there were, how many passed, and
-- how long they took in all (for the average).
create table checks (
    part    text    not null,
    hour    integer not null, -- the hour's start, in seconds since 1970 (UTC)
    ok      integer not null default 0,
    total   integer not null default 0,
    ms_sum  integer not null default 0,
    primary key (part, hour)
);

-- Each part's last check, and how many in a row have failed. `server` is
-- whether HasteClip's server answers at all (for the alert to Discord).
create table state (
    part        text    primary key,
    ok          integer not null,
    ms          integer,
    checked_at  integer not null,
    fails       integer not null default 0,
    since       integer,          -- when the failing began
    alerted     integer not null default 0,
    detail      text
);

-- The last good answers from the server (/stats.json) and from Uptime Kuma
-- (incidents), kept for when they can't be reached.
create table snapshot (
    key         text    primary key,
    body        text    not null,
    fetched_at  integer not null
);
