CREATE TABLE IF NOT EXISTS magic_link_tokens (
    token_hash TEXT PRIMARY KEY,
    email TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX idx_magic_link_tokens_email ON magic_link_tokens (email);
CREATE INDEX idx_magic_link_tokens_expires ON magic_link_tokens (expires_at);
