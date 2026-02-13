-- Add migration script here
create table organisations (
    id uuid primary key not null,
    name text not null,

    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);

create unique index idx_organisations_name on organisations(name);

create table organisation_members (
    organisation_id uuid not null,
    user_id uuid not null,
    role text not null default 'member',

    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),

    PRIMARY KEY (organisation_id, user_id)
);

create index idx_organisation_members_user on organisation_members(user_id);
