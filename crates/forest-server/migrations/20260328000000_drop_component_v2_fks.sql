-- Drop FK constraints on component_artifacts and component_manifests.
-- These tables are populated during the upload phase (before commit),
-- when the component_id references component_staging, not components.
-- The component_id becomes a valid components.id only after commit_upload.
ALTER TABLE component_artifacts DROP CONSTRAINT IF EXISTS component_artifacts_component_id_fkey;
ALTER TABLE component_manifests DROP CONSTRAINT IF EXISTS component_manifests_component_id_fkey;
