-- Environments as a first-class resource, scoped to organisations
CREATE TABLE environments (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    organisation TEXT NOT NULL REFERENCES organisations(name),
    name TEXT NOT NULL,
    description TEXT,
    sort_order INTEGER NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX idx_environments_org_name ON environments(organisation, name);
CREATE INDEX idx_environments_organisation ON environments(organisation);

-- Backfill environments from existing destinations
INSERT INTO environments (organisation, name)
SELECT DISTINCT organisation, environment
FROM destinations
ON CONFLICT DO NOTHING;

-- Add environment_id to destinations, backfill, then make NOT NULL
ALTER TABLE destinations ADD COLUMN environment_id UUID REFERENCES environments(id);

UPDATE destinations d
SET environment_id = e.id
FROM environments e
WHERE e.organisation = d.organisation AND e.name = d.environment;

ALTER TABLE destinations ALTER COLUMN environment_id SET NOT NULL;
CREATE INDEX idx_destinations_environment_id ON destinations(environment_id);
