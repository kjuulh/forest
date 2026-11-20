CREATE TABLE IF NOT EXISTS slack_baselines (
    integration_id UUID PRIMARY KEY REFERENCES integrations(id) ON DELETE CASCADE,
    directory_mapping BOOLEAN NOT NULL DEFAULT false,
    dm_defaults JSONB NOT NULL DEFAULT '{}'::jsonb,
    updated_by TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Cached directory decisions (verified Forest email -> Slack account). Holds
-- only the Slack user id of a match, never the directory itself.
CREATE TABLE IF NOT EXISTS slack_directory_matches (
    integration_id UUID NOT NULL REFERENCES integrations(id) ON DELETE CASCADE,
    user_id TEXT NOT NULL,
    emails JSONB NOT NULL,
    status JSONB NOT NULL,
    checked_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (integration_id, user_id)
);

CREATE TABLE IF NOT EXISTS github_installations (
    installation_id BIGINT PRIMARY KEY,
    organisation TEXT NOT NULL,
    integration_id UUID NOT NULL UNIQUE REFERENCES integrations(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
