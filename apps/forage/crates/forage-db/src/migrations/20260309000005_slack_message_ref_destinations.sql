-- Add per-destination status tracking and release title to slack_message_refs
ALTER TABLE slack_message_refs ADD COLUMN IF NOT EXISTS destinations JSONB NOT NULL DEFAULT '{}';
ALTER TABLE slack_message_refs ADD COLUMN IF NOT EXISTS release_title TEXT NOT NULL DEFAULT '';
