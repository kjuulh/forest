-- apps/forest/TASKS/022-device-login.md §1.6
--
-- OAuth 2.0 device authorization grant (RFC 8628) projection.
--
-- Stores one row per device-login attempt. The aggregate stream is
-- keyed by `grant_id`; this projection adds the indexes the service
-- layer needs to look up a grant by either `device_code_hash` (CLI
-- polling) or `user_code` (forage approval page).
--
-- The raw `device_code` is never stored — only its SHA-256 hex digest,
-- so a DB compromise does not leak codes that are still valid.

CREATE TABLE device_login_grants (
    id                     UUID PRIMARY KEY,
    device_code_hash       VARCHAR(64)  NOT NULL,
    user_code              VARCHAR(32)  NOT NULL,
    client_name            TEXT         NOT NULL DEFAULT '',
    client_version         TEXT         NOT NULL DEFAULT '',
    scopes                 JSONB        NOT NULL DEFAULT '[]'::jsonb,
    status                 VARCHAR(16)  NOT NULL,
    expires_at             TIMESTAMPTZ  NOT NULL,
    interval_seconds       INTEGER      NOT NULL,
    approved_user_id       UUID                  REFERENCES users(id) ON DELETE SET NULL,
    approving_ip           TEXT,
    approving_user_agent   TEXT,
    approved_at            TIMESTAMPTZ,
    consumed_at            TIMESTAMPTZ,
    failed_lookup_count    INTEGER      NOT NULL DEFAULT 0,
    last_polled_at         TIMESTAMPTZ,
    created_at             TIMESTAMPTZ  NOT NULL DEFAULT now()
);

-- Lookup by device_code_hash during CLI polling — the hot path,
-- unique because hashes collide only on RNG failure.
CREATE UNIQUE INDEX device_login_grants_device_code_hash_idx
    ON device_login_grants (device_code_hash);

-- Lookup by user_code from the forage /device approval page.
-- Unique while a grant is non-terminal; a terminal (consumed/denied/
-- expired) user_code may be reused once the sweep deletes its row.
-- Service-layer collision check at issue time guards against
-- in-flight overlap.
CREATE UNIQUE INDEX device_login_grants_user_code_idx
    ON device_login_grants (user_code);

-- Sweep job scans grants whose expires_at has passed and that are
-- still in a non-terminal state.
CREATE INDEX device_login_grants_expires_at_idx
    ON device_login_grants (expires_at)
    WHERE status IN ('pending', 'approved');
