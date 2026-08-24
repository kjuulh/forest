-- Machine-to-machine OAuth: the `client_credentials` grant.
--
-- Until now an OAuth app could only act *on behalf of a user* — the
-- authorization-code flow, "Sign in with Forest". Services that need to
-- call Forest with no user in the loop (snag pulling integration data,
-- for one) had nowhere to go: Forage's HTTP surface is entirely
-- session-gated, and Forest's authorization-server RPCs are
-- service-account-only. This adds the missing grant.
--
-- ── Why a separate token table ───────────────────────────────────────
-- The obvious move is to make `oauth_access_tokens.user_id` nullable and
-- reuse it. That was rejected: every `sqlx::query!` selecting `user_id`
-- from that table would change type from `Uuid` to `Option<Uuid>`,
-- rippling through userinfo, refresh, grant listing, revocation and the
-- reaper — a lot of churn in code whose only fault is being adjacent,
-- and every one of those sites would then need a "can't happen" branch
-- for a user-less token.
--
-- A machine token is also genuinely a different animal: no user, no
-- refresh token (the client re-authenticates with its secret, which it
-- holds anyway), no consent, no id_token. Keeping the tables apart means
-- neither flow can accidentally accept the other's tokens, which is the
-- property that actually matters here.

-- Which grants an app may use. Existing apps keep exactly what they had.
ALTER TABLE oauth_apps
    ADD COLUMN IF NOT EXISTS grant_types TEXT[] NOT NULL
        DEFAULT '{authorization_code}';

-- Tokens issued to an application acting as itself.
--
-- Short-lived and re-mintable on demand, so there is deliberately no
-- refresh token: a client holding the secret can just ask again, and one
-- less long-lived credential is one less thing to leak.
CREATE TABLE IF NOT EXISTS oauth_client_tokens (
    token_hash   BYTEA PRIMARY KEY NOT NULL,
    app_id       UUID NOT NULL REFERENCES oauth_apps(id) ON DELETE CASCADE,
    scopes       TEXT[] NOT NULL DEFAULT '{}',
    expires_at   TIMESTAMPTZ NOT NULL,
    revoked_at   TIMESTAMPTZ,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at TIMESTAMPTZ
);

-- Revoking an app's machine access, and the reaper's expiry sweep.
CREATE INDEX IF NOT EXISTS idx_oauth_client_tokens_app
    ON oauth_client_tokens(app_id);
CREATE INDEX IF NOT EXISTS idx_oauth_client_tokens_expires
    ON oauth_client_tokens(expires_at);
