CREATE TABLE forest_replicas (
    name         TEXT        PRIMARY KEY,
    started_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
