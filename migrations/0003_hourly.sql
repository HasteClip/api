-- Totals per hour, for the charts on status.hasteclip.com: how many clips
-- were shared, pages opened, videos played, apps signed up, uploads refused
-- and clips removed in that hour, and how many clips were up at its end and
-- how much they took. Only how many: nothing about anyone in particular.
create table hourly (
    hour      timestamptz primary key,
    shares    integer not null default 0,
    opens     integer not null default 0,
    plays     integer not null default 0,
    sign_ups  integer not null default 0,
    refused   integer not null default 0,
    -- Shared clips that went (unshared, taken down or expired). With the
    -- clips that are up, these make how many were ever shared, from the
    -- first hour here on.
    removed   integer not null default 0,
    clips     integer not null default 0,
    bytes     bigint  not null default 0
);
