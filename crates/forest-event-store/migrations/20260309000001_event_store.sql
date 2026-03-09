-- Generic event store schema
-- Inspired by EventStore/Kurrent: streams contain ordered events,
-- projections derive state from those events.

-- Streams table: each aggregate instance gets a stream.
-- stream_id is user-defined (e.g. "order-{uuid}", "user-{uuid}").
-- stream_category extracted for category-based subscriptions.
CREATE TABLE IF NOT EXISTS es_streams (
    stream_id       TEXT        PRIMARY KEY,
    stream_category TEXT        NOT NULL,
    stream_version  BIGINT      NOT NULL DEFAULT 0,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_es_streams_category ON es_streams (stream_category);

-- Events table: append-only log. Global ordering via BIGSERIAL.
-- stream_version is the per-stream sequence number for optimistic concurrency.
CREATE TABLE IF NOT EXISTS es_events (
    global_position BIGSERIAL   PRIMARY KEY,
    stream_id       TEXT        NOT NULL REFERENCES es_streams(stream_id),
    stream_version  BIGINT      NOT NULL,
    event_type      TEXT        NOT NULL,
    data            JSONB       NOT NULL,
    metadata        JSONB       NOT NULL DEFAULT '{}',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),

    UNIQUE (stream_id, stream_version)
);

CREATE INDEX IF NOT EXISTS idx_es_events_stream ON es_events (stream_id, stream_version);
CREATE INDEX IF NOT EXISTS idx_es_events_type ON es_events (event_type);

-- Subscriptions table: tracks consumer position for catch-up subscriptions.
CREATE TABLE IF NOT EXISTS es_subscriptions (
    subscription_id TEXT        PRIMARY KEY,
    last_position   BIGINT      NOT NULL DEFAULT 0,
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
