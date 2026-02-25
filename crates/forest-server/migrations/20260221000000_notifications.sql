-- notifications table: stores all notification events
create table notifications (
    id uuid primary key default gen_random_uuid(),
    sequence bigserial not null,
    notification_type text not null,
    title text not null,
    body text not null,
    organisation text not null,
    project text not null,
    release_context jsonb not null default '{}',
    created_at timestamptz not null default now()
);

create index idx_notifications_sequence on notifications (sequence);
create index idx_notifications_org_project on notifications (organisation, project);

-- notification preferences: per-user, per-type, per-channel
create table notification_preferences (
    id uuid primary key default gen_random_uuid(),
    user_id uuid not null,
    notification_type text not null,
    channel text not null,
    enabled boolean not null default true,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);

create unique index idx_notification_prefs_unique
    on notification_preferences (user_id, notification_type, channel);
