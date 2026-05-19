-- Discriminator column so the same table can hold both magic-link login
-- tokens and email-verification tokens. Default keeps existing rows
-- consistent with their pre-migration semantics.
ALTER TABLE magic_link_tokens
    ADD COLUMN IF NOT EXISTS token_type TEXT NOT NULL DEFAULT 'magic-link';

-- Replace the email-only index with a (token_type, email) index so
-- per-type rate-limit counts (count_recent) can use it directly.
DROP INDEX IF EXISTS idx_magic_link_tokens_email;
CREATE INDEX IF NOT EXISTS idx_magic_link_tokens_type_email
    ON magic_link_tokens (token_type, email);
