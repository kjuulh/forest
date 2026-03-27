-- Component v2 registry: binary artifacts and manifests
--
-- Adds content-addressable binary storage and component manifests
-- for the v2 component system (CUE + binary plugins).

-- Component manifests store capability metadata published during `forest components publish`.
-- One manifest per component version.
CREATE TABLE component_manifests (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    component_id UUID NOT NULL REFERENCES components(id) ON DELETE CASCADE,
    version TEXT NOT NULL,
    manifest_json JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX idx_component_manifest_version
    ON component_manifests (component_id, version);

-- Component binary artifacts store per-platform binary metadata.
-- The actual binary content is stored in the content-addressable cache
-- (filesystem or object storage), keyed by sha256 hash.
CREATE TABLE component_artifacts (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    component_id UUID NOT NULL REFERENCES components(id) ON DELETE CASCADE,
    version TEXT NOT NULL,
    os TEXT NOT NULL,           -- linux, macos, windows
    arch TEXT NOT NULL,         -- amd64, arm64
    sha256 TEXT NOT NULL,       -- hex-encoded SHA-256 of the binary
    size_bytes BIGINT NOT NULL,
    storage_path TEXT,          -- optional: path in object storage (NULL = use sha256-based local cache)
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX idx_component_artifact_platform
    ON component_artifacts (component_id, version, os, arch);
CREATE INDEX idx_component_artifact_sha256
    ON component_artifacts (sha256);

-- Track the "kind" of a component version: "files" (v1) or "binary" (v2).
-- Added as a column on the existing components table.
ALTER TABLE components ADD COLUMN IF NOT EXISTS kind TEXT NOT NULL DEFAULT 'files';
