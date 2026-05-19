-- Project visibility: public or private.
-- Public projects make their releases and components discoverable by anyone.
-- Private projects (default) require org membership.
ALTER TABLE projects ADD COLUMN IF NOT EXISTS visibility TEXT NOT NULL DEFAULT 'private';
CREATE INDEX idx_projects_visibility ON projects (visibility);
