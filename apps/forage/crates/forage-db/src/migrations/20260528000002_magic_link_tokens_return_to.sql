-- DATA-251: carry the device-login (or other) intent through the
-- magic-link sign-in detour. Nullable — existing rows and the
-- email-verification flow leave it empty.
ALTER TABLE magic_link_tokens
    ADD COLUMN IF NOT EXISTS return_to TEXT;
