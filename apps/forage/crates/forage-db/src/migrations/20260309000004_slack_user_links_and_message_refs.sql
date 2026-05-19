-- Slack user identity links (user-level "Sign in with Slack")
CREATE TABLE IF NOT EXISTS slack_user_links (
    id UUID PRIMARY KEY,
    user_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    team_name TEXT NOT NULL DEFAULT '',
    slack_user_id TEXT NOT NULL,
    slack_username TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (user_id, team_id)
);

CREATE INDEX idx_slack_user_links_user ON slack_user_links(user_id);
CREATE INDEX idx_slack_user_links_team_slack ON slack_user_links(team_id, slack_user_id);

-- Slack message refs for update-in-place pattern (one message per release)
CREATE TABLE IF NOT EXISTS slack_message_refs (
    id UUID PRIMARY KEY,
    integration_id UUID NOT NULL REFERENCES integrations(id) ON DELETE CASCADE,
    release_id TEXT NOT NULL,
    channel_id TEXT NOT NULL,
    message_ts TEXT NOT NULL,
    last_event_type TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (integration_id, release_id)
);

CREATE INDEX idx_slack_message_refs_lookup ON slack_message_refs(integration_id, release_id);
