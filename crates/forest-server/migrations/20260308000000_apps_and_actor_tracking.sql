-- Apps: org-scoped integrations (like GitHub Apps)
CREATE TABLE apps (
    id UUID PRIMARY KEY,
    organisation_id UUID NOT NULL REFERENCES organisations(id),
    name TEXT NOT NULL,
    description TEXT,
    permissions JSONB NOT NULL DEFAULT '[]'::jsonb,
    created_by UUID NOT NULL REFERENCES users(id),
    suspended BOOLEAN NOT NULL DEFAULT false,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX idx_apps_org_name ON apps(organisation_id, name);

-- Tokens that apps use to authenticate
CREATE TABLE app_tokens (
    id UUID PRIMARY KEY,
    app_id UUID NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    token_hash BYTEA NOT NULL UNIQUE,
    expires_at TIMESTAMPTZ,
    last_used TIMESTAMPTZ,
    revoked BOOLEAN NOT NULL DEFAULT false,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_app_tokens_app ON app_tokens(app_id);

-- Track who performed actions (user or app)
ALTER TABLE artifact_staging ADD COLUMN actor_id UUID;
ALTER TABLE artifact_staging ADD COLUMN actor_type TEXT;

ALTER TABLE annotations ADD COLUMN actor_id UUID;
ALTER TABLE annotations ADD COLUMN actor_type TEXT;

ALTER TABLE release_intents ADD COLUMN actor_id UUID;
ALTER TABLE release_intents ADD COLUMN actor_type TEXT;
