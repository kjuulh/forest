-- Organisation-owned OAuth applications ("Sign in with Forest").
--
-- An OAuth app is a credential record owned by an organisation, used to run
-- the OAuth 2.0 / OIDC authorization-code flow against Forest. Only SHA-256
-- hashes of the client_secret, authorization codes, and issued tokens are
-- stored; the plaintext is shown to the client once. This single migration
-- covers the whole feature: apps, authorization codes, access/refresh tokens,
-- and remembered consent.

-- ── Applications ─────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS oauth_apps (
    id UUID PRIMARY KEY NOT NULL,
    organisation_id UUID NOT NULL REFERENCES organisations(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    homepage_url TEXT NOT NULL DEFAULT '',
    client_id TEXT NOT NULL,
    client_secret_hash BYTEA NOT NULL,
    redirect_uris TEXT[] NOT NULL DEFAULT '{}',
    scopes TEXT[] NOT NULL DEFAULT '{}',
    created_by UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- client_id is the public identifier presented at the authorize/token
-- endpoints; it must be globally unique.
CREATE UNIQUE INDEX IF NOT EXISTS idx_oauth_apps_client_id ON oauth_apps(client_id);

-- Org settings lists an org's apps.
CREATE INDEX IF NOT EXISTS idx_oauth_apps_org ON oauth_apps(organisation_id);

-- ── Authorization codes ──────────────────────────────────────────────
-- Single-use, short-lived. `nonce` (OIDC) is echoed into the id_token at
-- exchange; PKCE challenge is verified at exchange.
CREATE TABLE IF NOT EXISTS oauth_authorization_codes (
    code_hash BYTEA PRIMARY KEY NOT NULL,
    app_id UUID NOT NULL REFERENCES oauth_apps(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    redirect_uri TEXT NOT NULL,
    scopes TEXT[] NOT NULL DEFAULT '{}',
    code_challenge TEXT,
    code_challenge_method TEXT,
    nonce TEXT,
    expires_at TIMESTAMPTZ NOT NULL,
    consumed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_oauth_codes_app ON oauth_authorization_codes(app_id);

-- ── Issued tokens ────────────────────────────────────────────────────
-- Carry the granted scopes and the consenting user. `refresh_hash` is rotated
-- on each refresh; recently-revoked rows are retained for reuse detection.
CREATE TABLE IF NOT EXISTS oauth_access_tokens (
    token_hash BYTEA PRIMARY KEY NOT NULL,
    app_id UUID NOT NULL REFERENCES oauth_apps(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    scopes TEXT[] NOT NULL DEFAULT '{}',
    refresh_hash BYTEA,
    expires_at TIMESTAMPTZ NOT NULL,
    refresh_expires_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at TIMESTAMPTZ
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_oauth_access_tokens_refresh
    ON oauth_access_tokens(refresh_hash) WHERE refresh_hash IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_oauth_access_tokens_app ON oauth_access_tokens(app_id);
CREATE INDEX IF NOT EXISTS idx_oauth_access_tokens_user ON oauth_access_tokens(user_id);

-- ── Remembered consent ───────────────────────────────────────────────
-- Which scopes a user has approved for an app, so the consent screen can be
-- skipped when a later request is already covered. One row per (app, user);
-- scopes accumulate (union) across approvals. Cleared on grant revocation.
CREATE TABLE IF NOT EXISTS oauth_consents (
    app_id UUID NOT NULL REFERENCES oauth_apps(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    scopes TEXT[] NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (app_id, user_id)
);
