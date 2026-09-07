-- Prepare Forest's existing event-store tables for mire 0.3.
--
-- This migration is deliberately additive. The legacy forest-event-store can
-- continue to read and append after it runs: transaction_id has a database
-- default, while the columns the legacy writer does not know about remain
-- nullable. Mixed legacy/mire application replicas are still unsupported.
--
-- The CREATE statements also make this safe on a fresh database. Historically
-- forest-event-store created these tables after forest-server's SQLx migrations.

CREATE TABLE IF NOT EXISTS es_streams (
    stream_id        TEXT        PRIMARY KEY,
    stream_category  TEXT        NOT NULL,
    stream_version   BIGINT      NOT NULL DEFAULT 0,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_es_streams_category
    ON es_streams (stream_category);

CREATE TABLE IF NOT EXISTS es_events (
    global_position  BIGSERIAL   PRIMARY KEY,
    stream_id        TEXT        NOT NULL REFERENCES es_streams(stream_id),
    stream_version   BIGINT      NOT NULL,
    event_type       TEXT        NOT NULL,
    data             JSONB       NOT NULL,
    metadata         JSONB       NOT NULL DEFAULT '{}',
    transaction_id   XID8        NOT NULL DEFAULT pg_current_xact_id(),
    stream_category  TEXT,
    payload_size     BIGINT,
    metadata_size    BIGINT,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (stream_id, stream_version)
);

CREATE TABLE IF NOT EXISTS es_subscriptions (
    subscription_id      TEXT        PRIMARY KEY,
    last_position        BIGINT      NOT NULL DEFAULT 0,
    last_transaction_id  XID8        NOT NULL DEFAULT '0'::xid8,
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Mire's initial migration uses transaction_id immediately when it creates its
-- cursor index. CREATE TABLE IF NOT EXISTS cannot add this column to Forest's
-- pre-existing table, so it must exist before EventStore::migrate is called.
ALTER TABLE es_events
    ADD COLUMN IF NOT EXISTS transaction_id XID8;

UPDATE es_events
SET transaction_id = pg_current_xact_id()
WHERE transaction_id IS NULL;

ALTER TABLE es_events
    ALTER COLUMN transaction_id SET DEFAULT pg_current_xact_id(),
    ALTER COLUMN transaction_id SET NOT NULL;

-- Map a legacy global-position checkpoint to Mire's compound
-- (transaction_id, global_position) cursor. All events at or before the old
-- checkpoint remain acknowledged; later events remain visible.
ALTER TABLE es_subscriptions
    ADD COLUMN IF NOT EXISTS last_transaction_id XID8;

UPDATE es_subscriptions AS subscription
SET last_transaction_id = COALESCE(
    (
        SELECT event.transaction_id
        FROM es_events AS event
        WHERE event.global_position <= subscription.last_position
        ORDER BY event.global_position DESC
        LIMIT 1
    ),
    '0'::xid8
)
WHERE subscription.last_transaction_id IS NULL;

ALTER TABLE es_subscriptions
    ALTER COLUMN last_transaction_id SET DEFAULT '0'::xid8,
    ALTER COLUMN last_transaction_id SET NOT NULL;

-- Mire category subscriptions read the denormalized event category. The legacy
-- writer does not populate it, so the final cutover audit/backfill must run
-- after all legacy writers have stopped as well.
ALTER TABLE es_events
    ADD COLUMN IF NOT EXISTS stream_category TEXT;

UPDATE es_events AS event
SET stream_category = stream.stream_category
FROM es_streams AS stream
WHERE event.stream_id = stream.stream_id
  AND event.stream_category IS NULL;

-- Stored sizes let Mire enforce byte budgets without repeatedly serializing
-- historical JSON. They intentionally remain nullable for compatibility with
-- the legacy writer; Mire uses COALESCE for old/null rows.
ALTER TABLE es_events
    ADD COLUMN IF NOT EXISTS payload_size BIGINT,
    ADD COLUMN IF NOT EXISTS metadata_size BIGINT;

UPDATE es_events
SET payload_size = COALESCE(payload_size, octet_length(data::text)),
    metadata_size = COALESCE(metadata_size, octet_length(metadata::text))
WHERE payload_size IS NULL
   OR metadata_size IS NULL;

CREATE INDEX IF NOT EXISTS idx_es_events_stream
    ON es_events (stream_id, stream_version);

CREATE INDEX IF NOT EXISTS idx_es_events_type
    ON es_events (event_type);

CREATE INDEX IF NOT EXISTS idx_es_events_txid
    ON es_events (transaction_id, global_position);

CREATE INDEX IF NOT EXISTS idx_es_events_category_cursor
    ON es_events (stream_category, transaction_id, global_position);
