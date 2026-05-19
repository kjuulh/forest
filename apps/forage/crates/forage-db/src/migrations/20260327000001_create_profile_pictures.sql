CREATE TABLE profile_pictures (
    id UUID PRIMARY KEY,
    user_id TEXT NOT NULL UNIQUE,
    content_type TEXT NOT NULL,
    data BYTEA NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
