ALTER TABLE slack_message_refs ADD COLUMN IF NOT EXISTS thread_keys JSONB NOT NULL DEFAULT '[]';
ALTER TABLE slack_message_refs ADD COLUMN IF NOT EXISTS rendered_hash TEXT NOT NULL DEFAULT '';
ALTER TABLE slack_message_refs ADD COLUMN IF NOT EXISTS destination_count INT NOT NULL DEFAULT 0;

CREATE TABLE IF NOT EXISTS slack_emoji_sets (
    organisation TEXT PRIMARY KEY,
    emoji JSONB NOT NULL,
    updated_by TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
