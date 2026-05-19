-- Add plan stage support: mode column + plan_output on release_states

-- Mode distinguishes plan vs deploy execution for the same destination
ALTER TABLE release_states ADD COLUMN mode TEXT NOT NULL DEFAULT 'deploy';

-- Captured plan output (e.g. terraform plan output) for review
ALTER TABLE release_states ADD COLUMN plan_output TEXT;

-- Recreate partial unique index scoped by mode, so a plan and deploy for
-- the same project+destination can both be in-flight simultaneously.
DROP INDEX idx_release_active;
CREATE UNIQUE INDEX idx_release_active
    ON release_states (project_id, destination_id, mode)
    WHERE status IN ('ASSIGNED', 'RUNNING');
