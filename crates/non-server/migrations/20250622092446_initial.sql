-- Add migration script here

create table components (
    id uuid primary key default gen_random_uuid(),
    name string not null,
    namespace string not null,
    version string not null,

    created timestamptz not null default now(),
    updated timestamptz not null default now()
);

CREATE UNIQUE INDEX idx_component_unique_version ON components (name, namespace, version);

create table component_staging (
    id uuid primary key default gen_random_uuid(),
    name string not null,
    namespace string not null,
    version string not null,

    status string not null,

    created timestamptz not null default now(),
    updated timestamptz not null default now()
);
CREATE UNIQUE INDEX idx_component_staging_unique_version ON component_staging (name, namespace, version);

create table component_files (
    id uuid primary key default gen_random_uuid(),
    component_id uuid not null,

    file_path string not null,
    file_content bytes not null,

    created timestamptz not null default now(),
    updated timestamptz not null default now()
);
