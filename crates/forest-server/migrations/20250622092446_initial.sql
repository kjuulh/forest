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

    project_id uuid not null,

    ref JSONB not null,
    
    created timestamptz not null default now(),
    updated timestamptz not null default now()
);
CREATE UNIQUE INDEX idx_annotations_slug ON annotations (slug);

create table projects (
    id uuid primary key default gen_random_uuid(),

    namespace TEXT not null,
    project TEXT not null,

    created timestamptz not null default now(),
    updated timestamptz not null default now()
);
CREATE INDEX idx_project_namespace ON projects (namespace);
CREATE UNIQUE INDEX idx_project_unique ON projects (namespace, project);

-- release_intents captures each individual release request (per artifact)
-- Each time a user requests a release, a new row is inserted here
-- One intent can fan out to multiple destinations via releases table
create table release_intents (
    id uuid primary key default gen_random_uuid(),
    artifact uuid not null,
    annotation_id uuid not null,
    project_id uuid not null,

    created timestamptz not null default now(),
    updated timestamptz not null default now()
);
CREATE INDEX idx_release_intent_project_id ON release_intents (project_id);
CREATE INDEX idx_release_intent_artifact ON release_intents (artifact);

-- releases tracks the current active release per project+destination
-- Points to the release_intent that is currently being rolled out
-- Each destination has its own status within the release intent
create table releases (
    id uuid primary key default gen_random_uuid(),
    release_intent_id uuid not null references release_intents(id),

    project_id uuid not null,
    destination_id uuid not null,

    status TEXT not null,

    created timestamptz not null default now(),
    updated timestamptz not null default now()
);
CREATE UNIQUE INDEX idx_release_destination_unique ON releases (project_id, destination_id);
CREATE INDEX idx_release_project_id ON releases (project_id);
CREATE INDEX idx_release_destination ON releases (destination_id);
CREATE INDEX idx_release_intent ON releases (release_intent_id);
CREATE INDEX idx_release_status ON releases (status);

create table destinations (
    id uuid primary key default gen_random_uuid(),
    name TEXT not null,
    environment TEXT not null,
    type_organisation TEXT not null,
    type_name TEXT not null,
    type_version INTEGER not null,
    metadata JSONB not null,
    created timestamptz not null default now(),
    updated timestamptz not null default now()
);
CREATE UNIQUE INDEX idx_destinations_name ON destinations (name);
CREATE INDEX idx_destinations_environment ON destinations (environment);

create table release_logs (
    id uuid primary key default gen_random_uuid(),
    release_attempt uuid not null,
    release_intent_id uuid not null references release_intents(id),
    destination_id uuid not null,
    log_lines JSONB not null,
    sequence bigserial not null,
    created timestamptz not null default now(),
    updated timestamptz not null default now()
);
CREATE INDEX idx_release_logs_intent_destination ON release_logs (release_intent_id, destination_id);
CREATE UNIQUE INDEX idx_release_logs_intent_destination_sequence ON release_logs (release_attempt, release_intent_id, destination_id, sequence);
