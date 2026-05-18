-- Component shape taxonomy (TASKS/018-global-tools.md §1a.2e).
--
-- Adds a `shape` column to `components` that is computed from the manifest's
-- (kind, tool, methods) at publish time and persisted in the same transaction.
-- The four shapes are:
--   - 'component'         — binary + methods, no tool facet (status quo today)
--   - 'hybrid_component'  — binary + methods + tool
--   - 'tool_binary'       — binary + tool, no methods
--   - 'tool_external'     — external manifest (URL-hosted) + tool
ALTER TABLE components
    ADD COLUMN IF NOT EXISTS shape TEXT NOT NULL DEFAULT 'component'
        CHECK (shape IN ('component', 'hybrid_component', 'tool_binary', 'tool_external'));

-- The `kind` column already exists; widen its allowed values to include 'external'
-- (no CHECK constraint to alter, it's a free-form TEXT today).
COMMENT ON COLUMN components.kind IS
    'one of: binary (uploaded), external (URL-hosted), files (v1 cue-only)';

CREATE INDEX IF NOT EXISTS idx_components_shape ON components (shape);

-- `component_staging.shape` is set by `publish_manifest` after the manifest
-- has been validated; `commit_upload` then promotes it onto `components`.
ALTER TABLE component_staging
    ADD COLUMN IF NOT EXISTS shape TEXT;

