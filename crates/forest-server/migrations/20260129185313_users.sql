-- Add migration script here
create table users (
    id uuid primary key not null,
    username text not null,

    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);

create unique index idx_username on users(username);


create table user_emails (
    user_id uuid not null,
    email text not null,
    verified boolean not null default false,

    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),

    PRIMARY KEY (user_id, email)
);

create unique index idx_user_emails_unique on user_emails(email);


create table identities (
    id uuid primary key not null,
    user_id uuid not null,
    provider text not null, -- native / oidc etc
    provider_user_id text not null,
    provider_email text,
    provider_data jsonb,

    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);

create index idx_identities_user_id on identities(user_id);


create table provider_native_credentials(
    id uuid not null primary key,
    user_id uuid not null,
    password_hash bytea not null,

    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);
create unique index idx_provider_native_credentials_user_id on provider_native_credentials(user_id);


create table provider_native_mfa(
    id uuid not null primary key,
    user_id uuid not null,
    type text not null,
    secret bytea not null,
    verified boolean not null default false,
    last_used_at timestamptz,

    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);
create unique index idx_provider_native_mfa_user_id on provider_native_mfa(user_id);


create table sessions (
    id uuid primary key not null,
    user_id uuid not null,
    token_hash bytea not null,
    info jsonb,
    expires_at timestamptz,
    revoked_at timestamptz,
    
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);
create index idx_sessions_user on sessions(user_id);


create table provider_oauth_states(
    id uuid primary key not null,
    provider text not null,
    state text not null,
    redirect_uri text,
    data jsonb not null,
    expires_at timestamptz,

    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);


create table personal_access_tokens(
    id uuid primary key not null,
    user_id uuid not null,
    name text not null,
    token_hash bytea not null,
    scopes jsonb not null,
    expires_at timestamptz,
    last_used timestamptz,

    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);
create index idx_personal_access_tokens_user on personal_access_tokens(user_id);
create index idx_personal_access_tokens_user_hash on personal_access_tokens(user_id, token_hash);
