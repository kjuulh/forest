-- Release signals: named observations about a release on one destination,
-- pushed in by whoever is in a position to observe them.
--
-- The point of these is that a pipeline can then wait on evidence a release
-- landed rather than on `WaitStageConfig.duration_seconds`, which is a sleep.
-- See interface/proto/forest/v1/signals.proto for why they are push-based and
-- not restricted to the provider that performed the deploy.
--
-- Keyed (intent, destination, name) rather than (intent, destination) as
-- release_health_observations is: a destination reports several *different*
-- things about one release — `rollout` from the provider, `smoke` from
-- somewhere else — and they must not overwrite each other. The latest report
-- of a given name wins.

CREATE TABLE release_signals (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    release_intent_id UUID NOT NULL,
    release_id UUID NOT NULL,
    organisation TEXT NOT NULL,
    project TEXT NOT NULL,

    destination_name TEXT NOT NULL,
    environment TEXT NOT NULL,

    -- The reporter's own name for what it observed: `rollout`, `smoke`.
    name TEXT NOT NULL,

    -- One of the HealthStatus values, as a string: HEALTHY, PROGRESSING,
    -- DEGRADED, UNHEALTHY, MISSING. Reused rather than a second vocabulary
    -- minted, so a health agent and a gate mean the same thing by HEALTHY.
    status TEXT NOT NULL,

    -- What a person reads first when a gate did not open.
    detail TEXT NOT NULL DEFAULT '',

    -- Free-form: which agent or provider said so, for when a signal is wrong
    -- and someone has to work out where to go and look.
    reported_by TEXT NOT NULL DEFAULT '',

    -- When the reporter observed it, not when we stored it. A gate compares
    -- this against the stage preceding it, so a HEALTHY observed before the
    -- deploy started cannot satisfy anything.
    observed_at TIMESTAMPTZ NOT NULL,

    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- The upsert target: latest report of one name, for one destination, on one
-- release intent.
CREATE UNIQUE INDEX idx_release_signals_intent_destination_name
    ON release_signals (release_intent_id, destination_name, name);

-- How a gate reads them: everything for an intent, in one query.
CREATE INDEX idx_release_signals_intent
    ON release_signals (release_intent_id);
