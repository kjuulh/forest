-- Add migration script here

create table components (
    id uuid primary key default gen_random_uuid(),
    name text not null,
    namespace text not null,
    version text not null,

    created timestamptz not null default now(),
    updated timestamptz not null default now()
);

CREATE UNIQUE INDEX idx_component_unique_version ON components (name, namespace, version);

create table component_staging (
    id uuid primary key default gen_random_uuid(),
    name text not null,
    namespace text not null,
    version text not null,

    status text not null,

    created timestamptz not null default now(),
    updated timestamptz not null default now()
);
CREATE UNIQUE INDEX idx_component_staging_unique_version ON component_staging (name, namespace, version);

create table component_files (
    id uuid primary key default gen_random_uuid(),
    component_id uuid not null,

    file_path text not null,
    file_content bytea not null,

    created timestamptz not null default now(),
    updated timestamptz not null default now()
);

create table artifact_staging (
    id uuid primary key default gen_random_uuid(),

    artifact_id uuid not null,

    created timestamptz not null default now(),
    updated timestamptz not null default now()
);

create table artifact_files (
    id uuid primary key default gen_random_uuid(),

    artifact_staging_id uuid not null,

    env text not null,
    destination text not null,

    file_name text not null,
    file_content uuid not null, -- blob storage id
    
    created timestamptz not null default now(),
    updated timestamptz not null default now()
);

create table blob_storage (
    id uuid primary key default gen_random_uuid(),

    content text,

    created timestamptz not null default now(),
    updated timestamptz not null default now()
);

create table artifacts (
    id uuid primary key default gen_random_uuid(),
    artifact_id uuid not null,
    created timestamptz not null default now(),
    updated timestamptz not null default now()
);

create table annotations (
    id uuid primary key default gen_random_uuid(),
    slug TEXT not null,

    artifact_id uuid not null,
    metadata JSONB not null,
    source JSONB not null,
    context JSONB not null,

    namespace TEXT not null,
    project TEXT not null,

    ref JSONB not null,
    
    created timestamptz not null default now(),
    updated timestamptz not null default now()
);

CREATE UNIQUE INDEX idx_annotations_slug ON annotations (slug);

create table releases (
    id uuid primary key default gen_random_uuid(),
    artifact uuid not null,
    annotation_id uuid not null,
    destination TEXT not null,

    status TEXT not null,

    created timestamptz not null default now(),
    updated timestamptz not null default now()
);
