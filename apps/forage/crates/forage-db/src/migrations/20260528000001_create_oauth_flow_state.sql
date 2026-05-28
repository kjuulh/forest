-- Per-flow state for OAuth and MFA redirects. Keyed by (provider, state)
-- so the same opaque state token cannot be redeemed across providers
-- (defence in depth). See forage_core::auth::oauth_state for why this is
-- a server-side store rather than a cookie.
CREATE TABLE IF NOT EXISTS oauth_flow_state (
    provider TEXT NOT NULL,
    state TEXT NOT NULL,
    return_to TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (provider, state)
);

CREATE INDEX IF NOT EXISTS idx_oauth_flow_state_expires
    ON oauth_flow_state (expires_at);
