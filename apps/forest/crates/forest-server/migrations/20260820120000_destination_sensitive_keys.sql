-- Metadata keys a destination declares as credentials, on top of whatever its
-- destination type marks sensitive in its field schema.
--
-- This exists because free-form keys need to be markable too: the terraform
-- destination forwards every key it does not declare as a TF_VAR_*, so real
-- credentials (aws_secret_access_key, cloudflare_token, …) live outside any
-- type schema and could not otherwise be flagged.
--
-- Additive and backward compatible: existing rows default to '[]', i.e. no
-- destination-declared sensitive keys, which is exactly the prior behaviour.
ALTER TABLE destinations
    ADD COLUMN IF NOT EXISTS sensitive_keys jsonb NOT NULL DEFAULT '[]'::jsonb;
