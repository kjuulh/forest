-- Release-scoped tokens for runner authentication.
-- Each token grants a runner access to the specific release data it needs.

CREATE TABLE release_tokens (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    -- SHA-256 hash of the raw token (the raw token is never stored)
    token_hash BYTEA NOT NULL UNIQUE,
    release_id UUID NOT NULL,
    release_intent_id UUID NOT NULL,
    artifact_id UUID NOT NULL,
    destination_id UUID NOT NULL,
    project_id UUID NOT NULL,
    runner_id TEXT NOT NULL,
    environment TEXT NOT NULL,
    created TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires TIMESTAMPTZ NOT NULL,
    revoked BOOLEAN NOT NULL DEFAULT false
);

CREATE INDEX idx_release_tokens_hash ON release_tokens (token_hash);
CREATE INDEX idx_release_tokens_runner ON release_tokens (runner_id) WHERE NOT revoked;
