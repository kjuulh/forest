-- Release health observations.
-- Each row represents the latest health observation for a release + destination.
-- Upserted by the health agent on each poll cycle.

CREATE TABLE release_health_observations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    release_intent_id UUID NOT NULL,
    release_id UUID NOT NULL,
    destination_name TEXT NOT NULL,
    environment TEXT NOT NULL,
    organisation TEXT NOT NULL,
    project TEXT NOT NULL,

    -- Full observation document
    observation JSONB NOT NULL,

    -- Derived aggregate status for fast querying
    status TEXT NOT NULL DEFAULT 'PROGRESSING',

    -- Human-readable message
    message TEXT NOT NULL DEFAULT '',

    observed_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Latest observation per release-intent + destination (upsert target)
CREATE UNIQUE INDEX idx_health_obs_intent_destination
    ON release_health_observations (release_intent_id, destination_name);

-- Query by intent
CREATE INDEX idx_health_obs_intent
    ON release_health_observations (release_intent_id);
